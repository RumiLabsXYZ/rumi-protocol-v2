import { describe, expect, it, vi } from 'vitest';
import {
  buildRepaymentPayment,
  buildTrustSetTx,
  claimActionsFor,
  configGate,
  createNonceStore,
  createXrplIcusdApi,
  estimateBorrow,
  formatE8s,
  mapConfig,
  mapOperation,
  parseE8s,
  parseIssuedValueToE8s,
  repaymentSummary,
  RepaymentMismatchError,
  type RepaymentIntent,
  type XrplIcusdConfigView,
} from './xrplIcusdService';

const ISSUER = 'rIssuer1111111111111111111111111';
const CURRENCY = '6963555344000000000000000000000000000000';
const USER = 'rUser1111111111111111111111111111';

function config(over: Partial<XrplIcusdConfigView> = {}): XrplIcusdConfigView {
  return {
    enabled: true,
    ready: true,
    issuancePaused: false,
    settlementPaused: false,
    issuerAddress: ISSUER,
    currencyHex: CURRENCY,
    network: 'testnet',
    perOperationCapE8s: 100_000_000_000n,
    outstandingCapE8s: 100_000_000_000_000n,
    configVersion: 1n,
    ...over,
  };
}

function view(over: Record<string, unknown> = {}): any {
  return {
    observed_exact_amount: [],
    kind: { Borrow: null },
    net_e8s: 99_00000000n,
    recipient: [USER],
    network: { Testnet: null },
    fee_e8s: 1_00000000n,
    xrpl_tx_hashes: [],
    updated_at_ns: 1n,
    operation_id: 5n,
    vault_id: [7n],
    ic_ledger_blocks: [],
    resolution: [],
    created_at_ns: 1n,
    held_exact_amount: [],
    next_action: { Wait: null },
    notes: [],
    debt_applied_e8s: 0n,
    phase: { DebtCommitted: null },
    gross_e8s: 100_00000000n,
    settlement_cutoff_ns: [],
    ...over,
  };
}

function intent(over: Partial<RepaymentIntent> = {}, payment: Record<string, unknown> = {}): RepaymentIntent {
  return {
    operationId: 9n,
    issuer: ISSUER,
    currency: CURRENCY,
    payer: USER,
    destinationTag: 424242,
    maxE8s: 40_00000000n,
    network: 'testnet',
    quoteTimeNs: 1n,
    paymentJson: JSON.stringify({
      TransactionType: 'Payment',
      Account: USER,
      Destination: ISSUER,
      DestinationTag: 424242,
      Amount: { currency: CURRENCY, issuer: ISSUER, value: '40' },
      ...payment,
    }),
    ...over,
  };
}

describe('exact icUSD money', () => {
  it('parses and formats e8s without Number', () => {
    expect(parseE8s('99.12345678')).toBe(99_12345678n);
    expect(parseE8s('1')).toBe(1_00000000n);
    expect(parseE8s('0.00000001')).toBe(1n);
    for (const bad of ['', '.', '1.', '.5', '-1', '1e3', '0.123456789', '1,5', 'abc', '1.2.3']) {
      expect(parseE8s(bad), bad).toBeNull();
    }
    expect(parseE8s('184467440737.09551615')).toBe((1n << 64n) - 1n);
    expect(parseE8s('184467440737.09551616')).toBeNull();
    expect(formatE8s(99_00000000n)).toBe('99.00');
    expect(formatE8s(1n)).toBe('0.00000001');
    expect(formatE8s(123456789_12345678n)).toBe('123456789.12345678');
  });
  it('parses issued values exactly or refuses', () => {
    expect(parseIssuedValueToE8s('40')).toBe(40_00000000n);
    expect(parseIssuedValueToE8s('40.5')).toBe(40_50000000n);
    expect(parseIssuedValueToE8s('40.500000000')).toBe(40_50000000n);
    expect(parseIssuedValueToE8s('1e1')).toBeNull();
    expect(parseIssuedValueToE8s('0.000000001')).toBeNull();
    expect(parseIssuedValueToE8s('-1')).toBeNull();
  });
});

describe('config gate', () => {
  it('maps config and explains disabled / paused states', () => {
    const raw: any = {
      settlement_paused: false, issuer_setup: { NotStarted: null }, verified_minting_account: [],
      escrow_account: {}, issuer_address: [ISSUER], max_fee_drops: 20n, network: [{ Testnet: null }],
      enabled: true, per_operation_cap_e8s: 5n, outstanding_cap_e8s: 6n, config_version: 3n,
      currency_hex: CURRENCY, ready: true, issuance_paused: false, policy: [],
    };
    const cfg = mapConfig(raw);
    expect(cfg.issuerAddress).toBe(ISSUER);
    expect(cfg.network).toBe('testnet');
    expect(configGate(cfg).state).toBe('active');
    expect(configGate({ ...cfg, enabled: false }).state).toBe('disabled');
    expect(configGate({ ...cfg, ready: false }).state).toBe('not_ready');
    expect(configGate({ ...cfg, issuancePaused: true }).state).toBe('paused');
    expect(configGate({ ...cfg, settlementPaused: true }).state).toBe('paused');
    expect(configGate({ ...cfg, issuancePaused: true }).message).toMatch(/paused/i);
  });
});

describe('trustline_exact_identity_and_limit', () => {
  it('builds the TrustSet only from backend config and the current connection', () => {
    const tx = buildTrustSetTx(config(), { address: USER, network: 'testnet' });
    expect(tx).toEqual({
      TransactionType: 'TrustSet',
      LimitAmount: { currency: CURRENCY, issuer: ISSUER, value: '1000000' },
    });
  });
  it('refuses a wrong network, missing connection or unusable config', () => {
    expect(() => buildTrustSetTx(config(), { address: USER, network: 'mainnet' })).toThrow(/network/i);
    expect(() => buildTrustSetTx(config(), null)).toThrow(/connect/i);
    expect(() => buildTrustSetTx(config({ enabled: false }), { address: USER, network: 'testnet' })).toThrow();
    expect(() => buildTrustSetTx(config({ issuerAddress: null }), { address: USER, network: 'testnet' })).toThrow();
    expect(() => buildTrustSetTx(config({ currencyHex: 'USD' }), { address: USER, network: 'testnet' })).toThrow(/currency/i);
  });
  it('uppercases the currency and formats a fractional limit exactly', () => {
    const tx = buildTrustSetTx(config({ currencyHex: CURRENCY.toLowerCase(), outstandingCapE8s: 12_50000000n }), { address: USER, network: 'testnet' });
    expect(tx.LimitAmount.currency).toBe(CURRENCY);
    expect(tx.LimitAmount.value).toBe('12.5');
  });
});

describe('debt_and_net_are_distinct', () => {
  it('estimates gross debt, fee and net separately', () => {
    const q = estimateBorrow(100_00000000n, 0.01);
    expect(q.grossE8s).toBe(100_00000000n);
    expect(q.feeE8s).toBe(1_00000000n);
    expect(q.netE8s).toBe(99_00000000n);
    expect(q.netE8s).not.toBe(q.grossE8s);
  });
  it('keeps debt recorded and net delivered apart on the operation', () => {
    const ui = mapOperation(view({ phase: { Delivered: null } }));
    expect(ui.grossE8s).toBe(100_00000000n);
    expect(ui.netE8s).toBe(99_00000000n);
    expect(ui.feeE8s).toBe(1_00000000n);
    expect(ui.deliveredE8s).toBe(99_00000000n);
  });
});

describe('pending_delivery_does_not_show_delivered', () => {
  it('labels pre-delivery phases as delivery pending and never as delivered', () => {
    for (const phase of ['DebtCommitted', 'IssuePrepared', 'Signed', 'Submitted']) {
      const ui = mapOperation(view({ phase: { [phase]: null } }));
      expect(ui.state, phase).toBe('delivery_pending');
      expect(ui.label).toBe('Debt recorded; icUSD delivery pending');
      expect(ui.deliveredE8s).toBeNull();
      expect(ui.label).not.toMatch(/delivered$/i);
    }
    expect(mapOperation(view({ phase: { Prepared: null } })).state).toBe('processing');
    expect(mapOperation(view({ phase: { EscrowMintPending: null } })).state).toBe('processing');
    expect(mapOperation(view({ phase: { Delivered: null } })).state).toBe('delivered');
  });
});

describe('repay_uses_exact_token_and_intent_tag', () => {
  it('builds the issued Payment only from the verified intent', () => {
    const tx = buildRepaymentPayment(intent(), config(), { address: USER, network: 'testnet' });
    expect(tx).toEqual({
      TransactionType: 'Payment',
      Destination: ISSUER,
      DestinationTag: 424242,
      Amount: { currency: CURRENCY, issuer: ISSUER, value: '40' },
    });
    expect(tx).not.toHaveProperty('Paths');
    expect(tx).not.toHaveProperty('SendMax');
    expect(tx).not.toHaveProperty('Flags');
  });

  const cases: Array<[string, RepaymentIntent]> = [
    ['wrong issuer in amount', intent({}, { Amount: { currency: CURRENCY, issuer: 'rEvil', value: '40' } })],
    ['wrong currency', intent({}, { Amount: { currency: 'USD', issuer: ISSUER, value: '40' } })],
    ['native XRP amount', intent({}, { Amount: '40000000' })],
    ['wrong destination', intent({}, { Destination: 'rEvil' })],
    ['wrong tag', intent({}, { DestinationTag: 1 })],
    ['missing tag', intent({}, { DestinationTag: undefined })],
    ['over max', intent({}, { Amount: { currency: CURRENCY, issuer: ISSUER, value: '40.00000001' } })],
    ['zero', intent({}, { Amount: { currency: CURRENCY, issuer: ISSUER, value: '0' } })],
    ['sub-e8s', intent({}, { Amount: { currency: CURRENCY, issuer: ISSUER, value: '39.123456789' } })],
    ['paths', intent({}, { Paths: [[{ currency: 'XRP' }]] })],
    ['sendmax', intent({}, { SendMax: { currency: CURRENCY, issuer: ISSUER, value: '50' } })],
    ['delivermin', intent({}, { DeliverMin: { currency: CURRENCY, issuer: ISSUER, value: '1' } })],
    ['partial payment flag', intent({}, { Flags: 0x00020000 })],
    ['wrong account', intent({}, { Account: 'rOther' })],
    ['not a payment', intent({}, { TransactionType: 'TrustSet' })],
    ['intent issuer mismatch', intent({ issuer: 'rEvil' })],
    ['intent payer mismatch', intent({ payer: 'rOther' })],
    ['intent network mismatch', intent({ network: 'mainnet' })],
    ['intent tag mismatch', intent({ destinationTag: 7 })],
    ['garbage json', intent({ paymentJson: '{nope' })],
  ];
  it.each(cases)('payment_json mismatch refused: %s', (_name, bad) => {
    expect(() => buildRepaymentPayment(bad, config(), { address: USER, network: 'testnet' })).toThrow(RepaymentMismatchError);
  });
  it('refuses when the connected wallet is not the intent payer or is on another network', () => {
    expect(() => buildRepaymentPayment(intent(), config(), { address: 'rOther', network: 'testnet' })).toThrow(RepaymentMismatchError);
    expect(() => buildRepaymentPayment(intent(), config(), { address: USER, network: 'mainnet' })).toThrow(RepaymentMismatchError);
    expect(() => buildRepaymentPayment(intent(), config(), null)).toThrow(RepaymentMismatchError);
  });
});

describe('repay_actual_residual_displayed', () => {
  it('shows what the backend applied and held, not what the wallet sent', () => {
    const op = view({
      kind: { Repayment: null },
      phase: { Held: null },
      observed_exact_amount: ['40'],
      debt_applied_e8s: 0n,
      held_exact_amount: ['40'],
      next_action: { ResolveClaim: null },
    });
    const s = repaymentSummary(mapOperation(op), 60_00000000n);
    expect(s.receivedExact).toBe('40');
    expect(s.appliedE8s).toBe(0n);
    expect(s.heldExact).toBe('40');
    expect(s.remainingDebtE8s).toBe(60_00000000n);

    const partial = mapOperation(view({
      kind: { Repayment: null }, phase: { Complete: null },
      observed_exact_amount: ['40'], debt_applied_e8s: 39_50000000n, held_exact_amount: ['0.5'],
    }));
    const s2 = repaymentSummary(partial, null);
    expect(s2.appliedE8s).toBe(39_50000000n);
    expect(s2.heldExact).toBe('0.5');
    expect(s2.remainingDebtE8s).toBeNull();
  });
  it('maps repayment phases to pending, received, debt updated', () => {
    const st = (p: string) => mapOperation(view({ kind: { Repayment: null }, phase: { [p]: null } })).state;
    expect(st('IntentPrepared')).toBe('repay_payment_pending');
    expect(st('ReturnObserved')).toBe('repay_xrpl_received');
    expect(st('ReturnVerified')).toBe('repay_xrpl_received');
    expect(st('EscrowBurnPending')).toBe('repay_xrpl_received');
    expect(st('DebtApplied')).toBe('repay_debt_updated');
    expect(st('Complete')).toBe('complete');
  });
});

describe('held_refund_claim_has_action', () => {
  it('offers claim actions for a held or refund-pending operation', () => {
    const held = mapOperation(view({ kind: { Repayment: null }, phase: { Held: null }, next_action: { ResolveClaim: null }, held_exact_amount: ['40'] }));
    expect(held.state).toBe('held');
    expect(held.claimActions).toEqual(['ReceiveCanonical', 'ReissueXrpl']);
    const refund = mapOperation(view({ kind: { Refund: null }, phase: { RefundPending: null }, next_action: { ResolveClaim: null } }));
    expect(refund.state).toBe('refund_pending');
    expect(refund.claimActions.length).toBeGreaterThan(0);
  });
  it('offers cancel only for an unexecuted repayment burn', () => {
    const op = mapOperation(view({ kind: { Repayment: null }, phase: { EscrowBurnPending: null }, next_action: { ResolveClaim: null } }));
    expect(claimActionsFor(op.raw)).toEqual(['CancelUnexecutedRepayment']);
  });
  it('offers nothing once resolved or when no action is expected', () => {
    expect(mapOperation(view({ phase: { Held: null }, next_action: { ResolveClaim: null }, resolution: [{ ReceiveCanonical: null }] })).claimActions).toEqual([]);
    expect(mapOperation(view({ phase: { Held: null }, next_action: { Wait: null } })).claimActions).toEqual([]);
  });
  it('shows resume for a non-terminal operation and flags operator contact', () => {
    expect(mapOperation(view({ phase: { Signed: null } })).canResume).toBe(true);
    expect(mapOperation(view({ phase: { Delivered: null } })).canResume).toBe(false);
    const c = mapOperation(view({ phase: { Signed: null }, next_action: { ContactOperator: null } }));
    expect(c.state).toBe('recoverable_error');
  });
});

describe('request nonces', () => {
  function mem() {
    const m = new Map<string, string>();
    return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => { m.set(k, v); }, removeItem: (k: string) => { m.delete(k); } };
  }
  it('creates 32-byte nonces from crypto and reuses them per intent across reloads', () => {
    const storage = mem();
    const a = createNonceStore({ storage, owner: 'p' });
    const n1 = a.getOrCreate('borrow|7|100');
    expect(n1.length).toBe(32);
    expect(a.getOrCreate('borrow|7|100')).toEqual(n1);
    const b = createNonceStore({ storage, owner: 'p' });
    expect(b.getOrCreate('borrow|7|100')).toEqual(n1);
    expect(b.getOrCreate('borrow|7|101')).not.toEqual(n1);
    b.forget('borrow|7|100');
    expect(createNonceStore({ storage, owner: 'p' }).getOrCreate('borrow|7|100')).not.toEqual(n1);
  });
  it('still reuses within a session when storage throws', () => {
    const bad = { getItem: () => { throw new Error('x'); }, setItem: () => { throw new Error('x'); }, removeItem: () => { throw new Error('x'); } };
    const s = createNonceStore({ storage: bad, owner: 'p' });
    expect(s.getOrCreate('k')).toEqual(s.getOrCreate('k'));
  });
  it('is isolated per owner', () => {
    const storage = mem();
    expect(createNonceStore({ storage, owner: 'a' }).getOrCreate('k')).not.toEqual(createNonceStore({ storage, owner: 'b' }).getOrCreate('k'));
  });
});

describe('api config availability', () => {
  it('reports unavailable instead of crashing when the actor lacks the config method', async () => {
    const api = createXrplIcusdApi({ getActor: async () => ({}) as any, getPublicActor: async () => ({}) as any });
    expect(await api.getConfig()).toMatchObject({ ok: false, kind: 'unavailable' });
    // A missing update method is a transport failure, never a crash and never "definitive".
    const r = await api.borrowToXrpl({ vaultId: 7n, grossE8s: 1n, recipient: USER, network: 'testnet', requestNonce: new Uint8Array(32) });
    expect(r).toMatchObject({ ok: false, kind: 'network', definitive: false });
  });
  it('passes nonce, network variant and optional top-up to the actor and maps errors', async () => {
    const prepare = vi.fn(async () => ({ Ok: {
      operation_id: 9n, issuer: ISSUER, currency: CURRENCY, payer: USER, destination_tag: 424242, max_e8s: 40_00000000n,
      network: { Testnet: null }, payment_json: '{}', quote_time_ns: 5n,
    } }));
    const api = createXrplIcusdApi({ getActor: async () => ({ prepare_xrpl_repayment: prepare }) as any, getPublicActor: async () => ({}) as any });
    const nonce = new Uint8Array(32).fill(3);
    const r = await api.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, payer: USER, network: 'testnet', requestNonce: nonce, fullClose: true, topUpOperation: 4n });
    expect(r.ok).toBe(true);
    expect(prepare).toHaveBeenCalledWith({
      vault_id: 7n, max_e8s: 40_00000000n, payer: USER, network: { Testnet: null }, request_nonce: nonce, full_close: true, top_up_operation: [4n],
    });
    if (r.ok) expect(r.data.destinationTag).toBe(424242);

    const err = createXrplIcusdApi({ getActor: async () => ({ prepare_xrpl_repayment: async () => ({ Err: { TrustLineMissing: 'no line' } }) }) as any, getPublicActor: async () => ({}) as any });
    const e = await err.prepareRepayment({ vaultId: 7n, maxE8s: 1n, payer: USER, network: 'testnet', requestNonce: nonce, fullClose: false, topUpOperation: null });
    expect(e).toMatchObject({ ok: false, kind: 'backend' });
    if (!e.ok) expect(e.error).toMatch(/trust line/i);
  });
});

describe('final Candid shapes', () => {
  it('maps the held phase, cutoff and ResolveClaim actions from the generated OperationView', () => {
    const ui = mapOperation(view({
      kind: { Repayment: null }, phase: { Held: null }, next_action: { ResolveClaim: null },
      held_exact_amount: ['12.5'], settlement_cutoff_ns: [99n],
    }));
    expect(ui.state).toBe('held');
    expect(ui.heldExact).toBe('12.5');
    expect(ui.settlementCutoffNs).toBe(99n);
    expect(ui.claimActions).toEqual(['ReceiveCanonical', 'ReissueXrpl']);
  });
  it('offers only CancelUnexecutedRepayment while the escrow burn is pending', () => {
    expect(claimActionsFor(view({ kind: { Repayment: null }, phase: { EscrowBurnPending: null }, next_action: { ResolveClaim: null } })))
      .toEqual(['CancelUnexecutedRepayment']);
  });
  it('formats every error variant in the generated XrplIcusdError', async () => {
    const { formatXrplIcusdError } = await import('./xrplIcusdService');
    for (const k of ['InvalidArgument', 'NotApplicable', 'Policy', 'CoverageUnavailable', 'MintingAccount', 'Unavailable', 'Internal']) {
      expect(formatXrplIcusdError({ [k]: 'why' })).toContain('why');
    }
    expect(formatXrplIcusdError({ NonceConflict: null })).toMatch(/already used/);
  });
  it('observeRepayment returns the backend operation view and sends the generated argument shape', async () => {
    const observe = vi.fn(async () => ({ Ok: view({ kind: { Repayment: null }, phase: { ReturnVerified: null } }) }));
    const api = createXrplIcusdApi({ getActor: async () => ({ observe_xrpl_repayment: observe }) as any, getPublicActor: async () => ({}) as any });
    const r = await api.observeRepayment(9n, 'ABCD');
    expect(observe).toHaveBeenCalledWith({ operation_id: 9n, tx_hash: 'ABCD' });
    expect(r.ok && r.data.state).toBe('repay_xrpl_received');
  });
});
