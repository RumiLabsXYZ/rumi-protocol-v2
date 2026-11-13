import { mount, unmount, tick } from 'svelte';
import { writable } from 'svelte/store';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({ repay: vi.fn(), close: vi.fn() }));
vi.mock('../../stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: Object.assign(writable({principal:null,tokenBalances:{}}), {refreshBalance:vi.fn()}) };
});
vi.mock('../../stores/collateralStore', () => ({ collateralStore: {getCollateralInfo:()=>({symbol:'ICP',decimals:8,price:10,minVaultDebt:10000000,interestRateApr:0,ledgerFee:10000})} }));
vi.mock('../../stores/vaultStore', () => ({vaultStore:{refreshVault:vi.fn(),refreshVaults:vi.fn()}}));
vi.mock('../../stores/toast', () => ({toastStore:{success:vi.fn(),error:vi.fn()}}));
vi.mock('../../stores/appDataStore', async () => {
  const {writable}=await import('svelte/store');
  return {appDataStore:{fetchProtocolStatus:async()=>{},fetchCollateralTotals:async()=>{}},protocolStatus:writable({}),collateralTotals:writable({})};
});
vi.mock('../../stores/seasonStore', async () => {
  const {writable}=await import('svelte/store');
  return {seasonStore:{ensureLoaded:()=>{}},earningActive:writable(false)};
});
vi.mock('../../services/protocol', () => ({protocolService:{getProtocolStatus:async()=>({minIcusdAmount:500000000})}}));
vi.mock('../../services/ProtocolManager', () => ({protocolManager:{repayToVault:fx.repay,repayAndCloseVault:fx.close}}));
vi.mock('../../services/protocol/apiClient', () => ({ApiClient:{getVaultInterestRate:async()=>0}}));
vi.mock('../../services/tokenService', () => ({TokenService:{getTokenBalance:async()=>0n}}));
vi.mock('../../config', () => ({CANISTER_IDS:{ICP_LEDGER:'ryjl3-tyaaa-aaaaa-aaaba-cai'}}));
vi.mock('$lib/protocol', () => ({MINIMUM_CR:1.5,LIQUIDATION_CR:1.33,E8S:100000000,getMinimumCR:()=>1.5,getLiquidationCR:()=>1.33}));
vi.mock('../points/MultiplierBadge.svelte',async()=>({default:(await import('../../../tests/vault-recovery-fixtures/Quiet.svelte')).default}));
vi.mock('./XrpExistingDeposit.svelte',async()=>({default:(await import('../../../tests/vault-recovery-fixtures/Quiet.svelte')).default}));
import Harness from '../../../tests/vault-recovery-fixtures/RepayCard.svelte';
import {walletStore} from '../../stores/wallet';
let component:ReturnType<typeof mount>|null=null,target:HTMLDivElement;
const base={vaultId:33,borrowedIcusd:303.9384,icpMargin:200,collateralAmount:200,collateralType:'ryjl3-tyaaa-aaaaa-aaaba-cai'};
let vaultInput=writable({...base});
const input=()=>target.querySelector<HTMLInputElement>('.action-input-repay')!;
const max=()=>[...target.querySelectorAll<HTMLButtonElement>('button')].find(b=>b.textContent?.trim().startsWith('Max:'))!;
const submit=()=>target.querySelector<HTMLButtonElement>('.btn-submit-debt')!;
async function show(balance=30393000000n){
  (walletStore as any).set({principal:Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'),tokenBalances:{ICUSD:{raw:balance,formatted:String(Number(balance)/1e8)}}});
  component=mount(Harness,{target,props:{vaultInput}});await tick();await tick();
  [...target.querySelectorAll<HTMLButtonElement>('.pill-debt')].find(b=>b.textContent?.trim()==='Repay')!.click();await tick();
}
beforeEach(()=>{target=document.createElement('div');document.body.append(target);vaultInput=writable({...base});fx.repay.mockReset();fx.close.mockReset();});
afterEach(async()=>{if(component)await unmount(component);component=null;target.remove();});
it('keeps an untouched Max selection valid when refreshed debt lowers the cap',async()=>{
 await show();max().click();await tick();const previous=input().valueAsNumber;
 vaultInput.set({...base,borrowedIcusd:300});await tick();
 expect(Number(input().max)).toBeLessThan(previous);
 expect(input().valueAsNumber).toBeLessThanOrEqual(Number(input().max));
 expect(submit().disabled).toBe(false);
 expect(target.textContent).toContain('Max was reduced');
 expect(fx.repay).not.toHaveBeenCalled();expect(fx.close).not.toHaveBeenCalled();
});
it('does not rewrite a manually entered amount after a cap decrease',async()=>{
 await show();max().click();await tick();input().value='300.899015';input().dispatchEvent(new Event('input',{bubbles:true}));await tick();
 vaultInput.set({...base,borrowedIcusd:300});await tick();
 expect(input().value).toBe('300.899015');expect(submit().disabled).toBe(true);
 expect(target.textContent).toContain('Choose Max again');
});
it('keeps partial repayment when the balance cannot cover debt and approval',async()=>{
 await show(30393939999n); // one e8 below debt + approval fee
 max().click();await tick();
 expect(input().valueAsNumber).toBeLessThan(base.borrowedIcusd);
 expect(submit().textContent?.trim()).toBe('Repay');
 expect(target.textContent).toContain('0.001');
});
it('never raises an untouched Max selection when the safe cap grows',async()=>{
 await show();max().click();await tick();const selected=input().value;
 vaultInput.set({...base,borrowedIcusd:304});await tick();
 expect(input().value).toBe(selected);expect(submit().disabled).toBe(false);
});
it('freezes the entered amount and Max while a repayment request is pending',async()=>{
 fx.repay.mockReturnValue(new Promise(()=>{}));
 await show();max().click();await tick();const selected=input().value;
 submit().click();await tick();
 expect(fx.repay).toHaveBeenCalledOnce();
 vaultInput.set({...base,borrowedIcusd:300});await tick();
 expect(input().value).toBe(selected);expect(input().disabled).toBe(true);expect(max().disabled).toBe(true);
 max().click();expect(fx.repay).toHaveBeenCalledOnce();expect(fx.close).not.toHaveBeenCalled();
});
it('offers full repay and close with one approval fee and a fee-free burn',async()=>{
 await show(30393940000n);max().click();await tick();
 expect(input().valueAsNumber).toBe(base.borrowedIcusd);
 expect(submit().textContent?.trim()).toBe('Repay & Close');
});

it('keeps manual repayment disabled when the safe maximum falls to zero',async()=>{
 await show();input().value='1';input().dispatchEvent(new Event('input',{bubbles:true}));await tick();
 (walletStore as any).update((w:any)=>({...w,tokenBalances:{ICUSD:{raw:0n,formatted:'0'}}}));await tick();
 expect(input().value).toBe('1');expect(submit().disabled).toBe(true);
 expect(fx.repay).not.toHaveBeenCalled();
});

it('uses a wallet-limited near-full payment without closing and previews forgiveness',async()=>{
 vaultInput.set({...base,borrowedIcusd:2});
 fx.repay.mockReturnValue(new Promise(()=>{}));
 await show(199600000n);max().click();await tick();
 expect(input().valueAsNumber).toBe(1.995);
 expect(submit().textContent?.trim()).toBe('Repay');
 expect(target.textContent).toContain('Estimated debt after payment: 0');
 expect(target.textContent).toContain('Estimated forgiveness: 0.005');
 submit().click();await tick();
 expect(fx.repay).toHaveBeenCalledWith(33,1.995);
 expect(fx.close).not.toHaveBeenCalled();
});
it('shows exactly 0.1 as remaining debt rather than forgiven',async()=>{
 vaultInput.set({...base,borrowedIcusd:2});
 await show(190100000n);max().click();await tick();
 expect(input().valueAsNumber).toBe(1.9);
 expect(target.textContent).toContain('Estimated debt after payment: 0.1');
 expect(target.textContent).not.toContain('Estimated forgiveness:');
 expect(submit().textContent?.trim()).toBe('Repay');
});
