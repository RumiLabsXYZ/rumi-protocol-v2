/**
 * Base -> ICP: BridgedIcUSD on Base burned through `bridgeOut` to release
 * escrowed icUSD on the Internet Computer.
 *
 * Two layers, like `icpToBase.ts`:
 *  - pure: `evaluateBaseToIcpDraft` (what blocks the send button and why),
 *    `classifyUnsentIntent` (what to do with a record found after a reload) and
 *    `reduceBaseToIcp` (the flow state machine), and
 *  - effects: `readBaseSnapshot`, `sendBridgeOut`, `checkIntentOnChain`,
 *    `trackBaseToIcp`, `notifyBridge`.
 *
 * Safety properties:
 *  - A random 32 byte client nonce is generated and the contract-computed
 *    `transferId` is read with `eth_call` BEFORE the wallet prompt. The intent
 *    (nonce, transfer id, sender, amount, destination, chain, contract) is
 *    persisted before the wallet is asked to sign.
 *  - Immediately before `eth_sendTransaction` the chain id and the selected
 *    account are read again and `bridgeOutUsed(transferId)` is checked. A
 *    change aborts the send; a used transfer id means "already sent, track it".
 *  - A resend after a reload uses the SAME nonce (the contract also rejects a
 *    repeated (sender, nonce)). An unsent intent can only be sent from the
 *    account that created it, because the transfer id depends on the sender.
 *  - Wallet receipts are hints. Only the canister's release record can show a
 *    transfer as released.
 */

import type { Principal } from '@dfinity/principal';
import type { ReleaseView, Result_1 } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import {
  ZERO_BYTES32,
  decodeBool,
  decodeBytes32,
  decodeUint256,
  encodeBalanceOf,
  encodeBridgeOut,
  encodeBridgeOutUsed,
  encodeComputeTransferId,
  revertMessage,
  sameAddress,
  isBytes32,
} from './abi';
import { tryParseE8s, U64_MAX } from './amounts';
import type { BaseChainConfig } from './chains';
import {
  ethCall,
  estimateGas,
  describeWalletError,
  getAccounts,
  getChainId,
  getEthBalance,
  getGasPrice,
  getReceipt,
  isUserRejection,
  sendTransaction,
  type Eip1193Provider,
  type EvmReceipt,
} from './evmWallet';
import {
  IntentStoreError,
  intentId,
  newClientNonce,
  type BaseToIcpIntent,
  type IntentStore,
  type RandomBytes,
} from './intents';
import {
  FINALITY_NOTE,
  abortableSleep,
  deriveBaseOutTracking,
  formatBridgeError,
  pollWithBackoff,
  type BaseOutTracking,
  type Sleep,
} from './status';

// ---------------------------------------------------------------------------
// Pure: draft evaluation
// ---------------------------------------------------------------------------

export type BaseDraftState =
  | 'no_wallet'
  | 'not_connected'
  | 'wrong_network'
  | 'icp_signed_out'
  | 'invalid_amount'
  | 'below_minimum'
  | 'balance_unknown'
  | 'insufficient_balance'
  | 'preflight_failed'
  | 'insufficient_gas'
  | 'ready';

export interface BaseDraftInput {
  walletAvailable: boolean;
  account: string | null;
  chainId: number | null;
  expectedChain: Pick<BaseChainConfig, 'chainId' | 'name'>;
  /** Text form of the signed-in ICP principal, or null when signed out. */
  icpPrincipal: string | null;
  icpPrincipalIsAnonymous: boolean;
  amountText: string;
  /** BridgedIcUSD balance of the connected account. Null when it could not be read. */
  balanceE8s: bigint | null;
  /** Contract (or canister-reported) minimum for bridgeOut. */
  minimumE8s: bigint | null;
  ethBalanceWei: bigint | null;
  /** Estimated gas cost (estimate x gas price) in wei. Null when it could not be estimated. */
  gasCostWei: bigint | null;
  /** A decoded revert from `eth_estimateGas` (for example the bridge-out pause). */
  preflightError: string | null;
}

export interface BaseDraftEvaluation {
  state: BaseDraftState;
  message: string;
  amountE8s: bigint | null;
  canSend: boolean;
  /** Non-blocking notes, for example "gas could not be estimated". */
  warnings: string[];
}

export function evaluateBaseToIcpDraft(
  input: BaseDraftInput,
  format: (e8s: bigint) => string,
): BaseDraftEvaluation {
  const blocked = (state: BaseDraftState, message: string, amountE8s: bigint | null = null): BaseDraftEvaluation => ({
    state,
    message,
    amountE8s,
    canSend: false,
    warnings: [],
  });
  if (!input.walletAvailable) {
    return blocked('no_wallet', 'No Base-compatible wallet was found in this browser. Install or unlock one to continue.');
  }
  if (!input.account) return blocked('not_connected', 'Connect your Base wallet to continue.');
  if (input.chainId !== input.expectedChain.chainId) {
    return blocked('wrong_network', `Your wallet is on a different network. Switch to ${input.expectedChain.name} to continue.`);
  }
  if (!input.icpPrincipal || input.icpPrincipalIsAnonymous) {
    return blocked('icp_signed_out', 'Sign in with your ICP wallet first. The bridge releases icUSD to the account you are signed in with.');
  }
  const parsed = tryParseE8s(input.amountText);
  if (!parsed.ok) return blocked('invalid_amount', parsed.message);
  const amount = parsed.value;
  if (input.minimumE8s !== null && amount < input.minimumE8s) {
    return blocked('below_minimum', `The minimum bridge-out is ${format(input.minimumE8s)} icUSD.`, amount);
  }
  if (input.balanceE8s === null) {
    return blocked('balance_unknown', 'Could not read your icUSD balance on Base. Check your wallet connection and try again.', amount);
  }
  if (amount > input.balanceE8s) {
    return blocked('insufficient_balance', `You have ${format(input.balanceE8s)} icUSD on Base, which is less than ${format(amount)}.`, amount);
  }
  if (input.preflightError) {
    return blocked('preflight_failed', input.preflightError, amount);
  }
  if (input.ethBalanceWei !== null && input.gasCostWei !== null && input.ethBalanceWei < input.gasCostWei) {
    return blocked('insufficient_gas', 'Insufficient ETH for gas. Add a little ETH on this network, then try again.', amount);
  }
  const warnings: string[] = [];
  if (input.gasCostWei === null || input.ethBalanceWei === null) {
    warnings.push('Gas could not be estimated, so your wallet will decide whether you have enough ETH.');
  }
  return { state: 'ready', message: 'Ready to send.', amountE8s: amount, canSend: true, warnings };
}

// ---------------------------------------------------------------------------
// Pure: what to do with a stored intent after a reload
// ---------------------------------------------------------------------------

export type IntentRecovery =
  /** The transfer exists on Base (or a hash is stored): follow it through the canister. */
  | { action: 'track' }
  /** Unsent, and the right account and network are connected: it can be sent with the SAME nonce. */
  | { action: 'resume' }
  /** Unsent, but a different Base account is connected. Cannot be sent from it; may be discarded. */
  | { action: 'account_mismatch'; expected: string }
  /** Unsent, but the wallet is on another network or not connected: nothing can be checked or sent. */
  | { action: 'needs_wallet'; reason: 'no_wallet' | 'not_connected' | 'wrong_network' }
  /** `bridgeOutUsed` could not be read. */
  | { action: 'unknown' };

export function classifyUnsentIntent(
  intent: BaseToIcpIntent,
  context: {
    walletAvailable: boolean;
    account: string | null;
    chainId: number | null;
    /** Result of `bridgeOutUsed(intent.transferId)` on the intent's own chain, or null if unreadable/unchecked. */
    used: boolean | null;
  },
): IntentRecovery {
  if (intent.txHash) return { action: 'track' };
  if (context.used === true) return { action: 'track' };
  if (!context.walletAvailable) return { action: 'needs_wallet', reason: 'no_wallet' };
  if (!context.account) return { action: 'needs_wallet', reason: 'not_connected' };
  if (context.chainId !== intent.chainId) return { action: 'needs_wallet', reason: 'wrong_network' };
  if (context.used === null) return { action: 'unknown' };
  if (!sameAddress(context.account, intent.from)) return { action: 'account_mismatch', expected: intent.from };
  return { action: 'resume' };
}

// ---------------------------------------------------------------------------
// Pure: the flow state machine
// ---------------------------------------------------------------------------

export type AbortReason = 'chain_changed' | 'account_changed' | 'session_changed' | 'check_failed' | 'intent_mismatch';

export type BaseToIcpState =
  | { kind: 'draft' }
  /** The confirmation step: shows the ICP destination and the amount. */
  | { kind: 'confirming'; amountE8s: bigint }
  /** The wallet prompt is open. */
  | { kind: 'awaiting_wallet'; intent: BaseToIcpIntent }
  /** The user declined in the wallet (4001). Nothing was sent; back to the draft. */
  | { kind: 'rejected' }
  /** The send was stopped before the wallet prompt. Nothing was sent. */
  | { kind: 'aborted'; reason: AbortReason; message: string }
  /** The wallet or Base refused or failed. `uncertain` means a transaction may still exist. */
  | { kind: 'failed'; message: string; intent: BaseToIcpIntent | null; uncertain: boolean }
  | { kind: 'blocked'; message: string }
  | { kind: 'tracking'; intent: BaseToIcpIntent; tracking: BaseOutTracking };

export type BaseToIcpEvent =
  /** Open the confirmation step. `resume` is set when resending a saved unsent intent. */
  | { type: 'confirm'; amountE8s: bigint; resume?: boolean }
  | { type: 'back' }
  | { type: 'reset' }
  /** Leave a settled or uncertain state; the saved intent stays in the recovery list. */
  | { type: 'dismiss' }
  | { type: 'wallet_prompt'; intent: BaseToIcpIntent }
  | { type: 'sent'; intent: BaseToIcpIntent }
  | { type: 'already_used'; intent: BaseToIcpIntent }
  | { type: 'rejected' }
  | { type: 'aborted'; reason: AbortReason; message: string }
  | { type: 'failed'; message: string; intent: BaseToIcpIntent | null; uncertain: boolean }
  | { type: 'blocked'; message: string }
  | { type: 'track'; intent: BaseToIcpIntent }
  | { type: 'tracking_update'; tracking: BaseOutTracking }
  | { type: 'session_changed' };

export const INITIAL_BASE_TO_ICP_STATE: BaseToIcpState = { kind: 'draft' };

const AWAITING_FINALITY_AFTER_BURN: BaseOutTracking = {
  stage: 'awaiting_finality',
  label: 'Awaiting finality',
  detail: `The burn is recorded on Base. The bridge releases on the Internet Computer only after Base L1 batch finality, ${FINALITY_NOTE}.`,
  tone: 'pending',
  terminal: false,
};

const SUBMITTED: BaseOutTracking = {
  stage: 'submitted',
  label: 'Submitted to Base',
  detail: 'Waiting for Base to include the transaction. This status comes from your wallet and is only a hint.',
  tone: 'pending',
  terminal: false,
};

export function reduceBaseToIcp(state: BaseToIcpState, event: BaseToIcpEvent): BaseToIcpState {
  switch (event.type) {
    case 'confirm':
      if (state.kind !== 'draft' && state.kind !== 'rejected' && state.kind !== 'aborted' && state.kind !== 'failed' && state.kind !== 'blocked') {
        return state;
      }
      // A new transfer cannot start over an uncertain one; resuming that very intent can.
      if (state.kind === 'failed' && state.uncertain && !event.resume) return state;
      return { kind: 'confirming', amountE8s: event.amountE8s };
    case 'dismiss':
      if (state.kind === 'awaiting_wallet' || state.kind === 'tracking') return state;
      return { kind: 'draft' };
    case 'back':
      return state.kind === 'confirming' ? { kind: 'draft' } : state;
    case 'reset':
      // Leaving a prompt or an uncertain send behind would hide it, so only settled states reset.
      if (state.kind === 'awaiting_wallet') return state;
      if (state.kind === 'failed' && state.uncertain) return state;
      return { kind: 'draft' };
    case 'wallet_prompt':
      if (state.kind !== 'confirming') return state;
      return { kind: 'awaiting_wallet', intent: event.intent };
    case 'sent':
      if (state.kind !== 'awaiting_wallet' && state.kind !== 'failed') return state;
      return { kind: 'tracking', intent: event.intent, tracking: SUBMITTED };
    case 'already_used':
      return { kind: 'tracking', intent: event.intent, tracking: AWAITING_FINALITY_AFTER_BURN };
    case 'track':
      return { kind: 'tracking', intent: event.intent, tracking: event.intent.txHash ? SUBMITTED : AWAITING_FINALITY_AFTER_BURN };
    case 'rejected':
      if (state.kind !== 'awaiting_wallet') return state;
      return { kind: 'rejected' };
    case 'aborted':
      if (state.kind !== 'confirming' && state.kind !== 'awaiting_wallet') return state;
      return { kind: 'aborted', reason: event.reason, message: event.message };
    case 'failed':
      if (state.kind !== 'awaiting_wallet' && state.kind !== 'confirming') return state;
      return { kind: 'failed', message: event.message, intent: event.intent, uncertain: event.uncertain };
    case 'blocked':
      return { kind: 'blocked', message: event.message };
    case 'tracking_update':
      if (state.kind !== 'tracking') return state;
      return { ...state, tracking: event.tracking };
    case 'session_changed':
      // An open wallet prompt cannot be cancelled from here; the intent stays on disk for recovery.
      return state.kind === 'tracking' ? state : { kind: 'draft' };
    default:
      return state;
  }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

export interface BaseToIcpDeps {
  provider: Eip1193Provider;
  chain: BaseChainConfig;
  /** Lower-cased BridgedIcUSD address from `get_bridge_info`. */
  contract: string;
  /** Lower-cased connected Base account. */
  account: string;
  /** The signed-in ICP principal that will receive the icUSD. */
  icpPrincipal: Principal;
  store: IntentStore;
  reads: {
    getRelease(transferId: string): Promise<ReleaseView | null>;
  };
  bridge?: {
    notifyBaseBridgeOut(txHash: string): Promise<Result_1>;
  };
  /** Synchronous liveness check for the ICP wallet session (generation + principal). */
  isCurrent(): boolean;
  random?: RandomBytes;
  now?(): number;
  sleep?: Sleep;
}

export type BaseEmit = (event: BaseToIcpEvent) => void;

/** Everything the draft evaluation needs from Base, read through the wallet provider. */
export interface BaseSnapshot {
  balanceE8s: bigint | null;
  ethBalanceWei: bigint | null;
  gasCostWei: bigint | null;
  preflightError: string | null;
}

/**
 * Read the connected account's icUSD balance and ETH balance, and estimate the
 * gas of the exact `bridgeOut` calldata. Each read fails independently: an
 * unreadable value is `null`, never a guess.
 */
export async function readBaseSnapshot(
  deps: Pick<BaseToIcpDeps, 'provider' | 'contract' | 'account' | 'icpPrincipal' | 'random'>,
  amountE8s: bigint | null,
): Promise<BaseSnapshot> {
  const { provider, contract, account } = deps;
  const snapshot: BaseSnapshot = { balanceE8s: null, ethBalanceWei: null, gasCostWei: null, preflightError: null };
  const [balance, eth] = await Promise.allSettled([
    ethCall(provider, contract, encodeBalanceOf(account)).then(decodeUint256),
    getEthBalance(provider, account),
  ]);
  if (balance.status === 'fulfilled') snapshot.balanceE8s = balance.value;
  if (eth.status === 'fulfilled') snapshot.ethBalanceWei = eth.value;
  if (amountE8s !== null && amountE8s > 0n && amountE8s <= U64_MAX) {
    try {
      const data = encodeBridgeOut({
        amountE8s,
        icpPrincipal: deps.icpPrincipal.toUint8Array(),
        icpSubaccount: ZERO_BYTES32,
        clientNonce: newClientNonce(deps.random),
      });
      const [gas, price] = await Promise.all([estimateGas(provider, { from: account, to: contract, data }), getGasPrice(provider)]);
      snapshot.gasCostWei = gas * price;
    } catch (error) {
      const decoded = revertMessage(error);
      if (decoded) snapshot.preflightError = decoded;
      // Any other failure just leaves the gas cost unknown.
    }
  }
  return snapshot;
}

export type SendOutcome =
  | { kind: 'sent'; intent: BaseToIcpIntent; txHash: string }
  | { kind: 'already_used'; intent: BaseToIcpIntent }
  | { kind: 'rejected' }
  | { kind: 'aborted'; reason: AbortReason }
  | { kind: 'failed'; message: string; uncertain: boolean; intent: BaseToIcpIntent | null }
  | { kind: 'blocked'; message: string };

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error ?? 'unknown error');
}

/** Read `bridgeOutUsed(transferId)` on the contract and chain the intent was created for. */
export async function readTransferUsed(provider: Eip1193Provider, contract: string, transferId: string): Promise<boolean> {
  return decodeBool(await ethCall(provider, contract, encodeBridgeOutUsed(transferId)));
}

/**
 * Create (or resume) a bridge-out and ask the wallet to send it.
 *
 * `resume` is an unsent intent found after a reload; it is sent with the SAME
 * nonce. For a new transfer the nonce and transfer id are created and persisted
 * first.
 */
/** Sends that are between "user pressed send" and "wallet answered", keyed by chain, contract and account. */
const sendsInFlight = new Set<string>();

export async function sendBridgeOut(
  deps: BaseToIcpDeps,
  request: { amountE8s: bigint; resume?: BaseToIcpIntent },
  emit: BaseEmit,
): Promise<SendOutcome> {
  // A second press (or a second component) must not start a second transfer while one is being prepared or signed.
  const flightKey = `${deps.chain.chainId}:${deps.contract}:${deps.account}`.toLowerCase();
  if (sendsInFlight.has(flightKey)) {
    emit({ type: 'blocked', message: 'A transfer from this account is already being prepared. Wait for it to finish.' });
    return { kind: 'blocked', message: 'send already in flight' };
  }
  sendsInFlight.add(flightKey);
  try {
    return await sendBridgeOutOnce(deps, request, emit);
  } finally {
    sendsInFlight.delete(flightKey);
  }
}

async function sendBridgeOutOnce(
  deps: BaseToIcpDeps,
  request: { amountE8s: bigint; resume?: BaseToIcpIntent },
  emit: BaseEmit,
): Promise<SendOutcome> {
  const { provider, chain, contract, account } = deps;
  const abort = (reason: AbortReason, message: string, intent?: BaseToIcpIntent): SendOutcome => {
    if (intent && !request.resume) forget(deps, intent);
    emit({ type: 'aborted', reason, message });
    return { kind: 'aborted', reason };
  };

  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return { kind: 'aborted', reason: 'session_changed' };
  }

  if (deps.icpPrincipal.isAnonymous()) {
    return abort('check_failed', 'Sign in with your ICP wallet first. The anonymous principal cannot receive icUSD.');
  }

  let intent: BaseToIcpIntent;
  if (request.resume) {
    intent = request.resume;
    if (
      !sameAddress(intent.from, account) ||
      intent.chainId !== chain.chainId ||
      !sameAddress(intent.contract, contract) ||
      intent.destinationPrincipal !== deps.icpPrincipal.toText()
    ) {
      return abort('intent_mismatch', 'This unsent transfer belongs to a different account, network or ICP principal and cannot be sent from here.');
    }
  } else {
    // 1. Nonce and the contract's own transfer id, both before anything is persisted or signed.
    const clientNonce = newClientNonce(deps.random);
    let transferId: string;
    try {
      transferId = decodeBytes32(await ethCall(provider, contract, encodeComputeTransferId(account, clientNonce)));
    } catch (error) {
      emit({ type: 'blocked', message: `Could not read the transfer id from Base: ${describeWalletError(error)}` });
      return { kind: 'blocked', message: 'transfer id unavailable' };
    }
    intent = {
      v: 1,
      kind: 'base_to_icp',
      clientNonce,
      transferId,
      from: account,
      amountE8s: request.amountE8s,
      destinationPrincipal: deps.icpPrincipal.toText(),
      chainId: chain.chainId,
      contract,
      createdAt: (deps.now ?? Date.now)(),
      status: 'prepared',
    };
    // 2. Persist BEFORE the wallet prompt. If this fails nothing is sent.
    try {
      deps.store.put(intent);
    } catch (error) {
      const message = error instanceof IntentStoreError ? error.message : errorText(error);
      emit({ type: 'blocked', message });
      return { kind: 'blocked', message };
    }
  }

  emit({ type: 'wallet_prompt', intent });

  // 3. Re-check the world immediately before sending.
  let calldata: string;
  try {
    calldata = encodeBridgeOut({
      amountE8s: intent.amountE8s,
      icpPrincipal: deps.icpPrincipal.toUint8Array(),
      icpSubaccount: ZERO_BYTES32,
      clientNonce: intent.clientNonce,
    });
  } catch (error) {
    return abort('check_failed', errorText(error), intent);
  }
  try {
    const [liveChain, liveAccounts] = await Promise.all([getChainId(provider), getAccounts(provider)]);
    if (liveChain !== chain.chainId) {
      return abort('chain_changed', `Your wallet moved to another network. Switch back to ${chain.name} and review the transfer again.`, intent);
    }
    if (!sameAddress(liveAccounts[0], intent.from)) {
      return abort('account_changed', 'The selected Base account changed. Reconnect the account you started with and review the transfer again.', intent);
    }
    if (!deps.isCurrent()) {
      emit({ type: 'session_changed' });
      if (!request.resume) forget(deps, intent);
      return { kind: 'aborted', reason: 'session_changed' };
    }
    const used = await readTransferUsed(provider, intent.contract, intent.transferId);
    if (used) {
      const sent = markSent(deps, intent, undefined);
      emit({ type: 'already_used', intent: sent });
      return { kind: 'already_used', intent: sent };
    }
  } catch (error) {
    return abort('check_failed', `Could not verify the transfer before sending: ${describeWalletError(error)}`, intent);
  }

  // 4. The wallet prompt.
  let txHash: string;
  try {
    txHash = await sendTransaction(provider, { from: account, to: contract, data: calldata });
  } catch (error) {
    if (isUserRejection(error)) {
      if (!request.resume) forget(deps, intent);
      emit({ type: 'rejected' });
      return { kind: 'rejected' };
    }
    const reverted = revertMessage(error);
    if (reverted) {
      // The wallet's own simulation said the call would revert, so nothing was broadcast.
      if (!request.resume) forget(deps, intent);
      emit({ type: 'failed', message: reverted, intent: null, uncertain: false });
      return { kind: 'failed', message: reverted, uncertain: false, intent: null };
    }
    const message = `${describeWalletError(error, 'The wallet did not confirm the transaction.')} Check your wallet activity before sending again. This transfer is saved on this device and can be checked or resumed.`;
    emit({ type: 'failed', message, intent, uncertain: true });
    return { kind: 'failed', message, uncertain: true, intent };
  }

  const sent = markSent(deps, intent, txHash);
  emit({ type: 'sent', intent: sent });
  return { kind: 'sent', intent: sent, txHash };
}

function forget(deps: BaseToIcpDeps, intent: BaseToIcpIntent): void {
  try {
    deps.store.remove(intentId(intent));
  } catch {
    // A leftover unsent record is resolved by the recovery check on the next visit.
  }
}

function markSent(deps: BaseToIcpDeps, intent: BaseToIcpIntent, txHash: string | undefined): BaseToIcpIntent {
  try {
    return deps.store.patch<BaseToIcpIntent>(intentId(intent), { status: 'sent', ...(txHash ? { txHash } : {}) });
  } catch {
    // The wallet already has the transaction; the hash is a hint kept for convenience.
    return { ...intent, status: 'sent', ...(txHash ? { txHash } : {}) };
  }
}

export type OnChainCheck = 'used' | 'unused' | 'unreadable' | 'wrong_network' | 'no_wallet';

/** Check an intent against the contract it was created for. Needs the wallet on the intent's chain. */
export async function checkIntentOnChain(provider: Eip1193Provider | null, intent: BaseToIcpIntent): Promise<OnChainCheck> {
  if (!provider) return 'no_wallet';
  try {
    if ((await getChainId(provider)) !== intent.chainId) return 'wrong_network';
    return (await readTransferUsed(provider, intent.contract, intent.transferId)) ? 'used' : 'unused';
  } catch {
    return 'unreadable';
  }
}

export type DiscardResult = 'discarded' | 'sent_on_chain' | 'cannot_verify';

/**
 * Forget an UNSENT intent. It is removed only after `bridgeOutUsed(transferId)`
 * shows, on the intent's own chain, that the burn never happened. A sent intent
 * (stored hash, or used on chain) is never discarded.
 */
export async function discardUnsentIntent(
  deps: Pick<BaseToIcpDeps, 'store'> & { provider: Eip1193Provider | null },
  intent: BaseToIcpIntent,
): Promise<DiscardResult> {
  if (intent.txHash || intent.status !== 'prepared') return 'sent_on_chain';
  const check = await checkIntentOnChain(deps.provider, intent);
  if (check === 'used') return 'sent_on_chain';
  if (check !== 'unused') return 'cannot_verify';
  deps.store.remove(intentId(intent));
  return 'discarded';
}

/**
 * Follow a transfer: the wallet's receipt (a hint) plus the canister's release
 * record (the authority), until the canister shows a final status.
 */
export async function trackBaseToIcp(
  deps: {
    provider: Eip1193Provider | null;
    reads: BaseToIcpDeps['reads'];
    store: IntentStore;
    sleep?: Sleep;
  },
  intent: BaseToIcpIntent,
  onUpdate: (tracking: BaseOutTracking) => void,
  signal?: AbortSignal,
  onError?: (error: unknown) => void,
): Promise<'done' | 'aborted' | 'exhausted'> {
  let receipt: EvmReceipt | null = null;
  let used: boolean | null = null;
  return pollWithBackoff<BaseOutTracking>({
    fetch: async () => {
      const release = await deps.reads.getRelease(intent.transferId);
      const provider = deps.provider;
      // Hints from the wallet. A failure here must never hide the canister's answer.
      if (provider && !release) {
        if (intent.txHash && !receipt) {
          try {
            if ((await getChainId(provider)) === intent.chainId) receipt = await getReceipt(provider, intent.txHash);
          } catch {
            receipt = null;
          }
        }
        if (!receipt && used !== true) used = (await checkIntentOnChain(provider, intent)) === 'used' ? true : used;
      }
      return deriveBaseOutTracking({ receipt, release, transferUsed: used });
    },
    isDone: (tracking) => tracking.terminal,
    onUpdate: (tracking) => {
      onUpdate(tracking);
      if (tracking.terminal && intent.status !== 'done') {
        try {
          deps.store.patch<BaseToIcpIntent>(intentId(intent), { status: 'done' });
        } catch {
          // Cosmetic.
        }
      }
    },
    onError,
    sleep: deps.sleep ?? abortableSleep,
    signal,
  });
}

/**
 * Optional, separate user click: tell the bridge which Base transaction to look
 * at sooner. It moves no funds; settlement still waits for the finalized log.
 * For Oisy this is an ICP update call and opens a prompt, so call it directly
 * from the click handler.
 */
export async function notifyBridge(
  deps: Pick<BaseToIcpDeps, 'bridge' | 'store' | 'isCurrent'>,
  intent: BaseToIcpIntent,
): Promise<{ ok: true; intent: BaseToIcpIntent } | { ok: false; message: string }> {
  if (!deps.bridge) return { ok: false, message: 'The bridge is not available.' };
  if (!intent.txHash || !isBytes32(intent.txHash)) return { ok: false, message: 'There is no transaction hash to report yet.' };
  if (!deps.isCurrent()) return { ok: false, message: 'Your wallet session changed. Nothing was submitted.' };
  try {
    const result = await deps.bridge.notifyBaseBridgeOut(intent.txHash);
    if ('Err' in result) return { ok: false, message: formatBridgeError(result.Err) };
    let updated = intent;
    try {
      updated = deps.store.patch<BaseToIcpIntent>(intentId(intent), { notified: true });
    } catch {
      updated = { ...intent, notified: true };
    }
    return { ok: true, intent: updated };
  } catch (error) {
    return { ok: false, message: `The bridge did not confirm the notification: ${errorText(error)}` };
  }
}
