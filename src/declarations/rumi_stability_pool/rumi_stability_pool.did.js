export const idlFactory = ({ IDL }) => {
  const StabilityPoolInitArgs = IDL.Record({
    'protocol_canister_id' : IDL.Principal,
    'authorized_admins' : IDL.Vec(IDL.Principal),
  });
  const StabilityPoolError = IDL.Variant({
    'LedgerTransferFailed' : IDL.Record({ 'reason' : IDL.Text }),
    'EmergencyPaused' : IDL.Null,
    'AlreadyOptedOut' : IDL.Record({ 'collateral' : IDL.Principal }),
    'TokenNotActive' : IDL.Record({ 'ledger' : IDL.Principal }),
    'InsufficientBalance' : IDL.Record({
      'token' : IDL.Principal,
      'available' : IDL.Nat64,
      'required' : IDL.Nat64,
    }),
    'CollateralNotFound' : IDL.Record({ 'ledger' : IDL.Principal }),
    'NoPositionFound' : IDL.Null,
    'AmountTooLow' : IDL.Record({ 'minimum_e8s' : IDL.Nat64 }),
    'Unauthorized' : IDL.Null,
    'InterCanisterCallFailed' : IDL.Record({
      'method' : IDL.Text,
      'target' : IDL.Text,
    }),
    'LiquidationFailed' : IDL.Record({
      'vault_id' : IDL.Nat64,
      'reason' : IDL.Text,
    }),
    'SystemBusy' : IDL.Null,
    'AlreadyOptedIn' : IDL.Record({ 'collateral' : IDL.Principal }),
    'TokenNotAccepted' : IDL.Record({ 'ledger' : IDL.Principal }),
    'InsufficientPoolBalance' : IDL.Null,
  });
  const LiquidationResult = IDL.Record({
    'error_message' : IDL.Opt(IDL.Text),
    'stables_consumed' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
    'vault_id' : IDL.Nat64,
    'collateral_gained' : IDL.Nat64,
    'success' : IDL.Bool,
    'collateral_type' : IDL.Principal,
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
  const StablecoinConfig = IDL.Record({
    'decimals' : IDL.Nat8,
    'transfer_fee' : IDL.Opt(IDL.Nat64),
    'ledger_id' : IDL.Principal,
    'priority' : IDL.Nat8,
    'is_active' : IDL.Bool,
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
    'total_claimed_gains' : IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
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
  const PoolConfiguration = IDL.Record({
    'emergency_pause' : IDL.Bool,
    'min_deposit_e8s' : IDL.Nat64,
    'authorized_admins' : IDL.Vec(IDL.Principal),
    'max_liquidations_per_batch' : IDL.Nat64,
  });
  return IDL.Service({
    'admin_correct_balance' : IDL.Func(
        [IDL.Principal, IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : StabilityPoolError })],
        [],
      ),
    'admin_reset_token_failures' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : StabilityPoolError })],
        [],
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
    'claim_collateral' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : StabilityPoolError })],
        [],
      ),
    'deposit' : IDL.Func(
        [IDL.Principal, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : StabilityPoolError })],
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
    'get_liquidation_history' : IDL.Func(
        [IDL.Opt(IDL.Nat64)],
        [IDL.Vec(PoolLiquidationRecord)],
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
    'notify_liquidatable_vaults' : IDL.Func(
        [IDL.Vec(LiquidatableVaultInfo)],
        [IDL.Vec(LiquidationResult)],
        [],
      ),
    'opt_in_collateral' : IDL.Func(
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
