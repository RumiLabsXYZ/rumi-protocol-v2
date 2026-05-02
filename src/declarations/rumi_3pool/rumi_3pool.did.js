export const idlFactory = ({ IDL }) => {
  const GetBlocksResult = IDL.Rec();
  const Icrc3Value = IDL.Rec();
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
    'BurnSlippageExceeded' : IDL.Record({
      'actual_bps' : IDL.Nat16,
      'max_bps' : IDL.Nat16,
    }),
    'NotAuthorizedBurnCaller' : IDL.Null,
    'ZeroAmount' : IDL.Null,
    'InsufficientLpBalance' : IDL.Record({
      'available' : IDL.Nat,
      'required' : IDL.Nat,
    }),
    'BurnFailed' : IDL.Record({ 'token' : IDL.Text, 'reason' : IDL.Text }),
    'MathOverflow' : IDL.Null,
    'Unauthorized' : IDL.Null,
    'InvariantNotConverged' : IDL.Null,
    'InsufficientLiquidity' : IDL.Null,
    'TransferFailed' : IDL.Record({ 'token' : IDL.Text, 'reason' : IDL.Text }),
    'SlippageExceeded' : IDL.Null,
    'PoolEmpty' : IDL.Null,
    'PoolLocked' : IDL.Null,
    'InsufficientPoolBalance' : IDL.Record({
      'token' : IDL.Text,
      'available' : IDL.Nat,
      'required' : IDL.Nat,
    }),
  });
  const AuthorizedRedeemAndBurnArgs = IDL.Record({
    'token_amount' : IDL.Nat,
    'lp_amount' : IDL.Nat,
    'max_slippage_bps' : IDL.Nat16,
    'token_ledger' : IDL.Principal,
  });
  const RedeemAndBurnResult = IDL.Record({
    'lp_amount_burned' : IDL.Nat,
    'burn_block_index' : IDL.Nat64,
    'token_amount_burned' : IDL.Nat,
  });
  const FeeCurveParams = IDL.Record({
    'max_fee_bps' : IDL.Nat16,
    'imb_saturation' : IDL.Nat64,
    'min_fee_bps' : IDL.Nat16,
  });
  const ThreePoolAdminAction = IDL.Variant({
    'SetAdminFee' : IDL.Record({ 'fee_bps' : IDL.Nat64 }),
    'RampA' : IDL.Record({
      'future_a_time' : IDL.Nat64,
      'future_a' : IDL.Nat64,
    }),
    'StopRampA' : IDL.Record({ 'frozen_a' : IDL.Nat64 }),
    'RemoveAuthorizedBurnCaller' : IDL.Record({ 'canister' : IDL.Principal }),
    'WithdrawAdminFees' : IDL.Record({ 'amounts' : IDL.Vec(IDL.Nat) }),
    'SetSwapFee' : IDL.Record({ 'fee_bps' : IDL.Nat64 }),
    'FeeCurveParamsUpdated' : IDL.Record({
      'new' : FeeCurveParams,
      'old' : IDL.Opt(FeeCurveParams),
    }),
    'AddAuthorizedBurnCaller' : IDL.Record({ 'canister' : IDL.Principal }),
    'SetPaused' : IDL.Record({ 'paused' : IDL.Bool }),
  });
  const ThreePoolAdminEvent = IDL.Record({
    'id' : IDL.Nat64,
    'action' : ThreePoolAdminAction,
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
  });
  const StatsWindow = IDL.Variant({
    'AllTime' : IDL.Null,
    'Last7d' : IDL.Null,
    'Last24h' : IDL.Null,
    'Last30d' : IDL.Null,
  });
  const BalancePoint = IDL.Record({
    'timestamp' : IDL.Nat64,
    'balances' : IDL.Vec(IDL.Nat),
  });
  const FeePoint = IDL.Record({
    'timestamp' : IDL.Nat64,
    'avg_fee_bps' : IDL.Nat32,
  });
  const FeeBucket = IDL.Record({
    'volume_per_token' : IDL.Vec(IDL.Nat),
    'min_bps' : IDL.Nat16,
    'swap_count' : IDL.Nat64,
    'max_bps' : IDL.Nat16,
  });
  const FeeStats = IDL.Record({
    'rebalancing_swap_count' : IDL.Nat64,
    'rebalancing_swap_pct' : IDL.Nat32,
    'buckets' : IDL.Vec(FeeBucket),
  });
  const ImbalanceEventKind = IDL.Variant({
    'Swap' : IDL.Null,
    'Liquidity' : IDL.Null,
  });
  const ImbalanceSnapshot = IDL.Record({
    'imbalance_after' : IDL.Nat64,
    'virtual_price_after' : IDL.Nat,
    'timestamp' : IDL.Nat64,
    'event_kind' : ImbalanceEventKind,
  });
  const ImbalanceStats = IDL.Record({
    'avg' : IDL.Nat64,
    'max' : IDL.Nat64,
    'min' : IDL.Nat64,
    'samples' : IDL.Vec(IDL.Tuple(IDL.Nat64, IDL.Nat64)),
    'current' : IDL.Nat64,
  });
  const LiquidityAction = IDL.Variant({
    'AddLiquidity' : IDL.Null,
    'Donate' : IDL.Null,
    'RemoveOneCoin' : IDL.Null,
    'RemoveLiquidity' : IDL.Null,
  });
  const LiquidityEvent = IDL.Record({
    'id' : IDL.Nat64,
    'fee' : IDL.Opt(IDL.Nat),
    'action' : LiquidityAction,
    'lp_amount' : IDL.Nat,
    'amounts' : IDL.Vec(IDL.Nat),
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
    'coin_index' : IDL.Opt(IDL.Nat8),
  });
  const LiquidityEventV2 = IDL.Record({
    'id' : IDL.Nat64,
    'fee' : IDL.Opt(IDL.Nat),
    'action' : LiquidityAction,
    'imbalance_after' : IDL.Nat64,
    'is_rebalancing' : IDL.Bool,
    'virtual_price_after' : IDL.Nat,
    'lp_amount' : IDL.Nat,
    'pool_balances_after' : IDL.Vec(IDL.Nat),
    'fee_bps' : IDL.Opt(IDL.Nat16),
    'imbalance_before' : IDL.Nat64,
    'migrated' : IDL.Bool,
    'amounts' : IDL.Vec(IDL.Nat),
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
    'coin_index' : IDL.Opt(IDL.Nat8),
  });
  const PoolHealth = IDL.Record({
    'imbalance_trend_1h' : IDL.Int32,
    'current_imbalance' : IDL.Nat64,
    'fee_at_max_imbalance_swap' : IDL.Nat16,
    'last_swap_age_seconds' : IDL.Nat64,
    'arb_opportunity_score' : IDL.Nat8,
    'fee_at_min' : IDL.Nat16,
  });
  const PoolStateView = IDL.Record({
    'amp' : IDL.Nat64,
    'imbalance' : IDL.Nat64,
    'virtual_price' : IDL.Nat,
    'fee_curve' : FeeCurveParams,
    'lp_total_supply' : IDL.Nat,
    'balances' : IDL.Vec(IDL.Nat),
    'normalized_balances' : IDL.Vec(IDL.Nat),
  });
  const PoolStats = IDL.Record({
    'liquidity_added_count' : IDL.Nat64,
    'liquidity_removed_count' : IDL.Nat64,
    'arb_swap_count' : IDL.Nat64,
    'swap_volume_per_token' : IDL.Vec(IDL.Nat),
    'swap_count' : IDL.Nat64,
    'total_fees_collected' : IDL.Vec(IDL.Nat),
    'arb_volume_per_token' : IDL.Vec(IDL.Nat),
    'avg_fee_bps' : IDL.Nat32,
    'unique_swappers' : IDL.Nat64,
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
  const SwapEvent = IDL.Record({
    'id' : IDL.Nat64,
    'fee' : IDL.Nat,
    'token_in' : IDL.Nat8,
    'amount_out' : IDL.Nat,
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
    'amount_in' : IDL.Nat,
    'token_out' : IDL.Nat8,
  });
  const SwapEventV2 = IDL.Record({
    'id' : IDL.Nat64,
    'fee' : IDL.Nat,
    'imbalance_after' : IDL.Nat64,
    'token_in' : IDL.Nat8,
    'is_rebalancing' : IDL.Bool,
    'virtual_price_after' : IDL.Nat,
    'pool_balances_after' : IDL.Vec(IDL.Nat),
    'fee_bps' : IDL.Nat16,
    'imbalance_before' : IDL.Nat64,
    'migrated' : IDL.Bool,
    'amount_out' : IDL.Nat,
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Principal,
    'amount_in' : IDL.Nat,
    'token_out' : IDL.Nat8,
  });
  const VirtualPricePoint = IDL.Record({
    'virtual_price' : IDL.Nat,
    'timestamp' : IDL.Nat64,
  });
  const VolumePoint = IDL.Record({
    'volume_per_token' : IDL.Vec(IDL.Nat),
    'timestamp' : IDL.Nat64,
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
  const GetArchivesArgs = IDL.Record({ 'from' : IDL.Opt(IDL.Principal) });
  const ArchiveInfo = IDL.Record({
    'end' : IDL.Nat,
    'canister_id' : IDL.Principal,
    'start' : IDL.Nat,
  });
  const GetArchivesResult = IDL.Record({ 'archives' : IDL.Vec(ArchiveInfo) });
  const GetBlocksArgs = IDL.Record({ 'start' : IDL.Nat, 'length' : IDL.Nat });
  Icrc3Value.fill(
    IDL.Variant({
      'Int' : IDL.Int,
      'Map' : IDL.Vec(IDL.Tuple(IDL.Text, Icrc3Value)),
      'Nat' : IDL.Nat,
      'Blob' : IDL.Vec(IDL.Nat8),
      'Text' : IDL.Text,
      'Array' : IDL.Vec(Icrc3Value),
    })
  );
  const BlockWithId = IDL.Record({ 'id' : IDL.Nat, 'block' : Icrc3Value });
  const ArchivedBlocksCallback = IDL.Func(
      [IDL.Vec(GetBlocksArgs)],
      [GetBlocksResult],
      ['query'],
    );
  const ArchivedBlocks = IDL.Record({
    'args' : IDL.Vec(GetBlocksArgs),
    'callback' : ArchivedBlocksCallback,
  });
  GetBlocksResult.fill(
    IDL.Record({
      'log_length' : IDL.Nat,
      'blocks' : IDL.Vec(BlockWithId),
      'archived_blocks' : IDL.Vec(ArchivedBlocks),
    })
  );
  const Icrc3DataCertificate = IDL.Record({
    'certificate' : IDL.Vec(IDL.Nat8),
    'hash_tree' : IDL.Vec(IDL.Nat8),
  });
  const SupportedBlockType = IDL.Record({
    'url' : IDL.Text,
    'block_type' : IDL.Text,
  });
  const OptimalRebalanceQuote = IDL.Record({
    'dx' : IDL.Nat,
    'imbalance_after' : IDL.Nat64,
    'token_in' : IDL.Nat8,
    'profit_bps_estimate' : IDL.Nat64,
    'fee_bps' : IDL.Nat16,
    'imbalance_before' : IDL.Nat64,
    'amount_out' : IDL.Nat,
    'token_out' : IDL.Nat8,
  });
  const QuoteSwapResult = IDL.Record({
    'imbalance_after' : IDL.Nat64,
    'token_in' : IDL.Nat8,
    'is_rebalancing' : IDL.Bool,
    'virtual_price_after' : IDL.Nat,
    'fee_bps' : IDL.Nat16,
    'imbalance_before' : IDL.Nat64,
    'virtual_price_before' : IDL.Nat,
    'amount_out' : IDL.Nat,
    'amount_in' : IDL.Nat,
    'token_out' : IDL.Nat8,
    'fee_native' : IDL.Nat,
  });
  return IDL.Service({
    'add_authorized_burn_caller' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'add_liquidity' : IDL.Func(
        [IDL.Vec(IDL.Nat), IDL.Nat],
        [IDL.Variant({ 'Ok' : IDL.Nat, 'Err' : ThreePoolError })],
        [],
      ),
    'authorized_redeem_and_burn' : IDL.Func(
        [AuthorizedRedeemAndBurnArgs],
        [IDL.Variant({ 'Ok' : RedeemAndBurnResult, 'Err' : ThreePoolError })],
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
    'get_admin_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_admin_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(ThreePoolAdminEvent)],
        ['query'],
      ),
    'get_admin_fees' : IDL.Func([], [IDL.Vec(IDL.Nat)], ['query']),
    'get_all_lp_holders' : IDL.Func(
        [],
        [IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat))],
        ['query'],
      ),
    'get_authorized_burn_callers' : IDL.Func(
        [],
        [IDL.Vec(IDL.Principal)],
        ['query'],
      ),
    'get_balance_series' : IDL.Func(
        [StatsWindow, IDL.Nat64],
        [IDL.Vec(BalancePoint)],
        ['query'],
      ),
    'get_fee_curve_params' : IDL.Func([], [FeeCurveParams], ['query']),
    'get_fee_series' : IDL.Func(
        [StatsWindow, IDL.Nat64],
        [IDL.Vec(FeePoint)],
        ['query'],
      ),
    'get_fee_stats' : IDL.Func([StatsWindow], [FeeStats], ['query']),
    'get_imbalance_history' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(ImbalanceSnapshot)],
        ['query'],
      ),
    'get_imbalance_stats' : IDL.Func(
        [StatsWindow],
        [ImbalanceStats],
        ['query'],
      ),
    'get_liquidity_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_liquidity_event_count_v2' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_liquidity_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(LiquidityEvent)],
        ['query'],
      ),
    'get_liquidity_events_by_principal' : IDL.Func(
        [IDL.Principal, IDL.Nat64, IDL.Nat64],
        [IDL.Vec(LiquidityEventV2)],
        ['query'],
      ),
    'get_liquidity_events_v2' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(LiquidityEventV2)],
        ['query'],
      ),
    'get_lp_balance' : IDL.Func([IDL.Principal], [IDL.Nat], ['query']),
    'get_pool_health' : IDL.Func([], [PoolHealth], ['query']),
    'get_pool_state' : IDL.Func([], [PoolStateView], ['query']),
    'get_pool_stats' : IDL.Func([StatsWindow], [PoolStats], ['query']),
    'get_pool_status' : IDL.Func([], [PoolStatus], ['query']),
    'get_swap_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_swap_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(SwapEvent)],
        ['query'],
      ),
    'get_swap_events_by_principal' : IDL.Func(
        [IDL.Principal, IDL.Nat64, IDL.Nat64],
        [IDL.Vec(SwapEventV2)],
        ['query'],
      ),
    'get_swap_events_by_time_range' : IDL.Func(
        [IDL.Nat64, IDL.Nat64, IDL.Nat64],
        [IDL.Vec(SwapEventV2)],
        ['query'],
      ),
    'get_swap_events_v2' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(SwapEventV2)],
        ['query'],
      ),
    'get_top_lps' : IDL.Func(
        [IDL.Nat64],
        [IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat, IDL.Nat32))],
        ['query'],
      ),
    'get_top_swappers' : IDL.Func(
        [StatsWindow, IDL.Nat64],
        [IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64, IDL.Nat))],
        ['query'],
      ),
    'get_virtual_price_series' : IDL.Func(
        [StatsWindow, IDL.Nat64],
        [IDL.Vec(VirtualPricePoint)],
        ['query'],
      ),
    'get_volume_series' : IDL.Func(
        [StatsWindow, IDL.Nat64],
        [IDL.Vec(VolumePoint)],
        ['query'],
      ),
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
    'icrc3_get_archives' : IDL.Func(
        [GetArchivesArgs],
        [GetArchivesResult],
        ['query'],
      ),
    'icrc3_get_blocks' : IDL.Func(
        [IDL.Vec(GetBlocksArgs)],
        [GetBlocksResult],
        ['query'],
      ),
    'icrc3_get_tip_certificate' : IDL.Func(
        [],
        [IDL.Opt(Icrc3DataCertificate)],
        ['query'],
      ),
    'icrc3_supported_block_types' : IDL.Func(
        [],
        [IDL.Vec(SupportedBlockType)],
        ['query'],
      ),
    'quote_optimal_rebalance' : IDL.Func(
        [IDL.Nat8, IDL.Nat8],
        [IDL.Variant({ 'Ok' : OptimalRebalanceQuote, 'Err' : ThreePoolError })],
        ['query'],
      ),
    'quote_swap' : IDL.Func(
        [IDL.Nat8, IDL.Nat8, IDL.Nat],
        [IDL.Variant({ 'Ok' : QuoteSwapResult, 'Err' : ThreePoolError })],
        ['query'],
      ),
    'ramp_a' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : ThreePoolError })],
        [],
      ),
    'remove_authorized_burn_caller' : IDL.Func(
        [IDL.Principal],
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
    'set_fee_curve_params' : IDL.Func(
        [FeeCurveParams],
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
    'simulate_swap_path' : IDL.Func(
        [IDL.Vec(IDL.Tuple(IDL.Nat8, IDL.Nat8, IDL.Nat))],
        [
          IDL.Variant({
            'Ok' : IDL.Vec(QuoteSwapResult),
            'Err' : ThreePoolError,
          }),
        ],
        ['query'],
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
