import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export type BotAdminAction = { 'VaultsNotified' : { 'count' : bigint } } |
  { 'ConfigUpdated' : null };
export interface BotAdminEvent {
  'action' : BotAdminAction,
  'timestamp' : bigint,
  'caller' : string,
}
export interface BotConfig {
  'ckusdt_ledger' : [] | [Principal],
  'icp_fee_e8s' : [] | [bigint],
  'admin' : Principal,
  'backend_principal' : Principal,
  'icpswap_zero_for_one' : [] | [boolean],
  'ckusdc_ledger' : Principal,
  'kong_swap_principal' : [] | [Principal],
  'icp_ledger' : Principal,
  'treasury_principal' : Principal,
  'icpswap_pool' : Principal,
  'icusd_ledger' : [] | [Principal],
  'max_slippage_bps' : number,
  'ckusdc_fee_e6' : [] | [bigint],
  'three_pool_principal' : [] | [Principal],
}
export interface BotInitArgs { 'config' : BotConfig }
export interface BotStats {
  'total_debt_covered_e8s' : bigint,
  'total_collateral_to_treasury_e8s' : bigint,
  'total_ckusdc_deposited_e6' : bigint,
  'events_count' : bigint,
  'total_collateral_received_e8s' : bigint,
}
export interface CycleManagerCyclesStatus {
  'idle_burn_cycles_per_day' : [] | [bigint],
  'stable_memory_bytes' : [] | [bigint],
  'low_watermark' : bigint,
  'balance' : bigint,
  'heap_memory_bytes' : [] | [bigint],
  'healthy' : boolean,
  'freeze_threshold_secs' : bigint,
}
export interface CycleManagerMetric {
  'key' : string,
  'value' : bigint,
  'count' : bigint,
  'label' : [] | [string],
}
export type IcpTreasuryBonusState = { 'Paid' : null } |
  { 'Prepared' : null } |
  { 'Quarantined' : null };
export interface IcpTreasuryBonusTransfer {
  'block_index' : [] | [bigint],
  'args' : TransferArg,
  'from' : Account,
  'ledger' : Principal,
}
export interface LiquidatableVaultInfo {
  'collateral_amount' : bigint,
  'recommended_liquidation_amount' : bigint,
  'collateral_price_e8s' : bigint,
  'debt_amount' : bigint,
  'vault_id' : bigint,
  'collateral_type' : Principal,
}
export interface LiquidationRecordV1 {
  'id' : bigint,
  'ckusdc_payment_block_index' : [] | [bigint],
  'claim_timestamp' : [] | [bigint],
  'status' : LiquidationStatus,
  'payment_memo' : [] | [Uint8Array | number[]],
  'ckusdc_transferred_e6' : bigint,
  'oracle_price_e8s' : bigint,
  'ckusdc_received_e6' : bigint,
  'error_message' : [] | [string],
  'icp_treasury_bonus_state' : [] | [IcpTreasuryBonusState],
  'icp_to_treasury_e8s' : bigint,
  'collateral_claimed_e8s' : bigint,
  'icp_treasury_transfer' : [] | [IcpTreasuryBonusTransfer],
  'vault_id' : bigint,
  'slippage_bps' : number,
  'timestamp' : bigint,
  'confirm_retry_count' : number,
  'debt_to_cover_e8s' : bigint,
  'ckusdc_payment_amount_e6' : [] | [bigint],
  'effective_price_e8s' : bigint,
  'icp_swapped_e8s' : bigint,
}
export type LiquidationRecordVersioned = { 'V1' : LiquidationRecordV1 };
export type LiquidationStatus = { 'ClaimFailed' : null } |
  { 'SwapFailed' : null } |
  { 'ConfirmFailed' : null } |
  { 'AdminResolved' : null } |
  { 'TransferFailed' : null } |
  { 'Completed' : null };
export type Result = { 'Ok' : null } |
  { 'Err' : string };
export type Result_1 = { 'Ok' : SwapResult } |
  { 'Err' : string };
export type Result_2 = { 'Ok' : bigint } |
  { 'Err' : string };
export interface SwapResult {
  'ckusdc_received_e6' : bigint,
  'effective_price_e8s' : bigint,
}
export interface TransferArg {
  'to' : Account,
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
export interface _SERVICE {
  'admin_approve_pool' : ActorMethod<[], undefined>,
  'admin_recover_ckusdc_shortfall' : ActorMethod<[bigint, bigint], Result>,
  'admin_refresh_fees' : ActorMethod<[], [bigint, bigint]>,
  'admin_requeue_pending_bot_claim' : ActorMethod<[bigint], Result>,
  'admin_resolve_pool_ordering' : ActorMethod<[], undefined>,
  'admin_retry_claim_return' : ActorMethod<[bigint], Result>,
  'admin_retry_stuck_claim' : ActorMethod<[bigint], undefined>,
  'admin_submit_paused_claim_ckusdc_payment' : ActorMethod<[bigint], Result>,
  'admin_sweep_ckusdc' : ActorMethod<[Principal, [] | [bigint]], undefined>,
  'admin_test_swap' : ActorMethod<[bigint], Result_1>,
  'backend_claim_request_id_floor' : ActorMethod<[], Result_2>,
  'cycle_manager_metrics' : ActorMethod<[], Array<CycleManagerMetric>>,
  'cycles_status' : ActorMethod<[], CycleManagerCyclesStatus>,
  'get_admin_event_count' : ActorMethod<[], bigint>,
  'get_admin_events' : ActorMethod<[bigint, bigint], Array<BotAdminEvent>>,
  'get_bot_stats' : ActorMethod<[], BotStats>,
  'get_liquidation' : ActorMethod<[bigint], [] | [LiquidationRecordVersioned]>,
  'get_liquidation_count' : ActorMethod<[], bigint>,
  'get_liquidation_events' : ActorMethod<
    [bigint, bigint],
    Array<LiquidationRecordVersioned>
  >,
  'get_liquidations' : ActorMethod<
    [bigint, bigint],
    Array<LiquidationRecordVersioned>
  >,
  'get_paused_claim_ckusdc_payment_account' : ActorMethod<
    [bigint],
    [] | [Account]
  >,
  'get_stuck_liquidations' : ActorMethod<[], Array<LiquidationRecordVersioned>>,
  'notify_liquidatable_vaults' : ActorMethod<
    [Array<LiquidatableVaultInfo>],
    undefined
  >,
  'set_config' : ActorMethod<[BotConfig], undefined>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
