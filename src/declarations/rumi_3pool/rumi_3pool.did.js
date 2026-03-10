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
