import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import type { BridgeError, DepositView } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import { formatE8s } from './amounts';
import {
  ALLOWANCE_EXPIRY_MARGIN_MS,
  APPROVAL_TTL_MS,
  INITIAL_ICP_TO_BASE_STATE,
  QUOTE_MAX_AGE_MS,
  allowanceNeedsApproval,
  buildQuote,
  checkQuote,
  chooseLedgerFee,
  discardIntent,
  findDepositByRequestId,
  hasUnresolvedIntent,
  isQuoteStale,
  prepareIcpToBase,
  reduceIcpToBase,
  requestDepositRefund,
  resumeIcpToBase,
  retryIcpToBase,
  startIcpToBase,
  trackDeposit,
  validateRecipient,
  type IcpToBaseDeps,
  type IcpToBaseEvent,
  type IcpToBaseQuote,
  type IcpToBaseState,
  type PrepareDeps,
  type QuoteLimits,
} from './icpToBase';
import { IntentStore, type IcpToBaseIntent } from './intents';
import { TEST_OWNER, createMemoryStorage, makeDeposit } from './testSupport';
import { bytesToHex, hexToBytes } from './abi';

const ME = TEST_OWNER.toText();
const BRIDGE = Principal.fromText('tfesu-vyaaa-aaaap-qrd7a-cai');
const RECIPIENT = '0x1111111111111111111111111111111111111111';
const CONTRACT = '0x2222222222222222222222222222222222222222';
const NOW = 1_800_000_000_000;
const AMOUNT = 2_500_000_000n; // 25 icUSD
const LEDGER_FEE = 100_000n;
const DEPOSIT_FEE = 10_000n;
const LIMITS: QuoteLimits = { minDepositE8s: 100_000_000n, maxDepositE8s: 50_000_000_000n, depositsPaused: false, halted: null };
const fmt = (e8s: bigint) => formatE8s(e8s);

function quote(overrides: Partial<Parameters<typeof buildQuote>[0]> = {}): IcpToBaseQuote {
  return buildQuote({
    amountE8s: AMOUNT,
    recipient: RECIPIENT,
    ledgerFeeE8s: LEDGER_FEE,
    depositFeeE8s: DEPOSIT_FEE,
    allowanceE8s: 0n,
    allowanceExpiresAtNs: null,
    balanceE8s: 10_000_000_000n,
    nowMs: NOW,
    ...overrides,
  });
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

describe('validateRecipient', () => {
  it('accepts a 20 byte address and lower-cases it', () => {
    expect(validateRecipient('  0xAbCdEf0123456789aBcDeF0123456789AbCdEf01 ', CONTRACT)).toEqual({
      ok: true,
      address: '0xabcdef0123456789abcdef0123456789abcdef01',
    });
  });

  it('rejects empty, malformed, zero and the token contract', () => {
    expect(validateRecipient('', CONTRACT)).toMatchObject({ ok: false, code: 'empty' });
    expect(validateRecipient('1111111111111111111111111111111111111111', CONTRACT)).toMatchObject({ ok: false, code: 'format' });
    expect(validateRecipient('0x1234', CONTRACT)).toMatchObject({ ok: false, code: 'format' });
    expect(validateRecipient(`0x${'g'.repeat(40)}`, CONTRACT)).toMatchObject({ ok: false, code: 'format' });
    expect(validateRecipient(`0x${'0'.repeat(40)}`, CONTRACT)).toMatchObject({ ok: false, code: 'zero' });
    expect(validateRecipient(CONTRACT.toUpperCase().replace('0X', '0x'), CONTRACT)).toMatchObject({ ok: false, code: 'contract' });
  });

  it('does not reject anything as the contract when the contract is unknown', () => {
    expect(validateRecipient(CONTRACT, null)).toMatchObject({ ok: true });
  });
});

describe('ledger fee and allowance rules', () => {
  it('plans around the larger of the live and the bridge fee', () => {
    expect(chooseLedgerFee(100_000n, null)).toBe(100_000n);
    expect(chooseLedgerFee(100_000n, 90_000n)).toBe(100_000n);
    expect(chooseLedgerFee(100_000n, 120_000n)).toBe(120_000n);
  });

  it('needs approval only when the allowance is short or about to expire', () => {
    const required = AMOUNT + LEDGER_FEE;
    expect(allowanceNeedsApproval(0n, null, required, NOW)).toBe(true);
    expect(allowanceNeedsApproval(required - 1n, null, required, NOW)).toBe(true);
    expect(allowanceNeedsApproval(required, null, required, NOW)).toBe(false);
    expect(allowanceNeedsApproval(required * 10n, null, required, NOW)).toBe(false);
    const inNs = (ms: number) => BigInt(NOW + ms) * 1_000_000n;
    expect(allowanceNeedsApproval(required, inNs(ALLOWANCE_EXPIRY_MARGIN_MS - 1_000), required, NOW)).toBe(true);
    expect(allowanceNeedsApproval(required, inNs(ALLOWANCE_EXPIRY_MARGIN_MS + 60_000), required, NOW)).toBe(false);
    expect(allowanceNeedsApproval(required, inNs(-5_000), required, NOW)).toBe(true);
  });
});

describe('buildQuote', () => {
  it('discloses every amount when an approval is needed', () => {
    const q = quote();
    expect(q).toMatchObject({
      amountE8s: 2_500_000_000n,
      ledgerFeeE8s: 100_000n,
      approvalFeeE8s: 100_000n,
      depositFeeE8s: 10_000n,
      mintAmountE8s: 2_499_990_000n,
      requiredAllowanceE8s: 2_500_100_000n,
      needsApproval: true,
      totalDebitE8s: 2_500_200_000n,
    });
  });

  it('charges no approval fee when the allowance already covers amount plus fee', () => {
    const q = quote({ allowanceE8s: 2_500_100_000n });
    expect(q.needsApproval).toBe(false);
    expect(q.approvalFeeE8s).toBe(0n);
    expect(q.totalDebitE8s).toBe(2_500_100_000n);
  });

  it('never mints a negative amount', () => {
    expect(quote({ amountE8s: 5_000n }).mintAmountE8s).toBe(0n);
    expect(quote({ amountE8s: DEPOSIT_FEE }).mintAmountE8s).toBe(0n);
  });
});

describe('checkQuote', () => {
  it('passes a healthy quote', () => {
    expect(checkQuote(quote(), LIMITS, fmt)).toBeNull();
  });

  it('reports each blocking problem', () => {
    expect(checkQuote(quote(), { ...LIMITS, halted: 'coverage' }, fmt)).toMatchObject({ code: 'halted' });
    expect(checkQuote(quote(), { ...LIMITS, depositsPaused: true }, fmt)).toMatchObject({ code: 'paused' });
    expect(checkQuote(quote({ amountE8s: 50_000_000n }), LIMITS, fmt)).toMatchObject({ code: 'below_minimum' });
    expect(checkQuote(quote({ amountE8s: 60_000_000_000n }), LIMITS, fmt)).toMatchObject({ code: 'above_maximum' });
    expect(checkQuote(quote(), { ...LIMITS, minDepositE8s: 1n, maxDepositE8s: 1n << 63n }, fmt)).toBeNull();
    expect(checkQuote(quote({ amountE8s: 100_000_000n, depositFeeE8s: 100_000_000n }), LIMITS, fmt)).toMatchObject({ code: 'fee_exceeds_amount' });
    const broke = checkQuote(quote({ balanceE8s: 2_500_000_000n }), LIMITS, fmt);
    expect(broke).toMatchObject({ code: 'insufficient_balance' });
    expect(broke!.message).toContain('25.002 icUSD');
  });

  it('skips the balance check when the balance is unknown', () => {
    expect(checkQuote(quote({ balanceE8s: null }), LIMITS, fmt)).toBeNull();
  });

  it('expires a quote after two minutes', () => {
    expect(isQuoteStale(quote(), NOW + QUOTE_MAX_AGE_MS)).toBe(false);
    expect(isQuoteStale(quote(), NOW + QUOTE_MAX_AGE_MS + 1)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Reducer
// ---------------------------------------------------------------------------

function intent(overrides: Partial<IcpToBaseIntent> = {}): IcpToBaseIntent {
  return {
    v: 1,
    kind: 'icp_to_base',
    clientRequestId: `0x${'ab'.repeat(16)}`,
    amountE8s: AMOUNT,
    recipient: RECIPIENT,
    expectedDepositFee: DEPOSIT_FEE,
    createdAt: NOW,
    status: 'prepared',
    ...overrides,
  };
}

function drive(events: IcpToBaseEvent[], from: IcpToBaseState = INITIAL_ICP_TO_BASE_STATE): IcpToBaseState {
  return events.reduce(reduceIcpToBase, from);
}

describe('reduceIcpToBase', () => {
  const q = quote();

  it('walks the happy path with an approval', () => {
    const deposit = makeDeposit({ status: { Escrowed: null } });
    expect(drive([{ type: 'quote_started' }]).kind).toBe('quoting');
    expect(drive([{ type: 'quote_started' }, { type: 'quote_ready', quote: q, problem: null }])).toMatchObject({ kind: 'review', quote: q });
    const approving = drive([
      { type: 'quote_started' },
      { type: 'quote_ready', quote: q, problem: null },
      { type: 'confirmed', quote: q, intent: intent(), needsApproval: true },
    ]);
    expect(approving.kind).toBe('approving');
    const submitting = reduceIcpToBase(approving, { type: 'approval_ok' });
    expect(submitting).toMatchObject({ kind: 'submitting', quote: q });
    const tracking = reduceIcpToBase(submitting, { type: 'submit_ok', deposit });
    expect(tracking).toMatchObject({ kind: 'tracking', deposit });
    const next = makeDeposit({ status: { Delivered: null } });
    expect(reduceIcpToBase(tracking, { type: 'deposit_updated', deposit: next })).toMatchObject({ kind: 'tracking', deposit: next });
  });

  it('skips the approving state when no approval is needed', () => {
    const state = drive([{ type: 'confirmed', quote: quote({ allowanceE8s: 10n ** 12n }), intent: intent(), needsApproval: false }]);
    expect(state.kind).toBe('submitting');
  });

  it('keeps the review problem so the confirm button can stay disabled', () => {
    const problem = { code: 'below_minimum' as const, message: 'too small' };
    expect(drive([{ type: 'quote_started' }, { type: 'quote_ready', quote: q, problem }])).toMatchObject({ kind: 'review', problem });
  });

  it('returns to the review with a message when the approval is declined', () => {
    const state = drive([
      { type: 'confirmed', quote: q, intent: intent(), needsApproval: true },
      { type: 'approval_failed', message: 'declined', declined: true },
    ]);
    expect(state).toMatchObject({ kind: 'approval_failed', declined: true, message: 'declined', quote: q });
    expect(reduceIcpToBase(state, { type: 'edit' }).kind).toBe('draft');
  });

  it('asks for a new confirmation when the fee changed', () => {
    const state = drive([
      { type: 'confirmed', quote: q, intent: intent(), needsApproval: false },
      { type: 'submit_fee_changed', currentFeeE8s: 20_000n },
    ]);
    expect(state).toMatchObject({ kind: 'fee_changed', currentFeeE8s: 20_000n, quote: q });
    // It cannot be confirmed from here; the user has to re-quote and review.
    expect(reduceIcpToBase(state, { type: 'confirmed', quote: q, intent: intent(), needsApproval: false }).kind).toBe('submitting');
    const requoted = drive([{ type: 'quote_started' }, { type: 'quote_ready', quote: q, problem: null }], state);
    expect(requoted.kind).toBe('review');
  });

  it('shows a canister rejection without creating a deposit', () => {
    const state = drive([
      { type: 'confirmed', quote: q, intent: intent(), needsApproval: false },
      { type: 'submit_rejected', message: 'paused' },
    ]);
    expect(state).toMatchObject({ kind: 'rejected', message: 'paused' });
  });

  it('resolves an uncertain submission by lookup, or leaves it for a manual retry', () => {
    const base = drive([
      { type: 'confirmed', quote: q, intent: intent(), needsApproval: false },
      { type: 'submit_uncertain', message: 'lost reply' },
    ]);
    expect(base).toMatchObject({ kind: 'uncertain', lookup: 'looking' });
    // Retry is not offered while the lookup is still running.
    expect(reduceIcpToBase(base, { type: 'retry' })).toBe(base);

    const deposit = makeDeposit();
    expect(reduceIcpToBase(base, { type: 'lookup_found', deposit })).toMatchObject({ kind: 'tracking', deposit });

    const missing = reduceIcpToBase(base, { type: 'lookup_not_found' });
    expect(missing).toMatchObject({ kind: 'uncertain', lookup: 'not_found' });
    const unavailable = reduceIcpToBase(base, { type: 'lookup_unavailable' });
    expect(unavailable).toMatchObject({ kind: 'uncertain', lookup: 'unavailable' });

    const retried = reduceIcpToBase(missing, { type: 'retry' });
    expect(retried).toMatchObject({ kind: 'submitting' });
    expect((retried as { intent: IcpToBaseIntent }).intent.clientRequestId).toBe(intent().clientRequestId);
  });

  it('cannot be edited or re-quoted while a request is in flight', () => {
    const inflight = drive([{ type: 'confirmed', quote: q, intent: intent(), needsApproval: true }]);
    expect(reduceIcpToBase(inflight, { type: 'edit' })).toBe(inflight);
    expect(reduceIcpToBase(inflight, { type: 'reset' })).toBe(inflight);
    expect(reduceIcpToBase(inflight, { type: 'quote_started' })).toBe(inflight);
    const uncertain = drive([{ type: 'submit_uncertain', message: 'x' }], drive([{ type: 'confirmed', quote: q, intent: intent(), needsApproval: false }]));
    expect(reduceIcpToBase(uncertain, { type: 'edit' })).toBe(uncertain);
    expect(reduceIcpToBase(uncertain, { type: 'quote_started' })).toBe(uncertain);
  });

  it('ignores events that do not apply to the current state', () => {
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'approval_ok' })).toBe(INITIAL_ICP_TO_BASE_STATE);
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'quote_ready', quote: q, problem: null })).toBe(INITIAL_ICP_TO_BASE_STATE);
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'submit_ok', deposit: makeDeposit() })).toBe(INITIAL_ICP_TO_BASE_STATE);
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'deposit_updated', deposit: makeDeposit() })).toBe(INITIAL_ICP_TO_BASE_STATE);
  });

  it('follows a known deposit (reload recovery) unless a request is in flight', () => {
    const deposit = makeDeposit({ status: { Escrowed: null } });
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'track_existing', intent: intent(), deposit })).toMatchObject({ kind: 'tracking', deposit });
    const inflight = drive([{ type: 'confirmed', quote: q, intent: intent(), needsApproval: true }]);
    expect(reduceIcpToBase(inflight, { type: 'track_existing', intent: intent(), deposit })).toBe(inflight);
  });

  it('aborts on a session change from any state', () => {
    for (const state of [
      INITIAL_ICP_TO_BASE_STATE,
      drive([{ type: 'confirmed', quote: q, intent: intent(), needsApproval: true }]),
      drive([{ type: 'confirmed', quote: q, intent: intent(), needsApproval: false }]),
    ]) {
      expect(reduceIcpToBase(state, { type: 'session_changed' }).kind).toBe('session_changed');
    }
  });

  it('turns a blocked start or a failed quote into an error', () => {
    expect(reduceIcpToBase(INITIAL_ICP_TO_BASE_STATE, { type: 'blocked', message: 'no storage' })).toMatchObject({ kind: 'error', message: 'no storage' });
    const quoting = drive([{ type: 'quote_started' }]);
    expect(reduceIcpToBase(quoting, { type: 'quote_failed', message: 'offline' })).toMatchObject({ kind: 'error', message: 'offline' });
  });

  it('a fee change with no quote (retry path) asks for a fresh transfer', () => {
    const state = drive([
      { type: 'confirmed', quote: null, intent: intent(), needsApproval: false },
      { type: 'submit_fee_changed', currentFeeE8s: 1n },
    ]);
    expect(state).toMatchObject({ kind: 'error' });
  });
});

// ---------------------------------------------------------------------------
// Effect runner
// ---------------------------------------------------------------------------

interface Harness {
  deps: IcpToBaseDeps;
  store: IntentStore;
  storage: ReturnType<typeof createMemoryStorage>;
  approve: ReturnType<typeof vi.fn>;
  bridgeToBase: ReturnType<typeof vi.fn>;
  requestDepositRefund: ReturnType<typeof vi.fn>;
  getDepositByRequest: ReturnType<typeof vi.fn>;
  getDeposit: ReturnType<typeof vi.fn>;
  events: IcpToBaseEvent[];
  emit: (e: IcpToBaseEvent) => void;
  state: () => IcpToBaseState;
  current: { value: boolean };
}

function harness(overrides: { deposit?: DepositView } = {}): Harness {
  const storage = createMemoryStorage();
  const store = new IntentStore(storage, ME);
  const current = { value: true };
  const deposit = overrides.deposit ?? makeDeposit({ status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) });
  const approve = vi.fn(async () => ({ Ok: 11n }));
  const bridgeToBase = vi.fn(async () => ({ Ok: deposit }));
  const requestDepositRefund = vi.fn(async () => ({ Ok: makeDeposit({ status: { Refunding: null } }) }));
  const getDepositByRequest = vi.fn(async () => null as DepositView | null);
  const getDeposit = vi.fn(async () => deposit as DepositView | null);
  const deps: IcpToBaseDeps = {
    owner: TEST_OWNER,
    store,
    bridgeCanister: BRIDGE,
    ledger: { approve: approve as any },
    bridge: { bridgeToBase: bridgeToBase as any, requestDepositRefund: requestDepositRefund as any },
    reads: { getDeposit: getDeposit as any, getDepositByRequest: getDepositByRequest as any },
    isCurrent: () => current.value,
    random: (length) => new Uint8Array(length).fill(0xcd),
    now: () => NOW,
    sleep: async () => {},
  };
  const events: IcpToBaseEvent[] = [];
  let state: IcpToBaseState = INITIAL_ICP_TO_BASE_STATE;
  const emit = (e: IcpToBaseEvent) => {
    events.push(e);
    state = reduceIcpToBase(state, e);
  };
  return { deps, store, storage, approve, bridgeToBase, requestDepositRefund, getDepositByRequest, getDeposit, events, emit, state: () => state, current };
}

const REQUEST_ID = `0x${'cd'.repeat(16)}`;

describe('startIcpToBase: signing order and persistence', () => {
  it('saves the intent, then calls approve synchronously (no await before the first signer call)', async () => {
    const h = harness();
    let intentSavedWhenApproveCalled: unknown = null;
    h.approve.mockImplementation(async () => {
      intentSavedWhenApproveCalled = h.store.get(REQUEST_ID);
      return { Ok: 11n };
    });
    const q = quote();
    const promise = startIcpToBase(h.deps, q, h.emit);
    // Still inside the "click handler": nothing has been awaited yet.
    expect(h.approve).toHaveBeenCalledTimes(1);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(intentSavedWhenApproveCalled).toMatchObject({ kind: 'icp_to_base', clientRequestId: REQUEST_ID, status: 'prepared', amountE8s: AMOUNT, recipient: RECIPIENT, expectedDepositFee: DEPOSIT_FEE });
    await promise;
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
  });

  it('requests exactly amount + ledger fee, expiring in 30 minutes, for the bridge canister only', async () => {
    const h = harness();
    await startIcpToBase(h.deps, quote(), h.emit);
    const args = h.approve.mock.calls[0][0];
    expect(args.amount).toBe(AMOUNT + LEDGER_FEE);
    expect(args.spender).toEqual({ owner: BRIDGE, subaccount: [] });
    expect(args.expires_at).toEqual([BigInt(NOW + APPROVAL_TTL_MS) * 1_000_000n]);
    expect(args.from_subaccount).toEqual([]);
    expect(args.fee).toEqual([]);
    expect(args.expected_allowance).toEqual([]);
  });

  it('goes straight to bridge_to_base, synchronously, when the allowance is sufficient', async () => {
    const h = harness();
    const q = quote({ allowanceE8s: AMOUNT + LEDGER_FEE });
    const promise = startIcpToBase(h.deps, q, h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    await promise;
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.state().kind).toBe('tracking');
  });

  it('sends the exact bridge_to_base arguments built from the intent', async () => {
    const h = harness();
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    const args = h.bridgeToBase.mock.calls[0][0];
    expect(args).toMatchObject({
      amount_e8s: AMOUNT,
      base_recipient: RECIPIENT,
      from_subaccount: [],
      expected_deposit_fee_e8s: DEPOSIT_FEE,
    });
    expect(bytesToHex(args.client_request_id)).toBe(REQUEST_ID);
    expect(args.client_request_id).toHaveLength(16);
  });

  it('records the deposit id on the intent once the canister answers', async () => {
    const h = harness();
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'submitted', depositId: 1n });
    expect(h.state()).toMatchObject({ kind: 'tracking' });
  });

  it('marks the intent done when the answer is already terminal', async () => {
    const h = harness({ deposit: makeDeposit({ status: { PullFailed: { reason: 'insufficient allowance' } } }) });
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'done' });
    expect(h.state()).toMatchObject({ kind: 'tracking' });
  });

  it('makes no wallet call at all when the intent cannot be saved', async () => {
    const h = harness();
    h.storage.setItem = () => {
      throw new Error('quota');
    };
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.state()).toMatchObject({ kind: 'error' });
  });

  it('makes no wallet call when the session is no longer current', async () => {
    const h = harness();
    h.current.value = false;
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.state().kind).toBe('session_changed');
    expect(h.store.list()).toEqual([]);
  });

  it('refuses to start a second transfer while an earlier request has no outcome', async () => {
    const h = harness();
    h.store.put(intent({ clientRequestId: `0x${'01'.repeat(16)}` }));
    expect(hasUnresolvedIntent(h.store)).toBe(true);
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.state()).toMatchObject({ kind: 'error' });
  });
});

describe('startIcpToBase: approval outcomes', () => {
  it('stops cleanly when the approval is declined: no bridge call, no leftover intent', async () => {
    const h = harness();
    h.approve.mockRejectedValue(new Error('User rejected the request'));
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.state()).toMatchObject({ kind: 'approval_failed', declined: true });
    expect(h.store.list()).toEqual([]);
  });

  it('reports other approval failures without claiming a rejection', async () => {
    const h = harness();
    h.approve.mockRejectedValue(new Error('network down'));
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.state()).toMatchObject({ kind: 'approval_failed', declined: false });
    expect((h.state() as { message: string }).message).toContain('network down');
    expect(h.bridgeToBase).not.toHaveBeenCalled();
  });

  it('maps a ledger approve error to plain language', async () => {
    const h = harness();
    h.approve.mockResolvedValue({ Err: { InsufficientFunds: { balance: 5_000n } } });
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.state()).toMatchObject({ kind: 'approval_failed', declined: false });
    expect((h.state() as { message: string }).message).toMatch(/cannot cover the approval fee/);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.store.list()).toEqual([]);
  });

  it('does not call the bridge if the session changed during the approval', async () => {
    const h = harness();
    h.approve.mockImplementation(async () => {
      h.current.value = false;
      return { Ok: 1n };
    });
    await startIcpToBase(h.deps, quote(), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.state().kind).toBe('session_changed');
    // The unsent intent is kept so it can be resolved after signing in again.
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'prepared' });
  });
});

describe('startIcpToBase: bridge outcomes', () => {
  it('requires a new confirmation when the fee changed, and records the request as not created', async () => {
    const h = harness();
    h.bridgeToBase.mockResolvedValue({ Err: { FeeChanged: { current_fee_e8s: 20_000n } } as BridgeError });
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'fee_changed', currentFeeE8s: 20_000n });
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'failed' });
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    expect(hasUnresolvedIntent(h.store)).toBe(false);
  });

  it('treats the wallet consent refusal for a stale fee as a clean rejection', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error('ConsentMessageUnavailable: deposit fee changed: you were shown 0.0001 icUSD, current fee is 0.0002 icUSD'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'rejected' });
    expect((h.state() as { message: string }).message).toMatch(/fee changed/);
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'failed' });
    expect(h.getDepositByRequest).not.toHaveBeenCalled();
  });

  it('shows a typed no-effect error and never retries it', async () => {
    const h = harness();
    h.bridgeToBase.mockResolvedValue({ Err: { Paused: 'deposits are paused' } as BridgeError });
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'rejected' });
    expect((h.state() as { message: string }).message).toMatch(/paused/);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    expect(h.getDepositByRequest).not.toHaveBeenCalled();
  });
});

describe('uncertain outcomes are resolved by get_deposit_by_request', () => {
  const landed = () => makeDeposit({ id: 9n, status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) });

  it('finds the deposit after a thrown call (reply lost) and moves to tracking', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error("Cannot read properties of undefined (reading '_arr')"));
    h.getDepositByRequest.mockResolvedValue(landed());
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'tracking' });
    expect((h.state() as { deposit: DepositView }).deposit.id).toBe(9n);
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'submitted', depositId: 9n });
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
  });

  it('treats Internal and Rpc errors as uncertain, not as rejections', async () => {
    for (const err of [{ Internal: 'boom' }, { Rpc: 'timeout' }, { Ledger: 'unreachable' }] as BridgeError[]) {
      const h = harness();
      h.bridgeToBase.mockResolvedValue({ Err: err });
      h.getDepositByRequest.mockResolvedValue(landed());
      await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
      expect(h.state().kind, JSON.stringify(Object.keys(err))).toBe('tracking');
    }
  });

  it('does not re-send automatically when the lookup finds nothing', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error('timeout'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'uncertain', lookup: 'not_found' });
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'prepared' });
  });

  it('a manual retry reuses the SAME client_request_id and ends in one deposit', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValueOnce(new Error('timeout'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    const uncertain = h.state() as { kind: 'uncertain'; intent: IcpToBaseIntent };
    expect(uncertain.kind).toBe('uncertain');

    h.bridgeToBase.mockResolvedValueOnce({ Ok: landed() });
    const promise = retryIcpToBase(h.deps, uncertain.intent, h.emit);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(2); // issued synchronously by the click
    await promise;

    const [first, second] = h.bridgeToBase.mock.calls.map((call) => call[0]);
    expect(bytesToHex(first.client_request_id)).toBe(REQUEST_ID);
    expect(bytesToHex(second.client_request_id)).toBe(REQUEST_ID);
    expect(second).toEqual(first);
    expect(h.store.list()).toHaveLength(1);
    expect(h.state()).toMatchObject({ kind: 'tracking' });
  });

  it('keeps the SAME id even if the second attempt is uncertain too', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error('timeout'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    const first = h.state() as { intent: IcpToBaseIntent };
    await retryIcpToBase(h.deps, first.intent, h.emit);
    await retryIcpToBase(h.deps, (h.state() as { intent: IcpToBaseIntent }).intent, h.emit);
    const ids = new Set(h.bridgeToBase.mock.calls.map((call) => bytesToHex(call[0].client_request_id)));
    expect([...ids]).toEqual([REQUEST_ID]);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(3);
    expect(h.store.list()).toHaveLength(1);
  });

  it('resolves through a public query, so no wallet type (Oisy included) ever gets an "unavailable" lookup', async () => {
    const h = harness();
    // The signing side has no caller-bound query at all; the lookup is anonymous.
    expect(Object.keys(h.deps.bridge).sort()).toEqual(['bridgeToBase', 'requestDepositRefund']);
    h.bridgeToBase.mockRejectedValueOnce(new Error('Cannot read properties of undefined (reading \'_arr\')'));
    h.getDepositByRequest.mockResolvedValue(landed());
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'tracking' });
    expect(h.events.map((e) => e.type)).not.toContain('lookup_unavailable');
  });

  it('asks get_deposit_by_request with the signed-in owner and the exact request id bytes', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error('timeout'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.getDepositByRequest).toHaveBeenCalledTimes(1);
    const [owner, requestId] = h.getDepositByRequest.mock.calls[0];
    expect(owner.toText()).toBe(ME);
    expect(Array.from(requestId as Uint8Array)).toEqual(Array.from({ length: 16 }, () => 0xcd));
  });

  it('reports the lookup as unavailable only when the query itself fails', async () => {
    const h = harness();
    h.bridgeToBase.mockRejectedValue(new Error('timeout'));
    h.getDepositByRequest.mockRejectedValue(new Error('offline'));
    await startIcpToBase(h.deps, quote({ allowanceE8s: 10n ** 12n }), h.emit);
    expect(h.state()).toMatchObject({ kind: 'uncertain', lookup: 'unavailable' });
    // A retry is still safe and reuses the same id.
    h.bridgeToBase.mockResolvedValueOnce({ Ok: landed() });
    await retryIcpToBase(h.deps, (h.state() as { intent: IcpToBaseIntent }).intent, h.emit);
    expect(h.state().kind).toBe('tracking');
  });

  it('does not trust an answer that is not this request (wrong id or wrong owner)', async () => {
    const h = harness();
    h.getDepositByRequest.mockResolvedValueOnce(makeDeposit({ client_request_id: new Uint8Array(16).fill(0x01) }));
    expect(await findDepositByRequestId(h.deps, REQUEST_ID)).toBe('unavailable');
    h.getDepositByRequest.mockResolvedValueOnce(
      makeDeposit({ client_request_id: new Uint8Array(16).fill(0xcd), owner: Principal.fromText('aaaaa-aa') }),
    );
    expect(await findDepositByRequestId(h.deps, REQUEST_ID)).toBe('unavailable');
  });

  it('finds the deposit regardless of the request id letter case, and returns null when the bridge has none', async () => {
    const h = harness();
    h.getDepositByRequest.mockResolvedValueOnce(landed());
    expect(await findDepositByRequestId(h.deps, REQUEST_ID.toUpperCase().replace('0X', '0x'))).toMatchObject({ id: 9n });
    h.getDepositByRequest.mockResolvedValueOnce(null);
    expect(await findDepositByRequestId(h.deps, REQUEST_ID)).toBeNull();
  });
});

describe('resumeIcpToBase (reload recovery)', () => {
  it('finds an earlier request by its id and tracks it', async () => {
    const h = harness();
    const earlier = intent({ clientRequestId: REQUEST_ID });
    h.store.put(earlier);
    h.getDepositByRequest.mockResolvedValue(makeDeposit({ id: 4n, client_request_id: new Uint8Array(16).fill(0xcd) }));
    await resumeIcpToBase(h.deps, earlier, h.emit);
    expect(h.state()).toMatchObject({ kind: 'tracking' });
    expect(h.store.get(REQUEST_ID)).toMatchObject({ status: 'submitted', depositId: 4n });
    expect(h.bridgeToBase).not.toHaveBeenCalled();
  });

  it('offers retry or discard when the earlier request never reached the bridge', async () => {
    const h = harness();
    const earlier = intent({ clientRequestId: REQUEST_ID });
    h.store.put(earlier);
    await resumeIcpToBase(h.deps, earlier, h.emit);
    expect(h.state()).toMatchObject({ kind: 'uncertain', lookup: 'not_found' });
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    discardIntent(h.deps, earlier);
    expect(h.store.list()).toEqual([]);
  });
});

describe('prepareIcpToBase', () => {
  function prepareDeps(over: Partial<PrepareDeps> = {}): PrepareDeps {
    return {
      fetchConfig: async () => ({ depositFeeE8s: DEPOSIT_FEE, bridgeLedgerFeeE8s: null, limits: LIMITS }),
      fetchLedgerFee: async () => LEDGER_FEE,
      readAllowance: async () => ({ allowance: 0n, expiresAtNs: null }),
      readBalance: async () => 10_000_000_000n,
      now: () => NOW,
      ...over,
    };
  }

  it('builds a quote from anonymous reads', async () => {
    const { quote: q, problem } = await prepareIcpToBase(prepareDeps(), { amountE8s: AMOUNT, recipient: RECIPIENT }, fmt);
    expect(problem).toBeNull();
    expect(q).toMatchObject({ needsApproval: true, ledgerFeeE8s: LEDGER_FEE, depositFeeE8s: DEPOSIT_FEE });
  });

  it('skips the approval when the allowance already covers amount + fee', async () => {
    const { quote: q } = await prepareIcpToBase(
      prepareDeps({ readAllowance: async () => ({ allowance: AMOUNT + LEDGER_FEE, expiresAtNs: BigInt(NOW + 3_600_000) * 1_000_000n }) }),
      { amountE8s: AMOUNT, recipient: RECIPIENT },
      fmt,
    );
    expect(q.needsApproval).toBe(false);
  });

  it('uses the bridge ledger fee when it is larger than the live one', async () => {
    const { quote: q } = await prepareIcpToBase(
      prepareDeps({ fetchConfig: async () => ({ depositFeeE8s: DEPOSIT_FEE, bridgeLedgerFeeE8s: 150_000n, limits: LIMITS }) }),
      { amountE8s: AMOUNT, recipient: RECIPIENT },
      fmt,
    );
    expect(q.ledgerFeeE8s).toBe(150_000n);
    expect(q.requiredAllowanceE8s).toBe(AMOUNT + 150_000n);
  });

  it('fails instead of guessing when the ledger fee cannot be read', async () => {
    await expect(
      prepareIcpToBase(prepareDeps({ fetchLedgerFee: async () => Promise.reject(new Error('ledger offline')) }), { amountE8s: AMOUNT, recipient: RECIPIENT }, fmt),
    ).rejects.toThrow('ledger offline');
  });

  it('flags a blocking problem with the quote', async () => {
    const { problem } = await prepareIcpToBase(prepareDeps({ readBalance: async () => 1n }), { amountE8s: AMOUNT, recipient: RECIPIENT }, fmt);
    expect(problem).toMatchObject({ code: 'insufficient_balance' });
  });
});

describe('trackDeposit', () => {
  it('maps successive canister statuses until delivery, then marks the intent done', async () => {
    const h = harness();
    h.store.put(intent({ status: 'submitted', depositId: 1n }));
    const sequence = [
      makeDeposit({ status: { PullPending: null } }),
      makeDeposit({ status: { Escrowed: null } }),
      makeDeposit({ status: { MintInFlight: null }, mint_stage: [{ Submitted: null }] }),
      makeDeposit({ status: { MintInFlight: null }, mint_stage: [{ Included: { block_number: 88n } }] }),
      makeDeposit({ status: { Delivered: null }, delivered: [{ block_number: 88n, block_hash: '0x1', tx_hash: `0x${'12'.repeat(32)}` }] }),
    ];
    let i = 0;
    h.getDeposit.mockImplementation(async () => sequence[Math.min(i++, sequence.length - 1)]);
    const seen: string[] = [];
    const result = await trackDeposit(h.deps, 1n, (d) => seen.push(Object.keys(d.status)[0] + (d.mint_stage[0] ? `:${Object.keys(d.mint_stage[0])[0]}` : '')));
    expect(result).toBe('done');
    expect(seen).toEqual(['PullPending', 'Escrowed', 'MintInFlight:Submitted', 'MintInFlight:Included', 'Delivered']);
    expect(h.store.list()[0]).toMatchObject({ status: 'done' });
  });

  it('keeps polling through a held deposit (it is not terminal)', async () => {
    const h = harness();
    const sequence = [makeDeposit({ status: { MintHeld: { reason: 'cap' } } }), makeDeposit({ status: { MintHeld: { reason: 'cap' } } }), makeDeposit({ status: { Delivered: null } })];
    let i = 0;
    h.getDeposit.mockImplementation(async () => sequence[i++]);
    const seen: DepositView[] = [];
    expect(await trackDeposit(h.deps, 1n, (d) => seen.push(d))).toBe('done');
    expect(seen).toHaveLength(3);
  });

  it('survives query errors and stops when aborted', async () => {
    const h = harness();
    const controller = new AbortController();
    const onError = vi.fn();
    h.getDeposit.mockRejectedValue(new Error('offline'));
    h.deps.sleep = async () => {
      controller.abort();
    };
    expect(await trackDeposit(h.deps, 1n, () => {}, controller.signal, onError)).toBe('aborted');
    expect(onError).toHaveBeenCalledTimes(1);
  });

  it('treats a missing deposit as an error, not as a status', async () => {
    const h = harness();
    const onError = vi.fn();
    h.getDeposit.mockResolvedValue(null);
    await trackDeposit(h.deps, 77n, () => {}, undefined, onError);
    // Runs to the attempt limit with a no-op sleep; every attempt reports the same error.
    expect(onError).toHaveBeenCalled();
    expect(onError.mock.calls[0][0].message).toContain('Deposit #77 was not found');
  });
});

describe('requestDepositRefund', () => {
  const held = () => makeDeposit({ id: 5n, status: { MintHeld: { reason: 'supply cap reached' } } });

  it('refunds a held deposit for its owner and returns the new view', async () => {
    const h = harness();
    const promise = requestDepositRefund(h.deps, held());
    expect(h.requestDepositRefund).toHaveBeenCalledWith(5n); // issued synchronously by the click
    const result = await promise;
    expect(result).toMatchObject({ ok: true });
    expect((result as { deposit: DepositView }).deposit.status).toEqual({ Refunding: null });
  });

  it('refuses every other status without calling the canister', async () => {
    const h = harness();
    for (const status of [
      { PullPending: null },
      { PullUnknown: null },
      { PullFailed: { reason: 'x' } },
      { Escrowed: null },
      { MintInFlight: null },
      { Delivered: null },
      { Refunding: null },
      { Refunded: null },
    ] as const) {
      const result = await requestDepositRefund(h.deps, makeDeposit({ status: status as any }));
      expect(result.ok, JSON.stringify(status)).toBe(false);
    }
    expect(h.requestDepositRefund).not.toHaveBeenCalled();
  });

  it('refuses a deposit owned by someone else', async () => {
    const h = harness();
    const stranger = makeDeposit({ status: { MintHeld: { reason: 'x' } }, owner: Principal.fromText('aaaaa-aa') });
    expect((await requestDepositRefund(h.deps, stranger)).ok).toBe(false);
    expect(h.requestDepositRefund).not.toHaveBeenCalled();
  });

  it('does not call the canister if the session changed', async () => {
    const h = harness();
    h.current.value = false;
    expect((await requestDepositRefund(h.deps, held())).ok).toBe(false);
    expect(h.requestDepositRefund).not.toHaveBeenCalled();
  });

  it('surfaces a canister error and a thrown call', async () => {
    const h = harness();
    h.requestDepositRefund.mockResolvedValueOnce({ Err: { InvalidState: 'mint not proven absent' } });
    expect(await requestDepositRefund(h.deps, held())).toMatchObject({ ok: false });
    h.requestDepositRefund.mockRejectedValueOnce(new Error('timeout'));
    const thrown = await requestDepositRefund(h.deps, held());
    expect(thrown.ok).toBe(false);
    expect((thrown as { message: string }).message).toMatch(/Check the deposit status/);
  });
});

describe('request id format', () => {
  it('is 16 bytes of randomness encoded as hex', async () => {
    const h = harness();
    await startIcpToBase(h.deps, quote(), h.emit);
    const stored = h.store.list()[0] as IcpToBaseIntent;
    expect(hexToBytes(stored.clientRequestId)).toHaveLength(16);
  });
});
