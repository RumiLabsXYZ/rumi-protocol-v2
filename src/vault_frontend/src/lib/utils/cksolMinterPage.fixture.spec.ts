import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => {
  function store<T>(v: T) { const listeners = new Set<(v:T)=>void>(); return { subscribe(fn:(v:T)=>void) { listeners.add(fn); fn(v); return () => { listeners.delete(fn); }; }, set(n:T) { v=n; listeners.forEach(fn=>fn(v)); } }; }
  return { wallet: store<any>({ isConnected: false, principal: null }), generation: store(0), collaterals: store<any>({ collaterals: [] }),
    info: vi.fn(), budget: vi.fn(), attempt: vi.fn(), address: vi.fn(), start: vi.fn(), quote: vi.fn(), submit: vi.fn(), depositStatus: vi.fn(), withdrawalStatus: vi.fn(), balance: vi.fn(), acknowledge: vi.fn(), oisy: false, saved: null as any };
});
vi.mock('$lib/stores/wallet', () => ({ walletStore: fx.wallet }));
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: fx.generation }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: () => fx.oisy }));
vi.mock('$lib/stores/collateralStore', () => ({ collateralStore: { subscribe: fx.collaterals.subscribe, fetchSupportedCollateral: vi.fn(async () => []) } }));
vi.mock('$lib/services/cksolMinter', () => ({
  isCksolOisyWallet: () => fx.oisy, getCksolMinter: () => ({ get_minter_info: fx.info, deposit_sol_status: fx.depositStatus, withdraw_sol_status: fx.withdrawalStatus }),
  getCksolLedger: () => ({ icrc1_balance_of: fx.balance }),
  getCksolRelay: () => ({ get_cksol_relay_budget: fx.budget, get_cksol_deposit: fx.attempt }),
  clearSolDeposit: vi.fn(async () => {}), getSolDepositAddress: fx.address, startSolDeposit: fx.start, quoteSolWithdrawal: fx.quote, submitSolWithdrawal: fx.submit,
  loadSolWithdrawal: () => fx.saved, acknowledgeSolWithdrawal: fx.acknowledge, solWithdrawalKey: (s:string) => `withdrawal:${s}`, CKSOL_RELAY_CYCLES: 1_000_000_000_000n,
}));
import Page from '../../routes/solana/+page.svelte';
const ownerA = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'), ownerB = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
let host: HTMLDivElement, instance: any;
async function settle() { for(let i=0;i<20;i++) { await Promise.resolve(); flushSync(); } }
function button(text:string) { return [...host.querySelectorAll('button')].find(b=>b.textContent?.includes(text))!; }
function render() { instance=mount(Page,{target:host}); flushSync(); }
beforeEach(() => {
  vi.clearAllMocks(); fx.acknowledge.mockReset(); fx.oisy=false; fx.saved=null;
  fx.wallet.set({isConnected:false,principal:null}); fx.generation.set(0);
  fx.info.mockResolvedValue({ minimum_deposit_amount:20_000_000n, minimum_withdrawal_amount:2_000_000n, withdrawal_fee:1_000_000n, deposit_sol_required_cycles:1_000_000_000_000n });
  fx.budget.mockResolvedValue(3_000_000_000_000n); fx.attempt.mockResolvedValue([]); fx.balance.mockResolvedValue(1_000_000_000n);
  fx.address.mockResolvedValue('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax'); fx.start.mockResolvedValue(42n); fx.depositStatus.mockResolvedValue({Queued:{sweepable_amount:100n}});
  host=document.createElement('div'); document.body.appendChild(host);
});
afterEach(() => { if(instance) unmount(instance); host.remove(); vi.useRealTimers(); });
describe('ckSOL real page', () => {
  it('shows a wallet connection prompt and never allocates a deposit address signed out', async () => {
    render(); await settle(); expect(host.textContent).toContain('Connect your ICP wallet'); expect(button('Get my SOL').disabled).toBe(true); expect(fx.address).not.toHaveBeenCalled();
  });
  it('fails closed when relay is unavailable without preventing public minter terms', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); fx.budget.mockRejectedValue(new Error('method not found'));
    render(); await settle(); expect(button('Get my SOL').disabled).toBe(true); expect(host.textContent).toContain('0.02 SOL'); expect(host.textContent).toContain('Do not send new SOL');
  });
  it('binds a personal address to the connected principal', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Get my SOL').click(); await settle();
    expect(fx.address).toHaveBeenCalledWith(ownerA); expect(host.textContent).toContain(ownerA.toText());
  });
  it('discards an old wallet address response after A to B to A transitions', async () => {
    let resolve!: (s:string)=>void; fx.address.mockImplementation(()=>new Promise<string>(r=>resolve=r));
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Get my SOL').click(); await settle();
    fx.generation.set(1); fx.wallet.set({isConnected:true,principal:ownerB}); fx.generation.set(2); fx.wallet.set({isConnected:true,principal:ownerA}); await settle();
    resolve('STALE_ADDRESS'); await settle(); expect(host.textContent).not.toContain('STALE_ADDRESS');
  });
  it('keeps a quarantined deposit from exposing a new address', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); fx.attempt.mockResolvedValue([{status:{Queued:42n}}]); fx.depositStatus.mockResolvedValue({Quarantined:{signature:'s'}});
    render(); await settle(); expect(button('Get my SOL').disabled).toBe(true); expect(host.textContent).toContain('quarantined');
  });
  it('blocks OISY redemption before any quote or approval flow', async () => {
    fx.oisy=true; fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Redeem SOL').click(); await settle();
    expect(host.textContent).toContain('consent interface required by OISY'); expect(button('Get withdrawal quote').disabled).toBe(true); expect(fx.submit).not.toHaveBeenCalled();
  });
  it('cancels deposit polling on component teardown', async () => {
    vi.useFakeTimers(); fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Get my SOL').click(); await settle(); button('Process my deposit').click(); await settle();
    const calls=fx.depositStatus.mock.calls.length; unmount(instance); instance=null; await vi.advanceTimersByTimeAsync(60_000);
    expect(fx.depositStatus.mock.calls.length).toBe(calls);
  });
});

describe('ckSOL same-owner operation/session changes', () => {
  it('does not allow a new deposit while the provider is replacing the connected wallet', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA,loading:true}); render(); await settle();
    expect(button('Get my SOL').disabled).toBe(true); expect(host.textContent).toContain('Connect your ICP wallet');
    expect(fx.address).not.toHaveBeenCalled();
  });
  it('ignores an old burn status after the current saved withdrawal changes', async () => {
    fx.saved={id:'old',owner:ownerA.toText(),amount:'100000000',address:'GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax',phase:'Submitted',createdAt:1,burnBlock:'8'};
    fx.withdrawalStatus.mockResolvedValue({Pending:null}); fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Redeem SOL').click(); await settle();
    let resolve!: (s:any)=>void; fx.withdrawalStatus.mockImplementationOnce(()=>new Promise(r=>resolve=r));
    button('Check withdrawal status').click(); await settle();
    fx.saved={...fx.saved,id:'new',burnBlock:'9'};
    button('Refresh balance').click(); await settle();
    resolve({TxFinalized:{Success:{transaction_id:'OLD_SIGNATURE'}}}); await settle();
    expect(host.textContent).not.toContain('finalized successfully');
    expect(host.textContent).not.toContain('Acknowledge completed withdrawal');
    expect(host.textContent).toContain('Burn block: 9');
  });
});

describe('ckSOL failed acknowledgement recovery', () => {
  it('clears the old success when a stale tab reloads the newer saved withdrawal', async () => {
    fx.saved={id:'old',owner:ownerA.toText(),amount:'100000000',address:'GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax',phase:'Submitted',createdAt:1,burnBlock:'8'};
    fx.withdrawalStatus.mockResolvedValue({TxFinalized:{Success:{transaction_id:'old-signature'}}});
    fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_options:any,fn:any)=>fn({})}});
    render(); await settle(); button('Redeem SOL').click(); await settle();
    expect(button('Acknowledge completed')).toBeTruthy();
    fx.saved={...fx.saved,id:'new',burnBlock:'9'};
    fx.acknowledge.mockImplementation(()=>{throw new Error('saved withdrawal changed');});
    button('Acknowledge completed').click(); await settle();
    expect(host.textContent).toContain('Burn block: 9');
    expect(host.textContent).not.toContain('finalized successfully');
    expect(button('Acknowledge completed')).toBeUndefined();
  });
});
