import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export type BotAdminAction = { 'VaultsNotified' : { 'count' : bigint } } |
  { 'ConfigUpdated' : null };
export interface BotAdminEvent {
  'action' : BotAdminAction,
  'timestamp' : bigint,
  'caller' : string,
}
export interface BotClaimJournal {
  'status' : BotClaimJournalStatus,
  'collateral_price_e8s' : bigint,
  'payment_memo' : Uint8Array | number[],
  'collateral_outbound_fee_e8s' : [] | [bigint],
  'collateral_return' : [] | [BotReturnTransferJournal],
  'collateral_amount_e8s' : bigint,
  'claim_generation' : bigint,
  'vault_id' : bigint,
  'collateral_received_amount_e8s' : [] | [bigint],
  'collateral_return_memo' : Uint8Array | number[],
  'debt_covered_e8s' : bigint,
  'swap_intents' : Array<BotSwapIntent>,
  'failed_return_attempts' : Array<BotReturnTransferJournal>,
}
export type BotClaimJournalStatus = { 'ReturnPending' : null } |
  { 'PaymentShortfall' : null } |
  { 'SwapMayHaveStarted' : null } |
  { 'ReturnFeeQueryPending' : null } |
  { 'ReturnFeeRefreshExhausted' : null };
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
export interface BotPaymentJournal {
  'status' : BotPaymentStatus,
  'shortfall_receipt_observed' : boolean,
  'collateral_price_e8s' : bigint,
  'backend_principal' : Principal,
  'receipt' : [] | [TransferReceipt],
  'fee_e6' : bigint,
  'shortfall_topup' : [] | [BotPaymentTopUpJournal],
  'ckusdc_received_e6' : bigint,
  'memo' : Uint8Array | number[],
  'collateral_amount_e8s' : bigint,
  'claim_generation' : bigint,
  'vault_id' : bigint,
  'gross_amount_e6' : bigint,
  'collateral_received_amount_e8s' : [] | [bigint],
  'held_surplus_e6' : bigint,
  'ledger_principal' : Principal,
  'amount_e6' : bigint,
  'created_at_time' : bigint,
  'debt_covered_e8s' : bigint,
  'icp_swapped_e8s' : bigint,
}
export type BotPaymentStatus = { 'ReceiptObserved' : null } |
  { 'NoEffect' : null } |
  { 'Confirmed' : null } |
  { 'Ambiguous' : null } |
  { 'Prepared' : null };
export interface BotPaymentTopUpJournal {
  'status' : BotPaymentStatus,
  'backend_principal' : Principal,
  'receipt' : [] | [TransferReceipt],
  'fee_e6' : bigint,
  'memo' : Uint8Array | number[],
  'funding_allocation_e6' : bigint,
  'ledger_principal' : Principal,
  'amount_e6' : bigint,
  'created_at_time' : bigint,
}
export interface BotReturnTransferJournal {
  'status' : BotReturnTransferStatus,
  'backend_principal' : Principal,
  'receipt' : [] | [TransferReceipt],
  'memo' : Uint8Array | number[],
  'fee_e8s' : bigint,
  'amount_e8s' : bigint,
  'transfer_fee_e8s' : [] | [bigint],
  'ledger_principal' : Principal,
  'created_at_time' : bigint,
}
export type BotReturnTransferStatus = { 'ReceiptObserved' : null } |
  { 'NoEffect' : null } |
  { 'Ambiguous' : null } |
  { 'Prepared' : null } |
  { 'FeeMismatchAmbiguous' : null };
export interface BotStats {
  'total_debt_covered_e8s' : bigint,
  'total_ckusdc_surplus_held_e6' : bigint,
  'total_collateral_to_treasury_e8s' : bigint,
  'total_ckusdc_deposited_e6' : bigint,
  'events_count' : bigint,
  'total_collateral_received_e8s' : bigint,
}
export interface BotSwapIntent {
  'output_ledger_principal' : Principal,
  'input_ledger_principal' : Principal,
  'input_fee_e8s' : bigint,
  'zero_for_one' : boolean,
  'attempt_ordinal' : number,
  'pre_swap_ckusdc_balance_e6' : bigint,
  'pool_principal' : Principal,
  'amount_in_e8s' : bigint,
  'created_at_time' : bigint,
  'output_fee_e6' : bigint,
  'amount_out_minimum_e6' : bigint,
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
  'status' : LiquidationStatus,
  'ckusdc_transferred_e6' : bigint,
  'oracle_price_e8s' : bigint,
  'ckusdc_received_e6' : bigint,
  'error_message' : [] | [string],
  'icp_to_treasury_e8s' : bigint,
  'collateral_claimed_e8s' : bigint,
  'vault_id' : bigint,
  'slippage_bps' : number,
  'timestamp' : bigint,
  'confirm_retry_count' : number,
  'debt_to_cover_e8s' : bigint,
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
export interface SwapResult {
  'ckusdc_received_e6' : bigint,
  'effective_price_e8s' : bigint,
}
export type TestSwapResult = { 'Ok' : SwapResult } |
  { 'Err' : string };
export interface TransferReceipt {
  'block_index' : bigint,
  'created_at_time' : bigint,
  'amount' : bigint,
}
export interface _SERVICE {
  'admin_approve_pool' : ActorMethod<[], undefined>,
  'admin_authorize_shortfall_topup' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
  'admin_reconcile_payment_block' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
  'admin_reconcile_return_block' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
  'admin_refresh_fees' : ActorMethod<[], [bigint, bigint]>,
  'admin_resolve_pool_ordering' : ActorMethod<[], undefined>,
  'admin_retry_stuck_claim' : ActorMethod<[bigint], undefined>,
  'admin_sweep_ckusdc' : ActorMethod<[Principal, [] | [bigint]], undefined>,
  'admin_test_swap' : ActorMethod<[bigint], TestSwapResult>,
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
  'get_pending_claim_journals' : ActorMethod<[], Array<BotClaimJournal>>,
  'get_pending_payment_journals' : ActorMethod<[], Array<BotPaymentJournal>>,
  'get_processing_paused' : ActorMethod<[], boolean>,
  'get_stuck_liquidations' : ActorMethod<[], Array<LiquidationRecordVersioned>>,
  'notify_liquidatable_vaults' : ActorMethod<
    [Array<LiquidatableVaultInfo>],
    undefined
  >,
  'set_config' : ActorMethod<[BotConfig], undefined>,
  'set_processing_paused' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : string }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
