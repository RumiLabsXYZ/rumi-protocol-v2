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
    'ClaimNotFound' : IDL.Null,
  });
  const CurveType = IDL.Variant({ 'ConstantProduct' : IDL.Null });
  const CreatePoolArgs = IDL.Record({
    'token_a' : IDL.Principal,
    'token_b' : IDL.Principal,
    'curve' : CurveType,
    'fee_bps' : IDL.Nat16,
  });
  const PendingClaim = IDL.Record({
    'id' : IDL.Nat64,
    'token' : IDL.Principal,
    'claimant' : IDL.Principal,
    'subaccount' : IDL.Vec(IDL.Nat8),
    'created_at' : IDL.Nat64,
    'pool_id' : IDL.Text,
    'amount' : IDL.Nat,
    'reason' : IDL.Text,
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
  const Icrc21Error = IDL.Variant({
    'GenericError' : IDL.Record({
      'description' : IDL.Text,
      'error_code' : IDL.Nat64,
    }),
    'UnsupportedCanisterCall' : IDL.Record({ 'description' : IDL.Text }),
    'ConsentMessageUnavailable' : IDL.Record({ 'description' : IDL.Text }),
  });
  const Icrc28TrustedOriginsResponse = IDL.Record({
    'trusted_origins' : IDL.Vec(IDL.Text),
  });
  const AmmSwapEvent = IDL.Record({
    'id' : IDL.Nat64,
    'caller' : IDL.Principal,
    'pool_id' : IDL.Text,
    'token_in' : IDL.Principal,
    'amount_in' : IDL.Nat,
    'token_out' : IDL.Principal,
    'amount_out' : IDL.Nat,
    'fee' : IDL.Nat,
    'timestamp' : IDL.Nat64,
  });
  const AmmLiquidityAction = IDL.Variant({
    'AddLiquidity' : IDL.Null,
    'RemoveLiquidity' : IDL.Null,
  });
  const AmmLiquidityEvent = IDL.Record({
    'id' : IDL.Nat64,
    'caller' : IDL.Principal,
    'pool_id' : IDL.Text,
    'action' : AmmLiquidityAction,
    'token_a' : IDL.Principal,
    'amount_a' : IDL.Nat,
    'token_b' : IDL.Principal,
    'amount_b' : IDL.Nat,
    'lp_shares' : IDL.Nat,
    'timestamp' : IDL.Nat64,
  });
  const AmmAdminAction = IDL.Variant({
    'CreatePool' : IDL.Record({ 'pool_id' : IDL.Text, 'token_a' : IDL.Principal, 'token_b' : IDL.Principal, 'fee_bps' : IDL.Nat16 }),
    'SetFee' : IDL.Record({ 'pool_id' : IDL.Text, 'fee_bps' : IDL.Nat16 }),
    'SetProtocolFee' : IDL.Record({ 'pool_id' : IDL.Text, 'protocol_fee_bps' : IDL.Nat16 }),
    'WithdrawProtocolFees' : IDL.Record({ 'pool_id' : IDL.Text, 'amount_a' : IDL.Nat, 'amount_b' : IDL.Nat }),
    'PausePool' : IDL.Record({ 'pool_id' : IDL.Text }),
    'UnpausePool' : IDL.Record({ 'pool_id' : IDL.Text }),
    'SetPoolCreationOpen' : IDL.Record({ 'open' : IDL.Bool }),
    'SetMaintenanceMode' : IDL.Record({ 'enabled' : IDL.Bool }),
    'ClaimPending' : IDL.Record({ 'claim_id' : IDL.Nat64, 'claimant' : IDL.Principal, 'amount' : IDL.Nat }),
    'ResolvePendingClaim' : IDL.Record({ 'claim_id' : IDL.Nat64 }),
  });
  const AmmAdminEvent = IDL.Record({
    'id' : IDL.Nat64,
    'caller' : IDL.Principal,
    'action' : AmmAdminAction,
    'timestamp' : IDL.Nat64,
  });
  const SwapResult = IDL.Record({ 'fee' : IDL.Nat, 'amount_out' : IDL.Nat });
  return IDL.Service({
    'add_liquidity' : IDL.Func(
        [IDL.Text, IDL.Nat, IDL.Nat, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        [],
      ),
    'claim_pending' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
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
    'get_pending_claims' : IDL.Func([], [IDL.Vec(PendingClaim)], ['query']),
    'get_pool' : IDL.Func([IDL.Text], [IDL.Opt(PoolInfo)], ['query']),
    'get_pools' : IDL.Func([], [IDL.Vec(PoolInfo)], ['query']),
    'get_quote' : IDL.Func(
        [IDL.Text, IDL.Principal, IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : AmmError })],
        ['query'],
      ),
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
    'resolve_pending_claim' : IDL.Func(
        [IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : AmmError })],
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
    'get_amm_swap_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(AmmSwapEvent)],
        ['query'],
      ),
    'get_amm_swap_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_amm_liquidity_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(AmmLiquidityEvent)],
        ['query'],
      ),
    'get_amm_liquidity_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_amm_admin_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(AmmAdminEvent)],
        ['query'],
      ),
    'get_amm_admin_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
  });
};
export const init = ({ IDL }) => {
  const AmmInitArgs = IDL.Record({ 'admin' : IDL.Principal });
  return [AmmInitArgs];
};
