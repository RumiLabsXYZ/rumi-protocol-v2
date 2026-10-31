/**
 * ICP -> Base: icUSD on the Internet Computer to BridgedIcUSD on Base.
 *
 * Two layers:
 *  - a pure state machine (`reduceIcpToBase`, `buildQuote`, `validateRecipient`,
 *    `checkQuote`) that the specs exercise exhaustively, and
 *  - a thin effect runner (`prepareIcpToBase`, `startIcpToBase`,
 *    `bridgeAfterApproval`, `retryIcpToBase`, `resumeIcpToBase`, `trackDeposit`,
 *    `requestDepositRefund`) that talks to the ledger and the bridge canister
 *    through injected deps.
 *
 * Safety properties:
 *  - The intent (with its client request id) is persisted BEFORE the first
 *    signer call, and every signer call is issued synchronously inside its click
 *    handler, with no await in front of it.
 *  - Popup-signer wallets (Oisy) only open their popup from a direct user
 *    gesture, and only for the FIRST signer call of that gesture: a second
 *    signer call after an await fails with "Signer window should not be opened
 *    outside of click handler". So when an approval is needed and
 *    `deps.separateApproval` is set, one click performs only `icrc2_approve`, the
 *    flow stops in the `approved` state, and a second click performs
 *    `bridge_to_base` as the first signer call of that click. The intent and its
 *    client request id are persisted (status `approved`) across both clicks and
 *    across a reload. Delegation wallets (Internet Identity, Plug) have no popup
 *    and keep the single click path: approve, then `bridge_to_base`.
 *  - An approval is requested only when the current allowance is short, for
 *    exactly `amount + ledger fee`, with a short expiry.
 *  - An uncertain outcome (thrown error, lost reply, `Internal`/`Rpc` errors)
 *    is resolved by looking the request up with `get_deposit_by_request`. A manual
 *    retry reuses the SAME id, so the canister returns the existing deposit
 *    instead of creating a second one. Nothing is ever re-sent automatically.
 *  - A wallet or signer is asked only after `assertCanSign` (through
 *    `signingBlockMessage`) agrees: a disabled destination, in either direction,
 *    gets no signing request at all. The check is synchronous, so it costs a
 *    popup-signer's click nothing. A paused or halted bridge is NOT a disabled
 *    destination; that stays the form's `bridgeBlocked` and the canister's own answer.
 *  - The canister is the settlement authority; this module only displays it.
 */

import type { Principal } from '@dfinity/principal';
import type {
  BridgeError,
  BridgeToBaseArgs,
  DepositView,
  Result_3,
} from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import type { ApproveArgs, ApproveResult } from '$declarations/icusd_ledger/icusd_ledger.did';
import { bytesToHex, hexToBytes, isAddress, isZeroAddress } from './abi';
import { formatE8s, tryParseE8s } from './amounts';
import type { BridgeConfig } from './chains';
import {
  CHECKSUM_ERROR_MESSAGE,
  CHECKSUM_UNVERIFIABLE_MESSAGE,
  checksumStatus,
  type Keccak256,
} from './eip55';
import {
  INTENT_VERSION,
  IntentStoreError,
  intentDestination,
  intentId,
  isResolved,
  newClientRequestId,
  type IcpToBaseIntent,
  type IntentStore,
  type RandomBytes,
} from './intents';
import { intentBindingMessage, signingBlockMessage, type SigningContext } from './signingGuard';
import {
  abortableSleep,
  bridgeErrorKind,
  classifyBridgeError,
  describeDeposit,
  formatApproveError,
  formatBridgeError,
  pollWithBackoff,
  type Sleep,
} from './status';

export const APPROVAL_TTL_MS = 30 * 60 * 1000;
/** An existing allowance that expires sooner than this is replaced rather than relied on. */
export const ALLOWANCE_EXPIRY_MARGIN_MS = 10 * 60 * 1000;
/** A quote older than this must be refreshed before the user can confirm it. */
export const QUOTE_MAX_AGE_MS = 2 * 60 * 1000;

// ---------------------------------------------------------------------------
// Pure: recipient and quote
// ---------------------------------------------------------------------------

export type RecipientCheck =
  | { ok: true; address: string }
  | { ok: false; code: 'empty' | 'format' | 'checksum' | 'checksum_unverifiable' | 'zero' | 'contract'; message: string };

/**
 * Validate a Base recipient: 0x + 40 hex, a valid EIP-55 checksum when the hex
 * mixes cases (all-lowercase and all-uppercase are accepted as unchecksummed),
 * not the zero address, not the bridge token contract. The accepted address is
 * returned lowercase, which is what is stored and sent to the canister.
 * `hash` overrides the app's Keccak-256 (specs only).
 */
export function validateRecipient(input: string, contract: string | null, hash?: Keccak256 | null): RecipientCheck {
  const text = (input ?? '').trim();
  if (text === '') return { ok: false, code: 'empty', message: 'Enter the Base address that should receive the icUSD.' };
  if (!isAddress(text)) {
    return { ok: false, code: 'format', message: 'A Base address is 0x followed by 40 hexadecimal characters.' };
  }
  const checksum = checksumStatus(text, hash);
  if (checksum === 'invalid') return { ok: false, code: 'checksum', message: CHECKSUM_ERROR_MESSAGE };
  if (checksum === 'unverifiable') {
    return { ok: false, code: 'checksum_unverifiable', message: CHECKSUM_UNVERIFIABLE_MESSAGE };
  }
  if (isZeroAddress(text)) {
    return { ok: false, code: 'zero', message: 'The zero address cannot receive icUSD.' };
  }
  if (contract && text.toLowerCase() === contract.toLowerCase()) {
    return { ok: false, code: 'contract', message: 'That is the icUSD token contract itself, not a wallet. Use your own Base address.' };
  }
  return { ok: true, address: text.toLowerCase() };
}

export interface IcpToBaseQuote {
  amountE8s: bigint;
  recipient: string;
  /** Ledger fee charged on top of the amount by the bridge's `transfer_from`. */
  ledgerFeeE8s: bigint;
  /** Ledger fee for the approval itself; zero when no approval is needed. */
  approvalFeeE8s: bigint;
  /** Bridge deposit fee, deducted from the minted amount. */
  depositFeeE8s: bigint;
  /** What arrives on Base: amount minus the deposit fee. */
  mintAmountE8s: bigint;
  /** Allowance the bridge needs: amount plus the ledger fee. */
  requiredAllowanceE8s: bigint;
  currentAllowanceE8s: bigint;
  allowanceExpiresAtNs: bigint | null;
  needsApproval: boolean;
  /** Everything leaving the user's icUSD balance: amount + ledger fee + approval fee. */
  totalDebitE8s: bigint;
  balanceE8s: bigint | null;
  createdAt: number;
}

export interface QuoteInputs {
  amountE8s: bigint;
  recipient: string;
  ledgerFeeE8s: bigint;
  depositFeeE8s: bigint;
  allowanceE8s: bigint;
  allowanceExpiresAtNs: bigint | null;
  balanceE8s: bigint | null;
  nowMs: number;
}

/** The ledger fee to plan around: the larger of the live ledger fee and the bridge's cached one. */
export function chooseLedgerFee(liveFeeE8s: bigint, bridgeFeeE8s: bigint | null): bigint {
  return bridgeFeeE8s !== null && bridgeFeeE8s > liveFeeE8s ? bridgeFeeE8s : liveFeeE8s;
}

/** Whether a fresh `icrc2_approve` is needed before the bridge can pull `requiredE8s`. */
export function allowanceNeedsApproval(
  allowanceE8s: bigint,
  expiresAtNs: bigint | null,
  requiredE8s: bigint,
  nowMs: number,
): boolean {
  if (allowanceE8s < requiredE8s) return true;
  if (expiresAtNs !== null) {
    const safeUntilNs = BigInt(nowMs + ALLOWANCE_EXPIRY_MARGIN_MS) * 1_000_000n;
    if (expiresAtNs <= safeUntilNs) return true;
  }
  return false;
}

export function buildQuote(input: QuoteInputs): IcpToBaseQuote {
  const requiredAllowanceE8s = input.amountE8s + input.ledgerFeeE8s;
  const needsApproval = allowanceNeedsApproval(
    input.allowanceE8s,
    input.allowanceExpiresAtNs,
    requiredAllowanceE8s,
    input.nowMs,
  );
  const approvalFeeE8s = needsApproval ? input.ledgerFeeE8s : 0n;
  const mintAmountE8s = input.amountE8s > input.depositFeeE8s ? input.amountE8s - input.depositFeeE8s : 0n;
  return {
    amountE8s: input.amountE8s,
    recipient: input.recipient,
    ledgerFeeE8s: input.ledgerFeeE8s,
    approvalFeeE8s,
    depositFeeE8s: input.depositFeeE8s,
    mintAmountE8s,
    requiredAllowanceE8s,
    currentAllowanceE8s: input.allowanceE8s,
    allowanceExpiresAtNs: input.allowanceExpiresAtNs,
    needsApproval,
    totalDebitE8s: requiredAllowanceE8s + approvalFeeE8s,
    balanceE8s: input.balanceE8s,
    createdAt: input.nowMs,
  };
}

export type QuoteProblemCode =
  | 'below_minimum'
  | 'above_maximum'
  | 'fee_exceeds_amount'
  | 'insufficient_balance'
  | 'paused'
  | 'halted';

export interface QuoteProblem {
  code: QuoteProblemCode;
  message: string;
}

export interface QuoteLimits {
  minDepositE8s: bigint;
  maxDepositE8s: bigint;
  depositsPaused: boolean;
  halted: string | null;
}

/** First blocking problem with a quote, or null. Checked in the order a user would fix them. */
export function checkQuote(
  quote: IcpToBaseQuote,
  limits: QuoteLimits,
  format: (e8s: bigint) => string,
): QuoteProblem | null {
  if (limits.halted) {
    return { code: 'halted', message: `The bridge is halted for safety: ${limits.halted}` };
  }
  if (limits.depositsPaused) {
    return { code: 'paused', message: 'Deposits to Base are paused right now. Your icUSD has not moved.' };
  }
  if (quote.amountE8s < limits.minDepositE8s) {
    return { code: 'below_minimum', message: `The minimum transfer is ${format(limits.minDepositE8s)} icUSD.` };
  }
  if (quote.amountE8s > limits.maxDepositE8s) {
    return { code: 'above_maximum', message: `The maximum per transfer is ${format(limits.maxDepositE8s)} icUSD.` };
  }
  if (quote.mintAmountE8s <= 0n) {
    return { code: 'fee_exceeds_amount', message: 'The amount must be larger than the bridge fee.' };
  }
  if (quote.balanceE8s !== null && quote.balanceE8s < quote.totalDebitE8s) {
    return {
      code: 'insufficient_balance',
      message: `You need ${format(quote.totalDebitE8s)} icUSD (amount plus ledger fees) but have ${format(quote.balanceE8s)} icUSD.`,
    };
  }
  return null;
}

export function isQuoteStale(quote: IcpToBaseQuote, nowMs: number): boolean {
  return nowMs - quote.createdAt > QUOTE_MAX_AGE_MS;
}

// ---------------------------------------------------------------------------
// Pure: the flow state machine
// ---------------------------------------------------------------------------

export type IcpToBaseState =
  | { kind: 'draft' }
  | { kind: 'quoting' }
  | { kind: 'review'; quote: IcpToBaseQuote; problem: QuoteProblem | null }
  /** The canister reported a different deposit fee. The user must review and confirm the new one. */
  | { kind: 'fee_changed'; quote: IcpToBaseQuote; currentFeeE8s: bigint }
  | { kind: 'approving'; quote: IcpToBaseQuote; intent: IcpToBaseIntent }
  /**
   * The approval is confirmed and the transfer has NOT been sent. Waiting for a
   * second click (popup-signer wallets). `quote` is null after a reload.
   */
  | { kind: 'approved'; quote: IcpToBaseQuote | null; intent: IcpToBaseIntent }
  /** Approval declined or failed. No funds moved. */
  | { kind: 'approval_failed'; quote: IcpToBaseQuote; message: string; declined: boolean }
  | { kind: 'submitting'; quote: IcpToBaseQuote | null; intent: IcpToBaseIntent }
  /** The canister rejected the request before creating anything. */
  /** `reapprove` is set when the allowance is the problem: review again to approve anew. */
  | { kind: 'rejected'; quote: IcpToBaseQuote | null; message: string; reapprove: boolean }
  /** Outcome unknown. `lookup` says whether the request could be found by its id. */
  | {
      kind: 'uncertain';
      intent: IcpToBaseIntent;
      message: string;
      lookup: 'looking' | 'not_found' | 'unavailable';
    }
  | { kind: 'tracking'; intent: IcpToBaseIntent; deposit: DepositView }
  | { kind: 'session_changed' }
  | { kind: 'error'; message: string };

export type IcpToBaseEvent =
  | { type: 'edit' }
  | { type: 'reset' }
  | { type: 'quote_started' }
  | { type: 'quote_ready'; quote: IcpToBaseQuote; problem: QuoteProblem | null }
  | { type: 'quote_failed'; message: string }
  | { type: 'confirmed'; quote: IcpToBaseQuote | null; intent: IcpToBaseIntent; needsApproval: boolean }
  /** Approval confirmed and the transfer is sent in the same click (delegation wallets). */
  | { type: 'approval_ok' }
  /** Approval confirmed; the transfer waits for a second click (popup-signer wallets). */
  | { type: 'approval_confirmed' }
  /** The second click: the transfer request is being sent. */
  | { type: 'bridge_started' }
  /** Reload recovery: an earlier request is being looked up. */
  | { type: 'resume_started'; intent: IcpToBaseIntent }
  /** Reload recovery: the approval was granted earlier and the bridge has no deposit yet. */
  | { type: 'resume_approved'; intent: IcpToBaseIntent }
  | { type: 'approval_failed'; message: string; declined: boolean }
  | { type: 'submit_ok'; deposit: DepositView }
  | { type: 'submit_fee_changed'; currentFeeE8s: number | bigint }
  | { type: 'submit_rejected'; message: string; reapprove?: boolean }
  | { type: 'submit_uncertain'; message: string }
  | { type: 'lookup_found'; deposit: DepositView }
  | { type: 'lookup_not_found' }
  | { type: 'lookup_unavailable' }
  | { type: 'retry' }
  | { type: 'deposit_updated'; deposit: DepositView }
  /** Follow a deposit that is already known (found in the intent journal after a reload). */
  | { type: 'track_existing'; intent: IcpToBaseIntent; deposit: DepositView }
  | { type: 'session_changed' }
  /** The flow could not start (storage unavailable, an unresolved earlier request). Nothing was sent. */
  | { type: 'blocked'; message: string };

export const INITIAL_ICP_TO_BASE_STATE: IcpToBaseState = { kind: 'draft' };

export function reduceIcpToBase(state: IcpToBaseState, event: IcpToBaseEvent): IcpToBaseState {
  switch (event.type) {
    case 'reset':
    case 'edit':
      // Editing is only possible while nothing is in flight. An approved request must be bridged or discarded.
      if (state.kind === 'approving' || state.kind === 'submitting' || state.kind === 'approved') return state;
      if (state.kind === 'uncertain' && event.type === 'edit') return state;
      return { kind: 'draft' };
    case 'session_changed':
      return { kind: 'session_changed' };
    case 'quote_started':
      if (state.kind === 'approving' || state.kind === 'submitting' || state.kind === 'uncertain' || state.kind === 'approved') return state;
      return { kind: 'quoting' };
    case 'quote_ready':
      if (state.kind !== 'quoting') return state;
      return { kind: 'review', quote: event.quote, problem: event.problem };
    case 'quote_failed':
      if (state.kind !== 'quoting') return state;
      return { kind: 'error', message: event.message };
    case 'confirmed':
      if (state.kind === 'approving' || state.kind === 'submitting' || state.kind === 'approved') return state;
      return event.needsApproval && event.quote
        ? { kind: 'approving', quote: event.quote, intent: event.intent }
        : { kind: 'submitting', quote: event.quote, intent: event.intent };
    case 'approval_ok':
      if (state.kind !== 'approving') return state;
      return { kind: 'submitting', quote: state.quote, intent: state.intent };
    case 'approval_confirmed':
      if (state.kind !== 'approving') return state;
      return { kind: 'approved', quote: state.quote, intent: state.intent };
    case 'bridge_started':
      if (state.kind !== 'approved') return state;
      return { kind: 'submitting', quote: state.quote, intent: state.intent };
    case 'resume_started':
      if (state.kind === 'approving' || state.kind === 'submitting' || state.kind === 'approved') return state;
      return { kind: 'uncertain', intent: event.intent, message: 'This request was started on this device earlier and has no recorded outcome.', lookup: 'looking' };
    case 'resume_approved':
      if (state.kind !== 'uncertain') return state;
      return { kind: 'approved', quote: null, intent: event.intent };
    case 'approval_failed':
      if (state.kind !== 'approving') return state;
      return { kind: 'approval_failed', quote: state.quote, message: event.message, declined: event.declined };
    case 'submit_ok':
      if (state.kind !== 'submitting' && state.kind !== 'uncertain') return state;
      return { kind: 'tracking', intent: state.intent, deposit: event.deposit };
    case 'submit_fee_changed':
      if (state.kind !== 'submitting') return state;
      if (!state.quote) {
        return {
          kind: 'error',
          message: `The bridge deposit fee changed to ${formatE8s(BigInt(event.currentFeeE8s))} icUSD. No icUSD moved. Review the transfer again to see the new amounts.`,
        };
      }
      return {
        kind: 'fee_changed',
        quote: state.quote,
        currentFeeE8s: BigInt(event.currentFeeE8s),
      };
    case 'submit_rejected':
      if (state.kind !== 'submitting') return state;
      return { kind: 'rejected', quote: state.quote, message: event.message, reapprove: event.reapprove === true };
    case 'submit_uncertain':
      if (state.kind !== 'submitting') return state;
      return { kind: 'uncertain', intent: state.intent, message: event.message, lookup: 'looking' };
    case 'lookup_found':
      if (state.kind !== 'uncertain') return state;
      return { kind: 'tracking', intent: state.intent, deposit: event.deposit };
    case 'lookup_not_found':
      if (state.kind !== 'uncertain') return state;
      return { ...state, lookup: 'not_found' };
    case 'lookup_unavailable':
      if (state.kind !== 'uncertain') return state;
      return { ...state, lookup: 'unavailable' };
    case 'retry':
      if (state.kind !== 'uncertain' || state.lookup === 'looking') return state;
      // Same intent, therefore the same client request id.
      return { kind: 'submitting', quote: null, intent: state.intent };
    case 'deposit_updated':
      if (state.kind !== 'tracking') return state;
      return { ...state, deposit: event.deposit };
    case 'track_existing':
      if (state.kind === 'approving' || state.kind === 'submitting' || state.kind === 'uncertain' || state.kind === 'approved') return state;
      return { kind: 'tracking', intent: event.intent, deposit: event.deposit };
    case 'blocked':
      return { kind: 'error', message: event.message };
    default:
      return state;
  }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

export interface IcpToBaseConfig {
  depositFeeE8s: bigint;
  /** The bridge canister's own cached ledger fee, when it has one. */
  bridgeLedgerFeeE8s: bigint | null;
  limits: QuoteLimits;
}

/** Read-only dependencies used to build a quote. All reads are anonymous queries. */
export interface PrepareDeps {
  /** Fresh bridge configuration (deposit fee, limits, pauses). */
  fetchConfig(): Promise<IcpToBaseConfig>;
  /** Live icUSD ledger fee. Must throw on failure: an approval is never sized from a guess. */
  fetchLedgerFee(): Promise<bigint>;
  /** Anonymous `icrc2_allowance` for (user, bridge canister). */
  readAllowance(): Promise<{ allowance: bigint; expiresAtNs: bigint | null }>;
  readBalance(): Promise<bigint>;
  now(): number;
}

export async function prepareIcpToBase(
  deps: PrepareDeps,
  request: { amountE8s: bigint; recipient: string },
  format: (e8s: bigint) => string,
): Promise<{ quote: IcpToBaseQuote; problem: QuoteProblem | null; limits: QuoteLimits }> {
  const [config, liveFee, allowance, balance] = await Promise.all([
    deps.fetchConfig(),
    deps.fetchLedgerFee(),
    deps.readAllowance(),
    deps.readBalance(),
  ]);
  const quote = buildQuote({
    amountE8s: request.amountE8s,
    recipient: request.recipient,
    ledgerFeeE8s: chooseLedgerFee(liveFee, config.bridgeLedgerFeeE8s),
    depositFeeE8s: config.depositFeeE8s,
    allowanceE8s: allowance.allowance,
    allowanceExpiresAtNs: allowance.expiresAtNs,
    balanceE8s: balance,
    nowMs: deps.now(),
  });
  return { quote, problem: checkQuote(quote, config.limits, format), limits: config.limits };
}

/** Signing dependencies. Every method under `ledger` and `bridge` may open a wallet prompt. */
export interface IcpToBaseDeps {
  /** The signed-in ICP principal (the deposit owner). */
  owner: Principal;
  store: IntentStore;
  bridgeCanister: Principal;
  ledger: {
    approve(args: ApproveArgs): Promise<ApproveResult>;
  };
  bridge: {
    bridgeToBase(args: BridgeToBaseArgs): Promise<Result_3>;
    requestDepositRefund(id: bigint): Promise<Result_3>;
  };
  /** Public queries made with an anonymous actor, so they work for every wallet type (Oisy included). */
  reads: {
    getDeposit(id: bigint): Promise<DepositView | null>;
    /** `get_deposit_by_request(owner, client_request_id)`. */
    getDepositByRequest(owner: Principal, clientRequestId: Uint8Array): Promise<DepositView | null>;
  };
  /**
   * True for popup-signer wallets (Oisy): every signer call must be the first
   * signer call of its own click, so the approval and the transfer are two clicks.
   * False for delegation wallets (Internet Identity, Plug): one click does both.
   */
  separateApproval: boolean;
  /** Synchronous liveness check: same wallet session and same principal as when the action began. */
  isCurrent(): boolean;
  /** The destination this action is for, and its newest known status. */
  signing: SigningContext;
  random?: RandomBytes;
  now?(): number;
  sleep?: Sleep;
}

export type Emit = (event: IcpToBaseEvent) => void;

const DECLINED = /reject|denied|declin|cancel|closed|dismiss/i;

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') {
    return (error as { message: string }).message;
  }
  return String(error ?? 'unknown error');
}

export function bridgeToBaseArgs(intent: IcpToBaseIntent): BridgeToBaseArgs {
  return {
    amount_e8s: intent.amountE8s,
    base_recipient: intent.recipient,
    from_subaccount: [],
    client_request_id: hexToBytes(intent.clientRequestId),
    expected_deposit_fee_e8s: intent.expectedDepositFee,
  };
}

/** True while an earlier ICP -> Base request has no recorded outcome. */
export function hasUnresolvedIntent(store: IntentStore): boolean {
  return unresolvedIntents(store).length > 0;
}

/**
 * Requests with no recorded outcome: not yet sent (`prepared`) or approved but not yet bridged (`approved`).
 * Pass a destination id to list only that destination's requests (legacy requests count as Base). The
 * guard in `startIcpToBase` always looks at all of them.
 */
export function unresolvedIntents(store: IntentStore, destinationId?: string): IcpToBaseIntent[] {
  return store
    .list()
    .filter(
      (intent): intent is IcpToBaseIntent =>
        intent.kind === 'icp_to_base' &&
        (intent.status === 'prepared' || intent.status === 'approved') &&
        (destinationId === undefined || intentDestination(intent) === destinationId),
    );
}

/**
 * Entry point for the confirm button. Call it directly from the click handler.
 *
 * Everything before the first signer call is synchronous: the intent is saved
 * (localStorage is synchronous) and the first ledger or bridge call is issued
 * without an await in front of it. With `deps.separateApproval` and an approval
 * needed, this click ends after `icrc2_approve`; `bridgeAfterApproval` is the
 * second click.
 */
export function startIcpToBase(deps: IcpToBaseDeps, quote: IcpToBaseQuote, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  // Before the intent is saved and before any signer call: a disabled destination leaves no trace.
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  if (hasUnresolvedIntent(deps.store)) {
    emit({
      type: 'blocked',
      message: 'An earlier bridge request on this device has no recorded outcome yet. Resolve or discard it first.',
    });
    return Promise.resolve();
  }
  const intent: IcpToBaseIntent = {
    v: INTENT_VERSION,
    destination: deps.signing.destination.id,
    kind: 'icp_to_base',
    clientRequestId: newClientRequestId(deps.random),
    amountE8s: quote.amountE8s,
    recipient: quote.recipient,
    expectedDepositFee: quote.depositFeeE8s,
    createdAt: (deps.now ?? Date.now)(),
    status: 'prepared',
  };
  try {
    deps.store.put(intent);
  } catch (error) {
    emit({ type: 'blocked', message: error instanceof IntentStoreError ? error.message : errorMessage(error) });
    return Promise.resolve();
  }
  emit({ type: 'confirmed', quote, intent, needsApproval: quote.needsApproval });
  return continueAfterConfirm(deps, quote, intent, emit);
}

async function continueAfterConfirm(
  deps: IcpToBaseDeps,
  quote: IcpToBaseQuote,
  intent: IcpToBaseIntent,
  emit: Emit,
): Promise<void> {
  if (quote.needsApproval) {
    // First signer call. No await precedes it.
    const nowMs = (deps.now ?? Date.now)();
    const approveCall = deps.ledger.approve({
      amount: quote.requiredAllowanceE8s,
      spender: { owner: deps.bridgeCanister, subaccount: [] },
      expires_at: [BigInt(nowMs + APPROVAL_TTL_MS) * 1_000_000n],
      expected_allowance: [],
      memo: [],
      fee: [],
      from_subaccount: [],
      created_at_time: [],
    });
    let approval: ApproveResult;
    try {
      approval = await approveCall;
    } catch (error) {
      // Nothing was submitted to the bridge, so the intent can go. A late-landing approval is harmless:
      // the next quote sees the allowance and skips the approval step.
      abandonIntent(deps, intent);
      const message = errorMessage(error);
      emit({
        type: 'approval_failed',
        declined: DECLINED.test(message),
        message: DECLINED.test(message)
          ? 'The approval was declined in your wallet. Nothing was submitted.'
          : `The approval did not complete: ${message}`,
      });
      return;
    }
    if ('Err' in approval) {
      abandonIntent(deps, intent);
      emit({ type: 'approval_failed', declined: false, message: formatApproveError(approval.Err) });
      return;
    }
    // The approval is a fact worth keeping: it survives a reload and gates the "ready to bridge" state.
    markApproved(deps, intent);
    if (!deps.isCurrent()) {
      // The approval is harmless and expires; the unsent intent stays so it can be resolved later.
      emit({ type: 'session_changed' });
      return;
    }
    if (deps.separateApproval) {
      // Popup wallets cannot sign a second request in this click. Stop here; the next click sends the transfer.
      emit({ type: 'approval_confirmed' });
      return;
    }
    emit({ type: 'approval_ok' });
  }
  await submitToBridge(deps, intent, emit);
}

/**
 * Second click of the two-click path (popup-signer wallets): the approval is
 * already confirmed, so this click sends `bridge_to_base` as its FIRST signer
 * call, with the SAME client request id as the approval click. Also used after a
 * reload, for an intent that was approved earlier.
 */
export function bridgeAfterApproval(deps: IcpToBaseDeps, intent: IcpToBaseIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  emit({ type: 'bridge_started' });
  return submitToBridge(deps, intent, emit);
}

/**
 * Manual retry of an uncertain request. Reuses the intent's client request id, so
 * the canister answers with the existing deposit instead of creating another one.
 * Call directly from a click handler.
 */
export function retryIcpToBase(deps: IcpToBaseDeps, intent: IcpToBaseIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  emit({ type: 'retry' });
  return submitToBridge(deps, intent, emit);
}

async function submitToBridge(deps: IcpToBaseDeps, intent: IcpToBaseIntent, emit: Emit): Promise<void> {
  // Every path to `bridge_to_base` (second click, retry, and the step after an approval that may have taken
  // a while) passes here. Synchronous, so the first signer call of a click still has no await in front of it.
  // Two questions: may this destination sign at all, and does THIS saved request belong to it? A request
  // saved for another destination is neither sent nor rewritten; its recovery stays with its own destination.
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' }) ?? intentBindingMessage(deps.signing, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return;
  }
  let result: Result_3;
  try {
    result = await deps.bridge.bridgeToBase(bridgeToBaseArgs(intent));
  } catch (error) {
    const message = errorMessage(error);
    // The bridge's own consent text (shown by Oisy) refuses a stale fee before anything is signed.
    if (/deposit fee changed/i.test(message)) {
      markFailed(deps, intent, 'fee changed');
      emit({
        type: 'submit_rejected',
        message: 'The bridge deposit fee changed since you reviewed this transfer. Review the new amounts and confirm again.',
      });
      return;
    }
    await resolveUncertain(deps, intent, emit, `The bridge call did not return a result: ${message}`, 'submit_uncertain');
    return;
  }
  if ('Ok' in result) {
    acceptDeposit(deps, intent, result.Ok);
    emit({ type: 'submit_ok', deposit: result.Ok });
    return;
  }
  const err: BridgeError = result.Err;
  if ('FeeChanged' in err) {
    markFailed(deps, intent, 'fee changed');
    emit({ type: 'submit_fee_changed', currentFeeE8s: err.FeeChanged.current_fee_e8s });
    return;
  }
  if (classifyBridgeError(err) === 'no_effect') {
    markFailed(deps, intent, bridgeErrorKind(err));
    emit({
      type: 'submit_rejected',
      message: formatBridgeError(err),
      // The allowance is short or expired: nothing was created, review again to approve anew.
      reapprove: 'InsufficientAllowance' in err,
    });
    return;
  }
  await resolveUncertain(deps, intent, emit, formatBridgeError(err), 'submit_uncertain');
}

async function resolveUncertain(
  deps: IcpToBaseDeps,
  intent: IcpToBaseIntent,
  emit: Emit,
  message: string,
  first: 'submit_uncertain' | null,
  fromReload = false,
): Promise<void> {
  if (first) emit({ type: first, message });
  const outcome = await findDepositByRequestId(deps, intent.clientRequestId);
  if (outcome === 'unavailable') {
    emit({ type: 'lookup_unavailable' });
    return;
  }
  if (outcome === null) {
    // After a reload, an approved request the bridge has never seen is simply ready to bridge.
    emit(fromReload && intent.status === 'approved' ? { type: 'resume_approved', intent } : { type: 'lookup_not_found' });
    return;
  }
  acceptDeposit(deps, intent, outcome);
  emit({ type: 'lookup_found', deposit: outcome });
}

/**
 * Look the request up on the bridge by (owner, client request id) with a public
 * query. Returns the deposit, null when the bridge has no such request, or
 * 'unavailable' when the query failed or answered with a deposit that is not
 * this request (never treated as "not found").
 */
export async function findDepositByRequestId(
  deps: Pick<IcpToBaseDeps, 'reads' | 'owner'>,
  clientRequestId: string,
): Promise<DepositView | null | 'unavailable'> {
  try {
    const deposit = await deps.reads.getDepositByRequest(deps.owner, hexToBytes(clientRequestId));
    if (deposit === null) return null;
    const sameRequest = bytesToHex(deposit.client_request_id as ArrayLike<number>).toLowerCase() === clientRequestId.toLowerCase();
    const sameOwner = deposit.owner.toText() === deps.owner.toText();
    return sameRequest && sameOwner ? deposit : 'unavailable';
  } catch {
    return 'unavailable';
  }
}

/**
 * Reload path: an intent from an earlier session that never recorded an outcome.
 * Looks it up by client request id and reports through the same events.
 */
export async function resumeIcpToBase(deps: IcpToBaseDeps, intent: IcpToBaseIntent, emit: Emit): Promise<void> {
  emit({ type: 'resume_started', intent });
  await resolveUncertain(deps, intent, emit, '', null, true);
}

/** Forget an unsent intent. Only safe once a lookup showed the request never reached the bridge. */
export function discardIntent(deps: Pick<IcpToBaseDeps, 'store'>, intent: IcpToBaseIntent): void {
  deps.store.remove(intentId(intent));
}

function abandonIntent(deps: IcpToBaseDeps, intent: IcpToBaseIntent): void {
  try {
    deps.store.remove(intentId(intent));
  } catch {
    // A stale unsent record is resolved on the next visit.
  }
}

function markApproved(deps: IcpToBaseDeps, intent: IcpToBaseIntent): void {
  try {
    deps.store.patch<IcpToBaseIntent>(intentId(intent), { status: 'approved' });
  } catch {
    // The approval stands on the ledger either way; the intent stays 'prepared' and resolves the same way.
  }
}

function markFailed(deps: IcpToBaseDeps, intent: IcpToBaseIntent, failure: string): void {
  try {
    deps.store.patch<IcpToBaseIntent>(intentId(intent), { status: 'failed', failure });
  } catch {
    // The canister rejected the request, so a stale record is harmless.
  }
}

function acceptDeposit(deps: IcpToBaseDeps, intent: IcpToBaseIntent, deposit: DepositView): void {
  const terminal = describeDeposit(deposit, { principal: deps.owner.toText() }).terminal;
  try {
    deps.store.patch<IcpToBaseIntent>(intentId(intent), {
      status: terminal ? 'done' : 'submitted',
      depositId: deposit.id,
    });
  } catch {
    // The canister holds the deposit; losing the local note only costs convenience.
  }
}

// ---------------------------------------------------------------------------
// Tracking and refund
// ---------------------------------------------------------------------------

/** Poll `get_deposit` (an anonymous query) with bounded backoff until the deposit settles. */
export async function trackDeposit(
  deps: { reads: Pick<IcpToBaseDeps['reads'], 'getDeposit'>; owner: Principal; store: IntentStore; sleep?: Sleep },
  depositId: bigint,
  onDeposit: (deposit: DepositView) => void,
  signal?: AbortSignal,
  onError?: (error: unknown) => void,
): Promise<'done' | 'aborted' | 'exhausted'> {
  return pollWithBackoff<DepositView>({
    fetch: async () => {
      const deposit = await deps.reads.getDeposit(depositId);
      if (!deposit) throw new Error(`Deposit #${depositId} was not found.`);
      return deposit;
    },
    isDone: (deposit) => describeDeposit(deposit, { principal: deps.owner.toText() }).terminal,
    onUpdate: (deposit) => {
      onDeposit(deposit);
      if (describeDeposit(deposit, { principal: deps.owner.toText() }).terminal) {
        const known = deps.store
          .list()
          .find((i): i is IcpToBaseIntent => i.kind === 'icp_to_base' && i.depositId === deposit.id);
        if (known && !isResolved(known)) {
          try {
            deps.store.patch<IcpToBaseIntent>(intentId(known), { status: 'done' });
          } catch {
            // Cosmetic.
          }
        }
      }
    },
    onError,
    sleep: deps.sleep ?? abortableSleep,
    signal,
  });
}

/**
 * Ask the canister to refund a held deposit. Offered only to the owner of a
 * `MintHeld` user deposit. Call directly from the click handler.
 */
export async function requestDepositRefund(
  deps: Pick<IcpToBaseDeps, 'bridge' | 'owner' | 'isCurrent' | 'signing'>,
  deposit: DepositView,
): Promise<{ ok: true; deposit: DepositView } | { ok: false; message: string }> {
  if (!describeDeposit(deposit, { principal: deps.owner.toText() }).canRefund) {
    return { ok: false, message: 'Only the owner of a held deposit can request a refund.' };
  }
  if (!deps.isCurrent()) return { ok: false, message: 'Your wallet session changed. Nothing was submitted.' };
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) return { ok: false, message: notAllowed };
  try {
    const result = await deps.bridge.requestDepositRefund(deposit.id);
    if ('Ok' in result) return { ok: true, deposit: result.Ok };
    return { ok: false, message: formatBridgeError(result.Err) };
  } catch (error) {
    return {
      ok: false,
      message: `The refund request did not return a result (${errorMessage(error)}). Check the deposit status before trying again.`,
    };
  }
}
