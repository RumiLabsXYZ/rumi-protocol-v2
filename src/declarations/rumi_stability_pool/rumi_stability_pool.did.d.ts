import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface CfxClaimPayoutRecovery {
  'claim_id' : bigint,
  'claimant' : Principal,
  'op_id' : bigint,
  'chain_sentinel' : Principal,
  'amount_wei' : bigint,
  'failed_at_ns' : bigint,
  'reason' : string,
}
export interface ChainAbsorbAutoConfig {
  'enabled' : boolean,
  'interval_seconds' : bigint,
  'max_scan_per_chain' : bigint,
}
export interface ChainAbsorbAutoStatus {
  'tick_in_flight' : boolean,
  'last_tick' : [] | [ChainAbsorbAutoTickRecord],
  'config' : ChainAbsorbAutoConfig,
}
export interface ChainAbsorbAutoTickRecord {
  'skipped_reason' : [] | [string],
  'started_at_ns' : bigint,
  'attempted_vault_id' : [] | [bigint],
  'candidates_scanned' : bigint,
  'completed_at_ns' : bigint,
  'error' : [] | [string],
  'absorbed' : [] | [ChainSpAbsorbResult],
}
export interface ChainLiquidatableVaultInfo {
  'sized_repay_e8s' : bigint,
  'cr_e4' : bigint,
  'collateral_native' : bigint,
  'vault_id' : bigint,
  'sp_attempted' : boolean,
  'chain_collateral_sentinel' : Principal,
  'chain_id' : number,
  'liquidation_threshold_e4' : bigint,
  'effective_debt_e8s' : bigint,
  'debt_e8s' : bigint,
}
export interface ChainSpAbsorbCandidate {
  'icusd_to_burn_e8s' : bigint,
  'vault' : ChainLiquidatableVaultInfo,
  'pending_status' : [] | [ChainSpAbsorbIntentStatus],
}
export interface ChainSpAbsorbCompletion {
  'result' : ChainSpAbsorbResult,
  'completed_at_ns' : bigint,
  'vault_id' : bigint,
}
export interface ChainSpAbsorbIntent {
  'last_error' : [] | [string],
  'status' : ChainSpAbsorbIntentStatus,
  'icusd_to_burn_e8s' : bigint,
  'burn_proof' : [] | [SpWritedownProof],
  'stables_consumed' : Array<[Principal, bigint]>,
  'updated_at_ns' : bigint,
  'vault_id' : bigint,
  'chain_sentinel' : Principal,
  'created_at_ns' : bigint,
  'burn_created_at_time_ns' : bigint,
  'chain_id' : number,
  'backend_result' : [] | [ChainStabilityPoolLiquidationResult],
  'icusd_ledger' : Principal,
  'icusd_minting_account' : IcrcAccount,
}
export type ChainSpAbsorbIntentStatus = { 'BackendRejected' : null } |
  { 'Burned' : null } |
  { 'BackendAccepted' : null } |
  { 'Prepared' : null } |
  { 'LocalApplied' : null };
export interface ChainSpAbsorbResult {
  'collateral_price_e8s' : bigint,
  'liquidated_debt_e8s' : bigint,
  'collateral_received_native' : bigint,
  'block_index' : bigint,
  'claim_id' : bigint,
  'custody_address' : string,
  'vault_id' : bigint,
  'chain_id' : number,
  'icusd_burned_e8s' : bigint,
  'success' : boolean,
}
export interface ChainStabilityPoolLiquidationResult {
  'collateral_price_e8s' : bigint,
  'liquidated_debt_e8s' : bigint,
  'collateral_received_native' : bigint,
  'block_index' : bigint,
  'claim_id' : bigint,
  'custody_address' : string,
  'vault_id' : bigint,
  'chain_id' : number,
  'success' : boolean,
}
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
export interface Icrc10SupportedStandard { 'url' : string, 'name' : string }
export interface Icrc21ConsentInfo {
  'metadata' : Icrc21ConsentMessageResponseMetadata,
  'consent_message' : Icrc21ConsentMessage,
}
export type Icrc21ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<Icrc21LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
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
export interface IcrcAccount {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
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
export interface PendingRefund {
  'id' : bigint,
  'user' : Principal,
  'created_at' : bigint,
  'amount' : bigint,
  'token_ledger' : Principal,
  'reason' : string,
}
export interface PoolConfiguration {
  'emergency_pause' : boolean,
  'min_deposit_e8s' : bigint,
  'authorized_admins' : Array<Principal>,
  'max_liquidations_per_batch' : bigint,
}
export interface PoolEvent {
  'id' : bigint,
  'timestamp' : bigint,
  'caller' : Principal,
  'event_type' : PoolEventType,
}
export type PoolEventType = {
    'Withdraw' : { 'amount' : bigint, 'token_ledger' : Principal }
  } |
  { 'OperationsResumed' : null } |
  { 'CollateralRegistered' : { 'ledger' : Principal, 'symbol' : string } } |
  { 'OptOutCollateral' : { 'collateral_type' : Principal } } |
  { 'OptInCollateral' : { 'collateral_type' : Principal } } |
  { 'StablecoinRegistered' : { 'ledger' : Principal, 'symbol' : string } } |
  { 'Deposit' : { 'amount' : bigint, 'token_ledger' : Principal } } |
  { 'InterestReceived' : { 'amount' : bigint, 'token_ledger' : Principal } } |
  { 'ConfigurationUpdated' : null } |
  { 'LiquidationNotification' : { 'vault_count' : bigint } } |
  { 'EmergencyPauseActivated' : null } |
  {
    'DepositAs3USD' : {
      'amount_in' : bigint,
      'lp_minted' : bigint,
      'token_ledger' : Principal,
    }
  } |
  {
    'ClaimCollateral' : { 'collateral_ledger' : Principal, 'amount' : bigint }
  } |
  {
    'LiquidationExecuted' : {
      'stables_consumed_e8s' : bigint,
      'vault_id' : bigint,
      'collateral_gained' : bigint,
      'success' : boolean,
      'collateral_type' : Principal,
    }
  } |
  {
    'CollateralGainCorrected' : {
      'user' : Principal,
      'new_amount' : bigint,
      'collateral_ledger' : Principal,
    }
  } |
  {
    'BalanceCorrected' : {
      'user' : Principal,
      'new_amount' : bigint,
      'token_ledger' : Principal,
    }
  };
export interface PoolLiquidationRecord {
  'collateral_price_e8s' : [] | [bigint],
  'stables_consumed' : Array<[Principal, bigint]>,
  'vault_id' : bigint,
  'depositors_count' : bigint,
  'collateral_gained' : bigint,
  'timestamp' : bigint,
  'collateral_type' : Principal,
}
export type SpProofLedger = { 'IcusdBurn' : null } |
  { 'ThreePoolTransfer' : null };
export interface SpWritedownProof {
  'block_index' : bigint,
  'ledger_kind' : SpProofLedger,
  'vault_id_memo' : bigint,
}
export type StabilityPoolError = {
    'LedgerTransferFailed' : { 'reason' : string }
  } |
  { 'RefundClaimNotFound' : null } |
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
  { 'InvalidPayoutAddress' : { 'reason' : string } } |
  { 'CollateralNotFound' : { 'ledger' : Principal } } |
  { 'NoPositionFound' : null } |
  { 'AmountTooLow' : { 'minimum_e8s' : bigint } } |
  { 'Unauthorized' : null } |
  { 'InterCanisterCallFailed' : { 'method' : string, 'target' : string } } |
  { 'PayoutAddressRequired' : { 'collateral' : Principal } } |
  { 'LiquidationFailed' : { 'vault_id' : bigint, 'reason' : string } } |
  { 'SystemBusy' : null } |
  { 'AlreadyOptedIn' : { 'collateral' : Principal } } |
  { 'TokenNotAccepted' : { 'ledger' : Principal } } |
  { 'InsufficientPoolBalance' : null };
export interface StabilityPoolInitArgs {
  'protocol_canister_id' : Principal,
  'authorized_admins' : Array<Principal>,
}
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
  'cfx_claims' : [] | [Array<[Principal, bigint]>],
  'total_claimed_gains' : Array<[Principal, bigint]>,
  'native_payout_addresses' : [] | [Array<[Principal, string]>],
  'total_usd_value_e8s' : bigint,
  'opted_out_collateral' : Array<Principal>,
}
export interface _SERVICE {
  'admin_correct_balance' : ActorMethod<
    [Principal, Principal, bigint],
    { 'Ok' : string } |
      { 'Err' : StabilityPoolError }
  >,
  'admin_correct_collateral_gain' : ActorMethod<
    [Principal, Principal, bigint],
    { 'Ok' : string } |
      { 'Err' : StabilityPoolError }
  >,
  'check_chain_absorb_capacity' : ActorMethod<[Principal, bigint], boolean>,
  'check_pool_capacity' : ActorMethod<[Principal, bigint], boolean>,
  'claim_all_collateral' : ActorMethod<
    [],
    { 'Ok' : Array<[Principal, bigint]> } |
      { 'Err' : StabilityPoolError }
  >,
  'claim_cfx' : ActorMethod<
    [Principal, string],
    { 'Ok' : bigint } |
      { 'Err' : StabilityPoolError }
  >,
  'claim_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : bigint } |
      { 'Err' : StabilityPoolError }
  >,
  'claim_pending_refund' : ActorMethod<
    [bigint],
    { 'Ok' : bigint } |
      { 'Err' : StabilityPoolError }
  >,
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
  'get_chain_absorb_auto_status' : ActorMethod<[], ChainAbsorbAutoStatus>,
  'get_chain_collateral_sentinel' : ActorMethod<[number], Principal>,
  'get_completed_chain_absorbs' : ActorMethod<
    [[] | [bigint]],
    Array<ChainSpAbsorbCompletion>
  >,
  'get_liquidation_history' : ActorMethod<
    [[] | [bigint]],
    Array<PoolLiquidationRecord>
  >,
  'get_pending_chain_absorbs' : ActorMethod<[], Array<ChainSpAbsorbIntent>>,
  'get_pending_refunds' : ActorMethod<[[] | [Principal]], Array<PendingRefund>>,
  'get_pool_event_count' : ActorMethod<[], bigint>,
  'get_pool_events' : ActorMethod<[bigint, bigint], Array<PoolEvent>>,
  'get_pool_status' : ActorMethod<[], StabilityPoolStatus>,
  'get_suspended_tokens' : ActorMethod<[], Array<[Principal, number]>>,
  'get_user_position' : ActorMethod<
    [[] | [Principal]],
    [] | [UserStabilityPosition]
  >,
  'icrc10_supported_standards' : ActorMethod<
    [],
    Array<Icrc10SupportedStandard>
  >,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [Icrc21ConsentMessageRequest],
    Icrc21ConsentMessageResponse
  >,
  'list_depositor_principals' : ActorMethod<[], Array<Principal>>,
  'notify_liquidatable_vaults' : ActorMethod<
    [Array<LiquidatableVaultInfo>],
    Array<LiquidationResult>
  >,
  'opt_in_cfx' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'opt_in_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'opt_in_native_collateral' : ActorMethod<
    [Principal, string],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'opt_out_cfx' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'opt_out_collateral' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'receive_interest_revenue' : ActorMethod<
    [Principal, bigint, [] | [Principal]],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'recredit_failed_cfx_claim_payout' : ActorMethod<
    [CfxClaimPayoutRecovery],
    { 'Ok' : boolean } |
      { 'Err' : StabilityPoolError }
  >,
  'register_cfx_collateral' : ActorMethod<
    [number],
    { 'Ok' : Principal } |
      { 'Err' : StabilityPoolError }
  >,
  'register_collateral' : ActorMethod<
    [CollateralInfo],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
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
  'scan_chain_absorb_candidates' : ActorMethod<
    [[] | [bigint]],
    { 'Ok' : Array<ChainSpAbsorbCandidate> } |
      { 'Err' : StabilityPoolError }
  >,
  'set_chain_absorb_auto_config' : ActorMethod<
    [ChainAbsorbAutoConfig],
    { 'Ok' : null } |
      { 'Err' : StabilityPoolError }
  >,
  'sp_absorb_chain_vault' : ActorMethod<
    [bigint],
    { 'Ok' : ChainSpAbsorbResult } |
      { 'Err' : StabilityPoolError }
  >,
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
