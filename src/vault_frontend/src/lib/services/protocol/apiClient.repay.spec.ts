import { beforeEach, afterEach, expect, it, vi } from 'vitest';
const fx = vi.hoisted(() => ({ repay:vi.fn(), close:vi.fn(), stable:vi.fn() }));
vi.mock('@dfinity/agent',()=>({HttpAgent:class {fetchRootKey(){return Promise.resolve();}},Actor:{createActor:()=>({})}}));
vi.mock('../../config',()=>({CANISTER_IDS:{},LOCAL_CANISTER_IDS:{},CONFIG:{host:'https://icp-api.io',currentCanisterId:'aaaaa-aa'}}));
vi.mock('../../stores/wallet',async()=>{const {writable}=await import('svelte/store');return {walletStore:writable({})};});
vi.mock('./walletOperations',()=>({walletOperations:{},isOisyWallet:()=>false,largeApprovalExpiry:()=>[],assertActionBoundContextCurrent:()=>{},StaleActionSessionError:class extends Error {}}));
vi.mock('../pnp',()=>({pnp:{}}));
vi.mock('./queryOperations',()=>({QueryOperations:{}}));
vi.mock('../auth',async()=>{const {writable}=await import('svelte/store');return {currentWalletType:writable(null),walletSessionGeneration:writable(0),WALLET_TYPES:{}};});
vi.mock('../PermissionManager',()=>({permissionManager:{}}));
vi.mock('../../stores/collateralStore',()=>({collateralStore:{}}));
vi.mock('../tokenService',()=>({TokenService:{}}));
vi.mock('./vaultOperationJournal',()=>({}));
vi.mock('./oisyResilience',()=>({callWithOisyFalseNegativeGuard:(fn:()=>unknown)=>fn(),isOisyArrFalseNegative:()=>false,isOisyLandedSentinel:()=>false}));
import { ApiClient, updateMinIcusdAmount } from './apiClient';
beforeEach(()=>{
 vi.spyOn(ApiClient,'executeSequentialOperation').mockImplementation((fn:any)=>fn());
 vi.spyOn(ApiClient as any,'getAuthenticatedActor').mockResolvedValue({repay_to_vault:fx.repay,repay_and_close_vault:fx.close,repay_to_vault_with_stable:fx.stable});
 vi.spyOn(ApiClient as any,'getRawBorrowedE8s').mockResolvedValue(20000000n);
 fx.repay.mockReset().mockResolvedValue({Ok:1n});
 fx.close.mockReset().mockResolvedValue({Ok:{repay_block_index:1n,collateral_return_block_index:[]}});
 fx.stable.mockReset().mockResolvedValue({Ok:1n});
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
