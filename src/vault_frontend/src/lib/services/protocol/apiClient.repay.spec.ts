import { beforeEach, afterEach, expect, it, vi } from 'vitest';
const fx = vi.hoisted(() => ({ repay:vi.fn(), close:vi.fn(), stable:vi.fn(), oisy:false }));
vi.mock('@dfinity/agent',()=>({HttpAgent:class {fetchRootKey(){return Promise.resolve();}},Actor:{createActor:()=>({})}}));
vi.mock('../../config',()=>({CANISTER_IDS:{},LOCAL_CANISTER_IDS:{},CONFIG:{host:'https://icp-api.io',currentCanisterId:'aaaaa-aa',currentIcusdLedgerId:'ryjl3-tyaaa-aaaaa-aaaba-cai',getStableLedgerId:(token:string)=>token==='CKUSDC'?'xevnm-gaaaa-aaaar-qafnq-cai':'cngnf-vqaaa-aaaar-qag4q-cai'}}));
vi.mock('../../stores/wallet',async()=>{const {writable}=await import('svelte/store');return {walletStore:writable({})};});
vi.mock('./walletOperations',()=>({walletOperations:{checkCollateralAllowanceBound:vi.fn().mockResolvedValue(1_000_000_000_000_000_000n),approveCollateralTransferBound:vi.fn().mockResolvedValue({success:true})},isOisyWallet:()=>fx.oisy,largeApprovalExpiry:()=>[],assertActionBoundContextCurrent:(ctx:any)=>{if(!ctx.assertCurrent())throw new Error('wallet session changed');},StaleActionSessionError:class extends Error {}}));
vi.mock('../pnp',()=>({pnp:{}}));
vi.mock('./queryOperations',()=>({QueryOperations:{}}));
vi.mock('../auth',async()=>{const {writable}=await import('svelte/store');return {currentWalletType:writable(null),walletSessionGeneration:writable(0),WALLET_TYPES:{}};});
vi.mock('../PermissionManager',()=>({permissionManager:{}}));
vi.mock('../../stores/collateralStore',()=>({collateralStore:{}}));
vi.mock('../tokenService',()=>({TokenService:{}}));
vi.mock('./vaultOperationJournal',()=>({}));
vi.mock('./oisyResilience',()=>({callWithOisyFalseNegativeGuard:(fn:()=>unknown)=>fn(),isOisyArrFalseNegative:()=>false,isOisyLandedSentinel:()=>false}));
import { ApiClient, updateMinIcusdAmount } from './apiClient';
import { walletOperations } from './walletOperations';
const ctx={expectedPrincipalText:'aaaaa-aa',assertCurrent:()=>true};
beforeEach(()=>{
 fx.oisy=false;
 vi.spyOn(ApiClient,'executeSequentialOperation').mockImplementation((fn:any)=>fn());
 vi.spyOn(ApiClient as any,'getAuthenticatedActor').mockResolvedValue({repay_to_vault:fx.repay,repay_and_close_vault:fx.close,repay_to_vault_with_stable:fx.stable});
 vi.spyOn(ApiClient as any,'getBoundAuthenticatedActor').mockResolvedValue({repay_to_vault:fx.repay,repay_and_close_vault:fx.close,repay_to_vault_with_stable:fx.stable});
 vi.spyOn(ApiClient as any,'getRawBorrowedE8s').mockResolvedValue(20000000n);
 fx.repay.mockReset().mockResolvedValue({Ok:1n});
 fx.close.mockReset().mockResolvedValue({Ok:{repay_block_index:1n,collateral_return_block_index:[]}});
 fx.stable.mockReset().mockResolvedValue({Ok:1n});
 vi.mocked(walletOperations.checkCollateralAllowanceBound).mockReset().mockResolvedValue(1_000_000_000_000_000_000n);
 vi.mocked(walletOperations.approveCollateralTransferBound).mockReset().mockResolvedValue({success:true});
 updateMinIcusdAmount(500000000);
});
afterEach(()=>{vi.restoreAllMocks();updateMinIcusdAmount(10000000);});
it('dispatches ordinary 0.1 independent of the borrowing minimum on both rails',async()=>{
 expect((await ApiClient.repayToVault(33,0.1)).success).toBe(true);
 expect(fx.repay).toHaveBeenCalledWith({vault_id:33n,amount:10000000n});
 expect((await ApiClient.repayToVaultWithStable(33,0.1,'CKUSDC')).success).toBe(true);
 expect(fx.stable).toHaveBeenCalledWith({vault_id:33n,amount:10000000n,token_type:{CKUSDC:null}});
});
it('rejects subminimum ordinary payment before dispatch but permits explicit close',async()=>{
 expect((await ApiClient.repayToVault(33,0.05)).success).toBe(false);
 expect((await ApiClient.repayToVaultWithStable(33,0.05,'CKUSDT')).success).toBe(false);
 expect(fx.repay).not.toHaveBeenCalled();expect(fx.stable).not.toHaveBeenCalled();
 expect((await ApiClient.repayAndCloseVault(33,0.05)).success).toBe(true);
 expect(fx.close).toHaveBeenCalledWith({vault_id:33n,amount:5000000n});
});
it('does not dispatch a zero-unit or nonfinite explicit close payment',async()=>{
 for(const amount of [0,-1,NaN,Infinity,0.000000001])expect((await ApiClient.repayAndCloseVault(33,amount)).success).toBe(false);
 expect(fx.close).not.toHaveBeenCalled();
});
it('dispatches exact raw repayment units without a Number round trip',async()=>{
 const result=await ApiClient.repayToVaultBound(ctx,33,12_500_002n);
 expect(result.success).toBe(true);
 expect(fx.repay).toHaveBeenCalledWith({vault_id:33n,amount:12_500_002n});
});
it('dispatches exact six-decimal stable input as debt-denominated e8s',async()=>{
 const result=await ApiClient.repayToVaultWithStableBound(ctx,33,12_500_000n,'CKUSDC');
 expect(result.success).toBe(true);
 expect(fx.stable).toHaveBeenCalledWith({vault_id:33n,amount:12_500_000n,token_type:{CKUSDC:null}});
});
it('rejects stable precision the backend would otherwise truncate',async()=>{
 const result=await ApiClient.repayToVaultWithStableBound(ctx,33,12_500_002n,'CKUSDC');
 expect(result.success).toBe(false);
 expect(result.error).toContain('6 decimal places');
 expect(fx.stable).not.toHaveBeenCalled();
});
it('marks a lost bound repayment reply ambiguous instead of inviting a blind replay',async()=>{
 fx.repay.mockRejectedValueOnce(new Error('transport lost'));
 const result=await ApiClient.repayToVaultBound(ctx,33,12_500_002n);
 expect(result.success).toBe(false);
 expect(result.ambiguous).toBe(true);
 expect(result.error).toContain('Check the vault and ledger before retrying');
});
it('does not dispatch after the wallet session changes while allowance is loading',async()=>{
 let live=true;let release!:(value:bigint)=>void;
 vi.mocked(walletOperations.checkCollateralAllowanceBound).mockReturnValueOnce(new Promise(resolve=>{release=resolve;}));
 const action=ApiClient.repayToVaultBound({expectedPrincipalText:'aaaaa-aa',assertCurrent:()=>live},33,12_500_002n);
 await vi.waitFor(()=>expect(walletOperations.checkCollateralAllowanceBound).toHaveBeenCalledOnce());
 live=false;release(0n);
 const result=await action;
 expect(result.success).toBe(false);
 expect(fx.repay).not.toHaveBeenCalled();
 expect(walletOperations.approveCollateralTransferBound).not.toHaveBeenCalled();
});
it('reports a possibly changed allowance but does not repay after a post-approval session change',async()=>{
 let live=true;let release!:(value:{success:boolean})=>void;
 vi.mocked(walletOperations.checkCollateralAllowanceBound).mockResolvedValueOnce(0n);
 vi.mocked(walletOperations.approveCollateralTransferBound).mockReturnValueOnce(new Promise(resolve=>{release=resolve;}));
 const action=ApiClient.repayToVaultBound({expectedPrincipalText:'aaaaa-aa',assertCurrent:()=>live},33,12_500_002n);
 await vi.waitFor(()=>expect(walletOperations.approveCollateralTransferBound).toHaveBeenCalledOnce());
 live=false;release({success:true});
 const result=await action;
 expect(result.success).toBe(false);
 expect(result.approvalMayHaveMutated).toBe(true);
 expect(fx.repay).not.toHaveBeenCalled();
});
it('keeps the Oisy click path free of an allowance query before approval and exact dispatch',async()=>{
 fx.oisy=true;
 const result=await ApiClient.repayToVaultBound(ctx,33,12_500_002n);
 expect(result.success).toBe(true);
 expect(walletOperations.checkCollateralAllowanceBound).not.toHaveBeenCalled();
 expect(walletOperations.approveCollateralTransferBound).toHaveBeenCalledWith(
  ctx,100_000_000_000_000_000n,'aaaaa-aa','ryjl3-tyaaa-aaaaa-aaaba-cai'
 );
 expect(fx.repay).toHaveBeenCalledWith({vault_id:33n,amount:12_500_002n});
});
