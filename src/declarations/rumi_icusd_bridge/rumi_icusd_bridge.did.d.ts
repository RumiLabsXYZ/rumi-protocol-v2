import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface AuditEntry {
  'seq' : bigint,
  'tx_id' : [] | [bigint],
  'deposit_id' : [] | [bigint],
  'at_ns' : bigint,
  'note' : string,
  'transition' : [] | [Transition],
  'transfer_id' : [] | [string],
}
export interface BaseAddresses { 'admin' : string, 'minter' : string }
export interface BaseInclusion {
  'block_hash' : string,
  'block_number' : bigint,
  'tx_hash' : string,
}
export type BaseTxStage = { 'Queued' : null } |
  { 'Finalized' : { 'block_number' : bigint, 'success' : boolean } } |
  { 'Included' : { 'block_number' : bigint } } |
  { 'Submitted' : null };
export type BridgeArg = { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs };
export type BridgeError = { 'Rpc' : string } |
  { 'Internal' : string } |
  { 'Anonymous' : null } |
  { 'Paused' : string } |
  {
    'InsufficientAllowance' : {
      'allowance_e8s' : bigint,
      'required_e8s' : bigint,
    }
  } |
  { 'AmountBelowMinimum' : { 'min_e8s' : bigint } } |
  { 'CapacityExceeded' : { 'available_e8s' : bigint } } |
  { 'NotFound' : null } |
  { 'TooManyActiveOperations' : null } |
  { 'Unauthorized' : null } |
  { 'NotOwner' : null } |
  { 'InvalidRecipient' : string } |
  {
    'RateLimited' : { 'window_resets_at_ns' : bigint, 'available_e8s' : bigint }
  } |
  { 'NotConfigured' : string } |
  { 'InvalidArgument' : string } |
  { 'Ledger' : string } |
  { 'Halted' : string } |
  { 'RequestIdConflict' : null } |
  { 'InvalidState' : string } |
  { 'FeeChanged' : { 'current_fee_e8s' : bigint } } |
  {
    'InsufficientFunds' : { 'required_e8s' : bigint, 'balance_e8s' : bigint }
  } |
  { 'AmountAboveMaximum' : { 'max_e8s' : bigint } };
export interface BridgeFees {
  'release_fee_e8s' : bigint,
  'deposit_fee_e8s' : bigint,
}
export interface BridgeInfo {
  'gas' : GasConfig,
  'observer' : ObserverStatus,
  'mint_window_used_e8s' : bigint,
  'contract' : [] | [ContractBindingView],
  'min_quorum' : number,
  'escrow_account' : Account,
  'fees' : BridgeFees,
  'active_releases' : number,
  'observer_config' : ObserverConfig,
  'finality_policy' : string,
  'window_resets_at_ns' : bigint,
  'lanes' : Array<LaneStatus>,
  'release_window_used_e8s' : bigint,
  'version' : string,
  'chain_id' : bigint,
  'ledger_fee_e8s' : [] | [bigint],
  'active_deposits' : number,
  'halted' : [] | [string],
  'icusd_ledger' : Principal,
  'limits' : BridgeLimits,
  'rpc_service_count' : number,
  'pauses' : Pauses,
}
export interface BridgeLimits {
  'max_active_per_caller' : number,
  'release_window_limit_e8s' : bigint,
  'max_deposit_e8s' : bigint,
  'max_active_global' : number,
  'min_deposit_e8s' : bigint,
  'window_seconds' : bigint,
  'mint_window_limit_e8s' : bigint,
  'max_outstanding_e8s' : bigint,
}
export interface BridgeToBaseArgs {
  'base_recipient' : string,
  'client_request_id' : Uint8Array | number[],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'amount_e8s' : bigint,
  'expected_deposit_fee_e8s' : bigint,
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
export type ContractAdminAction = { 'UnpauseBridgeOut' : null } |
  { 'SetSupplyCap' : { 'cap_e8s' : bigint } } |
  { 'SetMinBridgeOut' : { 'minimum_e8s' : bigint } } |
  { 'SetMintRateLimit' : { 'limit_e8s' : bigint, 'window_seconds' : bigint } } |
  { 'PauseBridgeOut' : null } |
  { 'UnpauseMint' : null } |
  { 'PauseMint' : null };
export interface ContractBindingView {
  'bound_at_ns' : bigint,
  'supply_cap_e8s' : bigint,
  'mint_window_seconds' : bigint,
  'address' : string,
  'mint_window_limit_e8s' : bigint,
  'start_block' : bigint,
  'min_bridge_out_e8s' : bigint,
}
export type DepositKindView = { 'UserDeposit' : null } |
  { 'Remint' : { 'transfer_id' : string } };
export type DepositStatus = { 'Refunding' : null } |
  { 'Refunded' : null } |
  { 'MintHeld' : { 'reason' : string } } |
  { 'PullPending' : null } |
  { 'PullFailed' : { 'reason' : string } } |
  { 'Delivered' : null } |
  { 'MintInFlight' : null } |
  { 'Escrowed' : null } |
  { 'PullUnknown' : null };
export interface DepositView {
  'id' : bigint,
  'base_recipient' : string,
  'status' : DepositStatus,
  'mint_tx_hashes' : Array<string>,
  'client_request_id' : Uint8Array | number[],
  'refund_amount_e8s' : [] | [bigint],
  'owner' : Principal,
  'from' : [] | [Account],
  'kind' : DepositKindView,
  'mint_stage' : [] | [BaseTxStage],
  'mint_id' : string,
  'pull_block_index' : [] | [bigint],
  'mint_amount_e8s' : bigint,
  'updated_at_ns' : bigint,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'delivered' : [] | [BaseInclusion],
  'refund_block_index' : [] | [bigint],
  'deposit_fee_e8s' : bigint,
}
export type DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
export interface ErrorInfo { 'description' : string }
export interface GasConfig {
  'replace_after_secs' : bigint,
  'mint_gas_limit' : bigint,
  'max_priority_fee_per_gas_wei' : bigint,
  'admin_gas_limit' : bigint,
  'max_fee_per_gas_wei' : bigint,
  'min_gas_reserve_wei' : bigint,
}
export interface HintView {
  'submitted_at_ns' : bigint,
  'tx_hash' : string,
  'submitted_by' : Principal,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface InitArgs {
  'ecdsa_key_name' : string,
  'base_chain_id' : bigint,
  'rpc_services' : Array<RpcServiceConfig>,
  'min_quorum' : number,
  'fee_recipient' : [] | [Account],
  'evm_rpc' : Principal,
  'admins' : Array<Principal>,
  'icusd_ledger' : Principal,
}
export type L2MainnetService = { 'Alchemy' : null } |
  { 'Llama' : null } |
  { 'BlockPi' : null } |
  { 'PublicNode' : null } |
  { 'Ankr' : null };
export type LaneKind = { 'Minter' : null } |
  { 'Admin' : null };
export interface LaneStatus {
  'next_nonce' : [] | [bigint],
  'unfinalized_txs' : number,
  'lane' : LaneKind,
  'address' : [] | [string],
  'awaiting_gas' : boolean,
  'halted' : [] | [string],
  'unincluded_tx' : [] | [bigint],
}
export interface LineDisplayPage { 'lines' : Array<string> }
export interface ListDepositsArgs {
  'start_after' : [] | [bigint],
  'limit' : number,
}
export interface ListDepositsForOwnerArgs {
  'owner' : Principal,
  'start_after' : [] | [bigint],
  'limit' : number,
}
export interface ListReleasesArgs {
  'start_after_sequence' : [] | [bigint],
  'owner' : Principal,
  'limit' : number,
}
export interface ListReleasesForSenderArgs {
  'start_after_sequence' : [] | [bigint],
  'base_sender' : string,
  'limit' : number,
}
export interface ObserverConfig {
  'idle_interval_secs' : bigint,
  'cycles_per_rpc_call' : bigint,
  'logs_chunk_blocks' : bigint,
  'logs_max_response_bytes' : bigint,
  'default_max_response_bytes' : bigint,
  'active_interval_secs' : bigint,
}
export interface ObserverStatus {
  'last_error' : [] | [string],
  'deferred_since_ns' : [] | [bigint],
  'last_run_ns' : [] | [bigint],
  'processed_count' : bigint,
  'cursor_block' : bigint,
  'deferred_reason' : [] | [string],
  'pending_hints' : Array<HintView>,
  'last_finalized_block' : [] | [bigint],
}
export interface Pauses {
  'mints' : boolean,
  'observer' : boolean,
  'releases' : boolean,
  'deposits' : boolean,
}
export type ReleaseStatus = { 'Reminted' : { 'deposit_id' : bigint } } |
  { 'TransferUnknown' : null } |
  { 'Held' : { 'reason' : string } } |
  { 'Released' : null } |
  { 'Reminting' : { 'deposit_id' : bigint } } |
  { 'Pending' : null };
export interface ReleaseView {
  'status' : ReleaseStatus,
  'destination' : [] | [Account],
  'raw_icp_principal' : Uint8Array | number[],
  'log_index' : bigint,
  'release_block_index' : [] | [bigint],
  'base_sender' : string,
  'updated_at_ns' : bigint,
  'transfer_id' : string,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'release_fee_e8s' : bigint,
  'ledger_fee_e8s' : [] | [bigint],
  'base_tx_hash' : string,
  'base_block_number' : bigint,
  'amount_to_receive_e8s' : [] | [bigint],
  'icp_subaccount' : Uint8Array | number[],
  'sequence' : bigint,
}
export type Result = { 'Ok' : ContractBindingView } |
  { 'Err' : BridgeError };
export type Result_1 = { 'Ok' : null } |
  { 'Err' : BridgeError };
export type Result_2 = { 'Ok' : SupplyAudit } |
  { 'Err' : BridgeError };
export type Result_3 = { 'Ok' : DepositView } |
  { 'Err' : BridgeError };
export type Result_4 = { 'Ok' : bigint } |
  { 'Err' : BridgeError };
export type Result_5 = { 'Ok' : BaseAddresses } |
  { 'Err' : BridgeError };
export type Result_6 = { 'Ok' : ConsentInfo } |
  { 'Err' : Icrc21Error };
export type RpcServiceConfig = { 'BaseMainnet' : L2MainnetService } |
  { 'Custom' : { 'url' : string } };
export interface StandardRecord { 'url' : string, 'name' : string }
export interface SupplyAudit {
  'fees_e8s' : bigint,
  'icp_ledger_total_supply_e8s' : [] | [bigint],
  'inflight_mint_e8s' : bigint,
  'pending_refund_e8s' : bigint,
  'base_total_supply_e8s' : [] | [bigint],
  'reserved_pulls_e8s' : bigint,
  'pending_mint_e8s' : bigint,
  'escrow_account' : Account,
  'circulating_on_base_e8s' : [] | [bigint],
  'base_supply_exact_match' : [] | [boolean],
  'escrow_balance_e8s' : [] | [bigint],
  'outstanding_base_e8s' : bigint,
  'surplus_e8s' : [] | [bigint],
  'base_supply_block' : [] | [bigint],
  'reporting_note' : string,
  'required_escrow_e8s' : bigint,
  'liabilities_e8s' : bigint,
  'held_e8s' : bigint,
  'circulating_on_icp_e8s' : [] | [bigint],
  'pending_release_e8s' : bigint,
  'escrow_balance_checked_at_ns' : [] | [bigint],
  'deficit_e8s' : [] | [bigint],
  'unknown_outflow_e8s' : bigint,
  'coverage_ok' : [] | [boolean],
  'base_supply_within_bound' : [] | [boolean],
}
export type Transition = { 'RemintDelivered' : { 'amount' : bigint } } |
  { 'BridgeOutHeld' : { 'amount' : bigint } } |
  { 'PullFailed' : { 'amount' : bigint } } |
  { 'SweepCompleted' : { 'amount' : bigint } } |
  { 'OutflowResolved' : { 'amount' : bigint } } |
  { 'BridgeOutAccepted' : { 'amount' : bigint } } |
  { 'RefundStarted' : { 'amount' : bigint } } |
  { 'AdmitDeposit' : { 'amount' : bigint } } |
  { 'RemintStarted' : { 'amount' : bigint } } |
  { 'ReleaseHeld' : { 'amount' : bigint } } |
  { 'MintDelivered' : { 'fee' : bigint, 'amount' : bigint } } |
  { 'RefundCompleted' : { 'amount' : bigint } } |
  { 'ReleaseCompleted' : { 'amount' : bigint, 'release_fee' : bigint } } |
  { 'PullSucceeded' : { 'amount' : bigint } } |
  { 'OutflowUnknown' : { 'amount' : bigint } };
export interface UpgradeArgs { 'admins' : [] | [Array<Principal>] }
export interface _SERVICE {
  'admin_bind_contract' : ActorMethod<[string], Result>,
  'admin_cancel_stuck_tx' : ActorMethod<[bigint], Result_1>,
  'admin_clear_halt' : ActorMethod<[], Result_2>,
  'admin_clear_lane_halt' : ActorMethod<[LaneKind], Result_1>,
  'admin_reconcile' : ActorMethod<[], Result_2>,
  'admin_retry_deposit' : ActorMethod<[bigint], Result_3>,
  'admin_run_tick' : ActorMethod<[], Result_1>,
  'admin_set_fees' : ActorMethod<[BridgeFees], Result_1>,
  'admin_set_gas' : ActorMethod<[GasConfig], Result_1>,
  'admin_set_limits' : ActorMethod<[BridgeLimits], Result_1>,
  'admin_set_observer_config' : ActorMethod<[ObserverConfig], Result_1>,
  'admin_set_pauses' : ActorMethod<[Pauses], Result_1>,
  'admin_set_rpc_services' : ActorMethod<
    [Array<RpcServiceConfig>, number],
    Result_1
  >,
  'admin_submit_contract_action' : ActorMethod<[ContractAdminAction], Result_4>,
  'admin_sweep_fees' : ActorMethod<[bigint], Result_4>,
  'bridge_to_base' : ActorMethod<[BridgeToBaseArgs], Result_3>,
  'bridge_version' : ActorMethod<[], string>,
  'get_audit_log' : ActorMethod<[bigint, number], Array<AuditEntry>>,
  'get_base_addresses' : ActorMethod<[], Result_5>,
  'get_bridge_info' : ActorMethod<[], BridgeInfo>,
  'get_deposit' : ActorMethod<[bigint], [] | [DepositView]>,
  'get_deposit_by_request' : ActorMethod<
    [Principal, Uint8Array | number[]],
    [] | [DepositView]
  >,
  'get_release' : ActorMethod<[string], [] | [ReleaseView]>,
  'get_supply_audit' : ActorMethod<[], SupplyAudit>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    Result_6
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'list_deposits_for_owner' : ActorMethod<
    [ListDepositsForOwnerArgs],
    Array<DepositView>
  >,
  'list_my_deposits' : ActorMethod<[ListDepositsArgs], Array<DepositView>>,
  'list_releases_for_base_sender' : ActorMethod<
    [ListReleasesForSenderArgs],
    Array<ReleaseView>
  >,
  'list_releases_for_owner' : ActorMethod<
    [ListReleasesArgs],
    Array<ReleaseView>
  >,
  'notify_base_bridge_out' : ActorMethod<[string], Result_1>,
  'request_deposit_refund' : ActorMethod<[bigint], Result_3>,
  'set_admins' : ActorMethod<[Array<Principal>], Result_1>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
