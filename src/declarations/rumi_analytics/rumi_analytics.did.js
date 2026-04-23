export const idlFactory = ({ IDL }) => {
  const InitArgs = IDL.Record({
    'amm' : IDL.Principal,
    'three_pool' : IDL.Principal,
    'admin' : IDL.Principal,
    'icusd_ledger' : IDL.Principal,
    'stability_pool' : IDL.Principal,
    'backend' : IDL.Principal,
  });
  const ApyQuery = IDL.Record({ 'window_days' : IDL.Opt(IDL.Nat32) });
  const ApyResponse = IDL.Record({
    'lp_apy_pct' : IDL.Opt(IDL.Float64),
    'window_days' : IDL.Nat32,
    'sp_apy_pct' : IDL.Opt(IDL.Float64),
  });
  const BalanceTrackerStats = IDL.Record({
    'token' : IDL.Principal,
    'total_tracked_e8s' : IDL.Nat64,
    'holder_count' : IDL.Nat64,
  });
  const ErrorCounters = IDL.Record({
    'amm' : IDL.Nat64,
    'three_pool' : IDL.Nat64,
    'icusd_ledger' : IDL.Nat64,
    'stability_pool' : IDL.Nat64,
    'backend' : IDL.Nat64,
  });
  const CursorStatus = IDL.Record({
    'last_error' : IDL.Opt(IDL.Text),
    'source_count' : IDL.Nat64,
    'name' : IDL.Text,
    'last_success_ns' : IDL.Nat64,
    'cursor_position' : IDL.Nat64,
  });
  const CollectorHealth = IDL.Record({
    'balance_tracker_stats' : IDL.Vec(BalanceTrackerStats),
    'backfill_active' : IDL.Vec(IDL.Principal),
    'error_counters' : ErrorCounters,
    'last_pull_cycle_ns' : IDL.Nat64,
    'cursors' : IDL.Vec(CursorStatus),
  });
  const RangeQuery = IDL.Record({
    'to_ts' : IDL.Opt(IDL.Nat64),
    'from_ts' : IDL.Opt(IDL.Nat64),
    'offset' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Opt(IDL.Nat32),
  });
  const HourlyCycleSnapshot = IDL.Record({
    'timestamp_ns' : IDL.Nat64,
    'cycle_balance' : IDL.Nat,
  });
  const CycleSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(HourlyCycleSnapshot),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const HourlyFeeCurveSnapshot = IDL.Record({
    'collateral_stats' : IDL.Vec(
      IDL.Tuple(IDL.Principal, IDL.Nat64, IDL.Nat64, IDL.Float64)
    ),
    'timestamp_ns' : IDL.Nat64,
    'system_cr_bps' : IDL.Nat32,
  });
  const FeeCurveSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(HourlyFeeCurveSnapshot),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const DailyFeeRollup = IDL.Record({
    'redemption_count' : IDL.Nat32,
    'borrow_count' : IDL.Nat32,
    'timestamp_ns' : IDL.Nat64,
    'swap_fees_e8s' : IDL.Nat64,
    'redemption_fees_e8s' : IDL.Opt(IDL.Nat64),
    'borrowing_fees_e8s' : IDL.Opt(IDL.Nat64),
  });
  const FeeSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyFeeRollup),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const DailyHolderRow = IDL.Record({
    'total_supply_tracked_e8s' : IDL.Nat64,
    'token' : IDL.Principal,
    'new_holders_today' : IDL.Nat32,
    'timestamp_ns' : IDL.Nat64,
    'median_balance_e8s' : IDL.Nat64,
    'total_holders' : IDL.Nat32,
    'top_10_pct_bps' : IDL.Nat32,
    'top_50' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'distribution_buckets' : IDL.Vec(IDL.Nat32),
    'gini_bps' : IDL.Nat32,
  });
  const HolderSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyHolderRow),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const DailyLiquidationRollup = IDL.Record({
    'total_debt_covered_e8s' : IDL.Nat64,
    'timestamp_ns' : IDL.Nat64,
    'total_collateral_seized_e8s' : IDL.Nat64,
    'redistribution_count' : IDL.Nat32,
    'by_collateral' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'partial_count' : IDL.Nat32,
    'full_count' : IDL.Nat32,
  });
  const LiquidationSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyLiquidationRollup),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const OhlcQuery = IDL.Record({
    'to_ts' : IDL.Opt(IDL.Nat64),
    'collateral' : IDL.Principal,
    'from_ts' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Opt(IDL.Nat32),
    'bucket_secs' : IDL.Opt(IDL.Nat64),
  });
  const OhlcCandle = IDL.Record({
    'low' : IDL.Float64,
    'timestamp_ns' : IDL.Nat64,
    'high' : IDL.Float64,
    'close' : IDL.Float64,
    'open' : IDL.Float64,
  });
  const OhlcResponse = IDL.Record({
    'collateral' : IDL.Principal,
    'candles' : IDL.Vec(OhlcCandle),
    'bucket_secs' : IDL.Nat64,
    'symbol' : IDL.Text,
  });
  const PegStatus = IDL.Record({
    'virtual_price' : IDL.Nat,
    'timestamp_ns' : IDL.Nat64,
    'pool_balances' : IDL.Vec(IDL.Nat),
    'balance_ratios' : IDL.Vec(IDL.Float64),
    'max_imbalance_pct' : IDL.Float64,
  });
  const FastPriceSnapshot = IDL.Record({
    'timestamp_ns' : IDL.Nat64,
    'prices' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Float64, IDL.Text)),
  });
  const PriceSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(FastPriceSnapshot),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const TwapEntry = IDL.Record({
    'latest_price' : IDL.Float64,
    'collateral' : IDL.Principal,
    'sample_count' : IDL.Nat32,
    'twap_price' : IDL.Float64,
    'symbol' : IDL.Text,
  });
  const ProtocolSummary = IDL.Record({
    'peg' : IDL.Opt(PegStatus),
    'lp_apy_pct' : IDL.Opt(IDL.Float64),
    'timestamp_ns' : IDL.Nat64,
    'sp_apy_pct' : IDL.Opt(IDL.Float64),
    'total_debt_e8s' : IDL.Nat64,
    'circulating_supply_icusd_e8s' : IDL.Opt(IDL.Nat),
    'prices' : IDL.Vec(TwapEntry),
    'total_vault_count' : IDL.Nat32,
    'total_collateral_usd_e8s' : IDL.Nat64,
    'system_cr_bps' : IDL.Nat32,
    'swap_count_24h' : IDL.Nat32,
    'volume_24h_e8s' : IDL.Nat64,
  });
  const DailyStabilityRow = IDL.Record({
    'collateral_gains' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'timestamp_ns' : IDL.Nat64,
    'total_depositors' : IDL.Nat64,
    'stablecoin_balances' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'total_deposits_e8s' : IDL.Nat64,
    'total_interest_received_e8s' : IDL.Nat64,
    'total_liquidations_executed' : IDL.Nat64,
  });
  const StabilitySeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyStabilityRow),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const DailySwapRollup = IDL.Record({
    'three_pool_fees_e8s' : IDL.Nat64,
    'timestamp_ns' : IDL.Nat64,
    'three_pool_swap_count' : IDL.Nat32,
    'amm_volume_e8s' : IDL.Nat64,
    'three_pool_volume_e8s' : IDL.Nat64,
    'amm_swap_count' : IDL.Nat32,
    'amm_fees_e8s' : IDL.Nat64,
    'unique_swappers' : IDL.Nat32,
  });
  const SwapSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailySwapRollup),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const Fast3PoolSnapshot = IDL.Record({
    'virtual_price' : IDL.Nat,
    'decimals' : IDL.Vec(IDL.Nat8),
    'timestamp_ns' : IDL.Nat64,
    'lp_total_supply' : IDL.Nat,
    'balances' : IDL.Vec(IDL.Nat),
  });
  const ThreePoolSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(Fast3PoolSnapshot),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const TopHoldersQuery = IDL.Record({
    'token' : IDL.Principal,
    'limit' : IDL.Opt(IDL.Nat32),
  });
  const TopHolderRow = IDL.Record({
    'principal' : IDL.Principal,
    'balance_e8s' : IDL.Nat64,
    'share_bps' : IDL.Nat32,
  });
  const TopHoldersResponse = IDL.Record({
    'token' : IDL.Principal,
    'total_supply_e8s' : IDL.Nat64,
    'source' : IDL.Text,
    'rows' : IDL.Vec(TopHolderRow),
    'total_holders' : IDL.Nat32,
    'generated_at_ns' : IDL.Nat64,
  });
  const TradeActivityQuery = IDL.Record({ 'window_secs' : IDL.Opt(IDL.Nat64) });
  const TradeActivityResponse = IDL.Record({
    'total_swaps' : IDL.Nat32,
    'three_pool_swaps' : IDL.Nat32,
    'total_volume_e8s' : IDL.Nat64,
    'avg_trade_size_e8s' : IDL.Nat64,
    'window_secs' : IDL.Nat64,
    'amm_swaps' : IDL.Nat32,
    'total_fees_e8s' : IDL.Nat64,
    'unique_traders' : IDL.Nat32,
  });
  const DailyTvlRow = IDL.Record({
    'three_pool_reserve_0_e8s' : IDL.Opt(IDL.Nat),
    'timestamp_ns' : IDL.Nat64,
    'three_pool_reserve_2_e8s' : IDL.Opt(IDL.Nat),
    'three_pool_virtual_price_e18' : IDL.Opt(IDL.Nat),
    'total_icusd_supply_e8s' : IDL.Nat,
    'system_collateral_ratio_bps' : IDL.Nat32,
    'total_icp_collateral_e8s' : IDL.Nat,
    'three_pool_reserve_1_e8s' : IDL.Opt(IDL.Nat),
    'stability_pool_deposits_e8s' : IDL.Opt(IDL.Nat64),
    'three_pool_lp_supply_e8s' : IDL.Opt(IDL.Nat),
  });
  const TvlSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyTvlRow),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const TwapQuery = IDL.Record({ 'window_secs' : IDL.Opt(IDL.Nat64) });
  const TwapResponse = IDL.Record({
    'window_secs' : IDL.Nat64,
    'entries' : IDL.Vec(TwapEntry),
  });
  const CollateralStats = IDL.Record({
    'total_collateral_e8s' : IDL.Nat64,
    'median_cr_bps' : IDL.Nat32,
    'price_usd_e8s' : IDL.Nat64,
    'total_debt_e8s' : IDL.Nat64,
    'max_cr_bps' : IDL.Nat32,
    'collateral_type' : IDL.Principal,
    'min_cr_bps' : IDL.Nat32,
    'vault_count' : IDL.Nat32,
  });
  const DailyVaultSnapshotRow = IDL.Record({
    'timestamp_ns' : IDL.Nat64,
    'median_cr_bps' : IDL.Nat32,
    'total_debt_e8s' : IDL.Nat64,
    'total_vault_count' : IDL.Nat32,
    'total_collateral_usd_e8s' : IDL.Nat64,
    'collaterals' : IDL.Vec(CollateralStats),
  });
  const VaultSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyVaultSnapshotRow),
    'next_from_ts' : IDL.Opt(IDL.Nat64),
  });
  const VolatilityQuery = IDL.Record({
    'collateral' : IDL.Principal,
    'window_secs' : IDL.Opt(IDL.Nat64),
  });
  const VolatilityResponse = IDL.Record({
    'collateral' : IDL.Principal,
    'sample_count' : IDL.Nat32,
    'window_secs' : IDL.Nat64,
    'symbol' : IDL.Text,
    'annualized_vol_pct' : IDL.Float64,
  });
  const HttpRequest = IDL.Record({
    'url' : IDL.Text,
    'method' : IDL.Text,
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(IDL.Tuple(IDL.Text, IDL.Text)),
  });
  const HttpResponse = IDL.Record({
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(IDL.Tuple(IDL.Text, IDL.Text)),
    'status_code' : IDL.Nat16,
  });
  return IDL.Service({
    'get_admin' : IDL.Func([], [IDL.Principal], ['query']),
    'get_apys' : IDL.Func([ApyQuery], [ApyResponse], ['query']),
    'get_collector_health' : IDL.Func([], [CollectorHealth], ['query']),
    'get_cycle_series' : IDL.Func(
        [RangeQuery],
        [CycleSeriesResponse],
        ['query'],
      ),
    'get_fee_curve_series' : IDL.Func(
        [RangeQuery],
        [FeeCurveSeriesResponse],
        ['query'],
      ),
    'get_fee_series' : IDL.Func([RangeQuery], [FeeSeriesResponse], ['query']),
    'get_holder_series' : IDL.Func(
        [RangeQuery, IDL.Principal],
        [HolderSeriesResponse],
        ['query'],
      ),
    'get_liquidation_series' : IDL.Func(
        [RangeQuery],
        [LiquidationSeriesResponse],
        ['query'],
      ),
    'get_ohlc' : IDL.Func([OhlcQuery], [OhlcResponse], ['query']),
    'get_peg_status' : IDL.Func([], [IDL.Opt(PegStatus)], ['query']),
    'get_price_series' : IDL.Func(
        [RangeQuery],
        [PriceSeriesResponse],
        ['query'],
      ),
    'get_protocol_summary' : IDL.Func([], [ProtocolSummary], ['query']),
    'get_stability_series' : IDL.Func(
        [RangeQuery],
        [StabilitySeriesResponse],
        ['query'],
      ),
    'get_swap_series' : IDL.Func([RangeQuery], [SwapSeriesResponse], ['query']),
    'get_three_pool_series' : IDL.Func(
        [RangeQuery],
        [ThreePoolSeriesResponse],
        ['query'],
      ),
    'get_top_holders' : IDL.Func(
        [TopHoldersQuery],
        [TopHoldersResponse],
        ['query'],
      ),
    'get_trade_activity' : IDL.Func(
        [TradeActivityQuery],
        [TradeActivityResponse],
        ['query'],
      ),
    'get_tvl_series' : IDL.Func([RangeQuery], [TvlSeriesResponse], ['query']),
    'get_twap' : IDL.Func([TwapQuery], [TwapResponse], ['query']),
    'get_vault_series' : IDL.Func(
        [RangeQuery],
        [VaultSeriesResponse],
        ['query'],
      ),
    'get_volatility' : IDL.Func(
        [VolatilityQuery],
        [VolatilityResponse],
        ['query'],
      ),
    'http_request' : IDL.Func([HttpRequest], [HttpResponse], ['query']),
    'ping' : IDL.Func([], [IDL.Text], ['query']),
    'start_backfill' : IDL.Func([IDL.Principal], [IDL.Text], []),
  });
};
export const init = ({ IDL }) => {
  const InitArgs = IDL.Record({
    'amm' : IDL.Principal,
    'three_pool' : IDL.Principal,
    'admin' : IDL.Principal,
    'icusd_ledger' : IDL.Principal,
    'stability_pool' : IDL.Principal,
    'backend' : IDL.Principal,
  });
  return [InitArgs];
};
