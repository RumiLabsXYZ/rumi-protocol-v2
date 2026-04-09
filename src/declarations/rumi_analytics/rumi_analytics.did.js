export const idlFactory = ({ IDL }) => {
  const InitArgs = IDL.Record({
    'amm' : IDL.Principal,
    'three_pool' : IDL.Principal,
    'admin' : IDL.Principal,
    'icusd_ledger' : IDL.Principal,
    'stability_pool' : IDL.Principal,
    'backend' : IDL.Principal,
  });
  const RangeQuery = IDL.Record({
    'to_ts' : IDL.Opt(IDL.Nat64),
    'from_ts' : IDL.Opt(IDL.Nat64),
    'offset' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Opt(IDL.Nat32),
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
  const HeaderField = IDL.Tuple(IDL.Text, IDL.Text);
  const HttpRequest = IDL.Record({
    'url' : IDL.Text,
    'method' : IDL.Text,
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(HeaderField),
  });
  const HttpResponse = IDL.Record({
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(HeaderField),
    'status_code' : IDL.Nat16,
  });
  return IDL.Service({
    'get_admin' : IDL.Func([], [IDL.Principal], ['query']),
    'get_stability_series' : IDL.Func(
        [RangeQuery],
        [StabilitySeriesResponse],
        ['query'],
      ),
    'get_tvl_series' : IDL.Func([RangeQuery], [TvlSeriesResponse], ['query']),
    'get_vault_series' : IDL.Func(
        [RangeQuery],
        [VaultSeriesResponse],
        ['query'],
      ),
    'http_request' : IDL.Func([HttpRequest], [HttpResponse], ['query']),
    'ping' : IDL.Func([], [IDL.Text], ['query']),
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
