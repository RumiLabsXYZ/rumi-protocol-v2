import { writable, get } from 'svelte/store';
import { threePoolService } from './threePoolService';

/**
 * Single source of truth for the USD price of one 3USD token.
 *
 * 3USD is the 3pool's LP token, so its USD value is the pool's virtual price
 * (scaled by 1e18), not $1. The virtual price only grows as swap fees and
 * icUSD interest accrue to the pool, so it is always >= 1.0.
 *
 * Every USD-value computation involving a 3USD amount must go through here.
 * Pricing 3USD at $1 understates holdings by however much yield has accrued
 * (roughly 8.8% as of July 2026).
 */

/** Par value. Used only as a floor before the first successful fetch. */
const PAR = 1;

/** How long a fetched price stays fresh. The virtual price moves very slowly. */
const TTL_MS = 60_000;

/** Last successfully fetched price, shared by every consumer. */
export const threeUsdPrice = writable<number>(PAR);

let lastFetchedAt = 0;
let inFlight: Promise<number> | null = null;

function parseVirtualPrice(virtualPrice: bigint | number | undefined): number | null {
  if (virtualPrice === undefined || virtualPrice === null) return null;
  const value = Number(virtualPrice) / 1e18;
  // A zero/NaN virtual price means the pool is uninitialized or the read
  // failed. Never let that collapse a balance to $0.
  if (!Number.isFinite(value) || value <= 0) return null;
  return value;
}

/**
 * Derive the 3USD price from an already-fetched pool status, and refresh the
 * shared store with it. Use this when the caller has a `PoolStatus` in hand so
 * it does not pay for a second query.
 */
export function threeUsdPriceFromPoolStatus(
  poolStatus: { virtual_price?: bigint | number } | null | undefined,
): number {
  const value = parseVirtualPrice(poolStatus?.virtual_price);
  if (value === null) return get(threeUsdPrice);
  threeUsdPrice.set(value);
  lastFetchedAt = Date.now();
  return value;
}

/**
 * Current USD price of one 3USD, fetching the 3pool virtual price if the
 * cached value is stale. Concurrent callers share a single in-flight query.
 *
 * On failure this returns the last known good price (or par before the first
 * successful fetch) rather than throwing or returning 0, so a transient query
 * failure degrades to a slight understatement instead of a $0 balance.
 */
export async function getThreeUsdPrice(): Promise<number> {
  const cached = get(threeUsdPrice);
  if (Date.now() - lastFetchedAt < TTL_MS) return cached;
  if (inFlight) return inFlight;

  inFlight = (async () => {
    try {
      const status = await threePoolService.getPoolStatus();
      return threeUsdPriceFromPoolStatus(status);
    } catch (e) {
      console.warn('3USD price fetch failed, using last known price:', e);
      return get(threeUsdPrice);
    } finally {
      inFlight = null;
    }
  })();

  return inFlight;
}

/**
 * Synchronous read of the cached price, for render paths that cannot await
 * (pure sync utils, Svelte markup expressions).
 *
 * Triggers a background refresh when the cache is stale so the next render
 * gets a fresh value. Returns par until the first successful fetch lands, so
 * an early caller understates rather than showing $0.
 */
export function peekThreeUsdPrice(): number {
  if (Date.now() - lastFetchedAt >= TTL_MS && !inFlight) {
    void getThreeUsdPrice();
  }
  return get(threeUsdPrice);
}
