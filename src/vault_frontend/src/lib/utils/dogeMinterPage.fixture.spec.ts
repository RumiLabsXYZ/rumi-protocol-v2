import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import { Principal } from '@dfinity/principal';
import { dogeWithdrawalKey, readDogeWithdrawal, type DogeWithdrawal } from './dogeMinterRecovery';

const fx = vi.hoisted(() => {
  function store(value: any) {
    const subs = new Set<(v: any) => void>();
    return { subscribe(fn: (v: any) => void) { subs.add(fn); fn(value); return () => subs.delete(fn); }, set(next: any) { value = next; subs.forEach(fn => fn(value)); } };
  }
  return { walletGeneration: store(0), connected: store(false), principal: store(null), address: vi.fn(), update: vi.fn(), status: vi.fn(), redeem: vi.fn(), begin: vi.fn(), oisy: vi.fn(() => false) };
});
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: fx.walletGeneration }));
vi.mock('$lib/stores/wallet', () => ({ isConnected: fx.connected, principal: fx.principal }));
vi.mock('$lib/config', () => ({ CANISTER_IDS: { CKDOGE_MINTER: 'eqltq-xqaaa-aaaar-qb3vq-cai', CKDOGE_LEDGER: 'ledger' } }));
vi.mock('$lib/services/ckdogeMinterActors', () => ({
  getPublicMinterActor: async () => ({ get_doge_address: fx.address, get_minter_info: async () => ({}), estimate_withdrawal_fee: async () => ({ Ok: { dogecoin_fee: 10n, minter_fee: 5n } }), retrieve_doge_status: fx.status }),
  updateDogeBalanceForOwner: fx.update, redeemDogeWithApproval: fx.redeem,
  DogeWithdrawalPreflightError: class extends Error {},
}));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFee: async () => 10n, getCachedLedgerFee: () => 10n }));
vi.mock('$lib/services/minterOisySigner', () => ({ beginMinterOisyAction: fx.begin }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: fx.oisy }));
import Page from '../../routes/doge/+page.svelte';
const A = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const B = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const ADDRESS = 'DBXu2kgc3xtvCUWFcxFE3r9hEYgmuaaCyD';
let component: ReturnType<typeof mount> | null = null;
let target: HTMLDivElement;
async function settle() { for (let i = 0; i < 20; i++) { await Promise.resolve(); flushSync(); } }
function connect(principal = A) { fx.connected.set(true); fx.principal.set(principal); }
async function render() { component = mount(Page, { target }); await settle(); }
function click(text: string) {
  const button = Array.from(target.querySelectorAll('button')).find(b => b.textContent?.trim() === text);
  expect(button, text).toBeTruthy(); button!.click();
}
function input(id: string, value: string) { const el = target.querySelector(id) as HTMLInputElement; el.value = value; el.dispatchEvent(new Event('input', { bubbles: true })); }
function saved(overrides: Partial<DogeWithdrawal> = {}): DogeWithdrawal {
  const receipt: DogeWithdrawal = { version: 1, id: 'saved', owner: A.toText(), minter: 'eqltq-xqaaa-aaaar-qb3vq-cai', address: ADDRESS, amount: '5000000000', createdAt: 1, phase: 'retrieving', ...overrides };
  localStorage.setItem(dogeWithdrawalKey(A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai'), JSON.stringify(receipt)); return receipt;
}
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks(); localStorage.clear(); fx.walletGeneration.set(0); fx.connected.set(false); fx.principal.set(null);
  const tails = new Map<string, Promise<any>>();
  Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: vi.fn((name: string, _opts: any, run: () => Promise<any>) => {
    const next = (tails.get(name) ?? Promise.resolve()).then(run); tails.set(name, next.catch(() => {})); return next;
  }) } });
  fx.address.mockResolvedValue(ADDRESS); fx.update.mockResolvedValue({ Err: { NoNewUtxos: { current_confirmations: 0, required_confirmations: 60, pending_utxos: [] } } });
  fx.status.mockResolvedValue({ Pending: null }); fx.oisy.mockReturnValue(false);
  target = document.createElement('div'); document.body.append(target);
});
afterEach(async () => { if (component) await unmount(component); component = null; target.remove(); vi.useRealTimers(); });

describe('real DOGE minter page', () => {
  it('restores the address and automatically checks deposits on connect and reload without wallet actions', async () => {
    connect(); await render();
    expect(fx.address).toHaveBeenCalledWith({ owner: [A], subaccount: [] });
    expect(fx.update).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain('automatically detect confirmed deposits');
    expect(target.textContent).not.toContain('I sent the DOGE');
    expect(target.textContent).toContain('Watching for your DOGE deposit');
    expect(target.textContent).not.toMatch(/attempt \d+ of \d+/i);
    expect(document.title).toBe('ckDOGE Minter | Rumi');
    await unmount(component!); component = null; await render();
    expect(fx.update).toHaveBeenCalledTimes(2); expect(fx.redeem).not.toHaveBeenCalled(); expect(fx.begin).not.toHaveBeenCalled();
  });
  it('restores an unknown attempt and never clears it or dispatches from focus or timers', async () => {
    const receipt = saved(); connect(); await render(); click('Redeem DOGE'); await settle();
    expect(target.textContent).toContain('needs reconciliation');
    window.dispatchEvent(new Event('focus')); await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')).toEqual(receipt);
    expect(fx.redeem).not.toHaveBeenCalled(); expect(fx.begin).not.toHaveBeenCalled();
  });
  it('automatically follows an accepted burn through Submitted to Confirmed and keeps its receipt', async () => {
    saved({ phase: 'accepted', burnBlock: '42', approveBlock: '40' });
    fx.status.mockResolvedValueOnce({ Submitted: { txid: [1, 2] } }).mockResolvedValue({ Confirmed: { txid: [1, 2] } });
    connect(); await render(); click('Redeem DOGE'); await settle();
    expect(fx.status).toHaveBeenCalledWith({ block_index: 42n });
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.phase).toBe('terminal');
    expect(target.textContent).toContain('DOGE has landed');
    const calls = fx.status.mock.calls.length; await vi.advanceTimersByTimeAsync(45_000); expect(fx.status).toHaveBeenCalledTimes(calls);
    expect(fx.redeem).not.toHaveBeenCalled();
  });
  it('holds authoritative Unknown and WillReimburse instead of enabling another withdrawal', async () => {
    saved({ phase: 'accepted', burnBlock: '42' }); fx.status.mockResolvedValue({ Unknown: null });
    connect(); await render(); click('Redeem DOGE'); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.phase).toBe('accepted');
    fx.status.mockResolvedValue({ WillReimburse: null }); await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.phase).toBe('accepted');
    expect(target.textContent).toContain('Withdrawal in progress'); expect(fx.redeem).not.toHaveBeenCalled();
  });

  it('saves uncertain lineage before dispatch, opens OISY in the click, and holds a lost reply across reload', async () => {
    const close = vi.fn(async () => {});
    fx.oisy.mockReturnValue(true); fx.begin.mockReturnValue({ ready: Promise.resolve(), close });
    fx.redeem.mockImplementation(async () => {
      expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.phase).toBe('approving');
      throw new Error('reply lost');
    });
    connect(); await render(); click('Redeem DOGE'); await settle();
    input('#doge-redeem-address', ADDRESS); input('#doge-redeem-amount', '50'); await settle();
    await vi.advanceTimersByTimeAsync(300); await settle();
    click('Send it back to Dogecoin'); expect(fx.begin).toHaveBeenCalledTimes(1); await settle();
    expect(fx.redeem).toHaveBeenCalledTimes(1); expect(close).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain('needs reconciliation');
    await unmount(component!); component = null; await render(); click('Redeem DOGE'); await settle();
    expect(target.textContent).toContain('needs reconciliation'); expect(fx.redeem).toHaveBeenCalledTimes(1);
  });
  it('invalidates an old RPC on same-owner wallet-session replacement', async () => {
    let resolve!: (v: string) => void;
    fx.address.mockImplementationOnce(() => new Promise<string>(r => { resolve = r; }));
    connect(); await render(); fx.walletGeneration.set(1); await settle();
    resolve('DSTALESESSION'); await settle();
    expect(target.textContent).not.toContain('DSTALESESSION'); expect(target.textContent).toContain(ADDRESS);
  });


  it('resumes accepted reimbursement to its authoritative terminal receipt', async () => {
    saved({ phase: 'accepted', burnBlock: '42' });
    fx.status.mockResolvedValue({ Reimbursed: { account: { owner: A, subaccount: [] }, mint_block_index: 50n, amount: 5000000000n, reason: { CallFailed: null } } });
    connect(); await render(); click('Redeem DOGE'); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.phase).toBe('terminal');
    expect(target.textContent).toContain('reimbursed');
  });
  it('keeps checking a reusable address after a completed mint', async () => {
    fx.update.mockResolvedValueOnce({ Ok: [{ Minted: { minted_amount: 5000000000n, block_index: 8n } }] });
    connect(); await render(); expect(target.textContent).toContain('Minted 50 DOGE');
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(fx.update).toHaveBeenCalledTimes(2); expect(target.textContent).toContain('Minted 50 DOGE');
  });
  it('does not overlap deposit checks while a request is in flight', async () => {
    let resolve!: (v: any) => void;
    fx.update.mockImplementationOnce(() => new Promise(r => { resolve = r; }));
    connect(); await render(); window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(120_000); await settle(); expect(fx.update).toHaveBeenCalledTimes(1);
    resolve({ Ok: [] }); await settle(); await vi.advanceTimersByTimeAsync(60_000); await settle(); expect(fx.update).toHaveBeenCalledTimes(2);
  });


  it('saves an accepted result for the original owner after an A -> B -> A session transition', async () => {
    let finish!: (value: any) => void;
    fx.redeem.mockImplementation(() => new Promise(r => { finish = r; }));
    connect(); await render(); click('Redeem DOGE'); await settle();
    input('#doge-redeem-address', ADDRESS); input('#doge-redeem-amount', '50'); await settle();
    await vi.advanceTimersByTimeAsync(300); await settle(); click('Send it back to Dogecoin'); await settle();
    expect(fx.redeem).toHaveBeenCalledTimes(1);
    fx.walletGeneration.set(1); fx.principal.set(B); fx.principal.set(A); await settle();
    finish({ kind: 'success', approveBlockIndex: 40n, burnBlockIndex: 42n }); await settle();
    expect(readDogeWithdrawal(localStorage, A.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')?.burnBlock).toBe('42');
    expect(readDogeWithdrawal(localStorage, B.toText(), 'eqltq-xqaaa-aaaar-qb3vq-cai')).toBeNull();
    expect(fx.redeem).toHaveBeenCalledTimes(1);
  });

  it('rejects stale address results across A -> B -> A', async () => {
    let resolve!: (v: string) => void;
    fx.address.mockImplementationOnce(() => new Promise<string>(r => { resolve = r; }));
    connect(); await render(); fx.principal.set(B); await settle(); fx.principal.set(A); await settle();
    resolve('DSTALEADDRESS'); await settle();
    expect(target.textContent).not.toContain('DSTALEADDRESS'); expect(target.textContent).toContain(ADDRESS);
  });
});
