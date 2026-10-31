import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface AuditEntry {
  'seq' : bigint,
  'return_id' : [] | [string],
  'deposit_id' : [] | [bigint],
  'at_ns' : bigint,
  'op_id' : [] | [bigint],
  'note' : string,
  'transition' : [] | [Transition],
}
export interface BootstrapStatus {
  'return_account' : [] | [string],
  'bind_note' : [] | [string],
  'mint_authority' : [] | [string],
  'return_owner' : [] | [string],
  'pool_free' : number,
  'mint_state' : MintState,
  'pool_registered' : number,
  'pool_reserved' : number,
  'keys_derived' : boolean,
  'fee_payer' : [] | [string],
  'pending_bootstrap_txs' : number,
  'mint_account' : [] | [string],
  'pool_unverified' : number,
}
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
  'rpc' : RpcSettings,
  'initialized' : boolean,
  'fees' : BridgeFees,
  'lane' : LaneSettings,
  'memo_program' : string,
  'escrow_subaccount' : string,
  'sol_rpc' : Principal,
  'schema_version' : number,
  'schnorr_key_name' : string,
  'admins' : Array<Principal>,
  'token_program' : TokenProgram,
  'halted' : [] | [string],
  'icusd_ledger' : Principal,
  'genesis_hash' : string,
  'instance_salt' : string,
  'limits' : BridgeLimits,
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
export interface BridgeToSolanaArgs {
  'client_request_id' : Uint8Array | number[],
  'solana_recipient' : string,
  'from_subaccount' : [] | [Uint8Array | number[]],
  'amount_e8s' : bigint,
  'expected_deposit_fee_e8s' : bigint,
}
export interface BucketsView {
  'reserved_pulls' : bigint,
  'pending_mint' : bigint,
  'unknown_outflow' : bigint,
  'pending_refund' : bigint,
  'outstanding' : bigint,
  'fees' : bigint,
  'held' : bigint,
  'burn_unknown' : bigint,
  'stray' : bigint,
  'return_custodied' : bigint,
  'pending_release' : bigint,
}
export type ClaimStatus = { 'Refunding' : { 'op_id' : bigint } } |
  { 'Refunded' : { 'op_id' : bigint } } |
  { 'Held' : null } |
  { 'Retained' : null };
export interface ClaimView {
  'status' : ClaimStatus,
  'verified_sender' : [] | [string],
  'inner_instruction' : [] | [number],
  'outer_instruction' : number,
  'refund_id' : string,
  'amount_e8s' : bigint,
  'source_account' : string,
}
export type ConsensusSetting = { 'Equality' : null } |
  { 'Threshold' : { 'min' : number, 'total' : [] | [number] } };
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
export interface CustodyReading {
  'status' : CustodyStatusView,
  'at_ns' : bigint,
  'expected' : bigint,
  'custody' : bigint,
  'supply' : bigint,
}
export type CustodyStatusView = { 'Ok' : null } |
  { 'Deficit' : { 'deficit' : bigint } } |
  { 'Reconciling' : { 'deficit' : bigint } } |
  { 'Excess' : { 'excess' : bigint } };
export type DepositKindView = { 'UserDeposit' : null } |
  { 'Remint' : { 'return_id' : string } };
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
  'status' : DepositStatus,
  'client_request_id' : Uint8Array | number[],
  'refund_amount_e8s' : [] | [bigint],
  'owner' : Principal,
  'from' : [] | [Account],
  'kind' : DepositKindView,
  'mint_id' : [] | [string],
  'pull_block_index' : [] | [bigint],
  'mint_amount_e8s' : bigint,
  'updated_at_ns' : bigint,
  'solana_recipient' : string,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'delivered' : [] | [SolanaInclusion],
  'refund_block_index' : [] | [bigint],
  'deposit_fee_e8s' : bigint,
  'mint_op_id' : [] | [bigint],
}
export type DeviceSpec = { 'GenericDisplay' : null } |
  {
    'LineDisplay' : {
      'characters_per_line' : number,
      'lines_per_page' : number,
    }
  };
export interface ErrorInfo { 'description' : string }
export interface EscrowReading {
  'ok' : boolean,
  'balance' : bigint,
  'at_ns' : bigint,
  'required' : bigint,
}
export type HeldOpResolution = { 'Abandon' : { 'reason' : string } } |
  { 'Recheck' : null };
export interface HintView {
  'signature' : string,
  'submitted_at_ns' : bigint,
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
  'rpc' : [] | [RpcSettings],
  'fee_recipient' : [] | [Account],
  'lane' : [] | [LaneSettings],
  'memo_program' : string,
  'sol_rpc' : Principal,
  'schnorr_key_name' : string,
  'admins' : Array<Principal>,
  'token_program' : TokenProgram,
  'icusd_ledger' : Principal,
  'genesis_hash' : string,
}
export type Lane = { 'Burn' : null } |
  { 'Mint' : null };
export interface LaneSettings {
  'rebroadcast_interval_secs' : bigint,
  'discover_limit' : number,
  'cancel_after_secs' : bigint,
  'nonce_pool_size' : number,
  'reconcile_alarm_secs' : bigint,
  'max_reissues' : number,
  'history_horizon_secs' : bigint,
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
export interface ListOpsArgs { 'start_after' : [] | [bigint], 'limit' : number }
export interface ListReleasesArgs {
  'owner' : Principal,
  'start_after_seq' : [] | [bigint],
  'limit' : number,
}
export interface ListReleasesForSenderArgs {
  'solana_sender' : string,
  'start_after_seq' : [] | [bigint],
  'limit' : number,
}
export interface ListReturnsArgs {
  'start_after_seq' : [] | [bigint],
  'limit' : number,
}
export interface ListStraysArgs { 'after' : [] | [string], 'limit' : number }
export type MintState = { 'None' : null } |
  { 'Bound' : null } |
  { 'DeployedUnbound' : null };
export interface MonitorStatus {
  'c1' : [] | [EscrowReading],
  'c2' : [] | [SupplyReading],
  'custody' : [] | [CustodyReading],
  'last_run_ns' : [] | [bigint],
  'custody_strikes' : number,
  'last_note' : [] | [string],
  'c2_strikes' : number,
  'c1_strikes' : number,
}
export interface ObserverStatus {
  'return_account' : [] | [string],
  'strays' : bigint,
  'walk_active' : boolean,
  'cursor' : [] | [string],
  'returns_active' : bigint,
  'last_run_ns' : [] | [bigint],
  'caught_up' : boolean,
  'last_note' : [] | [string],
  'queued' : bigint,
  'processed' : bigint,
  'last_progress_ns' : [] | [bigint],
  'credits_halted' : [] | [string],
}
export type OpKindView = { 'Burn' : null } |
  { 'Mint' : null } |
  { 'Refund' : null };
export type OpStateView = { 'Abandoned' : { 'reason' : string } } |
  { 'Held' : { 'reason' : string } } |
  { 'NeedsReissue' : { 'reason' : string } } |
  { 'Active' : null } |
  {
    'Delivered' : { 'signature' : string, 'slot' : bigint, 'adopted' : boolean }
  };
export interface OpView {
  'id' : bigint,
  'refund_eligibility' : [] | [RefundEligibility],
  'kind' : OpKindView,
  'lane' : Lane,
  'note' : [] | [string],
  'generation' : number,
  'updated_at_ns' : bigint,
  'created_at_ns' : bigint,
  'state' : OpStateView,
  'tx_ids' : Array<string>,
  'slot_index' : [] | [number],
  'external_id' : string,
  'amount' : bigint,
}
export interface Pauses {
  'mints' : boolean,
  'observer' : boolean,
  'releases' : boolean,
  'deposits' : boolean,
}
export type RefundEligibility = { 'ProvenNotDelivered' : null } |
  { 'Unresolved' : null } |
  { 'Unsigned' : null };
export type ReleaseStatus = { 'Reminted' : { 'deposit_id' : bigint } } |
  { 'TransferUnknown' : null } |
  { 'Held' : { 'reason' : string } } |
  { 'Released' : null } |
  { 'Reminting' : { 'deposit_id' : bigint } } |
  { 'Pending' : null };
export interface ReleaseView {
  'seq' : bigint,
  'status' : ReleaseStatus,
  'return_id' : string,
  'destination' : Account,
  'solana_sender' : string,
  'release_block_index' : [] | [bigint],
  'updated_at_ns' : bigint,
  'burn_slot' : bigint,
  'return_slot' : bigint,
  'created_at_ns' : bigint,
  'burn_signature' : string,
  'amount_e8s' : bigint,
  'release_fee_e8s' : bigint,
  'burn_op_id' : bigint,
  'ledger_fee_e8s' : [] | [bigint],
  'return_signature' : string,
  'amount_to_receive_e8s' : [] | [bigint],
}
export type Result = { 'Ok' : null } |
  { 'Err' : BridgeError };
export type Result_1 = { 'Ok' : OpView } |
  { 'Err' : BridgeError };
export type Result_2 = { 'Ok' : DepositView } |
  { 'Err' : BridgeError };
export type Result_3 = { 'Ok' : MonitorStatus } |
  { 'Err' : BridgeError };
export type Result_4 = { 'Ok' : bigint } |
  { 'Err' : BridgeError };
export type Result_5 = { 'Ok' : ConsentInfo } |
  { 'Err' : Icrc21Error };
export type ReturnStatus = { 'Burned' : { 'burn_op_id' : bigint } } |
  { 'Burning' : { 'burn_op_id' : bigint } } |
  { 'Custodied' : null };
export interface ReturnView {
  'seq' : bigint,
  'status' : ReturnStatus,
  'return_id' : string,
  'destination' : Account,
  'signature' : string,
  'slot' : bigint,
  'sender' : string,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'source_account' : string,
  'client_nonce' : bigint,
}
export interface RpcSettings {
  'response_size_estimate' : [] | [bigint],
  'verify' : ConsensusSetting,
  'discover' : ConsensusSetting,
  'cycles_per_call' : bigint,
  'cluster' : [] | [SolRpcCluster],
}
export type SolRpcCluster = { 'Mainnet' : null } |
  { 'Testnet' : null } |
  { 'Devnet' : null };
export interface SolanaInclusion { 'signature' : string, 'slot' : bigint }
export interface StandardRecord { 'url' : string, 'name' : string }
export interface StrayView {
  'claims' : Array<ClaimView>,
  'signature' : string,
  'verified_sender' : [] | [string],
  'slot' : bigint,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'source_account' : [] | [string],
  'reason' : string,
}
export interface SupplyReading {
  'inflight_mint' : bigint,
  'at_ns' : bigint,
  'quiescent' : boolean,
  'within_bound' : boolean,
  'unattributed_gap' : [] | [bigint],
  'supply' : bigint,
  'bound' : bigint,
  'unattributed_gap_blocker' : [] | [string],
}
export type TokenProgram = { 'Classic' : null } |
  { 'Token2022' : null };
export type Transition = { 'RemintDelivered' : { 'amount' : bigint } } |
  { 'BurnSigned' : { 'amount' : bigint } } |
  { 'PullFailed' : { 'amount' : bigint } } |
  { 'BurnFinalized' : { 'amount' : bigint } } |
  { 'StrayObserved' : { 'amount' : bigint } } |
  { 'SweepCompleted' : { 'amount' : bigint } } |
  { 'OutflowResolved' : { 'amount' : bigint } } |
  { 'BurnNotLanded' : { 'amount' : bigint } } |
  { 'RefundStarted' : { 'amount' : bigint } } |
  { 'StrayRefunded' : { 'amount' : bigint } } |
  { 'AdmitDeposit' : { 'amount' : bigint } } |
  { 'RemintStarted' : { 'amount' : bigint } } |
  { 'ReturnCustodied' : { 'amount' : bigint } } |
  { 'ReleaseHeld' : { 'amount' : bigint } } |
  { 'MintDelivered' : { 'fee' : bigint, 'amount' : bigint } } |
  { 'RefundCompleted' : { 'amount' : bigint } } |
  { 'ReleaseCompleted' : { 'amount' : bigint, 'release_fee' : bigint } } |
  { 'PullSucceeded' : { 'amount' : bigint } } |
  { 'OutflowUnknown' : { 'amount' : bigint } };
export interface UpgradeArgs { 'admins' : [] | [Array<Principal>] }
export interface _SERVICE {
  'admin_clear_credits_halt' : ActorMethod<[string], Result>,
  'admin_clear_lane_halt' : ActorMethod<[Lane, string], Result>,
  'admin_resolve_held_op' : ActorMethod<[bigint, HeldOpResolution], Result_1>,
  'admin_retry_deposit' : ActorMethod<[bigint], Result_2>,
  'admin_run_monitors' : ActorMethod<[], Result_3>,
  'admin_run_tick' : ActorMethod<[], Result>,
  'admin_set_fees' : ActorMethod<[BridgeFees], Result>,
  'admin_set_limits' : ActorMethod<[BridgeLimits], Result>,
  'admin_set_pauses' : ActorMethod<[Pauses], Result>,
  'admin_sweep_fees' : ActorMethod<[bigint], Result_4>,
  'bridge_to_solana' : ActorMethod<[BridgeToSolanaArgs], Result_2>,
  'bridge_version' : ActorMethod<[], string>,
  'get_audit_log' : ActorMethod<[bigint, number], Array<AuditEntry>>,
  'get_bootstrap_status' : ActorMethod<[], BootstrapStatus>,
  'get_bridge_info' : ActorMethod<[], BridgeInfo>,
  'get_buckets' : ActorMethod<[], BucketsView>,
  'get_deposit' : ActorMethod<[bigint], [] | [DepositView]>,
  'get_deposit_by_request' : ActorMethod<
    [Principal, Uint8Array | number[]],
    [] | [DepositView]
  >,
  'get_monitor_status' : ActorMethod<[], MonitorStatus>,
  'get_observer_status' : ActorMethod<[], ObserverStatus>,
  'get_release' : ActorMethod<[string], [] | [ReleaseView]>,
  'get_return' : ActorMethod<[string], [] | [ReturnView]>,
  'get_sol_op' : ActorMethod<[bigint], [] | [OpView]>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    Result_5
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'list_deposits_for_owner' : ActorMethod<
    [ListDepositsForOwnerArgs],
    Array<DepositView>
  >,
  'list_my_deposits' : ActorMethod<[ListDepositsArgs], Array<DepositView>>,
  'list_releases_for_owner' : ActorMethod<
    [ListReleasesArgs],
    Array<ReleaseView>
  >,
  'list_releases_for_solana_sender' : ActorMethod<
    [ListReleasesForSenderArgs],
    Array<ReleaseView>
  >,
  'list_return_hints' : ActorMethod<[], Array<HintView>>,
  'list_returns' : ActorMethod<[ListReturnsArgs], Array<ReturnView>>,
  'list_strays' : ActorMethod<[ListStraysArgs], Array<StrayView>>,
  'list_unresolved_ops' : ActorMethod<[ListOpsArgs], Array<OpView>>,
  'notify_solana_return' : ActorMethod<[string], Result>,
  'request_deposit_refund' : ActorMethod<[bigint], Result_2>,
  'set_admins' : ActorMethod<[Array<Principal>], Result>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
