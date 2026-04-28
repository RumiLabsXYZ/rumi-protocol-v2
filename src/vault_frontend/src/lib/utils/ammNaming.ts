/**
 * Helpers for displaying AMM pool identifiers as friendly names.
 *
 * The AMM identifies pools by a string `pool_id` (typically a pair of canister
 * IDs joined by `_`). For UI surfaces we want stable, sequential labels like
 * "AMM1", "AMM2" plus the token pair, e.g. "AMM1 · 3USD/ICP".
 *
 * The numbering is computed client-side from a registry: pools are sorted by
 * `pool_id` lexicographically so the index is deterministic across reloads
 * and across users — without needing a backend index field.
 */

import { getTokenSymbol } from './explorerHelpers';

export interface AmmPoolLike {
  pool_id: string;
  token_a?: any;
  token_b?: any;
}

let registry: AmmPoolLike[] = [];
let indexById: Map<string, number> = new Map();

function principalToText(p: any): string {
  if (!p) return '';
  if (typeof p === 'string') return p;
  if (typeof p?.toText === 'function') return p.toText();
  return String(p);
}

/**
 * Seed the registry with the AMM pool list. Call this once after fetching
 * `getPools()`. Subsequent `ammPoolLabel(...)` lookups are synchronous.
 */
export function setAmmPoolRegistry(pools: AmmPoolLike[]): void {
  const sorted = [...pools].sort((a, b) => a.pool_id.localeCompare(b.pool_id));
  registry = sorted;
  indexById = new Map();
  sorted.forEach((p, i) => indexById.set(p.pool_id, i + 1));
}

/**
 * 1-based index for a pool by its id. Returns null if the pool isn't in the
 * registry yet (e.g. registry hasn't loaded). Numbering is alphabetical by
 * `pool_id` so it's deterministic.
 */
export function ammPoolIndex(poolId: string | undefined | null): number | null {
  if (!poolId) return null;
  return indexById.get(poolId) ?? null;
}

/**
 * Token pair label like "3USD/ICP". Falls back to a shortened pool id when
 * tokens aren't resolvable.
 */
export function ammPoolPair(poolId: string | undefined | null, fallbackTokenA?: any, fallbackTokenB?: any): string {
  if (!poolId) return '';
  const pool = registry.find((p) => p.pool_id === poolId);
  const aRaw = pool?.token_a ?? fallbackTokenA;
  const bRaw = pool?.token_b ?? fallbackTokenB;
  const a = aRaw ? getTokenSymbol(principalToText(aRaw)) : '';
  const b = bRaw ? getTokenSymbol(principalToText(bRaw)) : '';
  if (a && b) return `${a}/${b}`;
  return poolId;
}

/**
 * Short pool label like "AMM1". Use when you only have room for the index.
 */
export function ammPoolShortLabel(poolId: string | undefined | null): string {
  const idx = ammPoolIndex(poolId);
  return idx != null ? `AMM${idx}` : 'AMM';
}

/**
 * Long pool label like "AMM1 · 3USD/ICP". Use in headings / table cells where
 * you have room for both the index and the pair.
 */
export function ammPoolLabel(poolId: string | undefined | null, fallbackTokenA?: any, fallbackTokenB?: any): string {
  const short = ammPoolShortLabel(poolId);
  const pair = ammPoolPair(poolId, fallbackTokenA, fallbackTokenB);
  if (pair && pair !== poolId) return `${short} · ${pair}`;
  return short;
}
