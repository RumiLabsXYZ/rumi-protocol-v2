import type { Principal } from '@dfinity/principal';
import type { ActorMethod } from '@dfinity/agent';
import type { IDL } from '@dfinity/candid';

export interface AcceptedWithdrawalRequest {
  'burn_block_index' : LedgerBurnIndex,
  'burned_amount' : Lamport,
  'account' : Account,
  'solana_address' : Address,
  'amount_to_transfer' : Lamport,
}
export interface Account {
  'owner' : Principal,
  'subaccount' : [] | [Uint8Array | number[]],
}
export type Address = string;
export interface CreditedDeposit {
  'deposit_id' : DepositSolId,
  'amount_to_mint' : Lamport,
}
export interface DepositSolArgs {
  'owner' : [] | [Principal],
  'subaccount' : [] | [Uint8Array | number[]],
}
export type DepositSolError = {
    'ValueTooSmall' : {
      'balance' : Lamport,
      'minimum_deposit_amount' : Lamport,
    }
  } |
  { 'TemporarilyUnavailable' : string } |
  { 'AlreadyProcessing' : null } |
  { 'InsufficientCycles' : InsufficientCyclesError } |
  { 'Quarantined' : { 'deposit_id' : DepositSolId } };
export type DepositSolId = bigint;
export type DepositSolResult = { 'Ok' : DepositSolId } |
  { 'Err' : DepositSolError };
export type DepositSolStatus = { 'Queued' : { 'sweepable_amount' : Lamport } } |
  { 'Swept' : { 'signature' : Signature } } |
  { 'Finalized' : { 'signature' : Signature } } |
  { 'NotFound' : null } |
  {
    'Minted' : { 'minted_amount' : Lamport, 'block_index' : LedgerMintIndex }
  } |
  { 'Dropped' : { 'signature' : Signature } } |
  { 'Quarantined' : { 'signature' : Signature } };
export type Ed25519KeyName = { 'MainnetTestKey1' : null } |
  { 'LocalDevelopment' : null } |
  { 'MainnetProdKey1' : null };
export interface Event { 'timestamp' : bigint, 'payload' : EventType }
export type EventType = {
    'SucceededTransaction' : { 'signature' : Signature }
  } |
  {
    'SubmittedTransaction' : {
      'signature' : Signature,
      'signers' : Array<Signer>,
      'transaction' : VersionedTransactionMessage,
      'purpose' : TransactionPurpose,
    }
  } |
  { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs } |
  {
    'MinterPublicKeyFetched' : {
      'public_key' : Uint8Array | number[],
      'chain_code' : Uint8Array | number[],
    }
  } |
  { 'AcceptedWithdrawalRequest' : AcceptedWithdrawalRequest } |
  {
    'CreditedSweep' : {
      'mints' : Array<CreditedDeposit>,
      'signature' : Signature,
      'amount_received' : Lamport,
    }
  } |
  {
    'CreatedWithdrawalTransaction' : {
      'nonce_value' : Hash,
      'nonce_account' : Address,
      'burn_indices' : BigUint64Array | bigint[],
    }
  } |
  { 'QuarantinedPendingMint' : { 'deposit_id' : DepositSolId } } |
  {
    'QueuedDeposit' : {
      'deposit_id' : DepositSolId,
      'balance' : Lamport,
      'address' : Address,
      'account' : Account,
    }
  } |
  { 'ExpiredTransaction' : { 'signature' : Signature } } |
  {
    'MintedSweptDeposit' : {
      'deposit_id' : DepositSolId,
      'mint_block_index' : LedgerMintIndex,
    }
  } |
  { 'FailedTransaction' : { 'signature' : Signature } } |
  { 'QuarantinedSweep' : { 'signature' : Signature } };
export interface GetDepositAddressArgs {
  'owner' : [] | [Principal],
  'subaccount' : [] | [Uint8Array | number[]],
}
export interface GetEventsArgs { 'start' : bigint, 'length' : bigint }
export interface GetEventsResult {
  'total_event_count' : bigint,
  'events' : Array<Event>,
}
export type Hash = string;
export interface InitArgs {
  'deposit_sol_fee' : bigint,
  'solana_network' : SolanaNetwork,
  'withdrawal_fee' : Lamport,
  'minimum_deposit_amount' : Lamport,
  'sol_rpc_canister_id' : Principal,
  'deposit_sol_required_cycles' : bigint,
  'nonce_accounts' : Array<string>,
  'minimum_withdrawal_amount' : Lamport,
  'ledger_canister_id' : Principal,
  'master_key_name' : Ed25519KeyName,
}
export interface InsufficientCyclesError {
  'expected' : bigint,
  'received' : bigint,
}
export type Lamport = bigint;
export type LedgerBurnIndex = bigint;
export type LedgerMintIndex = bigint;
export type MinterArg = { 'Upgrade' : UpgradeArgs } |
  { 'Init' : InitArgs };
export interface MinterInfo {
  'deposit_sol_fee' : bigint,
  'balance' : Lamport,
  'withdrawal_fee' : Lamport,
  'minimum_deposit_amount' : Lamport,
  'deposit_sol_required_cycles' : bigint,
  'nonce_accounts' : Array<string>,
  'minimum_withdrawal_amount' : Lamport,
  'minter_address' : [] | [string],
}
export type Signature = string;
export type Signer = { 'Account' : Account } |
  { 'Minter' : null };
export type SolanaNetwork = { 'Mainnet' : null } |
  { 'Testnet' : null } |
  { 'Devnet' : null };
export type TransactionPurpose = {
    'SweepDeposit' : {
      'block_height' : bigint,
      'deposit_ids' : BigUint64Array | bigint[],
    }
  } |
  { 'Withdrawal' : { 'burn_indices' : BigUint64Array | bigint[] } };
export type TxFinalizedStatus = {
    'Success' : { 'transaction_id' : Signature }
  } |
  { 'Failure' : { 'transaction_id' : Signature } };
export interface UpgradeArgs {
  'deposit_sol_fee' : [] | [bigint],
  'withdrawal_fee' : [] | [Lamport],
  'minimum_deposit_amount' : [] | [Lamport],
  'nonce_accounts_to_add' : [] | [Array<string>],
  'sol_rpc_canister_id' : [] | [Principal],
  'deposit_sol_required_cycles' : [] | [bigint],
  'minimum_withdrawal_amount' : [] | [Lamport],
}
export type VersionedTransactionMessage = { 'Legacy' : Uint8Array | number[] };
export interface WithdrawSolArgs {
  'from_subaccount' : [] | [Uint8Array | number[]],
  'address' : Address,
  'amount' : Lamport,
}
export type WithdrawSolError = { 'MalformedAddress' : Address } |
  {
    'ValueTooSmall' : {
      'withdrawal_amount' : Lamport,
      'minimum_withdrawal_amount' : Lamport,
    }
  } |
  { 'TemporarilyUnavailable' : string } |
  { 'InvalidDestination' : string } |
  { 'InsufficientAllowance' : { 'allowance' : Lamport } } |
  { 'AlreadyProcessing' : null } |
  { 'InsufficientFunds' : { 'balance' : Lamport } };
export interface WithdrawSolOk { 'block_index' : LedgerBurnIndex }
export type WithdrawSolResult = { 'Ok' : WithdrawSolOk } |
  { 'Err' : WithdrawSolError };
export type WithdrawSolStatus = { 'NotFound' : null } |
  { 'TxFinalized' : TxFinalizedStatus } |
  { 'TxSent' : { 'transaction_id' : Signature } } |
  { 'Pending' : null };
export interface WithdrawSolStatusArgs { 'block_index' : LedgerBurnIndex }
export interface _SERVICE {
  'deposit_sol' : ActorMethod<[DepositSolArgs], DepositSolResult>,
  'deposit_sol_status' : ActorMethod<[DepositSolId], DepositSolStatus>,
  'get_deposit_address' : ActorMethod<[GetDepositAddressArgs], Address>,
  'get_events' : ActorMethod<[GetEventsArgs], GetEventsResult>,
  'get_minter_info' : ActorMethod<[], MinterInfo>,
  'withdraw_sol' : ActorMethod<[WithdrawSolArgs], WithdrawSolResult>,
  'withdraw_sol_status' : ActorMethod<
    [WithdrawSolStatusArgs],
    WithdrawSolStatus
  >,
}
export declare const idlFactory: IDL.InterfaceFactory;
export declare const init: (args: { IDL: typeof IDL }) => IDL.Type[];
