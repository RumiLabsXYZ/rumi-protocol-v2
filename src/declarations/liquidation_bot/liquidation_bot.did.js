export const idlFactory = ({ IDL }) => {
  const BotConfig = IDL.Record({
    'ckusdt_ledger' : IDL.Opt(IDL.Principal),
    'icp_fee_e8s' : IDL.Opt(IDL.Nat64),
    'admin' : IDL.Principal,
    'backend_principal' : IDL.Principal,
    'icpswap_zero_for_one' : IDL.Opt(IDL.Bool),
    'ckusdc_ledger' : IDL.Principal,
    'kong_swap_principal' : IDL.Opt(IDL.Principal),
    'icp_ledger' : IDL.Principal,
    'treasury_principal' : IDL.Principal,
    'icpswap_pool' : IDL.Principal,
    'icusd_ledger' : IDL.Opt(IDL.Principal),
    'max_slippage_bps' : IDL.Nat16,
    'ckusdc_fee_e6' : IDL.Opt(IDL.Nat64),
    'three_pool_principal' : IDL.Opt(IDL.Principal),
  });
  const BotInitArgs = IDL.Record({ 'config' : BotConfig });
  const SwapResult = IDL.Record({
    'ckusdc_received_e6' : IDL.Nat64,
    'effective_price_e8s' : IDL.Nat64,
  });
  const TestSwapResult = IDL.Variant({ 'Ok' : SwapResult, 'Err' : IDL.Text });
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
  const BotAdminAction = IDL.Variant({
    'VaultsNotified' : IDL.Record({ 'count' : IDL.Nat64 }),
    'ConfigUpdated' : IDL.Null,
  });
  const BotAdminEvent = IDL.Record({
    'action' : BotAdminAction,
    'timestamp' : IDL.Nat64,
    'caller' : IDL.Text,
  });
  const BotStats = IDL.Record({
    'total_debt_covered_e8s' : IDL.Nat64,
    'total_collateral_to_treasury_e8s' : IDL.Nat64,
    'total_ckusdc_deposited_e6' : IDL.Nat64,
    'events_count' : IDL.Nat64,
    'total_collateral_received_e8s' : IDL.Nat64,
  });
  const LiquidationStatus = IDL.Variant({
    'ClaimFailed' : IDL.Null,
    'SwapFailed' : IDL.Null,
    'ConfirmFailed' : IDL.Null,
    'AdminResolved' : IDL.Null,
    'TransferFailed' : IDL.Null,
    'Completed' : IDL.Null,
  });
  const LiquidationRecordV1 = IDL.Record({
    'id' : IDL.Nat64,
    'status' : LiquidationStatus,
    'ckusdc_transferred_e6' : IDL.Nat64,
    'oracle_price_e8s' : IDL.Nat64,
    'ckusdc_received_e6' : IDL.Nat64,
    'error_message' : IDL.Opt(IDL.Text),
    'icp_to_treasury_e8s' : IDL.Nat64,
    'collateral_claimed_e8s' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'slippage_bps' : IDL.Int32,
    'timestamp' : IDL.Nat64,
    'confirm_retry_count' : IDL.Nat8,
    'debt_to_cover_e8s' : IDL.Nat64,
    'effective_price_e8s' : IDL.Nat64,
    'icp_swapped_e8s' : IDL.Nat64,
  });
  const LiquidationRecordVersioned = IDL.Variant({
    'V1' : LiquidationRecordV1,
  });
  const LiquidatableVaultInfo = IDL.Record({
    'collateral_amount' : IDL.Nat64,
    'recommended_liquidation_amount' : IDL.Nat64,
    'collateral_price_e8s' : IDL.Nat64,
    'debt_amount' : IDL.Nat64,
    'vault_id' : IDL.Nat64,
    'collateral_type' : IDL.Principal,
  });
  return IDL.Service({
    'admin_approve_pool' : IDL.Func([], [], []),
    'admin_refresh_fees' : IDL.Func([], [IDL.Nat64, IDL.Nat64], []),
    'admin_resolve_pool_ordering' : IDL.Func([], [], []),
    'admin_retry_stuck_claim' : IDL.Func([IDL.Nat64], [], []),
    'admin_sweep_ckusdc' : IDL.Func(
        [IDL.Principal, IDL.Opt(IDL.Nat64)],
        [],
        [],
      ),
    'admin_test_swap' : IDL.Func([IDL.Nat64], [TestSwapResult], []),
    'cycle_manager_metrics' : IDL.Func(
        [],
        [IDL.Vec(CycleManagerMetric)],
        ['query'],
      ),
    'cycles_status' : IDL.Func([], [CycleManagerCyclesStatus], ['query']),
    'get_admin_event_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_admin_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(BotAdminEvent)],
        ['query'],
      ),
    'get_bot_stats' : IDL.Func([], [BotStats], ['query']),
    'get_liquidation' : IDL.Func(
        [IDL.Nat64],
        [IDL.Opt(LiquidationRecordVersioned)],
        ['query'],
      ),
    'get_liquidation_count' : IDL.Func([], [IDL.Nat64], ['query']),
    'get_liquidation_events' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(LiquidationRecordVersioned)],
        ['query'],
      ),
    'get_liquidations' : IDL.Func(
        [IDL.Nat64, IDL.Nat64],
        [IDL.Vec(LiquidationRecordVersioned)],
        ['query'],
      ),
    'get_stuck_liquidations' : IDL.Func(
        [],
        [IDL.Vec(LiquidationRecordVersioned)],
        ['query'],
      ),
    'notify_liquidatable_vaults' : IDL.Func(
        [IDL.Vec(LiquidatableVaultInfo)],
        [],
        [],
      ),
    'set_config' : IDL.Func([BotConfig], [], []),
  });
};
export const init = ({ IDL }) => {
  const BotConfig = IDL.Record({
    'ckusdt_ledger' : IDL.Opt(IDL.Principal),
    'icp_fee_e8s' : IDL.Opt(IDL.Nat64),
    'admin' : IDL.Principal,
    'backend_principal' : IDL.Principal,
    'icpswap_zero_for_one' : IDL.Opt(IDL.Bool),
    'ckusdc_ledger' : IDL.Principal,
    'kong_swap_principal' : IDL.Opt(IDL.Principal),
    'icp_ledger' : IDL.Principal,
    'treasury_principal' : IDL.Principal,
    'icpswap_pool' : IDL.Principal,
    'icusd_ledger' : IDL.Opt(IDL.Principal),
    'max_slippage_bps' : IDL.Nat16,
    'ckusdc_fee_e6' : IDL.Opt(IDL.Nat64),
    'three_pool_principal' : IDL.Opt(IDL.Principal),
  });
  const BotInitArgs = IDL.Record({ 'config' : BotConfig });
  return [BotInitArgs];
};
