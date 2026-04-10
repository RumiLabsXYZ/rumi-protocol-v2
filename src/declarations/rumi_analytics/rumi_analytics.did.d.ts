import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

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
export interface DailyStabilityRow {
  'collateral_gains' : Array<[Principal, bigint]>,
  'timestamp_ns' : bigint,
  'total_depositors' : bigint,
  'stablecoin_balances' : Array<[Principal, bigint]>,
  'total_deposits_e8s' : bigint,
  'total_interest_received_e8s' : bigint,
  'total_liquidations_executed' : bigint,
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
export interface HolderSeriesResponse {
  'rows' : Array<DailyHolderRow>,
  'next_from_ts' : [] | [bigint],
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
export interface TvlSeriesResponse {
  'rows' : Array<DailyTvlRow>,
  'next_from_ts' : [] | [bigint],
}
export interface VaultSeriesResponse {
  'rows' : Array<DailyVaultSnapshotRow>,
  'next_from_ts' : [] | [bigint],
}
export interface _SERVICE {
  'get_admin' : ActorMethod<[], Principal>,
  'get_collector_health' : ActorMethod<[], CollectorHealth>,
  'get_holder_series' : ActorMethod<
    [RangeQuery, Principal],
    HolderSeriesResponse
  >,
  'get_stability_series' : ActorMethod<[RangeQuery], StabilitySeriesResponse>,
  'get_tvl_series' : ActorMethod<[RangeQuery], TvlSeriesResponse>,
  'get_vault_series' : ActorMethod<[RangeQuery], VaultSeriesResponse>,
  'http_request' : ActorMethod<[HttpRequest], HttpResponse>,
  'ping' : ActorMethod<[], string>,
  'start_backfill' : ActorMethod<[Principal], string>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
