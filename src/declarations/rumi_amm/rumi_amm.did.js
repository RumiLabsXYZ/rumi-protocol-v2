export const idlFactory = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin' : IDL.Principal });
  const CurveType = IDL.Variant({ 'ConstantProduct' : IDL.Null });
  const SwapResult = IDL.Record({
    'amount_out' : IDL.Nat,
    'fee' : IDL.Nat,
  });
  const AmmError = IDL.Variant({
    'PoolNotFound' : IDL.Null,
    'PoolAlreadyExists' : IDL.Null,
    'PoolPaused' : IDL.Null,
    'ZeroAmount' : IDL.Null,
    'InsufficientOutput' : IDL.Record({
      'expected_min' : IDL.Nat,
      'actual' : IDL.Nat,
    }),
    'InsufficientLiquidity' : IDL.Null,
    'InsufficientLpShares' : IDL.Record({
      'required' : IDL.Nat,
      'available' : IDL.Nat,
    }),
    'InvalidToken' : IDL.Null,
    'TransferFailed' : IDL.Record({ 'token' : IDL.Text, 'reason' : IDL.Text }),
    'Unauthorized' : IDL.Null,
    'MathOverflow' : IDL.Null,
    'DisproportionateLiquidity' : IDL.Null,
  });
  const PoolInfo = IDL.Record({
    'pool_id' : IDL.Text,
    'token_a' : IDL.Principal,
    'token_b' : IDL.Principal,
    'reserve_a' : IDL.Nat,
    'reserve_b' : IDL.Nat,
    'fee_bps' : IDL.Nat16,
    'protocol_fee_bps' : IDL.Nat16,
    'curve' : CurveType,
    'total_lp_shares' : IDL.Nat,
    'paused' : IDL.Bool,
  });
  const CreatePoolArgs = IDL.Record({
    'token_a' : IDL.Principal,
    'token_b' : IDL.Principal,
    'fee_bps' : IDL.Nat16,
    'curve' : CurveType,
  });
  return IDL.Service({
    'health' : IDL.Func([], [IDL.Text], ['query']),
    'swap' : IDL.Func(
        [IDL.Text, IDL.Principal, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : SwapResult, 'Err' : AmmError })],
        [],
      ),
    'add_liquidity' : IDL.Func(
        [IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        [],
      ),
    'remove_liquidity' : IDL.Func(
        [IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Tuple(IDL.Nat, IDL.Nat), 'Err' : AmmError })],
        [],
      ),
    'get_pool' : IDL.Func([IDL.Text], [IDL.Opt(PoolInfo)], ['query']),
    'get_pools' : IDL.Func([], [IDL.Vec(PoolInfo)], ['query']),
    'get_quote' : IDL.Func(
        [IDL.Text, IDL.Principal, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        ['query'],
      ),
    'get_lp_balance' : IDL.Func(
        [IDL.Text, IDL.Principal],
        [IDL.Nat],
        ['query'],
      ),
    'create_pool' : IDL.Func(
        [CreatePoolArgs],
        [IDL.Variant({ 'Ok' : IDL.Text, 'Err' : AmmError })],
        [],
      ),
    'set_fee' : IDL.Func(
        [IDL.Text, IDL.Nat16],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'set_protocol_fee' : IDL.Func(
        [IDL.Text, IDL.Nat16],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'withdraw_protocol_fees' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Tuple(IDL.Nat, IDL.Nat), 'Err' : AmmError })],
        [],
      ),
    'pause_pool' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
    'unpause_pool' : IDL.Func(
        [IDL.Text],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin' : IDL.Principal });
  return [AmmInitArgs];
};
