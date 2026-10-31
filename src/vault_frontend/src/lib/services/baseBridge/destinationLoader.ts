/**
 * Reads the destination list from the bridge canister.
 *
 * Kept apart from `destinations.ts` so the pure destination logic (and the specs of
 * the flows that only need its types) never pulls in an actor or a wallet store.
 */

import type { BridgeConfigResult } from './chains';
import { getBridgeCanisterId, loadBridgeConfig } from './bridgeActor';
import { buildDestinations, type DestinationView } from './destinations';

/**
 * Read the destination list: the Base entry from the canister's `get_bridge_info`, resolved by the
 * same `resolveBridgeConfig` the page has always used, then the static disabled entries.
 *
 * Rejects when the canister cannot be read, exactly as `loadBridgeConfig` does, so a caller that keeps
 * its last good list through a failed refresh behaves as the page always has. `readBase` is a test seam.
 */
export async function loadDestinations(
  readBase: () => Promise<BridgeConfigResult> = loadBridgeConfig,
): Promise<DestinationView[]> {
  return buildDestinations(await readBase(), getBridgeCanisterId());
}
