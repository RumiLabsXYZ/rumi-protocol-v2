/**
 * Sui -> ICP: the holder burns icUSD through the Gate's `bridge_out`, and the bridge releases the same amount (minus the
 * release fee) from escrow on the Internet Computer to the ICP account written into the call.
 *
 * Two layers, like the Base and Solana flows: a pure state machine (`reduceSuiReturn`, `evaluateReturnDraft`) and an
 * effect runner (`prepareReturn`, `startReturn`, `rebroadcastReturn`, `reconcileReturn`, `trackReturn`, ...) that talks
 * to the wallet, a keyless Sui node adapter and the bridge canister through injected dependencies.
 *
 * What the holder signs is exactly the reviewed call (`transaction.ts`): the accepted Move `bridge_out(gate, coin,
 * icp_principal, icp_subaccount, client_nonce)` with an exact-amount coin. The exact network, package, Gate, coins, sender,
 * amount, ICP account, nonce and the predicted `transfer_id` (never a transaction digest) are shown before the wallet is
 * asked and bound to the attempt.
 *
 * Safety properties:
 *  - The attempt is persisted BEFORE each effect: first the intent with its coins, nonce and predicted transfer id
 *    (`prepared`), then `signing` right before the wallet is asked, then (for a wallet that returns signed bytes) the
 *    validated bytes, signature and typed digest (`signed`) BEFORE anything is broadcast, then `sent`. The wallet call
 *    is the first asynchronous step of the click.
 *  - A wallet that returns signed bytes is checked against the reviewed attempt (`validateSignedReturn`). One that
 *    changed the transaction gets nothing broadcast.
 *  - A rejection is clean (nothing was signed): the attempt is forgotten. For a wallet that signs AND executes, only the
 *    explicit user-rejection code counts (`isExplicitUserRejection`); message text such as "closed" or "cancelled" is not
 *    proof, because a wallet can execute and then lose its connection. Any other sign-and-execute error, and any
 *    AMBIGUOUS result, holds the ORIGINAL intent. It is never rebuilt, never signed again, never given a new nonce and
 *    never replaced; the holder resolves it with evidence (the saved digest at the node, the bridge's own records).
 *  - ABSENCE IS NOT PROOF. A node answers "not found" for anything pruned, not yet indexed or unknown, and an epoch
 *    passing proves nothing about a transaction a wallet may have sent. Only a FINALIZED failure of the exact saved
 *    attempt lets it be forgotten.
 *  - A rebroadcast sends the SAME saved bytes (the same digest), so it cannot pay twice.
 *  - A wallet callback, a digest or an event is a hint. Only the bridge canister's release record settles a return.
 *  - A wallet is asked only after `assertCanSign` agrees (a disabled destination never reaches it), and only while the
 *    wallet account and network are the ones the attempt was prepared for.
 */

import { Principal } from '@dfinity/principal';
import type { CandidateView, ReleaseView, Result } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { fromBase64, toBase64 } from '@mysten/sui/utils';
import { formatE8s, tryParseE8s } from '../baseBridge/amounts';
import { defaultRandomBytes, newClientRequestId, type RandomBytes } from '../baseBridge/intents';
import { intentBindingMessage, signingBlockMessage, type SigningContext } from '../baseBridge/signingGuard';
import { abortableSleep, pollWithBackoff, type Sleep } from '../baseBridge/status';
import { fromHex, type Bytes } from '../solanaBridge/bytes';
import { MemoError, checkDestinationPrincipal } from '../solanaBridge/memo';
import { parseSuiAddress } from './address';
import type { SuiBridgeConfig } from './config';
import { U64_MAX, transferId } from './ids';
import { SuiIntentStoreError, type SuiIntentStore, type SuiReturnIntent } from './intents';
import type { SuiCoin, SuiRpc } from './rpc';
import { describeSuiRelease, describeSuiReturn } from './status';
import { SUBACCOUNT_BYTES, buildReturnTransaction, selectCoins, validateSignedReturn, type ExpectedReturn } from './transaction';
import { isExplicitUserRejection, isWalletRejection, type SuiTransactionLike, type SuiWalletSession } from './walletStandard';

// ---------------------------------------------------------------------------
// Pure: the draft
// ---------------------------------------------------------------------------

export interface ReturnDraftInput {
  /** The amount as typed, in icUSD. */
  amountText: string;
  /** The ICP principal as typed. */
  principalText: string;
  /** Optional subaccount hex, empty for the default (all zero). */
  subaccountText: string;
}

export interface ReturnDraftEvaluation {
  ok: boolean;
  /** The first thing to fix, in the order a user would fix it. */
  problem: string | null;
  /** A warning that does not block sending. */
  warning: string | null;
  amountE8s: bigint | null;
  principal: string | null;
  /** 64 lowercase hex digits for a non-default subaccount, else null. */
  subaccount: string | null;
}

const ZERO_SUBACCOUNT = '0'.repeat(64);

/** The subaccount field: empty or 64 hex digits. All zeros is the default and is carried as null. */
export function parseSubaccountField(text: string): { ok: true; hex: string | null; bytes: Bytes } | { ok: false; message: string } {
  const t = text.trim().toLowerCase();
  if (t === '' || t === ZERO_SUBACCOUNT) return { ok: true, hex: null, bytes: new Uint8Array(SUBACCOUNT_BYTES) };
  if (!/^[0-9a-f]{64}$/.test(t)) return { ok: false, message: 'A subaccount is 64 hexadecimal characters, or leave it empty for the default.' };
  return { ok: true, hex: t, bytes: fromHex(t)! };
}

export function evaluateReturnDraft(input: ReturnDraftInput, config: SuiBridgeConfig, holdings: readonly SuiCoin[] | null): ReturnDraftEvaluation {
  const fail = (problem: string): ReturnDraftEvaluation => ({ ok: false, problem, warning: null, amountE8s: null, principal: null, subaccount: null });
  if (config.halted) return fail(`The bridge is halted for safety: ${config.halted}`);
  const parsed = tryParseE8s(input.amountText);
  if (!parsed.ok) return fail(parsed.message);
  const amount = parsed.value;
  if (amount > U64_MAX) return fail('That amount is larger than the Gate can take in one return.');
  if (amount < config.minBridgeOutE8s) return fail(`The Gate accepts at least ${formatE8s(config.minBridgeOutE8s)} icUSD in one return.`);
  if (amount <= config.releaseFeeE8s) return fail(`The amount must be larger than the release fee of ${formatE8s(config.releaseFeeE8s)} icUSD.`);
  try {
    checkDestinationPrincipal(input.principalText, config.instanceCanisterId);
  } catch (error) {
    return fail(error instanceof MemoError ? error.message : String(error));
  }
  const subaccount = parseSubaccountField(input.subaccountText);
  if (!subaccount.ok) return fail(subaccount.message);
  if (holdings !== null) {
    const picked = selectCoins(holdings, amount);
    if (!picked.ok) return fail(picked.message);
  }
  const warning = config.releasesPaused
    ? 'Releases to the Internet Computer are paused right now. The bridge will still record your burn and release it once releases resume.'
    : null;
  return { ok: true, problem: null, warning, amountE8s: amount, principal: input.principalText, subaccount: subaccount.hex };
}

// ---------------------------------------------------------------------------
// The plan: what the holder reviews before the wallet is asked
// ---------------------------------------------------------------------------

export interface ReturnPlan {
  config: SuiBridgeConfig;
  /** The holder's Sui address: sender, signer and gas payer. */
  sender: string;
  /** The coins the amount is taken from, in call order. */
  coins: SuiCoin[];
  coinTotalE8s: bigint;
  amountE8s: bigint;
  icpPrincipal: string;
  /** 64 lowercase hex digits for a non-default subaccount, else null (the all-zero default). */
  icpSubaccount: string | null;
  nonce: string;
  /** The Gate's transfer id this attempt will have (the release key at the bridge). */
  transferId: string;
  /** The holder's SUI balance in MIST (gas, 9 decimals), when it could be read. */
  gasBalanceMist: bigint | null;
  /** What the wallet is handed. */
  transaction: SuiTransactionLike;
  createdAt: number;
}

export interface ReturnDeps {
  config: SuiBridgeConfig;
  signing: SigningContext;
  store: SuiIntentStore;
  rpc: SuiRpc;
  /** The connected wallet at the moment of use. Never cached. */
  wallet(): SuiWalletSession | null;
  reads: {
    getRelease(transferId: string): Promise<ReleaseView | null>;
    /** The observer's candidate for this sender and nonce, a hint about what the bridge has heard. */
    getCandidate(sender: string, nonce: string): Promise<CandidateView | null>;
  };
  bridge?: { notifySuiReturn(digest: string): Promise<Result> };
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

function principalBytes(text: string): Bytes {
  return Principal.fromText(text).toUint8Array();
}

function transferIdOf(config: SuiBridgeConfig, sender: string, nonce: string): string {
  return transferId(fromHex(config.network.chainIdentifier)!, fromHex(config.gateId.slice(2))!, fromHex(sender.slice(2))!, BigInt(nonce));
}

/**
 * Build the plan for review: validates the draft, reads the holder's coins and gas balance and builds the exact call.
 * Makes no wallet call and persists nothing.
 */
export async function prepareReturn(deps: ReturnDeps, input: ReturnDraftInput): Promise<PrepareResult> {
  const session = deps.wallet();
  if (!session) return { ok: false, message: 'Connect your Sui wallet first.' };
  if (!session.chains.includes(deps.config.network.chain)) {
    return { ok: false, message: `Your wallet account is not on ${deps.config.network.name}. Switch the network in your wallet.` };
  }
  if (!parseSuiAddress(session.address).ok) return { ok: false, message: 'Your wallet account is not a Sui address this page can use.' };
  const evaluation = evaluateReturnDraft(input, deps.config, null);
  if (!evaluation.ok || evaluation.amountE8s === null) return { ok: false, message: evaluation.problem ?? 'Check the amount.' };
  let holdings: SuiCoin[];
  let gas: bigint | null = null;
  try {
    holdings = await deps.rpc.listCoins(session.address, deps.config.coinType);
    gas = await deps.rpc.getSuiBalance(session.address);
  } catch (error) {
    return { ok: false, message: `Sui could not be read: ${error instanceof Error ? error.message : String(error)}` };
  }
  const checked = evaluateReturnDraft(input, deps.config, holdings);
  if (!checked.ok) return { ok: false, message: checked.problem ?? 'Check the amount.' };
  if (gas === 0n) {
    return { ok: false, message: 'Your Sui address holds no SUI. SUI (not icUSD) pays the gas for this transfer. Add some SUI first.' };
  }
  const picked = selectCoins(holdings, evaluation.amountE8s);
  if (!picked.ok) return { ok: false, message: picked.message };
  const nonce = randomNonce(deps.random ?? defaultRandomBytes).toString();
  const subaccount = parseSubaccountField(input.subaccountText);
  if (!subaccount.ok) return { ok: false, message: subaccount.message };
  const bytes = principalBytes(input.principalText);
  const transaction = buildReturnTransaction({
    packageId: deps.config.packageId,
    gateId: deps.config.gateId,
    gateInitialSharedVersion: deps.config.gateInitialSharedVersion,
    sender: session.address,
    coins: picked.coins,
    amountE8s: evaluation.amountE8s,
    principalBytes: bytes,
    subaccount: subaccount.bytes,
    nonce: BigInt(nonce),
  });
  return {
    ok: true,
    plan: {
      config: deps.config,
      sender: session.address,
      coins: picked.coins,
      coinTotalE8s: picked.totalE8s,
      amountE8s: evaluation.amountE8s,
      icpPrincipal: input.principalText,
      icpSubaccount: subaccount.hex,
      nonce,
      transferId: transferIdOf(deps.config, session.address, nonce),
      gasBalanceMist: gas,
      transaction,
      createdAt: (deps.now ?? Date.now)(),
    },
  };
}

/** What a wallet's answer is checked against, from the saved intent alone (so a reload checks the same thing). */
export function expectationOf(config: SuiBridgeConfig, intent: SuiReturnIntent): ExpectedReturn {
  return {
    sender: intent.sender,
    packageId: intent.packageId,
    gateId: intent.gateId,
    gateInitialSharedVersion: config.gateInitialSharedVersion,
    coinIds: intent.coins.map((c) => c.objectId),
    coinTotalE8s: intent.coins.reduce((sum, c) => sum + c.balance, 0n),
    amountE8s: intent.amountE8s,
    principalBytes: principalBytes(intent.icpPrincipal),
    subaccount: intent.icpSubaccount ? fromHex(intent.icpSubaccount)! : new Uint8Array(SUBACCOUNT_BYTES),
    nonce: BigInt(intent.nonce),
  };
}

/** Rebuild the SAME call from a saved intent (a never-asked attempt after a reload). */
export function transactionOfIntent(config: SuiBridgeConfig, intent: SuiReturnIntent): SuiTransactionLike {
  const expected = expectationOf(config, intent);
  return buildReturnTransaction({
    packageId: intent.packageId,
    gateId: intent.gateId,
    gateInitialSharedVersion: expected.gateInitialSharedVersion,
    sender: intent.sender,
    coins: intent.coins,
    amountE8s: intent.amountE8s,
    principalBytes: expected.principalBytes,
    subaccount: expected.subaccount,
    nonce: expected.nonce,
  });
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

export type ChainEvidence =
  /** The node reports the exact saved transaction executed and is included in a checkpoint. */
  | { kind: 'finalized'; checkpoint: bigint }
  /** Executed, but not yet in a checkpoint. */
  | { kind: 'in_flight' }
  /**
   * The node reports that the exact saved attempt failed in execution. Only a failure included in a checkpoint
   * (`finalized`) is definitive; the Gate then burned nothing.
   */
  | { kind: 'failed_on_chain'; finalized: boolean }
  /**
   * The node does not know the digest. This is NOT proof that the transfer never executed: a node answers "not found" for
   * anything pruned, not yet indexed or unknown.
   */
  | { kind: 'not_found' }
  /** No digest was ever recorded (a sign-and-execute wallet that did not say). Only the bridge's records can settle it. */
  | { kind: 'no_digest' }
  | { kind: 'unavailable'; message: string };

export interface ReturnEvidence {
  /** The typed transaction digest the evidence is about, when one is saved. */
  digest: string | null;
  chain: ChainEvidence;
  /** The bridge canister's release record, keyed by the predicted transfer id. */
  release: ReleaseView | null;
  /** The observer's candidate for this sender and nonce: a hint about what the bridge has heard. */
  candidate: CandidateView | null;
}

export type ReturnVerdict =
  /** The bridge has recorded the return. */
  | 'credited'
  /** Final on Sui; the bridge will record it (or can be told). */
  | 'landed'
  /** Seen on Sui but not final yet (including a failure that is not final). It can still change. */
  | 'in_flight'
  /** The exact saved attempt FAILED in execution and Sui FINALIZED that failure: nothing was burned. */
  | 'failed_on_chain'
  /** Cannot tell. This includes "not found": absence is never proof. */
  | 'unknown';

export function verdictOf(evidence: ReturnEvidence): ReturnVerdict {
  if (evidence.release) return 'credited';
  switch (evidence.chain.kind) {
    case 'finalized':
      return 'landed';
    case 'in_flight':
      return 'in_flight';
    case 'failed_on_chain':
      return evidence.chain.finalized ? 'failed_on_chain' : 'in_flight';
    default:
      return 'unknown';
  }
}

/**
 * Whether the original attempt may be forgotten: the wallet was never asked, or Sui FINALIZED a failure of the exact
 * saved attempt. Absence, a passed epoch and an unfinalized failure never qualify.
 */
export function mayDiscard(intent: SuiReturnIntent, verdict: ReturnVerdict | null): boolean {
  if (intent.status === 'prepared') return true;
  return verdict === 'failed_on_chain';
}

/**
 * Collect the evidence for one attempt. It reads only: no wallet is asked and nothing is sent. The node is asked about
 * the saved digest; the bridge is asked about the predicted transfer id. A node that cannot be reached is `unavailable`,
 * never "not found".
 */
export async function reconcileReturn(deps: Pick<ReturnDeps, 'rpc' | 'reads'>, intent: SuiReturnIntent): Promise<ReturnEvidence> {
  const digest = intent.digest ?? null;
  let chain: ChainEvidence;
  if (digest === null) {
    chain = { kind: 'no_digest' };
  } else {
    try {
      const status = await deps.rpc.getTransactionStatus(digest);
      if (status === null) chain = { kind: 'not_found' };
      else if (status.checkpoint !== null) chain = status.success ? { kind: 'finalized', checkpoint: status.checkpoint } : { kind: 'failed_on_chain', finalized: true };
      else chain = status.success ? { kind: 'in_flight' } : { kind: 'failed_on_chain', finalized: false };
    } catch (error) {
      chain = { kind: 'unavailable', message: error instanceof Error ? error.message : String(error) };
    }
  }
  let release: ReleaseView | null = null;
  let candidate: CandidateView | null = null;
  try {
    release = await deps.reads.getRelease(intent.transferId);
    if (!release) candidate = await deps.reads.getCandidate(intent.sender, intent.nonce);
  } catch {
    // The canister is read again on the next poll.
  }
  return { digest, chain, release, candidate };
}

// ---------------------------------------------------------------------------
// Pure: the flow state machine
// ---------------------------------------------------------------------------

export type SuiReturnState =
  | { kind: 'draft' }
  | { kind: 'preparing' }
  | { kind: 'review'; plan: ReturnPlan }
  /** Saved earlier (reload): the wallet was never asked about it. It can be continued or discarded. */
  | { kind: 'prepared'; intent: SuiReturnIntent }
  /** The wallet has the transaction. Do not close the page. */
  | { kind: 'signing'; intent: SuiReturnIntent }
  /** Signed bytes are saved; the broadcast is under way. */
  | { kind: 'signed'; intent: SuiReturnIntent }
  /** The user said no in the wallet. Nothing was signed. */
  | { kind: 'declined' }
  /** Nothing was sent. */
  | { kind: 'failed'; message: string }
  /** The signed bytes are saved but the broadcast result is unknown. The same bytes can be sent again. */
  | { kind: 'broadcast_uncertain'; intent: SuiReturnIntent; message: string }
  /** An ambiguous sign-and-execute: the original intent is held until evidence settles it. */
  | { kind: 'held'; intent: SuiReturnIntent; message: string }
  /** Broadcast; waiting for the bridge. */
  | { kind: 'tracking'; intent: SuiReturnIntent; evidence: ReturnEvidence | null }
  | { kind: 'session_changed' }
  | { kind: 'blocked'; message: string };

export type SuiReturnEvent =
  | { type: 'reset' }
  | { type: 'preparing' }
  | { type: 'plan_ready'; plan: ReturnPlan }
  | { type: 'plan_failed'; message: string }
  | { type: 'signing'; intent: SuiReturnIntent }
  | { type: 'signed'; intent: SuiReturnIntent }
  | { type: 'declined' }
  | { type: 'failed'; message: string }
  | { type: 'broadcast_uncertain'; intent: SuiReturnIntent; message: string }
  | { type: 'held'; intent: SuiReturnIntent; message: string }
  | { type: 'sent'; intent: SuiReturnIntent }
  | { type: 'evidence'; intent: SuiReturnIntent; evidence: ReturnEvidence }
  | { type: 'resume'; intent: SuiReturnIntent }
  | { type: 'session_changed' }
  | { type: 'blocked'; message: string };

export const INITIAL_SUI_RETURN_STATE: SuiReturnState = { kind: 'draft' };

export function reduceSuiReturn(state: SuiReturnState, event: SuiReturnEvent): SuiReturnState {
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

export type Emit = (event: SuiReturnEvent) => void;

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') return (error as { message: string }).message;
  return String(error ?? 'unknown error');
}

const IN_FLIGHT_STATUSES: readonly SuiReturnIntent['status'][] = ['prepared', 'signing', 'signed', 'held'];

/** Attempts on this device with no recorded outcome (the wallet may or may not have signed them). */
export function unresolvedReturns(store: SuiIntentStore): SuiReturnIntent[] {
  return store.list().filter((i): i is SuiReturnIntent => i.kind === 'sui_to_icp' && IN_FLIGHT_STATUSES.includes(i.status));
}

/** Attempts that were broadcast and are not yet settled by the bridge: tracked after a reload. */
export function trackedReturns(store: SuiIntentStore, destination: string): SuiReturnIntent[] {
  return store.listFor(destination).filter((i): i is SuiReturnIntent => i.kind === 'sui_to_icp' && i.status === 'sent');
}

function save(deps: Pick<ReturnDeps, 'store'>, intent: SuiReturnIntent, patch: Partial<SuiReturnIntent>): SuiReturnIntent {
  return deps.store.patch<SuiReturnIntent>(intent.id, patch);
}

function forget(deps: Pick<ReturnDeps, 'store'>, intent: SuiReturnIntent): void {
  try {
    deps.store.remove(intent.id);
  } catch {
    // An unsent record is resolved on the next visit.
  }
}

/**
 * The confirm button. Call it directly from the click handler: everything before the first wallet call is synchronous
 * (the journal is synchronous storage), so a wallet that needs the click's user gesture still has it.
 */
export function startReturn(deps: ReturnDeps, plan: ReturnPlan, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  // Before anything is saved and before any wallet call: a disabled destination leaves no trace.
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'sui' });
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  const session = deps.wallet();
  if (!session) {
    emit({ type: 'blocked', message: 'Your Sui wallet is not connected. Nothing was signed.' });
    return Promise.resolve();
  }
  if (session.address !== plan.sender) {
    emit({ type: 'blocked', message: 'Your Sui wallet account changed since you reviewed this transfer. Review it again. Nothing was signed.' });
    return Promise.resolve();
  }
  if (!session.chains.includes(plan.config.network.chain)) {
    emit({ type: 'blocked', message: `Your wallet account is not on ${plan.config.network.name}. Switch the network in your wallet. Nothing was signed.` });
    return Promise.resolve();
  }
  if (unresolvedReturns(deps.store).length > 0) {
    emit({ type: 'blocked', message: 'An earlier Sui return on this device has no recorded outcome yet. It stays saved exactly as it was; check its evidence below. Nothing was signed.' });
    return Promise.resolve();
  }
  if (!session.canSign && !session.canSignAndExecute) {
    emit({ type: 'blocked', message: 'This wallet cannot sign Sui transactions. Nothing was signed.' });
    return Promise.resolve();
  }
  const signMode: SuiReturnIntent['signMode'] = session.canSign ? 'sign' : 'sign_and_execute';
  const intent: SuiReturnIntent = {
    v: 1,
    kind: 'sui_to_icp',
    id: newClientRequestId(deps.random).slice(2),
    destination: plan.config.network.networkId,
    chainIdentifier: plan.config.network.chainIdentifier,
    instance: plan.config.instanceCanisterId,
    packageId: plan.config.packageId,
    gateId: plan.config.gateId,
    coinType: plan.config.coinType,
    sender: plan.sender,
    coins: plan.coins.map((c) => ({ objectId: c.objectId, version: c.version, digest: c.digest, balance: c.balance })),
    icpPrincipal: plan.icpPrincipal,
    icpSubaccount: plan.icpSubaccount,
    amountE8s: plan.amountE8s,
    nonce: plan.nonce,
    transferId: plan.transferId,
    signMode,
    createdAt: (deps.now ?? Date.now)(),
    status: 'prepared',
  };
  let saved: SuiReturnIntent;
  try {
    deps.store.put(intent);
    saved = save(deps, intent, { status: 'signing' });
  } catch (error) {
    emit({ type: 'blocked', message: error instanceof SuiIntentStoreError ? error.message : errorMessage(error) });
    return Promise.resolve();
  }
  emit({ type: 'signing', intent: saved });
  return askWallet(deps, session, plan.transaction, saved, emit);
}

/** Ask the wallet for the saved attempt's call, once. The wallet call is the first await. */
async function askWallet(deps: ReturnDeps, session: SuiWalletSession, transaction: SuiTransactionLike, intent: SuiReturnIntent, emit: Emit): Promise<void> {
  const chain = deps.config.network.chain;
  const expected = expectationOf(deps.config, intent);
  if (intent.signMode === 'sign') {
    let signed;
    try {
      signed = await session.signTransaction(transaction, chain);
    } catch (error) {
      forget(deps, intent);
      emit(isWalletRejection(error) ? { type: 'declined' } : { type: 'failed', message: `The wallet did not return a signed transaction (${errorMessage(error)}). Nothing was sent.` });
      return;
    }
    let bytes: Bytes;
    try {
      bytes = fromBase64(signed.bytes);
    } catch {
      forget(deps, intent);
      emit({ type: 'failed', message: 'The wallet returned bytes that are not a transaction. Nothing was sent.' });
      return;
    }
    const check = await validateSignedReturn(bytes, signed.signature, expected);
    if (!check.ok) {
      forget(deps, intent);
      emit({ type: 'failed', message: `${check.message} It was not sent: the bridge would not recognize it as your return. Nothing was sent.` });
      return;
    }
    // Persist the validated bytes, the signature and the typed digest BEFORE anything is broadcast.
    let withBytes: SuiReturnIntent;
    try {
      withBytes = save(deps, intent, { status: 'signed', signedBytes: toBase64(bytes), signature: signed.signature, digest: check.digest });
    } catch (error) {
      emit({ type: 'failed', message: `${errorMessage(error)} The signed transaction was not sent.` });
      forget(deps, intent);
      return;
    }
    emit({ type: 'signed', intent: withBytes });
    await broadcast(deps, withBytes, bytes, emit);
    return;
  }
  // Sign and execute: the wallet builds and executes, so the answer can be ambiguous.
  let executed;
  try {
    executed = await session.signAndExecuteTransaction(transaction, chain);
  } catch (error) {
    // Only an explicit user-rejection code clears the attempt. A wallet that already executed can throw text such as
    // "connection closed"; message matching is not proof, so anything else keeps the original attempt held.
    if (isExplicitUserRejection(error)) {
      forget(deps, intent);
      emit({ type: 'declined' });
      return;
    }
    hold(deps, intent, `The wallet did not say whether it executed the transaction (${errorMessage(error)}).`, emit);
    return;
  }
  let bytes: Bytes;
  try {
    bytes = fromBase64(executed.bytes);
  } catch {
    hold(deps, intent, `The wallet answered with unusable bytes, so it is unknown what it executed (digest ${executed.digest}).`, emit);
    return;
  }
  const check = await validateSignedReturn(bytes, executed.signature, expected);
  if (!check.ok) {
    hold(deps, intent, `${check.message} The wallet may already have executed it (digest ${executed.digest}).`, emit);
    return;
  }
  if (check.digest !== executed.digest) {
    hold(deps, intent, `The wallet reported digest ${executed.digest} but the transaction it returned has digest ${check.digest}.`, emit);
    return;
  }
  try {
    emit({ type: 'sent', intent: save(deps, intent, { status: 'sent', signedBytes: toBase64(bytes), signature: executed.signature, digest: check.digest }) });
  } catch {
    // The returned evidence is a hint; the journal keeps the held record, and the bridge's records settle it.
    hold(deps, intent, `The wallet executed the transaction (digest ${check.digest}) but its record could not be saved on this device.`, emit);
  }
}

function hold(deps: ReturnDeps, intent: SuiReturnIntent, note: string, emit: Emit): void {
  let held = intent;
  try {
    held = save(deps, intent, { status: 'held', note });
  } catch {
    held = { ...intent, status: 'held', note };
  }
  emit({ type: 'held', intent: held, message: `${note} The original attempt is held; it is not rebuilt, signed again or given a new nonce. Check it against the bridge's records below.` });
}

async function broadcast(deps: ReturnDeps, intent: SuiReturnIntent, signed: Bytes, emit: Emit): Promise<void> {
  try {
    const reported = await deps.rpc.executeTransaction(signed, [intent.signature!]);
    if (intent.digest && reported.digest !== intent.digest) {
      // The node's answer differs from the digest derived from the signed bytes. The derived one is the attempt's identity.
      emit({ type: 'broadcast_uncertain', intent, message: 'The Sui node reported a different digest than the signed transaction carries. The saved transaction is unchanged.' });
      return;
    }
    emit({ type: 'sent', intent: save(deps, intent, { status: 'sent' }) });
  } catch (error) {
    emit({
      type: 'broadcast_uncertain',
      intent,
      message: `The signed transaction may not have reached Sui (${errorMessage(error)}). The same saved transaction can be sent again; it cannot burn twice.`,
    });
  }
}

/**
 * Send the SAME saved signed bytes again (a click). Never rebuilds or re-signs; a different transaction would be a new,
 * separate decision. A passed epoch or a missing history entry never changes that.
 */
export function rebroadcastReturn(deps: ReturnDeps, intent: SuiReturnIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'sui' }) ?? intentBindingMessage(deps.signing, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  const signed = intent.signedBytes && intent.signMode === 'sign' ? safeFromBase64(intent.signedBytes) : null;
  if (!signed || !intent.signature || (intent.status !== 'signed' && intent.status !== 'sent')) {
    emit({ type: 'blocked', message: 'There is no saved signed transaction to send again.' });
    return Promise.resolve();
  }
  return broadcast(deps, intent, signed, emit);
}

function safeFromBase64(text: string): Bytes | null {
  try {
    return fromBase64(text);
  } catch {
    return null;
  }
}

/** Forget an attempt whose FINALIZED failure proves it moved nothing (or one the wallet was never asked about). */
export function discardReturn(
  deps: Pick<ReturnDeps, 'store'>,
  intent: SuiReturnIntent,
  verdict: ReturnVerdict | null,
): { ok: true } | { ok: false; message: string } {
  if (!mayDiscard(intent, verdict)) {
    return { ok: false, message: 'This attempt cannot be discarded: Sui has not finalized a failure of it. Not finding it is not proof that it never executed, so it stays on this device, unchanged.' };
  }
  try {
    deps.store.remove(intent.id);
  } catch (error) {
    return { ok: false, message: errorMessage(error) };
  }
  return { ok: true };
}

/**
 * Re-ask the wallet for an attempt that was only `prepared` (the wallet was never asked): the same call is rebuilt from the
 * saved intent, with the same coins, nonce and transfer id. Nothing is signed or sent unless the user approves in the wallet.
 */
export async function continuePrepared(deps: ReturnDeps, intent: SuiReturnIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return;
  }
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'sui' }) ?? intentBindingMessage(deps.signing, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return;
  }
  const session = deps.wallet();
  if (!session || session.address !== intent.sender || !session.chains.includes(deps.config.network.chain)) {
    emit({ type: 'blocked', message: 'Connect the wallet account this attempt was prepared for, on the same network. Nothing was signed.' });
    return;
  }
  if (intent.status !== 'prepared') {
    emit({ type: 'blocked', message: 'This attempt is no longer waiting for a first signature.' });
    return;
  }
  let saved: SuiReturnIntent;
  try {
    saved = save(deps, intent, { status: 'signing', signMode: session.canSign ? 'sign' : 'sign_and_execute' });
  } catch (error) {
    emit({ type: 'blocked', message: error instanceof SuiIntentStoreError ? error.message : errorMessage(error) });
    return;
  }
  emit({ type: 'signing', intent: saved });
  await askWallet(deps, session, transactionOfIntent(deps.config, saved), saved, emit);
}

// ---------------------------------------------------------------------------
// Progress, tracking and the optional hint
// ---------------------------------------------------------------------------

export function describeReturnProgress(evidence: ReturnEvidence | null): { label: string; detail: string; terminal: boolean } {
  if (!evidence) return { label: 'Sent to Sui', detail: 'Waiting for the bridge to see the burn.', terminal: false };
  if (evidence.release || evidence.candidate) {
    const d = describeSuiReturn(evidence.release, evidence.candidate);
    return { label: d.label, detail: d.detail, terminal: d.terminal };
  }
  switch (evidence.chain.kind) {
    case 'finalized':
      return {
        label: 'Final on Sui',
        detail: 'The transaction is final on Sui. The bridge opens the release after it verifies the burn from the Gate; this is a wait, not a failure.',
        terminal: false,
      };
    case 'in_flight':
      return { label: 'Waiting for finality', detail: 'Sui reports the transaction but has not included it in a checkpoint yet.', terminal: false };
    case 'failed_on_chain':
      return evidence.chain.finalized
        ? { label: 'The transaction failed on Sui', detail: 'Sui finalized the failure: the Gate burned nothing, so no icUSD moved. You can start again.', terminal: true }
        : {
            label: 'Failed, but not final yet',
            detail: 'Sui reports a failure that is not final yet. A result that is not final can still change, so the attempt stays saved and nothing is sent or signed again.',
            terminal: false,
          };
    case 'not_found':
      return {
        label: 'Not found by this check',
        detail: 'The Sui node does not report this transaction. That is not proof that it did not execute: a node can have pruned it or not indexed it yet. The attempt stays saved and nothing is sent or signed again. Only a finalized result on Sui or the bridge\'s own records settle it.',
        terminal: false,
      };
    case 'no_digest':
      return {
        label: 'Waiting for the bridge',
        detail: 'The wallet did not report a transaction digest, so only the bridge\'s own records can settle this attempt. It stays saved exactly as it was.',
        terminal: false,
      };
    default:
      return { label: 'Checking', detail: 'Sui could not be read just now. The attempt is kept and checked again.', terminal: false };
  }
}

/**
 * Poll the evidence of a broadcast attempt until the bridge's release record is terminal. Marks the intent `done` only from
 * that record. Read-only.
 */
export async function trackReturn(
  deps: Pick<ReturnDeps, 'rpc' | 'reads' | 'store' | 'sleep'>,
  intent: SuiReturnIntent,
  onEvidence: (evidence: ReturnEvidence) => void,
  signal?: AbortSignal,
): Promise<'done' | 'aborted' | 'exhausted'> {
  return pollWithBackoff<ReturnEvidence>({
    fetch: () => reconcileReturn(deps, intent),
    isDone: (e) => (e.release ? describeSuiRelease(e.release).terminal : false),
    onUpdate: (e) => {
      onEvidence(e);
      if (e.release && describeSuiRelease(e.release).terminal) {
        try {
          deps.store.patch<SuiReturnIntent>(intent.id, { status: 'done' });
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
 * "Tell the bridge now" (an explicit click, the first signer call of that click; never automatic, never after a reload).
 * A hint: it makes the bridge look at the digest sooner and settles nothing. The bridge also finds the burn by itself.
 */
export async function notifyReturn(
  deps: Pick<ReturnDeps, 'bridge' | 'isCurrent' | 'signing'>,
  digest: string,
): Promise<{ ok: true } | { ok: false; message: string }> {
  if (!deps.bridge) return { ok: false, message: 'Sign in with your ICP wallet to tell the bridge.' };
  if (!deps.isCurrent()) return { ok: false, message: 'Your wallet session changed. Nothing was submitted.' };
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) return { ok: false, message: notAllowed };
  try {
    const result = await deps.bridge.notifySuiReturn(digest);
    return 'Ok' in result ? { ok: true } : { ok: false, message: `The bridge did not accept the hint: ${JSON.stringify(result.Err, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))}` };
  } catch (error) {
    return { ok: false, message: `The hint did not return a result (${errorMessage(error)}). The bridge still finds the burn by itself.` };
  }
}
