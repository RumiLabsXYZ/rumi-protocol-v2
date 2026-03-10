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
  const Icrc28TrustedOriginsResponse = IDL.Record({
    'trusted_origins' : IDL.Vec(IDL.Text),
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
    'get_admin_fees' : IDL.Func([], [IDL.Vec(IDL.Nat)], ['query']),
    'get_lp_balance' : IDL.Func([IDL.Principal], [IDL.Nat], ['query']),
    'get_pool_status' : IDL.Func([], [PoolStatus], ['query']),
    'health' : IDL.Func([], [IDL.Text], ['query']),
    'icrc10_supported_standards' : IDL.Func(
        [],
        [IDL.Vec(StandardRecord)],
        ['query'],
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
    'set_paused' : IDL.Func(
        [IDL.Bool],
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
