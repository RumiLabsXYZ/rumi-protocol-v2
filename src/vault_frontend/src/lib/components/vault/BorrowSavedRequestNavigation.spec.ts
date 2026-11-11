import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';
vi.hoisted(() => { Object.defineProperty(window, 'matchMedia', {configurable:true, value:()=>({matches:false, addEventListener:()=>{}, removeEventListener:()=>{}})}); });
const fx = vi.hoisted(() => {
  function store(value: any) { const listeners = new Set<(v:any)=>void>(); return { subscribe(fn:(v:any)=>void) {listeners.add(fn); fn(value); return () => listeners.delete(fn);}, set(next:any) {value=next; listeners.forEach(fn=>fn(value));} }; }
  const wallet=store({}), collateral=store({}), app=store({});
  const derived=(s:any, f:(v:any)=>any)=>({subscribe(fn:(v:any)=>void){return s.subscribe((v:any)=>fn(f(v)));}});
  return {store, wallet, collateral, app, derived, open:vi.fn(), inspect:vi.fn(), resume:vi.fn(), cancel:vi.fn(), fetch:vi.fn().mockResolvedValue(undefined), refresh:vi.fn().mockResolvedValue(undefined)};
});
vi.mock('$lib/stores/wallet',()=>({walletStore:{subscribe:fx.wallet.subscribe, refreshBalance:fx.refresh},isConnected:fx.derived(fx.wallet,v=>v.isConnected),principal:fx.derived(fx.wallet,v=>v.principal)}));
vi.mock('$lib/stores/collateralStore',()=>({collateralStore:{subscribe:fx.collateral.subscribe, fetchSupportedCollateral:async()=>[], getCollateralInfo:()=>({symbol:'ICP',decimals:8})},activeCollateralTypes:fx.derived(fx.collateral,v=>v.collaterals)}));
vi.mock('$lib/stores/appDataStore',()=>({appDataStore:{subscribe:fx.app.subscribe, fetchProtocolStatus:fx.fetch, refreshAll:fx.refresh},protocolStatus:fx.derived(fx.app,v=>v.protocolStatus),isLoadingProtocol:fx.store(false)}));
vi.mock('$lib/stores/developer',()=>({developerAccess:fx.store(true)}));
vi.mock('$lib/stores/seasonStore',()=>({seasonStore:{ensureLoaded:vi.fn()},earningActive:fx.store(false)}));
vi.mock('$lib/services/protocol',()=>({protocolService:{openVaultAndBorrow:fx.open}}));
vi.mock('$lib/services/protocol/apiClient',()=>({ApiClient:{getVaultPullRecovery:fx.inspect,resumeSavedVaultCollateralPull:fx.resume,cancelSavedVaultCollateralPull:fx.cancel}}));
vi.mock('$lib/services/auth',()=>({currentWalletType:fx.store('internet-identity'),walletSessionGeneration:fx.store(0),WALLET_TYPES:{OISY:'oisy'}}));
vi.mock('$lib/services/oisySigner',()=>({openOisySignerChannel:vi.fn()}));
vi.mock('$lib/stores/vaultStore',()=>({vaultStore:Object.assign(fx.store({vaults:[]}),{refreshVaults:fx.refresh})}));
vi.mock('$lib/stores/toast',()=>({toastStore:{success:vi.fn(),error:vi.fn()}}));
vi.mock('$lib/config',()=>({CANISTER_IDS:{ICP_LEDGER:'ryjl3-tyaaa-aaaaa-aaaba-cai'}}));
vi.mock('$lib/protocol',()=>({MINIMUM_CR:1.5,LIQUIDATION_CR:1.33}));
vi.mock('$lib/utils/nativeXrpBorrowFlow',()=>({isNativeXrpCollateral:()=>false,xrpCreditedCollateral:()=>0,XRPL_BASE_RESERVE_XRP:1}));
vi.mock('$lib/components/dashboard/ProtocolStats.svelte',async()=>({default:(await import('../../../tests/vault-recovery-fixtures/Quiet.svelte')).default}));
vi.mock('$lib/components/points/MultiplierBadge.svelte',async()=>({default:(await import('../../../tests/vault-recovery-fixtures/Quiet.svelte')).default}));
vi.mock('$lib/components/borrow/XrpBorrowModal.svelte',async()=>({default:(await import('../../../tests/vault-recovery-fixtures/Quiet.svelte')).default}));
import Page from '../../../routes/+page.svelte';
import Banner from './VaultPullRecoveryBanner.svelte';
const owner=Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const error='Vault operation 3 is still bound to different arguments. It remains held; restore the exact saved request before retrying. No approval or dispatch was made for this action.';
const intent={version:1,owner:owner.toText(),operationId:'3',savedAt:10,request:{OpenVaultAndBorrow:{amount_e8s:140_000_000n,borrow_amount_e8s:240_000_000n,collateral_type:'ryjl3-tyaaa-aaaaa-aaaba-cai'}}};
let target:HTMLDivElement, instances:ReturnType<typeof mount>[]=[];
function button(label:string) {return [...target.querySelectorAll('button')].find(b=>b.textContent?.trim()===label);}
beforeEach(()=>{
 vi.clearAllMocks();
 fx.wallet.set({isConnected:true,loading:false,principal:owner,tokenBalances:{ICP:{raw:1_000_000_000n,formatted:'10'}}});
 fx.collateral.set({collaterals:[{principal:'ryjl3-tyaaa-aaaaa-aaaba-cai',symbol:'ICP',decimals:8,price:10,minimumCr:1.5,liquidationCr:1.33,ledgerFee:10_000,status:'Active',color:'#0ff',interestRateApr:0.05}]});
 fx.app.set({protocolStatus:{lastIcpRate:10,borrowingFeeCurveResolved:[]}});
 fx.open.mockResolvedValue({success:false,error}); fx.inspect.mockResolvedValue({intent,status:{active:[],acknowledged_through:2n}});
 target=document.createElement('div'); document.body.append(target);
});
afterEach(async()=>{for(const instance of instances) await unmount(instance);instances=[];target.remove();});
it('gives the actual ICP form error a direct read-only route to the exact saved request',async()=>{
 instances.push(mount(Banner,{target}),mount(Page,{target})); flushSync();
 await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
 expect(button('Review saved request')).toBeUndefined();
 button('Create Vault & Borrow icUSD')!.click();
 await vi.waitFor(()=>expect(target.textContent).toContain(error));
 const review=button('Review saved request'); expect(review).toBeDefined();
 expect(fx.open).toHaveBeenCalledOnce(); fx.inspect.mockClear(); review!.click();
 await vi.waitFor(()=>expect(fx.inspect).toHaveBeenCalledOnce());
 await vi.waitFor(() => expect(document.activeElement?.id).toBe('vault-pull-recovery'));
 expect(target.textContent).toContain('1.4 ICP'); expect(target.textContent).toContain('2.4 icUSD');
 expect(fx.open).toHaveBeenCalledOnce(); expect(fx.resume).not.toHaveBeenCalled(); expect(fx.cancel).not.toHaveBeenCalled();
});
it('keeps healthy successful borrowing free of extra recovery controls',async()=>{
 fx.open.mockResolvedValue({success:true,vaultId:215}); instances.push(mount(Page,{target})); flushSync();
 button('Create Vault & Borrow icUSD')!.click();
 await vi.waitFor(()=>expect(target.textContent).toContain('Successfully created vault #215'));
 expect(button('Review saved request')).toBeUndefined(); expect(fx.inspect).not.toHaveBeenCalled();
});
it('does not offer saved-request navigation for an unrelated validation error',async()=>{
 fx.open.mockResolvedValue({success:false,error:'Service temporarily unavailable'}); instances.push(mount(Page,{target})); flushSync();
 button('Create Vault & Borrow icUSD')!.click();
 await vi.waitFor(()=>expect(target.textContent).toContain('Service temporarily unavailable'));
 expect(button('Review saved request')).toBeUndefined();
});
