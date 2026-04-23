import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

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
export interface ApyQuery { 'window_days' : [] | [number] }
export interface ApyResponse {
  'lp_apy_pct' : [] | [number],
  'window_days' : number,
  'sp_apy_pct' : [] | [number],
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
  'total_icusd_supply_e8s' : bigint,
  'system_collateral_ratio_bps' : number,
  'total_icp_collateral_e8s' : bigint,
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
  'decimals' : Uint8Array | number[],
  'timestamp_ns' : bigint,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
}
export interface FastPriceSnapshot {
  'timestamp_ns' : bigint,
  'prices' : Array<[Principal, number, string]>,
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
export interface PriceSeriesResponse {
  'rows' : Array<FastPriceSnapshot>,
  'next_from_ts' : [] | [bigint],
}
export interface ProtocolSummary {
  'peg' : [] | [PegStatus],
  'lp_apy_pct' : [] | [number],
  'timestamp_ns' : bigint,
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
export interface RangeQuery {
  'to_ts' : [] | [bigint],
  'from_ts' : [] | [bigint],
  'offset' : [] | [bigint],
  'limit' : [] | [number],
}
export interface StabilitySeriesResponse {
  'rows' : Array<DailyStabilityRow>,
  'next_from_ts' : [] | [bigint],
}
export interface SwapSeriesResponse {
  'rows' : Array<DailySwapRollup>,
  'next_from_ts' : [] | [bigint],
}
export interface ThreePoolSeriesResponse {
  'rows' : Array<Fast3PoolSnapshot>,
  'next_from_ts' : [] | [bigint],
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
  'get_admin' : ActorMethod<[], Principal>,
  'get_admin_event_breakdown' : ActorMethod<
    [AdminEventBreakdownQuery],
    AdminEventBreakdownResponse
  >,
  'get_apys' : ActorMethod<[ApyQuery], ApyResponse>,
  'get_collector_health' : ActorMethod<[], CollectorHealth>,
  'get_cycle_series' : ActorMethod<[RangeQuery], CycleSeriesResponse>,
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
  'get_price_series' : ActorMethod<[RangeQuery], PriceSeriesResponse>,
  'get_protocol_summary' : ActorMethod<[], ProtocolSummary>,
  'get_stability_series' : ActorMethod<[RangeQuery], StabilitySeriesResponse>,
  'get_swap_series' : ActorMethod<[RangeQuery], SwapSeriesResponse>,
  'get_three_pool_series' : ActorMethod<[RangeQuery], ThreePoolSeriesResponse>,
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
  'start_backfill' : ActorMethod<[Principal], string>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
