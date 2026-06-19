// Anonymous @dfinity/agent actor for the Rumi backend on mainnet-staging (kvg63).
// Authority is the EVM signature carried in each intent, NOT the IC caller, so an
// anonymous agent is exactly right. A minimal hand-rolled IDL (the tracked
// declarations predate the _evm methods) — covers only what the UI calls.

import { Actor, HttpAgent } from "@dfinity/agent";
import { IDL } from "@dfinity/candid";
import type { Principal } from "@dfinity/principal";
import { BACKEND_CANISTER_ID, IC_HOST } from "./config";

export const idlFactory: IDL.InterfaceFactory = ({ IDL }) => {
  const VaultIntent = IDL.Record({
    action: IDL.Nat8,
    chain_id: IDL.Nat64,
    owner: IDL.Text,
    vault_id: IDL.Nat64,
    collateral_wei: IDL.Nat,
    debt_e8s: IDL.Nat,
    recipient: IDL.Text,
    nonce: IDL.Nat64,
    deadline_secs: IDL.Nat64,
  });
  // The _evm methods only ever return Ok | EvmAuth; ChainAdmin/GenericError
  // included defensively so an unexpected variant still decodes with a message.
  const ProtocolError = IDL.Variant({
    EvmAuth: IDL.Text,
    ChainAdmin: IDL.Text,
    GenericError: IDL.Text,
    TemporarilyUnavailable: IDL.Text,
    AnonymousCallerNotAllowed: IDL.Null,
  });
  const ChainVaultStatus = IDL.Variant({
    AwaitingDeposit: IDL.Null,
    MintPending: IDL.Null,
    Open: IDL.Null,
    Closing: IDL.Null,
    Closed: IDL.Null,
  });
  const ChainVaultV1 = IDL.Record({
    vault_id: IDL.Nat64,
    owner: IDL.Principal,
    collateral_chain: IDL.Nat32,
    custody_address: IDL.Text,
    collateral_amount_e18: IDL.Nat,
    debt_e8s: IDL.Nat,
    mint_recipient: IDL.Text,
    pending_mint_e8s: IDL.Nat,
    status: ChainVaultStatus,
    opened_at_ns: IDL.Nat64,
    owner_evm: IDL.Opt(IDL.Text),
    last_interest_accrual_ns: IDL.Nat64,
    pending_interest_mint_e8s: IDL.Nat,
  });
  const ResNat64 = IDL.Variant({ Ok: IDL.Nat64, Err: ProtocolError });
  const ResUnit = IDL.Variant({ Ok: IDL.Null, Err: ProtocolError });
  const blob = IDL.Vec(IDL.Nat8);
  return IDL.Service({
    open_chain_vault_evm: IDL.Func([VaultIntent, blob], [ResNat64], []),
    borrow_chain_vault_evm: IDL.Func([VaultIntent, blob], [ResUnit], []),
    withdraw_chain_collateral_evm: IDL.Func([VaultIntent, blob], [ResUnit], []),
    close_chain_vault_evm: IDL.Func([VaultIntent, blob], [ResUnit], []),
    get_chain_vault: IDL.Func([IDL.Nat64], [IDL.Opt(ChainVaultV1)], ["query"]),
    list_chain_vaults: IDL.Func([IDL.Nat32], [IDL.Vec(ChainVaultV1)], ["query"]),
  });
};

export type CandidIntent = {
  action: number;
  chain_id: bigint;
  owner: string;
  vault_id: bigint;
  collateral_wei: bigint;
  debt_e8s: bigint;
  recipient: string;
  nonce: bigint;
  deadline_secs: bigint;
};

export type ChainVault = {
  vault_id: bigint;
  owner: Principal;
  collateral_chain: number;
  custody_address: string;
  collateral_amount_e18: bigint;
  debt_e8s: bigint;
  mint_recipient: string;
  pending_mint_e8s: bigint;
  status:
    | { AwaitingDeposit: null }
    | { MintPending: null }
    | { Open: null }
    | { Closing: null }
    | { Closed: null };
  opened_at_ns: bigint;
  owner_evm: [] | [string];
  last_interest_accrual_ns: bigint;
  pending_interest_mint_e8s: bigint;
};

type Res<T> = { Ok: T } | { Err: Record<string, unknown> };

export interface Backend {
  open_chain_vault_evm(i: CandidIntent, sig: Uint8Array): Promise<Res<bigint>>;
  borrow_chain_vault_evm(i: CandidIntent, sig: Uint8Array): Promise<Res<null>>;
  withdraw_chain_collateral_evm(i: CandidIntent, sig: Uint8Array): Promise<Res<null>>;
  close_chain_vault_evm(i: CandidIntent, sig: Uint8Array): Promise<Res<null>>;
  get_chain_vault(vaultId: bigint): Promise<[] | [ChainVault]>;
  list_chain_vaults(chainId: number): Promise<ChainVault[]>;
}

let _actor: Backend | null = null;

export async function backend(): Promise<Backend> {
  if (_actor) return _actor;
  const agent = await HttpAgent.create({ host: IC_HOST });
  // Mainnet root key is hardcoded into the agent — NEVER fetchRootKey here.
  _actor = Actor.createActor<Backend>(idlFactory, {
    agent,
    canisterId: BACKEND_CANISTER_ID,
  });
  return _actor;
}

/** Pull a human message out of a `Result.Err` variant (EvmAuth/ChainAdmin/…). */
export function errText(err: Record<string, unknown>): string {
  const [k, v] = Object.entries(err)[0] ?? ["Error", ""];
  return typeof v === "string" && v.length ? `${k}: ${v}` : k;
}

export function statusName(s: ChainVault["status"]): string {
  return Object.keys(s)[0] ?? "Unknown";
}
