import { describe, expect, it, vi } from 'vitest';
import {
  createXrpDepositController,
  fetchValidatedBalanceDrops,
  xrpDepositBreakdown,
  type XrpDepositDeps,
} from './xrpDepositFlow';
import type { XrplConnection, WalletSubmission } from '$lib/services/xrplWalletService';

const CUSTODY = 'rCustody1111111111111111111111';
const USER = 'rUser';
const RESERVE = 1_000_000n;

function memoryStorage() {
  const m = new Map<string, string>();
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => { m.set(k, v); },
    removeItem: (k: string) => { m.delete(k); },
    m,
  };
}

function makeWallet(initial: XrplConnection | null = { address: USER, network: 'testnet' }) {
  let conn = initial;
  const handlers = new Set<(c: XrplConnection | null) => void>();
  const wallet = {
    id: 'xaman' as const,
    availability: () => ({ available: true }),
    connect: vi.fn(async () => conn!),
    getConnection: () => conn,
    signAndSubmit: vi.fn(async (): Promise<WalletSubmission> => ({ requestId: 'req-1', status: 'submitted', signed: true, hash: 'HASH1' })),
    resumeRequest: vi.fn(async (id: string): Promise<WalletSubmission> => ({ requestId: id, status: 'submitted', signed: true, hash: 'HASH1' })),
    disconnect: vi.fn(),
    onChange: (h: (c: XrplConnection | null) => void) => { handlers.add(h); return () => handlers.delete(h); },
    onPrompt: () => () => {},
    setConn(c: XrplConnection | null) { conn = c; handlers.forEach((h) => h(c)); },
    setConnSilently(c: XrplConnection | null) { conn = c; },
  };
  return wallet;
}

function makeVault(opts: { pending?: number[]; confirmResult?: any; owns?: boolean; readFails?: boolean } = {}) {
  // Simulated backend list: open() adds vault 7 to it, like the real canister.
  const list = (opts.pending ?? []).map((vaultId) => ({ vaultId, custodyAddress: CUSTODY, reserveBaseDrops: RESERVE, openedAtMs: vaultId }));
  return {
    open: vi.fn(async () => {
      list.push({ vaultId: 7, custodyAddress: CUSTODY, reserveBaseDrops: RESERVE, openedAtMs: 7 });
      return { success: true, data: { vaultId: 7, custodyAddress: CUSTODY, reserveBaseDrops: RESERVE } };
    }),
    readPendingDeposits: vi.fn(async () =>
      opts.readFails ? { ok: false as const, error: 'replica unreachable' } : { ok: true as const, deposits: [...list] }),
    confirm: vi.fn(async () => opts.confirmResult ?? ({ success: true, data: { creditedDrops: 11_345_678n } })),
    ownsVault: vi.fn(async () => opts.owns ?? true),
  };
}

function rpcFetch(balance: string | 'unfunded') {
  return vi.fn(async (_url: string, init: any) => {
    const body = JSON.parse(init.body);
    expect(body.method).toBe('account_info');
    expect(body.params[0]).toMatchObject({ account: CUSTODY, ledger_index: 'validated' });
    const result = balance === 'unfunded'
      ? { status: 'error', error: 'actNotFound' }
      : { status: 'success', validated: true, account_data: { Balance: balance } };
    return { ok: true, json: async () => ({ result }) } as Response;
  });
}

function setup(over: Partial<XrpDepositDeps> & { wallet?: ReturnType<typeof makeWallet>; vault?: ReturnType<typeof makeVault>; oisy?: boolean; balance?: string | 'unfunded'; storage?: ReturnType<typeof memoryStorage> } = {}) {
  const wallet = over.wallet ?? makeWallet();
  const vault = over.vault ?? makeVault();
  const storage = over.storage ?? memoryStorage();
  const fetchImpl = (over.fetchImpl as any) ?? rpcFetch(over.balance ?? '12345678');
  let clock = 0;
  const sleeps: number[] = [];
  const controller = createXrpDepositController({
    wallet: wallet as any,
    vault,
    isOisy: () => over.oisy ?? false,
    network: 'testnet',
    ownerKey: 'owner-1',
    storage,
    fetchImpl,
    sleep: async (ms) => { sleeps.push(ms); clock += ms; },
    now: () => clock,
    minCollateralDrops: 1n,
    ...(over as any),
  });
  return { wallet, vault, storage, fetchImpl, controller, sleeps };
}

describe('XrpBorrowModal deposit flow', () => {
  it('deposit_requests_exact_drops_to_backend_address', async () => {
    const { controller, wallet, vault } = setup();
    await controller.prepare(12_345_678n);
    await controller.prepare(12_345_678n); // idempotent
    controller.propose();
    await controller.pay();
    expect(vault.open).toHaveBeenCalledTimes(1);
    expect(wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    const [tx] = wallet.signAndSubmit.mock.calls[0] as any[];
    expect(tx).toEqual({ TransactionType: 'Payment', Account: USER, Destination: CUSTODY, Amount: '12345678' });
    expect(typeof tx.Amount).toBe('string');
  });

  it('reject_cancel_without_credit', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'req-1', status: 'rejected', signed: false, hash: null });
    const { controller, vault, fetchImpl } = setup({ wallet });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    const s = controller.getState();
    expect(s.phase).toBe('rejected');
    expect(s.creditedDrops).toBeNull();
    expect(vault.confirm).not.toHaveBeenCalled();
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it('reload_recovers_same_pending_vault', async () => {
    const storage = memoryStorage();
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'req-1', status: 'pending', signed: true, hash: null });
    const a = setup({ storage, wallet, balance: 'unfunded' });
    await a.controller.prepare(12_345_678n);
    a.controller.propose();
    await a.controller.pay();
    expect(a.controller.getState().phase).toBe('wallet_pending');

    // "Reload": brand-new controller, same storage and backend (vault 7 still pending).
    const vault = makeVault({ pending: [7] });
    const b = setup({ storage, wallet, vault, balance: '12345678' });
    await b.controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    expect(b.controller.getState().vault).toMatchObject({ vaultId: 7, custodyAddress: CUSTODY });
    await b.controller.resume();
    expect(wallet.resumeRequest).toHaveBeenCalledWith('req-1');
    expect(wallet.signAndSubmit).toHaveBeenCalledTimes(1); // no second payment
  });

  it('account_or_network_change_invalidates_proposal', async () => {
    const a = setup();
    await a.controller.prepare(5_000_000n);
    a.controller.propose();
    a.wallet.setConn({ address: 'rOther', network: 'testnet' });
    expect(a.controller.getState().phase).toBe('invalidated');
    await a.controller.pay();
    expect(a.wallet.signAndSubmit).not.toHaveBeenCalled();

    const b = setup();
    await b.controller.prepare(5_000_000n);
    b.controller.propose();
    b.wallet.setConn({ address: USER, network: 'mainnet' });
    expect(b.controller.getState().phase).toBe('invalidated');
    await b.controller.pay();
    expect(b.wallet.signAndSubmit).not.toHaveBeenCalled();

    // Change without an event is still caught at submit time (stale snapshot).
    const c = setup();
    await c.controller.prepare(5_000_000n);
    c.controller.propose();
    c.wallet.setConnSilently({ address: 'rOther', network: 'testnet' });
    await c.controller.pay();
    expect(c.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(c.controller.getState().phase).toBe('invalidated');
    expect(c.vault.open).toHaveBeenCalledTimes(1); // re-proposing does not open another vault
    c.wallet.setConnSilently({ address: USER, network: 'testnet' });
    c.controller.propose();
    await c.controller.pay();
    expect(c.wallet.signAndSubmit).toHaveBeenCalledTimes(1);
    expect(c.vault.open).toHaveBeenCalledTimes(1);
  });

  it('wallet_hash_does_not_credit_collateral', async () => {
    const { controller, vault, sleeps } = setup({ balance: 'unfunded' });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    const s = controller.getState();
    expect(s.txHash).toBe('HASH1');
    expect(s.creditedDrops).toBeNull();
    expect(vault.confirm).not.toHaveBeenCalled();
    expect(s.phase).toBe('timed_out'); // 2 minutes of unfunded polling, then Resume
    expect(sleeps[0]).toBe(2000);
    expect(Math.max(...sleeps)).toBeLessThanOrEqual(15000);
    expect(sleeps.reduce((a, b) => a + b, 0)).toBeLessThanOrEqual(120_000 + 15_000);
    // Backoff is monotonic.
    for (let i = 1; i < sleeps.length; i++) expect(sleeps[i]).toBeGreaterThanOrEqual(sleeps[i - 1]);
  });

  it('oisy_confirmation_requires_click_without_background_signer_calls', async () => {
    const { controller, vault } = setup({ oisy: true, balance: '99999999999' });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    expect(controller.getState().phase).toBe('ready_to_confirm');
    await Promise.resolve();
    expect(vault.confirm).not.toHaveBeenCalled();
    expect(vault.readPendingDeposits).not.toHaveBeenCalledTimes(99); // sanity: no polling signer reads
    await controller.confirmFromClick();
    expect(vault.confirm).toHaveBeenCalledTimes(1);
    expect(vault.confirm).toHaveBeenCalledWith(7);
    expect(controller.getState()).toMatchObject({ phase: 'confirmed', creditedDrops: 11_345_678n });
    await controller.confirmFromClick(); // double click: no second call
    expect(vault.confirm).toHaveBeenCalledTimes(1);
  });

  it('delegated sessions confirm exactly once automatically', async () => {
    const { controller, vault } = setup({ oisy: false });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    expect(vault.confirm).toHaveBeenCalledTimes(1);
    expect(controller.getState()).toMatchObject({ phase: 'confirmed', creditedDrops: 11_345_678n });
  });

  it('a spoofed browser balance only enables the confirm step; backend result is authority', async () => {
    const vault = makeVault({ confirmResult: { success: false, error: 'No XRP has reached this custody address yet.' }, owns: false });
    const { controller } = setup({ oisy: true, balance: '99999999999999', vault });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    expect(controller.getState().phase).toBe('ready_to_confirm');
    await controller.confirmFromClick();
    const s = controller.getState();
    expect(s.creditedDrops).toBeNull();
    expect(s.phase).not.toBe('confirmed');
    expect(s.error).toMatch(/custody address/);
  });

  it('already-consumed pending deposit is verified against the owner vault list, never a failed deposit', async () => {
    const consumed = { success: false, error: 'No pending XRP deposit for this vault (already confirmed or unknown).' };
    const ok = setup({ oisy: true, vault: makeVault({ confirmResult: consumed, owns: true }) });
    await ok.controller.prepare(12_345_678n);
    ok.controller.propose();
    await ok.controller.pay();
    await ok.controller.confirmFromClick();
    expect(ok.vault.ownsVault).toHaveBeenCalledWith(7);
    expect(ok.controller.getState()).toMatchObject({ phase: 'confirmed', alreadyConfirmed: true, creditedDrops: null });
    expect(ok.wallet.signAndSubmit).toHaveBeenCalledTimes(1);

    // Vault not found: surface an error, do not claim success.
    const bad = setup({ oisy: true, vault: makeVault({ confirmResult: consumed, owns: false }) });
    await bad.controller.prepare(12_345_678n);
    bad.controller.propose();
    await bad.controller.pay();
    await bad.controller.confirmFromClick();
    expect(bad.controller.getState().phase).not.toBe('confirmed');
  });

  it('blocks a second payment to an already confirmed address', async () => {
    const storage = memoryStorage();
    const a = setup({ storage });
    await a.controller.prepare(12_345_678n);
    a.controller.propose();
    await a.controller.pay();
    expect(a.controller.getState().phase).toBe('confirmed');
    a.controller.propose();
    await a.controller.pay();
    expect(a.wallet.signAndSubmit).toHaveBeenCalledTimes(1);

    const b = setup({ storage, vault: makeVault({ pending: [] }) });
    await b.controller.prepare(12_345_678n);
    expect(b.vault.open).not.toHaveBeenCalled();
    b.controller.propose();
    await b.controller.pay();
    expect(b.wallet.signAndSubmit).not.toHaveBeenCalled();
    expect(b.controller.getState().phase).toBe('confirmed');
  });

  it('reserve_is_not_presented_as_collateral', async () => {
    const b = xrpDepositBreakdown(12_345_678n, RESERVE);
    expect(b).toEqual({ sentDrops: 12_345_678n, reservedDrops: 1_000_000n, expectedCreditedDrops: 11_345_678n, viable: true });
    expect(xrpDepositBreakdown(1_000_000n, RESERVE)).toMatchObject({ expectedCreditedDrops: 0n, viable: false });
    expect(xrpDepositBreakdown(500_000n, RESERVE)).toMatchObject({ expectedCreditedDrops: 0n, viable: false });

    const { controller } = setup({ oisy: true });
    await controller.prepare(12_345_678n);
    const s = controller.getState();
    expect(s.reservedDrops).toBe(1_000_000n);
    expect(s.expectedCreditedDrops).toBe(11_345_678n);
    expect(s.creditedDrops).toBeNull(); // only the backend confirm result fills this
    controller.propose();
    await controller.pay();
    expect(controller.getState().creditedDrops).toBeNull();
  });

  it('rejects a send that does not exceed the reserve plus minimum collateral', async () => {
    const { controller, vault } = setup();
    await controller.prepare(1_000_000n);
    expect(controller.getState().phase).toBe('error');
    controller.propose();
    await controller.pay();
    expect(vault.confirm).not.toHaveBeenCalled();
  });
});

describe('deposit reload never opens a second vault', () => {
  it('a failed backend read keeps the hint, blocks open and offers retry', async () => {
    const storage = memoryStorage();
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'req-1', status: 'pending', signed: true, hash: null });
    const a = setup({ storage, wallet, balance: 'unfunded' });
    await a.controller.prepare(12_345_678n);
    a.controller.propose();
    await a.controller.pay();
    const hintBefore = storage.m.get('rumi_xrpl_deposit_hint_v1:owner-1');
    expect(hintBefore).toBeTruthy();

    const failing = makeVault({ readFails: true });
    const b = setup({ storage, wallet, vault: failing });
    await b.controller.prepare(12_345_678n);
    expect(failing.open).not.toHaveBeenCalled();
    expect(failing.ownsVault).not.toHaveBeenCalled();
    expect(b.controller.getState()).toMatchObject({ phase: 'error', pendingReadFailed: true, vault: null });
    expect(b.controller.getState().error).toMatch(/Nothing was opened/);
    expect(storage.m.get('rumi_xrpl_deposit_hint_v1:owner-1')).toBe(hintBefore); // hint kept

    // Retry once the backend answers: same vault recovered, still no open.
    const healthy = makeVault({ pending: [7] });
    const c = setup({ storage, wallet, vault: healthy });
    await c.controller.prepare(12_345_678n);
    expect(healthy.open).not.toHaveBeenCalled();
    expect(c.controller.getState()).toMatchObject({ phase: 'wallet_pending', pendingReadFailed: false });
  });

  it('retrying prepare() after a failed read opens exactly one vault once the read works', async () => {
    let fail = true;
    const vault = makeVault();
    vault.readPendingDeposits.mockImplementation(async () => (fail ? { ok: false as const, error: 'down' } : { ok: true as const, deposits: [] }));
    const { controller } = setup({ vault });
    await controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    fail = false;
    await controller.prepare(12_345_678n);
    expect(vault.open).toHaveBeenCalledTimes(1);
    expect(controller.getState().phase).toBe('awaiting_payment');
  });

  it('with no hint, adopts an existing backend pending deposit (custody + reserve from the backend)', async () => {
    const vault = makeVault();
    vault.readPendingDeposits.mockResolvedValue({
      ok: true as const,
      deposits: [{ vaultId: 3, custodyAddress: 'rBackendCustody', reserveBaseDrops: 2_000_000n, openedAtMs: 5 }],
    });
    const { controller } = setup({ vault });
    await controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    expect(controller.getState().vault).toEqual({ vaultId: 3, custodyAddress: 'rBackendCustody', reserveBaseDrops: 2_000_000n });
    expect(controller.getState().reservedDrops).toBe(2_000_000n);
  });

  it('a hint with a tampered custody address never wins over the backend', async () => {
    const storage = memoryStorage();
    storage.setItem('rumi_xrpl_deposit_hint_v1:owner-1', JSON.stringify({
      vaultId: 7, custodyAddress: 'rAttackerAddress', reserveBaseDrops: '1', sendDrops: '12345678',
    }));
    const { controller, wallet, vault } = setup({ storage, vault: makeVault({ pending: [7] }) });
    await controller.prepare(12_345_678n);
    expect(controller.getState().vault?.custodyAddress).toBe(CUSTODY);
    expect(controller.getState().vault?.reserveBaseDrops).toBe(RESERVE);
    controller.propose();
    await controller.pay();
    expect((wallet.signAndSubmit.mock.calls[0] as any[])[0].Destination).toBe(CUSTODY);
    expect(vault.open).not.toHaveBeenCalled();
  });

  it('a stale hint plus an unreadable vault list does not drop the hint or open a vault', async () => {
    const storage = memoryStorage();
    storage.setItem('rumi_xrpl_deposit_hint_v1:owner-1', JSON.stringify({
      vaultId: 9, custodyAddress: CUSTODY, reserveBaseDrops: '1000000', sendDrops: '12345678',
    }));
    const vault = makeVault({ pending: [] });
    vault.ownsVault.mockRejectedValue(new Error('vault list unavailable'));
    const { controller } = setup({ storage, vault });
    await controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    expect(storage.getItem('rumi_xrpl_deposit_hint_v1:owner-1')).toBeTruthy();
    expect(controller.getState()).toMatchObject({ phase: 'error', pendingReadFailed: true });
  });
});

describe('deposit resume amount drift', () => {
  it('persists the amount at pay time and a reload with a re-typed amount keeps the committed one', async () => {
    const storage = memoryStorage();
    const wallet = makeWallet();
    // Reload while the wallet request is still open: signAndSubmit never resolved, but the prompt fired.
    let prompt: ((p: any) => void) | null = null;
    wallet.onPrompt = ((h: any) => { prompt = h; return () => { prompt = null; }; }) as any;
    wallet.signAndSubmit.mockImplementation((async () => {
      prompt?.({ requestId: 'req-open', deepLink: 'x', qrPng: null });
      return new Promise<WalletSubmission>(() => {});
    }) as any);
    const a = setup({ storage, wallet, balance: 'unfunded' });
    await a.controller.prepare(12_345_678n);
    a.controller.propose();
    void a.controller.pay();
    await Promise.resolve();
    const hint = JSON.parse(storage.m.get('rumi_xrpl_deposit_hint_v1:owner-1')!);
    expect(hint).toMatchObject({ sendDrops: '12345678', payStarted: true, requestId: 'req-open' });

    // Reload: the user types a different amount; the committed one is shown and paid.
    const b = setup({ storage, vault: makeVault({ pending: [7] }) });
    await b.controller.prepare(20_000_000n);
    expect(b.controller.getState()).toMatchObject({ phase: 'wallet_pending', sentDrops: 12_345_678n });
    expect(b.controller.getState().requestId).toBe('req-open');
  });

  it('before any request id exists, a re-proposed payment still uses the committed amount', async () => {
    const storage = memoryStorage();
    storage.setItem('rumi_xrpl_deposit_hint_v1:owner-1', JSON.stringify({
      vaultId: 7, custodyAddress: CUSTODY, reserveBaseDrops: '1000000', sendDrops: '12345678', payStarted: true,
    }));
    const { controller, wallet } = setup({ storage, vault: makeVault({ pending: [7] }) });
    await controller.prepare(20_000_000n);
    controller.propose();
    await controller.pay();
    expect((wallet.signAndSubmit.mock.calls[0] as any[])[0].Amount).toBe('12345678');
  });
});

describe('wallet warnings reach the deposit UI', () => {
  it('a payload signed by another account or refused by the ledger is rejected with the warning, never credited', async () => {
    const wallet = makeWallet();
    wallet.signAndSubmit.mockResolvedValue({ requestId: 'req-1', status: 'rejected', signed: false, hash: null, warning: 'The XRP Ledger refused this transaction (tecUNFUNDED_PAYMENT). Nothing was applied.' });
    const { controller, vault } = setup({ wallet });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    expect(controller.getState().phase).toBe('rejected');
    expect(controller.getState().error).toMatch(/tecUNFUNDED_PAYMENT/);
    expect(vault.confirm).not.toHaveBeenCalled();
  });
});

describe('fetchValidatedBalanceDrops', () => {
  it('queries validated ledger, falls back across endpoints, treats actNotFound as 0', async () => {
    const calls: string[] = [];
    const f = vi.fn(async (url: string) => {
      calls.push(url);
      if (calls.length === 1) throw new Error('down');
      return { ok: true, json: async () => ({ result: { status: 'error', error: 'actNotFound' } }) } as Response;
    });
    const v = await fetchValidatedBalanceDrops(CUSTODY, ['https://a', 'https://b'], f as any);
    expect(v).toBe(0n);
    expect(calls).toEqual(['https://a', 'https://b']);
  });

  it('throws when every endpoint fails or returns an unvalidated result', async () => {
    const f = vi.fn(async () => ({ ok: true, json: async () => ({ result: { status: 'success', validated: false, account_data: { Balance: '5' } } }) }) as Response);
    await expect(fetchValidatedBalanceDrops(CUSTODY, ['https://a'], f as any)).rejects.toThrow();
  });
});

describe('unknown pending state (signer wallet, empty cache)', () => {
  it('opens nothing until the user explicitly confirms there is no unfinished deposit', async () => {
    const vault = makeVault();
    vault.readPendingDeposits.mockResolvedValue({ ok: true as const, deposits: [], unknown: true } as any);
    const { controller } = setup({ vault, oisy: true });
    await controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    expect(controller.getState()).toMatchObject({ phase: 'needs_open_confirmation', vault: null });
    expect(controller.getState().error).toMatch(/unfinished XRP deposit/);
    // A repeated prepare() (e.g. a re-render) still opens nothing.
    await controller.prepare(12_345_678n);
    expect(vault.open).not.toHaveBeenCalled();
    // The explicit click opens exactly one vault.
    await controller.acknowledgeNoPendingAndOpen();
    await controller.acknowledgeNoPendingAndOpen();
    expect(vault.open).toHaveBeenCalledTimes(1);
    expect(controller.getState().phase).toBe('awaiting_payment');
  });

  it('a hint, or a backend-confirmed empty list, needs no extra click', async () => {
    const confirmed = makeVault();
    const a = setup({ vault: confirmed });
    await a.controller.prepare(12_345_678n);
    expect(confirmed.open).toHaveBeenCalledTimes(1); // ok:true without unknown

    const pending = makeVault({ pending: [3] });
    pending.readPendingDeposits.mockResolvedValue({
      ok: true as const, unknown: true,
      deposits: [{ vaultId: 3, custodyAddress: CUSTODY, reserveBaseDrops: RESERVE, openedAtMs: 3 }],
    } as any);
    const b = setup({ vault: pending, oisy: true });
    await b.controller.prepare(12_345_678n);
    expect(pending.open).not.toHaveBeenCalled(); // adopts the cached pending vault
    expect(b.controller.getState().vault?.vaultId).toBe(3);
  });

  it('acknowledge is refused outside the confirmation phase', async () => {
    const vault = makeVault();
    const { controller } = setup({ vault });
    await controller.acknowledgeNoPendingAndOpen();
    expect(vault.open).not.toHaveBeenCalled();
  });
});

describe('a wallet failure before any request exists does not pin the amount', () => {
  it('signAndSubmit throwing without a prompt clears payStarted and the committed amount', async () => {
    const storage = memoryStorage();
    const wallet = makeWallet();
    wallet.signAndSubmit.mockRejectedValueOnce(new Error('popup blocked'));
    const { controller } = setup({ storage, wallet, balance: 'unfunded' });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    expect(controller.getState()).toMatchObject({ phase: 'awaiting_payment' });
    expect(controller.getState().error).toMatch(/popup blocked/);
    const hint = JSON.parse(storage.m.get('rumi_xrpl_deposit_hint_v1:owner-1')!);
    expect(hint.payStarted).toBeUndefined();
    // Reload with a re-typed amount: nothing was committed, so the new amount is used.
    const b = setup({ storage, vault: makeVault({ pending: [7] }) });
    await b.controller.prepare(20_000_000n);
    b.controller.propose();
    expect(b.controller.getState().proposal?.sendDrops).toBe(20_000_000n);
  });

  it('a throw AFTER the wallet request was created keeps the commitment', async () => {
    const storage = memoryStorage();
    const wallet = makeWallet();
    let prompt: ((p: any) => void) | null = null;
    wallet.onPrompt = ((h: any) => { prompt = h; return () => { prompt = null; }; }) as any;
    wallet.signAndSubmit.mockImplementation((async () => {
      prompt?.({ requestId: 'req-open', deepLink: 'x', qrPng: null });
      throw new Error('connection dropped');
    }) as any);
    const { controller } = setup({ storage, wallet, balance: 'unfunded' });
    await controller.prepare(12_345_678n);
    controller.propose();
    await controller.pay();
    const hint = JSON.parse(storage.m.get('rumi_xrpl_deposit_hint_v1:owner-1')!);
    expect(hint).toMatchObject({ payStarted: true, requestId: 'req-open', sendDrops: '12345678' });
  });
});
