/**
 * seasonStore.ts — one shared, lazily-loaded copy of the airdrop season status so
 * every "earn Nx points" badge can gate on the live season phase without each
 * component issuing its own query. Call `ensureLoaded()` from any component that
 * shows earning UI; the underlying queries are cached and loaded at most once.
 */
import { writable, derived } from 'svelte/store';
import { POINTS_ENABLED } from '$lib/config';
import { getEpochStatus, getPointsConfig, getFiatStablePointsPolicy } from '$lib/services/pointsService';
import { seasonState, type SeasonPhase } from '$lib/utils/points';
import { UNKNOWN_FIAT_STABLE_POINTS_POLICY, type FiatStablePointsPolicy } from '$lib/utils/fiatStablePointsPolicy';
import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';

interface SeasonData {
  status: PublicEpochStatus | null;
  config: PointsConfig | null;
  policy: FiatStablePointsPolicy | null;
  loaded: boolean;
}

const store = writable<SeasonData>({ status: null, config: null, policy: null, loaded: false });
let started = false;

async function ensureLoaded(): Promise<void> {
  if (started || !POINTS_ENABLED) return;
  started = true;
  const [statusResult, configResult, policyResult] = await Promise.allSettled([
    Promise.resolve().then(() => getEpochStatus()),
    Promise.resolve().then(() => getPointsConfig()),
    Promise.resolve().then(() => getFiatStablePointsPolicy()),
  ]);
  const status = statusResult.status === 'fulfilled' ? statusResult.value : null;
  const config = configResult.status === 'fulfilled' ? configResult.value : null;
  const policy =
    policyResult.status === 'fulfilled' && policyResult.value
      ? policyResult.value
      : UNKNOWN_FIAT_STABLE_POINTS_POLICY;
  store.set({ status, config, policy, loaded: true });

  if (statusResult.status === 'rejected' || configResult.status === 'rejected' || policyResult.status === 'rejected') {
    console.error('[seasonStore] partial load; failed queries will retry on next trigger', {
      status: statusResult.status === 'rejected' ? statusResult.reason : undefined,
      config: configResult.status === 'rejected' ? configResult.reason : undefined,
      policy: policyResult.status === 'rejected' ? policyResult.reason : undefined,
    });
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
