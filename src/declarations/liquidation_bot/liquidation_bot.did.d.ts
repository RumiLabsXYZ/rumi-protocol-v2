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
  /**
   * Legacy fields (ignored, kept for deserialization compat)
   */
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
export interface _SERVICE {
  'admin_approve_pool' : ActorMethod<[], undefined>,
  /**
   * Returns (icp_fee_e8s, ckusdc_fee_e6) freshly read from each ledger and
   * also writes them back into BotConfig so future swaps use the live values.
   */
  'admin_refresh_fees' : ActorMethod<[], [bigint, bigint]>,
  /**
   * Admin actions
   */
  'admin_resolve_pool_ordering' : ActorMethod<[], undefined>,
  'admin_retry_stuck_claim' : ActorMethod<[bigint], undefined>,
  'admin_sweep_ckusdc' : ActorMethod<[Principal, [] | [bigint]], undefined>,
  /**
   * Runs the live swap path (ICP -> ckUSDC) using `amount_e8s` from the bot's
   * ICP balance. ckUSDC stays in the bot; retrieve via `admin_sweep_ckusdc`.
   */
  'admin_test_swap' : ActorMethod<[bigint], TestSwapResult>,
  'get_admin_event_count' : ActorMethod<[], bigint>,
  /**
   * Admin events
   */
  'get_admin_events' : ActorMethod<[bigint, bigint], Array<BotAdminEvent>>,
  /**
   * Stats
   */
  'get_bot_stats' : ActorMethod<[], BotStats>,
  /**
   * History
   */
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
  'get_stuck_liquidations' : ActorMethod<[], Array<LiquidationRecordVersioned>>,
  /**
   * Core
   */
  'notify_liquidatable_vaults' : ActorMethod<
    [Array<LiquidatableVaultInfo>],
    undefined
  >,
  'set_config' : ActorMethod<[BotConfig], undefined>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
