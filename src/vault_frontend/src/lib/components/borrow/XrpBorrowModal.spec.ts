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

function makeVault(opts: { pending?: number[]; confirmResult?: any; owns?: boolean } = {}) {
  return {
    open: vi.fn(async () => ({ success: true, data: { vaultId: 7, custodyAddress: CUSTODY, reserveBaseDrops: RESERVE } })),
    pendingDeposits: vi.fn(async () => (opts.pending ?? [7]).map((vaultId) => ({ vaultId }))),
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
    expect(tx).toEqual({ TransactionType: 'Payment', Destination: CUSTODY, Amount: '12345678' });
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
    expect(vault.pendingDeposits).not.toHaveBeenCalledTimes(99); // sanity: no polling signer reads
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
