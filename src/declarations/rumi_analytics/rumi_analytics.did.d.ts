import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface AddressValuePoint {
  'ts_ns' : bigint,
  'breakdown' : Array<AddressValueSourceBreakdown>,
  'value_usd_e8s' : bigint,
}
export interface AddressValueSeriesQuery {
  'principal' : Principal,
  'resolution_ns' : [] | [bigint],
  'window_ns' : [] | [bigint],
}
export interface AddressValueSeriesResponse {
  'principal' : Principal,
  'resolution_ns' : bigint,
  'generated_at_ns' : bigint,
  'approximate_sources' : Array<string>,
  'window_ns' : bigint,
  'points' : Array<AddressValuePoint>,
}
export interface AddressValueSourceBreakdown {
  'source' : string,
  'value_usd_e8s' : bigint,
}
export interface AdminEventBreakdownQuery { 'window_ns' : [] | [bigint] }
export interface AdminEventBreakdownResponse {
  'labels' : Array<AdminEventLabelCount>,
  'generated_at_ns' : bigint,
  'window_ns' : bigint,
}
export interface AdminEventLabelCount {
  'count' : bigint,
  'label' : string,
  'last_at_ns' : [] | [bigint],
}
export interface AmmPoolSnapshot {
  'token_a' : Principal,
  'token_b' : Principal,
  'reserve_a' : bigint,
  'reserve_b' : bigint,
  'total_lp_shares' : bigint,
  'pool_id' : string,
}
export interface AnalyticsAmmLiquidityEvent {
  'action' : LiquidityAction,
  'timestamp_ns' : bigint,
  'source_event_id' : bigint,
  'lp_shares' : bigint,
  'caller' : Principal,
  'pool_id' : string,
}
export interface AnalyticsSwapEvent {
  'fee' : bigint,
  'timestamp_ns' : bigint,
  'token_in' : Principal,
  'source' : SwapSource,
  'source_event_id' : bigint,
  'amount_out' : bigint,
  'caller' : Principal,
  'amount_in' : bigint,
  'token_out' : Principal,
}
export interface ApyQuery { 'window_days' : [] | [number] }
export interface ApyResponse {
  'lp_apy_pct' : [] | [number],
  'amm_apy_pct' : [] | [number],
  'window_days' : number,
  'sp_apy_pct' : [] | [number],
}
export interface BackfillProgress {
  'cursor_after' : bigint,
  'from' : bigint,
  'scanned' : bigint,
  'complete' : boolean,
  'emitted' : bigint,
  'total_events' : bigint,
}
export interface BalanceTrackerStats {
  'token' : Principal,
  'total_tracked_e8s' : bigint,
  'holder_count' : bigint,
}
export interface CollateralStats {
  'total_collateral_e8s' : bigint,
  'median_cr_bps' : number,
  'price_usd_e8s' : bigint,
  'total_debt_e8s' : bigint,
  'max_cr_bps' : number,
  'collateral_type' : Principal,
  'min_cr_bps' : number,
  'vault_count' : number,
}
export interface CollectorHealth {
  'balance_tracker_stats' : Array<BalanceTrackerStats>,
  'backfill_active' : Array<Principal>,
  'error_counters' : ErrorCounters,
  'last_pull_cycle_ns' : bigint,
  'cursors' : Array<CursorStatus>,
}
export interface CursorStatus {
  'last_error' : [] | [string],
  'source_count' : bigint,
  'name' : string,
  'last_success_ns' : bigint,
  'cursor_position' : bigint,
}
export type CycleManagerCriticality = { 'Important' : null } |
  { 'Experimental' : null } |
  { 'Critical' : null } |
  { 'Standard' : null };
export interface CycleManagerCyclesStatus {
  'idle_burn_cycles_per_day' : [] | [bigint],
  'stable_memory_bytes' : [] | [bigint],
  'low_watermark' : bigint,
  'balance' : bigint,
  'heap_memory_bytes' : [] | [bigint],
  'healthy' : boolean,
  'freeze_threshold_secs' : bigint,
}
export type CycleManagerEnvironment = { 'Local' : null } |
  { 'Production' : null } |
  { 'Test' : null } |
  { 'Archived' : null } |
  { 'Staging' : null };
export interface CycleManagerMetric {
  'key' : string,
  'value' : bigint,
  'count' : bigint,
  'label' : [] | [string],
}
export interface CycleManagerTarget {
  'low_threshold_cycles' : bigint,
  'topup_cycles' : bigint,
  'owner' : [] | [string],
  'kind' : CycleManagerTargetKind,
  'name' : string,
  'tags' : Array<string>,
  'canister_id' : Principal,
  'expected_freeze_threshold_secs' : [] | [bigint],
  'expected_controllers' : Array<Principal>,
  'criticality' : CycleManagerCriticality,
  'environment' : CycleManagerEnvironment,
  'metrics_schema_version' : number,
  'project' : string,
}
export type CycleManagerTargetKind = { 'SelfReport' : null } |
  { 'Controlled' : null };
export interface CycleSeriesResponse {
  'rows' : Array<HourlyCycleSnapshot>,
  'next_from_ts' : [] | [bigint],
}
export interface DailyFeeRollup {
  'redemption_count' : number,
  'borrow_count' : number,
  'timestamp_ns' : bigint,
  'swap_fees_e8s' : bigint,
  'redemption_fees_e8s' : [] | [bigint],
  'borrowing_fees_e8s' : [] | [bigint],
}
export interface DailyHolderRow {
  'total_supply_tracked_e8s' : bigint,
  'token' : Principal,
  'new_holders_today' : number,
  'timestamp_ns' : bigint,
  'median_balance_e8s' : bigint,
  'total_holders' : number,
  'top_10_pct_bps' : number,
  'top_50' : Array<[Principal, bigint]>,
  'distribution_buckets' : Uint32Array | number[],
  'gini_bps' : number,
}
export interface DailyLiquidationRollup {
  'total_debt_covered_e8s' : bigint,
  'timestamp_ns' : bigint,
  'total_collateral_seized_e8s' : bigint,
  'redistribution_count' : number,
  'by_collateral' : Array<[Principal, bigint]>,
  'partial_count' : number,
  'full_count' : number,
}
export interface DailyStabilityRow {
  'collateral_gains' : Array<[Principal, bigint]>,
  'timestamp_ns' : bigint,
  'total_depositors' : bigint,
  'stablecoin_balances' : Array<[Principal, bigint]>,
  'total_deposits_e8s' : bigint,
  'total_interest_received_e8s' : bigint,
  'total_liquidations_executed' : bigint,
}
export interface DailySwapRollup {
  'three_pool_fees_e8s' : bigint,
  'timestamp_ns' : bigint,
  'three_pool_swap_count' : number,
  'amm_volume_e8s' : bigint,
  'three_pool_volume_e8s' : bigint,
  'amm_swap_count' : number,
  'amm_fees_e8s' : bigint,
  'unique_swappers' : number,
}
export interface DailyTvlRow {
  'three_pool_reserve_0_e8s' : [] | [bigint],
  'timestamp_ns' : bigint,
  'three_pool_reserve_2_e8s' : [] | [bigint],
  'three_pool_virtual_price_e18' : [] | [bigint],
  'amm_rewards_e8s' : [] | [bigint],
  'total_icusd_supply_e8s' : bigint,
  'system_collateral_ratio_bps' : number,
  'total_icp_collateral_e8s' : bigint,
  'amm_tvl_usd_e8s' : [] | [bigint],
  'three_pool_reserve_1_e8s' : [] | [bigint],
  'stability_pool_deposits_e8s' : [] | [bigint],
  'three_pool_lp_supply_e8s' : [] | [bigint],
}
export interface DailyVaultSnapshotRow {
  'timestamp_ns' : bigint,
  'median_cr_bps' : number,
  'total_debt_e8s' : bigint,
  'total_vault_count' : number,
  'total_collateral_usd_e8s' : bigint,
  'collaterals' : Array<CollateralStats>,
}
export interface ErrorCounters {
  'amm' : bigint,
  'three_pool' : bigint,
  'icusd_ledger' : bigint,
  'stability_pool' : bigint,
  'backend' : bigint,
}
export interface Fast3PoolSnapshot {
  'virtual_price' : bigint,
  'decimals' : [] | [Uint8Array | number[]],
  'timestamp_ns' : bigint,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
}
export interface FastPriceSnapshot {
  'timestamp_ns' : bigint,
  'prices' : Array<[Principal, number, string]>,
}
export interface FeeBreakdownQuery { 'window_ns' : [] | [bigint] }
export interface FeeBreakdownResponse {
  'redemption_count' : number,
  'borrow_count' : number,
  'redemption_fees_icusd_e8s' : bigint,
  'borrow_fees_icusd_e8s' : bigint,
  'start_ns' : bigint,
  'swap_count' : number,
  'swap_fees_icusd_e8s' : bigint,
  'end_ns' : bigint,
}
export interface FeeCurveSeriesResponse {
  'rows' : Array<HourlyFeeCurveSnapshot>,
  'next_from_ts' : [] | [bigint],
}
export interface FeeSeriesResponse {
  'rows' : Array<DailyFeeRollup>,
  'next_from_ts' : [] | [bigint],
}
export interface HolderSeriesResponse {
  'rows' : Array<DailyHolderRow>,
  'next_from_ts' : [] | [bigint],
}
export interface HourlyCycleSnapshot {
  'timestamp_ns' : bigint,
  'cycle_balance' : bigint,
}
export interface HourlyFeeCurveSnapshot {
  'collateral_stats' : Array<[Principal, bigint, bigint, number]>,
  'timestamp_ns' : bigint,
  'system_cr_bps' : number,
}
export interface HttpRequest {
  'url' : string,
  'method' : string,
  'body' : Uint8Array | number[],
  'headers' : Array<[string, string]>,
}
export interface HttpResponse {
  'body' : Uint8Array | number[],
  'headers' : Array<[string, string]>,
  'status_code' : number,
}
export interface InitArgs {
  'amm' : Principal,
  'three_pool' : Principal,
  'admin' : Principal,
  'icusd_ledger' : Principal,
  'stability_pool' : Principal,
  'backend' : Principal,
}
export interface LiquidationSeriesResponse {
  'rows' : Array<DailyLiquidationRollup>,
  'next_from_ts' : [] | [bigint],
}
export type LiquidityAction = { 'Add' : null } |
  { 'Remove' : null } |
  { 'Donate' : null } |
  { 'RemoveOneCoin' : null };
export interface OhlcCandle {
  'low' : number,
  'timestamp_ns' : bigint,
  'high' : number,
  'close' : number,
  'open' : number,
}
export interface OhlcQuery {
  'to_ts' : [] | [bigint],
  'collateral' : Principal,
  'from_ts' : [] | [bigint],
  'limit' : [] | [number],
  'bucket_secs' : [] | [bigint],
}
export interface OhlcResponse {
  'collateral' : Principal,
  'candles' : Array<OhlcCandle>,
  'bucket_secs' : bigint,
  'symbol' : string,
}
export interface PegStatus {
  'virtual_price' : bigint,
  'timestamp_ns' : bigint,
  'pool_balances' : Array<bigint>,
  'balance_ratios' : Array<number>,
  'max_imbalance_pct' : number,
}
export interface PoolRoute {
  'swap_count' : bigint,
  'volume_usd_e8s' : bigint,
  'avg_hop_count' : number,
  'route' : Array<Principal>,
}
export interface PoolRoutesQuery {
  'limit' : [] | [number],
  'window_ns' : [] | [bigint],
  'pool_id' : string,
}
export interface PoolRoutesResponse {
  'generated_at_ns' : bigint,
  'window_ns' : bigint,
  'pool_id' : string,
  'routes' : Array<PoolRoute>,
}
export interface PriceSeriesResponse {
  'rows' : Array<FastPriceSnapshot>,
  'next_from_ts' : [] | [bigint],
}
export interface ProtocolSummary {
  'peg' : [] | [PegStatus],
  'lp_apy_pct' : [] | [number],
  'timestamp_ns' : bigint,
  'amm_apy_pct' : [] | [number],
  'median_cr_bps' : number,
  'sp_apy_pct' : [] | [number],
  'total_debt_e8s' : bigint,
  'circulating_supply_icusd_e8s' : [] | [bigint],
  'prices' : Array<TwapEntry>,
  'total_vault_count' : number,
  'total_collateral_usd_e8s' : bigint,
  'system_cr_bps' : number,
  'swap_count_24h' : number,
  'volume_24h_e8s' : bigint,
}
export interface PullScheduleConfig {
  'period_secs_override' : [] | [bigint],
  'tick_secs_override' : [] | [bigint],
  'period_secs' : bigint,
  'schedule_layout_version' : number,
  'tick_secs' : bigint,
}
export interface RangeQuery {
  'to_ts' : [] | [bigint],
  'from_ts' : [] | [bigint],
  'offset' : [] | [bigint],
  'limit' : [] | [number],
}
export interface ResetErrorCountersArgs { 'sources' : [] | [Array<string>] }
export type Result = { 'Ok' : BackfillProgress } |
  { 'Err' : string };
export type Result_1 = { 'Ok' : null } |
  { 'Err' : string };
export interface StabilitySeriesResponse {
  'rows' : Array<DailyStabilityRow>,
  'next_from_ts' : [] | [bigint],
}
export interface SwapSeriesResponse {
  'rows' : Array<DailySwapRollup>,
  'next_from_ts' : [] | [bigint],
}
export type SwapSource = { 'Amm' : null } |
  { 'ThreePool' : null };
export interface ThreePoolSeriesResponse {
  'rows' : Array<Fast3PoolSnapshot>,
  'next_from_ts' : [] | [bigint],
}
export interface TokenFlowEdge {
  'to_token' : Principal,
  'from_token' : Principal,
  'swap_count' : bigint,
  'volume_usd_e8s' : bigint,
}
export interface TokenFlowQuery {
  'min_volume_usd_e8s' : [] | [bigint],
  'limit' : [] | [number],
  'window_ns' : [] | [bigint],
}
export interface TokenFlowResponse {
  'edges' : Array<TokenFlowEdge>,
  'generated_at_ns' : bigint,
  'window_ns' : bigint,
}
export interface TopCounterpartiesQuery {
  'principal' : Principal,
  'limit' : [] | [number],
  'window_ns' : [] | [bigint],
}
export interface TopCounterpartiesResponse {
  'principal' : Principal,
  'rows' : Array<TopCounterpartyRow>,
  'generated_at_ns' : bigint,
  'window_ns' : bigint,
}
export interface TopCounterpartyRow {
  'interaction_count' : bigint,
  'volume_e8s' : bigint,
  'counterparty' : Principal,
}
export interface TopHolderRow {
  'principal' : Principal,
  'balance_e8s' : bigint,
  'share_bps' : number,
}
export interface TopHoldersQuery {
  'token' : Principal,
  'limit' : [] | [number],
}
export interface TopHoldersResponse {
  'token' : Principal,
  'total_supply_e8s' : bigint,
  'source' : string,
  'rows' : Array<TopHolderRow>,
  'total_holders' : number,
  'generated_at_ns' : bigint,
}
export interface TopSpDepositorRow {
  'principal' : Principal,
  'total_deposited_e8s' : bigint,
  'current_balance_e8s' : bigint,
  'net_position_e8s' : bigint,
}
export interface TopSpDepositorsQuery {
  'limit' : [] | [number],
  'window_ns' : [] | [bigint],
}
export interface TopSpDepositorsResponse {
  'rows' : Array<TopSpDepositorRow>,
  'generated_at_ns' : bigint,
  'window_ns' : bigint,
}
export interface TradeActivityQuery { 'window_secs' : [] | [bigint] }
export interface TradeActivityResponse {
  'total_swaps' : number,
  'three_pool_swaps' : number,
  'total_volume_e8s' : bigint,
  'avg_trade_size_e8s' : bigint,
  'window_secs' : bigint,
  'amm_swaps' : number,
  'total_fees_e8s' : bigint,
  'unique_traders' : number,
}
export interface TvlSeriesResponse {
  'rows' : Array<DailyTvlRow>,
  'next_from_ts' : [] | [bigint],
}
export interface TwapEntry {
  'latest_price' : number,
  'collateral' : Principal,
  'sample_count' : number,
  'twap_price' : number,
  'symbol' : string,
}
export interface TwapQuery { 'window_secs' : [] | [bigint] }
export interface TwapResponse {
  'window_secs' : bigint,
  'entries' : Array<TwapEntry>,
}
export interface VaultSeriesResponse {
  'rows' : Array<DailyVaultSnapshotRow>,
  'next_from_ts' : [] | [bigint],
}
export interface VolatilityQuery {
  'collateral' : Principal,
  'window_secs' : [] | [bigint],
}
export interface VolatilityResponse {
  'collateral' : Principal,
  'sample_count' : number,
  'window_secs' : bigint,
  'symbol' : string,
  'annualized_vol_pct' : number,
}
export interface _SERVICE {
  'admin_backfill_add_margin_events' : ActorMethod<[bigint], Result>,
  'cycle_manager_metrics' : ActorMethod<[], Array<CycleManagerMetric>>,
  'cycle_manager_targets' : ActorMethod<[], Array<CycleManagerTarget>>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'debug_amm_pool_snapshot' : ActorMethod<[], Array<AmmPoolSnapshot>>,
  'debug_get_amm_event_counts' : ActorMethod<[], [bigint, bigint]>,
  'debug_get_amm_liquidity_events_raw' : ActorMethod<
    [bigint, bigint],
    Array<AnalyticsAmmLiquidityEvent>
  >,
  'debug_get_swap_events_raw' : ActorMethod<
    [bigint, bigint],
    Array<AnalyticsSwapEvent>
  >,
  'get_address_value_series' : ActorMethod<
    [AddressValueSeriesQuery],
    AddressValueSeriesResponse
  >,
  'get_admin' : ActorMethod<[], Principal>,
  'get_admin_event_breakdown' : ActorMethod<
    [AdminEventBreakdownQuery],
    AdminEventBreakdownResponse
  >,
  'get_apys' : ActorMethod<[ApyQuery], ApyResponse>,
  'get_collector_health' : ActorMethod<[], CollectorHealth>,
  'get_cycle_series' : ActorMethod<[RangeQuery], CycleSeriesResponse>,
  'get_fee_breakdown_window' : ActorMethod<
    [FeeBreakdownQuery],
    FeeBreakdownResponse
  >,
  'get_fee_curve_series' : ActorMethod<[RangeQuery], FeeCurveSeriesResponse>,
  'get_fee_series' : ActorMethod<[RangeQuery], FeeSeriesResponse>,
  'get_holder_series' : ActorMethod<
    [RangeQuery, Principal],
    HolderSeriesResponse
  >,
  'get_liquidation_series' : ActorMethod<
    [RangeQuery],
    LiquidationSeriesResponse
  >,
  'get_ohlc' : ActorMethod<[OhlcQuery], OhlcResponse>,
  'get_peg_status' : ActorMethod<[], [] | [PegStatus]>,
  'get_pool_routes' : ActorMethod<[PoolRoutesQuery], PoolRoutesResponse>,
  'get_price_series' : ActorMethod<[RangeQuery], PriceSeriesResponse>,
  'get_protocol_summary' : ActorMethod<[], ProtocolSummary>,
  'get_pull_schedule' : ActorMethod<[], PullScheduleConfig>,
  'get_sp_depositor_principals' : ActorMethod<[], Array<Principal>>,
  'get_stability_series' : ActorMethod<[RangeQuery], StabilitySeriesResponse>,
  'get_swap_series' : ActorMethod<[RangeQuery], SwapSeriesResponse>,
  'get_three_pool_series' : ActorMethod<[RangeQuery], ThreePoolSeriesResponse>,
  'get_token_flow' : ActorMethod<[TokenFlowQuery], TokenFlowResponse>,
  'get_top_counterparties' : ActorMethod<
    [TopCounterpartiesQuery],
    TopCounterpartiesResponse
  >,
  'get_top_holders' : ActorMethod<[TopHoldersQuery], TopHoldersResponse>,
  'get_top_sp_depositors' : ActorMethod<
    [TopSpDepositorsQuery],
    TopSpDepositorsResponse
  >,
  'get_trade_activity' : ActorMethod<
    [TradeActivityQuery],
    TradeActivityResponse
  >,
  'get_tvl_series' : ActorMethod<[RangeQuery], TvlSeriesResponse>,
  'get_twap' : ActorMethod<[TwapQuery], TwapResponse>,
  'get_vault_series' : ActorMethod<[RangeQuery], VaultSeriesResponse>,
  'get_volatility' : ActorMethod<[VolatilityQuery], VolatilityResponse>,
  'http_request' : ActorMethod<[HttpRequest], HttpResponse>,
  'ping' : ActorMethod<[], string>,
  'reset_error_counters' : ActorMethod<[ResetErrorCountersArgs], Result_1>,
  'set_pull_schedule' : ActorMethod<[bigint, bigint], Result_1>,
  'start_backfill' : ActorMethod<[Principal], string>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
