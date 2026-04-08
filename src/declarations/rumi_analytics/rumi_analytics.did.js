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
  const DailyTvlRow = IDL.Record({
    'timestamp_ns' : IDL.Nat64,
    'total_icusd_supply_e8s' : IDL.Nat,
    'system_collateral_ratio_bps' : IDL.Nat32,
    'total_icp_collateral_e8s' : IDL.Nat,
  });
  const TvlSeriesResponse = IDL.Record({
    'rows' : IDL.Vec(DailyTvlRow),
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
    'get_tvl_series' : IDL.Func([RangeQuery], [TvlSeriesResponse], ['query']),
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
