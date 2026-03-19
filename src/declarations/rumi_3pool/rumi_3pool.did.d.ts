import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

/**
 * ─── ICRC-1 / ICRC-2 Types ───
 */
export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface Allowance {
  'allowance' : bigint,
  'expires_at' : [] | [bigint],
}
export interface AllowanceArgs { 'account' : Account, 'spender' : Account }
export interface ApproveArgs {
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
  'expected_allowance' : [] | [bigint],
  'expires_at' : [] | [bigint],
  'spender' : Account,
}
export type ApproveError = {
    'GenericError' : { 'message' : string, 'error_code' : bigint }
  } |
  { 'TemporarilyUnavailable' : null } |
  { 'Duplicate' : { 'duplicate_of' : bigint } } |
  { 'BadFee' : { 'expected_fee' : bigint } } |
  { 'AllowanceChanged' : { 'current_allowance' : bigint } } |
  { 'CreatedInFuture' : { 'ledger_time' : bigint } } |
  { 'TooOld' : null } |
  { 'Expired' : { 'ledger_time' : bigint } } |
  { 'InsufficientFunds' : { 'balance' : bigint } };
export interface ArchiveInfo {
  'end' : bigint,
  'canister_id' : Principal,
  'start' : bigint,
}
export interface ArchivedBlocks {
  'args' : Array<GetBlocksArgs>,
  'callback' : [Principal, string],
}
export type ArchivedBlocksCallback = ActorMethod<
  [Array<GetBlocksArgs>],
  GetBlocksResult
>;
export interface AuthorizedRedeemAndBurnArgs {
  'token_amount' : bigint,
  'lp_amount' : bigint,
  'max_slippage_bps' : number,
  'token_ledger' : Principal,
}
export interface BlockWithId { 'id' : bigint, 'block' : Icrc3Value }
export interface ConsentInfo {
  'metadata' : ConsentMessageMetadata,
  'consent_message' : ConsentMessage,
}
export type ConsentMessage = {
    'LineDisplayMessage' : { 'pages' : Array<LineDisplayPage> }
  } |
  { 'GenericDisplayMessage' : string };
/**
 * ─── ICRC-21 / ICRC-28 / ICRC-10 Types ───
 */
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
export interface GetArchivesArgs { 'from' : [] | [Principal] }
export interface GetArchivesResult { 'archives' : Array<ArchiveInfo> }
export interface GetBlocksArgs { 'start' : bigint, 'length' : bigint }
export interface GetBlocksResult {
  'log_length' : bigint,
  'blocks' : Array<BlockWithId>,
  'archived_blocks' : Array<ArchivedBlocks>,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface Icrc3DataCertificate {
  'certificate' : Uint8Array | number[],
  'hash_tree' : Uint8Array | number[],
}
/**
 * ─── ICRC-3 Types ───
 */
export type Icrc3Value = { 'Int' : bigint } |
  { 'Map' : Array<[string, Icrc3Value]> } |
  { 'Nat' : bigint } |
  { 'Blob' : Uint8Array | number[] } |
  { 'Text' : string } |
  { 'Array' : Array<Icrc3Value> };
export interface LineDisplayPage { 'lines' : Array<string> }
export type MetadataValue = { 'Int' : bigint } |
  { 'Nat' : bigint } |
  { 'Blob' : Uint8Array | number[] } |
  { 'Text' : string };
export interface PoolStatus {
  'virtual_price' : bigint,
  'admin_fee_bps' : bigint,
  'swap_fee_bps' : bigint,
  'current_a' : bigint,
  'tokens' : Array<TokenConfig>,
  'lp_total_supply' : bigint,
  'balances' : Array<bigint>,
}
export interface RedeemAndBurnResult {
  'lp_amount_burned' : bigint,
  'burn_block_index' : bigint,
  'token_amount_burned' : bigint,
}
export interface StandardRecord { 'url' : string, 'name' : string }
export interface SupportedBlockType { 'url' : string, 'block_type' : string }
export type ThreePoolError = {
    'InsufficientOutput' : { 'actual' : bigint, 'expected_min' : bigint }
  } |
  { 'PoolPaused' : null } |
  { 'InvalidCoinIndex' : null } |
  { 'BurnSlippageExceeded' : { 'actual_bps' : number, 'max_bps' : number } } |
  { 'NotAuthorizedBurnCaller' : null } |
  { 'ZeroAmount' : null } |
  { 'InsufficientLpBalance' : { 'available' : bigint, 'required' : bigint } } |
  { 'BurnFailed' : { 'token' : string, 'reason' : string } } |
  { 'MathOverflow' : null } |
  { 'Unauthorized' : null } |
  { 'InvariantNotConverged' : null } |
  { 'InsufficientLiquidity' : null } |
  { 'TransferFailed' : { 'token' : string, 'reason' : string } } |
  { 'SlippageExceeded' : null } |
  { 'PoolEmpty' : null } |
  {
    'InsufficientPoolBalance' : {
      'token' : string,
      'available' : bigint,
      'required' : bigint,
    }
  };
export interface ThreePoolInitArgs {
  'admin_fee_bps' : bigint,
  'admin' : Principal,
  'swap_fee_bps' : bigint,
  'initial_a' : bigint,
  'tokens' : Array<TokenConfig>,
}
export interface TokenConfig {
  'decimals' : number,
  'precision_mul' : bigint,
  'ledger_id' : Principal,
  'symbol' : string,
}
export interface TransferArg {
  'to' : Account,
  'fee' : [] | [bigint],
  'memo' : [] | [Uint8Array | number[]],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
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
export interface TransferFromArgs {
  'to' : Account,
  'fee' : [] | [bigint],
  'spender_subaccount' : [] | [Uint8Array | number[]],
  'from' : Account,
  'memo' : [] | [Uint8Array | number[]],
  'created_at_time' : [] | [bigint],
  'amount' : bigint,
}
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
export interface VirtualPriceSnapshot {
  'virtual_price' : bigint,
  'timestamp_secs' : bigint,
  'lp_total_supply' : bigint,
}
export interface _SERVICE {
  'add_authorized_burn_caller' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'add_liquidity' : ActorMethod<
    [Array<bigint>, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'authorized_redeem_and_burn' : ActorMethod<
    [AuthorizedRedeemAndBurnArgs],
    { 'Ok' : RedeemAndBurnResult } |
      { 'Err' : ThreePoolError }
  >,
  'calc_add_liquidity_query' : ActorMethod<
    [Array<bigint>, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'calc_remove_liquidity_query' : ActorMethod<
    [bigint],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
  'calc_remove_one_coin_query' : ActorMethod<
    [bigint, number],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'calc_swap' : ActorMethod<
    [number, number, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'donate' : ActorMethod<
    [number, bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'get_admin_fees' : ActorMethod<[], Array<bigint>>,
  'get_authorized_burn_callers' : ActorMethod<[], Array<Principal>>,
  'get_lp_balance' : ActorMethod<[Principal], bigint>,
  'get_pool_status' : ActorMethod<[], PoolStatus>,
  'get_vp_snapshots' : ActorMethod<[], Array<VirtualPriceSnapshot>>,
  'health' : ActorMethod<[], string>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc1_balance_of' : ActorMethod<[Account], bigint>,
  'icrc1_decimals' : ActorMethod<[], number>,
  'icrc1_fee' : ActorMethod<[], bigint>,
  'icrc1_metadata' : ActorMethod<[], Array<[string, MetadataValue]>>,
  'icrc1_minting_account' : ActorMethod<[], [] | [Account]>,
  /**
   * ICRC-1 (3USD LP Token)
   */
  'icrc1_name' : ActorMethod<[], string>,
  'icrc1_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc1_symbol' : ActorMethod<[], string>,
  'icrc1_total_supply' : ActorMethod<[], bigint>,
  'icrc1_transfer' : ActorMethod<
    [TransferArg],
    { 'Ok' : bigint } |
      { 'Err' : TransferError }
  >,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    { 'Ok' : ConsentInfo } |
      { 'Err' : Icrc21Error }
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'icrc2_allowance' : ActorMethod<[AllowanceArgs], Allowance>,
  /**
   * ICRC-2
   */
  'icrc2_approve' : ActorMethod<
    [ApproveArgs],
    { 'Ok' : bigint } |
      { 'Err' : ApproveError }
  >,
  'icrc2_transfer_from' : ActorMethod<
    [TransferFromArgs],
    { 'Ok' : bigint } |
      { 'Err' : TransferFromError }
  >,
  'icrc3_get_archives' : ActorMethod<[GetArchivesArgs], GetArchivesResult>,
  /**
   * ICRC-3 (Transaction Log)
   */
  'icrc3_get_blocks' : ActorMethod<[Array<GetBlocksArgs>], GetBlocksResult>,
  'icrc3_get_tip_certificate' : ActorMethod<[], [] | [Icrc3DataCertificate]>,
  'icrc3_supported_block_types' : ActorMethod<[], Array<SupportedBlockType>>,
  'ramp_a' : ActorMethod<
    [bigint, bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'remove_authorized_burn_caller' : ActorMethod<
    [Principal],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'remove_liquidity' : ActorMethod<
    [bigint, Array<bigint>],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
  'remove_one_coin' : ActorMethod<
    [bigint, number, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'set_admin_fee' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'set_paused' : ActorMethod<
    [boolean],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'set_swap_fee' : ActorMethod<
    [bigint],
    { 'Ok' : null } |
      { 'Err' : ThreePoolError }
  >,
  'stop_ramp_a' : ActorMethod<[], { 'Ok' : null } | { 'Err' : ThreePoolError }>,
  'swap' : ActorMethod<
    [number, number, bigint, bigint],
    { 'Ok' : bigint } |
      { 'Err' : ThreePoolError }
  >,
  'withdraw_admin_fees' : ActorMethod<
    [],
    { 'Ok' : Array<bigint> } |
      { 'Err' : ThreePoolError }
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
