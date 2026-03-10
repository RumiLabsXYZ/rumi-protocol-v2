import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface PoolStatus {
  'virtual_price' : bigint,
  'admin_fee_bps' : bigint,
  'swap_fee_bps' : bigint,
  'current_a' : bigint,
  'tokens' : Array<TokenConfig>,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
}
export type ThreePoolError = {
    'InsufficientOutput' : { 'actual' : bigint, 'expected_min' : bigint }
  } |
  { 'PoolPaused' : null } |
  { 'InvalidCoinIndex' : null } |
  { 'ZeroAmount' : null } |
  { 'MathOverflow' : null } |
  { 'Unauthorized' : null } |
  { 'InvariantNotConverged' : null } |
  { 'InsufficientLiquidity' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'SlippageExceeded' : null } |
  { 'PoolEmpty' : null };
export interface ThreePoolInitArgs {
  'admin_fee_bps' : bigint,
  'admin' : Principal,
  'swap_fee_bps' : bigint,
  'initial_a' : bigint,
  'tokens' : Array<TokenConfig>,
}
export interface TokenConfig {
  'decimals' : number,
  'precision_mul' : bigint,
  'ledger_id' : Principal,
  'symbol' : string,
}
export interface _SERVICE {
  'add_liquidity' : ActorMethod<
    [Array<bigint>, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'calc_add_liquidity_query' : ActorMethod<
    [Array<bigint>, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'calc_remove_liquidity_query' : ActorMethod<
    [bigint],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
  'calc_remove_one_coin_query' : ActorMethod<
    [bigint, number],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'calc_swap' : ActorMethod<
    [number, number, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'get_admin_fees' : ActorMethod<[], Array<bigint>>,
  'get_lp_balance' : ActorMethod<[Principal], bigint>,
  'get_pool_status' : ActorMethod<[], PoolStatus>,
  'health' : ActorMethod<[], string>,
  'ramp_a' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'remove_liquidity' : ActorMethod<
    [bigint, Array<bigint>],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
  'remove_one_coin' : ActorMethod<
    [bigint, number, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'set_paused' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'stop_ramp_a' : ActorMethod<[], { 'Ok' : null } | { 'Err' : ThreePoolError }>,
  'swap' : ActorMethod<
    [number, number, bigint, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'withdraw_admin_fees' : ActorMethod<
    [],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
