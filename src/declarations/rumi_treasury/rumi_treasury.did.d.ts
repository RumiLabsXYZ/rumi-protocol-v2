import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface AssetBalance {
  'total' : bigint,
  'reserved' : bigint,
  'available' : bigint,
}
export type AssetType = { 'ICP' : null } |
  { 'ICUSD' : null } |
  { 'CKBTC' : null };
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
export type DepositType = { 'RedemptionFee' : null } |
  { 'MintingFee' : null } |
  { 'StabilityFee' : null } |
  { 'LiquidationSurplus' : null };
export interface TreasuryInitArgs {
  'controller' : Principal,
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
  'deposit' : ActorMethod<
    [DepositArgs],
    { 'Ok' : bigint } |
      { 'Err' : string }
  >,
  'get_deposits' : ActorMethod<
    [[] | [bigint], [] | [bigint]],
    Array<DepositRecord>
  >,
  'get_status' : ActorMethod<[], TreasuryStatus>,
  'set_controller' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
  'set_paused' : ActorMethod<[boolean], { 'Ok' : null } | { 'Err' : string }>,
  'withdraw' : ActorMethod<
    [WithdrawArgs],
    { 'Ok' : WithdrawResult } |
      { 'Err' : string }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
