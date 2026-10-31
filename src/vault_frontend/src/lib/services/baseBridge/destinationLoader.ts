/**
 * Reads the destination list from the bridge canisters.
 *
 * Kept apart from `destinations.ts` so the pure destination logic (and the specs of
 * the flows that only need its types) never pulls in an actor or a wallet store.
 */

import type { BridgeConfigResult } from './chains';
import { getBridgeCanisterId, loadBridgeConfig } from './bridgeActor';
import { buildDestinations, withSolanaDestination, type DestinationView } from './destinations';
import { loadSolanaDestination } from '../solanaBridge/solanaActor';

/**
 * Read the destination list: the Base entry from the canister's `get_bridge_info`, resolved by the
 * same `resolveBridgeConfig` the page has always used, then the static disabled entries. A Solana entry
 * replaces the static Solana row only when a configured Solana instance answers (none is configured today, so
 * no request is made and the row stays disabled).
 *
 * Rejects when the Base canister cannot be read, exactly as `loadBridgeConfig` does, so a caller that keeps
 * its last good list through a failed refresh behaves as the page always has. A failure of the Solana read never
 * rejects: it makes only the Solana row unavailable. `readBase` and `readSolana` are test seams.
 */
export async function loadDestinations(
  readBase: () => Promise<BridgeConfigResult> = loadBridgeConfig,
  readSolana: () => Promise<DestinationView | null> = loadSolanaDestination,
): Promise<DestinationView[]> {
  const list = buildDestinations(await readBase(), getBridgeCanisterId());
  const solana = await readSolana().catch(() => null);
  return solana ? withSolanaDestination(list, solana) : list;
}
