export const idlFactory = ({ IDL }) => {
  const SelfRecoveryPolicyArgs = IDL.Record({
    'refill_cycles' : IDL.Nat,
    'low_balance_threshold_cycles' : IDL.Nat,
    'daily_cap_cycles' : IDL.Nat,
    'protected_reserve_cycles' : IDL.Nat,
  });
  const GovernanceTimelocksArgs = IDL.Record({
    'unpause_secs' : IDL.Nat64,
    'spend_policy_secs' : IDL.Nat64,
    'target_registry_secs' : IDL.Nat64,
    'signer_change_secs' : IDL.Nat64,
  });
  const GlobalPolicyArgs = IDL.Record({
    'global_daily_cap_cycles' : IDL.Nat,
    'self_recovery_policy' : SelfRecoveryPolicyArgs,
    'sample_interval_secs' : IDL.Nat64,
    'timelocks' : GovernanceTimelocksArgs,
    'stale_after_secs' : IDL.Nat64,
    'min_icp_reserve_e8s' : IDL.Nat,
  });
  const InitArgs = IDL.Record({
    'approval_threshold' : IDL.Nat32,
    'signers' : IDL.Vec(IDL.Principal),
    'global_policy' : GlobalPolicyArgs,
  });
  const RemoveTargetError = IDL.Variant({
    'PendingReservationExists' : IDL.Null,
    'UnresolvedOperationExists' : IDL.Null,
  });
  const SelfRecoveryPolicyError = IDL.Variant({
    'DailyCapBelowRefill' : IDL.Null,
    'ZeroLowBalanceThreshold' : IDL.Null,
    'ProtectedReserveBelowRefill' : IDL.Null,
    'ZeroRefillCycles' : IDL.Null,
    'ZeroDailyCap' : IDL.Null,
    'CyclesValueOverflow' : IDL.Null,
  });
  const GovernanceTimelocksError = IDL.Variant({
    'ZeroSignerChangeSecs' : IDL.Null,
    'ZeroUnpauseSecs' : IDL.Null,
    'ZeroSpendPolicySecs' : IDL.Null,
    'ZeroTargetRegistrySecs' : IDL.Null,
  });
  const GlobalPolicyError = IDL.Variant({
    'StaleAfterBelowSampleInterval' : IDL.Null,
    'ZeroSampleInterval' : IDL.Null,
    'InvalidSelfRecoveryPolicy' : SelfRecoveryPolicyError,
    'CyclesValueOverflow' : IDL.Null,
    'InvalidTimelocks' : GovernanceTimelocksError,
    'ZeroGlobalDailyCap' : IDL.Null,
  });
  const AlarmError = IDL.Variant({
    'AlarmNotFound' : IDL.Null,
    'NoResolvedAlarmToEvict' : IDL.Null,
    'AlarmAlreadyResolved' : IDL.Null,
  });
  const ReservedPrincipalKind = IDL.Variant({
    'Cmc' : IDL.Null,
    'Anonymous' : IDL.Null,
    'IcpLedger' : IDL.Null,
    'SentinelSelf' : IDL.Null,
    'CyclesLedger' : IDL.Null,
    'ManagementCanister' : IDL.Null,
  });
  const TargetValidationError = IDL.Variant({
    'DuplicateTarget' : IDL.Null,
    'DailyCapBelowRefill' : IDL.Null,
    'ZeroLowBalanceThreshold' : IDL.Null,
    'NameEmpty' : IDL.Null,
    'ZeroRefillCycles' : IDL.Null,
    'TagTooLong' : IDL.Null,
    'ProjectTooLong' : IDL.Null,
    'TooManyTargets' : IDL.Null,
    'ReservedPrincipal' : ReservedPrincipalKind,
    'GlobalCapBelowDailyCap' : IDL.Null,
    'AutoTopupRequiresEnabledAndObserved' : IDL.Null,
    'BurnAnomalyLimitExceedsMaximum' : IDL.Null,
    'NameTooLong' : IDL.Null,
    'LowBalanceThresholdExceedsMaximum' : IDL.Null,
    'ProjectEmpty' : IDL.Null,
    'TagEmpty' : IDL.Null,
    'DailyCapExceedsMaximum' : IDL.Null,
    'RefillExceedsMaximum' : IDL.Null,
    'CyclesValueOverflow' : IDL.Null,
    'DuplicateTag' : IDL.Null,
    'ZeroBurnAnomalyLimit' : IDL.Null,
    'TooManyTags' : IDL.Null,
  });
  const GovernanceError = IDL.Variant({
    'SelfRecoveryCapBelowPendingReservation' : IDL.Record({
      'operation_id' : IDL.Nat64,
    }),
    'ManagementSigner' : IDL.Null,
    'DuplicateSigner' : IDL.Principal,
    'TargetNotPaused' : IDL.Null,
    'ProposalNotFound' : IDL.Null,
    'ThresholdNotMet' : IDL.Null,
    'RemoveTargetBlocked' : RemoveTargetError,
    'InvalidGlobalPolicy' : GlobalPolicyError,
    'TooManyTargets' : IDL.Null,
    'TargetNotFound' : IDL.Null,
    'EmptySigners' : IDL.Null,
    'GlobalCapBelowExistingTargetCap' : IDL.Record({
      'target' : IDL.Principal,
    }),
    'EmptyTargetUpdates' : IDL.Null,
    'TimelockNotElapsed' : IDL.Null,
    'ThresholdExceedsSigners' : IDL.Record({
      'threshold' : IDL.Nat32,
      'signer_count' : IDL.Nat32,
    }),
    'ProposalNotOpen' : IDL.Null,
    'Alarm' : AlarmError,
    'GlobalCapBelowUnresolvedOperationCap' : IDL.Record({
      'operation_id' : IDL.Nat64,
    }),
    'TooManyTargetUpdates' : IDL.Null,
    'SignerNotFound' : IDL.Principal,
    'TooManyOpenProposals' : IDL.Null,
    'NotSigner' : IDL.Null,
    'InvalidTarget' : TargetValidationError,
    'ThresholdZero' : IDL.Null,
    'DuplicateTargetUpdate' : IDL.Principal,
    'AnonymousSigner' : IDL.Null,
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Bool, 'Err' : GovernanceError });
  const FundingTrigger = IDL.Variant({
    'LowBalanceAutoTopup' : IDL.Null,
    'SelfRecovery' : IDL.Null,
    'ManualTopup' : IDL.Null,
  });
  const FundingAttemptResultClass = IDL.Variant({
    'TerminalFailure' : IDL.Null,
    'Indeterminate' : IDL.Null,
    'Success' : IDL.Null,
    'RetryableFailure' : IDL.Null,
  });
  const IcpFundingState = IDL.Variant({
    'NotifyPending' : IDL.Null,
    'TransferUnknown' : IDL.Null,
    'Refunded' : IDL.Null,
    'PlannedReserved' : IDL.Null,
    'TransferConfirmed' : IDL.Null,
    'Complete' : IDL.Null,
    'Terminal' : IDL.Null,
    'Quarantined' : IDL.Null,
    'LedgerSubmitted' : IDL.Null,
  });
  const CyclesFundingState = IDL.Variant({
    'PlannedReserved' : IDL.Null,
    'Complete' : IDL.Null,
    'Confirmed' : IDL.Null,
    'Unknown' : IDL.Null,
    'Terminal' : IDL.Null,
    'Submitted' : IDL.Null,
    'Quarantined' : IDL.Null,
  });
  const FundingOperationState = IDL.Variant({
    'Icp' : IcpFundingState,
    'Cycles' : CyclesFundingState,
  });
  const FundingAttemptRecord = IDL.Record({
    'at_secs' : IDL.Nat64,
    'result_class' : FundingAttemptResultClass,
    'ordinal' : IDL.Nat32,
    'phase' : FundingOperationState,
  });
  const IcpCmcDelivery = IDL.Variant({
    'SharedReserveMint' : IDL.Null,
    'DirectTopUp' : IDL.Null,
  });
  const IcpCmcSnapshot = IDL.Record({
    'source_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'rate_timestamp_secs' : IDL.Nat64,
    'created_at_time_ns' : IDL.Nat64,
    'cmc_principal' : IDL.Principal,
    'memo' : IDL.Nat64,
    'fee_e8s' : IDL.Nat64,
    'rate_xdr_permyriad_per_icp' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'cmc_account_identifier' : IDL.Vec(IDL.Nat8),
    'target_canister' : IDL.Principal,
    'ledger_principal' : IDL.Principal,
    'delivery' : IcpCmcDelivery,
    'source_principal' : IDL.Principal,
    'expected_cycles' : IDL.Nat,
  });
  const CyclesWithdrawSnapshot = IDL.Record({
    'destination' : IDL.Principal,
    'created_at_time_ns' : IDL.Nat64,
    'fee_cycles' : IDL.Nat,
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'amount_cycles' : IDL.Nat,
  });
  const FundingRailArguments = IDL.Variant({
    'Icp' : IcpCmcSnapshot,
    'Cycles' : CyclesWithdrawSnapshot,
  });
  const TargetFundingPolicy = IDL.Record({
    'refill_cycles' : IDL.Nat,
    'burn_anomaly_limit_cycles_per_day' : IDL.Opt(IDL.Nat),
    'low_balance_threshold_cycles' : IDL.Nat,
    'daily_cap_cycles' : IDL.Nat,
    'cooldown_secs' : IDL.Nat64,
  });
  const FundingOperation = IDL.Record({
    'id' : IDL.Nat64,
    'actual_cycles' : IDL.Opt(IDL.Nat),
    'trigger' : FundingTrigger,
    'reserved_amount_cycles' : IDL.Nat,
    'attempts' : IDL.Vec(FundingAttemptRecord),
    'updated_at_secs' : IDL.Nat64,
    'notify_attempt_started_at_secs' : IDL.Opt(IDL.Nat64),
    'state' : FundingOperationState,
    'target' : IDL.Principal,
    'confirmed_block_index' : IDL.Opt(IDL.Nat64),
    'target_registry_revision' : IDL.Nat64,
    'rail_arguments' : FundingRailArguments,
    'refund_block_index' : IDL.Opt(IDL.Nat64),
    'created_at_secs' : IDL.Nat64,
    'refund_block_hint' : IDL.Opt(IDL.Nat64),
    'funding_policy' : TargetFundingPolicy,
  });
  const Result_1 = IDL.Variant({ 'Ok' : FundingOperation, 'Err' : IDL.Text });
  const Result_2 = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : GovernanceError });
  const AlarmStatus = IDL.Variant({
    'Open' : IDL.Null,
    'Acknowledged' : IDL.Null,
    'Resolved' : IDL.Null,
  });
  const AlarmKind = IDL.Variant({
    'LowBalance' : IDL.Null,
    'SelfRecoveryUnresolved' : IDL.Null,
    'BurnAnomaly' : IDL.Null,
    'Unreachable' : IDL.Null,
    'FundingUnderDelivery' : IDL.Null,
    'FundingOverDelivery' : IDL.Null,
    'FundingQuarantined' : IDL.Null,
  });
  const PublicAlarm = IDL.Record({
    'id' : IDL.Nat64,
    'status' : AlarmStatus,
    'acknowledged_at_secs' : IDL.Opt(IDL.Nat64),
    'kind' : AlarmKind,
    'target' : IDL.Opt(IDL.Principal),
    'opened_at_secs' : IDL.Nat64,
    'resolved_at_secs' : IDL.Opt(IDL.Nat64),
  });
  const FundingRail = IDL.Variant({
    'IcpCmc' : IDL.Null,
    'CyclesLedger' : IDL.Null,
  });
  const FundingOutcome = IDL.Variant({
    'Refunded' : IDL.Null,
    'Terminal' : IDL.Null,
    'Completed' : IDL.Null,
  });
  const PublicTopupSummary = IDL.Record({
    'trigger' : IDL.Opt(FundingTrigger),
    'rail' : FundingRail,
    'target' : IDL.Principal,
    'amount_cycles' : IDL.Nat,
    'outcome' : FundingOutcome,
    'resolved_at_secs' : IDL.Nat64,
  });
  const PublicFundingStatus = IDL.Variant({
    'Blocked' : IDL.Null,
    'Ready' : IDL.Null,
    'Unknown' : IDL.Null,
    'Pending' : IDL.Null,
  });
  const PublicOverview = IDL.Record({
    'icp_ledger_balance_e8s' : IDL.Opt(IDL.Nat),
    'cycles_ledger_balance_as_of_secs' : IDL.Opt(IDL.Nat64),
    'unreachable_count' : IDL.Nat64,
    'total_observed_cycles' : IDL.Nat,
    'next_sample_at_secs' : IDL.Opt(IDL.Nat64),
    'alarm_count' : IDL.Nat64,
    'uninstalled_count' : IDL.Nat64,
    'low_count' : IDL.Nat64,
    'runtime_cycles' : IDL.Nat,
    'last_sample_at_secs' : IDL.Opt(IDL.Nat64),
    'sample_interval_secs' : IDL.Opt(IDL.Nat64),
    'unobserved_count' : IDL.Nat64,
    'stopped_count' : IDL.Nat64,
    'funding_account_owner' : IDL.Principal,
    'protected_self_reserve_cycles' : IDL.Opt(IDL.Nat),
    'cycles_ledger_balance_cycles' : IDL.Opt(IDL.Nat),
    'shared_reserve_conversion_status' : PublicFundingStatus,
    'stale_after_secs' : IDL.Opt(IDL.Nat64),
    'icp_available_e8s' : IDL.Opt(IDL.Nat),
    'cycles_ledger_available_cycles' : IDL.Opt(IDL.Nat),
    'target_count' : IDL.Nat64,
    'icp_ledger_balance_as_of_secs' : IDL.Opt(IDL.Nat64),
    'min_icp_reserve_e8s' : IDL.Opt(IDL.Nat),
    'healthy_count' : IDL.Nat64,
  });
  const PublicTargetState = IDL.Variant({
    'Low' : IDL.Null,
    'Stopped' : IDL.Null,
    'Healthy' : IDL.Null,
    'Unreachable' : IDL.Null,
    'Unobserved' : IDL.Null,
    'Uninstalled' : IDL.Null,
  });
  const Criticality = IDL.Variant({
    'Important' : IDL.Null,
    'Experimental' : IDL.Null,
    'Critical' : IDL.Null,
    'Standard' : IDL.Null,
  });
  const Environment = IDL.Variant({
    'Local' : IDL.Null,
    'Production' : IDL.Null,
    'Test' : IDL.Null,
    'Archived' : IDL.Null,
    'Staging' : IDL.Null,
  });
  const ObservationMode = IDL.Variant({
    'SelfReport' : IDL.Null,
    'BlackholeRelay' : IDL.Null,
    'Unobserved' : IDL.Null,
  });
  const PublicTargetRow = IDL.Record({
    'principal' : IDL.Principal,
    'advisory_balance_overflowed' : IDL.Bool,
    'last_success_at_secs' : IDL.Opt(IDL.Nat64),
    'recent_topups' : IDL.Vec(PublicTopupSummary),
    'auto_topup' : IDL.Bool,
    'refill_cycles' : IDL.Nat,
    'tags' : IDL.Vec(IDL.Text),
    'next_sample_at_secs' : IDL.Opt(IDL.Nat64),
    'advisory_balance_cycles' : IDL.Opt(IDL.Nat),
    'burn_anomaly_limit_cycles_per_day' : IDL.Opt(IDL.Nat),
    'display_name' : IDL.Text,
    'low_balance_threshold_cycles' : IDL.Nat,
    'enabled' : IDL.Bool,
    'state' : PublicTargetState,
    'runway_secs' : IDL.Opt(IDL.Nat64),
    'as_of_secs' : IDL.Nat64,
    'reported_operational_healthy' : IDL.Opt(IDL.Bool),
    'criticality' : Criticality,
    'environment' : Environment,
    'daily_cap_cycles' : IDL.Nat,
    'stale_for_secs' : IDL.Opt(IDL.Nat64),
    'burn_cycles_per_day' : IDL.Opt(IDL.Nat),
    'paused' : IDL.Bool,
    'observation_mode' : ObservationMode,
    'project' : IDL.Text,
    'cooldown_secs' : IDL.Nat64,
  });
  const PublicPage = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(FundingOperation),
  });
  const OperatorGovernanceView = IDL.Record({
    'approval_threshold' : IDL.Nat32,
    'signers' : IDL.Vec(IDL.Principal),
    'is_single_operator_mode' : IDL.Bool,
    'is_signer' : IDL.Bool,
    'single_operator_setup_available' : IDL.Bool,
    'configured_operator_principals' : IDL.Vec(IDL.Principal),
  });
  const ProposalStatus = IDL.Variant({
    'Open' : IDL.Null,
    'Executed' : IDL.Null,
    'Cancelled' : IDL.Null,
  });
  const TargetPatch = IDL.Record({
    'auto_topup' : IDL.Opt(IDL.Bool),
    'tags' : IDL.Opt(IDL.Vec(IDL.Text)),
    'display_name' : IDL.Opt(IDL.Text),
    'enabled' : IDL.Opt(IDL.Bool),
    'criticality' : IDL.Opt(Criticality),
    'environment' : IDL.Opt(Environment),
    'observation_mode' : IDL.Opt(ObservationMode),
    'project' : IDL.Opt(IDL.Text),
    'funding_policy' : IDL.Opt(TargetFundingPolicy),
  });
  const TargetUpdate = IDL.Record({
    'principal' : IDL.Principal,
    'patch' : TargetPatch,
  });
  const TargetArgs = IDL.Record({
    'principal' : IDL.Principal,
    'tags' : IDL.Vec(IDL.Text),
    'display_name' : IDL.Text,
    'criticality' : Criticality,
    'environment' : Environment,
    'observation_mode' : ObservationMode,
    'project' : IDL.Text,
    'funding_policy' : TargetFundingPolicy,
  });
  const ProposalPayload = IDL.Variant({
    'AddSigner' : IDL.Record({ 'signer' : IDL.Principal }),
    'UpdateTarget' : TargetUpdate,
    'UpdateTargets' : IDL.Record({ 'updates' : IDL.Vec(TargetUpdate) }),
    'SetSignerThreshold' : IDL.Record({ 'threshold' : IDL.Nat32 }),
    'RemoveTarget' : IDL.Record({ 'principal' : IDL.Principal }),
    'SetGlobalPolicy' : GlobalPolicyArgs,
    'UnpauseTarget' : IDL.Record({ 'principal' : IDL.Principal }),
    'RemoveSigner' : IDL.Record({ 'signer' : IDL.Principal }),
    'RegisterTarget' : TargetArgs,
  });
  const ProposalRecord = IDL.Record({
    'id' : IDL.Nat64,
    'status' : ProposalStatus,
    'proposer' : IDL.Principal,
    'created_at_secs' : IDL.Nat64,
    'payload' : ProposalPayload,
    'approvals' : IDL.Vec(IDL.Principal),
  });
  const PublicPage_1 = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(ProposalRecord),
  });
  const OperatorDashboard = IDL.Record({
    'alarms' : IDL.Vec(PublicAlarm),
    'topup_history' : IDL.Vec(PublicTopupSummary),
    'overview' : PublicOverview,
    'targets' : IDL.Vec(PublicTargetRow),
    'unresolved_operations' : IDL.Opt(PublicPage),
    'governance' : OperatorGovernanceView,
    'proposals' : IDL.Opt(PublicPage_1),
  });
  const SingleOperatorSetupError = IDL.Variant({
    'SetupAlreadyUsed' : IDL.Null,
    'NotOperator' : IDL.Null,
    'InvalidProjection' : IDL.Null,
  });
  const Result_3 = IDL.Variant({
    'Ok' : OperatorDashboard,
    'Err' : SingleOperatorSetupError,
  });
  const CycleManagerCyclesStatus = IDL.Record({
    'idle_burn_cycles_per_day' : IDL.Opt(IDL.Nat),
    'stable_memory_bytes' : IDL.Opt(IDL.Nat64),
    'low_watermark' : IDL.Nat,
    'balance' : IDL.Nat,
    'heap_memory_bytes' : IDL.Opt(IDL.Nat64),
    'healthy' : IDL.Bool,
    'freeze_threshold_secs' : IDL.Nat64,
  });
  const PermissionsView = IDL.Record({ 'is_signer' : IDL.Bool });
  const AuthenticatedQueryError = IDL.Variant({
    'TooManyItems' : IDL.Null,
    'InvalidCursor' : IDL.Null,
    'FutureCursor' : IDL.Null,
    'NotSigner' : IDL.Null,
  });
  const Result_4 = IDL.Variant({
    'Ok' : PermissionsView,
    'Err' : AuthenticatedQueryError,
  });
  const OperatorDashboardError = IDL.Variant({
    'InvalidProjection' : IDL.Null,
  });
  const Result_5 = IDL.Variant({
    'Ok' : OperatorDashboard,
    'Err' : OperatorDashboardError,
  });
  const StandardRecord = IDL.Record({ 'url' : IDL.Text, 'name' : IDL.Text });
  const ConsentMessageMetadata = IDL.Record({
    'utc_offset_minutes' : IDL.Opt(IDL.Int16),
    'language' : IDL.Text,
  });
  const DeviceSpec = IDL.Variant({
    'GenericDisplay' : IDL.Null,
    'FieldsDisplay' : IDL.Null,
  });
  const ConsentMessageSpec = IDL.Record({
    'metadata' : ConsentMessageMetadata,
    'device_spec' : IDL.Opt(DeviceSpec),
  });
  const ConsentMessageRequest = IDL.Record({
    'arg' : IDL.Vec(IDL.Nat8),
    'method' : IDL.Text,
    'user_preferences' : ConsentMessageSpec,
  });
  const TextValue = IDL.Record({ 'content' : IDL.Text });
  const TokenAmount = IDL.Record({
    'decimals' : IDL.Nat8,
    'amount' : IDL.Nat64,
    'symbol' : IDL.Text,
  });
  const DurationSeconds = IDL.Record({ 'amount' : IDL.Nat64 });
  const Value = IDL.Variant({
    'Text' : TextValue,
    'TokenAmount' : TokenAmount,
    'TimestampSeconds' : DurationSeconds,
    'DurationSeconds' : DurationSeconds,
  });
  const ConsentMessage = IDL.Variant({
    'FieldsDisplayMessage' : IDL.Record({
      'fields' : IDL.Vec(IDL.Tuple(IDL.Text, Value)),
      'intent' : IDL.Text,
    }),
    'GenericDisplayMessage' : IDL.Text,
  });
  const ConsentInfo = IDL.Record({
    'metadata' : ConsentMessageMetadata,
    'consent_message' : ConsentMessage,
  });
  const ErrorInfo = IDL.Record({ 'description' : IDL.Text });
  const Icrc21Error = IDL.Variant({
    'GenericError' : IDL.Record({
      'description' : IDL.Text,
      'error_code' : IDL.Nat,
    }),
    'InsufficientPayment' : ErrorInfo,
    'UnsupportedCanisterCall' : ErrorInfo,
    'ConsentMessageUnavailable' : ErrorInfo,
  });
  const ConsentMessageResult = IDL.Variant({
    'Ok' : ConsentInfo,
    'Err' : Icrc21Error,
  });
  const Result_6 = IDL.Variant({
    'Ok' : PublicPage_1,
    'Err' : AuthenticatedQueryError,
  });
  const PublicPage_2 = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(PublicAlarm),
  });
  const PublicQueryError = IDL.Variant({
    'TargetNotFound' : IDL.Null,
    'TooManyItems' : IDL.Null,
    'InvalidCursor' : IDL.Null,
    'FutureCursor' : IDL.Null,
    'StaleCursor' : IDL.Null,
  });
  const Result_7 = IDL.Variant({
    'Ok' : PublicPage_2,
    'Err' : PublicQueryError,
  });
  const AdvisoryCyclesBalance = IDL.Variant({
    'Exact' : IDL.Nat,
    'Overflow' : IDL.Null,
  });
  const Sample = IDL.Record({
    'balance' : IDL.Opt(AdvisoryCyclesBalance),
    'state' : PublicTargetState,
    'reported_operational_healthy' : IDL.Opt(IDL.Bool),
    'burn_cycles_per_hour' : IDL.Opt(IDL.Nat),
    'timestamp_secs' : IDL.Nat64,
  });
  const PublicPage_3 = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(Sample),
  });
  const Result_8 = IDL.Variant({
    'Ok' : PublicPage_3,
    'Err' : PublicQueryError,
  });
  const PublicPage_4 = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(PublicTargetRow),
  });
  const Result_9 = IDL.Variant({
    'Ok' : PublicPage_4,
    'Err' : PublicQueryError,
  });
  const PublicPage_5 = IDL.Record({
    'next_cursor' : IDL.Opt(IDL.Text),
    'items' : IDL.Vec(PublicTopupSummary),
  });
  const Result_10 = IDL.Variant({
    'Ok' : PublicPage_5,
    'Err' : PublicQueryError,
  });
  const Result_11 = IDL.Variant({
    'Ok' : PublicPage,
    'Err' : AuthenticatedQueryError,
  });
  const Result_12 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : GovernanceError });
  const Result_13 = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text });
  const SelfRecoveryDeliveryStatus = IDL.Variant({ 'Unknown' : IDL.Null });
  const SelfRecoveryDeliveryRiskCause = IDL.Variant({
    'Duplicate' : IDL.Record({ 'duplicate_of' : IDL.Nat64 }),
    'AttemptLimit' : IDL.Null,
    'TooOld' : IDL.Null,
    'LegacyQuarantine' : IDL.Null,
  });
  const SelfRecoveryDeliveryRisk = IDL.Record({
    'delivery_status' : SelfRecoveryDeliveryStatus,
    'cause' : SelfRecoveryDeliveryRiskCause,
    'operation_id' : IDL.Nat64,
    'observed_at_secs' : IDL.Nat64,
  });
  const Result_14 = IDL.Variant({
    'Ok' : IDL.Opt(SelfRecoveryDeliveryRisk),
    'Err' : AuthenticatedQueryError,
  });
  return IDL.Service({
    'acknowledge_alarm' : IDL.Func([IDL.Nat64], [Result], []),
    'approve_proposal' : IDL.Func([IDL.Nat64], [Result], []),
    'attach_block_proof' : IDL.Func([IDL.Nat64, IDL.Nat64], [Result_1], []),
    'attach_refund_block_proof' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [Result_1],
        [],
      ),
    'cancel_proposal' : IDL.Func([IDL.Nat64], [Result_2], []),
    'configure_single_operator_governance' : IDL.Func([], [Result_3], []),
    'cycles_status' : IDL.Func([], [CycleManagerCyclesStatus], ['query']),
    'execute_proposal' : IDL.Func([IDL.Nat64], [Result_2], []),
    'get_my_permissions' : IDL.Func([], [Result_4], ['query']),
    'get_operator_dashboard' : IDL.Func([], [Result_5], ['query']),
    'get_public_overview' : IDL.Func([], [PublicOverview], ['query']),
    'get_public_target' : IDL.Func(
        [IDL.Principal],
        [IDL.Opt(PublicTargetRow)],
        ['query'],
      ),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [ConsentMessageRequest],
        [ConsentMessageResult],
        [],
      ),
    'list_governance_proposals' : IDL.Func(
        [IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_6],
        ['query'],
      ),
    'list_public_alarms' : IDL.Func(
        [IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_7],
        ['query'],
      ),
    'list_public_samples' : IDL.Func(
        [IDL.Principal, IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_8],
        ['query'],
      ),
    'list_public_targets' : IDL.Func(
        [IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_9],
        ['query'],
      ),
    'list_public_topups' : IDL.Func(
        [IDL.Principal, IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_10],
        ['query'],
      ),
    'list_unresolved_funding_operations' : IDL.Func(
        [IDL.Opt(IDL.Text), IDL.Nat16],
        [Result_11],
        ['query'],
      ),
    'manual_top_up' : IDL.Func([IDL.Principal], [Result_1], []),
    'manual_top_up_with_amount' : IDL.Func(
        [IDL.Principal, FundingRail, IDL.Nat],
        [Result_1],
        [],
      ),
    'pause_target' : IDL.Func([IDL.Principal], [Result_2], []),
    'propose_add_signer' : IDL.Func([IDL.Principal], [Result_12], []),
    'propose_register_target' : IDL.Func([TargetArgs], [Result_12], []),
    'propose_remove_signer' : IDL.Func([IDL.Principal], [Result_12], []),
    'propose_remove_target' : IDL.Func([IDL.Principal], [Result_12], []),
    'propose_set_global_policy' : IDL.Func([GlobalPolicyArgs], [Result_12], []),
    'propose_set_signer_threshold' : IDL.Func([IDL.Nat32], [Result_12], []),
    'propose_unpause_target' : IDL.Func([IDL.Principal], [Result_12], []),
    'propose_update_target' : IDL.Func(
        [IDL.Principal, TargetPatch],
        [Result_12],
        [],
      ),
    'propose_update_targets' : IDL.Func(
        [IDL.Vec(TargetUpdate)],
        [Result_12],
        [],
      ),
    'resolve_unknown_as_spent' : IDL.Func([IDL.Nat64], [Result_1], []),
    'run_maintenance_now' : IDL.Func([], [Result_13], []),
    'self_recovery_delivery_risk' : IDL.Func([], [Result_14], ['query']),
  });
};
export const init = ({ IDL }) => {
  const SelfRecoveryPolicyArgs = IDL.Record({
    'refill_cycles' : IDL.Nat,
    'low_balance_threshold_cycles' : IDL.Nat,
    'daily_cap_cycles' : IDL.Nat,
    'protected_reserve_cycles' : IDL.Nat,
  });
  const GovernanceTimelocksArgs = IDL.Record({
    'unpause_secs' : IDL.Nat64,
    'spend_policy_secs' : IDL.Nat64,
    'target_registry_secs' : IDL.Nat64,
    'signer_change_secs' : IDL.Nat64,
  });
  const GlobalPolicyArgs = IDL.Record({
    'global_daily_cap_cycles' : IDL.Nat,
    'self_recovery_policy' : SelfRecoveryPolicyArgs,
    'sample_interval_secs' : IDL.Nat64,
    'timelocks' : GovernanceTimelocksArgs,
    'stale_after_secs' : IDL.Nat64,
    'min_icp_reserve_e8s' : IDL.Nat,
  });
  const InitArgs = IDL.Record({
    'approval_threshold' : IDL.Nat32,
    'signers' : IDL.Vec(IDL.Principal),
    'global_policy' : GlobalPolicyArgs,
  });
  return [InitArgs];
};
