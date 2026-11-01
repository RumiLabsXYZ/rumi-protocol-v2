/**
 * Glue between the pure Solana flows and the real world: the Solana bridge actors and the destination read.
 *
 * Reads are public queries made with an anonymous actor, so they never open a signer prompt. Only the calls that
 * change state (`bridge_to_solana`, `request_deposit_refund`, `notify_solana_return`, `icrc2_approve`) use the
 * ICP wallet's actor, created BEFORE the click. A Solana wallet is never involved here.
 *
 * While no Solana instance is configured (`CANISTER_IDS.ICUSD_SOL_BRIDGE` is empty, the state of every build today)
 * nothing in this file creates an actor or makes a request: the destination read answers `null` and the page keeps its
 * static, disabled Solana row.
 */

import { Actor, AnonymousIdentity, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { idlFactory as solIdl } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did.js';
import type { _SERVICE as SolService } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { solanaDestination, unreachableSolanaDestination, type DestinationView } from '../baseBridge/destinations';
import { resolveSolanaConfig } from './config';

/** The configured instance, or null (the usual state: no Solana destination is on offer). */
export function getSolanaBridgeCanisterId(): string | null {
  const id = (CANISTER_IDS as Record<string, string>).ICUSD_SOL_BRIDGE;
  return typeof id === 'string' && id.trim() !== '' ? id : null;
}

let anonymousAgent: HttpAgent | null = null;
export async function getAnonymousAgent(): Promise<HttpAgent> {
  if (!anonymousAgent) {
    anonymousAgent = new HttpAgent({ host: CONFIG.host, identity: new AnonymousIdentity() });
    if (CONFIG.isLocal) await anonymousAgent.fetchRootKey();
  }
  return anonymousAgent;
}

export async function getAnonymousSolanaBridge(): Promise<SolService> {
  const id = getSolanaBridgeCanisterId();
  if (!id) throw new Error('No Solana bridge instance is configured.');
  return Actor.createActor<SolService>(solIdl, { agent: await getAnonymousAgent(), canisterId: id });
}

/**
 * The Solana destination read by `loadDestinations`: null while no instance is configured (no request is made); else the
 * entry built from the canister's own `get_bridge_info` and `get_bootstrap_status` (enabled only if every check passes).
 */
export async function loadSolanaDestination(): Promise<DestinationView | null> {
  const id = getSolanaBridgeCanisterId();
  if (!id) return null;
  try {
    const bridge = await getAnonymousSolanaBridge();
    const [info, bootstrap] = await Promise.all([bridge.get_bridge_info(), bridge.get_bootstrap_status()]);
    return solanaDestination(resolveSolanaConfig(info, bootstrap, id), id);
  } catch (error) {
    return unreachableSolanaDestination(error instanceof Error ? error.message : String(error), id);
  }
}
