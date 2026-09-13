// Shared rate-loading state for the Earn surfaces (the `/earn` auto-selection
// redirect and the two-tab `EarnSubNav` badges on `/3usd` and
// `/stability-pool`). Both consumers need the same 3USD and Stability Pool
// rates; caching the in-flight/most-recent result here means a fresh `/earn`
// visit and the tab bar it renders don't fire duplicate canister queries.
//
// Each rate load is independent and never invents a 0% rate on failure: a
// missing/incomplete/non-finite result becomes `unavailable`, not a fake 0.

import { getThreePoolApy } from './threePoolApyService';
import { ProtocolService } from './protocol';
import { stabilityPoolService } from './stabilityPoolService';
import { liveSpApyPct } from '../utils/liveApy';

export type RateState =
  | { status: 'loading'; pct: null }
  | { status: 'ready'; pct: number }
  | { status: 'unavailable'; pct: null };

export const LOADING_RATE: RateState = { status: 'loading', pct: null };
export const UNAVAILABLE_RATE: RateState = { status: 'unavailable', pct: null };

/** How long the `/earn` redirect decision waits on either rate before treating it as unavailable for that decision. */
export const EARN_RATE_TIMEOUT_MS = 5000;

/** Reuse an in-flight/recent result across consumers instead of re-fetching immediately. */
const RATE_CACHE_TTL_MS = 30_000;

async function fetchThreeUsdRate(): Promise<RateState> {
  try {
    const r = await getThreePoolApy();
    // `complete` is false when any required input fell back to a default, so
    // a real fetch failure can't render as an invented 0.00% APY.
    if (!r.complete || !Number.isFinite(r.total_apy_pct)) {
      return UNAVAILABLE_RATE;
    }
    return { status: 'ready', pct: r.total_apy_pct };
  } catch (e) {
    console.warn('Earn: 3USD rate unavailable:', e);
    return UNAVAILABLE_RATE;
  }
}

async function fetchSpRate(): Promise<RateState> {
  try {
    const [protocolStatus, poolStatus] = await Promise.all([
      ProtocolService.getProtocolStatus(),
      stabilityPoolService.getPoolStatus(),
    ]);
    // Advertised rate = what a new icUSD depositor earns right now; the same
    // helper the Stability Pool page uses, so this can't drift from it.
    const pct = liveSpApyPct(protocolStatus as any, poolStatus as any);
    return pct !== null && Number.isFinite(pct)
      ? { status: 'ready', pct }
      : UNAVAILABLE_RATE;
  } catch (e) {
    console.warn('Earn: stability pool rate unavailable:', e);
    return UNAVAILABLE_RATE;
  }
}

let threeUsdPromise: Promise<RateState> | null = null;
let threeUsdFetchedAt = 0;

let spPromise: Promise<RateState> | null = null;
let spFetchedAt = 0;

export function loadThreeUsdRate(): Promise<RateState> {
  const now = Date.now();
  if (threeUsdPromise && now - threeUsdFetchedAt < RATE_CACHE_TTL_MS) {
    return threeUsdPromise;
  }
  threeUsdFetchedAt = now;
  threeUsdPromise = fetchThreeUsdRate();
  return threeUsdPromise;
}

export function loadSpRate(): Promise<RateState> {
  const now = Date.now();
  if (spPromise && now - spFetchedAt < RATE_CACHE_TTL_MS) {
    return spPromise;
  }
  spFetchedAt = now;
  spPromise = fetchSpRate();
  return spPromise;
}

/** Drop the module-scoped cache so the next `load*Rate()` call always fetches fresh. */
export function invalidateEarnRatesCache(): void {
  threeUsdPromise = null;
  threeUsdFetchedAt = 0;
  spPromise = null;
  spFetchedAt = 0;
}

/** Test-only alias, kept so existing specs don't need to change their calls. */
export const __resetEarnRatesCacheForTests = invalidateEarnRatesCache;

/**
 * A fresh `/earn` visit always wants the current highest-APY snapshot, not a
 * stale cached one from an earlier visit — so it drops the shared cache and
 * immediately kicks off both loads itself. Called synchronously during
 * `/earn`'s own component init (before its `EarnSubNav` child mounts), so
 * that child reuses these same in-flight promises via the normal
 * `loadThreeUsdRate`/`loadSpRate` cache instead of firing a second query.
 */
export function freshEarnSnapshot(): {
  loadThreeUsdRate: typeof loadThreeUsdRate;
  loadSpRate: typeof loadSpRate;
} {
  invalidateEarnRatesCache();
  loadThreeUsdRate();
  loadSpRate();
  return { loadThreeUsdRate, loadSpRate };
}

/**
 * Race a rate load against a bounded wait so a stalled canister query can
 * never hang the `/earn` redirect decision. The underlying load is not
 * cancelled: if it resolves later, cached consumers (like the tab badges)
 * still see the real result, but a decision already made using the fallback
 * does not get revisited.
 */
export function withTimeout<T>(promise: Promise<T>, ms: number, onTimeout: () => T): Promise<T> {
  return new Promise((resolve) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve(onTimeout());
    }, ms);
    promise.then(
      (value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(onTimeout());
      },
    );
  });
}

export type EarnDestination = '/3usd' | '/stability-pool';

/**
 * Which pool a fresh `/earn` visit should land on: whichever rate is ready
 * and higher. A single ready rate always wins over an unavailable one. Ties
 * or a double-unavailable both fall back to 3USD (never invents a winner
 * from two invented numbers).
 */
export function resolveEarnDestination(threeUsd: RateState, sp: RateState): EarnDestination {
  const threeReady = threeUsd.status === 'ready';
  const spReady = sp.status === 'ready';

  if (threeReady && spReady) {
    return sp.pct > threeUsd.pct ? '/stability-pool' : '/3usd';
  }
  if (spReady && !threeReady) return '/stability-pool';
  return '/3usd';
}
