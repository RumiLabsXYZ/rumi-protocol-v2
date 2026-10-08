import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export type AssetType = { 'Icp' : null } |
  { 'IcUsd' : null } |
  { 'CkUsdc' : null } |
  { 'CkUsdt' : null } |
  { 'ThreeUsd' : null };
export interface CycleManagerCyclesStatus {
  'idle_burn_cycles_per_day' : [] | [bigint],
  'stable_memory_bytes' : [] | [bigint],
  'low_watermark' : bigint,
  'balance' : bigint,
  'heap_memory_bytes' : [] | [bigint],
  'healthy' : boolean,
  'freeze_threshold_secs' : bigint,
}
export interface CycleManagerMetric {
  'key' : string,
  'value' : bigint,
  'count' : bigint,
  'label' : [] | [string],
}
export interface DepositKey { 'asset' : AssetType, 'venue' : Venue }
export interface DepositRecord {
  'asset' : AssetType,
  'venue' : Venue,
  'last_verified_at' : bigint,
  'deposited_at' : bigint,
  'recorded_value_usd' : bigint,
}
export interface EpochStatus {
  'legacy_transition_held' : boolean,
  'open_epoch' : [] | [OpenEpoch],
  'snapshot_seed_committed' : boolean,
  'driver_interval_secs' : bigint,
  'legacy_reseed_pending' : boolean,
  'revealed_seed_count' : bigint,
  'current_epoch_index' : bigint,
  'driver_enabled' : boolean,
}
export interface EpochSummary {
  'epoch_index' : bigint,
  'points_accrued_this_epoch' : bigint,
  'epoch_start_ns' : bigint,
  'registered_principals' : bigint,
  'active_principals' : bigint,
  'total_points_all' : bigint,
  'snapshot_a_ns' : bigint,
  'snapshot_b_ns' : bigint,
  'epoch_end_ns' : bigint,
}
export interface FiatStablePointsPolicy {
  'legacy_epoch' : [] | [bigint],
  'active_for_current_epoch' : boolean,
  'historical_next_offset' : bigint,
  'cutover_epoch' : [] | [bigint],
  'inline_legacy_topup_rows' : bigint,
  'historical_ledger_cutoff' : [] | [bigint],
  'inline_legacy_topups_complete' : boolean,
  'historical_complete' : boolean,
}
export interface FiatStableTopupProgress {
  'credited_rows' : number,
  'processed_rows' : number,
  'credited_points' : bigint,
  'complete' : boolean,
  'next_offset' : bigint,
}
export interface IngestStatus {
  'registered_count' : bigint,
  'poll_interval_secs' : bigint,
  'poll_enabled' : boolean,
  'sources' : Array<SourceStatus>,
}
export interface InitArgs {
  'admin' : [] | [Principal],
  'excluded_principals' : [] | [Array<Principal>],
  'snapshot_seed_commit' : [] | [Uint8Array | number[]],
  'season_start_ns' : [] | [bigint],
  'season_end_ns' : [] | [bigint],
}
export interface LeaderboardEntry {
  'principal' : Principal,
  'total_points' : bigint,
  'rank' : number,
  'estimated_share_bps' : number,
}
export interface OpenEpoch {
  'close_active' : bigint,
  'close_cursor' : [] | [Principal],
  'epoch_index' : bigint,
  'epoch_start_ns' : bigint,
  'a_cursor' : [] | [Principal],
  'b_complete' : boolean,
  'b_cursor' : [] | [Principal],
  'close_started' : boolean,
  'a_complete' : boolean,
  'snapshot_a_ns' : bigint,
  'snapshot_b_ns' : bigint,
  'close_points_accrued' : bigint,
  'epoch_end_ns' : bigint,
}
export interface PointEntry {
  'principal' : Principal,
  'epoch_index' : bigint,
  'source' : PointSource,
  'recorded_at_ns' : bigint,
  'points_delta' : bigint,
}
export interface PointEntryPage {
  'reached_end' : boolean,
  'entries' : Array<PointEntry>,
  'next_offset' : bigint,
}
export type PointSource = { 'CkStable3PoolMatched' : null } |
  { 'Registration' : null } |
  { 'CkStable3PoolFlat4x' : null } |
  { 'CkStable3PoolUnmatched' : null } |
  { 'CkStable3PoolUnmatchedTopUp' : null } |
  { 'VaultRepayment' : null } |
  { 'IcUsd3Pool' : null } |
  { 'AmmLp' : null } |
  { 'IcUsdStabilityPool' : null } |
  { 'IcUsdDebt' : null } |
  { 'ThreeUsdStabilityPool' : null };
export interface PointsConfig {
  'admin' : Principal,
  'registered_count' : bigint,
  'snapshot_seed_committed' : boolean,
  'excluded_count' : number,
  'season_start_ns' : bigint,
  'season_end_ns' : bigint,
  'current_epoch_index' : bigint,
}
export type PointsError = { 'Unauthorized' : null } |
  { 'Excluded' : null };
export interface PrincipalState {
  'principal' : Principal,
  'registered_at_ns' : bigint,
  'total_points' : bigint,
  'repayment_events' : Array<RepaymentEvent>,
  'first_qualifying_action' : QualifyingAction,
  'active_deposits' : Array<[DepositKey, DepositRecord]>,
  'last_epoch_processed' : bigint,
}
export interface PublicEpochStatus {
  'open_epoch' : [] | [PublicOpenEpoch],
  'snapshot_seed_committed' : boolean,
  'driver_interval_secs' : bigint,
  'revealed_seed_count' : bigint,
  'current_epoch_index' : bigint,
  'driver_enabled' : boolean,
}
export interface PublicOpenEpoch {
  'epoch_index' : bigint,
  'epoch_start_ns' : bigint,
  'snapshot_a_ns' : [] | [bigint],
  'snapshot_b_ns' : [] | [bigint],
  'epoch_end_ns' : bigint,
}
export type QualifyingAction = { 'ProvideAmmLiquidity' : null } |
  { 'MintIcUsd' : null } |
  { 'Deposit3Pool' : null } |
  { 'DepositStabilityPool' : null } |
  { 'RepayVault' : null };
export interface RegistrationInfo {
  'principal' : Principal,
  'registered_at_ns' : bigint,
  'first_qualifying_action' : QualifyingAction,
}
export interface RepaymentEvent {
  'asset' : AssetType,
  'repaid_at' : bigint,
  'amount_usd' : bigint,
  'window_end' : bigint,
}
export type Result = { 'Ok' : FiatStablePointsPolicy } |
  { 'Err' : string };
export type Result_1 = { 'Ok' : null } |
  { 'Err' : PointsError };
export type Result_2 = { 'Ok' : bigint } |
  { 'Err' : string };
export type Result_3 = { 'Ok' : FiatStableTopupProgress } |
  { 'Err' : string };
export type Result_4 = { 'Ok' : null } |
  { 'Err' : string };
export type Result_5 = { 'Ok' : bigint } |
  { 'Err' : PointsError };
export interface RevealedSeed {
  'revealed_at_ns' : bigint,
  'epoch_index' : bigint,
  'derivation_entropy' : [] | [Uint8Array | number[]],
  'seed' : Uint8Array | number[],
  'snapshot_time_a_ns' : bigint,
  'snapshot_time_b_ns' : bigint,
}
export interface SourceStatus {
  'tag' : number,
  'cursor' : bigint,
  'canister' : Principal,
}
export type Venue = { 'Amm' : null } |
  { 'ThreePool' : null } |
  { 'Vault' : null } |
  { 'StabilityPool' : null };
export interface _SERVICE {
  'activate_fiat_stable_4x' : ActorMethod<[], Result>,
  'add_excluded_principal' : ActorMethod<[Principal], Result_1>,
  'admin_rebuild_3pool_recorded' : ActorMethod<[], Result_2>,
  'apply_fiat_stable_topups' : ActorMethod<[number], Result_3>,
  'cycle_manager_metrics' : ActorMethod<[], Array<CycleManagerMetric>>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'force_epoch_tick' : ActorMethod<[], Result_1>,
  'get_asset_ledgers' : ActorMethod<[], Array<[number, Principal]>>,
  'get_epoch_history' : ActorMethod<[number, number], Array<EpochSummary>>,
  'get_epoch_status' : ActorMethod<[], PublicEpochStatus>,
  'get_epoch_status_admin' : ActorMethod<[], EpochStatus>,
  'get_excluded_principals' : ActorMethod<[], Array<Principal>>,
  'get_fiat_stable_points_policy' : ActorMethod<[], FiatStablePointsPolicy>,
  'get_ingest_status' : ActorMethod<[], IngestStatus>,
  'get_leaderboard' : ActorMethod<[number, number], Array<LeaderboardEntry>>,
  'get_pending_commit' : ActorMethod<[], Uint8Array | number[]>,
  'get_point_entries' : ActorMethod<[bigint, number], PointEntryPage>,
  'get_point_ledger_len' : ActorMethod<[], bigint>,
  'get_points_config' : ActorMethod<[], PointsConfig>,
  'get_principal_point_entries' : ActorMethod<
    [Principal, bigint, number],
    PointEntryPage
  >,
  'get_principal_state' : ActorMethod<[Principal], [] | [PrincipalState]>,
  'get_registration_info' : ActorMethod<[Principal], [] | [RegistrationInfo]>,
  'get_revealed_seed' : ActorMethod<[bigint], [] | [RevealedSeed]>,
  'is_excluded' : ActorMethod<[Principal], boolean>,
  'is_registered' : ActorMethod<[Principal], boolean>,
  'register_test_principal' : ActorMethod<[Principal], Result_1>,
  'remove_excluded_principal' : ActorMethod<[Principal], Result_1>,
  'set_asset_ledger' : ActorMethod<[number, Principal], Result_1>,
  'set_epoch_driver_enabled' : ActorMethod<[boolean], Result_1>,
  'set_epoch_driver_interval_secs' : ActorMethod<[bigint], Result_1>,
  'set_excluded_principals' : ActorMethod<[Array<Principal>], Result_1>,
  'set_poll_enabled' : ActorMethod<[boolean], Result_1>,
  'set_poll_interval_secs' : ActorMethod<[bigint], Result_1>,
  'set_season_end_ns' : ActorMethod<[bigint], Result_4>,
  'set_source_canister' : ActorMethod<[number, Principal], Result_1>,
  'start_season' : ActorMethod<[Uint8Array | number[]], Result_4>,
  'trigger_poll' : ActorMethod<[], Result_5>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
