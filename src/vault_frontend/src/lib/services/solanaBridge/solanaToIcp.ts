/**
 * Solana -> ICP: the holder returns icUSD from a Solana wallet and receives icUSD on the Internet Computer.
 *
 * Two layers, like the Base flows: a pure state machine (`reduceSolReturn`, `evaluateReturnDraft`) and an effect
 * runner (`prepareReturn`, `startReturn`, `rebroadcastReturn`, `reconcileReturn`, `trackReturn`, ...) that talks to the
 * wallet, a keyless Solana adapter and the bridge canister through injected dependencies.
 *
 * What the holder signs is exactly the backend's reference return (`transaction.ts`): a legacy transaction with a
 * recent blockhash, one TransferChecked from the holder's token account to the canonical return ATA, and the
 * `rumi1|r|...` memo. The exact destination account, mint, token program, amount and memo are shown before the
 * wallet is asked.
 *
 * Safety properties:
 *  - The attempt is persisted BEFORE each effect: first the intent with its exact unsigned message (`prepared`),
 *    then `signing` right before the wallet is asked, then the signed bytes and their derived first signature
 *    (`signed`) BEFORE anything is broadcast, then `sent`. The wallet call is the first asynchronous step of the click.
 *  - A wallet that returns signed bytes is checked: the bytes must be our message, signed once. A wallet that
 *    changed the transaction gets nothing broadcast (the bridge would read it as a stray, not a return).
 *  - A rejection is clean (nothing was signed): the attempt is forgotten. For a wallet that signs AND sends, only the
 *    explicit user-rejection code counts (`isExplicitUserRejection`); message text such as "closed" or "cancelled" is
 *    not proof, because a wallet can broadcast and then lose its connection. Any other sign-and-send error, and any
 *    AMBIGUOUS result, holds the ORIGINAL intent. It is never rebuilt, never signed again and never replaced
 *    automatically; the holder resolves it with evidence (the saved signature, a search for our exact message in the
 *    wallet's recent transactions, the bridge's own records).
 *  - ABSENCE IS NOT PROOF. The search reads only the latest wallet signatures and a node answers null for anything
 *    pruned or not yet indexed, so "not found" stays unknown even after the blockhash expired. Only a FINALIZED failure
 *    of the exact saved attempt lets it be forgotten (a failure seen at processed or confirmed can still be replaced
 *    by a successful finalized execution of the same signature).
 *  - A rebroadcast sends the SAME saved bytes (the same signature), so it cannot pay twice.
 *  - A wallet callback or a signature is a hint. Only the bridge canister's release record settles a return.
 *  - A wallet is asked only after `assertCanSign` agrees (a disabled destination never reaches it), and only while the
 *    wallet account and network are the ones the attempt was prepared for.
 */

import type { ReleaseView, ReturnView, Result } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { formatE8s, tryParseE8s } from '../baseBridge/amounts';
import { newClientRequestId, type RandomBytes, defaultRandomBytes } from '../baseBridge/intents';
import { intentBindingMessage, signingBlockMessage, type SigningContext } from '../baseBridge/signingGuard';
import { abortableSleep, pollWithBackoff, type Sleep } from '../baseBridge/status';
import { base58Decode, base58Encode, fromHex, pubkeyFromBase58, signatureFromBase58, toHex, type Bytes, bytesEqual } from './bytes';
import type { SolanaBridgeConfig } from './config';
import { returnId } from './ids';
import { MemoError, encodeReturnMemo } from './memo';
import type { BlockhashInfo, HolderTokenAccount, SolanaRpc } from './rpc';
import { SolIntentStoreError, type SolanaIntentStore, type SolReturnIntent } from './intents';
import { describeSolRelease, describeSolReturn } from './status';
import { buildReturnMessage, checkSignedTransaction, messageOfWire, unsignedTransactionBytes } from './transaction';
import { isExplicitUserRejection, isWalletRejection, type SolanaWalletSession } from './walletStandard';

// ---------------------------------------------------------------------------
// Pure: the draft
// ---------------------------------------------------------------------------

export interface ReturnDraftInput {
  /** The amount as typed, in icUSD. */
  amountText: string;
  /** The ICP principal as typed. */
  principalText: string;
  /** Optional subaccount hex, empty for the default. */
  subaccountText: string;
  /** The holder token account chosen, base58, or ''. */
  sourceAccount: string;
}

export interface ReturnDraftEvaluation {
  ok: boolean;
  /** The first thing to fix, in the order a user would fix it. */
  problem: string | null;
  /** A warning that does not block sending. */
  warning: string | null;
  amountE8s: bigint | null;
  principal: string | null;
  subaccount: string | null;
}

export function evaluateReturnDraft(
  input: ReturnDraftInput,
  config: SolanaBridgeConfig,
  accounts: readonly HolderTokenAccount[] | null,
): ReturnDraftEvaluation {
  const fail = (problem: string): ReturnDraftEvaluation => ({ ok: false, problem, warning: null, amountE8s: null, principal: null, subaccount: null });
  if (config.halted) return fail(`The bridge is halted for safety: ${config.halted}`);
  const parsed = tryParseE8s(input.amountText);
  if (!parsed.ok) return fail(parsed.message);
  const amount = parsed.value;
  if (amount > config.maxDepositE8s) return fail(`The bridge credits at most ${formatE8s(config.maxDepositE8s)} icUSD in one return.`);
  if (amount <= config.releaseFeeE8s) return fail(`The amount must be larger than the release fee of ${formatE8s(config.releaseFeeE8s)} icUSD.`);
  let memoProblem: string | null = null;
  let subaccount: string | null = null;
  try {
    encodeReturnMemo({ principal: input.principalText, subaccount: input.subaccountText || null, nonce: 0n }, config.instanceCanisterId);
    subaccount = input.subaccountText || null;
  } catch (error) {
    memoProblem = error instanceof MemoError ? error.message : String(error);
  }
  if (memoProblem) return fail(memoProblem);
  if (!input.sourceAccount) return fail('Choose the Solana token account the icUSD should leave from.');
  const source = accounts?.find((a) => a.address === input.sourceAccount);
  if (accounts !== null && !source) return fail('That token account does not belong to the connected wallet.');
  if (source && source.amountE8s < amount) return fail(`That token account holds ${formatE8s(source.amountE8s)} icUSD, less than ${formatE8s(amount)}.`);
  const warning = config.releasesPaused
    ? 'Releases to the Internet Computer are paused right now. The bridge will still credit your return and release it once releases resume.'
    : null;
  return { ok: true, problem: null, warning, amountE8s: amount, principal: input.principalText, subaccount };
}

// ---------------------------------------------------------------------------
// The plan: what the holder reviews before the wallet is asked
// ---------------------------------------------------------------------------

export interface ReturnPlan {
  config: SolanaBridgeConfig;
  /** The holder wallet (fee payer and signer), base58. */
  holder: string;
  sourceAccount: string;
  amountE8s: bigint;
  icpPrincipal: string;
  icpSubaccount: string | null;
  nonce: string;
  memo: string;
  blockhash: BlockhashInfo;
  message: Bytes;
  /** The canonical return account the tokens go to. */
  returnAccount: string;
  createdAt: number;
}

export interface ReturnDeps {
  config: SolanaBridgeConfig;
  signing: SigningContext;
  store: SolanaIntentStore;
  rpc: SolanaRpc;
  /** The connected wallet at the moment of use. Never cached. */
  wallet(): SolanaWalletSession | null;
  reads: {
    getReturn(returnId: string): Promise<ReturnView | null>;
    getRelease(returnId: string): Promise<ReleaseView | null>;
  };
  bridge?: { notifySolanaReturn(signature: string): Promise<Result> };
  isCurrent(): boolean;
  random?: RandomBytes;
  now?(): number;
  sleep?: Sleep;
}

export type PrepareResult = { ok: true; plan: ReturnPlan } | { ok: false; message: string };

function randomNonce(random: RandomBytes): bigint {
  let n = 0n;
  for (const byte of random(8)) n = (n << 8n) | BigInt(byte);
  return n;
}

/**
 * Build the plan for review: validates the draft, reads a recent blockhash and builds the exact message. Makes no wallet
 * call and persists nothing.
 */
export async function prepareReturn(deps: ReturnDeps, input: ReturnDraftInput): Promise<PrepareResult> {
  const session = deps.wallet();
  if (!session) return { ok: false, message: 'Connect your Solana wallet first.' };
  if (!session.chains.includes(deps.config.network.chain)) {
    return { ok: false, message: `Your wallet account is not on ${deps.config.network.name}. Switch the network in your wallet.` };
  }
  const evaluation = evaluateReturnDraft(input, deps.config, null);
  if (!evaluation.ok || evaluation.amountE8s === null) return { ok: false, message: evaluation.problem ?? 'Check the amount.' };
  let accounts: HolderTokenAccount[];
  let blockhash: BlockhashInfo;
  try {
    [accounts, blockhash] = await Promise.all([
      deps.rpc.getTokenAccounts(session.address, deps.config.mint, deps.config.tokenProgramId),
      deps.rpc.getLatestBlockhash(),
    ]);
  } catch (error) {
    return { ok: false, message: `Solana could not be read: ${error instanceof Error ? error.message : String(error)}` };
  }
  const checked = evaluateReturnDraft(input, deps.config, accounts);
  if (!checked.ok) return { ok: false, message: checked.problem ?? 'Check the amount.' };
  const nonce = randomNonce(deps.random ?? defaultRandomBytes).toString();
  const memo = encodeReturnMemo({ principal: input.principalText, subaccount: input.subaccountText || null, nonce }, deps.config.instanceCanisterId);
  const built = buildReturnMessage({
    tokenProgram: deps.config.tokenProgram,
    mint: pubkeyFromBase58(deps.config.mint)!,
    returnOwner: pubkeyFromBase58(deps.config.returnOwner)!,
    roleKeys: [deps.config.feePayer, deps.config.mintAuthority].map((k) => pubkeyFromBase58(k)!),
    holder: pubkeyFromBase58(session.address) ?? new Uint8Array(32),
    sourceTokenAccount: pubkeyFromBase58(input.sourceAccount)!,
    amount: evaluation.amountE8s,
    memo,
    recentBlockhash: base58Decode(blockhash.blockhash) ?? new Uint8Array(32),
  });
  const returnAccount = base58Encode(built.returnAta);
  if (returnAccount !== deps.config.returnAccount) {
    return { ok: false, message: 'The return account this page derived is not the one the bridge reports, so nothing was prepared.' };
  }
  return {
    ok: true,
    plan: {
      config: deps.config,
      holder: session.address,
      sourceAccount: input.sourceAccount,
      amountE8s: evaluation.amountE8s,
      icpPrincipal: input.principalText,
      icpSubaccount: input.subaccountText || null,
      nonce,
      memo,
      blockhash,
      message: built.message,
      returnAccount,
      createdAt: (deps.now ?? Date.now)(),
    },
  };
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

export type ChainEvidence =
  | { kind: 'finalized'; slot: bigint }
  | { kind: 'in_flight'; confirmation: 'processed' | 'confirmed' }
  /**
   * The node reports that the exact saved attempt failed in execution. Only a FINALIZED failure is definitive: a failure
   * seen at processed or confirmed can still be replaced by a successful finalized execution of the same signature.
   */
  | { kind: 'failed_on_chain'; finalized: boolean }
  /**
   * The node does not know the signature. This is NOT proof that the transfer never executed: the search reads only the
   * latest wallet signatures and a node answers null for anything pruned, not yet indexed or unknown. `blockhashExpired`
   * only says the blockhash can no longer be included; it never makes absence a proof.
   */
  | { kind: 'not_found'; blockhashExpired: boolean | null }
  | { kind: 'unavailable'; message: string };

export interface ReturnEvidence {
  /** The signature the evidence is about: saved, or recovered by searching for our exact message. */
  signature: string | null;
  recovered: boolean;
  chain: ChainEvidence;
  /** The bridge canister's record of the return, if it has one. */
  ret: ReturnView | null;
  release: ReleaseView | null;
}

export type ReturnVerdict =
  /** The bridge has credited it. */
  | 'credited'
  /** Finalized on Solana; the bridge will credit it (or can be told). */
  | 'landed'
  /** Seen on Solana but not final yet (including a failure that is not final). It can still change. */
  | 'in_flight'
  /** The exact saved attempt FAILED in execution and Solana FINALIZED that failure: the tokens did not move. */
  | 'failed_on_chain'
  /** Cannot tell. This includes "not found", even after the blockhash expired: absence is never proof. */
  | 'unknown';

export function verdictOf(evidence: ReturnEvidence): ReturnVerdict {
  if (evidence.ret) return 'credited';
  switch (evidence.chain.kind) {
    case 'finalized':
      return 'landed';
    case 'in_flight':
      return 'in_flight';
    case 'failed_on_chain':
      return evidence.chain.finalized ? 'failed_on_chain' : 'in_flight';
    default:
      // not_found (even with an expired blockhash) and unavailable.
      return 'unknown';
  }
}

/**
 * Whether the original attempt may be forgotten: the wallet was never asked, or Solana FINALIZED a failure of the exact
 * saved attempt. Absence, expiry and an unfinalized failure never qualify.
 */
export function mayDiscard(intent: SolReturnIntent, verdict: ReturnVerdict | null): boolean {
  if (intent.status === 'prepared') return true;
  return verdict === 'failed_on_chain';
}

function returnIdOf(intent: SolReturnIntent, signature: string): string | null {
  const genesis = pubkeyFromBase58(intent.genesisHash);
  const sig = signatureFromBase58(signature);
  return genesis && sig ? returnId(genesis, sig) : null;
}

async function carriesSavedMessage(rpc: SolanaRpc, signature: string, message: Bytes): Promise<boolean> {
  const wire = await rpc.getTransactionWire(signature);
  const found = wire ? messageOfWire(wire) : null;
  return !!found && bytesEqual(found, message);
}

/**
 * Collect the evidence for one attempt. It reads only: no wallet is asked and nothing is sent. A saved signature is
 * looked up directly; an attempt without one (an ambiguous sign-and-send, or a reload during signing) is searched for in
 * the wallet's recent transactions by its EXACT saved message.
 */
export async function reconcileReturn(deps: Pick<ReturnDeps, 'rpc' | 'reads'>, intent: SolReturnIntent): Promise<ReturnEvidence> {
  let signature = intent.signature ?? null;
  let recovered = false;
  const message = fromHex(intent.messageHex);
  if (signature === null && message !== null) {
    try {
      for (const entry of await deps.rpc.getSignatures(intent.wallet, 25)) {
        const wire = await deps.rpc.getTransactionWire(entry.signature);
        const found = wire ? messageOfWire(wire) : null;
        if (found && bytesEqual(found, message)) {
          signature = entry.signature;
          recovered = true;
          break;
        }
      }
    } catch (error) {
      return { signature: null, recovered: false, chain: { kind: 'unavailable', message: error instanceof Error ? error.message : String(error) }, ret: null, release: null };
    }
  }
  let chain: ChainEvidence;
  try {
    const status = signature ? await deps.rpc.getSignatureStatus(signature) : null;
    if (status) {
      const finalizedFailure = status.failed && status.confirmation === 'finalized';
      // A signature the wallet REPORTED (sign and send) is not derived from our bytes: before a finalized failure can count
      // as definitive, the transaction behind it must carry exactly the saved message.
      if (finalizedFailure && !recovered && intent.signMode === 'sign_and_send' && message !== null && !(await carriesSavedMessage(deps.rpc, signature as string, message))) {
        chain = { kind: 'unavailable', message: 'Solana reports a failure for the signature the wallet gave, but that transaction could not be matched to the saved message, so it is not treated as proof.' };
      } else if (status.failed) {
        chain = { kind: 'failed_on_chain', finalized: finalizedFailure };
      } else {
        chain = status.confirmation === 'finalized' ? { kind: 'finalized', slot: status.slot } : { kind: 'in_flight', confirmation: status.confirmation };
      }
    } else {
      let expired: boolean | null = null;
      if (intent.lastValidBlockHeight !== null) expired = (await deps.rpc.getBlockHeight()) > BigInt(intent.lastValidBlockHeight);
      chain = { kind: 'not_found', blockhashExpired: expired };
    }
  } catch (error) {
    chain = { kind: 'unavailable', message: error instanceof Error ? error.message : String(error) };
  }
  let ret: ReturnView | null = null;
  let release: ReleaseView | null = null;
  const id = signature ? returnIdOf(intent, signature) : null;
  if (id) {
    try {
      ret = await deps.reads.getReturn(id);
      release = await deps.reads.getRelease(id);
    } catch {
      // The canister is read again on the next poll.
    }
  }
  return { signature, recovered, chain, ret, release };
}

// ---------------------------------------------------------------------------
// Pure: the flow state machine
// ---------------------------------------------------------------------------

export type SolReturnState =
  | { kind: 'draft' }
  | { kind: 'preparing' }
  | { kind: 'review'; plan: ReturnPlan }
  /** Saved earlier (reload): the wallet was never asked about it. It can be continued or discarded. */
  | { kind: 'prepared'; intent: SolReturnIntent }
  /** The wallet has the transaction. Do not close the page. */
  | { kind: 'signing'; intent: SolReturnIntent }
  /** Signed bytes are saved; the broadcast is under way. */
  | { kind: 'signed'; intent: SolReturnIntent }
  /** The user said no in the wallet. Nothing was signed. */
  | { kind: 'declined' }
  /** Nothing was sent. */
  | { kind: 'failed'; message: string }
  /** The signed bytes are saved but the broadcast result is unknown. The same bytes can be sent again. */
  | { kind: 'broadcast_uncertain'; intent: SolReturnIntent; message: string }
  /** An ambiguous sign-and-send: the original intent is held until evidence settles it. */
  | { kind: 'held'; intent: SolReturnIntent; message: string }
  /** Broadcast; waiting for finality and the bridge. */
  | { kind: 'tracking'; intent: SolReturnIntent; evidence: ReturnEvidence | null }
  | { kind: 'session_changed' }
  | { kind: 'blocked'; message: string };

export type SolReturnEvent =
  | { type: 'reset' }
  | { type: 'preparing' }
  | { type: 'plan_ready'; plan: ReturnPlan }
  | { type: 'plan_failed'; message: string }
  | { type: 'signing'; intent: SolReturnIntent }
  | { type: 'signed'; intent: SolReturnIntent }
  | { type: 'declined' }
  | { type: 'failed'; message: string }
  | { type: 'broadcast_uncertain'; intent: SolReturnIntent; message: string }
  | { type: 'held'; intent: SolReturnIntent; message: string }
  | { type: 'sent'; intent: SolReturnIntent }
  | { type: 'evidence'; intent: SolReturnIntent; evidence: ReturnEvidence }
  | { type: 'resume'; intent: SolReturnIntent }
  | { type: 'session_changed' }
  | { type: 'blocked'; message: string };

export const INITIAL_SOL_RETURN_STATE: SolReturnState = { kind: 'draft' };

export function reduceSolReturn(state: SolReturnState, event: SolReturnEvent): SolReturnState {
  switch (event.type) {
    case 'reset':
      // Only a state with nothing in flight can be reset; a held or signed attempt stays until it is resolved.
      return state.kind === 'signing' || state.kind === 'signed' || state.kind === 'held' || state.kind === 'broadcast_uncertain' ? state : { kind: 'draft' };
    case 'preparing':
      return state.kind === 'signing' || state.kind === 'signed' || state.kind === 'held' || state.kind === 'broadcast_uncertain' ? state : { kind: 'preparing' };
    case 'plan_ready':
      return { kind: 'review', plan: event.plan };
    case 'plan_failed':
      return { kind: 'failed', message: event.message };
    case 'signing':
      return { kind: 'signing', intent: event.intent };
    case 'signed':
      return { kind: 'signed', intent: event.intent };
    case 'declined':
      return { kind: 'declined' };
    case 'failed':
      return { kind: 'failed', message: event.message };
    case 'broadcast_uncertain':
      return { kind: 'broadcast_uncertain', intent: event.intent, message: event.message };
    case 'held':
      return { kind: 'held', intent: event.intent, message: event.message };
    case 'sent':
      return { kind: 'tracking', intent: event.intent, evidence: null };
    case 'evidence':
      return { kind: 'tracking', intent: event.intent, evidence: event.evidence };
    case 'resume':
      return event.intent.status === 'prepared'
        ? { kind: 'prepared', intent: event.intent }
        : event.intent.status === 'held' || event.intent.status === 'signing'
        ? { kind: 'held', intent: event.intent, message: event.intent.note ?? 'This attempt has no recorded outcome. It is held until the evidence settles it.' }
        : event.intent.status === 'signed'
          ? { kind: 'broadcast_uncertain', intent: event.intent, message: 'This attempt was signed but never confirmed as sent. Sending the same saved transaction again is safe.' }
          : { kind: 'tracking', intent: event.intent, evidence: null };
    case 'session_changed':
      return { kind: 'session_changed' };
    case 'blocked':
      return { kind: 'blocked', message: event.message };
  }
}

export type Emit = (event: SolReturnEvent) => void;

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') return (error as { message: string }).message;
  return String(error ?? 'unknown error');
}

const IN_FLIGHT_STATUSES: readonly SolReturnIntent['status'][] = ['prepared', 'signing', 'signed', 'held'];

/** Attempts on this device with no recorded outcome (the wallet may or may not have signed them). */
export function unresolvedReturns(store: SolanaIntentStore): SolReturnIntent[] {
  return store.list().filter((i): i is SolReturnIntent => i.kind === 'solana_to_icp' && IN_FLIGHT_STATUSES.includes(i.status));
}

/** Attempts that were broadcast and are not yet settled by the bridge: tracked after a reload. */
export function trackedReturns(store: SolanaIntentStore, destination: string): SolReturnIntent[] {
  return store.listFor(destination).filter((i): i is SolReturnIntent => i.kind === 'solana_to_icp' && i.status === 'sent');
}

function save(deps: ReturnDeps, intent: SolReturnIntent, patch: Partial<SolReturnIntent>): SolReturnIntent {
  return deps.store.patch<SolReturnIntent>(intent.id, patch);
}

function forget(deps: ReturnDeps, intent: SolReturnIntent): void {
  try {
    deps.store.remove(intent.id);
  } catch {
    // An unsent record is resolved on the next visit.
  }
}

/**
 * The confirm button. Call it directly from the click handler: everything before the first wallet call is
 * synchronous (the journal is synchronous storage), so a wallet that needs the click's user gesture still has it.
 */
export function startReturn(deps: ReturnDeps, plan: ReturnPlan, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  // Before anything is saved and before any wallet call: a disabled destination leaves no trace.
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'solana' });
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  const session = deps.wallet();
  if (!session) {
    emit({ type: 'blocked', message: 'Your Solana wallet is not connected. Nothing was signed.' });
    return Promise.resolve();
  }
  if (session.address !== plan.holder) {
    emit({ type: 'blocked', message: 'Your Solana wallet account changed since you reviewed this transfer. Review it again. Nothing was signed.' });
    return Promise.resolve();
  }
  if (!session.chains.includes(plan.config.network.chain)) {
    emit({ type: 'blocked', message: `Your wallet is not on ${plan.config.network.name}. Nothing was signed.` });
    return Promise.resolve();
  }
  if (!session.canSign && !session.canSignAndSend) {
    emit({ type: 'blocked', message: 'This wallet cannot sign Solana transactions. Nothing was signed.' });
    return Promise.resolve();
  }
  if (unresolvedReturns(deps.store).length > 0) {
    emit({ type: 'blocked', message: 'An earlier Solana return on this device has no recorded outcome yet. Resolve it first. Nothing was signed.' });
    return Promise.resolve();
  }
  const signMode: SolReturnIntent['signMode'] = session.canSign ? 'sign' : 'sign_and_send';
  const intent: SolReturnIntent = {
    v: 1,
    kind: 'solana_to_icp',
    id: newClientRequestId(deps.random).slice(2),
    destination: plan.config.network.networkId,
    genesisHash: plan.config.network.genesisHash,
    instance: plan.config.instanceCanisterId,
    mint: plan.config.mint,
    tokenProgram: plan.config.tokenProgram,
    returnAccount: plan.returnAccount,
    wallet: plan.holder,
    sourceAccount: plan.sourceAccount,
    icpPrincipal: plan.icpPrincipal,
    icpSubaccount: plan.icpSubaccount,
    amountE8s: plan.amountE8s,
    nonce: plan.nonce,
    memo: plan.memo,
    blockhash: plan.blockhash.blockhash,
    lastValidBlockHeight: plan.blockhash.lastValidBlockHeight.toString(),
    messageHex: toHex(plan.message),
    signMode,
    createdAt: (deps.now ?? Date.now)(),
    status: 'prepared',
  };
  let saved: SolReturnIntent;
  try {
    deps.store.put(intent);
    saved = save(deps, intent, { status: 'signing' });
  } catch (error) {
    emit({ type: 'blocked', message: error instanceof SolIntentStoreError ? error.message : errorMessage(error) });
    return Promise.resolve();
  }
  emit({ type: 'signing', intent: saved });
  return askWallet(deps, session, plan.message, saved, emit);
}

/** Ask the wallet for the saved attempt's message, once. The wallet call is the first await. */
async function askWallet(deps: ReturnDeps, session: SolanaWalletSession, message: Bytes, intent: SolReturnIntent, emit: Emit): Promise<void> {
  const transaction = unsignedTransactionBytes(message);
  const chain = deps.config.network.chain;
  if (intent.signMode === 'sign') {
    let signed: Bytes;
    try {
      signed = await session.signTransaction(transaction, chain);
    } catch (error) {
      forget(deps, intent);
      emit(isWalletRejection(error) ? { type: 'declined' } : { type: 'failed', message: `The wallet did not return a signed transaction (${errorMessage(error)}). Nothing was sent.` });
      return;
    }
    const check = checkSignedTransaction(signed, message);
    if (!check.ok) {
      forget(deps, intent);
      emit({
        type: 'failed',
        message:
          check.reason === 'message_changed'
            ? 'Your wallet changed the transaction before signing it, so it was not sent: the bridge would not recognize it as a return. Nothing was sent.'
            : 'The wallet returned a transaction that is not signed as expected. Nothing was sent.',
      });
      return;
    }
    // Persist the signed bytes and the derived first signature BEFORE anything is broadcast.
    let withBytes: SolReturnIntent;
    try {
      withBytes = save(deps, intent, { status: 'signed', signedHex: toHex(signed), signature: base58Encode(check.signature) });
    } catch (error) {
      emit({ type: 'failed', message: `${errorMessage(error)} The signed transaction was not sent.` });
      forget(deps, intent);
      return;
    }
    emit({ type: 'signed', intent: withBytes });
    await broadcast(deps, withBytes, signed, emit);
    return;
  }
  // Sign and send: the wallet broadcasts, so the answer can be ambiguous.
  let signatureBytes: Bytes;
  try {
    signatureBytes = await session.signAndSendTransaction(transaction, chain);
  } catch (error) {
    // Only an explicit user-rejection code clears the attempt. A wallet that already broadcast can throw text such as
    // "connection closed"; message matching is not proof, so anything else keeps the original attempt held.
    if (isExplicitUserRejection(error)) {
      forget(deps, intent);
      emit({ type: 'declined' });
      return;
    }
    hold(deps, intent, `The wallet did not say whether it sent the transaction (${errorMessage(error)}).`, emit);
    return;
  }
  if (signatureBytes.length !== 64 || signatureBytes.every((b) => b === 0)) {
    hold(deps, intent, 'The wallet answered without a usable signature, so it is unknown whether it sent the transaction.', emit);
    return;
  }
  try {
    emit({ type: 'sent', intent: save(deps, intent, { status: 'sent', signature: base58Encode(signatureBytes) }) });
  } catch {
    // The signature is a hint; the journal keeps the held record, and evidence settles it.
    hold(deps, intent, 'The wallet sent the transaction but its signature could not be saved on this device.', emit);
  }
}

function hold(deps: ReturnDeps, intent: SolReturnIntent, note: string, emit: Emit): void {
  let held = intent;
  try {
    held = save(deps, intent, { status: 'held', note });
  } catch {
    held = { ...intent, status: 'held', note };
  }
  emit({ type: 'held', intent: held, message: `${note} The original attempt is held; it is not rebuilt or signed again. Check it against the chain and the bridge below.` });
}

async function broadcast(deps: ReturnDeps, intent: SolReturnIntent, signed: Bytes, emit: Emit): Promise<void> {
  try {
    const reported = await deps.rpc.sendTransaction(signed);
    if (intent.signature && reported !== intent.signature) {
      // The node's answer differs from the signature derived from the signed bytes. The derived one is the attempt's identity.
      emit({ type: 'broadcast_uncertain', intent, message: 'The Solana node reported a different signature than the signed transaction carries. The saved transaction is unchanged.' });
      return;
    }
    emit({ type: 'sent', intent: save(deps, intent, { status: 'sent' }) });
  } catch (error) {
    emit({
      type: 'broadcast_uncertain',
      intent,
      message: `The signed transaction may not have reached Solana (${errorMessage(error)}). The same saved transaction can be sent again; it cannot pay twice.`,
    });
  }
}

/**
 * Send the SAME saved signed bytes again (a click). Never rebuilds or re-signs; a different transaction would be a
 * new, separate decision.
 */
export function rebroadcastReturn(deps: ReturnDeps, intent: SolReturnIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'solana' }) ?? intentBindingMessage(deps.signing, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  const signed = intent.signedHex ? fromHex(intent.signedHex) : null;
  if (!signed || intent.status !== 'signed') {
    emit({ type: 'blocked', message: 'There is no saved signed transaction to send again.' });
    return Promise.resolve();
  }
  return broadcast(deps, intent, signed, emit);
}

/** Forget an attempt whose FINALIZED failure proves it moved nothing (or one the wallet was never asked about). */
export function discardReturn(
  deps: Pick<ReturnDeps, 'store'>,
  intent: SolReturnIntent,
  verdict: ReturnVerdict | null,
): { ok: true } | { ok: false; message: string } {
  if (!mayDiscard(intent, verdict)) {
    return { ok: false, message: 'This attempt cannot be discarded: Solana has not finalized a failure of it. Not finding it is not proof that it never executed, so it stays on this device, unchanged.' };
  }
  try {
    deps.store.remove(intent.id);
  } catch (error) {
    return { ok: false, message: errorMessage(error) };
  }
  return { ok: true };
}

/**
 * Re-ask the wallet to sign the SAVED message of an attempt that was only `prepared` (the wallet was never asked),
 * while its blockhash can still land. The bytes are the ones the user already reviewed.
 */
export async function continuePrepared(deps: ReturnDeps, intent: SolReturnIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return;
  }
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'solana' }) ?? intentBindingMessage(deps.signing, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return;
  }
  const session = deps.wallet();
  if (!session || session.address !== intent.wallet || !session.chains.includes(deps.config.network.chain)) {
    emit({ type: 'blocked', message: 'Connect the wallet account this attempt was prepared for, on the same network. Nothing was signed.' });
    return;
  }
  if (intent.status !== 'prepared') {
    emit({ type: 'blocked', message: 'Only an attempt the wallet was never asked about can be continued.' });
    return;
  }
  const message = fromHex(intent.messageHex);
  if (!message) {
    emit({ type: 'blocked', message: 'The saved transaction is unreadable.' });
    return;
  }
  let saved: SolReturnIntent;
  try {
    saved = save(deps, intent, { status: 'signing' });
  } catch (error) {
    emit({ type: 'blocked', message: errorMessage(error) });
    return;
  }
  emit({ type: 'signing', intent: saved });
  await askWallet(deps, session, message, saved, emit);
}

// ---------------------------------------------------------------------------
// Tracking
// ---------------------------------------------------------------------------

/** What the page shows for an attempt, from the evidence: a hint until the bridge's own record says otherwise. */
export function describeReturnProgress(evidence: ReturnEvidence | null): { label: string; detail: string; terminal: boolean } {
  if (!evidence) return { label: 'Sent to Solana', detail: 'Waiting for Solana to finalize the transaction.', terminal: false };
  if (evidence.release || evidence.ret) {
    const d = describeSolReturn(evidence.ret, evidence.release);
    return { label: d.label, detail: d.detail, terminal: d.terminal };
  }
  switch (evidence.chain.kind) {
    case 'finalized':
      return {
        label: 'Finalized on Solana',
        detail: 'The transfer is final. The bridge credits it after it reads it from Solana; this is a wait, not a failure.',
        terminal: false,
      };
    case 'in_flight':
      return { label: 'Waiting for finality', detail: `Solana reports the transaction as ${evidence.chain.confirmation}. The bridge acts only on finalized transfers.`, terminal: false };
    case 'failed_on_chain':
      return evidence.chain.finalized
        ? { label: 'The transaction failed on Solana', detail: 'Solana finalized the failure: it landed but did not execute, so no icUSD moved. You can start again.', terminal: true }
        : {
            label: 'Failed, but not final yet',
            detail: 'Solana reports a failure that is not final yet. A result that is not final can still change, so the attempt stays saved and nothing is sent or signed again.',
            terminal: false,
          };
    case 'not_found':
      return {
        label: evidence.chain.blockhashExpired ? 'Not found, outcome unknown' : 'Not found yet',
        detail: evidence.chain.blockhashExpired
          ? "Solana did not report this transaction and its blockhash has expired. That is not proof that it did not execute: this check reads only recent history, and a node can have pruned it or not indexed it. The attempt stays saved and nothing is sent or signed again. Only a finalized result on Solana or the bridge's own records settle it."
          : 'Solana does not know the transaction yet. It may still land; it is kept until the evidence settles it.',
        terminal: false,
      };
    default:
      return { label: 'Checking', detail: 'Solana could not be read just now. The attempt is kept and checked again.', terminal: false };
  }
}

/**
 * Poll the evidence of a broadcast attempt until the bridge's release record is terminal. Marks the intent `done` only
 * from that record. Read-only.
 */
export async function trackReturn(
  deps: Pick<ReturnDeps, 'rpc' | 'reads' | 'store' | 'sleep'>,
  intent: SolReturnIntent,
  onEvidence: (evidence: ReturnEvidence) => void,
  signal?: AbortSignal,
): Promise<'done' | 'aborted' | 'exhausted'> {
  return pollWithBackoff<ReturnEvidence>({
    fetch: () => reconcileReturn(deps, intent),
    isDone: (e) => (e.release ? describeSolRelease(e.release).terminal : false),
    onUpdate: (e) => {
      onEvidence(e);
      // A signature recovered by searching is saved, so the next reload does not search again.
      if (e.recovered && e.signature && !intent.signature) {
        try {
          deps.store.patch<SolReturnIntent>(intent.id, { signature: e.signature, status: 'sent' });
        } catch {
          // Cosmetic: the search can be repeated.
        }
      }
      if (e.release && describeSolRelease(e.release).terminal) {
        try {
          deps.store.patch<SolReturnIntent>(intent.id, { status: 'done' });
        } catch {
          // Cosmetic.
        }
      }
    },
    sleep: deps.sleep ?? abortableSleep,
    signal,
  });
}

/**
 * "Tell the bridge now" (a click, the first signer call of that click). A hint: it makes the bridge look at the signature
 * sooner and settles nothing. The bridge also finds the transfer by itself.
 */
export async function notifyReturn(
  deps: Pick<ReturnDeps, 'bridge' | 'isCurrent' | 'signing'>,
  signature: string,
): Promise<{ ok: true } | { ok: false; message: string }> {
  if (!deps.bridge) return { ok: false, message: 'Sign in with your ICP wallet to tell the bridge.' };
  if (!deps.isCurrent()) return { ok: false, message: 'Your wallet session changed. Nothing was submitted.' };
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) return { ok: false, message: notAllowed };
  try {
    const result = await deps.bridge.notifySolanaReturn(signature);
    return 'Ok' in result ? { ok: true } : { ok: false, message: `The bridge did not accept the hint: ${JSON.stringify(result.Err, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))}` };
  } catch (error) {
    return { ok: false, message: `The hint did not return a result (${errorMessage(error)}). The bridge still finds the transfer by itself.` };
  }
}
