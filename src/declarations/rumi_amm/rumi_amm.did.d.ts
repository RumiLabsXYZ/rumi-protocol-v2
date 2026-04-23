import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

/**
 * ── Admin Events ──
 */
export type AmmAdminAction = { 'SetPoolCreationOpen' : { 'open' : boolean } } |
  {
    'WithdrawProtocolFees' : {
      'amount_a' : bigint,
      'amount_b' : bigint,
      'pool_id' : string,
    }
  } |
  {
    'ClaimPending' : {
      'claim_id' : bigint,
      'claimant' : Principal,
      'amount' : bigint,
    }
  } |
  {
    'CreatePool' : {
      'token_a' : Principal,
      'token_b' : Principal,
      'fee_bps' : number,
      'pool_id' : string,
    }
  } |
  { 'SetProtocolFee' : { 'pool_id' : string, 'protocol_fee_bps' : number } } |
  { 'SetFee' : { 'fee_bps' : number, 'pool_id' : string } } |
  { 'SetMaintenanceMode' : { 'enabled' : boolean } } |
  { 'UnpausePool' : { 'pool_id' : string } } |
  { 'PausePool' : { 'pool_id' : string } } |
  { 'ResolvePendingClaim' : { 'claim_id' : bigint } };
export interface AmmAdminEvent {
  'id' : bigint,
  'action' : AmmAdminAction,
  'timestamp' : bigint,
  'caller' : Principal,
}
export interface AmmBalancePoint {
  'ts_ns' : bigint,
  'reserve_b_e8s' : bigint,
  'reserve_a_e8s' : bigint,
}
/**
 * ── AMM Errors ──
 */
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
  { 'PoolBusy' : null } |
  { 'InsufficientLiquidity' : null } |
  { 'MaintenanceMode' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'ClaimNotFound' : null };
export interface AmmEventsByPrincipalQuery {
  'who' : Principal,
  'pool' : string,
  'start' : bigint,
  'length' : bigint,
}
export interface AmmEventsByTimeRangeQuery {
  'start_ns' : bigint,
  'pool' : string,
  'limit' : bigint,
  'end_ns' : bigint,
}
export interface AmmFeePoint {
  'ts_ns' : bigint,
  'fees_a_e8s' : bigint,
  'fees_b_e8s' : bigint,
}
export interface AmmInitArgs { 'admin' : Principal }
/**
 * ── Liquidity Events ──
 */
export type AmmLiquidityAction = { 'AddLiquidity' : null } |
  { 'RemoveLiquidity' : null };
export interface AmmLiquidityEvent {
  'id' : bigint,
  'action' : AmmLiquidityAction,
  'token_a' : Principal,
  'token_b' : Principal,
  'amount_a' : bigint,
  'amount_b' : bigint,
  'lp_shares' : bigint,
  'timestamp' : bigint,
  'caller' : Principal,
  'pool_id' : string,
}
export interface AmmPoolStats {
  'volume_b_e8s' : bigint,
  'fees_a_e8s' : bigint,
  'pool' : string,
  'window' : AmmStatsWindow,
  'generated_at_ns' : bigint,
  'unique_lps' : number,
  'volume_a_e8s' : bigint,
  'swap_count' : number,
  'fees_b_e8s' : bigint,
  'unique_swappers' : number,
}
export interface AmmSeriesQuery {
  'pool' : string,
  'window' : AmmStatsWindow,
  'points' : number,
}
export interface AmmStatsQuery { 'pool' : string, 'window' : AmmStatsWindow }
/**
 * ── Analytics ──
 */
export type AmmStatsWindow = { 'All' : null } |
  { 'Day' : null } |
  { 'Hour' : null } |
  { 'Week' : null } |
  { 'Month' : null };
export interface AmmSwapEvent {
  'id' : bigint,
  'fee' : bigint,
  'token_in' : Principal,
  'amount_out' : bigint,
  'timestamp' : bigint,
  'caller' : Principal,
  'amount_in' : bigint,
  'token_out' : Principal,
  'pool_id' : string,
}
export interface AmmTopLpsQuery { 'pool' : string, 'limit' : number }
export interface AmmTopSwappersQuery {
  'pool' : string,
  'window' : AmmStatsWindow,
  'limit' : number,
}
export interface AmmVolumePoint {
  'ts_ns' : bigint,
  'volume_b_e8s' : bigint,
  'volume_a_e8s' : bigint,
  'swap_count' : number,
}
export interface ConsentInfo {
  'metadata' : ConsentMessageMetadata,
  'consent_message' : ConsentMessage,
}
export type ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
/**
 * ── ICRC-21 Consent Messages ──
 */
export interface ConsentMessageMetadata {
  'utc_offset_minutes' : [] | [number],
  'language' : string,
}
export interface ConsentMessageRequest {
  'arg' : Uint8Array | number[],
  'method' : string,
  'user_preferences' : ConsentMessageSpec,
}
export interface ConsentMessageSpec {
  'metadata' : ConsentMessageMetadata,
  'device_spec' : [] | [DeviceSpec],
}
export interface CreatePoolArgs {
  'token_a' : Principal,
  'token_b' : Principal,
  'curve' : CurveType,
  'fee_bps' : number,
}
export type CurveType = { 'ConstantProduct' : null };
export type DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
/**
 * ── HTTP Request ──
 */
export type HeaderField = [string, string];
/**
 * ── Holder Snapshots ──
 */
export interface HolderEntry { 'balance' : bigint, 'holder' : Principal }
export interface HolderSnapshot {
  'top_holders' : Array<HolderEntry>,
  'token' : string,
  'holder_count' : bigint,
  'timestamp' : bigint,
  'total_supply' : bigint,
}
export interface HttpRequest {
  'url' : string,
  'method' : string,
  'body' : Uint8Array | number[],
  'headers' : Array<HeaderField>,
}
export interface HttpResponse {
  'body' : Uint8Array | number[],
  'headers' : Array<HeaderField>,
  'status_code' : number,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : { 'description' : string } } |
  { 'ConsentMessageUnavailable' : { 'description' : string } };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface LineDisplayPage { 'lines' : Array<string> }
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
export interface StandardRecord { 'url' : string, 'name' : string }
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
  'get_amm_admin_event_count' : ActorMethod<[], bigint>,
  /**
   * ── Admin Event History ──
   */
  'get_amm_admin_events' : ActorMethod<[bigint, bigint], Array<AmmAdminEvent>>,
  'get_amm_balance_series' : ActorMethod<
    [AmmSeriesQuery],
    Array<AmmBalancePoint>
  >,
  'get_amm_fee_series' : ActorMethod<[AmmSeriesQuery], Array<AmmFeePoint>>,
  'get_amm_liquidity_event_count' : ActorMethod<[], bigint>,
  /**
   * ── Liquidity Event History ──
   */
  'get_amm_liquidity_events' : ActorMethod<
    [bigint, bigint],
    Array<AmmLiquidityEvent>
  >,
  'get_amm_liquidity_events_by_principal' : ActorMethod<
    [AmmEventsByPrincipalQuery],
    Array<AmmLiquidityEvent>
  >,
  'get_amm_pool_stats' : ActorMethod<[AmmStatsQuery], AmmPoolStats>,
  'get_amm_swap_event_count' : ActorMethod<[], bigint>,
  /**
   * ── Swap Event History ──
   */
  'get_amm_swap_events' : ActorMethod<[bigint, bigint], Array<AmmSwapEvent>>,
  'get_amm_swap_events_by_principal' : ActorMethod<
    [AmmEventsByPrincipalQuery],
    Array<AmmSwapEvent>
  >,
  'get_amm_swap_events_by_time_range' : ActorMethod<
    [AmmEventsByTimeRangeQuery],
    Array<AmmSwapEvent>
  >,
  'get_amm_top_lps' : ActorMethod<
    [AmmTopLpsQuery],
    Array<[Principal, bigint, number]>
  >,
  'get_amm_top_swappers' : ActorMethod<
    [AmmTopSwappersQuery],
    Array<[Principal, bigint, bigint]>
  >,
  /**
   * ── Analytics (parity with rumi_3pool) ──
   */
  'get_amm_volume_series' : ActorMethod<
    [AmmSeriesQuery],
    Array<AmmVolumePoint>
  >,
  'get_holder_snapshot_count' : ActorMethod<[string], bigint>,
  /**
   * ── Holder Snapshots ──
   */
  'get_holder_snapshots' : ActorMethod<
    [string, bigint, bigint],
    Array<HolderSnapshot>
  >,
  'get_latest_holder_snapshot' : ActorMethod<[string], [] | [HolderSnapshot]>,
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
  /**
   * ── HTTP ──
   */
  'http_request' : ActorMethod<[HttpRequest], HttpResponse>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  /**
   * ── ICRC-21 / ICRC-28 / ICRC-10 ──
   */
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    { 'Ok' : ConsentInfo } |
      { 'Err' : Icrc21Error }
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
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
  'set_admin' : ActorMethod<
    [Principal],
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
