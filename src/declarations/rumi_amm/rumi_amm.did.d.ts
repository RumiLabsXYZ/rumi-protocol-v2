import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export type AmmError = {
    'InsufficientOutput' : { 'actual' : bigint, 'expected_min' : bigint }
  } |
  { 'PoolPaused' : null } |
  { 'PoolCreationClosed' : null } |
  { 'PoolNotFound' : null } |
  { 'ZeroAmount' : null } |
  { 'DisproportionateLiquidity' : null } |
  { 'FeeBpsOutOfRange' : null } |
  { 'InvalidToken' : null } |
  { 'InsufficientLpShares' : { 'available' : bigint, 'required' : bigint } } |
  { 'MathOverflow' : null } |
  { 'Unauthorized' : null } |
  { 'PoolAlreadyExists' : null } |
  { 'InsufficientLiquidity' : null } |
  { 'MaintenanceMode' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'ClaimNotFound' : null };
export interface AmmInitArgs { 'admin' : Principal }
export interface CreatePoolArgs {
  'token_a' : Principal,
  'token_b' : Principal,
  'curve' : CurveType,
  'fee_bps' : number,
}
export type CurveType = { 'ConstantProduct' : null };
export interface PendingClaim {
  'id' : bigint,
  'token' : Principal,
  'claimant' : Principal,
  'subaccount' : Uint8Array | number[],
  'created_at' : bigint,
  'pool_id' : string,
  'amount' : bigint,
  'reason' : string,
}
export interface PoolInfo {
  'token_a' : Principal,
  'token_b' : Principal,
  'curve' : CurveType,
  'fee_bps' : number,
  'reserve_a' : bigint,
  'reserve_b' : bigint,
  'total_lp_shares' : bigint,
  'pool_id' : string,
  'protocol_fee_bps' : number,
  'paused' : boolean,
}
export interface SwapResult { 'fee' : bigint, 'amount_out' : bigint }
export interface _SERVICE {
  'add_liquidity' : ActorMethod<
    [string, bigint, bigint, bigint],
    { 'Ok' : bigint } |
      { 'Err' : AmmError }
  >,
  /**
   * ── Claims ──
   */
  'claim_pending' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  /**
   * ── Pool Creation (permissionless when open, otherwise admin-only) ──
   */
  'create_pool' : ActorMethod<
    [CreatePoolArgs],
    { 'Ok' : string } |
      { 'Err' : AmmError }
  >,
  'get_lp_balance' : ActorMethod<[string, Principal], bigint>,
  'get_pending_claims' : ActorMethod<[], Array<PendingClaim>>,
  /**
   * ── Queries ──
   */
  'get_pool' : ActorMethod<[string], [] | [PoolInfo]>,
  'get_pools' : ActorMethod<[], Array<PoolInfo>>,
  'get_quote' : ActorMethod<
    [string, Principal, bigint],
    { 'Ok' : bigint } |
      { 'Err' : AmmError }
  >,
  /**
   * ── Health ──
   */
  'health' : ActorMethod<[], string>,
  'is_maintenance_mode' : ActorMethod<[], boolean>,
  'is_pool_creation_open' : ActorMethod<[], boolean>,
  'pause_pool' : ActorMethod<[string], { 'Ok' : null } | { 'Err' : AmmError }>,
  'remove_liquidity' : ActorMethod<
    [string, bigint, bigint, bigint],
    { 'Ok' : [bigint, bigint] } |
      { 'Err' : AmmError }
  >,
  'resolve_pending_claim' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'set_fee' : ActorMethod<
    [string, number],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'set_maintenance_mode' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  /**
   * ── Admin ──
   */
  'set_pool_creation_open' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'set_protocol_fee' : ActorMethod<
    [string, number],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  /**
   * ── Core AMM ──
   */
  'swap' : ActorMethod<
    [string, Principal, bigint, bigint],
    { 'Ok' : SwapResult } |
      { 'Err' : AmmError }
  >,
  'unpause_pool' : ActorMethod<
    [string],
    { 'Ok' : null } |
      { 'Err' : AmmError }
  >,
  'withdraw_protocol_fees' : ActorMethod<
    [string],
    { 'Ok' : [bigint, bigint] } |
      { 'Err' : AmmError }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
