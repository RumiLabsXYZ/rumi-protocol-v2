export const idlFactory = ({ IDL }) => {
  const Mode = IDL.Variant({
    'ReadOnly' : IDL.Null,
    'GeneralAvailability' : IDL.Null,
    'Recovery' : IDL.Null,
  });
  const UpgradeArg = IDL.Record({ 'mode' : IDL.Opt(Mode) });
  const InitArg = IDL.Record({
    'ckusdc_ledger_principal' : IDL.Opt(IDL.Principal),
    'xrc_principal' : IDL.Principal,
    'icp_ledger_principal' : IDL.Principal,
    'fee_e8s' : IDL.Nat64,
    'ckusdt_ledger_principal' : IDL.Opt(IDL.Principal),
    'stability_pool_principal' : IDL.Opt(IDL.Principal),
    'treasury_principal' : IDL.Opt(IDL.Principal),
    'developer_principal' : IDL.Principal,
    'icusd_ledger_principal' : IDL.Principal,
  });
  const ProtocolArg = IDL.Variant({ 'Upgrade' : UpgradeArg, 'Init' : InitArg });
  const XrcAssetClass = IDL.Variant({
    'Cryptocurrency' : IDL.Null,
    'FiatCurrency' : IDL.Null,
  });
  const PriceSource = IDL.Variant({
    'Xrc' : IDL.Record({
      'quote_asset_class' : XrcAssetClass,
      'quote_asset' : IDL.Text,
      'base_asset_class' : XrcAssetClass,
      'base_asset' : IDL.Text,
    }),
  });
  const AddCollateralArg = IDL.Record({
    'redemption_fee_ceiling' : IDL.Opt(IDL.Float64),
    'debt_ceiling' : IDL.Nat64,
    'min_vault_debt' : IDL.Nat64,
    'min_collateral_deposit' : IDL.Nat64,
    'redemption_fee_floor' : IDL.Opt(IDL.Float64),
    'borrow_threshold_ratio' : IDL.Float64,
    'recovery_target_cr' : IDL.Float64,
    'ledger_canister_id' : IDL.Principal,
    'price_source' : PriceSource,
    'liquidation_bonus' : IDL.Float64,
    'display_color' : IDL.Opt(IDL.Text),
    'borrowing_fee' : IDL.Float64,
    'interest_rate_apr' : IDL.Float64,
    'liquidation_ratio' : IDL.Float64,
  });
  const TransferError = IDL.Variant({
    'GenericError' : IDL.Record({
      'message' : IDL.Text,
      'error_code' : IDL.Nat,
    }),
    'TemporarilyUnavailable' : IDL.Null,
    'BadBurn' : IDL.Record({ 'min_burn_amount' : IDL.Nat }),
    'Duplicate' : IDL.Record({ 'duplicate_of' : IDL.Nat }),
    'BadFee' : IDL.Record({ 'expected_fee' : IDL.Nat }),
    'CreatedInFuture' : IDL.Record({ 'ledger_time' : IDL.Nat64 }),
    'TooOld' : IDL.Null,
    'InsufficientFunds' : IDL.Record({ 'balance' : IDL.Nat }),
  });
  const TransferFromError = IDL.Variant({
    'GenericError' : IDL.Record({
      'message' : IDL.Text,
      'error_code' : IDL.Nat,
    }),
    'TemporarilyUnavailable' : IDL.Null,
    'InsufficientAllowance' : IDL.Record({ 'allowance' : IDL.Nat }),
    'BadBurn' : IDL.Record({ 'min_burn_amount' : IDL.Nat }),
    'Duplicate' : IDL.Record({ 'duplicate_of' : IDL.Nat }),
    'BadFee' : IDL.Record({ 'expected_fee' : IDL.Nat }),
    'CreatedInFuture' : IDL.Record({ 'ledger_time' : IDL.Nat64 }),
    'TooOld' : IDL.Null,
    'InsufficientFunds' : IDL.Record({ 'balance' : IDL.Nat }),
  });
  const ProtocolError = IDL.Variant({
    'GenericError' : IDL.Text,
    'TemporarilyUnavailable' : IDL.Text,
    'TransferError' : TransferError,
    'AlreadyProcessing' : IDL.Null,
    'AnonymousCallerNotAllowed' : IDL.Null,
    'AmountTooLow' : IDL.Record({ 'minimum_amount' : IDL.Nat64 }),
    'TransferFromError' : IDL.Tuple(TransferFromError, IDL.Nat64),
    'CallerNotOwner' : IDL.Null,
  });
  const Result = IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ProtocolError });
  const VaultArg = IDL.Record({ 'vault_id' : IDL.Nat64, 'amount' : IDL.Nat64 });
  const Result_1 = IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : ProtocolError });
  const SuccessWithFee = IDL.Record({
    'block_index' : IDL.Nat64,
    'fee_amount_paid' : IDL.Nat64,
    'collateral_amount_received' : IDL.Opt(IDL.Nat64),
  });
  const Result_2 = IDL.Variant({
    'Ok' : SuccessWithFee,
    'Err' : ProtocolError,
  });
  const Result_3 = IDL.Variant({
    'Ok' : IDL.Opt(IDL.Nat64),
    'Err' : ProtocolError,
  });
  const CandidVault = IDL.Record({
    'collateral_amount' : IDL.Nat64,
    'owner' : IDL.Principal,
    'vault_id' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
    'accrued_interest' : IDL.Nat64,
    'icp_margin_amount' : IDL.Nat64,
    'borrowed_icusd_amount' : IDL.Nat64,
  });
  const CollateralStatus = IDL.Variant({
    'Paused' : IDL.Null,
    'Active' : IDL.Null,
    'Deprecated' : IDL.Null,
    'Sunset' : IDL.Null,
    'Frozen' : IDL.Null,
  });
  const InterpolationMethod = IDL.Variant({ 'Linear' : IDL.Null });
  const RateMarker = IDL.Record({
    'multiplier' : IDL.Vec(IDL.Nat8),
    'cr_level' : IDL.Vec(IDL.Nat8),
  });
  const RateCurve = IDL.Record({
    'method' : InterpolationMethod,
    'markers' : IDL.Vec(RateMarker),
  });
  const CollateralConfig = IDL.Record({
    'last_redemption_time' : IDL.Nat64,
    'status' : CollateralStatus,
    'decimals' : IDL.Nat8,
    'recovery_interest_rate_apr' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'redemption_fee_ceiling' : IDL.Vec(IDL.Nat8),
    'healthy_cr' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'debt_ceiling' : IDL.Nat64,
    'min_vault_debt' : IDL.Nat64,
    'rate_curve' : IDL.Opt(RateCurve),
    'recovery_borrowing_fee' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'min_collateral_deposit' : IDL.Nat64,
    'last_price' : IDL.Opt(IDL.Float64),
    'last_price_timestamp' : IDL.Opt(IDL.Nat64),
    'redemption_fee_floor' : IDL.Vec(IDL.Nat8),
    'borrow_threshold_ratio' : IDL.Vec(IDL.Nat8),
    'ledger_fee' : IDL.Nat64,
    'recovery_target_cr' : IDL.Vec(IDL.Nat8),
    'current_base_rate' : IDL.Vec(IDL.Nat8),
    'ledger_canister_id' : IDL.Principal,
    'price_source' : PriceSource,
    'liquidation_bonus' : IDL.Vec(IDL.Nat8),
    'display_color' : IDL.Opt(IDL.Text),
    'borrowing_fee' : IDL.Vec(IDL.Nat8),
    'interest_rate_apr' : IDL.Vec(IDL.Nat8),
    'liquidation_ratio' : IDL.Vec(IDL.Nat8),
  });
  const CollateralTotals = IDL.Record({
    'decimals' : IDL.Nat8,
    'total_collateral' : IDL.Nat64,
    'total_debt' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
    'price' : IDL.Float64,
    'vault_count' : IDL.Nat64,
    'symbol' : IDL.Text,
  });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const GetEventsArg = IDL.Record({
    'start' : IDL.Nat64,
    'length' : IDL.Nat64,
  });
  const StableTokenType = IDL.Variant({
    'CKUSDC' : IDL.Null,
    'CKUSDT' : IDL.Null,
  });
  const Vault = IDL.Record({
    'collateral_amount' : IDL.Nat64,
    'owner' : IDL.Principal,
    'vault_id' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
    'last_accrual_time' : IDL.Nat64,
    'accrued_interest' : IDL.Nat64,
    'borrowed_icusd_amount' : IDL.Nat64,
  });
  const Event = IDL.Variant({
    'set_borrowing_fee' : IDL.Record({ 'rate' : IDL.Text }),
    'VaultWithdrawnAndClosed' : IDL.Record({
      'vault_id' : IDL.Nat64,
      'timestamp' : IDL.Nat64,
      'caller' : IDL.Principal,
      'amount' : IDL.Nat64,
    }),
    'claim_liquidity_returns' : IDL.Record({
      'block_index' : IDL.Nat64,
      'caller' : IDL.Principal,
      'amount' : IDL.Nat64,
    }),
    'collateral_withdrawn' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'amount' : IDL.Nat64,
    }),
    'repay_to_vault' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'repayed_amount' : IDL.Nat64,
    }),
    'provide_liquidity' : IDL.Record({
      'block_index' : IDL.Nat64,
      'caller' : IDL.Principal,
      'amount' : IDL.Nat64,
    }),
    'set_rmr_ceiling_cr' : IDL.Record({ 'value' : IDL.Text }),
    'set_recovery_rate_curve' : IDL.Record({ 'markers' : IDL.Text }),
    'set_ckstable_repay_fee' : IDL.Record({ 'rate' : IDL.Text }),
    'set_treasury_principal' : IDL.Record({ 'principal' : IDL.Principal }),
    'accrue_interest' : IDL.Record({ 'timestamp' : IDL.Nat64 }),
    'set_max_partial_liquidation_ratio' : IDL.Record({ 'rate' : IDL.Text }),
    'withdraw_and_close_vault' : IDL.Record({
      'block_index' : IDL.Opt(IDL.Nat64),
      'vault_id' : IDL.Nat64,
      'amount' : IDL.Nat64,
    }),
    'admin_vault_correction' : IDL.Record({
      'vault_id' : IDL.Nat64,
      'new_amount' : IDL.Nat64,
      'old_amount' : IDL.Nat64,
      'reason' : IDL.Text,
    }),
    'set_recovery_target_cr' : IDL.Record({ 'rate' : IDL.Text }),
    'init' : InitArg,
    'set_stable_ledger_principal' : IDL.Record({
      'principal' : IDL.Principal,
      'token_type' : StableTokenType,
    }),
    'open_vault' : IDL.Record({ 'block_index' : IDL.Nat64, 'vault' : Vault }),
    'redemption_on_vaults' : IDL.Record({
      'icusd_amount' : IDL.Nat64,
      'icusd_block_index' : IDL.Nat64,
      'owner' : IDL.Principal,
      'fee_amount' : IDL.Nat64,
      'current_icp_rate' : IDL.Vec(IDL.Nat8),
    }),
    'set_recovery_parameters' : IDL.Record({
      'recovery_interest_rate_apr' : IDL.Opt(IDL.Text),
      'recovery_borrowing_fee' : IDL.Opt(IDL.Text),
      'collateral_type' : IDL.Principal,
    }),
    'margin_transfer' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
    }),
    'admin_sweep_to_treasury' : IDL.Record({
      'block_index' : IDL.Nat64,
      'amount' : IDL.Nat64,
      'treasury' : IDL.Principal,
      'reason' : IDL.Text,
    }),
    'set_rmr_floor_cr' : IDL.Record({ 'value' : IDL.Text }),
    'set_rmr_ceiling' : IDL.Record({ 'value' : IDL.Text }),
    'upgrade' : UpgradeArg,
    'borrow_from_vault' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'fee_amount' : IDL.Nat64,
      'borrowed_amount' : IDL.Nat64,
    }),
    'set_reserve_redemptions_enabled' : IDL.Record({ 'enabled' : IDL.Bool }),
    'set_borrowing_fee_curve' : IDL.Record({ 'markers' : IDL.Text }),
    'set_interest_pool_share' : IDL.Record({ 'share' : IDL.Text }),
    'set_liquidation_protocol_share' : IDL.Record({ 'share' : IDL.Text }),
    'update_collateral_config' : IDL.Record({
      'config' : CollateralConfig,
      'collateral_type' : IDL.Principal,
    }),
    'redistribute_vault' : IDL.Record({ 'vault_id' : IDL.Nat64 }),
    'partial_collateral_withdrawn' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'amount' : IDL.Nat64,
    }),
    'set_rate_curve_markers' : IDL.Record({
      'markers' : IDL.Text,
      'collateral_type' : IDL.Opt(IDL.Text),
    }),
    'dust_forgiven' : VaultArg,
    'partial_liquidate_vault' : IDL.Record({
      'protocol_fee_collateral' : IDL.Opt(IDL.Nat64),
      'icp_rate' : IDL.Opt(IDL.Vec(IDL.Nat8)),
      'liquidator_payment' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'liquidator' : IDL.Opt(IDL.Principal),
      'icp_to_liquidator' : IDL.Nat64,
    }),
    'withdraw_liquidity' : IDL.Record({
      'block_index' : IDL.Nat64,
      'caller' : IDL.Principal,
      'amount' : IDL.Nat64,
    }),
    'admin_mint' : IDL.Record({
      'to' : IDL.Principal,
      'block_index' : IDL.Nat64,
      'amount' : IDL.Nat64,
      'reason' : IDL.Text,
    }),
    'set_three_pool_canister' : IDL.Record({ 'canister' : IDL.Principal }),
    'set_liquidation_bonus' : IDL.Record({ 'rate' : IDL.Text }),
    'reserve_redemption' : IDL.Record({
      'icusd_amount' : IDL.Nat64,
      'icusd_block_index' : IDL.Nat64,
      'fee_stable_amount' : IDL.Nat64,
      'owner' : IDL.Principal,
      'fee_amount' : IDL.Nat64,
      'stable_amount_sent' : IDL.Nat64,
      'stable_token_ledger' : IDL.Principal,
    }),
    'close_vault' : IDL.Record({
      'block_index' : IDL.Opt(IDL.Nat64),
      'vault_id' : IDL.Nat64,
    }),
    'update_collateral_status' : IDL.Record({
      'status' : CollateralStatus,
      'collateral_type' : IDL.Principal,
    }),
    'set_healthy_cr' : IDL.Record({
      'healthy_cr' : IDL.Opt(IDL.Text),
      'collateral_type' : IDL.Text,
    }),
    'set_redemption_fee_ceiling' : IDL.Record({ 'rate' : IDL.Text }),
    'add_margin_to_vault' : IDL.Record({
      'block_index' : IDL.Nat64,
      'vault_id' : IDL.Nat64,
      'margin_added' : IDL.Nat64,
    }),
    'set_stability_pool_principal' : IDL.Record({
      'principal' : IDL.Principal,
    }),
    'set_interest_split' : IDL.Record({ 'split' : IDL.Text }),
    'set_rmr_floor' : IDL.Record({ 'value' : IDL.Text }),
    'set_redemption_fee_floor' : IDL.Record({ 'rate' : IDL.Text }),
    'set_interest_rate' : IDL.Record({
      'collateral_type' : IDL.Principal,
      'interest_rate_apr' : IDL.Text,
    }),
    'set_reserve_redemption_fee' : IDL.Record({ 'fee' : IDL.Text }),
    'redemption_transfered' : IDL.Record({
      'icusd_block_index' : IDL.Nat64,
      'icp_block_index' : IDL.Nat64,
    }),
    'liquidate_vault' : IDL.Record({
      'mode' : Mode,
      'icp_rate' : IDL.Vec(IDL.Nat8),
      'vault_id' : IDL.Nat64,
      'liquidator' : IDL.Opt(IDL.Principal),
    }),
    'add_collateral_type' : IDL.Record({
      'config' : CollateralConfig,
      'collateral_type' : IDL.Principal,
    }),
    'set_stable_token_enabled' : IDL.Record({
      'enabled' : IDL.Bool,
      'token_type' : StableTokenType,
    }),
    'set_recovery_cr_multiplier' : IDL.Record({ 'multiplier' : IDL.Text }),
  });
  const Fees = IDL.Record({
    'redemption_fee' : IDL.Float64,
    'borrowing_fee' : IDL.Float64,
  });
  const InterestSplitArg = IDL.Record({
    'bps' : IDL.Nat64,
    'destination' : IDL.Text,
  });
  const LiquidityStatus = IDL.Record({
    'liquidity_provided' : IDL.Nat64,
    'total_liquidity_provided' : IDL.Nat64,
    'liquidity_pool_share' : IDL.Float64,
    'available_liquidity_reward' : IDL.Nat64,
    'total_available_returns' : IDL.Nat64,
  });
  const CollateralInterestInfo = IDL.Record({
    'total_debt_e8s' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
    'weighted_interest_rate' : IDL.Float64,
  });
  const ProtocolStatus = IDL.Record({
    'last_icp_timestamp' : IDL.Nat64,
    'borrowing_fee_curve_resolved' : IDL.Vec(
      IDL.Tuple(IDL.Float64, IDL.Float64)
    ),
    'recovery_mode_threshold' : IDL.Float64,
    'per_collateral_interest' : IDL.Vec(CollateralInterestInfo),
    'reserve_redemption_fee' : IDL.Float64,
    'mode' : Mode,
    'recovery_cr_multiplier' : IDL.Float64,
    'interest_pool_share' : IDL.Float64,
    'total_icusd_borrowed' : IDL.Nat64,
    'total_collateral_ratio' : IDL.Float64,
    'total_icp_margin' : IDL.Nat64,
    'recovery_target_cr' : IDL.Float64,
    'frozen' : IDL.Bool,
    'weighted_average_interest_rate' : IDL.Float64,
    'manual_mode_override' : IDL.Bool,
    'liquidation_bonus' : IDL.Float64,
    'reserve_redemptions_enabled' : IDL.Bool,
    'last_icp_rate' : IDL.Float64,
  });
  const ReserveBalance = IDL.Record({
    'balance' : IDL.Nat64,
    'ledger' : IDL.Principal,
    'symbol' : IDL.Text,
  });
  const StabilityPoolConfig = IDL.Record({
    'enabled' : IDL.Bool,
    'liquidation_discount' : IDL.Nat64,
    'stability_pool_canister' : IDL.Opt(IDL.Principal),
  });
  const TreasuryStats = IDL.Record({
    'pending_treasury_collateral_entries' : IDL.Nat64,
    'liquidation_protocol_share' : IDL.Float64,
    'interest_flush_threshold_e8s' : IDL.Nat64,
    'pending_treasury_interest' : IDL.Nat64,
    'treasury_principal' : IDL.Opt(IDL.Principal),
    'total_accrued_interest_system' : IDL.Nat64,
    'pending_interest_for_pools_total' : IDL.Nat64,
  });
  const Result_9 = IDL.Variant({ 'Ok' : IDL.Float64, 'Err' : ProtocolError });
  const HttpRequest = IDL.Record({
    'url' : IDL.Text,
    'method' : IDL.Text,
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(IDL.Tuple(IDL.Text, IDL.Text)),
  });
  const HttpResponse = IDL.Record({
    'body' : IDL.Vec(IDL.Nat8),
    'headers' : IDL.Vec(IDL.Tuple(IDL.Text, IDL.Text)),
    'status_code' : IDL.Nat16,
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
  const VaultArgWithToken = IDL.Record({
    'vault_id' : IDL.Nat64,
    'amount' : IDL.Nat64,
    'token_type' : StableTokenType,
  });
  const OpenVaultSuccess = IDL.Record({
    'block_index' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
  });
  const Result_5 = IDL.Variant({
    'Ok' : OpenVaultSuccess,
    'Err' : ProtocolError,
  });
  const Result_6 = IDL.Variant({ 'Ok' : IDL.Bool, 'Err' : ProtocolError });
  const ReserveRedemptionResult = IDL.Record({
    'icusd_block_index' : IDL.Nat64,
    'stable_token_used' : IDL.Principal,
    'vault_spillover_amount' : IDL.Nat64,
    'fee_amount' : IDL.Nat64,
    'stable_amount_sent' : IDL.Nat64,
  });
  const Result_7 = IDL.Variant({
    'Ok' : ReserveRedemptionResult,
    'Err' : ProtocolError,
  });
  const StabilityPoolLiquidationResult = IDL.Record({
    'fee' : IDL.Nat64,
    'collateral_price_e8s' : IDL.Nat64,
    'block_index' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'liquidated_debt' : IDL.Nat64,
    'success' : IDL.Bool,
    'collateral_type' : IDL.Text,
    'collateral_received' : IDL.Nat64,
  });
  const Result_8 = IDL.Variant({
    'Ok' : StabilityPoolLiquidationResult,
    'Err' : ProtocolError,
  });
  return IDL.Service({
    'add_collateral_token' : IDL.Func([AddCollateralArg], [Result], []),
    'add_margin_to_vault' : IDL.Func([VaultArg], [Result_1], []),
    'add_margin_with_deposit' : IDL.Func([IDL.Nat64], [Result_1], []),
    'admin_correct_vault_collateral' : IDL.Func(
        [IDL.Nat64, IDL.Nat64, IDL.Text],
        [Result],
        [],
      ),
    'admin_mint_icusd' : IDL.Func(
        [IDL.Nat64, IDL.Principal, IDL.Text],
        [Result_1],
        [],
      ),
    'admin_sweep_to_treasury' : IDL.Func([IDL.Text], [Result_1], []),
    'borrow_from_vault' : IDL.Func([VaultArg], [Result_2], []),
    'claim_liquidity_returns' : IDL.Func([], [Result_1], []),
    'clear_stuck_operations' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [Result_1],
        [],
      ),
    'close_vault' : IDL.Func([IDL.Nat64], [Result_3], []),
    'enter_recovery_mode' : IDL.Func([], [Result], []),
    'exit_recovery_mode' : IDL.Func([], [Result], []),
    'freeze_protocol' : IDL.Func([], [Result], []),
    'get_all_vaults' : IDL.Func([], [IDL.Vec(CandidVault)], ['query']),
    'get_borrowing_fee' : IDL.Func([], [IDL.Float64], ['query']),
    'get_ckstable_repay_fee' : IDL.Func([], [IDL.Float64], ['query']),
    'get_collateral_config' : IDL.Func(
        [IDL.Principal],
        [IDL.Opt(CollateralConfig)],
        ['query'],
      ),
    'get_collateral_totals' : IDL.Func(
        [],
        [IDL.Vec(CollateralTotals)],
        ['query'],
      ),
    'get_deposit_account' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [Account],
        ['query'],
      ),
    'get_events' : IDL.Func([GetEventsArg], [IDL.Vec(Event)], ['query']),
    'get_fees' : IDL.Func([IDL.Nat64], [Fees], ['query']),
    'get_interest_pool_share' : IDL.Func([], [IDL.Float64], ['query']),
    'get_interest_split' : IDL.Func([], [IDL.Vec(InterestSplitArg)], ['query']),
    'get_liquidatable_vaults' : IDL.Func([], [IDL.Vec(CandidVault)], ['query']),
    'get_liquidation_bonus' : IDL.Func([], [IDL.Float64], ['query']),
    'get_liquidation_protocol_share' : IDL.Func([], [IDL.Float64], ['query']),
    'get_liquidity_status' : IDL.Func(
        [IDL.Principal],
        [LiquidityStatus],
        ['query'],
      ),
    'get_max_partial_liquidation_ratio' : IDL.Func(
        [],
        [IDL.Float64],
        ['query'],
      ),
    'get_protocol_status' : IDL.Func([], [ProtocolStatus], ['query']),
    'get_recovery_cr_multiplier' : IDL.Func([], [IDL.Float64], ['query']),
    'get_recovery_target_cr' : IDL.Func([], [IDL.Float64], ['query']),
    'get_redemption_fee_ceiling' : IDL.Func([], [IDL.Float64], ['query']),
    'get_redemption_fee_floor' : IDL.Func([], [IDL.Float64], ['query']),
    'get_redemption_rate' : IDL.Func([], [IDL.Float64], ['query']),
    'get_reserve_balances' : IDL.Func([], [IDL.Vec(ReserveBalance)], ['query']),
    'get_reserve_redemption_fee' : IDL.Func([], [IDL.Float64], ['query']),
    'get_reserve_redemptions_enabled' : IDL.Func([], [IDL.Bool], ['query']),
    'get_rmr_ceiling' : IDL.Func([], [IDL.Float64], ['query']),
    'get_rmr_ceiling_cr' : IDL.Func([], [IDL.Float64], ['query']),
    'get_rmr_floor' : IDL.Func([], [IDL.Float64], ['query']),
    'get_rmr_floor_cr' : IDL.Func([], [IDL.Float64], ['query']),
    'get_stability_pool_config' : IDL.Func(
        [],
        [StabilityPoolConfig],
        ['query'],
      ),
    'get_stability_pool_principal' : IDL.Func(
        [],
        [IDL.Opt(IDL.Principal)],
        ['query'],
      ),
    'get_stable_token_enabled' : IDL.Func(
        [StableTokenType],
        [IDL.Bool],
        ['query'],
      ),
    'get_supported_collateral_types' : IDL.Func(
        [],
        [IDL.Vec(IDL.Tuple(IDL.Principal, CollateralStatus))],
        ['query'],
      ),
    'get_three_pool_canister' : IDL.Func(
        [],
        [IDL.Opt(IDL.Principal)],
        ['query'],
      ),
    'get_treasury_principal' : IDL.Func(
        [],
        [IDL.Opt(IDL.Principal)],
        ['query'],
      ),
    'get_treasury_stats' : IDL.Func([], [TreasuryStats], ['query']),
    'get_vault_history' : IDL.Func([IDL.Nat64], [IDL.Vec(Event)], ['query']),
    'get_vault_interest_rate' : IDL.Func([IDL.Nat64], [Result_9], ['query']),
    'get_vaults' : IDL.Func(
        [IDL.Opt(IDL.Principal)],
        [IDL.Vec(CandidVault)],
        ['query'],
      ),
    'http_request' : IDL.Func([HttpRequest], [HttpResponse], ['query']),
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
    'liquidate_vault' : IDL.Func([IDL.Nat64], [Result_2], []),
    'liquidate_vault_partial' : IDL.Func([VaultArg], [Result_2], []),
    'liquidate_vault_partial_with_stable' : IDL.Func(
        [VaultArgWithToken],
        [Result_2],
        [],
      ),
    'open_vault' : IDL.Func(
        [IDL.Nat64, IDL.Opt(IDL.Principal)],
        [Result_5],
        [],
      ),
    'open_vault_and_borrow' : IDL.Func(
        [IDL.Nat64, IDL.Nat64, IDL.Opt(IDL.Principal)],
        [Result_5],
        [],
      ),
    'open_vault_with_deposit' : IDL.Func(
        [IDL.Nat64, IDL.Opt(IDL.Principal)],
        [Result_5],
        [],
      ),
    'partial_liquidate_vault' : IDL.Func([VaultArg], [Result_2], []),
    'partial_repay_to_vault' : IDL.Func([VaultArg], [Result_1], []),
    'provide_liquidity' : IDL.Func([IDL.Nat64], [Result_1], []),
    'recover_pending_transfer' : IDL.Func([IDL.Nat64], [Result_6], []),
    'redeem_collateral' : IDL.Func([IDL.Principal, IDL.Nat64], [Result_2], []),
    'redeem_icp' : IDL.Func([IDL.Nat64], [Result_2], []),
    'redeem_reserves' : IDL.Func(
        [IDL.Nat64, IDL.Opt(IDL.Principal)],
        [Result_7],
        [],
      ),
    'repay_to_vault' : IDL.Func([VaultArg], [Result_1], []),
    'repay_to_vault_with_stable' : IDL.Func(
        [VaultArgWithToken],
        [Result_1],
        [],
      ),
    'set_borrowing_fee' : IDL.Func([IDL.Float64], [Result], []),
    'set_borrowing_fee_curve' : IDL.Func([IDL.Opt(IDL.Text)], [Result], []),
    'set_ckstable_repay_fee' : IDL.Func([IDL.Float64], [Result], []),
    'set_collateral_status' : IDL.Func(
        [IDL.Principal, CollateralStatus],
        [Result],
        [],
      ),
    'set_healthy_cr' : IDL.Func(
        [IDL.Principal, IDL.Opt(IDL.Float64)],
        [Result],
        [],
      ),
    'set_interest_flush_threshold' : IDL.Func([IDL.Nat64], [Result], []),
    'set_interest_pool_share' : IDL.Func([IDL.Float64], [Result], []),
    'set_interest_rate' : IDL.Func([IDL.Principal, IDL.Float64], [Result], []),
    'set_interest_split' : IDL.Func([IDL.Vec(InterestSplitArg)], [Result], []),
    'set_liquidation_bonus' : IDL.Func([IDL.Float64], [Result], []),
    'set_liquidation_protocol_share' : IDL.Func([IDL.Float64], [Result], []),
    'set_max_partial_liquidation_ratio' : IDL.Func([IDL.Float64], [Result], []),
    'set_rate_curve_markers' : IDL.Func(
        [IDL.Opt(IDL.Principal), IDL.Vec(IDL.Tuple(IDL.Float64, IDL.Float64))],
        [Result],
        [],
      ),
    'set_recovery_cr_multiplier' : IDL.Func([IDL.Float64], [Result], []),
    'set_recovery_parameters' : IDL.Func(
        [IDL.Principal, IDL.Opt(IDL.Float64), IDL.Opt(IDL.Float64)],
        [Result],
        [],
      ),
    'set_recovery_rate_curve' : IDL.Func(
        [IDL.Vec(IDL.Tuple(IDL.Text, IDL.Float64))],
        [Result],
        [],
      ),
    'set_recovery_target_cr' : IDL.Func([IDL.Float64], [Result], []),
    'set_redemption_fee_ceiling' : IDL.Func([IDL.Float64], [Result], []),
    'set_redemption_fee_floor' : IDL.Func([IDL.Float64], [Result], []),
    'set_reserve_redemption_fee' : IDL.Func([IDL.Float64], [Result], []),
    'set_reserve_redemptions_enabled' : IDL.Func([IDL.Bool], [Result], []),
    'set_rmr_ceiling' : IDL.Func([IDL.Float64], [Result], []),
    'set_rmr_ceiling_cr' : IDL.Func([IDL.Float64], [Result], []),
    'set_rmr_floor' : IDL.Func([IDL.Float64], [Result], []),
    'set_rmr_floor_cr' : IDL.Func([IDL.Float64], [Result], []),
    'set_stability_pool_principal' : IDL.Func([IDL.Principal], [Result], []),
    'set_stable_ledger_principal' : IDL.Func(
        [StableTokenType, IDL.Principal],
        [Result],
        [],
      ),
    'set_stable_token_enabled' : IDL.Func(
        [StableTokenType, IDL.Bool],
        [Result],
        [],
      ),
    'set_three_pool_canister' : IDL.Func([IDL.Principal], [Result], []),
    'set_treasury_principal' : IDL.Func([IDL.Principal], [Result], []),
    'stability_pool_liquidate' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [Result_8],
        [],
      ),
    'unfreeze_protocol' : IDL.Func([], [Result], []),
    'update_collateral_config' : IDL.Func(
        [IDL.Principal, CollateralConfig],
        [Result],
        [],
      ),
    'withdraw_and_close_vault' : IDL.Func([IDL.Nat64], [Result_3], []),
    'withdraw_collateral' : IDL.Func([IDL.Nat64], [Result_1], []),
    'withdraw_liquidity' : IDL.Func([IDL.Nat64], [Result_1], []),
    'withdraw_partial_collateral' : IDL.Func([VaultArg], [Result_1], []),
  });
};
export const init = ({ IDL }) => {
  const Mode = IDL.Variant({
    'ReadOnly' : IDL.Null,
    'GeneralAvailability' : IDL.Null,
    'Recovery' : IDL.Null,
  });
  const UpgradeArg = IDL.Record({ 'mode' : IDL.Opt(Mode) });
  const InitArg = IDL.Record({
    'ckusdc_ledger_principal' : IDL.Opt(IDL.Principal),
    'xrc_principal' : IDL.Principal,
    'icp_ledger_principal' : IDL.Principal,
    'fee_e8s' : IDL.Nat64,
    'ckusdt_ledger_principal' : IDL.Opt(IDL.Principal),
    'stability_pool_principal' : IDL.Opt(IDL.Principal),
    'treasury_principal' : IDL.Opt(IDL.Principal),
    'developer_principal' : IDL.Principal,
    'icusd_ledger_principal' : IDL.Principal,
  });
  const ProtocolArg = IDL.Variant({ 'Upgrade' : UpgradeArg, 'Init' : InitArg });
  return [ProtocolArg];
};
