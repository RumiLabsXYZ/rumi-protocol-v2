export const idlFactory = ({ IDL }) => {
  const TokenConfig = IDL.Record({
    'decimals' : IDL.Nat8,
    'precision_mul' : IDL.Nat64,
    'ledger_id' : IDL.Principal,
    'symbol' : IDL.Text,
  });
  const ThreePoolInitArgs = IDL.Record({
    'admin_fee_bps' : IDL.Nat64,
    'admin' : IDL.Principal,
    'swap_fee_bps' : IDL.Nat64,
    'initial_a' : IDL.Nat64,
    'tokens' : IDL.Vec(TokenConfig),
  });
  const ThreePoolError = IDL.Variant({
    'InsufficientOutput' : IDL.Record({
      'actual' : IDL.Nat,
      'expected_min' : IDL.Nat,
    }),
    'PoolPaused' : IDL.Null,
    'InvalidCoinIndex' : IDL.Null,
    'ZeroAmount' : IDL.Null,
    'MathOverflow' : IDL.Null,
    'Unauthorized' : IDL.Null,
    'InvariantNotConverged' : IDL.Null,
    'InsufficientLiquidity' : IDL.Null,
    'TransferFailed' : IDL.Record({ 'token' : IDL.Text, 'reason' : IDL.Text }),
    'SlippageExceeded' : IDL.Null,
    'PoolEmpty' : IDL.Null,
  });
  const PoolStatus = IDL.Record({
    'virtual_price' : IDL.Nat,
    'admin_fee_bps' : IDL.Nat64,
    'swap_fee_bps' : IDL.Nat64,
    'current_a' : IDL.Nat64,
    'tokens' : IDL.Vec(TokenConfig),
    'lp_total_supply' : IDL.Nat,
    'balances' : IDL.Vec(IDL.Nat),
  });
  const VirtualPriceSnapshot = IDL.Record({
    'virtual_price' : IDL.Nat,
    'timestamp_secs' : IDL.Nat64,
    'lp_total_supply' : IDL.Nat,
  });
  const StandardRecord = IDL.Record({ 'url' : IDL.Text, 'name' : IDL.Text });
  const Account = IDL.Record({
    'owner' : IDL.Principal,
    'subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const MetadataValue = IDL.Variant({
    'Int' : IDL.Int,
    'Nat' : IDL.Nat,
    'Blob' : IDL.Vec(IDL.Nat8),
    'Text' : IDL.Text,
  });
  const TransferArg = IDL.Record({
    'to' : Account,
    'fee' : IDL.Opt(IDL.Nat),
    'memo' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'created_at_time' : IDL.Opt(IDL.Nat64),
    'amount' : IDL.Nat,
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
  const Icrc28TrustedOriginsResponse = IDL.Record({
    'trusted_origins' : IDL.Vec(IDL.Text),
  });
  const AllowanceArgs = IDL.Record({
    'account' : Account,
    'spender' : Account,
  });
  const Allowance = IDL.Record({
    'allowance' : IDL.Nat,
    'expires_at' : IDL.Opt(IDL.Nat64),
  });
  const ApproveArgs = IDL.Record({
    'fee' : IDL.Opt(IDL.Nat),
    'memo' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'from_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'created_at_time' : IDL.Opt(IDL.Nat64),
    'amount' : IDL.Nat,
    'expected_allowance' : IDL.Opt(IDL.Nat),
    'expires_at' : IDL.Opt(IDL.Nat64),
    'spender' : Account,
  });
  const ApproveError = IDL.Variant({
    'GenericError' : IDL.Record({
      'message' : IDL.Text,
      'error_code' : IDL.Nat,
    }),
    'TemporarilyUnavailable' : IDL.Null,
    'Duplicate' : IDL.Record({ 'duplicate_of' : IDL.Nat }),
    'BadFee' : IDL.Record({ 'expected_fee' : IDL.Nat }),
    'AllowanceChanged' : IDL.Record({ 'current_allowance' : IDL.Nat }),
    'CreatedInFuture' : IDL.Record({ 'ledger_time' : IDL.Nat64 }),
    'TooOld' : IDL.Null,
    'Expired' : IDL.Record({ 'ledger_time' : IDL.Nat64 }),
    'InsufficientFunds' : IDL.Record({ 'balance' : IDL.Nat }),
  });
  const TransferFromArgs = IDL.Record({
    'to' : Account,
    'fee' : IDL.Opt(IDL.Nat),
    'spender_subaccount' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'from' : Account,
    'memo' : IDL.Opt(IDL.Vec(IDL.Nat8)),
    'created_at_time' : IDL.Opt(IDL.Nat64),
    'amount' : IDL.Nat,
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
  return IDL.Service({
    'add_liquidity' : IDL.Func(
        [IDL.Vec(IDL.Nat), IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        [],
      ),
    'calc_add_liquidity_query' : IDL.Func(
        [IDL.Vec(IDL.Nat), IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        ['query'],
      ),
    'calc_remove_liquidity_query' : IDL.Func(
        [IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Vec(IDL.Nat), 'Err' : ThreePoolError })],
        ['query'],
      ),
    'calc_remove_one_coin_query' : IDL.Func(
        [IDL.Nat, IDL.Nat8],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        ['query'],
      ),
    'calc_swap' : IDL.Func(
        [IDL.Nat8, IDL.Nat8, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        ['query'],
      ),
    'donate' : IDL.Func(
        [IDL.Nat8, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'get_admin_fees' : IDL.Func([], [IDL.Vec(IDL.Nat)], ['query']),
    'get_lp_balance' : IDL.Func([IDL.Principal], [IDL.Nat], ['query']),
    'get_pool_status' : IDL.Func([], [PoolStatus], ['query']),
    'get_vp_snapshots' : IDL.Func(
        [],
        [IDL.Vec(VirtualPriceSnapshot)],
        ['query'],
      ),
    'health' : IDL.Func([], [IDL.Text], ['query']),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc1_balance_of' : IDL.Func([Account], [IDL.Nat], ['query']),
    'icrc1_decimals' : IDL.Func([], [IDL.Nat8], ['query']),
    'icrc1_fee' : IDL.Func([], [IDL.Nat], ['query']),
    'icrc1_metadata' : IDL.Func(
        [],
        [IDL.Vec(IDL.Tuple(IDL.Text, MetadataValue))],
        ['query'],
      ),
    'icrc1_minting_account' : IDL.Func([], [IDL.Opt(Account)], ['query']),
    'icrc1_name' : IDL.Func([], [IDL.Text], ['query']),
    'icrc1_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
      ),
    'icrc1_symbol' : IDL.Func([], [IDL.Text], ['query']),
    'icrc1_total_supply' : IDL.Func([], [IDL.Nat], ['query']),
    'icrc1_transfer' : IDL.Func(
        [TransferArg],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : TransferError })],
        [],
      ),
    'icrc21_canister_call_consent_message' : IDL.Func(
        [ConsentMessageRequest],
        [IDL.Variant({ 'Ok' : ConsentInfo, 'Err' : Icrc21Error })],
        [],
      ),
    'icrc28_trusted_origins' : IDL.Func(
        [],
        [Icrc28TrustedOriginsResponse],
        ['query'],
      ),
    'icrc2_allowance' : IDL.Func([AllowanceArgs], [Allowance], ['query']),
    'icrc2_approve' : IDL.Func(
        [ApproveArgs],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ApproveError })],
        [],
      ),
    'icrc2_transfer_from' : IDL.Func(
        [TransferFromArgs],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : TransferFromError })],
        [],
      ),
    'ramp_a' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'remove_liquidity' : IDL.Func(
        [IDL.Nat, IDL.Vec(IDL.Nat)],
        [IDL.Variant({ 'Ok' : IDL.Vec(IDL.Nat), 'Err' : ThreePoolError })],
        [],
      ),
    'remove_one_coin' : IDL.Func(
        [IDL.Nat, IDL.Nat8, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        [],
      ),
    'set_admin_fee' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'set_paused' : IDL.Func(
        [IDL.Bool],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'set_swap_fee' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'stop_ramp_a' : IDL.Func(
        [],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'swap' : IDL.Func(
        [IDL.Nat8, IDL.Nat8, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        [],
      ),
    'withdraw_admin_fees' : IDL.Func(
        [],
        [IDL.Variant({ 'Ok' : IDL.Vec(IDL.Nat), 'Err' : ThreePoolError })],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const TokenConfig = IDL.Record({
    'decimals' : IDL.Nat8,
    'precision_mul' : IDL.Nat64,
    'ledger_id' : IDL.Principal,
    'symbol' : IDL.Text,
  });
  const ThreePoolInitArgs = IDL.Record({
    'admin_fee_bps' : IDL.Nat64,
    'admin' : IDL.Principal,
    'swap_fee_bps' : IDL.Nat64,
    'initial_a' : IDL.Nat64,
    'tokens' : IDL.Vec(TokenConfig),
  });
  return [ThreePoolInitArgs];
};
