export const idlFactory = ({ IDL }) => {
  const InitArgs = IDL.Record({
    'admin' : IDL.Opt(IDL.Principal),
    'excluded_principals' : IDL.Opt(IDL.Vec(IDL.Principal)),
    'snapshot_seed_commit' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'season_start_ns' : IDL.Opt(IDL.Nat64),
    'season_end_ns' : IDL.Opt(IDL.Nat64),
  });
  const PointsError = IDL.Variant({
    'Unauthorized' : IDL.Null,
    'Excluded' : IDL.Null,
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : PointsError });
  const EpochSummary = IDL.Record({
    'epoch_index' : IDL.Nat64,
    'points_accrued_this_epoch' : IDL.Nat,
    'epoch_start_ns' : IDL.Nat64,
    'registered_principals' : IDL.Nat64,
    'active_principals' : IDL.Nat64,
    'total_points_all' : IDL.Nat,
    'snapshot_a_ns' : IDL.Nat64,
    'snapshot_b_ns' : IDL.Nat64,
    'epoch_end_ns' : IDL.Nat64,
  });
  const PublicOpenEpoch = IDL.Record({
    'epoch_index' : IDL.Nat64,
    'epoch_start_ns' : IDL.Nat64,
    'snapshot_a_ns' : IDL.Opt(IDL.Nat64),
    'snapshot_b_ns' : IDL.Opt(IDL.Nat64),
    'epoch_end_ns' : IDL.Nat64,
  });
  const PublicEpochStatus = IDL.Record({
    'open_epoch' : IDL.Opt(PublicOpenEpoch),
    'snapshot_seed_committed' : IDL.Bool,
    'driver_interval_secs' : IDL.Nat64,
    'revealed_seed_count' : IDL.Nat64,
    'current_epoch_index' : IDL.Nat64,
    'driver_enabled' : IDL.Bool,
  });
  const OpenEpoch = IDL.Record({
    'close_active' : IDL.Nat64,
    'close_cursor' : IDL.Opt(IDL.Principal),
    'epoch_index' : IDL.Nat64,
    'epoch_start_ns' : IDL.Nat64,
    'a_cursor' : IDL.Opt(IDL.Principal),
    'b_complete' : IDL.Bool,
    'b_cursor' : IDL.Opt(IDL.Principal),
    'close_started' : IDL.Bool,
    'a_complete' : IDL.Bool,
    'snapshot_a_ns' : IDL.Nat64,
    'snapshot_b_ns' : IDL.Nat64,
    'close_points_accrued' : IDL.Nat,
    'epoch_end_ns' : IDL.Nat64,
  });
  const EpochStatus = IDL.Record({
    'open_epoch' : IDL.Opt(OpenEpoch),
    'snapshot_seed_committed' : IDL.Bool,
    'driver_interval_secs' : IDL.Nat64,
    'revealed_seed_count' : IDL.Nat64,
    'current_epoch_index' : IDL.Nat64,
    'driver_enabled' : IDL.Bool,
  });
  const SourceStatus = IDL.Record({
    'tag' : IDL.Nat8,
    'cursor' : IDL.Nat64,
    'canister' : IDL.Principal,
  });
  const IngestStatus = IDL.Record({
    'registered_count' : IDL.Nat64,
    'poll_interval_secs' : IDL.Nat64,
    'poll_enabled' : IDL.Bool,
    'sources' : IDL.Vec(SourceStatus),
  });
  const LeaderboardEntry = IDL.Record({
    'principal' : IDL.Principal,
    'total_points' : IDL.Nat,
    'rank' : IDL.Nat32,
    'estimated_share_bps' : IDL.Nat32,
  });
  const PointsConfig = IDL.Record({
    'admin' : IDL.Principal,
    'registered_count' : IDL.Nat64,
    'snapshot_seed_committed' : IDL.Bool,
    'excluded_count' : IDL.Nat32,
    'season_start_ns' : IDL.Nat64,
    'season_end_ns' : IDL.Nat64,
    'current_epoch_index' : IDL.Nat64,
  });
  const AssetType = IDL.Variant({
    'Icp' : IDL.Null,
    'IcUsd' : IDL.Null,
    'CkUsdc' : IDL.Null,
    'CkUsdt' : IDL.Null,
    'ThreeUsd' : IDL.Null,
  });
  const RepaymentEvent = IDL.Record({
    'asset' : AssetType,
    'repaid_at' : IDL.Nat64,
    'amount_usd' : IDL.Nat,
    'window_end' : IDL.Nat64,
  });
  const QualifyingAction = IDL.Variant({
    'ProvideAmmLiquidity' : IDL.Null,
    'MintIcUsd' : IDL.Null,
    'Deposit3Pool' : IDL.Null,
    'DepositStabilityPool' : IDL.Null,
    'RepayVault' : IDL.Null,
  });
  const Venue = IDL.Variant({
    'Amm' : IDL.Null,
    'ThreePool' : IDL.Null,
    'Vault' : IDL.Null,
    'StabilityPool' : IDL.Null,
  });
  const DepositKey = IDL.Record({ 'asset' : AssetType, 'venue' : Venue });
  const DepositRecord = IDL.Record({
    'asset' : AssetType,
    'venue' : Venue,
    'last_verified_at' : IDL.Nat64,
    'deposited_at' : IDL.Nat64,
    'recorded_value_usd' : IDL.Nat,
  });
  const PrincipalState = IDL.Record({
    'principal' : IDL.Principal,
    'registered_at_ns' : IDL.Nat64,
    'total_points' : IDL.Nat,
    'repayment_events' : IDL.Vec(RepaymentEvent),
    'first_qualifying_action' : QualifyingAction,
    'active_deposits' : IDL.Vec(IDL.Tuple(DepositKey, DepositRecord)),
    'last_epoch_processed' : IDL.Nat64,
  });
  const RegistrationInfo = IDL.Record({
    'principal' : IDL.Principal,
    'registered_at_ns' : IDL.Nat64,
    'first_qualifying_action' : QualifyingAction,
  });
  const RevealedSeed = IDL.Record({
    'revealed_at_ns' : IDL.Nat64,
    'epoch_index' : IDL.Nat64,
    'seed' : IDL.Vec(IDL.Nat8),
    'snapshot_time_a_ns' : IDL.Nat64,
    'snapshot_time_b_ns' : IDL.Nat64,
  });
  const Result_1 = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text });
  const Result_2 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : PointsError });
  return IDL.Service({
    'add_excluded_principal' : IDL.Func([IDL.Principal], [Result], []),
    'force_epoch_tick' : IDL.Func([], [Result], []),
    'get_asset_ledgers' : IDL.Func(
        [],
        [IDL.Vec(IDL.Tuple(IDL.Nat8, IDL.Principal))],
        ['query'],
      ),
    'get_epoch_history' : IDL.Func(
        [IDL.Nat32, IDL.Nat32],
        [IDL.Vec(EpochSummary)],
        ['query'],
      ),
    'get_epoch_status' : IDL.Func([], [PublicEpochStatus], ['query']),
    'get_epoch_status_admin' : IDL.Func([], [EpochStatus], ['query']),
    'get_excluded_principals' : IDL.Func(
        [],
        [IDL.Vec(IDL.Principal)],
        ['query'],
      ),
    'get_ingest_status' : IDL.Func([], [IngestStatus], ['query']),
    'get_leaderboard' : IDL.Func(
        [IDL.Nat32, IDL.Nat32],
        [IDL.Vec(LeaderboardEntry)],
        ['query'],
      ),
    'get_pending_commit' : IDL.Func([], [IDL.Vec(IDL.Nat8)], ['query']),
    'get_points_config' : IDL.Func([], [PointsConfig], ['query']),
    'get_principal_state' : IDL.Func(
        [IDL.Principal],
        [IDL.Opt(PrincipalState)],
        ['query'],
      ),
    'get_registration_info' : IDL.Func(
        [IDL.Principal],
        [IDL.Opt(RegistrationInfo)],
        ['query'],
      ),
    'get_revealed_seed' : IDL.Func(
        [IDL.Nat64],
        [IDL.Opt(RevealedSeed)],
        ['query'],
      ),
    'is_excluded' : IDL.Func([IDL.Principal], [IDL.Bool], ['query']),
    'is_registered' : IDL.Func([IDL.Principal], [IDL.Bool], ['query']),
    'register_test_principal' : IDL.Func([IDL.Principal], [Result], []),
    'remove_excluded_principal' : IDL.Func([IDL.Principal], [Result], []),
    'set_asset_ledger' : IDL.Func([IDL.Nat8, IDL.Principal], [Result], []),
    'set_epoch_driver_enabled' : IDL.Func([IDL.Bool], [Result], []),
    'set_epoch_driver_interval_secs' : IDL.Func([IDL.Nat64], [Result], []),
    'set_excluded_principals' : IDL.Func(
        [IDL.Vec(IDL.Principal)],
        [Result],
        [],
      ),
    'set_poll_enabled' : IDL.Func([IDL.Bool], [Result], []),
    'set_poll_interval_secs' : IDL.Func([IDL.Nat64], [Result], []),
    'set_source_canister' : IDL.Func([IDL.Nat8, IDL.Principal], [Result], []),
    'start_season' : IDL.Func([IDL.Vec(IDL.Nat8)], [Result_1], []),
    'trigger_poll' : IDL.Func([], [Result_2], []),
  });
};
export const init = ({ IDL }) => {
  const InitArgs = IDL.Record({
    'admin' : IDL.Opt(IDL.Principal),
    'excluded_principals' : IDL.Opt(IDL.Vec(IDL.Principal)),
    'snapshot_seed_commit' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'season_start_ns' : IDL.Opt(IDL.Nat64),
    'season_end_ns' : IDL.Opt(IDL.Nat64),
  });
  return [IDL.Opt(InitArgs)];
};
