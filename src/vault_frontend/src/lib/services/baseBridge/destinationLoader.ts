/**
 * Reads the destination list from the bridge canisters.
 *
 * Kept apart from `destinations.ts` so the pure destination logic (and the specs of
 * the flows that only need its types) never pulls in an actor or a wallet store.
 */

import type { BridgeConfigResult } from './chains';
import { getBridgeCanisterId, loadBridgeConfig } from './bridgeActor';
import {
  buildDestinations,
  unconfiguredDestinations,
  unreachableDestinations,
  withSolanaDestination,
  withSuiDestination,
  type DestinationView,
} from './destinations';
import { loadSolanaDestination } from '../solanaBridge/solanaActor';
import { loadSuiDestination } from '../suiBridge/suiActor';

/**
 * Read the destination list: the Base entry from the canister's `get_bridge_info`, resolved by the same
 * `resolveBridgeConfig` the page has always used, then the static disabled entries. A native entry (Solana, Sui) replaces
 * its static row only when a configured instance of it answers (none is configured today, so no request is made and the
 * rows stay disabled).
 *
 * Every destination's read is its own: the native reads do not wait for Base, and a failure of one never hides another.
 *  - Base NOT configured (an empty canister id, e.g. a native-only deployment): no Base request is made and the Base row
 *    says it is not available.
 *  - Base configured but unreachable: when no native instance is configured either, this rejects exactly as
 *    `loadBridgeConfig` does, so a caller that keeps its last good list through a failed refresh behaves as the page
 *    always has; when a native instance answered, the Base row is unavailable with the reason and the native entry stays
 *    usable.
 *  - A failure of a native read never rejects: it makes only that native row unavailable.
 * `readBase`, `readSolana` and `readSui` are test seams.
 */
export async function loadDestinations(
  readBase: () => Promise<BridgeConfigResult> = loadBridgeConfig,
  readSolana: () => Promise<DestinationView | null> = loadSolanaDestination,
  readSui: () => Promise<DestinationView | null> = loadSuiDestination,
): Promise<DestinationView[]> {
  const baseId = getBridgeCanisterId();
  // All reads start together; none waits for another.
  const base =
    baseId === ''
      ? null
      : readBase().then(
          (result) => ({ ok: true as const, result }),
          (error: unknown) => ({ ok: false as const, error }),
        );
  const [solana, sui] = await Promise.all([readSolana().catch(() => null), readSui().catch(() => null)]);
  let list: DestinationView[];
  if (base === null) {
    list = unconfiguredDestinations();
  } else {
    const outcome = await base;
    if (outcome.ok) {
      list = buildDestinations(outcome.result, baseId);
    } else if (solana === null && sui === null) {
      throw outcome.error;
    } else {
      list = unreachableDestinations(outcome.error instanceof Error ? outcome.error.message : String(outcome.error), baseId);
    }
  }
  if (solana) list = withSolanaDestination(list, solana);
  if (sui) list = withSuiDestination(list, sui);
  return list;
}
