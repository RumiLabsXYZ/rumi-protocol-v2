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
  'open_epoch' : [] | [OpenEpoch],
  'snapshot_seed_committed' : boolean,
  'driver_interval_secs' : bigint,
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
export type Result = { 'Ok' : null } |
  { 'Err' : PointsError };
export type Result_1 = { 'Ok' : null } |
  { 'Err' : string };
export type Result_2 = { 'Ok' : bigint } |
  { 'Err' : PointsError };
export interface RevealedSeed {
  'revealed_at_ns' : bigint,
  'epoch_index' : bigint,
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
  'add_excluded_principal' : ActorMethod<[Principal], Result>,
  'cycle_manager_metrics' : ActorMethod<[], Array<CycleManagerMetric>>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'force_epoch_tick' : ActorMethod<[], Result>,
  'get_asset_ledgers' : ActorMethod<[], Array<[number, Principal]>>,
  'get_epoch_history' : ActorMethod<[number, number], Array<EpochSummary>>,
  'get_epoch_status' : ActorMethod<[], PublicEpochStatus>,
  'get_epoch_status_admin' : ActorMethod<[], EpochStatus>,
  'get_excluded_principals' : ActorMethod<[], Array<Principal>>,
  'get_ingest_status' : ActorMethod<[], IngestStatus>,
  'get_leaderboard' : ActorMethod<[number, number], Array<LeaderboardEntry>>,
  'get_pending_commit' : ActorMethod<[], Uint8Array | number[]>,
  'get_points_config' : ActorMethod<[], PointsConfig>,
  'get_principal_state' : ActorMethod<[Principal], [] | [PrincipalState]>,
  'get_registration_info' : ActorMethod<[Principal], [] | [RegistrationInfo]>,
  'get_revealed_seed' : ActorMethod<[bigint], [] | [RevealedSeed]>,
  'is_excluded' : ActorMethod<[Principal], boolean>,
  'is_registered' : ActorMethod<[Principal], boolean>,
  'register_test_principal' : ActorMethod<[Principal], Result>,
  'remove_excluded_principal' : ActorMethod<[Principal], Result>,
  'set_asset_ledger' : ActorMethod<[number, Principal], Result>,
  'set_epoch_driver_enabled' : ActorMethod<[boolean], Result>,
  'set_epoch_driver_interval_secs' : ActorMethod<[bigint], Result>,
  'set_excluded_principals' : ActorMethod<[Array<Principal>], Result>,
  'set_poll_enabled' : ActorMethod<[boolean], Result>,
  'set_poll_interval_secs' : ActorMethod<[bigint], Result>,
  'set_source_canister' : ActorMethod<[number, Principal], Result>,
  'start_season' : ActorMethod<[Uint8Array | number[]], Result_1>,
  'trigger_poll' : ActorMethod<[], Result_2>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
