import {beforeEach,expect,it,vi} from 'vitest';
import { IDL } from '@dfinity/candid';
import { Principal } from '@dfinity/principal';
import { idlFactory } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import {runVaultPullOperation,acknowledgeHeldVaultPullRecovery,inspectVaultPullRecovery,normalizeVaultPullStatus,heldVaultPullRecovery,type VaultPullView,type VaultPullStatus} from './vaultOperationJournal';
const owner='rrkah-fqaaa-aaaaa-aaaaq-cai',key=`rumi:vault-pull-operation:v1:${owner}`;
const request={AddMargin:{vault_id:5n,amount_e8s:100n}};
const held=():VaultPullView=>({operation_id:1n,request,vault_id:5n,phase:{PullConfirmed:{block_index:42n}},recovery:[{vault_id:5n,block_index:42n,amount:100n,ledger:'ryjl3-tyaaa-aaaaa-aaaba-cai',payout_operation_id:900n}]});
function fixture(initial:VaultPullStatus){
 let status=initial;
 const adapter={currentOwner:()=>owner,getStatus:vi.fn(async()=>status),approve:vi.fn(async()=>({success:true})),dispatch:vi.fn(),completed:vi.fn(()=>({held:true})),acknowledge:vi.fn(async(id:bigint)=>{status={active:[],acknowledged_through:id};return id;})};
 return {adapter,set:(s:VaultPullStatus)=>status=s};
}
beforeEach(()=>{localStorage.clear();Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async(_n:string,_o:any,fn:any)=>fn({})}});});
it('retains a held liability notice without approval, retry or automatic ACK',async()=>{
 const {adapter}=fixture({active:[held()],acknowledged_through:0n});
 expect(await runVaultPullOperation({owner,request,adapter})).toEqual({held:true});
 expect(adapter.approve).not.toHaveBeenCalled();expect(adapter.dispatch).not.toHaveBeenCalled();expect(adapter.acknowledge).not.toHaveBeenCalled();expect(localStorage.getItem(key)).toBeTruthy();
});
it('detects recovery after a lost dispatch reply without paying twice or ACKing it',async()=>{
 const {adapter,set}=fixture({active:[],acknowledged_through:0n});
 adapter.dispatch.mockImplementation(async()=>{set({active:[held()],acknowledged_through:0n});throw Error('reply lost');});
 expect(await runVaultPullOperation({owner,request,adapter})).toEqual({held:true});
 expect(adapter.dispatch).toHaveBeenCalledOnce();expect(adapter.approve).toHaveBeenCalledOnce();expect(adapter.acknowledge).not.toHaveBeenCalled();
});
it('only explicit acknowledgement clears the notice; no payment method is invoked',async()=>{
 const view=held();const {adapter}=fixture({active:[view],acknowledged_through:0n});
 await runVaultPullOperation({owner,request,adapter});
 expect(await acknowledgeHeldVaultPullRecovery({owner,view,adapter})).toBe(1n);
 expect(localStorage.getItem(key)).toBeNull();expect(adapter.acknowledge).toHaveBeenCalledOnce();expect(adapter.approve).not.toHaveBeenCalled();expect(adapter.dispatch).not.toHaveBeenCalled();
});
it('reconciles a lost ACK reply once without resending acknowledgement',async()=>{
 const view=held();const {adapter,set}=fixture({active:[view],acknowledged_through:0n});
 adapter.acknowledge.mockImplementationOnce(async()=>{set({active:[],acknowledged_through:1n});throw Error('lost ACK');});
 expect(await acknowledgeHeldVaultPullRecovery({owner,view,adapter})).toBe(1n);expect(adapter.acknowledge).toHaveBeenCalledOnce();
});
it('does not acknowledge a changed liability or an unconfirmed pull',async()=>{
 const view=held();const changed=held();changed.recovery![0].payout_operation_id=901n;
 const {adapter,set}=fixture({active:[changed],acknowledged_through:0n});
 await expect(acknowledgeHeldVaultPullRecovery({owner,view,adapter})).rejects.toThrow('notice changed');
 const unconfirmed=held();unconfirmed.recovery=[];set({active:[unconfirmed],acknowledged_through:0n});
 await expect(acknowledgeHeldVaultPullRecovery({owner,view:unconfirmed,adapter})).rejects.toThrow('No confirmed');expect(adapter.acknowledge).not.toHaveBeenCalled();
});
it('a background inspection only reads the owner-bound notice',async()=>{
 const {adapter}=fixture({active:[held()],acknowledged_through:0n});
 expect((await inspectVaultPullRecovery({owner,adapter})).status.active[0]?.recovery?.[0].payout_operation_id).toBe(900n);
 expect(adapter.approve).not.toHaveBeenCalled();expect(adapter.dispatch).not.toHaveBeenCalled();expect(adapter.acknowledge).not.toHaveBeenCalled();
});

it('decodes the generated actor status with exact recovery amounts and wide payout references', () => {
 const ledger = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
 const amount = 13_019_090_000n;
 const payoutId = (1n << 100n) + 900n;
 const view = held();
 view.request = { AddMargin: { vault_id: 5n, amount_e8s: amount } };
 const row = { ...view.recovery![0], amount, payout_operation_id: payoutId };
 const wire = { Ok: { active: [{ ...view, confirmed_wide_block_index: [], recovery: [{ ...row, ledger }] }], acknowledged_through: 0n } };
 const method = (idlFactory({ IDL }) as IDL.ServiceClass)._fields.find(([name]) => name === 'get_vault_operation_status')![1];
 const bytes = IDL.encode(method.retTypes, [wire]);
 const decoded = IDL.decode(method.retTypes, bytes)[0] as any;
 const normalized = normalizeVaultPullStatus(decoded.Ok);
 const recovery = heldVaultPullRecovery(normalized.active[0]!);
 expect(recovery).toEqual({ ...row, ledger: ledger.toText() });
 expect(recovery!.amount).toBe(amount);
 expect(recovery!.payout_operation_id).toBe(payoutId);
});
