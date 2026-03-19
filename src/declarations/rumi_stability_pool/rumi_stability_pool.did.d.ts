import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface CollateralInfo {
  'status' : CollateralStatus,
  'decimals' : number,
  'ledger_id' : Principal,
  'symbol' : string,
}
export type CollateralStatus = { 'Paused' : null } |
  { 'Active' : null } |
  { 'Deprecated' : null } |
  { 'Sunset' : null } |
  { 'Frozen' : null };
/**
 * ── ICRC-10: Supported Standards ──
 */
export interface Icrc10SupportedStandard { 'url' : string, 'name' : string }
export interface Icrc21ConsentInfo {
  'metadata' : Icrc21ConsentMessageResponseMetadata,
  'consent_message' : Icrc21ConsentMessage,
}
export type Icrc21ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<Icrc21LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
/**
 * ── ICRC-21: Canister Call Consent Messages ──
 */
export interface Icrc21ConsentMessageMetadata {
  'utc_offset_minutes' : [] | [number],
  'language' : string,
}
export interface Icrc21ConsentMessageRequest {
  'arg' : Uint8Array | number[],
  'method' : string,
  'user_preferences' : Icrc21ConsentMessageSpec,
}
export type Icrc21ConsentMessageResponse = { 'Ok' : Icrc21ConsentInfo } |
  { 'Err' : Icrc21Error };
export interface Icrc21ConsentMessageResponseMetadata {
  'utc_offset_minutes' : [] | [number],
  'language' : string,
}
export interface Icrc21ConsentMessageSpec {
  'metadata' : Icrc21ConsentMessageMetadata,
  'device_spec' : [] | [Icrc21DeviceSpec],
}
export type Icrc21DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : Icrc21ErrorInfo } |
  { 'ConsentMessageUnavailable' : Icrc21ErrorInfo };
export interface Icrc21ErrorInfo { 'description' : string }
export interface Icrc21LineDisplayLine { 'line' : string }
export interface Icrc21LineDisplayPage {
  'lines' : Array<Icrc21LineDisplayLine>,
}
/**
 * ── Liquidation types ──
 */
export interface LiquidatableVaultInfo {
  'collateral_amount' : bigint,
  'debt_amount' : bigint,
  'vault_id' : bigint,
  'collateral_type' : Principal,
}
export interface LiquidationResult {
  'error_message' : [] | [string],
  'stables_consumed' : Array<[Principal, bigint]>,
  'vault_id' : bigint,
  'collateral_gained' : bigint,
  'success' : boolean,
  'collateral_type' : Principal,
}
/**
 * ── Configuration ──
 */
export interface PoolConfiguration {
  'emergency_pause' : boolean,
  'min_deposit_e8s' : bigint,
  'authorized_admins' : Array<Principal>,
  'max_liquidations_per_batch' : bigint,
}
export interface PoolLiquidationRecord {
  'collateral_price_e8s' : [] | [bigint],
  'stables_consumed' : Array<[Principal, bigint]>,
  'vault_id' : bigint,
  'depositors_count' : bigint,
  'collateral_gained' : bigint,
  'timestamp' : bigint,
  'collateral_type' : Principal,
}
/**
 * ── Error type ──
 */
export type StabilityPoolError = {
    'LedgerTransferFailed' : { 'reason' : string }
  } |
  { 'EmergencyPaused' : null } |
  { 'AlreadyOptedOut' : { 'collateral' : Principal } } |
  { 'TokenNotActive' : { 'ledger' : Principal } } |
  {
    'InsufficientBalance' : {
      'token' : Principal,
      'available' : bigint,
      'required' : bigint,
    }
  } |
  { 'CollateralNotFound' : { 'ledger' : Principal } } |
  { 'NoPositionFound' : null } |
  { 'AmountTooLow' : { 'minimum_e8s' : bigint } } |
  { 'Unauthorized' : null } |
  { 'InterCanisterCallFailed' : { 'method' : string, 'target' : string } } |
  { 'LiquidationFailed' : { 'vault_id' : bigint, 'reason' : string } } |
  { 'SystemBusy' : null } |
  { 'AlreadyOptedIn' : { 'collateral' : Principal } } |
  { 'TokenNotAccepted' : { 'ledger' : Principal } } |
  { 'InsufficientPoolBalance' : null };
/**
 * ──────────────────────────────────────────────────────────────
 * Rumi Stability Pool — Multi-Token Candid Interface
 * ──────────────────────────────────────────────────────────────
 * Init args
 */
export interface StabilityPoolInitArgs {
  'protocol_canister_id' : Principal,
  'authorized_admins' : Array<Principal>,
}
/**
 * ── Pool status / user position ──
 */
export interface StabilityPoolStatus {
  'collateral_gains' : Array<[Principal, bigint]>,
  'total_depositors' : bigint,
  'stablecoin_registry' : Array<StablecoinConfig>,
  'stablecoin_balances' : Array<[Principal, bigint]>,
  'total_deposits_e8s' : bigint,
  'total_interest_received_e8s' : bigint,
  'collateral_registry' : Array<CollateralInfo>,
  'emergency_paused' : boolean,
  'eligible_icusd_per_collateral' : Array<[Principal, bigint]>,
  'total_liquidations_executed' : bigint,
}
/**
 * ── Registry types ──
 */
export interface StablecoinConfig {
  'decimals' : number,
  'transfer_fee' : [] | [bigint],
  'ledger_id' : Principal,
  'underlying_pool' : [] | [Principal],
  'priority' : number,
  'is_active' : boolean,
  'is_lp_token' : [] | [boolean],
  'symbol' : string,
}
export interface UserStabilityPosition {
  'deposit_timestamp' : bigint,
  'total_interest_earned_e8s' : bigint,
  'collateral_gains' : Array<[Principal, bigint]>,
  'stablecoin_balances' : Array<[Principal, bigint]>,
  'total_claimed_gains' : Array<[Principal, bigint]>,
  'total_usd_value_e8s' : bigint,
  'opted_out_collateral' : Array<Principal>,
}
/**
 * ──────────────────────────────────────────────────────────────
 * Service
 * ──────────────────────────────────────────────────────────────
 */
export interface _SERVICE {
  'admin_correct_balance' : ActorMethod<
    [Principal, Principal, bigint],
    { 'Ok' : string } |
      { 'Err' : StabilityPoolError }
  >,
  'admin_reset_token_failures' : ActorMethod<
    [Principal],
    { 'Ok' : string } |
      { 'Err' : StabilityPoolError }
  >,
  'check_pool_capacity' : ActorMethod<[Principal, bigint], boolean>,
  'claim_all_collateral' : ActorMethod<
    [],
    { 'Ok' : Array<[Principal, bigint]> } |
      { 'Err' : StabilityPoolError }
  >,
  'claim_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : bigint } |
      { 'Err' : StabilityPoolError }
  >,
  /**
   * ── Deposit / Withdraw / Claim ──
   */
  'deposit' : ActorMethod<
    [Principal, bigint],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'deposit_as_3usd' : ActorMethod<
    [Principal, bigint],
    { 'Ok' : bigint } |
      { 'Err' : StabilityPoolError }
  >,
  'emergency_pause' : ActorMethod<
    [],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'execute_liquidation' : ActorMethod<
    [bigint],
    { 'Ok' : LiquidationResult } |
      { 'Err' : StabilityPoolError }
  >,
  'get_liquidation_history' : ActorMethod<
    [[] | [bigint]],
    Array<PoolLiquidationRecord>
  >,
  /**
   * ── Queries ──
   */
  'get_pool_status' : ActorMethod<[], StabilityPoolStatus>,
  'get_suspended_tokens' : ActorMethod<[], Array<[Principal, number]>>,
  'get_user_position' : ActorMethod<
    [[] | [Principal]],
    [] | [UserStabilityPosition]
  >,
  /**
   * ── ICRC-10: Supported Standards ──
   */
  'icrc10_supported_standards' : ActorMethod<
    [],
    Array<Icrc10SupportedStandard>
  >,
  /**
   * ── ICRC-21: Consent Messages ──
   */
  'icrc21_canister_call_consent_message' : ActorMethod<
    [Icrc21ConsentMessageRequest],
    Icrc21ConsentMessageResponse
  >,
  /**
   * ── Liquidation ──
   */
  'notify_liquidatable_vaults' : ActorMethod<
    [Array<LiquidatableVaultInfo>],
    Array<LiquidationResult>
  >,
  'opt_in_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  /**
   * ── Opt-in / Opt-out ──
   */
  'opt_out_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  /**
   * ── Interest Revenue ──
   */
  'receive_interest_revenue' : ActorMethod<
    [Principal, bigint, [] | [Principal]],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'register_collateral' : ActorMethod<
    [CollateralInfo],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  /**
   * ── Admin: Registry ──
   */
  'register_stablecoin' : ActorMethod<
    [StablecoinConfig],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'resume_operations' : ActorMethod<
    [],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  /**
   * ── Admin: Configuration ──
   */
  'update_pool_configuration' : ActorMethod<
    [PoolConfiguration],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'validate_pool_state' : ActorMethod<
    [],
    { 'Ok' : string } |
      { 'Err' : string }
  >,
  'withdraw' : ActorMethod<
    [Principal, bigint],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
