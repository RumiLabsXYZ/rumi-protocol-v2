import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export type AdvisoryCyclesBalance = { 'Exact' : bigint } |
  { 'Overflow' : null };
export type AlarmError = { 'AlarmNotFound' : null } |
  { 'NoResolvedAlarmToEvict' : null } |
  { 'AlarmAlreadyResolved' : null };
export type AlarmKind = { 'LowBalance' : null } |
  { 'SelfRecoveryUnresolved' : null } |
  { 'BurnAnomaly' : null } |
  { 'Unreachable' : null } |
  { 'FundingUnderDelivery' : null } |
  { 'FundingOverDelivery' : null } |
  { 'FundingQuarantined' : null };
export type AlarmStatus = { 'Open' : null } |
  { 'Acknowledged' : null } |
  { 'Resolved' : null };
export type AuthenticatedQueryError = { 'TooManyItems' : null } |
  { 'InvalidCursor' : null } |
  { 'FutureCursor' : null } |
  { 'NotSigner' : null };
export interface ConsentInfo {
  'metadata' : ConsentMessageMetadata,
  'consent_message' : ConsentMessage,
}
export type ConsentMessage = {
    'FieldsDisplayMessage' : {
      'fields' : Array<[string, Value]>,
      'intent' : string,
    }
  } |
  { 'GenericDisplayMessage' : string };
export interface ConsentMessageMetadata {
  'utc_offset_minutes' : [] | [number],
  'language' : string,
}
export interface ConsentMessageRequest {
  'arg' : Uint8Array | number[],
  'method' : string,
  'user_preferences' : ConsentMessageSpec,
}
export type ConsentMessageResult = { 'Ok' : ConsentInfo } |
  { 'Err' : Icrc21Error };
export interface ConsentMessageSpec {
  'metadata' : ConsentMessageMetadata,
  'device_spec' : [] | [DeviceSpec],
}
export type Criticality = { 'Important' : null } |
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
export type CyclesFundingState = { 'PlannedReserved' : null } |
  { 'Complete' : null } |
  { 'Confirmed' : null } |
  { 'Unknown' : null } |
  { 'Terminal' : null } |
  { 'Submitted' : null } |
  { 'Quarantined' : null };
export interface CyclesWithdrawSnapshot {
  'destination' : Principal,
  'created_at_time_ns' : bigint,
  'fee_cycles' : bigint,
  'from_subaccount' : [] | [Uint8Array | number[]],
  'amount_cycles' : bigint,
}
export type DeviceSpec = { 'GenericDisplay' : null } |
  { 'FieldsDisplay' : null };
export interface DurationSeconds { 'amount' : bigint }
export type Environment = { 'Local' : null } |
  { 'Production' : null } |
  { 'Test' : null } |
  { 'Archived' : null } |
  { 'Staging' : null };
export interface ErrorInfo { 'description' : string }
export interface FundingAttemptRecord {
  'at_secs' : bigint,
  'result_class' : FundingAttemptResultClass,
  'ordinal' : number,
  'phase' : FundingOperationState,
}
export type FundingAttemptResultClass = { 'TerminalFailure' : null } |
  { 'Indeterminate' : null } |
  { 'Success' : null } |
  { 'RetryableFailure' : null };
export interface FundingOperation {
  'id' : bigint,
  'actual_cycles' : [] | [bigint],
  'trigger' : FundingTrigger,
  'reserved_amount_cycles' : bigint,
  'attempts' : Array<FundingAttemptRecord>,
  'updated_at_secs' : bigint,
  'notify_attempt_started_at_secs' : [] | [bigint],
  'state' : FundingOperationState,
  'target' : Principal,
  'confirmed_block_index' : [] | [bigint],
  'target_registry_revision' : bigint,
  'rail_arguments' : FundingRailArguments,
  'refund_block_index' : [] | [bigint],
  'created_at_secs' : bigint,
  'refund_block_hint' : [] | [bigint],
  'funding_policy' : TargetFundingPolicy,
}
export type FundingOperationState = { 'Icp' : IcpFundingState } |
  { 'Cycles' : CyclesFundingState };
export type FundingOutcome = { 'Refunded' : null } |
  { 'Terminal' : null } |
  { 'Completed' : null };
export type FundingRail = { 'IcpCmc' : null } |
  { 'CyclesLedger' : null };
export type FundingRailArguments = { 'Icp' : IcpCmcSnapshot } |
  { 'Cycles' : CyclesWithdrawSnapshot };
export type FundingTrigger = { 'LowBalanceAutoTopup' : null } |
  { 'SelfRecovery' : null } |
  { 'ManualTopup' : null };
export interface GlobalPolicyArgs {
  'global_daily_cap_cycles' : bigint,
  'self_recovery_policy' : SelfRecoveryPolicyArgs,
  'sample_interval_secs' : bigint,
  'timelocks' : GovernanceTimelocksArgs,
  'stale_after_secs' : bigint,
  'min_icp_reserve_e8s' : bigint,
}
export type GlobalPolicyError = { 'StaleAfterBelowSampleInterval' : null } |
  { 'ZeroSampleInterval' : null } |
  { 'InvalidSelfRecoveryPolicy' : SelfRecoveryPolicyError } |
  { 'CyclesValueOverflow' : null } |
  { 'InvalidTimelocks' : GovernanceTimelocksError } |
  { 'ZeroGlobalDailyCap' : null };
export type GovernanceError = {
    'SelfRecoveryCapBelowPendingReservation' : { 'operation_id' : bigint }
  } |
  { 'ManagementSigner' : null } |
  { 'DuplicateSigner' : Principal } |
  { 'TargetNotPaused' : null } |
  { 'ProposalNotFound' : null } |
  { 'ThresholdNotMet' : null } |
  { 'RemoveTargetBlocked' : RemoveTargetError } |
  { 'InvalidGlobalPolicy' : GlobalPolicyError } |
  { 'TooManyTargets' : null } |
  { 'TargetNotFound' : null } |
  { 'EmptySigners' : null } |
  { 'GlobalCapBelowExistingTargetCap' : { 'target' : Principal } } |
  { 'TimelockNotElapsed' : null } |
  {
    'ThresholdExceedsSigners' : {
      'threshold' : number,
      'signer_count' : number,
    }
  } |
  { 'ProposalNotOpen' : null } |
  { 'Alarm' : AlarmError } |
  { 'GlobalCapBelowUnresolvedOperationCap' : { 'operation_id' : bigint } } |
  { 'SignerNotFound' : Principal } |
  { 'TooManyOpenProposals' : null } |
  { 'NotSigner' : null } |
  { 'InvalidTarget' : TargetValidationError } |
  { 'ThresholdZero' : null } |
  { 'AnonymousSigner' : null };
export interface GovernanceTimelocksArgs {
  'unpause_secs' : bigint,
  'spend_policy_secs' : bigint,
  'target_registry_secs' : bigint,
  'signer_change_secs' : bigint,
}
export type GovernanceTimelocksError = { 'ZeroSignerChangeSecs' : null } |
  { 'ZeroUnpauseSecs' : null } |
  { 'ZeroSpendPolicySecs' : null } |
  { 'ZeroTargetRegistrySecs' : null };
export interface IcpCmcSnapshot {
  'source_subaccount' : [] | [Uint8Array | number[]],
  'rate_timestamp_secs' : bigint,
  'created_at_time_ns' : bigint,
  'cmc_principal' : Principal,
  'memo' : bigint,
  'fee_e8s' : bigint,
  'rate_xdr_permyriad_per_icp' : bigint,
  'amount_e8s' : bigint,
  'cmc_account_identifier' : Uint8Array | number[],
  'target_canister' : Principal,
  'ledger_principal' : Principal,
  'source_principal' : Principal,
  'expected_cycles' : bigint,
}
export type IcpFundingState = { 'NotifyPending' : null } |
  { 'TransferUnknown' : null } |
  { 'Refunded' : null } |
  { 'PlannedReserved' : null } |
  { 'TransferConfirmed' : null } |
  { 'Complete' : null } |
  { 'Terminal' : null } |
  { 'Quarantined' : null } |
  { 'LedgerSubmitted' : null };
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'InsufficientPayment' : ErrorInfo } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface InitArgs {
  'approval_threshold' : number,
  'signers' : Array<Principal>,
  'global_policy' : GlobalPolicyArgs,
}
export type ObservationMode = { 'SelfReport' : null } |
  { 'BlackholeRelay' : null } |
  { 'Unobserved' : null };
export interface PermissionsView { 'is_signer' : boolean }
export type ProposalPayload = { 'AddSigner' : { 'signer' : Principal } } |
  { 'UpdateTarget' : { 'principal' : Principal, 'patch' : TargetPatch } } |
  { 'SetSignerThreshold' : { 'threshold' : number } } |
  { 'RemoveTarget' : { 'principal' : Principal } } |
  { 'SetGlobalPolicy' : GlobalPolicyArgs } |
  { 'UnpauseTarget' : { 'principal' : Principal } } |
  { 'RemoveSigner' : { 'signer' : Principal } } |
  { 'RegisterTarget' : TargetArgs };
export interface ProposalRecord {
  'id' : bigint,
  'status' : ProposalStatus,
  'proposer' : Principal,
  'created_at_secs' : bigint,
  'payload' : ProposalPayload,
  'approvals' : Array<Principal>,
}
export type ProposalStatus = { 'Open' : null } |
  { 'Executed' : null } |
  { 'Cancelled' : null };
export interface PublicAlarm {
  'id' : bigint,
  'status' : AlarmStatus,
  'acknowledged_at_secs' : [] | [bigint],
  'kind' : AlarmKind,
  'target' : [] | [Principal],
  'opened_at_secs' : bigint,
  'resolved_at_secs' : [] | [bigint],
}
export interface PublicOverview {
  'unreachable_count' : bigint,
  'total_observed_cycles' : bigint,
  'next_sample_at_secs' : [] | [bigint],
  'alarm_count' : bigint,
  'uninstalled_count' : bigint,
  'low_count' : bigint,
  'runtime_cycles' : bigint,
  'last_sample_at_secs' : [] | [bigint],
  'unobserved_count' : bigint,
  'stopped_count' : bigint,
  'protected_self_reserve_cycles' : [] | [bigint],
  'icp_available_e8s' : [] | [bigint],
  'cycles_ledger_available_cycles' : [] | [bigint],
  'target_count' : bigint,
  'healthy_count' : bigint,
}
export interface PublicPage {
  'next_cursor' : [] | [string],
  'items' : Array<ProposalRecord>,
}
export interface PublicPage_1 {
  'next_cursor' : [] | [string],
  'items' : Array<PublicAlarm>,
}
export interface PublicPage_2 {
  'next_cursor' : [] | [string],
  'items' : Array<Sample>,
}
export interface PublicPage_3 {
  'next_cursor' : [] | [string],
  'items' : Array<PublicTargetRow>,
}
export interface PublicPage_4 {
  'next_cursor' : [] | [string],
  'items' : Array<PublicTopupSummary>,
}
export interface PublicPage_5 {
  'next_cursor' : [] | [string],
  'items' : Array<FundingOperation>,
}
export type PublicQueryError = { 'TargetNotFound' : null } |
  { 'TooManyItems' : null } |
  { 'InvalidCursor' : null } |
  { 'FutureCursor' : null } |
  { 'StaleCursor' : null };
export interface PublicTargetRow {
  'principal' : Principal,
  'advisory_balance_overflowed' : boolean,
  'last_success_at_secs' : [] | [bigint],
  'recent_topups' : Array<PublicTopupSummary>,
  'refill_cycles' : bigint,
  'next_sample_at_secs' : [] | [bigint],
  'advisory_balance_cycles' : [] | [bigint],
  'display_name' : string,
  'low_balance_threshold_cycles' : bigint,
  'state' : PublicTargetState,
  'runway_secs' : [] | [bigint],
  'as_of_secs' : bigint,
  'reported_operational_healthy' : [] | [boolean],
  'criticality' : Criticality,
  'environment' : Environment,
  'stale_for_secs' : [] | [bigint],
  'burn_cycles_per_day' : [] | [bigint],
  'observation_mode' : ObservationMode,
  'project' : string,
}
export type PublicTargetState = { 'Low' : null } |
  { 'Stopped' : null } |
  { 'Healthy' : null } |
  { 'Unreachable' : null } |
  { 'Unobserved' : null } |
  { 'Uninstalled' : null };
export interface PublicTopupSummary {
  'rail' : FundingRail,
  'amount_cycles' : bigint,
  'outcome' : FundingOutcome,
  'resolved_at_secs' : bigint,
}
export type RemoveTargetError = { 'PendingReservationExists' : null } |
  { 'UnresolvedOperationExists' : null };
export type ReservedPrincipalKind = { 'Cmc' : null } |
  { 'Anonymous' : null } |
  { 'IcpLedger' : null } |
  { 'SentinelSelf' : null } |
  { 'CyclesLedger' : null } |
  { 'ManagementCanister' : null };
export type Result = { 'Ok' : boolean } |
  { 'Err' : GovernanceError };
export type Result_1 = { 'Ok' : FundingOperation } |
  { 'Err' : string };
export type Result_10 = { 'Ok' : bigint } |
  { 'Err' : GovernanceError };
export type Result_2 = { 'Ok' : null } |
  { 'Err' : GovernanceError };
export type Result_3 = { 'Ok' : PermissionsView } |
  { 'Err' : AuthenticatedQueryError };
export type Result_4 = { 'Ok' : PublicPage } |
  { 'Err' : AuthenticatedQueryError };
export type Result_5 = { 'Ok' : PublicPage_1 } |
  { 'Err' : PublicQueryError };
export type Result_6 = { 'Ok' : PublicPage_2 } |
  { 'Err' : PublicQueryError };
export type Result_7 = { 'Ok' : PublicPage_3 } |
  { 'Err' : PublicQueryError };
export type Result_8 = { 'Ok' : PublicPage_4 } |
  { 'Err' : PublicQueryError };
export type Result_9 = { 'Ok' : PublicPage_5 } |
  { 'Err' : AuthenticatedQueryError };
export interface Sample {
  'balance' : [] | [AdvisoryCyclesBalance],
  'state' : PublicTargetState,
  'reported_operational_healthy' : [] | [boolean],
  'burn_cycles_per_hour' : [] | [bigint],
  'timestamp_secs' : bigint,
}
export interface SelfRecoveryPolicyArgs {
  'refill_cycles' : bigint,
  'low_balance_threshold_cycles' : bigint,
  'daily_cap_cycles' : bigint,
  'protected_reserve_cycles' : bigint,
}
export type SelfRecoveryPolicyError = { 'DailyCapBelowRefill' : null } |
  { 'ZeroLowBalanceThreshold' : null } |
  { 'ProtectedReserveBelowRefill' : null } |
  { 'ZeroRefillCycles' : null } |
  { 'ZeroDailyCap' : null } |
  { 'CyclesValueOverflow' : null };
export interface StandardRecord { 'url' : string, 'name' : string }
export interface TargetArgs {
  'principal' : Principal,
  'tags' : Array<string>,
  'display_name' : string,
  'criticality' : Criticality,
  'environment' : Environment,
  'observation_mode' : ObservationMode,
  'project' : string,
  'funding_policy' : TargetFundingPolicy,
}
export interface TargetFundingPolicy {
  'refill_cycles' : bigint,
  'burn_anomaly_limit_cycles_per_day' : [] | [bigint],
  'low_balance_threshold_cycles' : bigint,
  'daily_cap_cycles' : bigint,
  'cooldown_secs' : bigint,
}
export interface TargetPatch {
  'auto_topup' : [] | [boolean],
  'tags' : [] | [Array<string>],
  'display_name' : [] | [string],
  'enabled' : [] | [boolean],
  'criticality' : [] | [Criticality],
  'environment' : [] | [Environment],
  'observation_mode' : [] | [ObservationMode],
  'project' : [] | [string],
  'funding_policy' : [] | [TargetFundingPolicy],
}
export type TargetValidationError = { 'DuplicateTarget' : null } |
  { 'DailyCapBelowRefill' : null } |
  { 'ZeroLowBalanceThreshold' : null } |
  { 'NameEmpty' : null } |
  { 'ZeroRefillCycles' : null } |
  { 'TagTooLong' : null } |
  { 'ProjectTooLong' : null } |
  { 'TooManyTargets' : null } |
  { 'ReservedPrincipal' : ReservedPrincipalKind } |
  { 'GlobalCapBelowDailyCap' : null } |
  { 'AutoTopupRequiresEnabledAndObserved' : null } |
  { 'BurnAnomalyLimitExceedsMaximum' : null } |
  { 'NameTooLong' : null } |
  { 'LowBalanceThresholdExceedsMaximum' : null } |
  { 'ProjectEmpty' : null } |
  { 'TagEmpty' : null } |
  { 'DailyCapExceedsMaximum' : null } |
  { 'RefillExceedsMaximum' : null } |
  { 'CyclesValueOverflow' : null } |
  { 'DuplicateTag' : null } |
  { 'ZeroBurnAnomalyLimit' : null } |
  { 'TooManyTags' : null };
export interface TextValue { 'content' : string }
export interface TimestampSeconds { 'amount' : bigint }
export interface TokenAmount {
  'decimals' : number,
  'amount' : bigint,
  'symbol' : string,
}
export type Value = { 'Text' : TextValue } |
  { 'TokenAmount' : TokenAmount } |
  { 'TimestampSeconds' : TimestampSeconds } |
  { 'DurationSeconds' : DurationSeconds };
export interface _SERVICE {
  'acknowledge_alarm' : ActorMethod<[bigint], Result>,
  'approve_proposal' : ActorMethod<[bigint], Result>,
  'attach_block_proof' : ActorMethod<[bigint, bigint], Result_1>,
  'attach_refund_block_proof' : ActorMethod<[bigint, bigint], Result_1>,
  'cancel_proposal' : ActorMethod<[bigint], Result_2>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'execute_proposal' : ActorMethod<[bigint], Result_2>,
  'get_my_permissions' : ActorMethod<[], Result_3>,
  'get_public_overview' : ActorMethod<[], PublicOverview>,
  'get_public_target' : ActorMethod<[Principal], [] | [PublicTargetRow]>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    ConsentMessageResult
  >,
  'list_governance_proposals' : ActorMethod<[[] | [string], number], Result_4>,
  'list_public_alarms' : ActorMethod<[[] | [string], number], Result_5>,
  'list_public_samples' : ActorMethod<
    [Principal, [] | [string], number],
    Result_6
  >,
  'list_public_targets' : ActorMethod<[[] | [string], number], Result_7>,
  'list_public_topups' : ActorMethod<
    [Principal, [] | [string], number],
    Result_8
  >,
  'list_unresolved_funding_operations' : ActorMethod<
    [[] | [string], number],
    Result_9
  >,
  'manual_top_up' : ActorMethod<[Principal], Result_1>,
  'pause_target' : ActorMethod<[Principal], Result_2>,
  'propose_add_signer' : ActorMethod<[Principal], Result_10>,
  'propose_register_target' : ActorMethod<[TargetArgs], Result_10>,
  'propose_remove_signer' : ActorMethod<[Principal], Result_10>,
  'propose_remove_target' : ActorMethod<[Principal], Result_10>,
  'propose_set_global_policy' : ActorMethod<[GlobalPolicyArgs], Result_10>,
  'propose_set_signer_threshold' : ActorMethod<[number], Result_10>,
  'propose_unpause_target' : ActorMethod<[Principal], Result_10>,
  'propose_update_target' : ActorMethod<[Principal, TargetPatch], Result_10>,
  'resolve_unknown_as_spent' : ActorMethod<[bigint], Result_1>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
