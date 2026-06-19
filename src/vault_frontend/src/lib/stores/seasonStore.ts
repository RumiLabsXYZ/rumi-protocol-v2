/**
 * seasonStore.ts — one shared, lazily-loaded copy of the airdrop season status so
 * every "earn Nx points" badge can gate on the live season phase without each
 * component issuing its own query. Call `ensureLoaded()` from any component that
 * shows earning UI; the underlying queries are cached and loaded at most once.
 */
import { writable, derived } from 'svelte/store';
import { POINTS_ENABLED } from '$lib/config';
import { getEpochStatus, getPointsConfig } from '$lib/services/pointsService';
import { seasonState, type SeasonPhase } from '$lib/utils/points';
import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';

interface SeasonData {
  status: PublicEpochStatus | null;
  config: PointsConfig | null;
  loaded: boolean;
}

const store = writable<SeasonData>({ status: null, config: null, loaded: false });
let started = false;

async function ensureLoaded(): Promise<void> {
  if (started || !POINTS_ENABLED) return;
  started = true;
  try {
    const [status, config] = await Promise.all([getEpochStatus(), getPointsConfig()]);
    store.set({ status, config, loaded: true });
  } catch (e) {
    // Don't latch a transient failure: release the guard so a later trigger
    // (navigation, another component mounting) retries instead of leaving the
    // season bar/badges stuck off for the rest of the session. The service
    // layer already retries each call with backoff before we get here.
    console.error('[seasonStore] load failed, will retry on next trigger', e);
    started = false;
  }
}

export const seasonStore = { subscribe: store.subscribe, ensureLoaded };

export const seasonPhase = derived(
  store,
  ($s): SeasonPhase => seasonState($s.status, $s.config, BigInt(Date.now()) * 1_000_000n),
);

/** Whether to show "earn Nx points" badges — true during the live season and the
 *  pre-season run-up (positions are counted once the season opens). */
export const earningActive = derived(seasonPhase, ($p) => $p === 'live' || $p === 'pre');
