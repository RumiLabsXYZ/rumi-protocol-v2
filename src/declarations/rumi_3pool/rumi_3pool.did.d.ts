import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

/**
 * ─── ICRC-1 / ICRC-2 Types ───
 */
export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface Allowance {
  'allowance' : bigint,
  'expires_at' : [] | [bigint],
}
export interface AllowanceArgs { 'account' : Account, 'spender' : Account }
export interface ApproveArgs {
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
  'expected_allowance' : [] | [bigint],
  'expires_at' : [] | [bigint],
  'spender' : Account,
}
export type ApproveError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'AllowanceChanged' : { 'current_allowance' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'Expired' : { 'ledger_time' : bigint } } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export interface ArchiveInfo {
  'end' : bigint,
  'canister_id' : Principal,
  'start' : bigint,
}
export interface ArchivedBlocks {
  'args' : Array<GetBlocksArgs>,
  'callback' : [Principal, string],
}
export type ArchivedBlocksCallback = ActorMethod<
  [Array<GetBlocksArgs>],
  GetBlocksResult
>;
export interface AuthorizedRedeemAndBurnArgs {
  'token_amount' : bigint,
  'lp_amount' : bigint,
  'max_slippage_bps' : number,
  'token_ledger' : Principal,
}
export interface BalancePoint {
  'timestamp' : bigint,
  'balances' : Array<bigint>,
}
export interface BlockWithId { 'id' : bigint, 'block' : Icrc3Value }
export interface ConsentInfo {
  'metadata' : ConsentMessageMetadata,
  'consent_message' : ConsentMessage,
}
export type ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
/**
 * ─── ICRC-21 / ICRC-28 / ICRC-10 Types ───
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
export type DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
export interface ErrorInfo { 'description' : string }
export interface FeeBucket {
  'volume_per_token' : Array<bigint>,
  'min_bps' : number,
  'swap_count' : bigint,
  'max_bps' : number,
}
export interface FeeCurveParams {
  'max_fee_bps' : number,
  'imb_saturation' : bigint,
  'min_fee_bps' : number,
}
export interface FeePoint { 'timestamp' : bigint, 'avg_fee_bps' : number }
export interface FeeStats {
  'rebalancing_swap_count' : bigint,
  'rebalancing_swap_pct' : number,
  'buckets' : Array<FeeBucket>,
}
export interface GetArchivesArgs { 'from' : [] | [Principal] }
export interface GetArchivesResult { 'archives' : Array<ArchiveInfo> }
export interface GetBlocksArgs { 'start' : bigint, 'length' : bigint }
export interface GetBlocksResult {
  'log_length' : bigint,
  'blocks' : Array<BlockWithId>,
  'archived_blocks' : Array<ArchivedBlocks>,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface Icrc3DataCertificate {
  'certificate' : Uint8Array | number[],
  'hash_tree' : Uint8Array | number[],
}
/**
 * ─── ICRC-3 Types ───
 */
export type Icrc3Value = { 'Int' : bigint } |
  { 'Map' : Array<[string, Icrc3Value]> } |
  { 'Nat' : bigint } |
  { 'Blob' : Uint8Array | number[] } |
  { 'Text' : string } |
  { 'Array' : Array<Icrc3Value> };
export type ImbalanceEventKind = { 'Swap' : null } |
  { 'Liquidity' : null };
export interface ImbalanceSnapshot {
  'imbalance_after' : bigint,
  'virtual_price_after' : bigint,
  'timestamp' : bigint,
  'event_kind' : ImbalanceEventKind,
}
export interface ImbalanceStats {
  'avg' : bigint,
  'max' : bigint,
  'min' : bigint,
  'samples' : Array<[bigint, bigint]>,
  'current' : bigint,
}
export interface LineDisplayPage { 'lines' : Array<string> }
export type LiquidityAction = { 'AddLiquidity' : null } |
  { 'Donate' : null } |
  { 'RemoveOneCoin' : null } |
  { 'RemoveLiquidity' : null };
export interface LiquidityEvent {
  'id' : bigint,
  'fee' : [] | [bigint],
  'action' : LiquidityAction,
  'lp_amount' : bigint,
  'amounts' : Array<bigint>,
  'timestamp' : bigint,
  'caller' : Principal,
  'coin_index' : [] | [number],
}
export interface LiquidityEventV2 {
  'id' : bigint,
  'fee' : [] | [bigint],
  'action' : LiquidityAction,
  'imbalance_after' : bigint,
  'is_rebalancing' : boolean,
  'virtual_price_after' : bigint,
  'lp_amount' : bigint,
  'pool_balances_after' : Array<bigint>,
  'fee_bps' : [] | [number],
  'imbalance_before' : bigint,
  'migrated' : boolean,
  'amounts' : Array<bigint>,
  'timestamp' : bigint,
  'caller' : Principal,
  'coin_index' : [] | [number],
}
export type MetadataValue = { 'Int' : bigint } |
  { 'Nat' : bigint } |
  { 'Blob' : Uint8Array | number[] } |
  { 'Text' : string };
export interface OptimalRebalanceQuote {
  'dx' : bigint,
  'imbalance_after' : bigint,
  'token_in' : number,
  'profit_bps_estimate' : bigint,
  'fee_bps' : number,
  'imbalance_before' : bigint,
  'amount_out' : bigint,
  'token_out' : number,
}
export interface PoolHealth {
  'imbalance_trend_1h' : number,
  'current_imbalance' : bigint,
  'fee_at_max_imbalance_swap' : number,
  'last_swap_age_seconds' : bigint,
  'arb_opportunity_score' : number,
  'fee_at_min' : number,
}
export interface PoolStateView {
  'amp' : bigint,
  'imbalance' : bigint,
  'virtual_price' : bigint,
  'fee_curve' : FeeCurveParams,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
  'normalized_balances' : Array<bigint>,
}
export interface PoolStats {
  'liquidity_added_count' : bigint,
  'liquidity_removed_count' : bigint,
  'arb_swap_count' : bigint,
  'swap_volume_per_token' : Array<bigint>,
  'swap_count' : bigint,
  'total_fees_collected' : Array<bigint>,
  'arb_volume_per_token' : Array<bigint>,
  'avg_fee_bps' : number,
  'unique_swappers' : bigint,
}
export interface PoolStatus {
  'virtual_price' : bigint,
  'admin_fee_bps' : bigint,
  'swap_fee_bps' : bigint,
  'current_a' : bigint,
  'tokens' : Array<TokenConfig>,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
}
export interface QuoteSwapResult {
  'imbalance_after' : bigint,
  'token_in' : number,
  'is_rebalancing' : boolean,
  'virtual_price_after' : bigint,
  'fee_bps' : number,
  'imbalance_before' : bigint,
  'virtual_price_before' : bigint,
  'amount_out' : bigint,
  'amount_in' : bigint,
  'token_out' : number,
  'fee_native' : bigint,
}
export interface RedeemAndBurnResult {
  'lp_amount_burned' : bigint,
  'burn_block_index' : bigint,
  'token_amount_burned' : bigint,
}
export interface StandardRecord { 'url' : string, 'name' : string }
/**
 * ─── Explorer types ───
 */
export type StatsWindow = { 'AllTime' : null } |
  { 'Last7d' : null } |
  { 'Last24h' : null } |
  { 'Last30d' : null };
export interface SupportedBlockType { 'url' : string, 'block_type' : string }
export interface SwapEvent {
  'id' : bigint,
  'fee' : bigint,
  'token_in' : number,
  'amount_out' : bigint,
  'timestamp' : bigint,
  'caller' : Principal,
  'amount_in' : bigint,
  'token_out' : number,
}
export interface SwapEventV2 {
  'id' : bigint,
  'fee' : bigint,
  'imbalance_after' : bigint,
  'token_in' : number,
  'is_rebalancing' : boolean,
  'virtual_price_after' : bigint,
  'pool_balances_after' : Array<bigint>,
  'fee_bps' : number,
  'imbalance_before' : bigint,
  'migrated' : boolean,
  'amount_out' : bigint,
  'timestamp' : bigint,
  'caller' : Principal,
  'amount_in' : bigint,
  'token_out' : number,
}
export type ThreePoolAdminAction = { 'SetAdminFee' : { 'fee_bps' : bigint } } |
  { 'RampA' : { 'future_a_time' : bigint, 'future_a' : bigint } } |
  { 'StopRampA' : { 'frozen_a' : bigint } } |
  { 'RemoveAuthorizedBurnCaller' : { 'canister' : Principal } } |
  { 'WithdrawAdminFees' : { 'amounts' : Array<bigint> } } |
  { 'SetSwapFee' : { 'fee_bps' : bigint } } |
  {
    'FeeCurveParamsUpdated' : {
      'new' : FeeCurveParams,
      'old' : [] | [FeeCurveParams],
    }
  } |
  { 'AddAuthorizedBurnCaller' : { 'canister' : Principal } } |
  { 'SetPaused' : { 'paused' : boolean } };
export interface ThreePoolAdminEvent {
  'id' : bigint,
  'action' : ThreePoolAdminAction,
  'timestamp' : bigint,
  'caller' : Principal,
}
export type ThreePoolError = {
    'InsufficientOutput' : { 'actual' : bigint, 'expected_min' : bigint }
  } |
  { 'PoolPaused' : null } |
  { 'InvalidCoinIndex' : null } |
  { 'BurnSlippageExceeded' : { 'actual_bps' : number, 'max_bps' : number } } |
  { 'NotAuthorizedBurnCaller' : null } |
  { 'ZeroAmount' : null } |
  { 'InsufficientLpBalance' : { 'available' : bigint, 'required' : bigint } } |
  { 'BurnFailed' : { 'token' : string, 'reason' : string } } |
  { 'MathOverflow' : null } |
  { 'Unauthorized' : null } |
  { 'InvariantNotConverged' : null } |
  { 'InsufficientLiquidity' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'SlippageExceeded' : null } |
  { 'PoolEmpty' : null } |
  {
    'InsufficientPoolBalance' : {
      'token' : string,
      'available' : bigint,
      'required' : bigint,
    }
  };
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
export interface TransferArg {
  'to' : Account,
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
export type TransferError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'BadBurn' : { 'min_burn_amount' : bigint } } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export interface TransferFromArgs {
  'to' : Account,
  'fee' : [] | [bigint],
  'spender_subaccount' : [] | [Uint8Array | number[]],
  'from' : Account,
  'memo' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
export type TransferFromError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'InsufficientAllowance' : { 'allowance' : bigint } } |
  { 'BadBurn' : { 'min_burn_amount' : bigint } } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export interface VirtualPricePoint {
  'virtual_price' : bigint,
  'timestamp' : bigint,
}
export interface VirtualPriceSnapshot {
  'virtual_price' : bigint,
  'timestamp_secs' : bigint,
  'lp_total_supply' : bigint,
}
export interface VolumePoint {
  'volume_per_token' : Array<bigint>,
  'timestamp' : bigint,
}
export interface _SERVICE {
  'add_authorized_burn_caller' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'add_liquidity' : ActorMethod<
    [Array<bigint>, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'authorized_redeem_and_burn' : ActorMethod<
    [AuthorizedRedeemAndBurnArgs],
    { 'Ok' : RedeemAndBurnResult } |
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
  'donate' : ActorMethod<
    [number, bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'get_admin_event_count' : ActorMethod<[], bigint>,
  'get_admin_events' : ActorMethod<
    [bigint, bigint],
    Array<ThreePoolAdminEvent>
  >,
  'get_admin_fees' : ActorMethod<[], Array<bigint>>,
  'get_all_lp_holders' : ActorMethod<[], Array<[Principal, bigint]>>,
  'get_authorized_burn_callers' : ActorMethod<[], Array<Principal>>,
  'get_balance_series' : ActorMethod<
    [StatsWindow, bigint],
    Array<BalancePoint>
  >,
  'get_fee_curve_params' : ActorMethod<[], FeeCurveParams>,
  'get_fee_series' : ActorMethod<[StatsWindow, bigint], Array<FeePoint>>,
  'get_fee_stats' : ActorMethod<[StatsWindow], FeeStats>,
  'get_imbalance_history' : ActorMethod<
    [bigint, bigint],
    Array<ImbalanceSnapshot>
  >,
  'get_imbalance_stats' : ActorMethod<[StatsWindow], ImbalanceStats>,
  'get_liquidity_event_count' : ActorMethod<[], bigint>,
  'get_liquidity_event_count_v2' : ActorMethod<[], bigint>,
  'get_liquidity_events' : ActorMethod<[bigint, bigint], Array<LiquidityEvent>>,
  /**
   * Explorer endpoints (E1-E14)
   */
  'get_liquidity_events_by_principal' : ActorMethod<
    [Principal, bigint, bigint],
    Array<LiquidityEventV2>
  >,
  'get_liquidity_events_v2' : ActorMethod<
    [bigint, bigint],
    Array<LiquidityEventV2>
  >,
  'get_lp_balance' : ActorMethod<[Principal], bigint>,
  'get_pool_health' : ActorMethod<[], PoolHealth>,
  'get_pool_state' : ActorMethod<[], PoolStateView>,
  'get_pool_stats' : ActorMethod<[StatsWindow], PoolStats>,
  'get_pool_status' : ActorMethod<[], PoolStatus>,
  'get_swap_event_count' : ActorMethod<[], bigint>,
  'get_swap_events' : ActorMethod<[bigint, bigint], Array<SwapEvent>>,
  'get_swap_events_by_principal' : ActorMethod<
    [Principal, bigint, bigint],
    Array<SwapEventV2>
  >,
  'get_swap_events_by_time_range' : ActorMethod<
    [bigint, bigint, bigint],
    Array<SwapEventV2>
  >,
  'get_swap_events_v2' : ActorMethod<[bigint, bigint], Array<SwapEventV2>>,
  'get_top_lps' : ActorMethod<[bigint], Array<[Principal, bigint, number]>>,
  'get_top_swappers' : ActorMethod<
    [StatsWindow, bigint],
    Array<[Principal, bigint, bigint]>
  >,
  'get_virtual_price_series' : ActorMethod<
    [StatsWindow, bigint],
    Array<VirtualPricePoint>
  >,
  'get_volume_series' : ActorMethod<[StatsWindow, bigint], Array<VolumePoint>>,
  'get_vp_snapshots' : ActorMethod<[], Array<VirtualPriceSnapshot>>,
  'health' : ActorMethod<[], string>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc1_balance_of' : ActorMethod<[Account], bigint>,
  'icrc1_decimals' : ActorMethod<[], number>,
  'icrc1_fee' : ActorMethod<[], bigint>,
  'icrc1_metadata' : ActorMethod<[], Array<[string, MetadataValue]>>,
  'icrc1_minting_account' : ActorMethod<[], [] | [Account]>,
  /**
   * ICRC-1 (3USD LP Token)
   */
  'icrc1_name' : ActorMethod<[], string>,
  'icrc1_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc1_symbol' : ActorMethod<[], string>,
  'icrc1_total_supply' : ActorMethod<[], bigint>,
  'icrc1_transfer' : ActorMethod<
    [TransferArg],
    { 'Ok' : bigint } |
      { 'Err' : TransferError }
  >,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    { 'Ok' : ConsentInfo } |
      { 'Err' : Icrc21Error }
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'icrc2_allowance' : ActorMethod<[AllowanceArgs], Allowance>,
  /**
   * ICRC-2
   */
  'icrc2_approve' : ActorMethod<
    [ApproveArgs],
    { 'Ok' : bigint } |
      { 'Err' : ApproveError }
  >,
  'icrc2_transfer_from' : ActorMethod<
    [TransferFromArgs],
    { 'Ok' : bigint } |
      { 'Err' : TransferFromError }
  >,
  'icrc3_get_archives' : ActorMethod<[GetArchivesArgs], GetArchivesResult>,
  /**
   * ICRC-3 (Transaction Log)
   */
  'icrc3_get_blocks' : ActorMethod<[Array<GetBlocksArgs>], GetBlocksResult>,
  'icrc3_get_tip_certificate' : ActorMethod<[], [] | [Icrc3DataCertificate]>,
  'icrc3_supported_block_types' : ActorMethod<[], Array<SupportedBlockType>>,
  'quote_optimal_rebalance' : ActorMethod<
    [number, number],
    { 'Ok' : OptimalRebalanceQuote } |
      { 'Err' : ThreePoolError }
  >,
  'quote_swap' : ActorMethod<
    [number, number, bigint],
    { 'Ok' : QuoteSwapResult } |
      { 'Err' : ThreePoolError }
  >,
  'ramp_a' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'remove_authorized_burn_caller' : ActorMethod<
    [Principal],
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
  'set_admin_fee' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'set_fee_curve_params' : ActorMethod<
    [FeeCurveParams],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'set_paused' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'set_swap_fee' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'simulate_swap_path' : ActorMethod<
    [Array<[number, number, bigint]>],
    { 'Ok' : Array<QuoteSwapResult> } |
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
