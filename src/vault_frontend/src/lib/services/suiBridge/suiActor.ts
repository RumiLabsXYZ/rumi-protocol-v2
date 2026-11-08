/**
 * Glue between the pure Sui flows and the real world: the Sui bridge actors and the destination read.
 *
 * Reads are public queries made with an anonymous actor, so they never open a signer prompt. Only the calls that change
 * state (`bridge_to_sui`, `request_deposit_refund`, `notify_sui_return`, `icrc2_approve`) use the ICP wallet's actor,
 * created BEFORE the click. A Sui wallet is never involved here.
 *
 * While no Sui instance is configured (`CANISTER_IDS.ICUSD_SUI_BRIDGE` is empty, the state of every build today) nothing in
 * this file creates an actor or makes a request: the destination read answers `null` and the page keeps its static,
 * disabled Sui row.
 */

import { Actor } from '@dfinity/agent';
import { CANISTER_IDS } from '$lib/config';
import { idlFactory as suiIdl } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did.js';
import type { _SERVICE as SuiService } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { suiDestination, unreachableSuiDestination, type DestinationView } from '../baseBridge/destinations';
import { getAnonymousAgent } from '../solanaBridge/solanaActor';
import { resolveSuiConfig } from './config';

/** The configured instance, or null (the usual state: no Sui destination is on offer). */
export function getSuiBridgeCanisterId(): string | null {
  const id = (CANISTER_IDS as Record<string, string>).ICUSD_SUI_BRIDGE;
  return typeof id === 'string' && id.trim() !== '' ? id : null;
}

export async function getAnonymousSuiBridge(): Promise<SuiService> {
  const id = getSuiBridgeCanisterId();
  if (!id) throw new Error('No Sui bridge instance is configured.');
  return Actor.createActor<SuiService>(suiIdl, { agent: await getAnonymousAgent(), canisterId: id });
}

/**
 * The Sui destination read by `loadDestinations`: null while no instance is configured (no request is made); else the entry
 * built from the canister's own `get_config`, `get_roles`, `get_mint_lane` and `get_observed_deployment` (enabled only if
 * every check passes). A failed read makes only this row unavailable: it never rejects.
 */
export async function loadSuiDestination(): Promise<DestinationView | null> {
  const id = getSuiBridgeCanisterId();
  if (!id) return null;
  try {
    const bridge = await getAnonymousSuiBridge();
    const [config, roles, lane, observed] = await Promise.all([bridge.get_config(), bridge.get_roles(), bridge.get_mint_lane(), bridge.get_observed_deployment()]);
    return suiDestination(resolveSuiConfig(config, roles, lane, observed[0] ?? null, id), id);
  } catch (error) {
    return unreachableSuiDestination(error instanceof Error ? error.message : String(error), id);
  }
}
