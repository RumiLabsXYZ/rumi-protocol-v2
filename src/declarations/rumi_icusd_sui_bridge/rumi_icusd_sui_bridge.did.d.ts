import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export type AttemptStatusView = { 'LandedFinal' : { 'checkpoint' : bigint } } |
  { 'Open' : null } |
  { 'NeverSigned' : { 'reason' : string } } |
  { 'FailedFinal' : { 'checkpoint' : bigint, 'error' : string } };
export interface AuditEntry {
  'seq' : bigint,
  'return_id' : [] | [string],
  'deposit_id' : [] | [bigint],
  'at_ns' : bigint,
  'op_id' : [] | [bigint],
  'note' : string,
  'transition' : [] | [Transition],
}
export interface AuditReport {
  'pending_and_held' : PendingHeldView,
  'self_reported' : SelfReportedView,
  'generated_at_ns' : bigint,
  'disclosures' : Array<string>,
  'independent' : IndependentObservationsView,
  'checks' : Array<CheckView>,
  'last_checked' : LastCheckedView,
  'books_at_last_check' : [] | [BucketsView],
}
export type BreachScopeView = { 'MintLane' : null } |
  { 'Bridge' : null };
export type BridgeArg = { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs };
export interface BridgeFees {
  'release_fee_e8s' : bigint,
  'deposit_fee_e8s' : bigint,
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
export interface BridgeToSuiArgs {
  'client_request_id' : Uint8Array | number[],
  'from_subaccount' : [] | [Uint8Array | number[]],
  'amount_e8s' : bigint,
  'expected_deposit_fee_e8s' : bigint,
  'sui_recipient' : string,
}
export interface BucketsView {
  'reserved_pulls' : bigint,
  'pending_mint' : bigint,
  'unknown_outflow' : bigint,
  'pending_refund' : bigint,
  'outstanding' : bigint,
  'fees' : bigint,
  'held' : bigint,
  'pending_release' : bigint,
}
export type CandidateSourceView = { 'Event' : null } |
  { 'Hint' : null };
export interface CandidateView {
  'verified' : boolean,
  'tries' : number,
  'sui_sender' : string,
  'source' : CandidateSourceView,
  'claimed_amount_e8s' : bigint,
  'claimed_sequence' : bigint,
  'tx_digest' : string,
  'waiting' : [] | [string],
  'client_nonce' : bigint,
}
export type CheckOutcomeView = { 'Ok' : null } |
  { 'Breach' : { 'detail' : string, 'scope' : BreachScopeView } } |
  { 'NotEvaluated' : { 'reason' : string } };
export interface CheckView { 'name' : string, 'outcome' : CheckOutcomeView }
export interface ConfigView {
  'signing_enabled' : boolean,
  'bridge_version' : string,
  'initialized' : boolean,
  'fee_recipient' : [] | [Account],
  'fees' : BridgeFees,
  'mirror' : MirrorSettings,
  'network' : SuiNetwork,
  'escrow_subaccount' : string,
  'schema_version' : number,
  'binding' : [] | [GateBindingView],
  'schnorr_key_name' : string,
  'short_chain_id' : string,
  'admins' : Array<Principal>,
  'halted' : [] | [string],
  'chain_identifier' : string,
  'icusd_ledger' : Principal,
  'instance_salt' : string,
  'limits' : BridgeLimits,
  'pauses' : Pauses,
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
export type DeliveryViaView = { 'ConsumedRecord' : null } |
  { 'Receipt' : null };
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
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'delivered' : [] | [SuiInclusion],
  'refund_block_index' : [] | [bigint],
  'sui_recipient' : string,
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
export interface GateBindingArg {
  'admin_cap_id' : string,
  'mint_cap_id' : string,
  'gate_id' : string,
  'package_id' : string,
}
export interface GateBindingView {
  'decimals' : number,
  'admin_cap_id' : string,
  'coin_type' : string,
  'mint_cap_id' : string,
  'gate_id' : string,
  'gate_type' : string,
  'package_id' : string,
}
export interface GateObservationView {
  'bridge_out_count' : bigint,
  'mint_count' : bigint,
  'supply_cap' : bigint,
  'burned_total' : bigint,
  'gate_version' : bigint,
  'total_supply' : bigint,
  'minted_total' : bigint,
}
export interface HttpHeader { 'value' : string, 'name' : string }
export interface HttpResponse {
  'status' : bigint,
  'body' : Uint8Array | number[],
  'headers' : Array<HttpHeader>,
}
export type Icrc21Error = {
    'GenericError' : { 'description' : string, 'error_code' : bigint }
  } |
  { 'UnsupportedCanisterCall' : ErrorInfo } |
  { 'ConsentMessageUnavailable' : ErrorInfo };
export interface Icrc28TrustedOriginsResponse {
  'trusted_origins' : Array<string>,
}
export interface IndependentObservationsView {
  'deployment' : [] | [ObservedDeploymentView],
  'checkpoint' : [] | [bigint],
  'gate' : [] | [GateObservationView],
  'observed_at_ns' : [] | [bigint],
  'escrow_balance_e8s' : [] | [bigint],
  'epoch' : [] | [bigint],
  'checkpoint_digest' : [] | [string],
}
export interface InitArgs {
  'fee_recipient' : [] | [Account],
  'fees' : [] | [BridgeFees],
  'mirror' : [] | [MirrorSettings],
  'network' : SuiNetwork,
  'admins' : Array<Principal>,
  'chain_identifier' : string,
  'icusd_ledger' : Principal,
  'limits' : [] | [BridgeLimits],
}
export interface LaneSettingsArg {
  'gas_budget' : bigint,
  'max_attempts' : number,
}
export interface LastCheckedView {
  'at_ns' : [] | [bigint],
  'checkpoint' : [] | [bigint],
  'age_ns' : [] | [bigint],
  'stale' : boolean,
  'caught_up' : boolean,
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
  'owner' : Principal,
  'start_after_seq' : [] | [bigint],
  'limit' : number,
}
export interface ListReleasesForSenderArgs {
  'sui_sender' : string,
  'start_after_seq' : [] | [bigint],
  'limit' : number,
}
export interface MintAttemptView {
  'status' : AttemptStatusView,
  'gas_signed' : boolean,
  'gas_sig_unknown' : boolean,
  'gas_budget' : bigint,
  'mint_cap_version' : bigint,
  'generation' : number,
  'epoch' : bigint,
  'gas_coin_version' : bigint,
  'sends' : number,
  'treasury_signed' : boolean,
  'tx_digest' : string,
  'signing_digest' : string,
  'treasury_sig_unknown' : boolean,
  'gas_price' : bigint,
}
export interface MintLaneView {
  'active_op' : [] | [bigint],
  'observed' : [] | [ObservedDeploymentView],
  'gas_budget' : bigint,
  'last_final_checkpoint' : bigint,
  'max_attempts' : number,
  'blocked_by' : Array<string>,
  'gas_coin' : [] | [string],
  'last_wait' : [] | [string],
  'lane_halt' : [] | [string],
  'ready' : boolean,
}
export type MintOpStatusView = { 'Failed' : { 'reason' : string } } |
  { 'Active' : null } |
  {
    'Delivered' : {
      'via' : DeliveryViaView,
      'checkpoint' : bigint,
      'tx_digest' : [] | [string],
      'sequence' : bigint,
    }
  };
export interface MintOpView {
  'id' : bigint,
  'status' : MintOpStatusView,
  'deposit_id' : bigint,
  'mint_id' : string,
  'attempts' : Array<MintAttemptView>,
  'updated_at_ns' : bigint,
  'hold_reason' : [] | [string],
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'sui_recipient' : string,
}
export interface MirrorSettings {
  'max_response_bytes' : number,
  'min_quorum' : number,
  'enabled' : boolean,
  'min_independent_providers' : number,
  'providers' : Array<ProviderConfig>,
}
export interface ObservedDeploymentView {
  'bridge_out_count' : bigint,
  'decimals' : number,
  'mint_count' : bigint,
  'supply_cap' : bigint,
  'mint_cap_owner' : string,
  'initial_shared_version' : bigint,
  'burned_total' : bigint,
  'checkpoint' : bigint,
  'mint_cap_version' : bigint,
  'observed_at_ns' : bigint,
  'gate_version' : bigint,
  'admin_cap_id' : string,
  'min_bridge_out' : bigint,
  'admin_cap_role' : [] | [SuiRole],
  'epoch' : bigint,
  'mint_cap_id' : string,
  'admin_cap_owner' : string,
  'module_digest_observed' : boolean,
  'total_supply' : bigint,
  'minted_total' : bigint,
  'symbol' : string,
  'checkpoint_digest' : string,
}
export interface ObserverProblemView {
  'at_ns' : bigint,
  'note' : string,
  'tx_digest' : [] | [string],
}
export interface ObserverView {
  'gap' : [] | [string],
  'scanned_through' : bigint,
  'started' : boolean,
  'returns_verified_through' : bigint,
  'caught_up' : boolean,
  'problems' : Array<ObserverProblemView>,
  'next_sequence' : bigint,
  'last_head' : bigint,
  'next_run_ns' : bigint,
  'last_wait' : [] | [string],
  'candidates' : Array<CandidateView>,
}
export interface Pauses {
  'mints' : boolean,
  'observer' : boolean,
  'releases' : boolean,
  'deposits' : boolean,
}
export interface PendingHeldView {
  'pending_refund_e8s' : bigint,
  'reasons' : Array<string>,
  'reserved_pulls_e8s' : bigint,
  'pending_mint_e8s' : bigint,
  'active_releases' : bigint,
  'held_releases' : bigint,
  'active_deposits' : bigint,
  'held_e8s' : bigint,
  'pending_release_e8s' : bigint,
  'held_deposits' : bigint,
  'active_mint_op' : [] | [bigint],
  'unknown_outflow_e8s' : bigint,
}
export interface ProviderConfig { 'id' : string, 'url' : string }
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
  'sui_sender' : string,
  'checkpoint' : bigint,
  'release_block_index' : [] | [bigint],
  'updated_at_ns' : bigint,
  'gate_sequence' : bigint,
  'created_at_ns' : bigint,
  'amount_e8s' : bigint,
  'release_fee_e8s' : bigint,
  'tx_digest' : string,
  'ledger_fee_e8s' : [] | [bigint],
  'amount_to_receive_e8s' : [] | [bigint],
  'client_nonce' : bigint,
}
export type Result = { 'Ok' : null } |
  { 'Err' : SuiBridgeError };
export type Result_1 = { 'Ok' : DepositView } |
  { 'Err' : SuiBridgeError };
export type Result_2 = { 'Ok' : bigint } |
  { 'Err' : SuiBridgeError };
export type Result_3 = { 'Ok' : Array<RoleView> } |
  { 'Err' : SuiBridgeError };
export type Result_4 = { 'Ok' : ConsentInfo } |
  { 'Err' : Icrc21Error };
export type Result_5 = { 'Ok' : ObservedDeploymentView } |
  { 'Err' : SuiBridgeError };
export type Result_6 = { 'Ok' : ConfigView } |
  { 'Err' : SuiBridgeError };
export interface RoleView {
  'public_key' : [] | [string],
  'role' : SuiRole,
  'derivation_path' : Array<string>,
  'address' : [] | [string],
}
export interface SelfReportedView {
  'minted_e8s' : bigint,
  'inflight_mint_e8s' : bigint,
  'inflight_mint_ops' : bigint,
  'minted_count' : bigint,
  'returned_e8s' : bigint,
  'returns_verified_through' : bigint,
  'buckets' : BucketsView,
}
export interface StandardRecord { 'url' : string, 'name' : string }
export type SuiBridgeError = { 'Internal' : string } |
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
  { 'DeploymentMismatch' : string } |
  { 'SignerFailed' : string } |
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
  { 'Unavailable' : string } |
  { 'Halted' : string } |
  { 'RequestIdConflict' : null } |
  { 'InvalidState' : string } |
  { 'FeeChanged' : { 'current_fee_e8s' : bigint } } |
  {
    'InsufficientFunds' : { 'required_e8s' : bigint, 'balance_e8s' : bigint }
  } |
  { 'Conflict' : string } |
  { 'AmountAboveMaximum' : { 'max_e8s' : bigint } };
export interface SuiInclusion {
  'checkpoint' : bigint,
  'tx_digest' : [] | [string],
  'sequence' : bigint,
}
export type SuiNetwork = { 'Mainnet' : null } |
  { 'Testnet' : null };
export type SuiRole = { 'UpgradeCustody' : null } |
  { 'Gas' : null } |
  { 'Treasury' : null };
export interface TransformArgs {
  'context' : Uint8Array | number[],
  'response' : HttpResponse,
}
export type Transition = { 'RemintDelivered' : { 'amount' : bigint } } |
  { 'PullFailed' : { 'amount' : bigint } } |
  { 'SweepCompleted' : { 'amount' : bigint } } |
  { 'ReturnFinalized' : { 'amount' : bigint } } |
  { 'OutflowResolved' : { 'amount' : bigint } } |
  { 'RefundStarted' : { 'amount' : bigint } } |
  { 'AdmitDeposit' : { 'amount' : bigint } } |
  { 'RemintStarted' : { 'amount' : bigint } } |
  { 'ReleaseHeld' : { 'amount' : bigint } } |
  { 'MintDelivered' : { 'fee' : bigint, 'amount' : bigint } } |
  { 'RefundCompleted' : { 'amount' : bigint } } |
  { 'ReleaseCompleted' : { 'amount' : bigint, 'release_fee' : bigint } } |
  { 'PullSucceeded' : { 'amount' : bigint } } |
  { 'OutflowUnknown' : { 'amount' : bigint } };
export interface UpgradeArgs {
  'mirror' : [] | [MirrorSettings],
  'admins' : [] | [Array<Principal>],
}
export interface _SERVICE {
  'admin_abandon_mint_op' : ActorMethod<[bigint, string], Result>,
  'admin_clear_halt' : ActorMethod<[string], Result>,
  'admin_clear_mint_lane_halt' : ActorMethod<[string], Result>,
  'admin_retry_deposit' : ActorMethod<[bigint], Result_1>,
  'admin_run_tick' : ActorMethod<[], Result>,
  'admin_set_fees' : ActorMethod<[BridgeFees], Result>,
  'admin_set_lane_settings' : ActorMethod<[LaneSettingsArg], Result>,
  'admin_set_limits' : ActorMethod<[BridgeLimits], Result>,
  'admin_set_pauses' : ActorMethod<[Pauses], Result>,
  'admin_sweep_fees' : ActorMethod<[bigint], Result_2>,
  'bridge_to_sui' : ActorMethod<[BridgeToSuiArgs], Result_1>,
  'bridge_version' : ActorMethod<[], string>,
  'derive_role_keys' : ActorMethod<[], Result_3>,
  'get_audit_log' : ActorMethod<[bigint, number], Array<AuditEntry>>,
  'get_audit_report' : ActorMethod<[], AuditReport>,
  'get_buckets' : ActorMethod<[], BucketsView>,
  'get_config' : ActorMethod<[], ConfigView>,
  'get_deposit' : ActorMethod<[bigint], [] | [DepositView]>,
  'get_deposit_by_request' : ActorMethod<
    [Principal, Uint8Array | number[]],
    [] | [DepositView]
  >,
  'get_mint_lane' : ActorMethod<[], MintLaneView>,
  'get_mint_op' : ActorMethod<[bigint], [] | [MintOpView]>,
  'get_observed_deployment' : ActorMethod<[], [] | [ObservedDeploymentView]>,
  'get_observer' : ActorMethod<[], ObserverView>,
  'get_release' : ActorMethod<[string], [] | [ReleaseView]>,
  'get_roles' : ActorMethod<[], Array<RoleView>>,
  'icrc10_supported_standards' : ActorMethod<[], Array<StandardRecord>>,
  'icrc21_canister_call_consent_message' : ActorMethod<
    [ConsentMessageRequest],
    Result_4
  >,
  'icrc28_trusted_origins' : ActorMethod<[], Icrc28TrustedOriginsResponse>,
  'list_deposits_for_owner' : ActorMethod<
    [ListDepositsForOwnerArgs],
    Array<DepositView>
  >,
  'list_mint_ops' : ActorMethod<[ListDepositsArgs], Array<MintOpView>>,
  'list_my_deposits' : ActorMethod<[ListDepositsArgs], Array<DepositView>>,
  'list_releases_for_owner' : ActorMethod<
    [ListReleasesArgs],
    Array<ReleaseView>
  >,
  'list_releases_for_sui_sender' : ActorMethod<
    [ListReleasesForSenderArgs],
    Array<ReleaseView>
  >,
  'notify_sui_return' : ActorMethod<[string], Result>,
  'observe_deployment' : ActorMethod<[], Result_5>,
  'request_deposit_refund' : ActorMethod<[bigint], Result_1>,
  'set_admins' : ActorMethod<[Array<Principal>], Result>,
  'set_gas_coin' : ActorMethod<[string], Result>,
  'set_gate_binding' : ActorMethod<[GateBindingArg], Result_6>,
  'transform_sui_graphql' : ActorMethod<[TransformArgs], HttpResponse>,
  'transform_sui_graphql_head' : ActorMethod<[TransformArgs], HttpResponse>,
  'transform_sui_send' : ActorMethod<[TransformArgs], HttpResponse>,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
