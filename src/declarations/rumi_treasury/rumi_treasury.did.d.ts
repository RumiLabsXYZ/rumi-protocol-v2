import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface AssetBalance {
  'total' : bigint,
  'reserved' : bigint,
  'available' : bigint,
}
export type AssetType = { 'ICP' : null } |
  { 'CKUSDC' : null } |
  { 'CKUSDT' : null } |
  { 'ICUSD' : null } |
  { 'CKBTC' : null };
export interface CycleManagerCyclesStatus {
  'idle_burn_cycles_per_day' : [] | [bigint],
  'stable_memory_bytes' : [] | [bigint],
  'low_watermark' : bigint,
  'balance' : bigint,
  'heap_memory_bytes' : [] | [bigint],
  'healthy' : boolean,
  'freeze_threshold_secs' : bigint,
}
export interface CycleManagerMetric {
  'key' : string,
  'value' : bigint,
  'count' : bigint,
  'label' : [] | [string],
}
export interface DepositArgs {
  'asset_type' : AssetType,
  'block_index' : bigint,
  'deposit_type' : DepositType,
  'memo' : [] | [string],
  'amount' : bigint,
}
export interface DepositRecord {
  'id' : bigint,
  'asset_type' : AssetType,
  'block_index' : bigint,
  'deposit_type' : DepositType,
  'memo' : [] | [string],
  'timestamp' : bigint,
  'amount' : bigint,
}
export type DepositType = { 'BorrowingFee' : null } |
  { 'LiquidationFee' : null } |
  { 'RedemptionFee' : null } |
  { 'InterestRevenue' : null };
export type TreasuryAction = {
    'Withdraw' : {
      'to' : Principal,
      'asset_type' : AssetType,
      'amount' : bigint,
    }
  } |
  {
    'Deposit' : {
      'asset_type' : AssetType,
      'deposit_type' : DepositType,
      'amount' : bigint,
    }
  } |
  { 'SetPaused' : { 'paused' : boolean } };
export interface TreasuryEvent {
  'id' : bigint,
  'action' : TreasuryAction,
  'timestamp' : bigint,
  'caller' : Principal,
}
export interface TreasuryInitArgs {
  'controller' : Principal,
  'ckusdt_ledger' : [] | [Principal],
  'ckusdc_ledger' : [] | [Principal],
  'icp_ledger' : Principal,
  'ckbtc_ledger' : [] | [Principal],
  'icusd_ledger' : Principal,
}
export interface TreasuryStatus {
  'controller' : Principal,
  'total_deposits' : bigint,
  'is_paused' : boolean,
  'balances' : Array<[AssetType, AssetBalance]>,
}
export interface WithdrawArgs {
  'to' : Principal,
  'request_id' : [] | [bigint],
  'asset_type' : AssetType,
  'memo' : [] | [string],
  'amount' : bigint,
}
export interface WithdrawResult {
  'fee' : bigint,
  'block_index' : bigint,
  'amount_transferred' : bigint,
}
export interface _SERVICE {
  'cycle_manager_metrics' : ActorMethod<[], Array<CycleManagerMetric>>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'deposit' : ActorMethod<
    [DepositArgs],
    { 'Ok' : bigint } |
      { 'Err' : string }
  >,
  'get_deposits' : ActorMethod<
    [[] | [bigint], [] | [bigint]],
    Array<DepositRecord>
  >,
  'get_event_count' : ActorMethod<[], bigint>,
  'get_events' : ActorMethod<
    [[] | [bigint], [] | [bigint]],
    Array<TreasuryEvent>
  >,
  'get_status' : ActorMethod<[], TreasuryStatus>,
  'record_stability_pool_unallocated_interest' : ActorMethod<
    [bigint, bigint, BigUint64Array | bigint[]],
    { 'Ok' : bigint } |
      { 'Err' : string }
  >,
  'set_paused' : ActorMethod<[boolean], { 'Ok' : null } | { 'Err' : string }>,
  'set_stability_pool_reporter' : ActorMethod<
    [[] | [Principal]],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
  'withdraw' : ActorMethod<
    [WithdrawArgs],
    { 'Ok' : WithdrawResult } |
      { 'Err' : string }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
