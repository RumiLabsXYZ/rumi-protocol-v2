import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface AmmInitArgs { 'admin' : Principal }
export type AmmError = { 'PoolNotFound' : null } |
  { 'PoolAlreadyExists' : null } |
  { 'PoolPaused' : null } |
  { 'ZeroAmount' : null } |
  { 'InsufficientOutput' : { 'expected_min' : bigint, 'actual' : bigint } } |
  { 'InsufficientLiquidity' : null } |
  { 'InsufficientLpShares' : { 'required' : bigint, 'available' : bigint } } |
  { 'InvalidToken' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'Unauthorized' : null } |
  { 'MathOverflow' : null } |
  { 'DisproportionateLiquidity' : null };
export type CurveType = { 'ConstantProduct' : null };
export interface CreatePoolArgs {
  'token_a' : Principal,
  'token_b' : Principal,
  'fee_bps' : number,
  'curve' : CurveType,
}
export interface PoolInfo {
  'pool_id' : string,
  'token_a' : Principal,
  'token_b' : Principal,
  'reserve_a' : bigint,
  'reserve_b' : bigint,
  'fee_bps' : number,
  'protocol_fee_bps' : number,
  'curve' : CurveType,
  'total_lp_shares' : bigint,
  'paused' : boolean,
}
export interface SwapResult { 'amount_out' : bigint, 'fee' : bigint }
export interface _SERVICE {
  'health' : ActorMethod<[], string>,
  'swap' : ActorMethod<
    [string, Principal, bigint, bigint],
    { 'Ok' : SwapResult } |
      { 'Err' : AmmError }
  >,
  'add_liquidity' : ActorMethod<
    [string, bigint, bigint, bigint],
    { 'Ok' : bigint } |
      { 'Err' : AmmError }
  >,
  'remove_liquidity' : ActorMethod<
    [string, bigint, bigint, bigint],
    { 'Ok' : [bigint, bigint] } |
      { 'Err' : AmmError }
  >,
  'get_pool' : ActorMethod<[string], [] | [PoolInfo]>,
  'get_pools' : ActorMethod<[], Array<PoolInfo>>,
  'get_quote' : ActorMethod<
    [string, Principal, bigint],
    { 'Ok' : bigint } |
      { 'Err' : AmmError }
  >,
  'get_lp_balance' : ActorMethod<[string, Principal], bigint>,
  'create_pool' : ActorMethod<
    [CreatePoolArgs],
    { 'Ok' : string } |
      { 'Err' : AmmError }
  >,
  'set_fee' : ActorMethod<
    [string, number],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'set_protocol_fee' : ActorMethod<
    [string, number],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'withdraw_protocol_fees' : ActorMethod<
    [string],
    { 'Ok' : [bigint, bigint] } |
      { 'Err' : AmmError }
  >,
  'pause_pool' : ActorMethod<
    [string],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'unpause_pool' : ActorMethod<
    [string],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
