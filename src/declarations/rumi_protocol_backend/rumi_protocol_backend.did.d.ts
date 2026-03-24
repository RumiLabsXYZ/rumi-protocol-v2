import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface AddCollateralArg {
  'redemption_fee_ceiling' : [] | [number],
  'debt_ceiling' : bigint,
  'min_vault_debt' : bigint,
  'min_collateral_deposit' : bigint,
  'redemption_fee_floor' : [] | [number],
  'borrow_threshold_ratio' : number,
  'ledger_canister_id' : Principal,
  'price_source' : PriceSource,
  'liquidation_bonus' : number,
  'display_color' : [] | [string],
  'borrowing_fee' : number,
  'interest_rate_apr' : number,
  'liquidation_ratio' : number,
}
export interface BotLiquidationResult {
  'collateral_amount' : bigint,
  'collateral_price_e8s' : bigint,
  'vault_id' : bigint,
  'debt_covered' : bigint,
}
export interface BotStatsResponse {
  'total_debt_covered_e8s' : bigint,
  'budget_remaining_e8s' : bigint,
  'liquidation_bot_principal' : [] | [Principal],
  'budget_total_e8s' : bigint,
  'budget_start_timestamp' : bigint,
  'total_icusd_deposited_e8s' : bigint,
}
export interface CandidVault {
  'collateral_amount' : bigint,
  'owner' : Principal,
  'vault_id' : bigint,
  'collateral_type' : Principal,
  'accrued_interest' : bigint,
  'icp_margin_amount' : bigint,
  'borrowed_icusd_amount' : bigint,
}
export interface CollateralConfig {
  'last_redemption_time' : bigint,
  'status' : CollateralStatus,
  'decimals' : number,
  'recovery_interest_rate_apr' : [] | [Uint8Array | number[]],
  'redemption_fee_ceiling' : Uint8Array | number[],
  'healthy_cr' : [] | [Uint8Array | number[]],
  'debt_ceiling' : bigint,
  'min_vault_debt' : bigint,
  'rate_curve' : [] | [RateCurve],
  'recovery_borrowing_fee' : [] | [Uint8Array | number[]],
  'min_collateral_deposit' : bigint,
  'last_price' : [] | [number],
  'last_price_timestamp' : [] | [bigint],
  'redemption_fee_floor' : Uint8Array | number[],
  'borrow_threshold_ratio' : Uint8Array | number[],
  'ledger_fee' : bigint,
  'recovery_target_cr' : Uint8Array | number[],
  'current_base_rate' : Uint8Array | number[],
  'ledger_canister_id' : Principal,
  'price_source' : PriceSource,
  'liquidation_bonus' : Uint8Array | number[],
  'display_color' : [] | [string],
  'borrowing_fee' : Uint8Array | number[],
  'interest_rate_apr' : Uint8Array | number[],
  'liquidation_ratio' : Uint8Array | number[],
}
export interface CollateralInterestInfo {
  'total_debt_e8s' : bigint,
  'collateral_type' : Principal,
  'weighted_interest_rate' : number,
}
export interface CollateralSnapshot {
  'total_collateral' : bigint,
  'total_debt' : bigint,
  'collateral_type' : Principal,
  'price' : number,
  'vault_count' : bigint,
}
export type CollateralStatus = { 'Paused' : null } |
  { 'Active' : null } |
  { 'Deprecated' : null } |
  { 'Sunset' : null } |
  { 'Frozen' : null };
export interface CollateralTotals {
  'decimals' : number,
  'total_collateral' : bigint,
  'total_debt' : bigint,
  'collateral_type' : Principal,
  'price' : number,
  'vault_count' : bigint,
  'symbol' : string,
}
export interface ConsentInfo {
  'metadata' : ConsentMessageMetadata,
  'consent_message' : ConsentMessage,
}
export type ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
export interface ConsentMessageMetadata {
  'utc_offset_minutes' : [] | [number],
  'language' : string,
}
export interface ConsentMessageRequest {
  'arg' : Uint8Array | number[],
  'method' : string,
  'user_preferences' : ConsentMessageSpec,
}
export interface ConsentMessageSpec {
  'metadata' : ConsentMessageMetadata,
  'device_spec' : [] | [DeviceSpec],
}
export type DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
export interface ErrorInfo { 'description' : string }
export type Event = { 'set_borrowing_fee' : { 'rate' : string } } |
  {
    'VaultWithdrawnAndClosed' : {
      'vault_id' : bigint,
      'timestamp' : bigint,
      'caller' : Principal,
      'amount' : bigint,
    }
  } |
  {
    'claim_liquidity_returns' : {
      'block_index' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : Principal,
      'amount' : bigint,
    }
  } |
  {
    'collateral_withdrawn' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : [] | [Principal],
      'amount' : bigint,
    }
  } |
  {
    'repay_to_vault' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'repayed_amount' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : [] | [Principal],
    }
  } |
  {
    'provide_liquidity' : {
      'block_index' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : Principal,
      'amount' : bigint,
    }
  } |
  { 'set_rmr_ceiling_cr' : { 'value' : string } } |
  { 'set_recovery_rate_curve' : { 'markers' : string } } |
  { 'set_ckstable_repay_fee' : { 'rate' : string } } |
  { 'set_treasury_principal' : { 'principal' : Principal } } |
  { 'accrue_interest' : { 'timestamp' : bigint } } |
  { 'set_max_partial_liquidation_ratio' : { 'rate' : string } } |
  {
    'withdraw_and_close_vault' : {
      'block_index' : [] | [bigint],
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : [] | [Principal],
      'amount' : bigint,
    }
  } |
  {
    'admin_vault_correction' : {
      'vault_id' : bigint,
      'new_amount' : bigint,
      'old_amount' : bigint,
      'reason' : string,
    }
  } |
  { 'set_recovery_target_cr' : { 'rate' : string } } |
  { 'init' : InitArg } |
  {
    'set_stable_ledger_principal' : {
      'principal' : Principal,
      'token_type' : StableTokenType,
    }
  } |
  {
    'open_vault' : {
      'block_index' : bigint,
      'vault' : Vault,
      'timestamp' : [] | [bigint],
    }
  } |
  {
    'redemption_on_vaults' : {
      'icusd_amount' : bigint,
      'icusd_block_index' : bigint,
      'owner' : Principal,
      'timestamp' : [] | [bigint],
      'fee_amount' : bigint,
      'current_icp_rate' : Uint8Array | number[],
    }
  } |
  {
    'set_recovery_parameters' : {
      'recovery_interest_rate_apr' : [] | [string],
      'recovery_borrowing_fee' : [] | [string],
      'collateral_type' : Principal,
    }
  } |
  {
    'set_collateral_borrowing_fee' : {
      'fee' : [] | [string],
      'rate' : [] | [string],
      'collateral_type' : Principal,
      'borrowing_fee' : [] | [string],
    }
  } |
  {
    'margin_transfer' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
    }
  } |
  {
    'admin_sweep_to_treasury' : {
      'block_index' : bigint,
      'amount' : bigint,
      'treasury' : Principal,
      'reason' : string,
    }
  } |
  { 'set_rmr_floor_cr' : { 'value' : string } } |
  { 'set_rmr_ceiling' : { 'value' : string } } |
  {
    'set_global_icusd_mint_cap' : {
      'cap' : [] | [string],
      'amount' : [] | [string],
    }
  } |
  { 'upgrade' : UpgradeArg } |
  {
    'borrow_from_vault' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'fee_amount' : bigint,
      'caller' : [] | [Principal],
      'borrowed_amount' : bigint,
    }
  } |
  { 'set_reserve_redemptions_enabled' : { 'enabled' : boolean } } |
  { 'set_min_icusd_amount' : { 'amount' : string } } |
  { 'set_borrowing_fee_curve' : { 'markers' : string } } |
  { 'set_interest_pool_share' : { 'share' : string } } |
  { 'set_liquidation_protocol_share' : { 'share' : string } } |
  {
    'update_collateral_config' : {
      'config' : CollateralConfig,
      'collateral_type' : Principal,
    }
  } |
  {
    'redistribute_vault' : { 'vault_id' : bigint, 'timestamp' : [] | [bigint] }
  } |
  {
    'partial_collateral_withdrawn' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : [] | [Principal],
      'amount' : bigint,
    }
  } |
  {
    'set_rate_curve_markers' : {
      'markers' : string,
      'collateral_type' : [] | [string],
    }
  } |
  {
    'dust_forgiven' : {
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'amount' : bigint,
    }
  } |
  {
    'partial_liquidate_vault' : {
      'protocol_fee_collateral' : [] | [bigint],
      'icp_rate' : [] | [Uint8Array | number[]],
      'liquidator_payment' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'liquidator' : [] | [Principal],
      'icp_to_liquidator' : bigint,
    }
  } |
  {
    'withdraw_liquidity' : {
      'block_index' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : Principal,
      'amount' : bigint,
    }
  } |
  {
    'admin_mint' : {
      'to' : Principal,
      'block_index' : bigint,
      'timestamp' : [] | [bigint],
      'amount' : bigint,
      'reason' : string,
    }
  } |
  { 'set_three_pool_canister' : { 'canister' : Principal } } |
  { 'set_liquidation_bonus' : { 'rate' : string } } |
  {
    'reserve_redemption' : {
      'icusd_amount' : bigint,
      'icusd_block_index' : bigint,
      'fee_stable_amount' : bigint,
      'owner' : Principal,
      'timestamp' : [] | [bigint],
      'fee_amount' : bigint,
      'stable_amount_sent' : bigint,
      'stable_token_ledger' : Principal,
    }
  } |
  {
    'close_vault' : {
      'block_index' : [] | [bigint],
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
    }
  } |
  {
    'update_collateral_status' : {
      'status' : CollateralStatus,
      'collateral_type' : Principal,
    }
  } |
  {
    'set_healthy_cr' : {
      'healthy_cr' : [] | [string],
      'collateral_type' : string,
    }
  } |
  { 'set_redemption_fee_ceiling' : { 'rate' : string } } |
  {
    'add_margin_to_vault' : {
      'block_index' : bigint,
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'caller' : [] | [Principal],
      'margin_added' : bigint,
    }
  } |
  { 'set_stability_pool_principal' : { 'principal' : Principal } } |
  { 'set_interest_split' : { 'split' : string } } |
  { 'set_bot_budget' : { 'start_timestamp' : bigint, 'total_e8s' : bigint } } |
  { 'set_rmr_floor' : { 'value' : string } } |
  { 'set_redemption_fee_floor' : { 'rate' : string } } |
  {
    'set_interest_rate' : {
      'collateral_type' : Principal,
      'interest_rate_apr' : string,
    }
  } |
  { 'set_reserve_redemption_fee' : { 'fee' : string } } |
  {
    'redemption_transfered' : {
      'icusd_block_index' : bigint,
      'icp_block_index' : bigint,
      'timestamp' : [] | [bigint],
    }
  } |
  { 'set_liquidation_bot_principal' : { 'principal' : Principal } } |
  {
    'liquidate_vault' : {
      'mode' : Mode,
      'icp_rate' : Uint8Array | number[],
      'vault_id' : bigint,
      'timestamp' : [] | [bigint],
      'liquidator' : [] | [Principal],
    }
  } |
  {
    'add_collateral_type' : {
      'config' : CollateralConfig,
      'collateral_type' : Principal,
    }
  } |
  {
    'set_stable_token_enabled' : {
      'enabled' : boolean,
      'token_type' : StableTokenType,
    }
  } |
  { 'set_recovery_cr_multiplier' : { 'multiplier' : string } };
export interface Fees { 'redemption_fee' : number, 'borrowing_fee' : number }
export interface GetEventsArg { 'start' : bigint, 'length' : bigint }
export interface GetEventsFilteredResponse {
  'total' : bigint,
  'events' : Array<[bigint, Event]>,
}
export interface HttpRequest {
  'url' : string,
  'method' : string,
  'body' : Uint8Array | number[],
  'headers' : Array<[string, string]>,
}
export interface HttpResponse {
  'body' : Uint8Array | number[],
  'headers' : Array<[string, string]>,
  'status_code' : number,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface InitArg {
  'ckusdc_ledger_principal' : [] | [Principal],
  'xrc_principal' : Principal,
  'icp_ledger_principal' : Principal,
  'fee_e8s' : bigint,
  'ckusdt_ledger_principal' : [] | [Principal],
  'stability_pool_principal' : [] | [Principal],
  'treasury_principal' : [] | [Principal],
  'developer_principal' : Principal,
  'icusd_ledger_principal' : Principal,
}
export interface InterestSplitArg { 'bps' : bigint, 'destination' : string }
export type InterpolationMethod = { 'Linear' : null };
export interface LineDisplayPage { 'lines' : Array<string> }
export interface LiquidityStatus {
  'liquidity_provided' : bigint,
  'total_liquidity_provided' : bigint,
  'liquidity_pool_share' : number,
  'available_liquidity_reward' : bigint,
  'total_available_returns' : bigint,
}
export type Mode = { 'ReadOnly' : null } |
  { 'GeneralAvailability' : null } |
  { 'Recovery' : null };
export interface OpenVaultSuccess {
  'block_index' : bigint,
  'vault_id' : bigint,
}
export interface PerCollateralRateCurve {
  'markers' : Array<[number, number]>,
  'base_rate' : number,
  'collateral_type' : Principal,
}
export type PriceSource = {
    'Xrc' : {
      'quote_asset_class' : XrcAssetClass,
      'quote_asset' : string,
      'base_asset_class' : XrcAssetClass,
      'base_asset' : string,
    }
  } |
  { 'CoinGecko' : { 'coin_id' : string, 'vs_currency' : string } } |
  {
    'LstWrapped' : {
      'quote_asset_class' : XrcAssetClass,
      'haircut' : number,
      'rate_canister_id' : Principal,
      'quote_asset' : string,
      'base_asset_class' : XrcAssetClass,
      'base_asset' : string,
      'rate_method' : string,
    }
  };
export type ProtocolArg = { 'Upgrade' : UpgradeArg } |
  { 'Init' : InitArg };
export type ProtocolError = { 'GenericError' : string } |
  { 'TemporarilyUnavailable' : string } |
  { 'TransferError' : TransferError } |
  { 'AlreadyProcessing' : null } |
  { 'AnonymousCallerNotAllowed' : null } |
  { 'AmountTooLow' : { 'minimum_amount' : bigint } } |
  { 'TransferFromError' : [TransferFromError, bigint] } |
  { 'CallerNotOwner' : null };
export interface ProtocolSnapshot {
  'total_debt' : bigint,
  'collateral_snapshots' : Array<CollateralSnapshot>,
  'timestamp' : bigint,
  'total_vault_count' : bigint,
  'total_collateral_value_usd' : bigint,
}
export interface ProtocolStatus {
  'last_icp_timestamp' : bigint,
  'borrowing_fee_curve_resolved' : Array<[number, number]>,
  'recovery_mode_threshold' : number,
  'per_collateral_interest' : Array<CollateralInterestInfo>,
  'reserve_redemption_fee' : number,
  'mode' : Mode,
  'interest_split' : Array<InterestSplitArg>,
  'recovery_cr_multiplier' : number,
  'interest_pool_share' : number,
  'total_icusd_borrowed' : bigint,
  'min_icusd_amount' : bigint,
  'total_collateral_ratio' : number,
  'ckstable_repay_fee' : number,
  'total_icp_margin' : bigint,
  'recovery_target_cr' : number,
  'frozen' : boolean,
  'weighted_average_interest_rate' : number,
  'manual_mode_override' : boolean,
  'liquidation_bonus' : number,
  'per_collateral_rate_curves' : Array<PerCollateralRateCurve>,
  'reserve_redemptions_enabled' : boolean,
  'global_icusd_mint_cap' : bigint,
  'last_icp_rate' : number,
}
export interface RateCurve {
  'method' : InterpolationMethod,
  'markers' : Array<RateMarker>,
}
export interface RateMarker {
  'multiplier' : Uint8Array | number[],
  'cr_level' : Uint8Array | number[],
}
export interface ReserveBalance {
  'balance' : bigint,
  'ledger' : Principal,
  'symbol' : string,
}
export interface ReserveRedemptionResult {
  'icusd_block_index' : bigint,
  'stable_token_used' : Principal,
  'vault_spillover_amount' : bigint,
  'fee_amount' : bigint,
  'stable_amount_sent' : bigint,
}
export type Result = { 'Ok' : null } |
  { 'Err' : ProtocolError };
export type Result_1 = { 'Ok' : bigint } |
  { 'Err' : ProtocolError };
export type Result_10 = { 'Ok' : StabilityPoolLiquidationResult } |
  { 'Err' : ProtocolError };
export type Result_11 = { 'Ok' : string } |
  { 'Err' : ProtocolError };
export type Result_2 = { 'Ok' : SuccessWithFee } |
  { 'Err' : ProtocolError };
export type Result_3 = { 'Ok' : BotLiquidationResult } |
  { 'Err' : ProtocolError };
export type Result_4 = { 'Ok' : [] | [bigint] } |
  { 'Err' : ProtocolError };
export type Result_5 = { 'Ok' : number } |
  { 'Err' : ProtocolError };
export type Result_6 = { 'Ok' : ConsentInfo } |
  { 'Err' : Icrc21Error };
export type Result_7 = { 'Ok' : OpenVaultSuccess } |
  { 'Err' : ProtocolError };
export type Result_8 = { 'Ok' : boolean } |
  { 'Err' : ProtocolError };
export type Result_9 = { 'Ok' : ReserveRedemptionResult } |
  { 'Err' : ProtocolError };
export interface StabilityPoolConfig {
  'enabled' : boolean,
  'liquidation_discount' : bigint,
  'stability_pool_canister' : [] | [Principal],
}
export interface StabilityPoolLiquidationResult {
  'fee' : bigint,
  'collateral_price_e8s' : bigint,
  'block_index' : bigint,
  'vault_id' : bigint,
  'liquidated_debt' : bigint,
  'success' : boolean,
  'collateral_type' : string,
  'collateral_received' : bigint,
}
export type StableTokenType = { 'CKUSDC' : null } |
  { 'CKUSDT' : null };
export interface StandardRecord { 'url' : string, 'name' : string }
export interface SuccessWithFee {
  'block_index' : bigint,
  'fee_amount_paid' : bigint,
  'collateral_amount_received' : [] | [bigint],
}
export type TransferError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'BadBurn' : { 'min_burn_amount' : bigint } } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export type TransferFromError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'InsufficientAllowance' : { 'allowance' : bigint } } |
  { 'BadBurn' : { 'min_burn_amount' : bigint } } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export interface TreasuryStats {
  'pending_treasury_collateral_entries' : bigint,
  'liquidation_protocol_share' : number,
  'interest_flush_threshold_e8s' : bigint,
  'pending_treasury_interest' : bigint,
  'treasury_principal' : [] | [Principal],
  'total_accrued_interest_system' : bigint,
  'pending_interest_for_pools_total' : bigint,
}
export interface UpgradeArg {
  'mode' : [] | [Mode],
  'description' : [] | [string],
}
export interface Vault {
  'collateral_amount' : bigint,
  'owner' : Principal,
  'vault_id' : bigint,
  'collateral_type' : Principal,
  'last_accrual_time' : bigint,
  'accrued_interest' : bigint,
  'borrowed_icusd_amount' : bigint,
}
export interface VaultArg { 'vault_id' : bigint, 'amount' : bigint }
export interface VaultArgWithToken {
  'vault_id' : bigint,
  'amount' : bigint,
  'token_type' : StableTokenType,
}
export type XrcAssetClass = { 'Cryptocurrency' : null } |
  { 'FiatCurrency' : null };
export interface _SERVICE {
  'add_collateral_token' : ActorMethod<[AddCollateralArg], Result>,
  'add_margin_to_vault' : ActorMethod<[VaultArg], Result_1>,
  'add_margin_with_deposit' : ActorMethod<[bigint], Result_1>,
  'admin_correct_vault_collateral' : ActorMethod<
    [bigint, bigint, string],
    Result
  >,
  'admin_mint_icusd' : ActorMethod<[bigint, Principal, string], Result_1>,
  'admin_sweep_to_treasury' : ActorMethod<[string], Result_1>,
  'borrow_from_vault' : ActorMethod<[VaultArg], Result_2>,
  'bot_cancel_liquidation' : ActorMethod<[bigint], Result>,
  'bot_claim_liquidation' : ActorMethod<[bigint], Result_3>,
  'bot_confirm_liquidation' : ActorMethod<[bigint], Result>,
  'bot_deposit_to_reserves' : ActorMethod<[bigint], Result>,
  'claim_liquidity_returns' : ActorMethod<[], Result_1>,
  'clear_stuck_operations' : ActorMethod<[[] | [Principal]], Result_1>,
  'close_vault' : ActorMethod<[bigint], Result_4>,
  'coingecko_transform' : ActorMethod<
    [
      {
        'context' : Uint8Array | number[],
        'response' : {
          'status' : bigint,
          'body' : Uint8Array | number[],
          'headers' : Array<{ 'value' : string, 'name' : string }>,
        },
      },
    ],
    {
      'status' : bigint,
      'body' : Uint8Array | number[],
      'headers' : Array<{ 'value' : string, 'name' : string }>,
    }
  >,
  'dev_force_bot_liquidate' : ActorMethod<[bigint], Result_3>,
  'dev_force_partial_bot_liquidate' : ActorMethod<[bigint], Result_3>,
  'dev_set_collateral_price' : ActorMethod<[Principal, number], Result_11>,
  'dev_test_cascade_liquidation' : ActorMethod<[bigint], Result_11>,
  'dev_test_pool_only_liquidation' : ActorMethod<[bigint], Result_11>,
  'enter_recovery_mode' : ActorMethod<[], Result>,
  'exit_recovery_mode' : ActorMethod<[], Result>,
  'freeze_protocol' : ActorMethod<[], Result>,
  'get_all_vaults' : ActorMethod<[], Array<CandidVault>>,
  'get_borrowing_fee' : ActorMethod<[], number>,
  'get_bot_allowed_collateral_types' : ActorMethod<[], Array<Principal>>,
  'get_bot_stats' : ActorMethod<[], BotStatsResponse>,
  'get_ckstable_repay_fee' : ActorMethod<[], number>,
  'get_collateral_config' : ActorMethod<[Principal], [] | [CollateralConfig]>,
  'get_collateral_totals' : ActorMethod<[], Array<CollateralTotals>>,
  'get_deposit_account' : ActorMethod<[[] | [Principal]], Account>,
  'get_event_count' : ActorMethod<[], bigint>,
  'get_events' : ActorMethod<[GetEventsArg], Array<Event>>,
  'get_events_by_principal' : ActorMethod<[Principal], Array<[bigint, Event]>>,
  'get_events_filtered' : ActorMethod<
    [GetEventsArg],
    GetEventsFilteredResponse
  >,
  'get_fees' : ActorMethod<[bigint], Fees>,
  'get_global_icusd_mint_cap' : ActorMethod<[], bigint>,
  'get_interest_pool_share' : ActorMethod<[], number>,
  'get_interest_split' : ActorMethod<[], Array<InterestSplitArg>>,
  'get_liquidatable_vaults' : ActorMethod<[], Array<CandidVault>>,
  'get_liquidation_bonus' : ActorMethod<[], number>,
  'get_liquidation_protocol_share' : ActorMethod<[], number>,
  'get_liquidity_status' : ActorMethod<[Principal], LiquidityStatus>,
  'get_min_icusd_amount' : ActorMethod<[], bigint>,
  'get_protocol_snapshots' : ActorMethod<
    [GetEventsArg],
    Array<ProtocolSnapshot>
  >,
  'get_protocol_status' : ActorMethod<[], ProtocolStatus>,
  'get_recovery_cr_multiplier' : ActorMethod<[], number>,
  'get_recovery_target_cr' : ActorMethod<[], number>,
  'get_redemption_fee_ceiling' : ActorMethod<[], number>,
  'get_redemption_fee_floor' : ActorMethod<[], number>,
  'get_redemption_rate' : ActorMethod<[], number>,
  'get_reserve_balances' : ActorMethod<[], Array<ReserveBalance>>,
  'get_reserve_redemption_fee' : ActorMethod<[], number>,
  'get_reserve_redemptions_enabled' : ActorMethod<[], boolean>,
  'get_rmr_ceiling' : ActorMethod<[], number>,
  'get_rmr_ceiling_cr' : ActorMethod<[], number>,
  'get_rmr_floor' : ActorMethod<[], number>,
  'get_rmr_floor_cr' : ActorMethod<[], number>,
  'get_snapshot_count' : ActorMethod<[], bigint>,
  'get_stability_pool_config' : ActorMethod<[], StabilityPoolConfig>,
  'get_stability_pool_principal' : ActorMethod<[], [] | [Principal]>,
  'get_stable_token_enabled' : ActorMethod<[StableTokenType], boolean>,
  'get_supported_collateral_types' : ActorMethod<
    [],
    Array<[Principal, CollateralStatus]>
  >,
  'get_three_pool_canister' : ActorMethod<[], [] | [Principal]>,
  'get_treasury_principal' : ActorMethod<[], [] | [Principal]>,
  'get_treasury_stats' : ActorMethod<[], TreasuryStats>,
  'get_vault_history' : ActorMethod<[bigint], Array<Event>>,
  'get_vault_interest_rate' : ActorMethod<[bigint], Result_5>,
  'get_vaults' : ActorMethod<[[] | [Principal]], Array<CandidVault>>,
  'http_request' : ActorMethod<[HttpRequest], HttpResponse>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    Result_6
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'liquidate_vault' : ActorMethod<[bigint], Result_2>,
  'liquidate_vault_partial' : ActorMethod<[VaultArg], Result_2>,
  'liquidate_vault_partial_with_stable' : ActorMethod<
    [VaultArgWithToken],
    Result_2
  >,
  'open_vault' : ActorMethod<[bigint, [] | [Principal]], Result_7>,
  'open_vault_and_borrow' : ActorMethod<
    [bigint, bigint, [] | [Principal]],
    Result_7
  >,
  'open_vault_with_deposit' : ActorMethod<[bigint, [] | [Principal]], Result_7>,
  'partial_liquidate_vault' : ActorMethod<[VaultArg], Result_2>,
  'partial_repay_to_vault' : ActorMethod<[VaultArg], Result_1>,
  'provide_liquidity' : ActorMethod<[bigint], Result_1>,
  'recover_pending_transfer' : ActorMethod<[bigint], Result_8>,
  'redeem_collateral' : ActorMethod<[Principal, bigint], Result_2>,
  'redeem_icp' : ActorMethod<[bigint], Result_2>,
  'redeem_reserves' : ActorMethod<[bigint, [] | [Principal]], Result_9>,
  'repay_to_vault' : ActorMethod<[VaultArg], Result_1>,
  'repay_to_vault_with_stable' : ActorMethod<[VaultArgWithToken], Result_1>,
  'reset_bot_budget' : ActorMethod<[bigint], Result>,
  'set_borrowing_fee' : ActorMethod<[number], Result>,
  'set_borrowing_fee_curve' : ActorMethod<[[] | [string]], Result>,
  'set_bot_allowed_collateral_types' : ActorMethod<[Array<Principal>], Result>,
  'set_ckstable_repay_fee' : ActorMethod<[number], Result>,
  'set_collateral_borrowing_fee' : ActorMethod<[Principal, number], Result>,
  'set_collateral_debt_ceiling' : ActorMethod<[Principal, bigint], Result>,
  'set_collateral_status' : ActorMethod<[Principal, CollateralStatus], Result>,
  'set_global_icusd_mint_cap' : ActorMethod<[bigint], Result>,
  'set_healthy_cr' : ActorMethod<[Principal, [] | [number]], Result>,
  'set_interest_flush_threshold' : ActorMethod<[bigint], Result>,
  'set_interest_pool_share' : ActorMethod<[number], Result>,
  'set_interest_rate' : ActorMethod<[Principal, number], Result>,
  'set_interest_split' : ActorMethod<[Array<InterestSplitArg>], Result>,
  'set_liquidation_bonus' : ActorMethod<[number], Result>,
  'set_liquidation_bot_config' : ActorMethod<[Principal, bigint], Result>,
  'set_liquidation_protocol_share' : ActorMethod<[number], Result>,
  'set_lst_haircut' : ActorMethod<[Principal, number], Result>,
  'set_min_icusd_amount' : ActorMethod<[bigint], Result>,
  'set_rate_curve_markers' : ActorMethod<
    [[] | [Principal], Array<[number, number]>],
    Result
  >,
  'set_recovery_cr_multiplier' : ActorMethod<[number], Result>,
  'set_recovery_parameters' : ActorMethod<
    [Principal, [] | [number], [] | [number]],
    Result
  >,
  'set_recovery_rate_curve' : ActorMethod<[Array<[string, number]>], Result>,
  'set_recovery_target_cr' : ActorMethod<[number], Result>,
  'set_redemption_fee_ceiling' : ActorMethod<[number], Result>,
  'set_redemption_fee_floor' : ActorMethod<[number], Result>,
  'set_reserve_redemption_fee' : ActorMethod<[number], Result>,
  'set_reserve_redemptions_enabled' : ActorMethod<[boolean], Result>,
  'set_rmr_ceiling' : ActorMethod<[number], Result>,
  'set_rmr_ceiling_cr' : ActorMethod<[number], Result>,
  'set_rmr_floor' : ActorMethod<[number], Result>,
  'set_rmr_floor_cr' : ActorMethod<[number], Result>,
  'set_stability_pool_principal' : ActorMethod<[Principal], Result>,
  'set_stable_ledger_principal' : ActorMethod<
    [StableTokenType, Principal],
    Result
  >,
  'set_stable_token_enabled' : ActorMethod<[StableTokenType, boolean], Result>,
  'set_three_pool_canister' : ActorMethod<[Principal], Result>,
  'set_treasury_principal' : ActorMethod<[Principal], Result>,
  'stability_pool_liquidate' : ActorMethod<[bigint, bigint], Result_10>,
  'stability_pool_liquidate_debt_burned' : ActorMethod<
    [bigint, bigint],
    Result_10
  >,
  'unfreeze_protocol' : ActorMethod<[], Result>,
  'update_collateral_config' : ActorMethod<
    [Principal, CollateralConfig],
    Result
  >,
  'withdraw_and_close_vault' : ActorMethod<[bigint], Result_4>,
  'withdraw_collateral' : ActorMethod<[bigint], Result_1>,
  'withdraw_liquidity' : ActorMethod<[bigint], Result_1>,
  'withdraw_partial_collateral' : ActorMethod<[VaultArg], Result_1>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
