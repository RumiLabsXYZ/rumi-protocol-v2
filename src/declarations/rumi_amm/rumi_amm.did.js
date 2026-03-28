export const idlFactory = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin' : IDL.Principal });
  const AmmError = IDL.Variant({
    'InsufficientOutput' : IDL.Record({
      'actual' : IDL.Nat,
      'expected_min' : IDL.Nat,
    }),
    'PoolPaused' : IDL.Null,
    'PoolCreationClosed' : IDL.Null,
    'PoolNotFound' : IDL.Null,
    'ZeroAmount' : IDL.Null,
    'DisproportionateLiquidity' : IDL.Null,
    'FeeBpsOutOfRange' : IDL.Null,
    'InvalidToken' : IDL.Null,
    'InsufficientLpShares' : IDL.Record({
      'available' : IDL.Nat,
      'required' : IDL.Nat,
    }),
    'MathOverflow' : IDL.Null,
    'Unauthorized' : IDL.Null,
    'PoolAlreadyExists' : IDL.Null,
    'InsufficientLiquidity' : IDL.Null,
    'MaintenanceMode' : IDL.Null,
    'TransferFailed' : IDL.Record({ 'token' : IDL.Text, 'reason' : IDL.Text }),
  });
  const CurveType = IDL.Variant({ 'ConstantProduct' : IDL.Null });
  const CreatePoolArgs = IDL.Record({
    'token_a' : IDL.Principal,
    'token_b' : IDL.Principal,
    'curve' : CurveType,
    'fee_bps' : IDL.Nat16,
  });
  const PoolInfo = IDL.Record({
    'token_a' : IDL.Principal,
    'token_b' : IDL.Principal,
    'curve' : CurveType,
    'fee_bps' : IDL.Nat16,
    'reserve_a' : IDL.Nat,
    'reserve_b' : IDL.Nat,
    'total_lp_shares' : IDL.Nat,
    'pool_id' : IDL.Text,
    'protocol_fee_bps' : IDL.Nat16,
    'paused' : IDL.Bool,
  });
  const SwapResult = IDL.Record({ 'fee' : IDL.Nat, 'amount_out' : IDL.Nat });
  return IDL.Service({
    'add_liquidity' : IDL.Func(
        [IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        [],
      ),
    'create_pool' : IDL.Func(
        [CreatePoolArgs],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : AmmError })],
        [],
      ),
    'get_lp_balance' : IDL.Func(
        [IDL.Text, IDL.Principal],
        [IDL.Nat],
        ['query'],
      ),
    'get_pool' : IDL.Func([IDL.Text], [IDL.Opt(PoolInfo)], ['query']),
    'get_pools' : IDL.Func([], [IDL.Vec(PoolInfo)], ['query']),
    'get_quote' : IDL.Func(
        [IDL.Text, IDL.Principal, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        ['query'],
      ),
    'health' : IDL.Func([], [IDL.Text], ['query']),
    'is_maintenance_mode' : IDL.Func([], [IDL.Bool], ['query']),
    'is_pool_creation_open' : IDL.Func([], [IDL.Bool], ['query']),
    'pause_pool' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'remove_liquidity' : IDL.Func(
        [IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Tuple(IDL.Nat, IDL.Nat), 'Err' : AmmError })],
        [],
      ),
    'set_fee' : IDL.Func(
        [IDL.Text, IDL.Nat16],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'set_maintenance_mode' : IDL.Func(
        [IDL.Bool],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'set_pool_creation_open' : IDL.Func(
        [IDL.Bool],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'set_protocol_fee' : IDL.Func(
        [IDL.Text, IDL.Nat16],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'swap' : IDL.Func(
        [IDL.Text, IDL.Principal, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : SwapResult, 'Err' : AmmError })],
        [],
      ),
    'unpause_pool' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'withdraw_protocol_fees' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Tuple(IDL.Nat, IDL.Nat), 'Err' : AmmError })],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin' : IDL.Principal });
  return [AmmInitArgs];
};
