export const idlFactory = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const ConsensusSetting = IDL.Variant({
    'Equality' : IDL.Null,
    'Threshold' : IDL.Record({ 'min' : IDL.Nat8, 'total' : IDL.Opt(IDL.Nat8) }),
  });
  const SolRpcCluster = IDL.Variant({
    'Mainnet' : IDL.Null,
    'Testnet' : IDL.Null,
    'Devnet' : IDL.Null,
  });
  const RpcSettings = IDL.Record({
    'response_size_estimate' : IDL.Opt(IDL.Nat64),
    'verify' : ConsensusSetting,
    'discover' : ConsensusSetting,
    'cycles_per_call' : IDL.Nat64,
    'cluster' : IDL.Opt(SolRpcCluster),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const LaneSettings = IDL.Record({
    'rebroadcast_interval_secs' : IDL.Nat64,
    'discover_limit' : IDL.Nat16,
    'cancel_after_secs' : IDL.Nat64,
    'nonce_pool_size' : IDL.Nat8,
    'reconcile_alarm_secs' : IDL.Nat64,
    'max_reissues' : IDL.Nat8,
    'history_horizon_secs' : IDL.Nat64,
  });
  const TokenProgram = IDL.Variant({
    'Classic' : IDL.Null,
    'Token2022' : IDL.Null,
  });
  const InitArgs = IDL.Record({
    'rpc' : IDL.Opt(RpcSettings),
    'fee_recipient' : IDL.Opt(Account),
    'lane' : IDL.Opt(LaneSettings),
    'memo_program' : IDL.Text,
    'sol_rpc' : IDL.Principal,
    'schnorr_key_name' : IDL.Text,
    'admins' : IDL.Vec(IDL.Principal),
    'token_program' : TokenProgram,
    'icusd_ledger' : IDL.Principal,
    'genesis_hash' : IDL.Text,
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  const BridgeError = IDL.Variant({
    'Rpc' : IDL.Text,
    'Internal' : IDL.Text,
    'Anonymous' : IDL.Null,
    'Paused' : IDL.Text,
    'InsufficientAllowance' : IDL.Record({
      'allowance_e8s' : IDL.Nat64,
      'required_e8s' : IDL.Nat64,
    }),
    'AmountBelowMinimum' : IDL.Record({ 'min_e8s' : IDL.Nat64 }),
    'CapacityExceeded' : IDL.Record({ 'available_e8s' : IDL.Nat64 }),
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
    'Halted' : IDL.Text,
    'RequestIdConflict' : IDL.Null,
    'InvalidState' : IDL.Text,
    'FeeChanged' : IDL.Record({ 'current_fee_e8s' : IDL.Nat64 }),
    'InsufficientFunds' : IDL.Record({
      'required_e8s' : IDL.Nat64,
      'balance_e8s' : IDL.Nat64,
    }),
    'AmountAboveMaximum' : IDL.Record({ 'max_e8s' : IDL.Nat64 }),
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : BridgeError });
  const Lane = IDL.Variant({ 'Burn' : IDL.Null, 'Mint' : IDL.Null });
  const HeldOpResolution = IDL.Variant({
    'Abandon' : IDL.Record({ 'reason' : IDL.Text }),
    'Recheck' : IDL.Null,
  });
  const RefundEligibility = IDL.Variant({
    'ProvenNotDelivered' : IDL.Null,
    'Unresolved' : IDL.Null,
    'Unsigned' : IDL.Null,
  });
  const OpKindView = IDL.Variant({
    'Burn' : IDL.Null,
    'Mint' : IDL.Null,
    'Refund' : IDL.Null,
  });
  const OpStateView = IDL.Variant({
    'Abandoned' : IDL.Record({ 'reason' : IDL.Text }),
    'Held' : IDL.Record({ 'reason' : IDL.Text }),
    'NeedsReissue' : IDL.Record({ 'reason' : IDL.Text }),
    'Active' : IDL.Null,
    'Delivered' : IDL.Record({
      'signature' : IDL.Text,
      'slot' : IDL.Nat64,
      'adopted' : IDL.Bool,
    }),
  });
  const OpView = IDL.Record({
    'id' : IDL.Nat64,
    'refund_eligibility' : IDL.Opt(RefundEligibility),
    'kind' : OpKindView,
    'lane' : Lane,
    'note' : IDL.Opt(IDL.Text),
    'generation' : IDL.Nat32,
    'updated_at_ns' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'state' : OpStateView,
    'tx_ids' : IDL.Vec(IDL.Text),
    'slot_index' : IDL.Opt(IDL.Nat8),
    'external_id' : IDL.Text,
    'amount' : IDL.Nat64,
  });
  const Result_1 = IDL.Variant({ 'Ok' : OpView, 'Err' : BridgeError });
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
  const SolanaInclusion = IDL.Record({
    'signature' : IDL.Text,
    'slot' : IDL.Nat64,
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
    'solana_recipient' : IDL.Text,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'delivered' : IDL.Opt(SolanaInclusion),
    'refund_block_index' : IDL.Opt(IDL.Nat64),
    'deposit_fee_e8s' : IDL.Nat64,
    'mint_op_id' : IDL.Opt(IDL.Nat64),
  });
  const Result_2 = IDL.Variant({ 'Ok' : DepositView, 'Err' : BridgeError });
  const EscrowReading = IDL.Record({
    'ok' : IDL.Bool,
    'balance' : IDL.Nat,
    'at_ns' : IDL.Nat64,
    'required' : IDL.Nat,
  });
  const SupplyReading = IDL.Record({
    'inflight_mint' : IDL.Nat,
    'at_ns' : IDL.Nat64,
    'quiescent' : IDL.Bool,
    'within_bound' : IDL.Bool,
    'unattributed_gap' : IDL.Opt(IDL.Nat),
    'supply' : IDL.Nat,
    'bound' : IDL.Nat,
    'unattributed_gap_blocker' : IDL.Opt(IDL.Text),
  });
  const CustodyStatusView = IDL.Variant({
    'Ok' : IDL.Null,
    'Deficit' : IDL.Record({ 'deficit' : IDL.Nat }),
    'Reconciling' : IDL.Record({ 'deficit' : IDL.Nat }),
    'Excess' : IDL.Record({ 'excess' : IDL.Nat }),
  });
  const CustodyReading = IDL.Record({
    'status' : CustodyStatusView,
    'at_ns' : IDL.Nat64,
    'expected' : IDL.Nat,
    'custody' : IDL.Nat,
    'supply' : IDL.Nat,
  });
  const MonitorStatus = IDL.Record({
    'c1' : IDL.Opt(EscrowReading),
    'c2' : IDL.Opt(SupplyReading),
    'custody' : IDL.Opt(CustodyReading),
    'last_run_ns' : IDL.Opt(IDL.Nat64),
    'custody_strikes' : IDL.Nat8,
    'last_note' : IDL.Opt(IDL.Text),
    'c2_strikes' : IDL.Nat8,
    'c1_strikes' : IDL.Nat8,
  });
  const Result_3 = IDL.Variant({ 'Ok' : MonitorStatus, 'Err' : BridgeError });
  const BridgeFees = IDL.Record({
    'release_fee_e8s' : IDL.Nat64,
    'deposit_fee_e8s' : IDL.Nat64,
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
  const Pauses = IDL.Record({
    'mints' : IDL.Bool,
    'observer' : IDL.Bool,
    'releases' : IDL.Bool,
    'deposits' : IDL.Bool,
  });
  const Result_4 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : BridgeError });
  const BridgeToSolanaArgs = IDL.Record({
    'client_request_id' : IDL.Vec(IDL.Nat8),
    'solana_recipient' : IDL.Text,
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'amount_e8s' : IDL.Nat64,
    'expected_deposit_fee_e8s' : IDL.Nat64,
  });
  const Transition = IDL.Variant({
    'RemintDelivered' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'BurnSigned' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'PullFailed' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'BurnFinalized' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'StrayObserved' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'SweepCompleted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'OutflowResolved' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'BurnNotLanded' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'RefundStarted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'StrayRefunded' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'AdmitDeposit' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'RemintStarted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'ReturnCustodied' : IDL.Record({ 'amount' : IDL.Nat64 }),
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
  const MintState = IDL.Variant({
    'None' : IDL.Null,
    'Bound' : IDL.Null,
    'DeployedUnbound' : IDL.Null,
  });
  const BootstrapStatus = IDL.Record({
    'return_account' : IDL.Opt(IDL.Text),
    'bind_note' : IDL.Opt(IDL.Text),
    'mint_authority' : IDL.Opt(IDL.Text),
    'return_owner' : IDL.Opt(IDL.Text),
    'pool_free' : IDL.Nat32,
    'mint_state' : MintState,
    'pool_registered' : IDL.Nat32,
    'pool_reserved' : IDL.Nat32,
    'keys_derived' : IDL.Bool,
    'fee_payer' : IDL.Opt(IDL.Text),
    'pending_bootstrap_txs' : IDL.Nat32,
    'mint_account' : IDL.Opt(IDL.Text),
    'pool_unverified' : IDL.Nat32,
  });
  const BridgeInfo = IDL.Record({
    'rpc' : RpcSettings,
    'initialized' : IDL.Bool,
    'fees' : BridgeFees,
    'lane' : LaneSettings,
    'memo_program' : IDL.Text,
    'escrow_subaccount' : IDL.Text,
    'sol_rpc' : IDL.Principal,
    'schema_version' : IDL.Nat32,
    'schnorr_key_name' : IDL.Text,
    'admins' : IDL.Vec(IDL.Principal),
    'token_program' : TokenProgram,
    'halted' : IDL.Opt(IDL.Text),
    'icusd_ledger' : IDL.Principal,
    'genesis_hash' : IDL.Text,
    'instance_salt' : IDL.Text,
    'limits' : BridgeLimits,
    'pauses' : Pauses,
  });
  const BucketsView = IDL.Record({
    'reserved_pulls' : IDL.Nat,
    'pending_mint' : IDL.Nat,
    'unknown_outflow' : IDL.Nat,
    'pending_refund' : IDL.Nat,
    'outstanding' : IDL.Nat,
    'fees' : IDL.Nat,
    'held' : IDL.Nat,
    'burn_unknown' : IDL.Nat,
    'stray' : IDL.Nat,
    'return_custodied' : IDL.Nat,
    'pending_release' : IDL.Nat,
  });
  const ObserverStatus = IDL.Record({
    'return_account' : IDL.Opt(IDL.Text),
    'strays' : IDL.Nat64,
    'walk_active' : IDL.Bool,
    'cursor' : IDL.Opt(IDL.Text),
    'returns_active' : IDL.Nat64,
    'last_run_ns' : IDL.Opt(IDL.Nat64),
    'caught_up' : IDL.Bool,
    'last_note' : IDL.Opt(IDL.Text),
    'queued' : IDL.Nat64,
    'processed' : IDL.Nat64,
    'last_progress_ns' : IDL.Opt(IDL.Nat64),
    'credits_halted' : IDL.Opt(IDL.Text),
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
    'solana_sender' : IDL.Text,
    'release_block_index' : IDL.Opt(IDL.Nat64),
    'updated_at_ns' : IDL.Nat64,
    'burn_slot' : IDL.Nat64,
    'return_slot' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'burn_signature' : IDL.Text,
    'amount_e8s' : IDL.Nat64,
    'release_fee_e8s' : IDL.Nat64,
    'burn_op_id' : IDL.Nat64,
    'ledger_fee_e8s' : IDL.Opt(IDL.Nat64),
    'return_signature' : IDL.Text,
    'amount_to_receive_e8s' : IDL.Opt(IDL.Nat64),
  });
  const ReturnStatus = IDL.Variant({
    'Burned' : IDL.Record({ 'burn_op_id' : IDL.Nat64 }),
    'Burning' : IDL.Record({ 'burn_op_id' : IDL.Nat64 }),
    'Custodied' : IDL.Null,
  });
  const ReturnView = IDL.Record({
    'seq' : IDL.Nat64,
    'status' : ReturnStatus,
    'return_id' : IDL.Text,
    'destination' : Account,
    'signature' : IDL.Text,
    'slot' : IDL.Nat64,
    'sender' : IDL.Text,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'source_account' : IDL.Text,
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
  const Result_5 = IDL.Variant({ 'Ok' : ConsentInfo, 'Err' : Icrc21Error });
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
    'solana_sender' : IDL.Text,
    'start_after_seq' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const HintView = IDL.Record({
    'signature' : IDL.Text,
    'submitted_at_ns' : IDL.Nat64,
    'submitted_by' : IDL.Principal,
  });
  const ListReturnsArgs = IDL.Record({
    'start_after_seq' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const ListStraysArgs = IDL.Record({
    'after' : IDL.Opt(IDL.Text),
    'limit' : IDL.Nat32,
  });
  const ClaimStatus = IDL.Variant({
    'Refunding' : IDL.Record({ 'op_id' : IDL.Nat64 }),
    'Refunded' : IDL.Record({ 'op_id' : IDL.Nat64 }),
    'Held' : IDL.Null,
    'Retained' : IDL.Null,
  });
  const ClaimView = IDL.Record({
    'status' : ClaimStatus,
    'verified_sender' : IDL.Opt(IDL.Text),
    'inner_instruction' : IDL.Opt(IDL.Nat16),
    'outer_instruction' : IDL.Nat16,
    'refund_id' : IDL.Text,
    'amount_e8s' : IDL.Nat64,
    'source_account' : IDL.Text,
  });
  const StrayView = IDL.Record({
    'claims' : IDL.Vec(ClaimView),
    'signature' : IDL.Text,
    'verified_sender' : IDL.Opt(IDL.Text),
    'slot' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'source_account' : IDL.Opt(IDL.Text),
    'reason' : IDL.Text,
  });
  const ListOpsArgs = IDL.Record({
    'start_after' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  return IDL.Service({
    'admin_clear_credits_halt' : IDL.Func([IDL.Text], [Result], []),
    'admin_clear_lane_halt' : IDL.Func([Lane, IDL.Text], [Result], []),
    'admin_resolve_held_op' : IDL.Func(
        [IDL.Nat64, HeldOpResolution],
        [Result_1],
        [],
      ),
    'admin_retry_deposit' : IDL.Func([IDL.Nat64], [Result_2], []),
    'admin_run_monitors' : IDL.Func([], [Result_3], []),
    'admin_run_tick' : IDL.Func([], [Result], []),
    'admin_set_fees' : IDL.Func([BridgeFees], [Result], []),
    'admin_set_limits' : IDL.Func([BridgeLimits], [Result], []),
    'admin_set_pauses' : IDL.Func([Pauses], [Result], []),
    'admin_sweep_fees' : IDL.Func([IDL.Nat64], [Result_4], []),
    'bridge_to_solana' : IDL.Func([BridgeToSolanaArgs], [Result_2], []),
    'bridge_version' : IDL.Func([], [IDL.Text], ['query']),
    'get_audit_log' : IDL.Func(
        [IDL.Nat64, IDL.Nat32],
        [IDL.Vec(AuditEntry)],
        ['query'],
      ),
    'get_bootstrap_status' : IDL.Func([], [BootstrapStatus], ['query']),
    'get_bridge_info' : IDL.Func([], [BridgeInfo], ['query']),
    'get_buckets' : IDL.Func([], [BucketsView], ['query']),
    'get_deposit' : IDL.Func([IDL.Nat64], [IDL.Opt(DepositView)], ['query']),
    'get_deposit_by_request' : IDL.Func(
        [IDL.Principal, IDL.Vec(IDL.Nat8)],
        [IDL.Opt(DepositView)],
        ['query'],
      ),
    'get_monitor_status' : IDL.Func([], [MonitorStatus], ['query']),
    'get_observer_status' : IDL.Func([], [ObserverStatus], ['query']),
    'get_release' : IDL.Func([IDL.Text], [IDL.Opt(ReleaseView)], ['query']),
    'get_return' : IDL.Func([IDL.Text], [IDL.Opt(ReturnView)], ['query']),
    'get_sol_op' : IDL.Func([IDL.Nat64], [IDL.Opt(OpView)], ['query']),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [ConsentMessageRequest],
        [Result_5],
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
    'list_releases_for_solana_sender' : IDL.Func(
        [ListReleasesForSenderArgs],
        [IDL.Vec(ReleaseView)],
        ['query'],
      ),
    'list_return_hints' : IDL.Func([], [IDL.Vec(HintView)], ['query']),
    'list_returns' : IDL.Func(
        [ListReturnsArgs],
        [IDL.Vec(ReturnView)],
        ['query'],
      ),
    'list_strays' : IDL.Func([ListStraysArgs], [IDL.Vec(StrayView)], ['query']),
    'list_unresolved_ops' : IDL.Func(
        [ListOpsArgs],
        [IDL.Vec(OpView)],
        ['query'],
      ),
    'notify_solana_return' : IDL.Func([IDL.Text], [Result], []),
    'request_deposit_refund' : IDL.Func([IDL.Nat64], [Result_2], []),
    'set_admins' : IDL.Func([IDL.Vec(IDL.Principal)], [Result], []),
  });
};
export const init = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const ConsensusSetting = IDL.Variant({
    'Equality' : IDL.Null,
    'Threshold' : IDL.Record({ 'min' : IDL.Nat8, 'total' : IDL.Opt(IDL.Nat8) }),
  });
  const SolRpcCluster = IDL.Variant({
    'Mainnet' : IDL.Null,
    'Testnet' : IDL.Null,
    'Devnet' : IDL.Null,
  });
  const RpcSettings = IDL.Record({
    'response_size_estimate' : IDL.Opt(IDL.Nat64),
    'verify' : ConsensusSetting,
    'discover' : ConsensusSetting,
    'cycles_per_call' : IDL.Nat64,
    'cluster' : IDL.Opt(SolRpcCluster),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const LaneSettings = IDL.Record({
    'rebroadcast_interval_secs' : IDL.Nat64,
    'discover_limit' : IDL.Nat16,
    'cancel_after_secs' : IDL.Nat64,
    'nonce_pool_size' : IDL.Nat8,
    'reconcile_alarm_secs' : IDL.Nat64,
    'max_reissues' : IDL.Nat8,
    'history_horizon_secs' : IDL.Nat64,
  });
  const TokenProgram = IDL.Variant({
    'Classic' : IDL.Null,
    'Token2022' : IDL.Null,
  });
  const InitArgs = IDL.Record({
    'rpc' : IDL.Opt(RpcSettings),
    'fee_recipient' : IDL.Opt(Account),
    'lane' : IDL.Opt(LaneSettings),
    'memo_program' : IDL.Text,
    'sol_rpc' : IDL.Principal,
    'schnorr_key_name' : IDL.Text,
    'admins' : IDL.Vec(IDL.Principal),
    'token_program' : TokenProgram,
    'icusd_ledger' : IDL.Principal,
    'genesis_hash' : IDL.Text,
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  return [BridgeArg];
};
