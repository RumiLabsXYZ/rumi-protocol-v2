import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => {
  function store<T>(v: T) { const listeners = new Set<(v:T)=>void>(); return { subscribe(fn:(v:T)=>void) { listeners.add(fn); fn(v); return () => { listeners.delete(fn); }; }, set(n:T) { v=n; listeners.forEach(fn=>fn(v)); } }; }
  return { wallet: store<any>({ isConnected: false, principal: null }), generation: store(0), auth: store<any>({walletType:'plug'}), collaterals: store<any>({ collaterals: [] }),
    solBalance: vi.fn(), info: vi.fn(), attempt: vi.fn(), address: vi.fn(), quote: vi.fn(), submit: vi.fn(), depositStatus: vi.fn(), withdrawalStatus: vi.fn(), balance: vi.fn(), acknowledge: vi.fn(), oisy: false, saved: null as any,
    icpQuote: vi.fn(), icpRecover: vi.fn(), icpSubmit: vi.fn(), icpResume: vi.fn(), icpLoad: vi.fn(), paidQuote: vi.fn(), submitPaid: vi.fn(), loadPaid: vi.fn(), recoverPaid: vi.fn(), claimRefund: vi.fn(), resumePaid: vi.fn(), continuePaid: vi.fn(), openSigner: vi.fn(), closeSigner: vi.fn() };
});
vi.mock('$lib/services/cksolOisySigner', () => ({ beginCksolOisyAction: (owner:any) => ({ready:fx.openSigner(owner),close:fx.closeSigner}) }));
vi.mock('$lib/services/cksolDepositBalance', () => ({ readSolDepositBalance: fx.solBalance }));
vi.mock('$lib/stores/wallet', () => ({ walletStore: fx.wallet }));
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: fx.generation, auth: fx.auth }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: () => fx.oisy }));
vi.mock('$lib/stores/collateralStore', () => ({ collateralStore: { subscribe: fx.collaterals.subscribe, fetchSupportedCollateral: vi.fn(async () => []) } }));
vi.mock('$lib/services/cksolMinter', () => ({
  isCksolOisyWallet: () => fx.oisy, getCksolMinter: () => ({ get_minter_info: fx.info, deposit_sol_status: fx.depositStatus, withdraw_sol_status: fx.withdrawalStatus }),
  getCksolLedger: () => ({ icrc1_balance_of: fx.balance }),
  getCksolRelay: () => ({ get_cksol_deposit: fx.attempt }),
  clearSolDeposit: vi.fn(async () => {}), getSolDepositAddress: fx.address, quoteSolWithdrawal: fx.quote, submitSolWithdrawal: fx.submit,
  loadSolWithdrawal: () => fx.saved, acknowledgeSolWithdrawal: fx.acknowledge, solWithdrawalKey: (s:string) => `withdrawal:${s}`,
}));
vi.mock('$lib/services/cksolPayment', () => ({
  quoteCksolPayment: fx.paidQuote, submitCksolPayment: fx.submitPaid, loadCksolPayment: fx.loadPaid, saveCksolPayment: vi.fn(),
  recoverCksolPayment: fx.recoverPaid, claimCksolRefund: fx.claimRefund, resumeFundedCksolPayment: fx.resumePaid, continueApprovedCksolPayment: fx.continuePaid, cksolPaymentKey: (s:string)=>`paid:${s}`,
}));
vi.mock('$lib/services/cksolIcpPayment',()=>({
  quoteCksolIcpPayment:fx.icpQuote,recoverCksolIcpPayment:fx.icpRecover,submitCksolIcpPayment:fx.icpSubmit,resumeCksolIcpPayment:fx.icpResume,
  continueCksolIcpApproval:vi.fn(),loadCksolIcpPayment:fx.icpLoad,
  icpPaymentSettled:(p:any)=>!p||p.phase==='NotStarted'||p.phase==='Receipt'&&['Complete','FundingRejected'].includes(p.receipt?.phase)
}));
import Page from '../../routes/solana/+page.svelte';
const ownerA = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'), ownerB = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
let host: HTMLDivElement, instance: any;
// This suite exercises the retained cycles-paid flow; choose it explicitly
// after mounting or a wallet-session reset. ICP-first behavior has its own fixture.
async function settle() { for(let i=0;i<60;i++) { await Promise.resolve(); flushSync(); } button('Use existing cycles')?.click(); flushSync(); }
function button(text:string) { return [...host.querySelectorAll('button')].find(b=>b.textContent?.includes(text) && !(text==='Mint ckSOL' && b.getAttribute('role')==='tab'))!; }
function render() { instance=mount(Page,{target:host}); flushSync(); button('Use existing cycles')?.click(); flushSync(); }
beforeEach(() => {
  Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_k:any,_o:any,fn:any)=>fn({})}});
  vi.resetAllMocks(); fx.openSigner.mockReset(); fx.openSigner.mockResolvedValue(undefined); fx.closeSigner.mockResolvedValue(undefined); fx.acknowledge.mockReset(); fx.oisy=false; fx.saved=null;
  fx.wallet.set({isConnected:false,principal:null}); fx.generation.set(0);
  fx.info.mockResolvedValue({ minimum_deposit_amount:20_000_000n, minimum_withdrawal_amount:2_000_000n, withdrawal_fee:1_000_000n, deposit_sol_required_cycles:1_000_000_000_000n });
  fx.solBalance.mockResolvedValue(1_920_000_000n);
  fx.attempt.mockResolvedValue([]); fx.balance.mockResolvedValue(1_000_000_000n);
  fx.paidQuote.mockResolvedValue({next_sequence:1n,funding_cycles:1_001_000_000_000n,processing_fee:1_000_000_000n,ledger_fee:100_000_000n,approval_amount:1_001_100_000_000n,refund_fee_reserve:100_000_000n,balance:5_000_000_000_000n});
  fx.icpQuote.mockResolvedValue({deposit:26_295_000n,total:26_315_000n,fee:10_000n,balance:100_000_000n,rate:40_000n,terms:{available:true,attached_cycles:1_000_000_000_000n,reserve_available_cycles:2_000_000_000_000n}});
  fx.icpRecover.mockResolvedValue(null);fx.icpLoad.mockReturnValue(null);
  fx.loadPaid.mockReturnValue(null); fx.recoverPaid.mockResolvedValue(null); fx.auth.set({walletType:'plug'});
  fx.address.mockResolvedValue('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax'); fx.depositStatus.mockResolvedValue({Queued:{sweepable_amount:100n}});
  host=document.createElement('div'); document.body.appendChild(host);
});
afterEach(() => { if(instance) unmount(instance); host.remove(); vi.useRealTimers(); });
describe('guided ckSOL mint', () => {
  it('restores the reusable address and detects SOL without an address button', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    expect(fx.address).toHaveBeenCalledWith(ownerA);
    expect(host.textContent).toContain('1.92 SOL received');
    expect(button('Mint ckSOL')).toBeTruthy();
  });
  it('recognizes an old Minted deposit without asking the user to acknowledge it', async () => {
    fx.attempt.mockResolvedValue([{status:{Queued:31n}}]);
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:201n}});
    fx.recoverPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'2',phase:'Receipt',createdAt:1,message:'Cycles approved. SOL processing has not started.',receipt:{phase:'Complete',deposit_id:[31n]}});
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    expect(button('Acknowledge completed deposit')).toBeUndefined();
    expect(host.textContent).not.toContain('processing has not started');
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('Mint complete');
    expect(button('Mint ckSOL')).toBeUndefined();
    button('Start another deposit').click(); await settle();
    expect(button('Mint ckSOL').disabled).toBe(false);
  });
  it('checks receipts automatically after a long absence and stops when unmounted', async () => {
    vi.useFakeTimers(); fx.wallet.set({isConnected:true,principal:ownerA});
    render();await settle(); const initial=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000);await settle();expect(fx.recoverPaid.mock.calls.length).toBeGreaterThan(initial);
    window.dispatchEvent(new Event('focus'));await settle();const current=fx.recoverPaid.mock.calls.length;
    unmount(instance);instance=null;await vi.advanceTimersByTimeAsync(60_000);
    window.dispatchEvent(new Event('focus'));await settle();expect(fx.recoverPaid.mock.calls.length).toBe(current);
  });
  it('keeps a settled payment blocked until the associated mint actually completes', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});fx.attempt.mockResolvedValue([{status:{Queued:31n}}]);
    fx.recoverPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'2',phase:'Receipt',createdAt:1,receipt:{phase:'Complete',deposit_id:[31n]}});
    render();await settle();expect(button('Mint ckSOL').disabled).toBe(true);expect(fx.address).not.toHaveBeenCalled();
  });
  it('allows a funded user to mint when advisory RPC is down and labels the unavailable balance', async () => {
    fx.solBalance.mockRejectedValue(new Error('RPC unavailable'));fx.wallet.set({isConnected:true,principal:ownerA});
    render();await settle();expect(host.textContent).toContain('balance checks are temporarily unavailable');expect(button('Mint ckSOL').disabled).toBe(false);
  });
  it('does not ask for payment when the known address balance is below the deposit minimum', async () => {
    fx.solBalance.mockResolvedValue(890_880n);fx.wallet.set({isConnected:true,principal:ownerA});
    render();await settle();expect(button('Mint ckSOL').disabled).toBe(true);expect(fx.submitPaid).not.toHaveBeenCalled();
  });
});

describe('post-submit mint state', () => {
  it.each(['Queued','Minted'])('does not reuse the pre-sweep balance when a new deposit returns %s', async state => {
    fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_k:any,_o:any,fn:any)=>fn({})}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'3',phase:'Receipt',createdAt:1,receipt:{phase:'Complete',deposit_id:[32n]}});
    fx.depositStatus.mockResolvedValue(state==='Queued'?{Queued:{sweepable_amount:1920000000n}}:{Minted:{minted_amount:1919999999n,block_index:202n}});
    render();await settle();button('Mint ckSOL').click();await settle();
    if(state==='Minted') expect(button('Mint ckSOL')).toBeUndefined();
    else expect(button('Mint ckSOL').disabled).toBe(true);
    expect(host.textContent).not.toContain('1.92 SOL received');
    if(state==='Minted')expect(host.textContent).toContain('1.919999999 ckSOL received');
  });
  it('can return a Funded payment without another approval or mint dispatch', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});
    const funded={owner:ownerA.toText(),sequence:'3',phase:'Receipt',createdAt:1,receipt:{phase:'Funded'}};
    fx.recoverPaid.mockResolvedValue(funded);fx.claimRefund.mockResolvedValue({Ok:{}});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_k:any,_o:any,fn:any)=>fn({})}});
    render();await settle();button('Cancel mint and return funded cycles').click();await settle();
    expect(fx.claimRefund).toHaveBeenCalledOnce();expect(fx.submitPaid).not.toHaveBeenCalled();expect(fx.resumePaid).not.toHaveBeenCalled();
  });
});

describe('ckSOL real page', () => {
  it('shows a wallet connection prompt and never allocates a deposit address signed out', async () => {
    render(); await settle(); expect(host.textContent).toContain('Connect your ICP wallet'); expect(button('Mint ckSOL').disabled).toBe(true); expect(fx.address).not.toHaveBeenCalled();
  });
  it('fails closed when paid quote endpoint is unavailable without preventing public minter terms', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); fx.paidQuote.mockRejectedValue(new Error('method not found'));
    render(); await settle(); expect(button('Mint ckSOL').disabled).toBe(true); expect(host.textContent).toContain('0.02 SOL'); expect(host.textContent).toContain('Do not send SOL');
  });
  it('binds a personal address to the connected principal', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    expect(fx.address).toHaveBeenCalledWith(ownerA); expect(host.textContent).toContain(ownerA.toText());
  });
  it('keeps the address and quote during a same-owner wallet balance refresh', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();

    fx.wallet.set({isConnected:true,principal:ownerA,balance:42}); await settle();
    expect(host.textContent).toContain('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax');
    expect(host.textContent).not.toContain('Paid deposit processing is unavailable');
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });
  it.each(['plug','internet-identity','oisy'])('routes paid processing for %s through its connected owner session', async walletType => {
    fx.oisy=walletType==='oisy'; fx.auth.set({walletType}); fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>fn({})}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'FundingPending'}});
    render(); await settle(); expect(fx.paidQuote).toHaveBeenCalledWith(ownerA,walletType);
     button('Mint ckSOL').click(); await settle();
    expect(fx.submitPaid).toHaveBeenCalledOnce(); expect(fx.submitPaid.mock.calls[0][0]).toEqual(expect.objectContaining({approval_amount:1_001_100_000_000n}));
    expect(fx.submitPaid.mock.calls[0][1]).toBe(ownerA); expect(host.textContent).toContain('FundingPending');
  });
  it('opens the OISY channel during the click before asynchronous locks and payment submission', async () => {
    fx.oisy=true; fx.auth.set({walletType:'oisy'}); fx.wallet.set({isConnected:true,principal:ownerA});
    let inClick=false, finishChannel!:()=>void;
    const enter=()=>{inClick=true;}, leave=()=>{inClick=false;};
    window.addEventListener('click',enter,true); window.addEventListener('click',leave);
    fx.openSigner.mockImplementation(() => {
      if(!inClick) throw new Error('Signer window should not be opened outside of click handler');
      return new Promise<void>(resolve=>{finishChannel=resolve;});
    });
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>{await Promise.resolve();return fn({});}}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'FundingPending'}});
    try {
      render(); await settle();  button('Mint ckSOL').click();
      expect(fx.openSigner).toHaveBeenCalledWith(ownerA);
      await settle(); expect(fx.submitPaid).not.toHaveBeenCalled();
      finishChannel(); await settle(); expect(host.textContent).toContain('FundingPending');
    } finally { window.removeEventListener('click',enter,true); window.removeEventListener('click',leave); }
  });
  it('does not submit a payment when the OISY channel cannot open', async () => {
    fx.oisy=true; fx.auth.set({walletType:'oisy'}); fx.wallet.set({isConnected:true,principal:ownerA});
    fx.openSigner.mockRejectedValueOnce(new Error('Signer window could not be opened'));
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>fn({})}});
    render(); await settle();  button('Mint ckSOL').click(); await settle();
    expect(host.textContent).toContain('Signer window could not be opened'); expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.closeSigner).toHaveBeenCalledOnce();
  });
  it('opens OISY again from the separate processing click without approving cycles again', async () => {
    fx.oisy=true; fx.auth.set({walletType:'oisy'}); fx.wallet.set({isConnected:true,principal:ownerA});
    const approved={owner:ownerA.toText(),sequence:'1',phase:'Approved',createdAt:1,approvalBlock:'7'};
    fx.loadPaid.mockReturnValue(approved); fx.recoverPaid.mockResolvedValue(approved);
    fx.continuePaid.mockResolvedValue({...approved,phase:'Receipt',receipt:{phase:'FundingPending'}});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>{await Promise.resolve();return fn({});}}});
    render(); await settle();
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('Action needed: approval recorded');
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('does not mean a processing payment was collected');
    button('Continue minting').click();
    expect(fx.openSigner).toHaveBeenCalledWith(ownerA);
    await settle(); expect(host.textContent).toContain('FundingPending'); expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.closeSigner).toHaveBeenCalledOnce();
  });
  it('adopts a successful payment receipt deposit id and checks its minter status', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>fn({})}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'DepositPending',deposit_id:[77n]}});
    render(); await settle();  button('Mint ckSOL').click(); await settle();
    expect(fx.depositStatus).toHaveBeenCalledWith(77n); expect(host.textContent).toContain('Deposit ID: 77');
  });
  it('allows a new paid request after the saved receipt settles', async () => {
    const settled={owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'Complete'}};
    fx.loadPaid.mockReturnValue(settled); fx.recoverPaid.mockResolvedValue(settled);
    fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>fn({})}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'2',phase:'Receipt',createdAt:2,receipt:{phase:'FundingPending'}});
    render(); await settle();  button('Mint ckSOL').click(); await settle();
    expect(fx.submitPaid).toHaveBeenCalledOnce();
  });
  it('does not let a stale paid receipt overwrite the next wallet owner', async () => {
    const saved={owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'FundingPending'}};
    fx.loadPaid.mockReturnValue(saved); fx.recoverPaid.mockResolvedValue(saved);
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    let resolve!: (value:any)=>void; fx.recoverPaid.mockImplementationOnce(()=>new Promise(r=>resolve=r));
    button('Check for updates').click(); await settle();
    fx.generation.set(1); fx.wallet.set({isConnected:true,principal:ownerB}); await settle();
    resolve({...saved,sequence:'99'}); await settle();
    expect(host.textContent).not.toContain('Paid payment sequence: 99');
  });
  it('shows a backend Funded receipt and recovery actions without a local journal', async () => {
    const funded={owner:ownerA.toText(),sequence:'12',phase:'Receipt',createdAt:12,receipt:{phase:'Funded',refundable_cycles:55n}};
    fx.wallet.set({isConnected:true,principal:ownerA}); fx.loadPaid.mockReturnValue(null); fx.recoverPaid.mockResolvedValue(funded);
    render(); await settle();
    expect(fx.recoverPaid).toHaveBeenCalled();expect(host.textContent).toContain('Paid payment sequence: 12');
    expect(host.textContent).toContain('without another cycles approval');expect(button('Continue minting')).toBeTruthy();
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('Action needed: processing funds recorded');
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('Select Continue minting below');
    expect(host.querySelector('[aria-label="Mint progress"]')?.textContent).toContain('Status checks will not submit this request automatically');
    expect(host.textContent).not.toContain('Click the mint action below');
  });
  it('allows a known rejected attempt to be reconciled by the next mint action', async () => {
    fx.attempt.mockResolvedValue([{status:{Rejected:'minter rejected'},started_at_ns:1n}]);
    fx.wallet.set({isConnected:true,principal:ownerA});
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_key:any,_opts:any,fn:any)=>fn({})}});
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'1',phase:'Receipt',createdAt:1,receipt:{phase:'FundingPending'}});
    render(); await settle();
    expect(button('Acknowledge rejected legacy attempt')).toBeUndefined();
    expect(button('Mint ckSOL').disabled).toBe(false);
    button('Mint ckSOL').click();await settle();
    expect(fx.submitPaid).toHaveBeenCalledOnce();
  });
  it('discards an old wallet address response after A to B to A transitions', async () => {
    let resolve!: (s:string)=>void; fx.address.mockImplementationOnce(()=>new Promise<string>(r=>resolve=r));
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    fx.generation.set(1); fx.wallet.set({isConnected:true,principal:ownerB}); fx.generation.set(2); fx.wallet.set({isConnected:true,principal:ownerA}); await settle();
    resolve('STALE_ADDRESS'); await settle(); expect(host.textContent).not.toContain('STALE_ADDRESS');
  });
  it('keeps a quarantined deposit from allowing another mint', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); fx.attempt.mockResolvedValue([{status:{Queued:42n}}]); fx.depositStatus.mockResolvedValue({Quarantined:{signature:'s'}});
    render(); await settle(); expect(button('Mint ckSOL').disabled).toBe(true); expect(host.textContent).toContain('quarantined');
  });
  it('blocks OISY redemption before any quote or approval flow', async () => {
    fx.oisy=true; fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle(); button('Redeem SOL').click(); await settle();
    expect(host.textContent).toContain('consent interface required by OISY'); expect(button('Get withdrawal quote').disabled).toBe(true); expect(fx.submit).not.toHaveBeenCalled();
  });
  it('cancels deposit polling on component teardown', async () => {
    vi.useFakeTimers(); fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    const calls=fx.depositStatus.mock.calls.length; unmount(instance); instance=null; await vi.advanceTimersByTimeAsync(60_000);
    expect(fx.depositStatus.mock.calls.length).toBe(calls);
  });
});

describe('ckSOL same-owner operation/session changes', () => {
  it('does not allow a new deposit while the provider is replacing the connected wallet', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA,loading:true}); render(); await settle();
    expect(button('Mint ckSOL').disabled).toBe(true); expect(host.textContent).toContain('Connect your ICP wallet');
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


describe('ICP-first page integration',()=>{
  async function renderIcp() { instance=mount(Page,{target:host});for(let i=0;i<100;i++){await Promise.resolve();flushSync();} }
  it('defaults to ICP and withholds a new SOL address when the ICP balance is insufficient',async()=>{
    fx.wallet.set({isConnected:true,principal:ownerA});fx.solBalance.mockResolvedValue(0n);
    fx.icpQuote.mockResolvedValue({deposit:26_295_000n,total:26_315_000n,fee:10_000n,balance:0n,rate:40_000n,terms:{available:true,attached_cycles:1_000_000_000_000n,reserve_available_cycles:2_000_000_000_000n}});
    await renderIcp();expect(host.textContent).toContain('Add at least 0.26315000 ICP');expect(host.textContent).not.toContain('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax');
    expect(button('Mint with ICP').disabled).toBe(true);expect(fx.icpSubmit).not.toHaveBeenCalled();
  });
  it('shows the existing SOL deposit and offers ICP processing without requiring cycles',async()=>{
    fx.wallet.set({isConnected:true,principal:ownerA});fx.paidQuote.mockResolvedValue({...await fx.paidQuote(),balance:0n});
    await renderIcp();expect(host.textContent).toContain('1.92 SOL received');expect(host.textContent).toContain('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax');expect(button('Mint with ICP').disabled).toBe(false);
  });
  it('does not expose a rent-only deposit address while both processing payment methods are unavailable',async()=>{
    fx.wallet.set({isConnected:true,principal:ownerA});fx.solBalance.mockResolvedValue(890_880n);
    fx.paidQuote.mockResolvedValue({...await fx.paidQuote(),balance:0n});
    fx.icpQuote.mockResolvedValue({...await fx.icpQuote(),balance:0n});
    await renderIcp();
    const address='GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax';
    expect(host.textContent).not.toContain(address);
    expect(host.textContent).toContain('Check processing funds before sending SOL');
    button('Use existing cycles').click();flushSync();
    expect(host.textContent).not.toContain(address);
  });
  it('recognizes an ICP funding reservation without mislabeling it as an unknown minter submission',async()=>{
    fx.wallet.set({isConnected:true,principal:ownerA});fx.attempt.mockResolvedValue([{status:{SubmittedOrUnknown:null}}]);
    fx.icpRecover.mockResolvedValue({owner:ownerA.toText(),sequence:'4',createdAt:1,phase:'Receipt',receipt:{sequence:4n,phase:'Funded',deposit_id:[]}});
    await renderIcp();expect(button('Continue funded mint').disabled).toBe(false);expect(button('Use existing cycles').disabled).toBe(true);
  });
});

describe('numbered mint steps and bounded automatic checks', () => {
  function completedPayment(phase = 'Complete') {
    return {owner:ownerA.toText(),sequence:'3',phase:'Receipt',createdAt:1,receipt:{phase,deposit_id:[32n]}};
  }
  function completedDeposit() {
    fx.attempt.mockResolvedValue([{status:{Queued:32n}}]);
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:1_919_996_000n,block_index:202n}});
    fx.solBalance.mockResolvedValue(890_880n);
    fx.recoverPaid.mockResolvedValue(completedPayment());
    fx.wallet.set({isConnected:true,principal:ownerA});
  }
  it('shows the ordered actions and detected SOL without starting a payment', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    const steps=[...host.querySelectorAll('h3')].map(h=>h.textContent);
    expect(steps).toEqual(expect.arrayContaining(['1. Check processing costs','2. Send SOL on Solana mainnet','3. Mint ckSOL','4. Check your connected ICP wallet']));
    expect(host.textContent).toContain('Detecting SOL does not mint it automatically');
    expect(host.textContent).toContain('Next: use the mint action in step 3');
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });
  it('stops after full completion, permits one manual check, and rearms only an explicit new deposit', async () => {
    vi.useFakeTimers(); completedDeposit(); render(); await settle();
    expect(host.textContent).toContain('Last completed deposit: 1.919996 ckSOL received');
    const initial=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(180_000); await settle();
    window.dispatchEvent(new Event('focus')); await settle();
    expect(fx.recoverPaid).toHaveBeenCalledTimes(initial);
    button('Check again').click(); await settle();
    expect(fx.recoverPaid).toHaveBeenCalledTimes(initial+1);
    await vi.advanceTimersByTimeAsync(180_000); await settle();
    expect(fx.recoverPaid).toHaveBeenCalledTimes(initial+1);
    button('Start another deposit').click(); await settle();
    const newStart=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(fx.recoverPaid).toHaveBeenCalledTimes(newStart+1);
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });
  it('backs off unchanged waiting reads and stops checking a hidden or disconnected account', async () => {
    vi.useFakeTimers(); fx.wallet.set({isConnected:true,principal:ownerA});fx.solBalance.mockResolvedValue(0n);
    render(); await settle(); const first=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000); await settle(); expect(fx.recoverPaid).toHaveBeenCalledTimes(first+1);
    await vi.advanceTimersByTimeAsync(29_999); await settle(); expect(fx.recoverPaid).toHaveBeenCalledTimes(first+1);
    await vi.advanceTimersByTimeAsync(1); await settle(); expect(fx.recoverPaid).toHaveBeenCalledTimes(first+2);
    Object.defineProperty(document,'visibilityState',{configurable:true,value:'hidden'});
    document.dispatchEvent(new Event('visibilitychange')); await settle();
    await vi.advanceTimersByTimeAsync(180_000); await settle(); expect(fx.recoverPaid).toHaveBeenCalledTimes(first+2);
    Object.defineProperty(document,'visibilityState',{configurable:true,value:'visible'});
    fx.wallet.set({isConnected:false,principal:null}); await settle(); const stopped=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(180_000); expect(fx.recoverPaid).toHaveBeenCalledTimes(stopped);
  });
  it('continues checking a pending refund after the SOL mint is complete', async () => {
    vi.useFakeTimers(); completedDeposit(); fx.recoverPaid.mockResolvedValue(completedPayment('RefundReady'));
    render(); await settle(); expect(host.textContent).toContain('processing payment or refund is still pending');
    const initial=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000); await settle(); expect(fx.recoverPaid.mock.calls.length).toBeGreaterThan(initial);
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });
  it('keeps the mint action stable during a background read and rejects its stale result after a click', async () => {
    vi.useFakeTimers();fx.wallet.set({isConnected:true,principal:ownerA}); render(); await settle();
    let release!: (v:any)=>void; fx.info.mockImplementationOnce(()=>new Promise(resolve=>release=resolve));
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(button('Mint ckSOL').disabled).toBe(false); expect(host.textContent).not.toContain('Checking request…');
    const reads=fx.recoverPaid.mock.calls.length;
    fx.submitPaid.mockResolvedValue(completedPayment('FundingPending'));
    button('Mint ckSOL').click(); await settle();
    release({minimum_deposit_amount:20_000_000n}); await settle();
    expect(fx.recoverPaid).toHaveBeenCalledTimes(reads);
    expect(fx.submitPaid).toHaveBeenCalledOnce(); expect(button('Mint ckSOL').disabled).toBe(true);
  });
});

describe('external ICP receipt progress', () => {
  it('resets a backed-off page timer when a different deposit arrives from the payment panel', async () => {
    vi.useFakeTimers(); fx.wallet.set({isConnected:true,principal:ownerA});
    const paid=(id:bigint)=>({owner:ownerA.toText(),sequence:'4',createdAt:1,phase:'Receipt',receipt:{sequence:4n,phase:'Funded',deposit_id:[id]}});
    fx.icpRecover.mockResolvedValue(paid(30n));
    instance=mount(Page,{target:host}); await settle();
    await vi.advanceTimersByTimeAsync(15_000);await settle();
    let resolve!: (value:any)=>void;
    fx.icpRecover.mockImplementationOnce(()=>new Promise(r=>resolve=r));
    await vi.advanceTimersByTimeAsync(30_000);await settle();
    await vi.advanceTimersByTimeAsync(5_000);await settle();
    const reads=fx.recoverPaid.mock.calls.length;
    resolve(paid(31n));await settle();
    await vi.advanceTimersByTimeAsync(14_999);await settle();expect(fx.recoverPaid).toHaveBeenCalledTimes(reads);
    await vi.advanceTimersByTimeAsync(1);await settle();expect(fx.recoverPaid).toHaveBeenCalledTimes(reads+1);
    expect(fx.icpSubmit).not.toHaveBeenCalled();expect(fx.submitPaid).not.toHaveBeenCalled();
  });
});

describe('terminal mint wallet balance', () => {
  it('refreshes ckSOL once after a new verified mint before completion polling stops', async () => {
    vi.useFakeTimers();fx.wallet.set({isConnected:true,principal:ownerA});render();await settle();
    expect(host.textContent).toContain('Connected wallet balance: 1 ckSOL');
    fx.balance.mockResolvedValue(2_919_999_999n);
    fx.submitPaid.mockResolvedValue({owner:ownerA.toText(),sequence:'3',phase:'Receipt',createdAt:1,receipt:{phase:'Complete',deposit_id:[32n]}});
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:1_919_999_999n,block_index:202n}});
    button('Mint ckSOL').click();await settle();
    expect(host.textContent).toContain('Connected wallet balance: 2.919999999 ckSOL');
    const reads=fx.balance.mock.calls.length;await vi.advanceTimersByTimeAsync(180_000);await settle();
    expect(fx.balance).toHaveBeenCalledTimes(reads);expect(fx.submitPaid).toHaveBeenCalledOnce();
  });
});

describe('terminal balance read isolation', () => {
  const result=()=>({owner:ownerA.toText(),sequence:'3',phase:'Receipt',createdAt:1,receipt:{phase:'Complete',deposit_id:[32n]}});
  it('shows an honest balance refresh action after a failed final read without resubmitting the mint', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});render();await settle();
    fx.balance.mockRejectedValueOnce(new Error('ledger unavailable'));
    fx.submitPaid.mockResolvedValue(result());fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    button('Mint ckSOL').click();await settle();
    expect(host.textContent).toContain('Mint verified. Check your wallet or select Check again');
    expect(host.textContent).not.toContain('Connected wallet balance: 1 ckSOL');
    button('Check again').click();await settle();expect(fx.submitPaid).toHaveBeenCalledOnce();
    expect(host.textContent).toContain('Connected wallet balance: 1 ckSOL');
  });
  it('discards the old final balance when the connected session changes before it returns', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});render();await settle();
    let resolve!: (value:bigint)=>void;fx.balance.mockImplementationOnce(()=>new Promise(r=>resolve=r));
    fx.submitPaid.mockResolvedValue(result());fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    button('Mint ckSOL').click();await settle();
    fx.generation.set(1);fx.wallet.set({isConnected:true,principal:ownerB});await settle();
    resolve(99_000_000_000n);await settle();
    expect(host.textContent).not.toContain('Connected wallet balance: 99 ckSOL');
    expect(fx.submitPaid).toHaveBeenCalledOnce();
  });
});

describe('mint completion with unavailable ICP payment status', () => {
  it('does not stop payment checks when no local receipt can establish settlement', async () => {
    vi.useFakeTimers();fx.wallet.set({isConnected:true,principal:ownerA});
    fx.attempt.mockResolvedValue([{status:{Queued:32n}}]);fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});fx.solBalance.mockResolvedValue(890_880n);
    fx.icpRecover.mockRejectedValue(new Error('payment status unavailable'));fx.icpLoad.mockReturnValue(null);fx.recoverPaid.mockResolvedValue(null);
    instance=mount(Page,{target:host});await settle();
    expect(host.textContent).toContain('Processing payment status is still being checked');
    expect(host.textContent).not.toContain('processing payment or refund is still pending');
    const reads=fx.icpRecover.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000);await settle();expect(fx.icpRecover.mock.calls.length).toBeGreaterThan(reads);
    expect(fx.icpSubmit).not.toHaveBeenCalled();
  });
});

describe('prominent mint request status', () => {
  const paid = (sequence: string, phase: string, id?: bigint) => ({
    owner: ownerA.toText(), sequence, phase: 'Receipt', createdAt: Number(sequence),
    receipt: { phase, deposit_id: id === undefined ? [] : [id] },
  });
  const progress = () => host.querySelector('[aria-label="Mint progress"]')!;
  function restore(id = 32n, phase = 'Complete') {
    fx.wallet.set({isConnected:true,principal:ownerA});
    fx.recoverPaid.mockResolvedValue(paid('3', phase, id));
    fx.attempt.mockResolvedValue([{status:{Queued:id}}]);
    fx.solBalance.mockResolvedValue(890_880n);
  }

  it('keeps delayed mint progress visible after payment settles and confirms only Minted', async () => {
    vi.useFakeTimers(); restore();
    render(); await settle();
    expect(progress().textContent).toContain('Mint request submitted');
    expect(host.textContent).not.toContain('Click the mint action below');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(progress().querySelector('.animate-spin')).not.toBeNull();
    expect(progress().textContent).toContain('Last successful mint check');
    expect(progress().textContent).toMatch(/Checking (this request automatically|the saved request now)/);
    fx.depositStatus.mockResolvedValue({Finalized:{signature:'synthetic-finalized'}});
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(progress().textContent).toContain('ckSOL mint pending');
    expect(progress().textContent).not.toContain('Mint complete');
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:1_919_996_001n,block_index:203n}});
    button('Check again').click(); await settle();
    expect(progress().textContent).toContain('Mint complete');
    expect(progress().textContent).toContain('1.919996001 ckSOL minted');
    expect(progress().textContent).toContain(ownerA.toText());
    expect(progress().textContent).toContain('203');
    const receiptLink=progress().querySelector('a[href="#cksol-mint-receipt"]') as HTMLAnchorElement;
    receiptLink.click(); await settle();
    expect((host.querySelector('#cksol-mint-receipt') as HTMLDetailsElement).open).toBe(true);
    expect(button('Mint ckSOL')).toBeUndefined();
    expect(host.textContent).not.toContain('mint action is waiting for a new SOL deposit');
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });

  it('does not use a previous Minted receipt for a new request without a deposit ID', async () => {
    restore(); fx.solBalance.mockResolvedValue(1_920_000_000n);
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    render(); await settle();
    expect(progress().textContent).toContain('Mint complete');
    button('Start another deposit').click(); await settle();
    let resolve!: (value: any) => void;
    fx.submitPaid.mockImplementationOnce(() => new Promise(r => resolve=r));
    button('Mint ckSOL').click(); await settle();
    expect(progress().textContent).toContain('Mint action in progress');
    expect(progress().textContent).not.toContain('Awaiting wallet confirmation');
    resolve(paid('4','FundingPending')); await settle();
    expect(progress().textContent).toContain('Processing request');
    expect(progress().textContent).not.toContain('Mint complete');
    fx.recoverPaid.mockResolvedValue(paid('4','Complete'));
    button('Check again').click(); await settle();
    expect(progress().textContent).toContain('no matching ckSOL mint has been confirmed');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(button('Mint ckSOL').disabled).toBe(true);
    expect(fx.submitPaid).toHaveBeenCalledOnce();
  });

  it('keeps an unmatched settled request checking after restoration instead of reusing old success', async () => {
    vi.useFakeTimers(); restore(); fx.solBalance.mockResolvedValue(1_920_000_000n);
    fx.recoverPaid.mockResolvedValue(paid('4','Complete'));
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    render(); await settle();
    expect(progress().textContent).toContain('no matching ckSOL mint has been confirmed');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(progress().querySelector('.animate-spin')).not.toBeNull();
    expect(button('Mint ckSOL').disabled).toBe(true);
    const checks=fx.recoverPaid.mock.calls.length;
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(fx.recoverPaid.mock.calls.length).toBeGreaterThan(checks);
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });

  it('keeps the last successful check separate from a failed read and resumes by checking', async () => {
    vi.useFakeTimers(); restore(); render(); await settle();
    const lastChecked = [...progress().querySelectorAll('p')].find(p => p.textContent?.includes('Last successful mint check'))!.textContent;
    fx.depositStatus.mockRejectedValueOnce(new Error('synthetic network unavailable'));
    button('Check again').click(); await settle();
    expect(progress().textContent).toContain('Checking mint status');
    expect(progress().textContent).toContain(lastChecked!);
    expect(progress().textContent).toContain('latest check failed');
    fx.depositStatus.mockResolvedValue({Swept:{signature:'synthetic-sweep'}});
    await vi.advanceTimersByTimeAsync(15_000); await settle();
    expect(progress().textContent).toContain('Processing SOL deposit');
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });

  it('restores the current owner/request without importing another owner’s completed mint', async () => {
    restore(); fx.depositStatus.mockResolvedValue({Minted:{minted_amount:1_919_996_001n,block_index:202n}});
    render(); await settle(); expect(progress().textContent).toContain('Mint complete');
    fx.recoverPaid.mockResolvedValue({...paid('7','Starting'),owner:ownerB.toText(),phase:'Starting',receipt:undefined});
    fx.attempt.mockResolvedValue([]);
    fx.generation.set(1); fx.wallet.set({isConnected:true,principal:ownerB}); await settle();
    expect(progress().getAttribute('data-request')).toContain(ownerB.toText());
    expect(progress().getAttribute('data-request')).toContain('|cycles|7');
    expect(progress().textContent).toContain('Confirming submission');
    expect(progress().textContent).toContain('Checking whether the saved submission was accepted');
    expect(progress().textContent).not.toContain('The request is being submitted');
    expect(progress().textContent).not.toContain('1.919996001');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });

  it('keeps confirmed mint success dominant while a matching refund remains pending', async () => {
    restore(32n,'RefundReady');
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:1_919_996_001n,block_index:202n}});
    render(); await settle();
    expect(progress().textContent).toContain('Mint complete');
    expect(progress().textContent).toContain('processing payment or refund is still pending');
    expect(button('Start another deposit').disabled).toBe(true);
    expect(button('Return unused cycles')).toBeTruthy();
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });

  it('does not let an older Minted ID override an unknown legacy submission', async () => {
    restore();
    fx.attempt.mockResolvedValue([{status:{SubmittedOrUnknown:null}}]);
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    render(); await settle();
    expect(progress().textContent).toContain('Checking mint status');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(host.textContent).toContain('This is the previous receipt, not confirmation of a new request');
    expect(button('Mint ckSOL').disabled).toBe(true);
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });

  it('shows the explicit continuation for a funded ICP reservation without calling it an unknown mint', async () => {
    fx.wallet.set({isConnected:true,principal:ownerA});
    fx.attempt.mockResolvedValue([{status:{SubmittedOrUnknown:null}}]);
    fx.icpRecover.mockResolvedValue(paid('8','Funded'));
    instance=mount(Page,{target:host}); await settle();
    expect(progress().textContent).toContain('Action needed: processing funds recorded');
    expect(progress().textContent).toContain('Select Continue funded mint below');
    expect(progress().textContent).toContain('Status checks will not submit this request automatically');
    expect(progress().textContent).not.toContain('latest mint result could not be confirmed');
    expect(button('Continue funded mint').disabled).toBe(false);
    expect(host.textContent).not.toContain('Click the mint action below');
    expect(fx.icpSubmit).not.toHaveBeenCalled(); expect(fx.icpResume).not.toHaveBeenCalled();
  });

  it.each(['Approved','Funded'])('keeps the existing cycles %s continuation enabled beside a historical Minted ID', async phase => {
    restore();
    const current=phase==='Approved'
      ? {...paid('4',phase),phase:'Approved',receipt:undefined,approvalBlock:'7'}
      : paid('4',phase);
    fx.recoverPaid.mockResolvedValue(current);
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    render(); await settle();
    expect(progress().textContent).toContain('Action needed:');
    expect(progress().textContent).toContain('Select Continue minting below');
    expect(progress().textContent).not.toContain('Mint complete');
    expect(button('Continue minting').disabled).toBe(false);
    expect(fx.submitPaid).not.toHaveBeenCalled(); expect(fx.continuePaid).not.toHaveBeenCalled();
    expect(fx.resumePaid).not.toHaveBeenCalled(); expect(fx.icpSubmit).not.toHaveBeenCalled();
  });

  it.each(['Approved','Funded'])('keeps the existing ICP %s continuation enabled beside a historical Minted ID', async phase => {
    fx.wallet.set({isConnected:true,principal:ownerA});
    fx.attempt.mockResolvedValue([{status:{Queued:32n}}]);
    fx.icpRecover.mockResolvedValue(phase==='Approved'
      ? {...paid('4',phase),phase:'Approved',receipt:undefined,approvalBlock:'7'}
      : paid('4',phase));
    fx.depositStatus.mockResolvedValue({Minted:{minted_amount:99n,block_index:202n}});
    instance=mount(Page,{target:host}); await settle();
    const label=phase==='Approved'?'Continue approved mint':'Continue funded mint';
    expect(progress().textContent).toContain('Action needed:');
    expect(progress().textContent).toContain(`Select ${label} below`);
    expect(progress().textContent).not.toContain('Mint complete');
    expect(button(label).disabled).toBe(false);
    expect(fx.icpSubmit).not.toHaveBeenCalled(); expect(fx.icpResume).not.toHaveBeenCalled();
    expect(fx.submitPaid).not.toHaveBeenCalled();
  });

  it('retains the minter-pending hold even when the payment is Funded', async () => {
    restore(32n,'Funded'); render(); await settle();
    expect(progress().textContent).toContain('Mint request submitted');
    expect(progress().textContent).not.toContain('Action needed:');
    expect(button('Continue minting').disabled).toBe(true);
    expect(fx.resumePaid).not.toHaveBeenCalled(); expect(fx.submitPaid).not.toHaveBeenCalled();
  });
});
