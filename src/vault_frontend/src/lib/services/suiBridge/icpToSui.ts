/**
 * ICP -> Sui: icUSD on the Internet Computer to icUSD on Sui, minted to the holder's Sui address.
 *
 * It follows the Base and Solana flows' rules (`../baseBridge/icpToBase.ts`, `../solanaBridge/icpToSolana.ts`) with the Sui
 * instance's types:
 *  - the intent, with its client request id, is persisted BEFORE the first signer call, and every signer call is
 *    issued synchronously inside its click handler;
 *  - popup-signer wallets (Oisy) do the approval and `bridge_to_sui` as two clicks; delegation wallets do both in one;
 *  - an approval is requested only when the allowance is short, for exactly `amount + ledger fee`, with a short expiry;
 *  - an uncertain outcome is resolved by `get_deposit_by_request`, and a manual retry reuses the SAME request id. An
 *    absent query is NOT proof that the original update never ran, so a request that may have been submitted is never
 *    discarded or replaced by a fresh id: the only way forward is the same-id continuation (BB-50 F3, BB-62);
 *  - an ICP signer is asked only after `assertCanSign` agrees (a disabled destination gets no request at all);
 *  - the canister is the settlement authority: a Sui digest shown here is a hint.
 *
 * The Sui wallet signs nothing in this direction: it only supplies the recipient address. The ICP wallet, the ICRC-2
 * allowance and the two-action consent stay exactly as the Base flow has them. The quote arithmetic is the Base flow's
 * (`buildQuote`), which is generic over a recipient string. Amounts are bigint e8s (icUSD has 8 decimals).
 */

import type { Principal } from '@dfinity/principal';
import type { ApproveArgs, ApproveResult } from '$declarations/icusd_ledger/icusd_ledger.did';
import type { BridgeToSuiArgs, DepositView, Result_1, SuiBridgeError } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { formatE8s } from '../baseBridge/amounts';
import {
  APPROVAL_TTL_MS,
  QUOTE_MAX_AGE_MS,
  buildQuote,
  chooseLedgerFee,
  type IcpToBaseConfig,
  type IcpToBaseQuote,
  type PrepareDeps,
  type QuoteLimits,
  type QuoteProblem,
} from '../baseBridge/icpToBase';
import { hexToBytes } from '../baseBridge/abi';
import { newClientRequestId, type RandomBytes } from '../baseBridge/intents';
import { intentBindingMessage, signingBlockMessage, type SigningContext } from '../baseBridge/signingGuard';
import { abortableSleep, formatApproveError, pollWithBackoff, type Sleep } from '../baseBridge/status';
import { bytesEqual } from '../solanaBridge/bytes';
import { isZeroAddress, normalizeSuiAddress } from './address';
import { savedIdentityMessage, type SuiBridgeConfig } from './config';
import { SuiIntentStoreError, type SuiIntentStore, type SuiDepositIntent } from './intents';
import { classifySuiBridgeError, describeSuiDeposit, formatSuiBridgeError } from './status';

export { buildQuote, chooseLedgerFee, QUOTE_MAX_AGE_MS };
export type { IcpToBaseQuote, PrepareDeps, QuoteLimits, QuoteProblem };

// ---------------------------------------------------------------------------
// Pure: the recipient and the quote
// ---------------------------------------------------------------------------

export type SuiRecipientCheck =
  | { ok: true; address: string }
  | { ok: false; code: 'empty' | 'format' | 'zero' | 'reserved'; message: string };

/**
 * Validate a Sui recipient the way the canister does (`parse_address_strict`): `0x` and exactly 64 hex digits (the form is
 * case-folded here, a short form is refused, never padded), not the zero address, and not one of the bridge's own
 * addresses (the package, the Gate, the caps or a signing role).
 */
export function validateSuiRecipient(input: string, config: SuiBridgeConfig): SuiRecipientCheck {
  const text = (input ?? '').trim();
  if (text === '') return { ok: false, code: 'empty', message: 'Enter the Sui address that should receive the icUSD.' };
  const parsed = normalizeSuiAddress(text);
  if (!parsed.ok) return { ok: false, code: 'format', message: parsed.message };
  if (isZeroAddress(parsed.address)) return { ok: false, code: 'zero', message: 'The zero address cannot receive icUSD.' };
  if ([config.packageId, config.gateId, config.mintCapId, config.adminCapId, ...config.roleAddresses].includes(parsed.address)) {
    return { ok: false, code: 'reserved', message: 'That address belongs to the bridge itself, not to a wallet. Use your own Sui address.' };
  }
  return { ok: true, address: parsed.address };
}

export function suiQuoteLimits(config: SuiBridgeConfig): QuoteLimits {
  return { minDepositE8s: config.minDepositE8s, maxDepositE8s: config.maxDepositE8s, depositsPaused: config.depositsPaused, halted: config.halted };
}

/** First blocking problem with a quote, in the order a user would fix them. Sui wording. */
export function checkSuiQuote(quote: IcpToBaseQuote, limits: QuoteLimits, format: (e8s: bigint) => string): QuoteProblem | null {
  if (limits.halted) return { code: 'halted', message: `The bridge is halted for safety: ${limits.halted}` };
  if (limits.depositsPaused) return { code: 'paused', message: 'Deposits to Sui are paused right now. Your icUSD has not moved.' };
  if (quote.amountE8s < limits.minDepositE8s) return { code: 'below_minimum', message: `The minimum transfer is ${format(limits.minDepositE8s)} icUSD.` };
  if (quote.amountE8s > limits.maxDepositE8s) return { code: 'above_maximum', message: `The maximum per transfer is ${format(limits.maxDepositE8s)} icUSD.` };
  if (quote.mintAmountE8s <= 0n) return { code: 'fee_exceeds_amount', message: 'The amount must be larger than the bridge fee.' };
  if (quote.balanceE8s !== null && quote.balanceE8s < quote.totalDebitE8s) {
    return {
      code: 'insufficient_balance',
      message: `You need ${format(quote.totalDebitE8s)} icUSD (amount plus ledger fees) but have ${format(quote.balanceE8s)} icUSD.`,
    };
  }
  return null;
}

/** Build a quote from anonymous reads. Nothing here opens a wallet prompt. */
export async function prepareIcpToSui(
  deps: PrepareDeps,
  request: { amountE8s: bigint; recipient: string },
  format: (e8s: bigint) => string,
): Promise<{ quote: IcpToBaseQuote; problem: QuoteProblem | null; limits: QuoteLimits }> {
  const [config, liveFee, allowance, balance] = await Promise.all([deps.fetchConfig(), deps.fetchLedgerFee(), deps.readAllowance(), deps.readBalance()]);
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
  return { quote, problem: checkSuiQuote(quote, config.limits, format), limits: config.limits };
}

export type { IcpToBaseConfig };

// ---------------------------------------------------------------------------
// Pure: the flow state machine
// ---------------------------------------------------------------------------

export type SuiDepositState =
  | { kind: 'draft' }
  | { kind: 'quoting' }
  | { kind: 'review'; quote: IcpToBaseQuote; problem: QuoteProblem | null }
  | { kind: 'fee_changed'; quote: IcpToBaseQuote; currentFeeE8s: bigint }
  | { kind: 'approving'; quote: IcpToBaseQuote; intent: SuiDepositIntent }
  /** The approval is confirmed and the transfer has NOT been sent: waiting for the second click (popup signers). */
  | { kind: 'approved'; quote: IcpToBaseQuote | null; intent: SuiDepositIntent }
  | { kind: 'approval_failed'; quote: IcpToBaseQuote; message: string; declined: boolean }
  | { kind: 'submitting'; quote: IcpToBaseQuote | null; intent: SuiDepositIntent }
  | { kind: 'rejected'; quote: IcpToBaseQuote | null; message: string; reapprove: boolean }
  | { kind: 'uncertain'; intent: SuiDepositIntent; message: string; lookup: 'looking' | 'not_found' | 'unavailable' }
  | { kind: 'tracking'; intent: SuiDepositIntent; deposit: DepositView }
  | { kind: 'session_changed' }
  | { kind: 'error'; message: string };

export type SuiDepositEvent =
  | { type: 'edit' }
  | { type: 'reset' }
  | { type: 'quote_started' }
  | { type: 'quote_ready'; quote: IcpToBaseQuote; problem: QuoteProblem | null }
  | { type: 'quote_failed'; message: string }
  | { type: 'confirmed'; quote: IcpToBaseQuote | null; intent: SuiDepositIntent; needsApproval: boolean }
  | { type: 'approval_ok' }
  | { type: 'approval_confirmed' }
  | { type: 'bridge_started' }
  | { type: 'resume_started'; intent: SuiDepositIntent }
  | { type: 'resume_approved'; intent: SuiDepositIntent }
  | { type: 'approval_failed'; message: string; declined: boolean }
  | { type: 'submit_ok'; deposit: DepositView }
  | { type: 'submit_fee_changed'; currentFeeE8s: bigint }
  | { type: 'submit_rejected'; message: string; reapprove?: boolean }
  | { type: 'submit_uncertain'; message: string }
  | { type: 'lookup_found'; deposit: DepositView }
  | { type: 'lookup_not_found' }
  | { type: 'lookup_unavailable' }
  | { type: 'retry' }
  | { type: 'deposit_updated'; deposit: DepositView }
  | { type: 'track_existing'; intent: SuiDepositIntent; deposit: DepositView }
  | { type: 'session_changed' }
  | { type: 'blocked'; message: string };

export const INITIAL_SUI_DEPOSIT_STATE: SuiDepositState = { kind: 'draft' };

const IN_FLIGHT = (s: SuiDepositState) => s.kind === 'approving' || s.kind === 'submitting' || s.kind === 'approved';

export function reduceSuiDeposit(state: SuiDepositState, event: SuiDepositEvent): SuiDepositState {
  switch (event.type) {
    case 'reset':
    case 'edit':
      if (IN_FLIGHT(state)) return state;
      if (state.kind === 'uncertain' && event.type === 'edit') return state;
      return { kind: 'draft' };
    case 'session_changed':
      return { kind: 'session_changed' };
    case 'quote_started':
      return IN_FLIGHT(state) || state.kind === 'uncertain' ? state : { kind: 'quoting' };
    case 'quote_ready':
      return state.kind === 'quoting' ? { kind: 'review', quote: event.quote, problem: event.problem } : state;
    case 'quote_failed':
      return state.kind === 'quoting' ? { kind: 'error', message: event.message } : state;
    case 'confirmed':
      if (IN_FLIGHT(state)) return state;
      return event.needsApproval && event.quote ? { kind: 'approving', quote: event.quote, intent: event.intent } : { kind: 'submitting', quote: event.quote, intent: event.intent };
    case 'approval_ok':
      return state.kind === 'approving' ? { kind: 'submitting', quote: state.quote, intent: state.intent } : state;
    case 'approval_confirmed':
      return state.kind === 'approving' ? { kind: 'approved', quote: state.quote, intent: state.intent } : state;
    case 'bridge_started':
      return state.kind === 'approved' ? { kind: 'submitting', quote: state.quote, intent: state.intent } : state;
    case 'resume_started':
      if (IN_FLIGHT(state)) return state;
      return { kind: 'uncertain', intent: event.intent, message: 'This request was started on this device earlier and has no recorded outcome.', lookup: 'looking' };
    case 'resume_approved':
      return state.kind === 'uncertain' ? { kind: 'approved', quote: null, intent: event.intent } : state;
    case 'approval_failed':
      return state.kind === 'approving' ? { kind: 'approval_failed', quote: state.quote, message: event.message, declined: event.declined } : state;
    case 'submit_ok':
      return state.kind === 'submitting' || state.kind === 'uncertain' ? { kind: 'tracking', intent: state.intent, deposit: event.deposit } : state;
    case 'submit_fee_changed':
      if (state.kind !== 'submitting') return state;
      return state.quote
        ? { kind: 'fee_changed', quote: state.quote, currentFeeE8s: event.currentFeeE8s }
        : { kind: 'error', message: `The bridge deposit fee changed to ${formatE8s(event.currentFeeE8s)} icUSD. No icUSD moved. Review the transfer again to see the new amounts.` };
    case 'submit_rejected':
      return state.kind === 'submitting' ? { kind: 'rejected', quote: state.quote, message: event.message, reapprove: event.reapprove === true } : state;
    case 'submit_uncertain':
      return state.kind === 'submitting' ? { kind: 'uncertain', intent: state.intent, message: event.message, lookup: 'looking' } : state;
    case 'lookup_found':
      return state.kind === 'uncertain' ? { kind: 'tracking', intent: state.intent, deposit: event.deposit } : state;
    case 'lookup_not_found':
      return state.kind === 'uncertain' ? { ...state, lookup: 'not_found' } : state;
    case 'lookup_unavailable':
      return state.kind === 'uncertain' ? { ...state, lookup: 'unavailable' } : state;
    case 'retry':
      return state.kind === 'uncertain' && state.lookup !== 'looking' ? { kind: 'submitting', quote: null, intent: state.intent } : state;
    case 'deposit_updated':
      return state.kind === 'tracking' ? { ...state, deposit: event.deposit } : state;
    case 'track_existing':
      return IN_FLIGHT(state) || state.kind === 'uncertain' ? state : { kind: 'tracking', intent: event.intent, deposit: event.deposit };
    case 'blocked':
      return { kind: 'error', message: event.message };
  }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

export interface SuiDepositDeps {
  config: SuiBridgeConfig;
  owner: Principal;
  store: SuiIntentStore;
  bridgeCanister: Principal;
  ledger: { approve(args: ApproveArgs): Promise<ApproveResult> };
  bridge: {
    bridgeToSui(args: BridgeToSuiArgs): Promise<Result_1>;
    requestDepositRefund(id: bigint): Promise<Result_1>;
  };
  reads: {
    getDeposit(id: bigint): Promise<DepositView | null>;
    getDepositByRequest(owner: Principal, clientRequestId: Uint8Array): Promise<DepositView | null>;
  };
  separateApproval: boolean;
  isCurrent(): boolean;
  signing: SigningContext;
  random?: RandomBytes;
  now?(): number;
  sleep?: Sleep;
}

export type Emit = (event: SuiDepositEvent) => void;

const DECLINED = /reject|denied|declin|cancel|closed|dismiss/i;

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') return (error as { message: string }).message;
  return String(error ?? 'unknown error');
}

export function bridgeToSuiArgs(intent: SuiDepositIntent): BridgeToSuiArgs {
  return {
    amount_e8s: intent.amountE8s,
    sui_recipient: intent.recipient,
    from_subaccount: [],
    client_request_id: hexToBytes(`0x${intent.id}`),
    expected_deposit_fee_e8s: intent.expectedDepositFee,
  };
}

export function unresolvedDeposits(store: SuiIntentStore, destination?: string): SuiDepositIntent[] {
  return store
    .list()
    .filter((i): i is SuiDepositIntent => i.kind === 'icp_to_sui' && (i.status === 'prepared' || i.status === 'approved') && (destination === undefined || i.destination === destination));
}

/** The confirm button. Call it directly from the click handler. */
export function startIcpToSui(deps: SuiDepositDeps, quote: IcpToBaseQuote, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return Promise.resolve();
  }
  const recipient = validateSuiRecipient(quote.recipient, deps.config);
  if (!recipient.ok) {
    emit({ type: 'blocked', message: recipient.message });
    return Promise.resolve();
  }
  if (unresolvedDeposits(deps.store).length > 0) {
    emit({ type: 'blocked', message: 'An earlier bridge request on this device has no recorded outcome yet. Check its status and continue it with the same request first.' });
    return Promise.resolve();
  }
  const intent: SuiDepositIntent = {
    v: 1,
    kind: 'icp_to_sui',
    id: newClientRequestId(deps.random).slice(2),
    destination: deps.config.network.networkId,
    chainIdentifier: deps.config.network.chainIdentifier,
    instance: deps.config.instanceCanisterId,
    packageId: deps.config.packageId,
    gateId: deps.config.gateId,
    coinType: deps.config.coinType,
    recipient: recipient.address,
    amountE8s: quote.amountE8s,
    expectedDepositFee: quote.depositFeeE8s,
    createdAt: (deps.now ?? Date.now)(),
    status: 'prepared',
  };
  try {
    deps.store.put(intent);
  } catch (error) {
    emit({ type: 'blocked', message: error instanceof SuiIntentStoreError ? error.message : errorMessage(error) });
    return Promise.resolve();
  }
  emit({ type: 'confirmed', quote, intent, needsApproval: quote.needsApproval });
  return continueAfterConfirm(deps, quote, intent, emit);
}

async function continueAfterConfirm(deps: SuiDepositDeps, quote: IcpToBaseQuote, intent: SuiDepositIntent, emit: Emit): Promise<void> {
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
      forget(deps, intent);
      const message = errorMessage(error);
      emit({
        type: 'approval_failed',
        declined: DECLINED.test(message),
        message: DECLINED.test(message) ? 'The approval was declined in your wallet. Nothing was submitted.' : `The approval did not complete: ${message}`,
      });
      return;
    }
    if ('Err' in approval) {
      forget(deps, intent);
      emit({ type: 'approval_failed', declined: false, message: formatApproveError(approval.Err) });
      return;
    }
    patch(deps, intent, { status: 'approved' });
    if (!deps.isCurrent()) {
      emit({ type: 'session_changed' });
      return;
    }
    if (deps.separateApproval) {
      emit({ type: 'approval_confirmed' });
      return;
    }
    emit({ type: 'approval_ok' });
  }
  await submitToBridge(deps, intent, emit);
}

/** The second click of the two-click path (popup signers), and the continuation after a reload. */
export function bridgeAfterApproval(deps: SuiDepositDeps, intent: SuiDepositIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  const elsewhere = savedIdentityMessage(deps.config, intent);
  if (elsewhere) {
    emit({ type: 'blocked', message: elsewhere });
    return Promise.resolve();
  }
  emit({ type: 'bridge_started' });
  return submitToBridge(deps, intent, emit);
}

/** Manual retry of an uncertain request: the same request id, so the canister answers with the existing deposit. */
export function retryIcpToSui(deps: SuiDepositDeps, intent: SuiDepositIntent, emit: Emit): Promise<void> {
  if (!deps.isCurrent()) {
    emit({ type: 'session_changed' });
    return Promise.resolve();
  }
  const elsewhere = savedIdentityMessage(deps.config, intent);
  if (elsewhere) {
    emit({ type: 'blocked', message: elsewhere });
    return Promise.resolve();
  }
  emit({ type: 'retry' });
  return submitToBridge(deps, intent, emit);
}

async function submitToBridge(deps: SuiDepositDeps, intent: SuiDepositIntent, emit: Emit): Promise<void> {
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' }) ?? intentBindingMessage(deps.signing, intent) ?? savedIdentityMessage(deps.config, intent);
  if (notAllowed) {
    emit({ type: 'blocked', message: notAllowed });
    return;
  }
  let result: Result_1;
  try {
    result = await deps.bridge.bridgeToSui(bridgeToSuiArgs(intent));
  } catch (error) {
    const message = errorMessage(error);
    if (/deposit fee changed/i.test(message)) {
      patch(deps, intent, { status: 'failed', failure: 'fee changed' });
      emit({ type: 'submit_rejected', message: 'The bridge deposit fee changed since you reviewed this transfer. Review the new amounts and confirm again.' });
      return;
    }
    await resolveUncertain(deps, intent, emit, `The bridge call did not return a result: ${message}`, 'submit_uncertain');
    return;
  }
  if ('Ok' in result) {
    accept(deps, intent, result.Ok);
    emit({ type: 'submit_ok', deposit: result.Ok });
    return;
  }
  const err: SuiBridgeError = result.Err;
  if ('FeeChanged' in err) {
    patch(deps, intent, { status: 'failed', failure: 'fee changed' });
    emit({ type: 'submit_fee_changed', currentFeeE8s: err.FeeChanged.current_fee_e8s });
    return;
  }
  if (classifySuiBridgeError(err as never) === 'no_effect') {
    patch(deps, intent, { status: 'failed', failure: Object.keys(err)[0] });
    emit({ type: 'submit_rejected', message: formatSuiBridgeError(err), reapprove: 'InsufficientAllowance' in err });
    return;
  }
  await resolveUncertain(deps, intent, emit, formatSuiBridgeError(err), 'submit_uncertain');
}

async function resolveUncertain(deps: SuiDepositDeps, intent: SuiDepositIntent, emit: Emit, message: string, first: 'submit_uncertain' | null, fromReload = false): Promise<void> {
  if (first) emit({ type: first, message });
  const outcome = await findDepositByRequestId(deps, intent.id);
  if (outcome === 'unavailable') {
    emit({ type: 'lookup_unavailable' });
    return;
  }
  if (outcome === null) {
    emit(fromReload && intent.status === 'approved' ? { type: 'resume_approved', intent } : { type: 'lookup_not_found' });
    return;
  }
  accept(deps, intent, outcome);
  emit({ type: 'lookup_found', deposit: outcome });
}

/** `get_deposit_by_request(owner, id)`: the deposit, null when the bridge has none, or 'unavailable' (never "not found"). */
export async function findDepositByRequestId(deps: Pick<SuiDepositDeps, 'reads' | 'owner'>, id: string): Promise<DepositView | null | 'unavailable'> {
  try {
    const deposit = await deps.reads.getDepositByRequest(deps.owner, hexToBytes(`0x${id}`));
    if (deposit === null) return null;
    const sameRequest = bytesEqual(Uint8Array.from(deposit.client_request_id as ArrayLike<number>), hexToBytes(`0x${id}`));
    return sameRequest && deposit.owner.toText() === deps.owner.toText() ? deposit : 'unavailable';
  } catch {
    return 'unavailable';
  }
}

/** Reload path: an intent from an earlier session that never recorded an outcome. */
export async function resumeIcpToSui(deps: SuiDepositDeps, intent: SuiDepositIntent, emit: Emit): Promise<void> {
  // A request of an EARLIER bridge is never looked up on the bridge now in use: that canister's absence (or a record that
  // merely shares an id) must not settle, clear or continue the old claim. It stays saved, untouched.
  const elsewhere = savedIdentityMessage(deps.config, intent);
  if (elsewhere) {
    emit({ type: 'blocked', message: elsewhere });
    return;
  }
  emit({ type: 'resume_started', intent });
  await resolveUncertain(deps, intent, emit, '', null, true);
}

function forget(deps: SuiDepositDeps, intent: SuiDepositIntent): void {
  try {
    deps.store.remove(intent.id);
  } catch {
    // A stale unsent record is resolved on the next visit.
  }
}

function patch(deps: Pick<SuiDepositDeps, 'store'>, intent: SuiDepositIntent, change: Partial<SuiDepositIntent>): void {
  try {
    deps.store.patch<SuiDepositIntent>(intent.id, change);
  } catch {
    // The canister is authoritative; a stale local note only costs convenience.
  }
}

function accept(deps: SuiDepositDeps, intent: SuiDepositIntent, deposit: DepositView): void {
  const terminal = describeSuiDeposit(deposit, { principal: deps.owner.toText() }).terminal;
  patch(deps, intent, { status: terminal ? 'done' : 'submitted', depositId: deposit.id });
}

/** Poll `get_deposit` with bounded backoff until the deposit settles. */
export async function trackSuiDeposit(
  deps: Pick<SuiDepositDeps, 'owner' | 'store' | 'sleep'> & { reads: Pick<SuiDepositDeps['reads'], 'getDeposit'> },
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
    isDone: (deposit) => describeSuiDeposit(deposit, { principal: deps.owner.toText() }).terminal,
    onUpdate: (deposit) => {
      onDeposit(deposit);
      if (describeSuiDeposit(deposit, { principal: deps.owner.toText() }).terminal) {
        const known = deps.store.list().find((i): i is SuiDepositIntent => i.kind === 'icp_to_sui' && i.depositId === deposit.id);
        if (known && known.status !== 'done') patch(deps, known, { status: 'done' });
      }
    },
    onError,
    sleep: deps.sleep ?? abortableSleep,
    signal,
  });
}

/** Ask the canister to refund a held deposit (a click). Offered only to the owner of a held user deposit. */
export async function requestSuiDepositRefund(
  deps: Pick<SuiDepositDeps, 'bridge' | 'owner' | 'isCurrent' | 'signing'>,
  deposit: DepositView,
): Promise<{ ok: true; deposit: DepositView } | { ok: false; message: string }> {
  if (!describeSuiDeposit(deposit, { principal: deps.owner.toText() }).canRefund) {
    return { ok: false, message: 'Only the owner of a held deposit can request a refund.' };
  }
  if (!deps.isCurrent()) return { ok: false, message: 'Your wallet session changed. Nothing was submitted.' };
  const notAllowed = signingBlockMessage(deps.signing, { wallet: 'icp_signer' });
  if (notAllowed) return { ok: false, message: notAllowed };
  try {
    const result = await deps.bridge.requestDepositRefund(deposit.id);
    return 'Ok' in result ? { ok: true, deposit: result.Ok } : { ok: false, message: formatSuiBridgeError(result.Err) };
  } catch (error) {
    return { ok: false, message: `The refund request did not return a result (${errorMessage(error)}). Check the deposit status before trying again.` };
  }
}
