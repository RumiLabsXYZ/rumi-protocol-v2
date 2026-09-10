export const idlFactory = ({ IDL }) => {
  const StabilityPoolInitArgs = IDL.Record({
    'protocol_canister_id' : IDL.Principal,
    'authorized_admins' : IDL.Vec(IDL.Principal),
  });
  const StabilityPoolError = IDL.Variant({
    'LedgerTransferFailed' : IDL.Record({ 'reason' : IDL.Text }),
    'RefundClaimNotFound' : IDL.Null,
    'EmergencyPaused' : IDL.Null,
    'AlreadyOptedOut' : IDL.Record({ 'collateral' : IDL.Principal }),
    'TokenNotActive' : IDL.Record({ 'ledger' : IDL.Principal }),
    'InsufficientBalance' : IDL.Record({
      'token' : IDL.Principal,
      'available' : IDL.Nat64,
      'required' : IDL.Nat64,
    }),
    'InvalidPayoutAddress' : IDL.Record({ 'reason' : IDL.Text }),
    'CollateralNotFound' : IDL.Record({ 'ledger' : IDL.Principal }),
    'NoPositionFound' : IDL.Null,
    'AmountTooLow' : IDL.Record({ 'minimum_e8s' : IDL.Nat64 }),
    'Unauthorized' : IDL.Null,
    'InterCanisterCallFailed' : IDL.Record({
      'method' : IDL.Text,
      'target' : IDL.Text,
    }),
    'PayoutAddressRequired' : IDL.Record({ 'collateral' : IDL.Principal }),
    'XrpClaimStillOutstanding' : IDL.Record({ 'claim_id' : IDL.Nat64 }),
    'LiquidationFailed' : IDL.Record({
      'vault_id' : IDL.Nat64,
      'reason' : IDL.Text,
    }),
    'XrpClaimStatusCheckFailed' : IDL.Record({ 'reason' : IDL.Text }),
    'SystemBusy' : IDL.Null,
    'AlreadyOptedIn' : IDL.Record({ 'collateral' : IDL.Principal }),
    'TokenNotAccepted' : IDL.Record({ 'ledger' : IDL.Principal }),
    'InsufficientPoolBalance' : IDL.Null,
  });
  const CycleManagerMetric = IDL.Record({
    'key' : IDL.Text,
    'value' : IDL.Nat,
    'count' : IDL.Nat64,
    'label' : IDL.Opt(IDL.Text),
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
  const LiquidationResult = IDL.Record({
    'error_message' : IDL.Opt(IDL.Text),
    'stables_consumed' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'vault_id' : IDL.Nat64,
    'collateral_gained' : IDL.Nat64,
    'success' : IDL.Bool,
    'collateral_type' : IDL.Principal,
  });
  const ChainSpAbsorbResult = IDL.Record({
    'collateral_price_e8s' : IDL.Nat64,
    'liquidated_debt_e8s' : IDL.Nat,
    'collateral_received_native' : IDL.Nat,
    'block_index' : IDL.Nat64,
    'claim_id' : IDL.Nat64,
    'custody_address' : IDL.Text,
    'vault_id' : IDL.Nat64,
    'chain_id' : IDL.Nat32,
    'icusd_burned_e8s' : IDL.Nat64,
    'success' : IDL.Bool,
  });
  const ChainAbsorbAutoTickRecord = IDL.Record({
    'skipped_reason' : IDL.Opt(IDL.Text),
    'started_at_ns' : IDL.Nat64,
    'attempted_vault_id' : IDL.Opt(IDL.Nat64),
    'candidates_scanned' : IDL.Nat64,
    'completed_at_ns' : IDL.Nat64,
    'error' : IDL.Opt(IDL.Text),
    'absorbed' : IDL.Opt(ChainSpAbsorbResult),
  });
  const ChainAbsorbAutoConfig = IDL.Record({
    'enabled' : IDL.Bool,
    'interval_seconds' : IDL.Nat64,
    'max_scan_per_chain' : IDL.Nat64,
  });
  const ChainAbsorbAutoStatus = IDL.Record({
    'tick_in_flight' : IDL.Bool,
    'last_tick' : IDL.Opt(ChainAbsorbAutoTickRecord),
    'config' : ChainAbsorbAutoConfig,
  });
  const ChainSpAbsorbCompletion = IDL.Record({
    'result' : ChainSpAbsorbResult,
    'completed_at_ns' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
  });
  const PoolLiquidationRecord = IDL.Record({
    'collateral_price_e8s' : IDL.Opt(IDL.Nat64),
    'stables_consumed' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'vault_id' : IDL.Nat64,
    'depositors_count' : IDL.Nat64,
    'collateral_gained' : IDL.Nat64,
    'timestamp' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
  });
  const NativeXrpPendingPayout = IDL.Record({
    'claim_id' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'destination_tag' : IDL.Opt(IDL.Nat32),
    'created_at_ns' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
    'payout_address' : IDL.Text,
    'drops' : IDL.Nat64,
  });
  const ChainSpAbsorbIntentStatus = IDL.Variant({
    'BackendRejected' : IDL.Null,
    'Burned' : IDL.Null,
    'BackendAccepted' : IDL.Null,
    'Prepared' : IDL.Null,
    'LocalApplied' : IDL.Null,
  });
  const SpProofLedger = IDL.Variant({
    'IcusdBurn' : IDL.Null,
    'ThreePoolTransfer' : IDL.Null,
  });
  const SpWritedownProof = IDL.Record({
    'block_index' : IDL.Nat64,
    'ledger_kind' : SpProofLedger,
    'vault_id_memo' : IDL.Nat64,
  });
  const ChainStabilityPoolLiquidationResult = IDL.Record({
    'collateral_price_e8s' : IDL.Nat64,
    'liquidated_debt_e8s' : IDL.Nat,
    'collateral_received_native' : IDL.Nat,
    'block_index' : IDL.Nat64,
    'claim_id' : IDL.Nat64,
    'custody_address' : IDL.Text,
    'vault_id' : IDL.Nat64,
    'chain_id' : IDL.Nat32,
    'success' : IDL.Bool,
  });
  const IcrcAccount = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const ChainSpAbsorbIntent = IDL.Record({
    'last_error' : IDL.Opt(IDL.Text),
    'status' : ChainSpAbsorbIntentStatus,
    'icusd_to_burn_e8s' : IDL.Nat64,
    'burn_proof' : IDL.Opt(SpWritedownProof),
    'stables_consumed' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'updated_at_ns' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'chain_sentinel' : IDL.Principal,
    'created_at_ns' : IDL.Nat64,
    'burn_created_at_time_ns' : IDL.Nat64,
    'chain_id' : IDL.Nat32,
    'backend_result' : IDL.Opt(ChainStabilityPoolLiquidationResult),
    'icusd_ledger' : IDL.Principal,
    'icusd_minting_account' : IcrcAccount,
  });
  const PendingRefund = IDL.Record({
    'id' : IDL.Nat64,
    'user' : IDL.Principal,
    'created_at' : IDL.Nat64,
    'amount' : IDL.Nat64,
    'token_ledger' : IDL.Principal,
    'reason' : IDL.Text,
  });
  const UnallocatedInterestForwardBatch = IDL.Record({
    'id' : IDL.Nat64,
    'fee' : IDL.Opt(IDL.Nat64),
    'source_mint_blocks' : IDL.Vec(IDL.Nat64),
    'last_error' : IDL.Opt(IDL.Text),
    'transfer_created_at_ns' : IDL.Opt(IDL.Nat64),
    'transfer_block_index' : IDL.Opt(IDL.Nat64),
    'treasury_recorded' : IDL.Bool,
    'created_at_ns' : IDL.Nat64,
    'token_ledger' : IDL.Principal,
    'gross_amount' : IDL.Nat64,
    'treasury' : IDL.Opt(IDL.Principal),
  });
  const PoolEventType = IDL.Variant({
    'Withdraw' : IDL.Record({
      'amount' : IDL.Nat64,
      'token_ledger' : IDL.Principal,
    }),
    'OperationsResumed' : IDL.Null,
    'CollateralRegistered' : IDL.Record({
      'ledger' : IDL.Principal,
      'symbol' : IDL.Text,
    }),
    'OptOutCollateral' : IDL.Record({ 'collateral_type' : IDL.Principal }),
    'OptInCollateral' : IDL.Record({ 'collateral_type' : IDL.Principal }),
    'StablecoinRegistered' : IDL.Record({
      'ledger' : IDL.Principal,
      'symbol' : IDL.Text,
    }),
    'Deposit' : IDL.Record({
      'amount' : IDL.Nat64,
      'token_ledger' : IDL.Principal,
    }),
    'InterestReceived' : IDL.Record({
      'amount' : IDL.Nat64,
      'token_ledger' : IDL.Principal,
    }),
    'ConfigurationUpdated' : IDL.Null,
    'LiquidationNotification' : IDL.Record({ 'vault_count' : IDL.Nat64 }),
    'EmergencyPauseActivated' : IDL.Null,
    'DepositAs3USD' : IDL.Record({
      'amount_in' : IDL.Nat64,
      'lp_minted' : IDL.Nat64,
      'token_ledger' : IDL.Principal,
    }),
    'ClaimCollateral' : IDL.Record({
      'collateral_ledger' : IDL.Principal,
      'amount' : IDL.Nat64,
    }),
    'LiquidationExecuted' : IDL.Record({
      'stables_consumed_e8s' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'collateral_gained' : IDL.Nat64,
      'success' : IDL.Bool,
      'collateral_type' : IDL.Principal,
    }),
    'CollateralGainCorrected' : IDL.Record({
      'user' : IDL.Principal,
      'new_amount' : IDL.Nat64,
      'collateral_ledger' : IDL.Principal,
    }),
    'BalanceCorrected' : IDL.Record({
      'user' : IDL.Principal,
      'new_amount' : IDL.Nat64,
      'token_ledger' : IDL.Principal,
    }),
  });
  const PoolEvent = IDL.Record({
    'id' : IDL.Nat64,
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
    'event_type' : PoolEventType,
  });
  const StablecoinConfig = IDL.Record({
    'decimals' : IDL.Nat8,
    'transfer_fee' : IDL.Opt(IDL.Nat64),
    'ledger_id' : IDL.Principal,
    'underlying_pool' : IDL.Opt(IDL.Principal),
    'priority' : IDL.Nat8,
    'is_active' : IDL.Bool,
    'is_lp_token' : IDL.Opt(IDL.Bool),
    'symbol' : IDL.Text,
  });
  const CollateralStatus = IDL.Variant({
    'Paused' : IDL.Null,
    'Active' : IDL.Null,
    'Deprecated' : IDL.Null,
    'Sunset' : IDL.Null,
    'Frozen' : IDL.Null,
  });
  const CollateralInfo = IDL.Record({
    'status' : CollateralStatus,
    'decimals' : IDL.Nat8,
    'ledger_id' : IDL.Principal,
    'symbol' : IDL.Text,
  });
  const StabilityPoolStatus = IDL.Record({
    'collateral_gains' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'total_depositors' : IDL.Nat64,
    'stablecoin_registry' : IDL.Vec(StablecoinConfig),
    'stablecoin_balances' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'total_deposits_e8s' : IDL.Nat64,
    'total_interest_received_e8s' : IDL.Nat64,
    'eligible_usd_per_collateral' : IDL.Opt(
      IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64))
    ),
    'collateral_registry' : IDL.Vec(CollateralInfo),
    'emergency_paused' : IDL.Bool,
    'eligible_icusd_per_collateral' : IDL.Vec(
      IDL.Tuple(IDL.Principal, IDL.Nat64)
    ),
    'total_liquidations_executed' : IDL.Nat64,
  });
  const UserStabilityPosition = IDL.Record({
    'deposit_timestamp' : IDL.Nat64,
    'total_interest_earned_e8s' : IDL.Nat64,
    'collateral_gains' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'stablecoin_balances' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'native_payout_destination_tags' : IDL.Opt(
      IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat32))
    ),
    'cfx_claims' : IDL.Opt(IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat))),
    'eligible_interest_collateral' : IDL.Opt(IDL.Vec(IDL.Principal)),
    'total_claimed_gains' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'native_payout_addresses' : IDL.Opt(
      IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Text))
    ),
    'total_usd_value_e8s' : IDL.Nat64,
    'opted_out_collateral' : IDL.Vec(IDL.Principal),
  });
  const Icrc10SupportedStandard = IDL.Record({
    'url' : IDL.Text,
    'name' : IDL.Text,
  });
  const Icrc21ConsentMessageMetadata = IDL.Record({
    'utc_offset_minutes' : IDL.Opt(IDL.Int16),
    'language' : IDL.Text,
  });
  const Icrc21DeviceSpec = IDL.Variant({
    'GenericDisplay' : IDL.Null,
    'LineDisplay' : IDL.Record({
      'characters_per_line' : IDL.Nat16,
      'lines_per_page' : IDL.Nat16,
    }),
  });
  const Icrc21ConsentMessageSpec = IDL.Record({
    'metadata' : Icrc21ConsentMessageMetadata,
    'device_spec' : IDL.Opt(Icrc21DeviceSpec),
  });
  const Icrc21ConsentMessageRequest = IDL.Record({
    'arg' : IDL.Vec(IDL.Nat8),
    'method' : IDL.Text,
    'user_preferences' : Icrc21ConsentMessageSpec,
  });
  const Icrc21ConsentMessageResponseMetadata = IDL.Record({
    'utc_offset_minutes' : IDL.Opt(IDL.Int16),
    'language' : IDL.Text,
  });
  const Icrc21LineDisplayLine = IDL.Record({ 'line' : IDL.Text });
  const Icrc21LineDisplayPage = IDL.Record({
    'lines' : IDL.Vec(Icrc21LineDisplayLine),
  });
  const Icrc21ConsentMessage = IDL.Variant({
    'LineDisplayMessage' : IDL.Record({
      'pages' : IDL.Vec(Icrc21LineDisplayPage),
    }),
    'GenericDisplayMessage' : IDL.Text,
  });
  const Icrc21ConsentInfo = IDL.Record({
    'metadata' : Icrc21ConsentMessageResponseMetadata,
    'consent_message' : Icrc21ConsentMessage,
  });
  const Icrc21ErrorInfo = IDL.Record({ 'description' : IDL.Text });
  const Icrc21Error = IDL.Variant({
    'GenericError' : IDL.Record({
      'description' : IDL.Text,
      'error_code' : IDL.Nat,
    }),
    'UnsupportedCanisterCall' : Icrc21ErrorInfo,
    'ConsentMessageUnavailable' : Icrc21ErrorInfo,
  });
  const Icrc21ConsentMessageResponse = IDL.Variant({
    'Ok' : Icrc21ConsentInfo,
    'Err' : Icrc21Error,
  });
  const LiquidatableVaultInfo = IDL.Record({
    'collateral_amount' : IDL.Nat64,
    'debt_amount' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
  });
  const CfxClaimPayoutRecovery = IDL.Record({
    'claim_id' : IDL.Nat64,
    'claimant' : IDL.Principal,
    'op_id' : IDL.Nat64,
    'chain_sentinel' : IDL.Principal,
    'amount_wei' : IDL.Nat,
    'failed_at_ns' : IDL.Nat64,
    'reason' : IDL.Text,
  });
  const ChainLiquidatableVaultInfo = IDL.Record({
    'sized_repay_e8s' : IDL.Nat,
    'cr_e4' : IDL.Nat64,
    'collateral_native' : IDL.Nat,
    'vault_id' : IDL.Nat64,
    'sp_attempted' : IDL.Bool,
    'chain_collateral_sentinel' : IDL.Principal,
    'chain_id' : IDL.Nat32,
    'liquidation_threshold_e4' : IDL.Nat64,
    'effective_debt_e8s' : IDL.Nat,
    'debt_e8s' : IDL.Nat,
  });
  const ChainSpAbsorbCandidate = IDL.Record({
    'icusd_to_burn_e8s' : IDL.Nat64,
    'vault' : ChainLiquidatableVaultInfo,
    'pending_status' : IDL.Opt(ChainSpAbsorbIntentStatus),
  });
  const PoolConfiguration = IDL.Record({
    'emergency_pause' : IDL.Bool,
    'min_deposit_e8s' : IDL.Nat64,
    'authorized_admins' : IDL.Vec(IDL.Principal),
    'max_liquidations_per_batch' : IDL.Nat64,
  });
  return IDL.Service({
    'ack_native_xrp_payout_settled' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'admin_correct_balance' : IDL.Func(
        [IDL.Principal, IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : StabilityPoolError })],
        [],
      ),
    'admin_correct_collateral_gain' : IDL.Func(
        [IDL.Principal, IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : StabilityPoolError })],
        [],
      ),
    'check_chain_absorb_capacity' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Bool],
        ['query'],
      ),
    'check_pool_capacity' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Bool],
        ['query'],
      ),
    'claim_all_collateral' : IDL.Func(
        [],
        [
          IDL.Variant({
            'Ok' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
            'Err' : StabilityPoolError,
          }),
        ],
        [],
      ),
    'claim_cfx' : IDL.Func(
        [IDL.Principal, IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : StabilityPoolError })],
        [],
      ),
    'claim_collateral' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : StabilityPoolError })],
        [],
      ),
    'claim_pending_refund' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : StabilityPoolError })],
        [],
      ),
    'confirm_unallocated_interest_forward_transfer' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'cycle_manager_metrics' : IDL.Func(
        [],
        [IDL.Vec(CycleManagerMetric)],
        ['query'],
      ),
    'cycles_status' : IDL.Func([], [CycleManagerCyclesStatus], ['query']),
    'deposit' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'deposit_as_3usd' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : StabilityPoolError })],
        [],
      ),
    'emergency_pause' : IDL.Func(
        [],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'execute_liquidation' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : LiquidationResult, 'Err' : StabilityPoolError })],
        [],
      ),
    'get_chain_absorb_auto_status' : IDL.Func(
        [],
        [ChainAbsorbAutoStatus],
        ['query'],
      ),
    'get_chain_collateral_sentinel' : IDL.Func(
        [IDL.Nat32],
        [IDL.Principal],
        ['query'],
      ),
    'get_completed_chain_absorbs' : IDL.Func(
        [IDL.Opt(IDL.Nat64)],
        [IDL.Vec(ChainSpAbsorbCompletion)],
        ['query'],
      ),
    'get_liquidation_history' : IDL.Func(
        [IDL.Opt(IDL.Nat64)],
        [IDL.Vec(PoolLiquidationRecord)],
        ['query'],
      ),
    'get_my_native_xrp_payouts' : IDL.Func(
        [],
        [IDL.Vec(NativeXrpPendingPayout)],
        ['query'],
      ),
    'get_pending_chain_absorbs' : IDL.Func(
        [],
        [IDL.Vec(ChainSpAbsorbIntent)],
        ['query'],
      ),
    'get_pending_refunds' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [IDL.Vec(PendingRefund)],
        ['query'],
      ),
    'get_pending_unallocated_interest_forwards' : IDL.Func(
        [],
        [IDL.Vec(UnallocatedInterestForwardBatch)],
        ['query'],
      ),
    'get_pool_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_pool_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(PoolEvent)],
        ['query'],
      ),
    'get_pool_status' : IDL.Func([], [StabilityPoolStatus], ['query']),
    'get_suspended_tokens' : IDL.Func(
        [],
        [IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat32))],
        ['query'],
      ),
    'get_user_position' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [IDL.Opt(UserStabilityPosition)],
        ['query'],
      ),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(Icrc10SupportedStandard)],
        ['query'],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [Icrc21ConsentMessageRequest],
        [Icrc21ConsentMessageResponse],
        [],
      ),
    'list_depositor_principals' : IDL.Func(
        [],
        [IDL.Vec(IDL.Principal)],
        ['query'],
      ),
    'notify_liquidatable_vaults' : IDL.Func(
        [IDL.Vec(LiquidatableVaultInfo)],
        [IDL.Vec(LiquidationResult)],
        [],
      ),
    'opt_in_cfx' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'opt_in_collateral' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'opt_in_native_collateral' : IDL.Func(
        [IDL.Principal, IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'opt_in_native_collateral_with_tag' : IDL.Func(
        [IDL.Principal, IDL.Text, IDL.Opt(IDL.Nat32)],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'opt_out_cfx' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'opt_out_collateral' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'receive_interest_revenue' : IDL.Func(
        [IDL.Principal, IDL.Nat64, IDL.Opt(IDL.Principal)],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'receive_interest_revenue_v2' : IDL.Func(
        [IDL.Principal, IDL.Nat64, IDL.Opt(IDL.Principal), IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'recredit_failed_cfx_claim_payout' : IDL.Func(
        [CfxClaimPayoutRecovery],
        [IDL.Variant({ 'Ok' : IDL.Bool, 'Err' : StabilityPoolError })],
        [],
      ),
    'register_cfx_collateral' : IDL.Func(
        [IDL.Nat32],
        [IDL.Variant({ 'Ok' : IDL.Principal, 'Err' : StabilityPoolError })],
        [],
      ),
    'register_collateral' : IDL.Func(
        [CollateralInfo],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'register_stablecoin' : IDL.Func(
        [StablecoinConfig],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'resume_operations' : IDL.Func(
        [],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'retry_unallocated_interest_forward' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'scan_chain_absorb_candidates' : IDL.Func(
        [IDL.Opt(IDL.Nat64)],
        [
          IDL.Variant({
            'Ok' : IDL.Vec(ChainSpAbsorbCandidate),
            'Err' : StabilityPoolError,
          }),
        ],
        [],
      ),
    'set_chain_absorb_auto_config' : IDL.Func(
        [ChainAbsorbAutoConfig],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'set_interest_treasury' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'sp_absorb_chain_vault' : IDL.Func(
        [IDL.Nat64],
        [
          IDL.Variant({
            'Ok' : ChainSpAbsorbResult,
            'Err' : StabilityPoolError,
          }),
        ],
        [],
      ),
    'update_pool_configuration' : IDL.Func(
        [PoolConfiguration],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
    'validate_pool_state' : IDL.Func(
        [],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : IDL.Text })],
        ['query'],
      ),
    'withdraw' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const StabilityPoolInitArgs = IDL.Record({
    'protocol_canister_id' : IDL.Principal,
    'authorized_admins' : IDL.Vec(IDL.Principal),
  });
  return [StabilityPoolInitArgs];
};
