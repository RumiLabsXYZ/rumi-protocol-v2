import { describe, expect, it, vi } from 'vitest';
import {
  createXrplIcusdController,
  fetchOwnerReserve,
  fetchTrustLine,
  importDisclosure,
  type XrplIcusdControllerDeps,
} from './xrplIcusdPanelController';
import { mapOperation, type RepaymentIntent, type XrplIcusdConfigView } from '$lib/services/xrplIcusdService';
import type { WalletSubmission, XrplConnection } from '$lib/services/xrplWalletService';

const ISSUER = 'rIssuer1111111111111111111111111';
const CURRENCY = '6963555344000000000000000000000000000000';
const USER = 'rUser1111111111111111111111111111';
const OTHER = 'rOther111111111111111111111111111';

function mem() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => { m.set(k, v); }, removeItem: (k: string) => { m.delete(k); }, m };
}

function cfg(over: Partial<XrplIcusdConfigView> = {}): XrplIcusdConfigView {
  return {
    enabled: true, ready: true, issuancePaused: false, settlementPaused: false, issuerAddress: ISSUER,
    currencyHex: CURRENCY, network: 'testnet', perOperationCapE8s: 1_000_000_00000000n,
    outstandingCapE8s: 1_000_000_00000000n, configVersion: 1n, ...over,
  };
}

function opView(over: Record<string, unknown> = {}): any {
  return {
    observed_exact_amount: [], kind: { Borrow: null }, net_e8s: 99_00000000n, recipient: [USER], network: { Testnet: null },
    fee_e8s: 1_00000000n, xrpl_tx_hashes: [], updated_at_ns: 1n, operation_id: 5n, vault_id: [7n], ic_ledger_blocks: [],
    resolution: [], created_at_ns: 1n, held_exact_amount: [], next_action: { Wait: null }, notes: [], debt_applied_e8s: 0n,
    phase: { DebtCommitted: null }, gross_e8s: 100_00000000n, settlement_cutoff_ns: [], ...over,
  };
}
const op = (over: Record<string, unknown> = {}) => mapOperation(opView(over));

function intent(over: Partial<RepaymentIntent> = {}): RepaymentIntent {
  return {
    operationId: 9n, issuer: ISSUER, currency: CURRENCY, payer: USER, destinationTag: 777, maxE8s: 40_00000000n,
    network: 'testnet', quoteTimeNs: 1n,
    paymentJson: JSON.stringify({ TransactionType: 'Payment', Account: USER, Destination: ISSUER, DestinationTag: 777,
      Amount: { currency: CURRENCY, issuer: ISSUER, value: '40' } }),
    ...over,
  };
}

function makeWallet(initial: XrplConnection | null = { address: USER, network: 'testnet' }) {
  let conn = initial;
  const handlers = new Set<(c: XrplConnection | null) => void>();
  const prompts = new Set<(p: any) => void>();
  return {
    id: 'xaman' as const,
    availability: () => ({ available: true }),
    connect: vi.fn(async () => conn!),
    getConnection: () => conn,
    signAndSubmit: vi.fn(async (): Promise<WalletSubmission> => ({ requestId: 'req-1', status: 'submitted', signed: true, hash: 'HASH1' })),
    resumeRequest: vi.fn(async (id: string): Promise<WalletSubmission> => ({ requestId: id, status: 'submitted', signed: true, hash: 'HASH1' })),
    disconnect: vi.fn(),
    onChange: (h: (c: XrplConnection | null) => void) => { handlers.add(h); return () => handlers.delete(h); },
    onPrompt: (h: (p: any) => void) => { prompts.add(h); return () => prompts.delete(h); },
    emitPrompt(p: any) { prompts.forEach((h) => h(p)); },
    setConn(c: XrplConnection | null) { conn = c; handlers.forEach((h) => h(c)); },
  };
}

function ok<T>(data: T) { return { ok: true as const, data }; }

function makeApi(over: Record<string, any> = {}) {
  return {
    getConfig: vi.fn(async () => ok(cfg())),
    listMyOperations: vi.fn(async () => ok([] as any[])),
    issueFromCanonical: vi.fn(async () => ok(op({ kind: { Import: null }, operation_id: 11n, vault_id: [], phase: { Prepared: null }, fee_e8s: 0n, gross_e8s: 5_00000000n, net_e8s: 5_00000000n }))),
    resume: vi.fn(async () => ok(op({ phase: { Signed: null } }))),
    resolveClaim: vi.fn(async () => ok(op({ phase: { ClaimResolved: null } }))),
    borrowToXrpl: vi.fn(async () => ok(op())),
    prepareRepayment: vi.fn(async () => ok(intent())),
    observeRepayment: vi.fn(async () => ok(op({ kind: { Repayment: null }, operation_id: 9n, phase: { ReturnVerified: null } }))),
    ...over,
  };
}

function rpcFetch(handlers: { account_lines?: any; server_state?: any }) {
  return vi.fn(async (_url: string, init: any) => {
    const body = JSON.parse(init.body);
    const result = handlers[body.method as 'account_lines' | 'server_state'];
    if (result instanceof Error) throw result;
    return { ok: true, json: async () => ({ result }) } as any;
  });
}

const LINE_OK = { status: 'success', validated: true, lines: [{ account: ISSUER, currency: CURRENCY, limit: '1000000', balance: '12.5' }] };

function setup(opts: { oisy?: boolean; api?: any; wallet?: ReturnType<typeof makeWallet>; fetchImpl?: any; approve?: any; ledgerFee?: any; storage?: any } = {}) {
  const api = opts.api ?? makeApi();
  const wallet = opts.wallet ?? makeWallet();
  const storage = opts.storage ?? mem();
  const fetchImpl = opts.fetchImpl ?? rpcFetch({ account_lines: { status: 'success', validated: true, lines: [] }, server_state: { status: 'success', state: { validated_ledger: { reserve_inc: 200000 } } } });
  const approve = opts.approve ?? vi.fn(async () => ({ success: true }));
  const ledgerFee = opts.ledgerFee ?? vi.fn(async () => 10_000n);
  const deps: XrplIcusdControllerDeps = {
    api, wallet, isOisy: () => !!opts.oisy, ownerKey: 'owner-1', storage, fetchImpl,
    endpoints: () => ['https://rpc.test'], sleep: async () => {}, approve, ledgerFee, now: () => 1,
  };
  const c = createXrplIcusdController(deps);
  return { c, api, wallet, storage, fetchImpl, approve, ledgerFee, deps };
}

/** Backend view of a repayment still waiting for its payment. */
const preparedOp = () => op({ kind: { Repayment: null }, operation_id: 9n, phase: { IntentPrepared: null } });
/** An API whose operation list shows repayment #9 waiting for payment (what the backend returns after prepare). */
const repayApi = (over: Record<string, any> = {}) => makeApi({ listMyOperations: vi.fn(async () => ok([preparedOp()])), ...over });

const updateMethods = ['issueFromCanonical', 'resume', 'resolveClaim', 'borrowToXrpl', 'prepareRepayment', 'observeRepayment'] as const;

describe('trust line (hint only, exact identity)', () => {
  it('present only when the exact issuer and currency line exists', async () => {
    const good = setup({ fetchImpl: rpcFetch({ account_lines: LINE_OK }) });
    await good.c.init();
    expect(good.c.getState().trustLine.status).toBe('present');
    expect(good.c.getState().trustLine.limit).toBe('1000000');

    const wrongCurrency = setup({ fetchImpl: rpcFetch({ account_lines: { status: 'success', validated: true, lines: [{ account: ISSUER, currency: 'USD', limit: '5' }] } }) });
    await wrongCurrency.c.init();
    expect(wrongCurrency.c.getState().trustLine.status).toBe('missing');

    const wrongIssuer = setup({ fetchImpl: rpcFetch({ account_lines: { status: 'success', validated: true, lines: [{ account: OTHER, currency: CURRENCY, limit: '5' }] } }) });
    await wrongIssuer.c.init();
    expect(wrongIssuer.c.getState().trustLine.status).toBe('missing');
  });

  it('fetchTrustLine queries account_lines filtered by the issuer peer at a validated ledger', async () => {
    const f = rpcFetch({ account_lines: LINE_OK });
    const line = await fetchTrustLine(USER, ISSUER, CURRENCY, ['https://rpc.test'], f as any);
    expect(line).toMatchObject({ present: true, limit: '1000000' });
    const body = JSON.parse((f.mock.calls[0] as any)[1].body);
    expect(body).toMatchObject({ method: 'account_lines', params: [{ account: USER, peer: ISSUER, ledger_index: 'validated' }] });
  });

  it('shows the live owner reserve, with a labelled fallback when RPC fails', async () => {
    const live = await fetchOwnerReserve(['https://rpc.test'], rpcFetch({ server_state: { status: 'success', state: { validated_ledger: { reserve_inc: 200000 } } } }) as any);
    expect(live).toEqual({ drops: 200000n, live: true });
    const down = await fetchOwnerReserve(['https://rpc.test'], rpcFetch({ server_state: new Error('offline') }) as any);
    expect(down).toEqual({ drops: 200000n, live: false });
  });

  it('wallet success for the TrustSet does not mark the trust line present', async () => {
    const t = setup({ fetchImpl: rpcFetch({ account_lines: { status: 'success', validated: true, lines: [] } }) });
    await t.c.init();
    t.c.proposeTrustSet();
    await t.c.signTrustSet();
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    const sent = (t.wallet.signAndSubmit.mock.calls as any)[0][0];
    expect(sent).toEqual({ TransactionType: 'TrustSet', Account: USER, LimitAmount: { currency: CURRENCY, issuer: ISSUER, value: '1000000' } });
    expect(t.c.getState().trustSet.phase).toBe('submitted');
    expect(t.c.getState().trustLine.status).not.toBe('present');
  });

  it('wallet rejection creates no fake success', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'r', status: 'rejected', signed: false, hash: null });
    const t = setup({ wallet });
    await t.c.init();
    t.c.proposeTrustSet();
    await t.c.signTrustSet();
    expect(t.c.getState().trustSet.phase).toBe('rejected');
    expect(t.c.getState().trustLine.status).not.toBe('present');
  });
});

describe('changed_wallet_cancels_stale_request', () => {
  it('a TrustSet proposal is invalidated by an account change and cannot be signed', async () => {
    const t = setup();
    await t.c.init();
    t.c.proposeTrustSet();
    expect(t.c.getState().trustSet.phase).toBe('proposed');
    t.wallet.setConn({ address: OTHER, network: 'testnet' });
    expect(t.c.getState().trustSet.phase).toBe('invalidated');
    await t.c.signTrustSet();
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
  });
  it('a network change invalidates it too and resets the trust line hint', async () => {
    const t = setup({ fetchImpl: rpcFetch({ account_lines: LINE_OK }) });
    await t.c.init();
    expect(t.c.getState().trustLine.status).toBe('present');
    t.c.proposeTrustSet();
    t.wallet.setConn({ address: USER, network: 'mainnet' });
    expect(t.c.getState().trustSet.phase).toBe('invalidated');
    expect(t.c.getState().trustLine.status).not.toBe('present');
  });
  it('a prepared repayment is not signed after the payer wallet changed', async () => {
    const t = setup();
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    expect(t.c.getState().repay.phase).toBe('prepared');
    t.wallet.setConn({ address: OTHER, network: 'testnet' });
    expect(t.c.getState().repay.phase).toBe('invalidated');
    await t.c.payRepayment();
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
  });
});

describe('repayment flow', () => {
  it('signs exactly the verified intent payment with an idempotency key per intent', async () => {
    const t = setup({ api: repayApi() });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    const [tx, opts] = (t.wallet.signAndSubmit.mock.calls as any)[0];
    expect(tx).toEqual({ TransactionType: 'Payment', Account: USER, Destination: ISSUER, DestinationTag: 777, Amount: { currency: CURRENCY, issuer: ISSUER, value: '40' } });
    expect(opts).toEqual({ key: 'xrpl-icusd-repay:9' });
  });

  it('refuses a mismatched payment_json before the wallet is asked', async () => {
    const api = makeApi({ prepareRepayment: vi.fn(async () => ok(intent({
      paymentJson: JSON.stringify({ TransactionType: 'Payment', Destination: 'rEvil', DestinationTag: 777, Amount: { currency: CURRENCY, issuer: ISSUER, value: '40' } }),
    }))) });
    const t = setup({ api });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(t.c.getState().repay.phase).toBe('error');
    expect(t.c.getState().repay.error).toMatch(/refused/i);
  });

  it('wallet rejection creates no fake success and no observation', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'r', status: 'rejected', signed: false, hash: null });
    const t = setup({ wallet, api: repayApi() });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    expect(t.c.getState().repay.phase).toBe('rejected');
    expect(t.api.observeRepayment).not.toHaveBeenCalled();
  });

  it('provider submission success does not mark debt paid (Oisy waits for a click)', async () => {
    const api = makeApi({ listMyOperations: vi.fn(async () => ok([op({ kind: { Repayment: null }, operation_id: 9n, phase: { IntentPrepared: null } })])) });
    const t = setup({ oisy: true, api });
    await t.c.init();
    await t.c.refresh();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    expect(t.c.getState().repay.phase).toBe('submitted');
    expect(t.api.observeRepayment).not.toHaveBeenCalled();
    expect(t.c.getState().operations[0].state).toBe('repay_payment_pending');
    await t.c.observeRepayment();
    expect(t.api.observeRepayment).toHaveBeenCalledWith(9n, 'HASH1');
  });

  it('non-Oisy sessions report the hash to the backend once, automatically', async () => {
    const t = setup({ api: repayApi() });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    expect(t.api.observeRepayment).toHaveBeenCalledTimes(1);
    expect(t.api.observeRepayment).toHaveBeenCalledWith(9n, 'HASH1');
  });

  it('stale full-close quote never triggers a silent second payment', async () => {
    const held = op({ kind: { Repayment: null }, operation_id: 9n, phase: { Held: null }, next_action: { ResolveClaim: null }, observed_exact_amount: ['40'], held_exact_amount: ['40'] });
    let backendOp = preparedOp(); // the backend moves to Held only after the payment is observed
    const api = makeApi({
      listMyOperations: vi.fn(async () => ok([backendOp, op({ kind: { Repayment: null }, operation_id: 10n, phase: { IntentPrepared: null } })])),
      observeRepayment: vi.fn(async () => { backendOp = held; return ok(held); }),
    });
    const t = setup({ api });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: true });
    await t.c.payRepayment();
    await t.c.refresh();
    expect(t.c.getState().operations.find((o) => o.id === 9n)!.state).toBe('held');
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    expect(t.api.prepareRepayment).toHaveBeenCalledTimes(1);
    // a poll must not sign or prepare anything either
    await t.c.pollTick();
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    expect(t.api.prepareRepayment).toHaveBeenCalledTimes(1);
    // explicit "Add payment" prepares a child bound to the parent but does not sign by itself
    t.api.prepareRepayment.mockResolvedValueOnce(ok(intent({ operationId: 10n, maxE8s: 2_00000000n,
      paymentJson: JSON.stringify({ TransactionType: 'Payment', Destination: ISSUER, DestinationTag: 778, Amount: { currency: CURRENCY, issuer: ISSUER, value: '2' } }), destinationTag: 778 })));
    await t.c.addPayment(9n, { vaultId: 7n, maxE8s: 2_00000000n });
    expect(t.api.prepareRepayment).toHaveBeenCalledTimes(2);
    expect((t.api.prepareRepayment.mock.calls as any)[1][0]).toMatchObject({ topUpOperation: 9n, vaultId: 7n, payer: USER });
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    await t.c.payRepayment();
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(2);
  });

  it('a second payRepayment for the same intent does not create another wallet request', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'req-1', status: 'pending', signed: false, hash: null });
    const t = setup({ wallet, api: repayApi() });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    await t.c.payRepayment();
    expect(wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    expect(wallet.resumeRequest).toHaveBeenCalledWith('req-1');
  });

  it('reuses the request nonce when a prepare is retried after a lost reply', async () => {
    const fails = makeApi();
    fails.prepareRepayment = vi.fn()
      .mockResolvedValueOnce({ ok: false, kind: 'network', definitive: false, error: 'timeout' })
      .mockResolvedValueOnce(ok(intent()))
      .mockResolvedValueOnce(ok(intent()));
    const t = setup({ api: fails });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    const n1 = (fails.prepareRepayment.mock.calls as any)[0][0].requestNonce;
    const n2 = (fails.prepareRepayment.mock.calls as any)[1][0].requestNonce;
    expect(n1.length).toBe(32);
    expect(n2).toEqual(n1);
    // The nonce is KEPT while the operation is still IntentPrepared (a reload + click returns the same operation)...
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    expect(fails.prepareRepayment).toHaveBeenCalledTimes(3);
    expect((fails.prepareRepayment.mock.calls as any)[2][0].requestNonce).toEqual(n1);
    // ...and forgotten once the backend shows the operation has left IntentPrepared.
    fails.listMyOperations.mockResolvedValue(ok([op({ kind: { Repayment: null }, operation_id: 9n, phase: { ReturnVerified: null } })]));
    await t.c.refresh();
    t.c.cancelRepayment();
    fails.prepareRepayment.mockResolvedValueOnce(ok(intent({ operationId: 12n })));
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    expect((fails.prepareRepayment.mock.calls as any)[3][0].requestNonce).not.toEqual(n1);
  });
});

describe('borrow', () => {
  it('sends gross debt, the connected address and network with a persisted nonce', async () => {
    const t = setup();
    await t.c.init();
    await t.c.borrow({ vaultId: 7n, grossE8s: 100_00000000n });
    const arg = (t.api.borrowToXrpl.mock.calls as any)[0][0];
    expect(arg).toMatchObject({ vaultId: 7n, grossE8s: 100_00000000n, recipient: USER, network: 'testnet' });
    expect(arg.requestNonce.length).toBe(32);
    const s = t.c.getState();
    expect(s.operations[0].state).toBe('delivery_pending');
    expect(s.operations[0].deliveredE8s).toBeNull();
    expect(s.operations[0].grossE8s).not.toBe(s.operations[0].netE8s);
  });
  it('retries with the same nonce after an ambiguous failure, then a new one after success', async () => {
    const api = makeApi();
    api.borrowToXrpl = vi.fn()
      .mockResolvedValueOnce({ ok: false, kind: 'network', definitive: false, error: 'lost' })
      .mockResolvedValueOnce(ok(op()))
      .mockResolvedValueOnce(ok(op({ operation_id: 6n })));
    const t = setup({ api });
    await t.c.init();
    const args = { vaultId: 7n, grossE8s: 100_00000000n };
    await t.c.borrow(args);
    await t.c.borrow(args);
    await t.c.borrow(args);
    const calls = api.borrowToXrpl.mock.calls as any;
    expect(calls[1][0].requestNonce).toEqual(calls[0][0].requestNonce);
    expect(calls[2][0].requestNonce).not.toEqual(calls[1][0].requestNonce);
  });
  it('reports not available instead of crashing when the endpoint is missing', async () => {
    const api = makeApi({
      borrowToXrpl: vi.fn(async () => ({ ok: false, kind: 'unavailable', definitive: true, error: 'Borrowing to XRPL is not available on this deployment yet.' })),
    });
    const t = setup({ api });
    await t.c.init();
    await t.c.borrow({ vaultId: 7n, grossE8s: 100_00000000n });
    expect(t.c.getState().borrow.error).toMatch(/not available/i);
    expect(t.c.getState().borrow.phase).toBe('error');
  });
  it('refuses new actions when the lane is disabled or paused', async () => {
    for (const over of [{ enabled: false }, { issuancePaused: true }]) {
      const api = makeApi({ getConfig: vi.fn(async () => ok(cfg(over))) });
      const t = setup({ api });
      await t.c.init();
      expect(t.c.getState().gate?.state).not.toBe('active');
      await t.c.borrow({ vaultId: 7n, grossE8s: 100_00000000n });
      await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 1n, fullClose: false });
      t.c.proposeTrustSet();
      expect(api.borrowToXrpl).not.toHaveBeenCalled();
      expect(api.prepareRepayment).not.toHaveBeenCalled();
      expect(t.c.getState().trustSet.phase).toBe('idle');
    }
  });
  it('does not borrow into a wallet on the wrong network', async () => {
    const t = setup({ wallet: makeWallet({ address: USER, network: 'mainnet' }) });
    await t.c.init();
    await t.c.borrow({ vaultId: 7n, grossE8s: 100_00000000n });
    expect(t.api.borrowToXrpl).not.toHaveBeenCalled();
    expect(t.c.getState().borrow.error).toMatch(/network/i);
  });
});

describe('reload_recovers_from_backend', () => {
  it('rebuilds every state from backend operations (newest first) with no local state at all', async () => {
    const ops = [
      op({ operation_id: 3n, phase: { DebtCommitted: null } }),
      op({ operation_id: 4n, kind: { Repayment: null }, phase: { ReturnObserved: null } }),
      op({ operation_id: 5n, kind: { Repayment: null }, phase: { Held: null }, next_action: { ResolveClaim: null }, held_exact_amount: ['1'] }),
      op({ operation_id: 6n, phase: { Delivered: null } }),
    ];
    const api = makeApi({ listMyOperations: vi.fn(async () => ok(ops)) });
    const t = setup({ api, storage: mem() });
    await t.c.init();
    const s = t.c.getState();
    expect(s.operationsSource).toBe('backend');
    expect(s.operations.map((o) => o.state)).toEqual(['delivered', 'held', 'repay_xrpl_received', 'delivery_pending']);
    expect(s.operations[1].claimActions).toEqual(['ReceiveCanonical', 'ReissueXrpl']);
  });

  it('resumes a stored wallet request for a pending repayment and reports it once (non-Oisy)', async () => {
    const storage = mem();
    storage.setItem('rumi_xrpl_icusd_repay_v1:owner-1', JSON.stringify({ '9': { requestId: 'req-9' } }));
    const api = makeApi({ listMyOperations: vi.fn(async () => ok([op({ operation_id: 9n, kind: { Repayment: null }, phase: { IntentPrepared: null } })])) });
    const t = setup({ api, storage });
    await t.c.init();
    expect(t.wallet.resumeRequest).toHaveBeenCalledWith('req-9');
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(api.observeRepayment).toHaveBeenCalledWith(9n, 'HASH1');
  });

  it('Oisy reload shows cached operations and makes no authenticated call until Refresh is clicked', async () => {
    const storage = mem();
    const first = setup({ oisy: false, storage, api: makeApi({ listMyOperations: vi.fn(async () => ok([op({ operation_id: 3n })])) }) });
    await first.c.init();
    const api2 = makeApi({ listMyOperations: vi.fn(async () => ok([op({ operation_id: 3n }), op({ operation_id: 4n })])) });
    const second = setup({ oisy: true, storage, api: api2 });
    await second.c.init();
    expect(api2.listMyOperations).not.toHaveBeenCalled();
    expect(second.c.getState().operationsSource).toBe('cache');
    expect(second.c.getState().operations.map((o) => o.id)).toEqual([3n]);
    await second.c.refresh();
    expect(api2.listMyOperations).toHaveBeenCalledTimes(1);
    expect(second.c.getState().operations.length).toBe(2);
    expect(second.c.getState().operationsSource).toBe('backend');
  });
});

describe('import (Move existing icUSD to XRPL)', () => {
  it('discloses allowance and ledger fees exactly', () => {
    const d = importDisclosure(5_00000000n, 10_000n);
    expect(d.allowanceE8s).toBe(5_00010000n);
    expect(d.totalLedgerFeesE8s).toBe(20_000n);
    expect(d.debitedE8s).toBe(5_00020000n);
  });
  it('import creates no debt: approve then issue, and never touches borrow or repay', async () => {
    const t = setup();
    await t.c.init();
    await t.c.importCanonical(5_00000000n);
    expect(t.approve).toHaveBeenCalledWith(5_00010000n);
    expect(t.api.issueFromCanonical).toHaveBeenCalledTimes(1);
    expect((t.api.issueFromCanonical.mock.calls as any)[0][0]).toMatchObject({ amountE8s: 5_00000000n, recipient: USER, network: 'testnet' });
    expect(t.approve.mock.invocationCallOrder[0]).toBeLessThan(t.api.issueFromCanonical.mock.invocationCallOrder[0]);
    expect(t.api.borrowToXrpl).not.toHaveBeenCalled();
    expect(t.api.prepareRepayment).not.toHaveBeenCalled();
    const o = t.c.getState().operations[0];
    expect(o.kind).toBe('Import');
    expect(o.debtAppliedE8s).toBe(0n);
    expect(o.feeE8s).toBe(0n);
  });
  it('Oisy: approve and issue are two separate clicks', async () => {
    const t = setup({ oisy: true });
    await t.c.init();
    await t.c.importCanonical(5_00000000n);
    expect(t.approve).toHaveBeenCalledTimes(1);
    expect(t.api.issueFromCanonical).not.toHaveBeenCalled();
    expect(t.c.getState().importFlow.phase).toBe('approved');
    await t.c.finishImport();
    expect(t.api.issueFromCanonical).toHaveBeenCalledTimes(1);
    expect(t.c.getState().importFlow.phase).toBe('done');
  });
  it('a wallet change between approve and issue cancels the import', async () => {
    const t = setup({ oisy: true });
    await t.c.init();
    await t.c.importCanonical(5_00000000n);
    t.wallet.setConn({ address: OTHER, network: 'testnet' });
    await t.c.finishImport();
    expect(t.api.issueFromCanonical).not.toHaveBeenCalled();
    expect(t.c.getState().importFlow.phase).toBe('invalidated');
  });
  it('a failed approval issues nothing', async () => {
    const t = setup({ approve: vi.fn(async () => ({ success: false, error: 'declined' })) });
    await t.c.init();
    await t.c.importCanonical(5_00000000n);
    expect(t.api.issueFromCanonical).not.toHaveBeenCalled();
    expect(t.c.getState().importFlow.phase).toBe('error');
  });
});

describe('recovery actions', () => {
  it('resume and claim resolution are explicit clicks that update the operation', async () => {
    const held = op({ operation_id: 5n, kind: { Repayment: null }, phase: { Held: null }, next_action: { ResolveClaim: null } });
    const api = makeApi({ listMyOperations: vi.fn(async () => ok([held])) });
    const t = setup({ api });
    await t.c.init();
    await t.c.resolveClaim(5n, 'ReceiveCanonical');
    expect(api.resolveClaim).toHaveBeenCalledWith(5n, 'ReceiveCanonical');
    await t.c.resume(5n);
    expect(api.resume).toHaveBeenCalledWith(5n);
  });
});

describe('Oisy: authenticated calls only from click handlers', () => {
  it('polling makes queries only: no update, and no caller-scoped read', async () => {
    const t = setup({ oisy: true });
    await t.c.init();
    const baseline = t.api.listMyOperations.mock.calls.length;
    for (let i = 0; i < 5; i++) await t.c.pollTick();
    for (const m of updateMethods) expect((t.api as any)[m]).not.toHaveBeenCalled();
    expect(t.api.listMyOperations.mock.calls.length).toBe(baseline);
    expect(t.api.getConfig.mock.calls.length).toBeGreaterThan(1);
  });
  it('non-Oisy polling reads operations but still never calls an update', async () => {
    const t = setup();
    await t.c.init();
    const baseline = t.api.listMyOperations.mock.calls.length;
    await t.c.pollTick();
    expect(t.api.listMyOperations.mock.calls.length).toBe(baseline + 1);
    for (const m of updateMethods) expect((t.api as any)[m]).not.toHaveBeenCalled();
  });
  it('an automatic (non-click) observe is blocked for Oisy', async () => {
    const t = setup({ oisy: true });
    await t.c.init();
    await t.c.prepareRepayment({ vaultId: 7n, maxE8s: 40_00000000n, fullClose: false });
    await t.c.payRepayment();
    await t.c.observeRepayment({ auto: true });
    expect(t.api.observeRepayment).not.toHaveBeenCalled();
    await t.c.observeRepayment();
    expect(t.api.observeRepayment).toHaveBeenCalledTimes(1);
  });
});

describe('recovery review fixes', () => {
  const PAY = { vaultId: 7n, maxE8s: 40_00000000n, fullClose: false };
  const PROMPT = { requestId: 'req-open', deepLink: 'https://xumm.app/sign/req-open', qrPng: 'https://xumm.app/sign/req-open_q.png' };

  it('TrustSet: an engine failure from the wallet is shown as failed, never submitted', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'r', status: 'rejected', signed: false, hash: null, warning: 'The XRP Ledger refused this transaction (tecNO_LINE_INSUF_RESERVE). Nothing was applied.' });
    const t = setup({ wallet });
    await t.c.init();
    t.c.proposeTrustSet();
    await t.c.signTrustSet();
    expect(t.c.getState().trustSet.phase).toBe('rejected');
    expect(t.c.getState().trustSet.error).toMatch(/tecNO_LINE_INSUF_RESERVE/);
    // and the user can ask again (a fresh request), not stuck
    await t.c.signTrustSet();
    expect(wallet.signAndSubmit).toHaveBeenCalledTimes(2);
  });

  it('TrustSet: when the line never appears the user is offered Enable icUSD again', async () => {
    const t = setup({ fetchImpl: rpcFetch({ account_lines: { status: 'success', validated: true, lines: [] } }) });
    await t.c.init();
    t.c.proposeTrustSet();
    await t.c.signTrustSet();
    expect(t.c.getState().trustSet.phase).toBe('submitted');
    await vi.waitFor(() => expect(t.c.getState().trustSet.phase).toBe('idle'));
    expect(t.c.getState().trustSet.error).toMatch(/enable icUSD again/i);
    expect(t.c.getState().trustLine.status).toBe('missing');
    t.c.proposeTrustSet();
    expect(t.c.getState().trustSet.phase).toBe('proposed');
  });

  it('exposes the wallet prompt (deep link + QR) while a request is open and clears it afterwards', async () => {
    const wallet = makeWallet();
    let release: (s: WalletSubmission) => void = () => {};
    wallet.signAndSubmit.mockImplementation(() => new Promise<WalletSubmission>((r) => { release = r; }));
    const t = setup({ wallet });
    await t.c.init();
    t.c.proposeTrustSet();
    const p = t.c.signTrustSet();
    await Promise.resolve();
    wallet.emitPrompt(PROMPT);
    expect(t.c.getState().prompt).toMatchObject({ ...PROMPT, flow: 'trustset' });
    release({ requestId: 'req-open', status: 'rejected', signed: false, hash: null });
    await p;
    expect(t.c.getState().prompt).toBeNull();
  });

  it('persists {operationId, requestId, paymentJson} the moment the request exists, then a reload resumes it without a second payment', async () => {
    const storage = mem();
    const wallet = makeWallet();
    wallet.signAndSubmit.mockImplementation(async () => {
      wallet.emitPrompt(PROMPT);
      return new Promise<WalletSubmission>(() => {}); // the tab "reloads" while Xaman is open
    });
    const a = setup({ storage, wallet, api: repayApi() });
    await a.c.init();
    await a.c.prepareRepayment(PAY);
    void a.c.payRepayment();
    await vi.waitFor(() => expect(a.c.getState().prompt?.flow).toBe('repay'));
    const hints = JSON.parse(storage.m.get('rumi_xrpl_icusd_repay_v1:owner-1')!);
    expect(hints['9']).toMatchObject({ requestId: 'req-open', account: USER });
    expect(JSON.parse(hints['9'].paymentJson)).toMatchObject({ TransactionType: 'Payment', Account: USER, Destination: ISSUER, DestinationTag: 777 });
    expect(hints['9'].intent).toMatchObject({ operationId: '9', payer: USER, destinationTag: 777 });
    a.c.dispose();

    // Reload: new controller + wallet, same storage and backend.
    const wallet2 = makeWallet();
    wallet2.resumeRequest.mockResolvedValue({ requestId: 'req-open', status: 'submitted', signed: true, hash: 'HASH9' });
    const b = setup({ storage, wallet: wallet2, api: repayApi() });
    await b.c.init();
    expect(wallet2.resumeRequest).toHaveBeenCalledWith('req-open', USER);
    expect(wallet2.signAndSubmit).not.toHaveBeenCalled();
    expect(b.api.prepareRepayment).not.toHaveBeenCalled();
    expect(b.c.getState().repay).toMatchObject({ operationId: 9n, hash: 'HASH9' });
    expect(b.c.getState().repay.intent?.destinationTag).toBe(777);
    // non-Oisy reports the txid to the backend automatically
    expect(b.api.observeRepayment).toHaveBeenCalledWith(9n, 'HASH9');
  });

  it('Oisy: a resumed signed payment waits for the observe click', async () => {
    const storage = mem();
    storage.setItem('rumi_xrpl_icusd_repay_v1:owner-1', JSON.stringify({ '9': { requestId: 'req-open', account: USER, intent: {
      operationId: '9', issuer: ISSUER, currency: CURRENCY, payer: USER, destinationTag: 777, maxE8s: '4000000000', network: 'testnet', paymentJson: '{}', quoteTimeNs: '1',
    } } }));
    storage.setItem('rumi_xrpl_icusd_ops_v1:owner-1', JSON.stringify([preparedOp().raw].map((r) => JSON.parse(JSON.stringify(r, (_k, v) => (typeof v === 'bigint' ? { $b: v.toString() } : v))))));
    const wallet = makeWallet();
    wallet.resumeRequest.mockResolvedValue({ requestId: 'req-open', status: 'submitted', signed: true, hash: 'HASH9' });
    const t = setup({ storage, wallet, oisy: true });
    await t.c.init();
    expect(wallet.resumeRequest).toHaveBeenCalledTimes(1);
    expect(t.c.getState().repay.phase).toBe('submitted');
    expect(t.api.observeRepayment).not.toHaveBeenCalled();
    expect(t.api.listMyOperations).not.toHaveBeenCalled();
    await t.c.observeRepayment();
    expect(t.api.observeRepayment).toHaveBeenCalledWith(9n, 'HASH9');
  });

  it('a prepared-but-unpaid repayment is restored after reload and never prepared twice', async () => {
    const storage = mem();
    const a = setup({ storage, api: repayApi() });
    await a.c.init();
    await a.c.prepareRepayment(PAY);
    const nonce1 = (a.api.prepareRepayment.mock.calls as any)[0][0].requestNonce;
    a.c.dispose();
    const b = setup({ storage, api: repayApi() });
    await b.c.init();
    expect(b.c.getState().repay).toMatchObject({ phase: 'prepared', operationId: 9n });
    expect(b.api.prepareRepayment).not.toHaveBeenCalled();
    // even a repeated Prepare click reuses the SAME nonce (backend returns the same operation)
    b.c.cancelRepayment();
    await b.c.prepareRepayment(PAY);
    expect((b.api.prepareRepayment.mock.calls as any)[0][0].requestNonce).toEqual(nonce1);
  });

  it('refuses to pay when the backend operation is no longer IntentPrepared (non-Oisy re-reads first)', async () => {
    let backendOp = preparedOp();
    const api = repayApi({ listMyOperations: vi.fn(async () => ok([backendOp])) });
    const t = setup({ api });
    await t.c.init();
    await t.c.prepareRepayment(PAY);
    backendOp = op({ kind: { Repayment: null }, operation_id: 9n, phase: { ReturnVerified: null } });
    await t.c.payRepayment();
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(t.c.getState().repay.phase).toBe('error');
    expect(t.c.getState().repay.error).toMatch(/no longer waiting for a payment/i);
  });

  it('refuses a quote older than 7 days', async () => {
    const day = 86_400_000;
    let clock = 1_000;
    const api = repayApi({ prepareRepayment: vi.fn(async () => ok(intent({ quoteTimeNs: 1_000n * 1_000_000n }))) });
    const t = setup({ api });
    const c = createXrplIcusdController({ ...t.deps, now: () => clock });
    await c.init();
    await c.prepareRepayment(PAY);
    clock = 1_000 + 7 * day + 1;
    await c.payRepayment();
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(c.getState().repay.error).toMatch(/expired/i);
    clock = 1_000 + 6 * day;
    await c.prepareRepayment(PAY);
    await c.payRepayment();
    expect(t.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
  });

  it('Oisy uses the cached operation view for the staleness check (no signer read)', async () => {
    const api = repayApi();
    const t = setup({ oisy: true, api });
    await t.c.init();
    await t.c.refresh(); // click: loads the cached view with op 9 = ReturnVerified
    api.listMyOperations.mockResolvedValue(ok([op({ kind: { Repayment: null }, operation_id: 9n, phase: { ReturnVerified: null } })]));
    await t.c.refresh();
    await t.c.prepareRepayment(PAY);
    const reads = api.listMyOperations.mock.calls.length;
    await t.c.payRepayment();
    expect(api.listMyOperations.mock.calls.length).toBe(reads); // no extra authenticated read
    expect(t.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(t.c.getState().repay.error).toMatch(/no longer waiting/i);
  });

  it('Oisy observe click makes exactly one authenticated call (no refreshOps after it)', async () => {
    const t = setup({ oisy: true });
    await t.c.init();
    await t.c.prepareRepayment(PAY);
    await t.c.payRepayment();
    const reads = t.api.listMyOperations.mock.calls.length;
    await t.c.observeRepayment();
    expect(t.api.observeRepayment).toHaveBeenCalledTimes(1);
    expect(t.api.listMyOperations.mock.calls.length).toBe(reads);
    expect(t.c.getState().operations.find((o) => o.id === 9n)).toBeTruthy(); // merged from the observe reply
    expect(t.c.getState().repay.phase).toBe('observed');
  });

  it('a payment signed by another account or refused by the ledger shows the wallet warning, never submitted', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'r', status: 'rejected', signed: false, hash: null, warning: 'The request was signed by rAttacker, not the connected wallet. It was not used.' });
    const t = setup({ wallet, api: repayApi() });
    await t.c.init();
    await t.c.prepareRepayment(PAY);
    await t.c.payRepayment();
    expect(t.c.getState().repay.phase).toBe('rejected');
    expect(t.c.getState().repay.error).toMatch(/rAttacker/);
    expect(t.api.observeRepayment).not.toHaveBeenCalled();
  });
});
