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
  unreachableSolanaDestination,
  unreachableSuiDestination,
  withSolanaDestination,
  withSuiDestination,
  type DestinationView,
} from './destinations';
import { getSolanaBridgeCanisterId, loadSolanaDestination } from '../solanaBridge/solanaActor';
import { getSuiBridgeCanisterId, loadSuiDestination } from '../suiBridge/suiActor';

/**
 * How long a destination read may take before the page stops waiting for it, when ANOTHER family has already answered. A
 * healthy entry must not wait forever behind an unrelated read that never settles (an unavailable replica, a stalled
 * transport), so every family but a lone Base is bounded by this. It is one local timer, not a scheduler.
 */
export const READ_DEADLINE_MS = 8_000;

type Settled<T> = { kind: 'value'; value: T } | { kind: 'error'; error: unknown } | { kind: 'late' };

/** Run a read (a synchronous throw counts as a rejection) and report how it ended, or `late` when the deadline fires first. */
function bounded<T>(read: () => Promise<T>, deadline: Promise<'late'>): Promise<Settled<T>> {
  return Promise.race([
    settle(read),
    deadline.then((): Settled<T> => ({ kind: 'late' })),
  ]);
}

function settle<T>(read: () => Promise<T>): Promise<Settled<T>> {
  return new Promise<T>((resolve) => resolve(read())).then(
    (value): Settled<T> => ({ kind: 'value', value }),
    (error: unknown): Settled<T> => ({ kind: 'error', error }),
  );
}

function lateMessage(ms: number): string {
  return `it did not answer within ${Math.round(ms / 1000)} seconds`;
}

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
 *  - A read that never SETTLES is bounded: a native read gets `deadlineMs`; Base gets the same bound once a native
 *    instance is configured. The stalled family's own row says it did not answer, and the other families are shown. A
 *    lone Base (nothing native configured) is awaited as it always was.
 * `readBase`, `readSolana`, `readSui` and `deadlineMs` are test seams.
 */
export async function loadDestinations(
  readBase: () => Promise<BridgeConfigResult> = loadBridgeConfig,
  readSolana: () => Promise<DestinationView | null> = loadSolanaDestination,
  readSui: () => Promise<DestinationView | null> = loadSuiDestination,
  deadlineMs: number = READ_DEADLINE_MS,
): Promise<DestinationView[]> {
  const baseId = getBridgeCanisterId();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<'late'>((resolve) => {
    timer = setTimeout(() => resolve('late'), deadlineMs);
  });
  try {
    // All reads start together; none waits for another.
    const base = baseId === '' ? null : settle(readBase);
    const [solanaRead, suiRead] = await Promise.all([bounded(readSolana, deadline), bounded(readSui, deadline)]);
    const solana =
      solanaRead.kind === 'value' ? solanaRead.value : solanaRead.kind === 'late' ? unreachableSolanaDestination(lateMessage(deadlineMs), getSolanaBridgeCanisterId()) : null;
    const sui = suiRead.kind === 'value' ? suiRead.value : suiRead.kind === 'late' ? unreachableSuiDestination(lateMessage(deadlineMs), getSuiBridgeCanisterId()) : null;
    let list: DestinationView[];
    if (base === null) {
      list = unconfiguredDestinations();
    } else {
      // Bounded only once a native instance is configured; a lone Base is awaited without a bound.
      const outcome = solana !== null || sui !== null ? await Promise.race([base, deadline.then((): Settled<BridgeConfigResult> => ({ kind: 'late' }))]) : await base;
      if (outcome.kind === 'value') {
        list = buildDestinations(outcome.value, baseId);
      } else if (outcome.kind === 'late') {
        list = unreachableDestinations(lateMessage(deadlineMs), baseId);
      } else if (solana === null && sui === null) {
        throw outcome.error;
      } else {
        list = unreachableDestinations(outcome.error instanceof Error ? outcome.error.message : String(outcome.error), baseId);
      }
    }
    if (solana) list = withSolanaDestination(list, solana);
    if (sui) list = withSuiDestination(list, sui);
    return list;
  } finally {
    clearTimeout(timer);
  }
}
