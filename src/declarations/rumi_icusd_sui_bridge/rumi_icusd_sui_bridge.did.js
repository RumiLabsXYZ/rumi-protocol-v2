export const idlFactory = ({ IDL }) => {
  const ProviderConfig = IDL.Record({ 'id' : IDL.Text, 'url' : IDL.Text });
  const MirrorSettings = IDL.Record({
    'max_response_bytes' : IDL.Nat32,
    'min_quorum' : IDL.Nat8,
    'enabled' : IDL.Bool,
    'min_independent_providers' : IDL.Nat8,
    'providers' : IDL.Vec(ProviderConfig),
  });
  const UpgradeArgs = IDL.Record({
    'mirror' : IDL.Opt(MirrorSettings),
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const BridgeFees = IDL.Record({
    'release_fee_e8s' : IDL.Nat64,
    'deposit_fee_e8s' : IDL.Nat64,
  });
  const SuiNetwork = IDL.Variant({
    'Mainnet' : IDL.Null,
    'Testnet' : IDL.Null,
  });
  const BridgeLimits = IDL.Record({
    'max_active_per_caller' : IDL.Nat32,
    'release_window_limit_e8s' : IDL.Nat64,
    'max_deposit_e8s' : IDL.Nat64,
    'max_active_global' : IDL.Nat32,
    'min_deposit_e8s' : IDL.Nat64,
    'window_seconds' : IDL.Nat64,
    'mint_window_limit_e8s' : IDL.Nat64,
    'max_outstanding_e8s' : IDL.Nat64,
  });
  const InitArgs = IDL.Record({
    'fee_recipient' : IDL.Opt(Account),
    'fees' : IDL.Opt(BridgeFees),
    'mirror' : IDL.Opt(MirrorSettings),
    'network' : SuiNetwork,
    'admins' : IDL.Vec(IDL.Principal),
    'chain_identifier' : IDL.Text,
    'icusd_ledger' : IDL.Principal,
    'limits' : IDL.Opt(BridgeLimits),
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  const SuiBridgeError = IDL.Variant({
    'Internal' : IDL.Text,
    'Anonymous' : IDL.Null,
    'Paused' : IDL.Text,
    'InsufficientAllowance' : IDL.Record({
      'allowance_e8s' : IDL.Nat64,
      'required_e8s' : IDL.Nat64,
    }),
    'AmountBelowMinimum' : IDL.Record({ 'min_e8s' : IDL.Nat64 }),
    'CapacityExceeded' : IDL.Record({ 'available_e8s' : IDL.Nat64 }),
    'DeploymentMismatch' : IDL.Text,
    'SignerFailed' : IDL.Text,
    'NotFound' : IDL.Null,
    'TooManyActiveOperations' : IDL.Null,
    'Unauthorized' : IDL.Null,
    'NotOwner' : IDL.Null,
    'InvalidRecipient' : IDL.Text,
    'RateLimited' : IDL.Record({
      'window_resets_at_ns' : IDL.Nat64,
      'available_e8s' : IDL.Nat64,
    }),
    'NotConfigured' : IDL.Text,
    'InvalidArgument' : IDL.Text,
    'Ledger' : IDL.Text,
    'Unavailable' : IDL.Text,
    'Halted' : IDL.Text,
    'RequestIdConflict' : IDL.Null,
    'InvalidState' : IDL.Text,
    'FeeChanged' : IDL.Record({ 'current_fee_e8s' : IDL.Nat64 }),
    'InsufficientFunds' : IDL.Record({
      'required_e8s' : IDL.Nat64,
      'balance_e8s' : IDL.Nat64,
    }),
    'Conflict' : IDL.Text,
    'AmountAboveMaximum' : IDL.Record({ 'max_e8s' : IDL.Nat64 }),
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : SuiBridgeError });
  const DepositStatus = IDL.Variant({
    'Refunding' : IDL.Null,
    'Refunded' : IDL.Null,
    'MintHeld' : IDL.Record({ 'reason' : IDL.Text }),
    'PullPending' : IDL.Null,
    'PullFailed' : IDL.Record({ 'reason' : IDL.Text }),
    'Delivered' : IDL.Null,
    'MintInFlight' : IDL.Null,
    'Escrowed' : IDL.Null,
    'PullUnknown' : IDL.Null,
  });
  const DepositKindView = IDL.Variant({
    'UserDeposit' : IDL.Null,
    'Remint' : IDL.Record({ 'return_id' : IDL.Text }),
  });
  const SuiInclusion = IDL.Record({
    'checkpoint' : IDL.Nat64,
    'tx_digest' : IDL.Opt(IDL.Text),
    'sequence' : IDL.Nat64,
  });
  const DepositView = IDL.Record({
    'id' : IDL.Nat64,
    'status' : DepositStatus,
    'client_request_id' : IDL.Vec(IDL.Nat8),
    'refund_amount_e8s' : IDL.Opt(IDL.Nat64),
    'owner' : IDL.Principal,
    'from' : IDL.Opt(Account),
    'kind' : DepositKindView,
    'mint_id' : IDL.Opt(IDL.Text),
    'pull_block_index' : IDL.Opt(IDL.Nat64),
    'mint_amount_e8s' : IDL.Nat64,
    'updated_at_ns' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'delivered' : IDL.Opt(SuiInclusion),
    'refund_block_index' : IDL.Opt(IDL.Nat64),
    'sui_recipient' : IDL.Text,
    'deposit_fee_e8s' : IDL.Nat64,
    'mint_op_id' : IDL.Opt(IDL.Nat64),
  });
  const Result_1 = IDL.Variant({ 'Ok' : DepositView, 'Err' : SuiBridgeError });
  const LaneSettingsArg = IDL.Record({
    'gas_budget' : IDL.Nat64,
    'max_attempts' : IDL.Nat32,
  });
  const Pauses = IDL.Record({
    'mints' : IDL.Bool,
    'observer' : IDL.Bool,
    'releases' : IDL.Bool,
    'deposits' : IDL.Bool,
  });
  const Result_2 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : SuiBridgeError });
  const BridgeToSuiArgs = IDL.Record({
    'client_request_id' : IDL.Vec(IDL.Nat8),
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'amount_e8s' : IDL.Nat64,
    'expected_deposit_fee_e8s' : IDL.Nat64,
    'sui_recipient' : IDL.Text,
  });
  const SuiRole = IDL.Variant({
    'UpgradeCustody' : IDL.Null,
    'Gas' : IDL.Null,
    'Treasury' : IDL.Null,
  });
  const RoleView = IDL.Record({
    'public_key' : IDL.Opt(IDL.Text),
    'role' : SuiRole,
    'derivation_path' : IDL.Vec(IDL.Text),
    'address' : IDL.Opt(IDL.Text),
  });
  const Result_3 = IDL.Variant({
    'Ok' : IDL.Vec(RoleView),
    'Err' : SuiBridgeError,
  });
  const Transition = IDL.Variant({
    'RemintDelivered' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'PullFailed' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'SweepCompleted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'ReturnFinalized' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'OutflowResolved' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'RefundStarted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'AdmitDeposit' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'RemintStarted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'ReleaseHeld' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'MintDelivered' : IDL.Record({ 'fee' : IDL.Nat64, 'amount' : IDL.Nat64 }),
    'RefundCompleted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'ReleaseCompleted' : IDL.Record({
      'amount' : IDL.Nat64,
      'release_fee' : IDL.Nat64,
    }),
    'PullSucceeded' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'OutflowUnknown' : IDL.Record({ 'amount' : IDL.Nat64 }),
  });
  const AuditEntry = IDL.Record({
    'seq' : IDL.Nat64,
    'return_id' : IDL.Opt(IDL.Text),
    'deposit_id' : IDL.Opt(IDL.Nat64),
    'at_ns' : IDL.Nat64,
    'op_id' : IDL.Opt(IDL.Nat64),
    'note' : IDL.Text,
    'transition' : IDL.Opt(Transition),
  });
  const PendingHeldView = IDL.Record({
    'pending_refund_e8s' : IDL.Nat,
    'reasons' : IDL.Vec(IDL.Text),
    'reserved_pulls_e8s' : IDL.Nat,
    'pending_mint_e8s' : IDL.Nat,
    'active_releases' : IDL.Nat64,
    'held_releases' : IDL.Nat64,
    'active_deposits' : IDL.Nat64,
    'held_e8s' : IDL.Nat,
    'pending_release_e8s' : IDL.Nat,
    'held_deposits' : IDL.Nat64,
    'active_mint_op' : IDL.Opt(IDL.Nat64),
    'unknown_outflow_e8s' : IDL.Nat,
  });
  const BucketsView = IDL.Record({
    'reserved_pulls' : IDL.Nat,
    'pending_mint' : IDL.Nat,
    'unknown_outflow' : IDL.Nat,
    'pending_refund' : IDL.Nat,
    'outstanding' : IDL.Nat,
    'fees' : IDL.Nat,
    'held' : IDL.Nat,
    'pending_release' : IDL.Nat,
  });
  const SelfReportedView = IDL.Record({
    'minted_e8s' : IDL.Nat,
    'inflight_mint_e8s' : IDL.Nat,
    'inflight_mint_ops' : IDL.Nat64,
    'minted_count' : IDL.Nat64,
    'returned_e8s' : IDL.Nat,
    'returns_verified_through' : IDL.Nat64,
    'buckets' : BucketsView,
  });
  const ObservedDeploymentView = IDL.Record({
    'bridge_out_count' : IDL.Nat64,
    'decimals' : IDL.Nat8,
    'mint_count' : IDL.Nat64,
    'supply_cap' : IDL.Nat64,
    'mint_cap_owner' : IDL.Text,
    'initial_shared_version' : IDL.Nat64,
    'burned_total' : IDL.Nat,
    'checkpoint' : IDL.Nat64,
    'mint_cap_version' : IDL.Nat64,
    'observed_at_ns' : IDL.Nat64,
    'gate_version' : IDL.Nat64,
    'admin_cap_id' : IDL.Text,
    'min_bridge_out' : IDL.Nat64,
    'admin_cap_role' : IDL.Opt(SuiRole),
    'epoch' : IDL.Nat64,
    'mint_cap_id' : IDL.Text,
    'admin_cap_owner' : IDL.Text,
    'module_digest_observed' : IDL.Bool,
    'total_supply' : IDL.Nat64,
    'minted_total' : IDL.Nat,
    'symbol' : IDL.Text,
    'checkpoint_digest' : IDL.Text,
  });
  const GateObservationView = IDL.Record({
    'bridge_out_count' : IDL.Nat64,
    'mint_count' : IDL.Nat64,
    'supply_cap' : IDL.Nat64,
    'burned_total' : IDL.Nat,
    'gate_version' : IDL.Nat64,
    'total_supply' : IDL.Nat64,
    'minted_total' : IDL.Nat,
  });
  const IndependentObservationsView = IDL.Record({
    'deployment' : IDL.Opt(ObservedDeploymentView),
    'checkpoint' : IDL.Opt(IDL.Nat64),
    'gate' : IDL.Opt(GateObservationView),
    'observed_at_ns' : IDL.Opt(IDL.Nat64),
    'escrow_balance_e8s' : IDL.Opt(IDL.Nat),
    'epoch' : IDL.Opt(IDL.Nat64),
    'checkpoint_digest' : IDL.Opt(IDL.Text),
  });
  const BreachScopeView = IDL.Variant({
    'MintLane' : IDL.Null,
    'Bridge' : IDL.Null,
  });
  const CheckOutcomeView = IDL.Variant({
    'Ok' : IDL.Null,
    'Breach' : IDL.Record({ 'detail' : IDL.Text, 'scope' : BreachScopeView }),
    'NotEvaluated' : IDL.Record({ 'reason' : IDL.Text }),
  });
  const CheckView = IDL.Record({
    'name' : IDL.Text,
    'outcome' : CheckOutcomeView,
  });
  const LastCheckedView = IDL.Record({
    'at_ns' : IDL.Opt(IDL.Nat64),
    'checkpoint' : IDL.Opt(IDL.Nat64),
    'age_ns' : IDL.Opt(IDL.Nat64),
    'stale' : IDL.Bool,
    'caught_up' : IDL.Bool,
  });
  const AuditReport = IDL.Record({
    'pending_and_held' : PendingHeldView,
    'self_reported' : SelfReportedView,
    'generated_at_ns' : IDL.Nat64,
    'disclosures' : IDL.Vec(IDL.Text),
    'independent' : IndependentObservationsView,
    'checks' : IDL.Vec(CheckView),
    'last_checked' : LastCheckedView,
    'books_at_last_check' : IDL.Opt(BucketsView),
  });
  const GateBindingView = IDL.Record({
    'decimals' : IDL.Nat8,
    'admin_cap_id' : IDL.Text,
    'coin_type' : IDL.Text,
    'mint_cap_id' : IDL.Text,
    'gate_id' : IDL.Text,
    'gate_type' : IDL.Text,
    'package_id' : IDL.Text,
  });
  const ConfigView = IDL.Record({
    'signing_enabled' : IDL.Bool,
    'bridge_version' : IDL.Text,
    'initialized' : IDL.Bool,
    'fee_recipient' : IDL.Opt(Account),
    'fees' : BridgeFees,
    'mirror' : MirrorSettings,
    'network' : SuiNetwork,
    'escrow_subaccount' : IDL.Text,
    'schema_version' : IDL.Nat32,
    'binding' : IDL.Opt(GateBindingView),
    'schnorr_key_name' : IDL.Text,
    'short_chain_id' : IDL.Text,
    'admins' : IDL.Vec(IDL.Principal),
    'halted' : IDL.Opt(IDL.Text),
    'chain_identifier' : IDL.Text,
    'icusd_ledger' : IDL.Principal,
    'instance_salt' : IDL.Text,
    'limits' : BridgeLimits,
    'pauses' : Pauses,
  });
  const MintLaneView = IDL.Record({
    'active_op' : IDL.Opt(IDL.Nat64),
    'observed' : IDL.Opt(ObservedDeploymentView),
    'gas_budget' : IDL.Nat64,
    'last_final_checkpoint' : IDL.Nat64,
    'max_attempts' : IDL.Nat32,
    'blocked_by' : IDL.Vec(IDL.Text),
    'gas_coin' : IDL.Opt(IDL.Text),
    'last_wait' : IDL.Opt(IDL.Text),
    'lane_halt' : IDL.Opt(IDL.Text),
    'ready' : IDL.Bool,
  });
  const DeliveryViaView = IDL.Variant({
    'ConsumedRecord' : IDL.Null,
    'Receipt' : IDL.Null,
  });
  const MintOpStatusView = IDL.Variant({
    'Failed' : IDL.Record({ 'reason' : IDL.Text }),
    'Active' : IDL.Null,
    'Delivered' : IDL.Record({
      'via' : DeliveryViaView,
      'checkpoint' : IDL.Nat64,
      'tx_digest' : IDL.Opt(IDL.Text),
      'sequence' : IDL.Nat64,
    }),
  });
  const AttemptStatusView = IDL.Variant({
    'LandedFinal' : IDL.Record({ 'checkpoint' : IDL.Nat64 }),
    'Open' : IDL.Null,
    'NeverSigned' : IDL.Record({ 'reason' : IDL.Text }),
    'FailedFinal' : IDL.Record({
      'checkpoint' : IDL.Nat64,
      'error' : IDL.Text,
    }),
  });
  const MintAttemptView = IDL.Record({
    'status' : AttemptStatusView,
    'gas_signed' : IDL.Bool,
    'gas_sig_unknown' : IDL.Bool,
    'gas_budget' : IDL.Nat64,
    'mint_cap_version' : IDL.Nat64,
    'generation' : IDL.Nat32,
    'epoch' : IDL.Nat64,
    'gas_coin_version' : IDL.Nat64,
    'sends' : IDL.Nat32,
    'treasury_signed' : IDL.Bool,
    'tx_digest' : IDL.Text,
    'signing_digest' : IDL.Text,
    'treasury_sig_unknown' : IDL.Bool,
    'gas_price' : IDL.Nat64,
  });
  const MintOpView = IDL.Record({
    'id' : IDL.Nat64,
    'status' : MintOpStatusView,
    'deposit_id' : IDL.Nat64,
    'mint_id' : IDL.Text,
    'attempts' : IDL.Vec(MintAttemptView),
    'updated_at_ns' : IDL.Nat64,
    'hold_reason' : IDL.Opt(IDL.Text),
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'sui_recipient' : IDL.Text,
  });
  const ObserverProblemView = IDL.Record({
    'at_ns' : IDL.Nat64,
    'note' : IDL.Text,
    'tx_digest' : IDL.Opt(IDL.Text),
  });
  const CandidateSourceView = IDL.Variant({
    'Event' : IDL.Null,
    'Hint' : IDL.Null,
  });
  const CandidateView = IDL.Record({
    'verified' : IDL.Bool,
    'tries' : IDL.Nat32,
    'sui_sender' : IDL.Text,
    'source' : CandidateSourceView,
    'claimed_amount_e8s' : IDL.Nat64,
    'claimed_sequence' : IDL.Nat64,
    'tx_digest' : IDL.Text,
    'waiting' : IDL.Opt(IDL.Text),
    'client_nonce' : IDL.Nat64,
  });
  const ObserverView = IDL.Record({
    'gap' : IDL.Opt(IDL.Text),
    'scanned_through' : IDL.Nat64,
    'started' : IDL.Bool,
    'returns_verified_through' : IDL.Nat64,
    'caught_up' : IDL.Bool,
    'problems' : IDL.Vec(ObserverProblemView),
    'next_sequence' : IDL.Nat64,
    'last_head' : IDL.Nat64,
    'next_run_ns' : IDL.Nat64,
    'last_wait' : IDL.Opt(IDL.Text),
    'candidates' : IDL.Vec(CandidateView),
  });
  const ReleaseStatus = IDL.Variant({
    'Reminted' : IDL.Record({ 'deposit_id' : IDL.Nat64 }),
    'TransferUnknown' : IDL.Null,
    'Held' : IDL.Record({ 'reason' : IDL.Text }),
    'Released' : IDL.Null,
    'Reminting' : IDL.Record({ 'deposit_id' : IDL.Nat64 }),
    'Pending' : IDL.Null,
  });
  const ReleaseView = IDL.Record({
    'seq' : IDL.Nat64,
    'status' : ReleaseStatus,
    'return_id' : IDL.Text,
    'destination' : Account,
    'sui_sender' : IDL.Text,
    'checkpoint' : IDL.Nat64,
    'release_block_index' : IDL.Opt(IDL.Nat64),
    'updated_at_ns' : IDL.Nat64,
    'gate_sequence' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'release_fee_e8s' : IDL.Nat64,
    'tx_digest' : IDL.Text,
    'ledger_fee_e8s' : IDL.Opt(IDL.Nat64),
    'amount_to_receive_e8s' : IDL.Opt(IDL.Nat64),
    'client_nonce' : IDL.Nat64,
  });
  const StandardRecord = IDL.Record({ 'url' : IDL.Text, 'name' : IDL.Text });
  const ConsentMessageMetadata = IDL.Record({
    'utc_offset_minutes' : IDL.Opt(IDL.Int16),
    'language' : IDL.Text,
  });
  const DeviceSpec = IDL.Variant({
    'GenericDisplay' : IDL.Null,
    'LineDisplay' : IDL.Record({
      'characters_per_line' : IDL.Nat16,
      'lines_per_page' : IDL.Nat16,
    }),
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
  const LineDisplayPage = IDL.Record({ 'lines' : IDL.Vec(IDL.Text) });
  const ConsentMessage = IDL.Variant({
    'LineDisplayMessage' : IDL.Record({ 'pages' : IDL.Vec(LineDisplayPage) }),
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
      'error_code' : IDL.Nat64,
    }),
    'UnsupportedCanisterCall' : ErrorInfo,
    'ConsentMessageUnavailable' : ErrorInfo,
  });
  const Result_4 = IDL.Variant({ 'Ok' : ConsentInfo, 'Err' : Icrc21Error });
  const Icrc28TrustedOriginsResponse = IDL.Record({
    'trusted_origins' : IDL.Vec(IDL.Text),
  });
  const ListDepositsForOwnerArgs = IDL.Record({
    'owner' : IDL.Principal,
    'start_after' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const ListDepositsArgs = IDL.Record({
    'start_after' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const ListReleasesArgs = IDL.Record({
    'owner' : IDL.Principal,
    'start_after_seq' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const ListReleasesForSenderArgs = IDL.Record({
    'sui_sender' : IDL.Text,
    'start_after_seq' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const Result_5 = IDL.Variant({
    'Ok' : ObservedDeploymentView,
    'Err' : SuiBridgeError,
  });
  const GateBindingArg = IDL.Record({
    'admin_cap_id' : IDL.Text,
    'mint_cap_id' : IDL.Text,
    'gate_id' : IDL.Text,
    'package_id' : IDL.Text,
  });
  const Result_6 = IDL.Variant({ 'Ok' : ConfigView, 'Err' : SuiBridgeError });
  const HttpHeader = IDL.Record({ 'value' : IDL.Text, 'name' : IDL.Text });
  const HttpResponse = IDL.Record({
    'status' : IDL.Nat,
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(HttpHeader),
  });
  const TransformArgs = IDL.Record({
    'context' : IDL.Vec(IDL.Nat8),
    'response' : HttpResponse,
  });
  return IDL.Service({
    'admin_abandon_mint_op' : IDL.Func([IDL.Nat64, IDL.Text], [Result], []),
    'admin_clear_halt' : IDL.Func([IDL.Text], [Result], []),
    'admin_clear_mint_lane_halt' : IDL.Func([IDL.Text], [Result], []),
    'admin_retry_deposit' : IDL.Func([IDL.Nat64], [Result_1], []),
    'admin_run_tick' : IDL.Func([], [Result], []),
    'admin_set_fees' : IDL.Func([BridgeFees], [Result], []),
    'admin_set_lane_settings' : IDL.Func([LaneSettingsArg], [Result], []),
    'admin_set_limits' : IDL.Func([BridgeLimits], [Result], []),
    'admin_set_pauses' : IDL.Func([Pauses], [Result], []),
    'admin_sweep_fees' : IDL.Func([IDL.Nat64], [Result_2], []),
    'bridge_to_sui' : IDL.Func([BridgeToSuiArgs], [Result_1], []),
    'bridge_version' : IDL.Func([], [IDL.Text], ['query']),
    'derive_role_keys' : IDL.Func([], [Result_3], []),
    'get_audit_log' : IDL.Func(
        [IDL.Nat64, IDL.Nat32],
        [IDL.Vec(AuditEntry)],
        ['query'],
      ),
    'get_audit_report' : IDL.Func([], [AuditReport], ['query']),
    'get_buckets' : IDL.Func([], [BucketsView], ['query']),
    'get_config' : IDL.Func([], [ConfigView], ['query']),
    'get_deposit' : IDL.Func([IDL.Nat64], [IDL.Opt(DepositView)], ['query']),
    'get_deposit_by_request' : IDL.Func(
        [IDL.Principal, IDL.Vec(IDL.Nat8)],
        [IDL.Opt(DepositView)],
        ['query'],
      ),
    'get_mint_lane' : IDL.Func([], [MintLaneView], ['query']),
    'get_mint_op' : IDL.Func([IDL.Nat64], [IDL.Opt(MintOpView)], ['query']),
    'get_observed_deployment' : IDL.Func(
        [],
        [IDL.Opt(ObservedDeploymentView)],
        ['query'],
      ),
    'get_observer' : IDL.Func([], [ObserverView], ['query']),
    'get_release' : IDL.Func([IDL.Text], [IDL.Opt(ReleaseView)], ['query']),
    'get_roles' : IDL.Func([], [IDL.Vec(RoleView)], ['query']),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [ConsentMessageRequest],
        [Result_4],
        [],
      ),
    'icrc28_trusted_origins' : IDL.Func(
        [],
        [Icrc28TrustedOriginsResponse],
        ['query'],
      ),
    'list_deposits_for_owner' : IDL.Func(
        [ListDepositsForOwnerArgs],
        [IDL.Vec(DepositView)],
        ['query'],
      ),
    'list_mint_ops' : IDL.Func(
        [ListDepositsArgs],
        [IDL.Vec(MintOpView)],
        ['query'],
      ),
    'list_my_deposits' : IDL.Func(
        [ListDepositsArgs],
        [IDL.Vec(DepositView)],
        ['query'],
      ),
    'list_releases_for_owner' : IDL.Func(
        [ListReleasesArgs],
        [IDL.Vec(ReleaseView)],
        ['query'],
      ),
    'list_releases_for_sui_sender' : IDL.Func(
        [ListReleasesForSenderArgs],
        [IDL.Vec(ReleaseView)],
        ['query'],
      ),
    'notify_sui_return' : IDL.Func([IDL.Text], [Result], []),
    'observe_deployment' : IDL.Func([], [Result_5], []),
    'request_deposit_refund' : IDL.Func([IDL.Nat64], [Result_1], []),
    'set_admins' : IDL.Func([IDL.Vec(IDL.Principal)], [Result], []),
    'set_gas_coin' : IDL.Func([IDL.Text], [Result], []),
    'set_gate_binding' : IDL.Func([GateBindingArg], [Result_6], []),
    'transform_sui_graphql' : IDL.Func(
        [TransformArgs],
        [HttpResponse],
        ['query'],
      ),
    'transform_sui_graphql_head' : IDL.Func(
        [TransformArgs],
        [HttpResponse],
        ['query'],
      ),
    'transform_sui_send' : IDL.Func([TransformArgs], [HttpResponse], ['query']),
  });
};
export const init = ({ IDL }) => {
  const ProviderConfig = IDL.Record({ 'id' : IDL.Text, 'url' : IDL.Text });
  const MirrorSettings = IDL.Record({
    'max_response_bytes' : IDL.Nat32,
    'min_quorum' : IDL.Nat8,
    'enabled' : IDL.Bool,
    'min_independent_providers' : IDL.Nat8,
    'providers' : IDL.Vec(ProviderConfig),
  });
  const UpgradeArgs = IDL.Record({
    'mirror' : IDL.Opt(MirrorSettings),
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const BridgeFees = IDL.Record({
    'release_fee_e8s' : IDL.Nat64,
    'deposit_fee_e8s' : IDL.Nat64,
  });
  const SuiNetwork = IDL.Variant({
    'Mainnet' : IDL.Null,
    'Testnet' : IDL.Null,
  });
  const BridgeLimits = IDL.Record({
    'max_active_per_caller' : IDL.Nat32,
    'release_window_limit_e8s' : IDL.Nat64,
    'max_deposit_e8s' : IDL.Nat64,
    'max_active_global' : IDL.Nat32,
    'min_deposit_e8s' : IDL.Nat64,
    'window_seconds' : IDL.Nat64,
    'mint_window_limit_e8s' : IDL.Nat64,
    'max_outstanding_e8s' : IDL.Nat64,
  });
  const InitArgs = IDL.Record({
    'fee_recipient' : IDL.Opt(Account),
    'fees' : IDL.Opt(BridgeFees),
    'mirror' : IDL.Opt(MirrorSettings),
    'network' : SuiNetwork,
    'admins' : IDL.Vec(IDL.Principal),
    'chain_identifier' : IDL.Text,
    'icusd_ledger' : IDL.Principal,
    'limits' : IDL.Opt(BridgeLimits),
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  return [BridgeArg];
};
