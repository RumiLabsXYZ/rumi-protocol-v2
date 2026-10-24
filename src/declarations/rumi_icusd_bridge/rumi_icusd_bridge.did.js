export const idlFactory = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const L2MainnetService = IDL.Variant({
    'Alchemy' : IDL.Null,
    'Llama' : IDL.Null,
    'BlockPi' : IDL.Null,
    'PublicNode' : IDL.Null,
    'Ankr' : IDL.Null,
  });
  const RpcServiceConfig = IDL.Variant({
    'BaseMainnet' : L2MainnetService,
    'Custom' : IDL.Record({ 'url' : IDL.Text }),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const InitArgs = IDL.Record({
    'ecdsa_key_name' : IDL.Text,
    'base_chain_id' : IDL.Nat64,
    'rpc_services' : IDL.Vec(RpcServiceConfig),
    'min_quorum' : IDL.Nat8,
    'fee_recipient' : IDL.Opt(Account),
    'evm_rpc' : IDL.Principal,
    'admins' : IDL.Vec(IDL.Principal),
    'icusd_ledger' : IDL.Principal,
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  const ContractBindingView = IDL.Record({
    'bound_at_ns' : IDL.Nat64,
    'supply_cap_e8s' : IDL.Nat,
    'mint_window_seconds' : IDL.Nat64,
    'address' : IDL.Text,
    'mint_window_limit_e8s' : IDL.Nat,
    'start_block' : IDL.Nat64,
    'min_bridge_out_e8s' : IDL.Nat,
  });
  const BridgeError = IDL.Variant({
    'Rpc' : IDL.Text,
    'Internal' : IDL.Text,
    'Anonymous' : IDL.Null,
    'Paused' : IDL.Text,
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
    'AmountAboveMaximum' : IDL.Record({ 'max_e8s' : IDL.Nat64 }),
  });
  const Result = IDL.Variant({
    'Ok' : ContractBindingView,
    'Err' : BridgeError,
  });
  const Result_1 = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : BridgeError });
  const SupplyAudit = IDL.Record({
    'fees_e8s' : IDL.Nat,
    'inflight_mint_e8s' : IDL.Nat,
    'pending_refund_e8s' : IDL.Nat,
    'base_total_supply_e8s' : IDL.Opt(IDL.Nat),
    'reserved_pulls_e8s' : IDL.Nat,
    'pending_mint_e8s' : IDL.Nat,
    'escrow_account' : Account,
    'base_supply_exact_match' : IDL.Opt(IDL.Bool),
    'escrow_balance_e8s' : IDL.Opt(IDL.Nat),
    'outstanding_base_e8s' : IDL.Nat,
    'surplus_e8s' : IDL.Opt(IDL.Nat),
    'base_supply_block' : IDL.Opt(IDL.Nat64),
    'reporting_note' : IDL.Text,
    'required_escrow_e8s' : IDL.Nat,
    'liabilities_e8s' : IDL.Nat,
    'held_e8s' : IDL.Nat,
    'pending_release_e8s' : IDL.Nat,
    'escrow_balance_checked_at_ns' : IDL.Opt(IDL.Nat64),
    'deficit_e8s' : IDL.Opt(IDL.Nat),
    'unknown_outflow_e8s' : IDL.Nat,
    'coverage_ok' : IDL.Opt(IDL.Bool),
    'base_supply_within_bound' : IDL.Opt(IDL.Bool),
  });
  const Result_2 = IDL.Variant({ 'Ok' : SupplyAudit, 'Err' : BridgeError });
  const LaneKind = IDL.Variant({ 'Minter' : IDL.Null, 'Admin' : IDL.Null });
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
    'Remint' : IDL.Record({ 'transfer_id' : IDL.Text }),
  });
  const BaseTxStage = IDL.Variant({
    'Queued' : IDL.Null,
    'Finalized' : IDL.Record({
      'block_number' : IDL.Nat64,
      'success' : IDL.Bool,
    }),
    'Included' : IDL.Record({ 'block_number' : IDL.Nat64 }),
    'Submitted' : IDL.Null,
  });
  const BaseInclusion = IDL.Record({
    'block_hash' : IDL.Text,
    'block_number' : IDL.Nat64,
    'tx_hash' : IDL.Text,
  });
  const DepositView = IDL.Record({
    'id' : IDL.Nat64,
    'base_recipient' : IDL.Text,
    'status' : DepositStatus,
    'mint_tx_hashes' : IDL.Vec(IDL.Text),
    'client_request_id' : IDL.Vec(IDL.Nat8),
    'refund_amount_e8s' : IDL.Opt(IDL.Nat64),
    'owner' : IDL.Principal,
    'from' : IDL.Opt(Account),
    'kind' : DepositKindView,
    'mint_stage' : IDL.Opt(BaseTxStage),
    'mint_id' : IDL.Text,
    'pull_block_index' : IDL.Opt(IDL.Nat64),
    'mint_amount_e8s' : IDL.Nat64,
    'updated_at_ns' : IDL.Nat64,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'delivered' : IDL.Opt(BaseInclusion),
    'refund_block_index' : IDL.Opt(IDL.Nat64),
    'deposit_fee_e8s' : IDL.Nat64,
  });
  const Result_3 = IDL.Variant({ 'Ok' : DepositView, 'Err' : BridgeError });
  const BridgeFees = IDL.Record({
    'release_fee_e8s' : IDL.Nat64,
    'deposit_fee_e8s' : IDL.Nat64,
  });
  const GasConfig = IDL.Record({
    'replace_after_secs' : IDL.Nat64,
    'mint_gas_limit' : IDL.Nat64,
    'max_priority_fee_per_gas_wei' : IDL.Nat,
    'admin_gas_limit' : IDL.Nat64,
    'max_fee_per_gas_wei' : IDL.Nat,
    'min_gas_reserve_wei' : IDL.Nat,
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
  const ObserverConfig = IDL.Record({
    'idle_interval_secs' : IDL.Nat64,
    'cycles_per_rpc_call' : IDL.Nat,
    'logs_chunk_blocks' : IDL.Nat64,
    'logs_max_response_bytes' : IDL.Nat64,
    'default_max_response_bytes' : IDL.Nat64,
    'active_interval_secs' : IDL.Nat64,
  });
  const Pauses = IDL.Record({
    'mints' : IDL.Bool,
    'observer' : IDL.Bool,
    'releases' : IDL.Bool,
    'deposits' : IDL.Bool,
  });
  const ContractAdminAction = IDL.Variant({
    'UnpauseBridgeOut' : IDL.Null,
    'SetSupplyCap' : IDL.Record({ 'cap_e8s' : IDL.Nat64 }),
    'SetMinBridgeOut' : IDL.Record({ 'minimum_e8s' : IDL.Nat64 }),
    'SetMintRateLimit' : IDL.Record({
      'limit_e8s' : IDL.Nat64,
      'window_seconds' : IDL.Nat64,
    }),
    'PauseBridgeOut' : IDL.Null,
    'UnpauseMint' : IDL.Null,
    'PauseMint' : IDL.Null,
  });
  const Result_4 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : BridgeError });
  const BridgeToBaseArgs = IDL.Record({
    'base_recipient' : IDL.Text,
    'client_request_id' : IDL.Vec(IDL.Nat8),
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'amount_e8s' : IDL.Nat64,
    'expected_deposit_fee_e8s' : IDL.Nat64,
  });
  const Transition = IDL.Variant({
    'RemintDelivered' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'BridgeOutHeld' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'PullFailed' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'SweepCompleted' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'OutflowResolved' : IDL.Record({ 'amount' : IDL.Nat64 }),
    'BridgeOutAccepted' : IDL.Record({ 'amount' : IDL.Nat64 }),
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
    'tx_id' : IDL.Opt(IDL.Nat64),
    'deposit_id' : IDL.Opt(IDL.Nat64),
    'at_ns' : IDL.Nat64,
    'note' : IDL.Text,
    'transition' : IDL.Opt(Transition),
    'transfer_id' : IDL.Opt(IDL.Text),
  });
  const BaseAddresses = IDL.Record({ 'admin' : IDL.Text, 'minter' : IDL.Text });
  const Result_5 = IDL.Variant({ 'Ok' : BaseAddresses, 'Err' : BridgeError });
  const HintView = IDL.Record({
    'submitted_at_ns' : IDL.Nat64,
    'tx_hash' : IDL.Text,
    'submitted_by' : IDL.Principal,
  });
  const ObserverStatus = IDL.Record({
    'last_error' : IDL.Opt(IDL.Text),
    'last_run_ns' : IDL.Opt(IDL.Nat64),
    'processed_count' : IDL.Nat64,
    'cursor_block' : IDL.Nat64,
    'pending_hints' : IDL.Vec(HintView),
    'last_finalized_block' : IDL.Opt(IDL.Nat64),
  });
  const LaneStatus = IDL.Record({
    'next_nonce' : IDL.Opt(IDL.Nat64),
    'unfinalized_txs' : IDL.Nat32,
    'lane' : LaneKind,
    'address' : IDL.Opt(IDL.Text),
    'awaiting_gas' : IDL.Bool,
    'halted' : IDL.Opt(IDL.Text),
    'unincluded_tx' : IDL.Opt(IDL.Nat64),
  });
  const BridgeInfo = IDL.Record({
    'gas' : GasConfig,
    'observer' : ObserverStatus,
    'mint_window_used_e8s' : IDL.Nat64,
    'contract' : IDL.Opt(ContractBindingView),
    'min_quorum' : IDL.Nat8,
    'escrow_account' : Account,
    'fees' : BridgeFees,
    'active_releases' : IDL.Nat32,
    'observer_config' : ObserverConfig,
    'finality_policy' : IDL.Text,
    'window_resets_at_ns' : IDL.Nat64,
    'lanes' : IDL.Vec(LaneStatus),
    'release_window_used_e8s' : IDL.Nat64,
    'version' : IDL.Text,
    'chain_id' : IDL.Nat64,
    'ledger_fee_e8s' : IDL.Opt(IDL.Nat64),
    'active_deposits' : IDL.Nat32,
    'halted' : IDL.Opt(IDL.Text),
    'icusd_ledger' : IDL.Principal,
    'limits' : BridgeLimits,
    'rpc_service_count' : IDL.Nat32,
    'pauses' : Pauses,
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
    'status' : ReleaseStatus,
    'destination' : IDL.Opt(Account),
    'raw_icp_principal' : IDL.Vec(IDL.Nat8),
    'log_index' : IDL.Nat64,
    'release_block_index' : IDL.Opt(IDL.Nat64),
    'base_sender' : IDL.Text,
    'updated_at_ns' : IDL.Nat64,
    'transfer_id' : IDL.Text,
    'created_at_ns' : IDL.Nat64,
    'amount_e8s' : IDL.Nat64,
    'release_fee_e8s' : IDL.Nat64,
    'ledger_fee_e8s' : IDL.Opt(IDL.Nat64),
    'base_tx_hash' : IDL.Text,
    'base_block_number' : IDL.Nat64,
    'amount_to_receive_e8s' : IDL.Opt(IDL.Nat64),
    'icp_subaccount' : IDL.Vec(IDL.Nat8),
    'sequence' : IDL.Nat64,
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
  const Result_6 = IDL.Variant({ 'Ok' : ConsentInfo, 'Err' : Icrc21Error });
  const Icrc28TrustedOriginsResponse = IDL.Record({
    'trusted_origins' : IDL.Vec(IDL.Text),
  });
  const ListDepositsArgs = IDL.Record({
    'start_after' : IDL.Opt(IDL.Nat64),
    'limit' : IDL.Nat32,
  });
  const ListReleasesArgs = IDL.Record({
    'start_after_sequence' : IDL.Opt(IDL.Nat64),
    'owner' : IDL.Principal,
    'limit' : IDL.Nat32,
  });
  return IDL.Service({
    'admin_bind_contract' : IDL.Func([IDL.Text], [Result], []),
    'admin_cancel_stuck_tx' : IDL.Func([IDL.Nat64], [Result_1], []),
    'admin_clear_halt' : IDL.Func([], [Result_2], []),
    'admin_clear_lane_halt' : IDL.Func([LaneKind], [Result_1], []),
    'admin_reconcile' : IDL.Func([], [Result_2], []),
    'admin_retry_deposit' : IDL.Func([IDL.Nat64], [Result_3], []),
    'admin_run_tick' : IDL.Func([], [Result_1], []),
    'admin_set_fees' : IDL.Func([BridgeFees], [Result_1], []),
    'admin_set_gas' : IDL.Func([GasConfig], [Result_1], []),
    'admin_set_limits' : IDL.Func([BridgeLimits], [Result_1], []),
    'admin_set_observer_config' : IDL.Func([ObserverConfig], [Result_1], []),
    'admin_set_pauses' : IDL.Func([Pauses], [Result_1], []),
    'admin_set_rpc_services' : IDL.Func(
        [IDL.Vec(RpcServiceConfig), IDL.Nat8],
        [Result_1],
        [],
      ),
    'admin_submit_contract_action' : IDL.Func(
        [ContractAdminAction],
        [Result_4],
        [],
      ),
    'admin_sweep_fees' : IDL.Func([IDL.Nat64], [Result_4], []),
    'bridge_to_base' : IDL.Func([BridgeToBaseArgs], [Result_3], []),
    'bridge_version' : IDL.Func([], [IDL.Text], ['query']),
    'get_audit_log' : IDL.Func(
        [IDL.Nat64, IDL.Nat32],
        [IDL.Vec(AuditEntry)],
        ['query'],
      ),
    'get_base_addresses' : IDL.Func([], [Result_5], []),
    'get_bridge_info' : IDL.Func([], [BridgeInfo], ['query']),
    'get_deposit' : IDL.Func([IDL.Nat64], [IDL.Opt(DepositView)], ['query']),
    'get_release' : IDL.Func([IDL.Text], [IDL.Opt(ReleaseView)], ['query']),
    'get_supply_audit' : IDL.Func([], [SupplyAudit], ['query']),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [ConsentMessageRequest],
        [Result_6],
        [],
      ),
    'icrc28_trusted_origins' : IDL.Func(
        [],
        [Icrc28TrustedOriginsResponse],
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
    'notify_base_bridge_out' : IDL.Func([IDL.Text], [Result_1], []),
    'request_deposit_refund' : IDL.Func([IDL.Nat64], [Result_3], []),
    'set_admins' : IDL.Func([IDL.Vec(IDL.Principal)], [Result_1], []),
  });
};
export const init = ({ IDL }) => {
  const UpgradeArgs = IDL.Record({
    'admins' : IDL.Opt(IDL.Vec(IDL.Principal)),
  });
  const L2MainnetService = IDL.Variant({
    'Alchemy' : IDL.Null,
    'Llama' : IDL.Null,
    'BlockPi' : IDL.Null,
    'PublicNode' : IDL.Null,
    'Ankr' : IDL.Null,
  });
  const RpcServiceConfig = IDL.Variant({
    'BaseMainnet' : L2MainnetService,
    'Custom' : IDL.Record({ 'url' : IDL.Text }),
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const InitArgs = IDL.Record({
    'ecdsa_key_name' : IDL.Text,
    'base_chain_id' : IDL.Nat64,
    'rpc_services' : IDL.Vec(RpcServiceConfig),
    'min_quorum' : IDL.Nat8,
    'fee_recipient' : IDL.Opt(Account),
    'evm_rpc' : IDL.Principal,
    'admins' : IDL.Vec(IDL.Principal),
    'icusd_ledger' : IDL.Principal,
  });
  const BridgeArg = IDL.Variant({ 'Upgrade' : UpgradeArgs, 'Init' : InitArgs });
  return [BridgeArg];
};
