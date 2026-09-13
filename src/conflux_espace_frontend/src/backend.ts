// Anonymous @dfinity/agent actor. Authority is the EVM signature carried in each
// intent, NOT the IC caller, so an anonymous agent is exactly right. Runtime IDL
// and TypeScript wire types come directly from the regenerated declaration;
// keeping a second hand-written Candid projection previously allowed result and
// record variants to drift silently.

import { Actor, HttpAgent } from "@dfinity/agent";
import { idlFactory as generatedIdlFactory } from "../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js";
import type {
  ChainPublicLaunchStatus as GeneratedChainPublicLaunchStatus,
  ChainVaultPage as GeneratedChainVaultPage,
  ChainVaultV1 as GeneratedChainVault,
  ProtocolError as GeneratedProtocolError,
  VaultIntent as GeneratedVaultIntent,
  _SERVICE as GeneratedBackend,
} from "../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js";
import { BACKEND_CANISTER_ID, IC_HOST } from "./config";

export const idlFactory = generatedIdlFactory;
export type ChainPublicLaunchStatus = GeneratedChainPublicLaunchStatus;
export type ChainVaultPage = GeneratedChainVaultPage;
export type ChainVault = GeneratedChainVault;
export type CandidIntent = GeneratedVaultIntent;
export type ProtocolError = GeneratedProtocolError;
export type Backend = Pick<GeneratedBackend,
  | "open_chain_vault_evm"
  | "borrow_chain_vault_evm"
  | "withdraw_chain_collateral_evm"
  | "close_chain_vault_evm"
  | "get_expected_evm_nonce"
  | "get_chain_public_launch_status"
  | "get_chain_vault"
  | "list_chain_vaults_page"
  | "get_protocol_status"
  | "get_collateral_totals"
  | "get_collateral_config"
  | "get_reserve_balances"
>;

let _actor: Backend | null = null;
let _agent: Promise<HttpAgent> | null = null;

export function queryAgent(): Promise<HttpAgent> {
  _agent ??= HttpAgent.create({ host: IC_HOST }).catch((error) => { _agent = null; throw error; });
  return _agent;
}

export async function backend(): Promise<Backend> {
  if (_actor) return _actor;
  const agent = await queryAgent();
  // Mainnet root key is hardcoded into the agent — NEVER fetchRootKey here.
  _actor = Actor.createActor<Backend>(idlFactory, {
    agent,
    canisterId: BACKEND_CANISTER_ID,
  });
  return _actor;
}

/** Pull a human message out of a `Result.Err` variant (EvmAuth/ChainAdmin/…). */
export function errText(err: ProtocolError | Record<string, unknown>): string {
  const [k, v] = Object.entries(err)[0] ?? ["Error", ""];
  return typeof v === "string" && v.length ? `${k}: ${v}` : k;
}

export function statusName(s: ChainVault["status"]): string {
  return Object.keys(s)[0] ?? "Unknown";
}
