/**
 * pointsService.ts — anonymous, TTL-cached query functions for the rumi_points
 * canister. Mirrors analyticsService.ts. All calls are public queries; no wallet.
 *
 * Errors are NOT swallowed: the canister-call functions reject on failure so the
 * UI can distinguish a real failure (show error + retry + toast) from genuinely
 * empty data (e.g. an unregistered principal -> null). The cache distinguishes a
 * hit from a miss explicitly, so a cached `null`/`false` is served from cache
 * (a stored `null` is a real value, not a miss).
 */
import type { Principal } from '@dfinity/principal';
import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { idlFactory } from '$declarations/rumi_points/rumi_points.did.js';
import type {
  _SERVICE as PointsService,
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
} from '$declarations/rumi_points/rumi_points.did';

const TTL = {
  STATUS: 15_000,
  CONFIG: 60_000,
  PRINCIPAL: 15_000,
  LEADERBOARD: 30_000,
} as const;

interface CacheEntry<T> {
  data: T;
  ts: number;
}
const cache = new Map<string, CacheEntry<unknown>>();

type Cached<T> = { hit: true; value: T } | { hit: false };

function getCached<T>(key: string, ttlMs: number): Cached<T> {
  const entry = cache.get(key);
  if (!entry) return { hit: false };
  if (Date.now() - entry.ts > ttlMs) {
    cache.delete(key);
    return { hit: false };
  }
  return { hit: true, value: entry.data as T };
}
function setCache<T>(key: string, data: T): T {
  cache.set(key, { data, ts: Date.now() });
  return data;
}
export function invalidatePointsCache(prefix?: string): void {
  if (!prefix) {
    cache.clear();
    return;
  }
  for (const key of cache.keys()) if (key.startsWith(prefix)) cache.delete(key);
}

/**
 * Retry a read-only query through transient failures (boundary-node blips,
 * momentary network drops, transient replica rejections). All points calls are
 * idempotent queries, so retrying is always safe. Backoff: ~300ms, ~600ms.
 * Throws the last error once attempts are exhausted, so genuine failures still
 * surface to the UI (error state + Retry).
 */
async function withRetry<T>(fn: () => Promise<T>, attempts = 3, baseDelayMs = 300): Promise<T> {
  let lastErr: unknown;
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (e) {
      lastErr = e;
      if (i < attempts - 1) {
        await new Promise((r) => setTimeout(r, baseDelayMs * 2 ** i));
      }
    }
  }
  throw lastErr;
}

let _actor: PointsService | null = null;
function getActor(): PointsService {
  if (_actor) return _actor;
  const host = CONFIG.isLocal ? 'http://localhost:4943' : 'https://icp0.io';
  const agent = new HttpAgent({ host });
  if (CONFIG.isLocal) {
    agent.fetchRootKey().catch((e) => console.warn('[pointsService] fetchRootKey failed', e));
  }
  _actor = Actor.createActor<PointsService>(idlFactory, {
    agent,
    canisterId: CANISTER_IDS.RUMI_POINTS,
  });
  return _actor;
}

export async function getEpochStatus(): Promise<PublicEpochStatus> {
  const key = 'points:status';
  const c = getCached<PublicEpochStatus>(key, TTL.STATUS);
  if (c.hit) return c.value;
  return setCache(key, await withRetry(() => getActor().get_epoch_status()));
}

export async function getPointsConfig(): Promise<PointsConfig> {
  const key = 'points:config';
  const c = getCached<PointsConfig>(key, TTL.CONFIG);
  if (c.hit) return c.value;
  return setCache(key, await withRetry(() => getActor().get_points_config()));
}

export async function isRegistered(p: Principal): Promise<boolean> {
  const key = `points:reg:${p.toText()}`;
  const c = getCached<boolean>(key, TTL.PRINCIPAL);
  if (c.hit) return c.value;
  return setCache(key, await withRetry(() => getActor().is_registered(p)));
}

export async function isExcluded(p: Principal): Promise<boolean> {
  const key = `points:excl:${p.toText()}`;
  const c = getCached<boolean>(key, TTL.PRINCIPAL);
  if (c.hit) return c.value;
  return setCache(key, await withRetry(() => getActor().is_excluded(p)));
}

export async function getPrincipalState(p: Principal): Promise<PrincipalState | null> {
  const key = `points:state:${p.toText()}`;
  const c = getCached<PrincipalState | null>(key, TTL.PRINCIPAL);
  if (c.hit) return c.value;
  // get_principal_state returns a candid opt: [] | [PrincipalState].
  const [val] = await withRetry(() => getActor().get_principal_state(p));
  return setCache<PrincipalState | null>(key, val ?? null);
}

export async function getLeaderboard(offset: number, limit: number): Promise<LeaderboardEntry[]> {
  const key = `points:lb:${offset}:${limit}`;
  const c = getCached<LeaderboardEntry[]>(key, TTL.LEADERBOARD);
  if (c.hit) return c.value;
  return setCache(key, await withRetry(() => getActor().get_leaderboard(offset, limit)));
}
