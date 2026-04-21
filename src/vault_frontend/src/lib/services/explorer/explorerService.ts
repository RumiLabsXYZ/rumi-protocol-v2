/**
 * explorerService.ts — Unified data service for the explorer.
 *
 * Wraps all canister calls (backend, stability pool, 3pool) with
 * TTL-based Map caching so multiple components sharing the same data
 * within a short window don't re-fetch.
 */

import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent } from '@dfinity/agent';
import { publicActor } from '$services/protocol/apiClient';
import { stabilityPoolService } from '$services/stabilityPoolService';
import { threePoolService } from '$services/threePoolService';
import { ammService } from '$services/ammService';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import type { _SERVICE as IcusdLedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';
import type { _SERVICE as IcusdIndexService } from '$declarations/icusd_index/icusd_index.did';

// ── TTL constants (ms) ───────────────────────────────────────────────────────

const TTL = {
	STATUS: 15_000,
	VAULTS: 30_000,
	COLLATERAL: 60_000,
	EVENTS: 10_000,
	SNAPSHOTS: 300_000,
	TREASURY: 60_000,
	LIQUIDATIONS: 30_000,
	POOL: 30_000
} as const;

// ── Cache infrastructure ─────────────────────────────────────────────────────

interface CacheEntry<T> {
	data: T;
	ts: number;
}

const cache = new Map<string, CacheEntry<unknown>>();

function getCached<T>(key: string, ttlMs: number): T | null {
	const entry = cache.get(key);
	if (!entry) return null;
	if (Date.now() - entry.ts > ttlMs) {
		cache.delete(key);
		return null;
	}
	return entry.data as T;
}

function setCache<T>(key: string, data: T): T {
	cache.set(key, { data, ts: Date.now() });
	return data;
}

/**
 * Invalidate all cached entries, or only those whose key starts with `prefix`.
 */
export function invalidateCache(prefix?: string): void {
	if (!prefix) {
		cache.clear();
		return;
	}
	for (const key of cache.keys()) {
		if (key.startsWith(prefix)) cache.delete(key);
	}
}

// ── Protocol status ──────────────────────────────────────────────────────────

export async function fetchProtocolStatus(): Promise<any | null> {
	const key = 'status:protocol';
	const cached = getCached<any>(key, TTL.STATUS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_protocol_status();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchProtocolStatus failed:', err);
		return null;
	}
}

// ── Protocol config (all admin-settable parameters) ─────────────────────────

export async function fetchProtocolConfig(): Promise<any | null> {
	const key = 'config:protocol';
	const cached = getCached<any>(key, TTL.STATUS);
	if (cached) return cached;

	try {
		const result = await (publicActor as any).get_protocol_config();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchProtocolConfig failed:', err);
		return null;
	}
}

// ── Protocol mode ────────────────────────────────────────────────────────────

/** Protocol mode is derived from protocol status (status.mode). */
export async function fetchProtocolMode(): Promise<string> {
	const status = await fetchProtocolStatus();
	if (!status?.mode) return 'Unknown';
	return Object.keys(status.mode)[0] ?? 'Unknown';
}

// ── Collateral ───────────────────────────────────────────────────────────────

export async function fetchCollateralConfigs(): Promise<any[]> {
	const key = 'collateral:configs';
	const cached = getCached<any[]>(key, TTL.COLLATERAL);
	if (cached) return cached;

	try {
		const supported = await publicActor.get_supported_collateral_types();
		const configs = await Promise.all(
			supported.map(async ([principal, _status]: [Principal, any]) => {
				try {
					const cfg = await publicActor.get_collateral_config(principal);
					// get_collateral_config returns opt ([] or [config])
					return cfg.length > 0 ? cfg[0] : null;
				} catch {
					return null;
				}
			})
		);
		const result = configs.filter((c: any) => c !== null);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchCollateralConfigs failed:', err);
		return [];
	}
}

export async function fetchCollateralTotals(): Promise<any[]> {
	const key = 'collateral:totals';
	const cached = getCached<any[]>(key, TTL.COLLATERAL);
	if (cached) return cached;

	try {
		const result = await publicActor.get_collateral_totals();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchCollateralTotals failed:', err);
		return [];
	}
}

/**
 * Derive collateral prices from collateral configs (each has `last_price`).
 * Returns a Map of principal string → price number.
 */
export async function fetchCollateralPrices(): Promise<Map<string, number>> {
	const key = 'collateral:prices';
	const cached = getCached<Map<string, number>>(key, TTL.COLLATERAL);
	if (cached) return cached;

	try {
		const configs = await fetchCollateralConfigs();
		const priceMap = new Map<string, number>();
		for (const cfg of configs) {
			const pid = cfg.ledger_canister_id?.toText?.() ?? cfg.collateral_type?.toText?.() ?? '';
			const price = cfg.last_price?.[0] ?? cfg.last_price ?? 0;
			if (pid) priceMap.set(pid, Number(price));
		}
		return setCache(key, priceMap);
	} catch (err) {
		console.error('[explorerService] fetchCollateralPrices failed:', err);
		return new Map();
	}
}

// ── Vaults ───────────────────────────────────────────────────────────────────

export async function fetchAllVaults(): Promise<any[]> {
	const key = 'vaults:all';
	const cached = getCached<any[]>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_all_vaults();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAllVaults failed:', err);
		return [];
	}
}

/** Fetch a single vault by ID. Uses get_all_vaults and filters. */
export async function fetchVault(vaultId: bigint | number): Promise<any | null> {
	const id = Number(vaultId);
	const key = `vaults:single:${id}`;
	const cached = getCached<any>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const allVaults = await fetchAllVaults();
		const vault = allVaults.find((v: any) => Number(v.vault_id) === id);
		if (vault) return setCache(key, vault);
		return null;
	} catch (err) {
		console.error('[explorerService] fetchVault failed:', err);
		return null;
	}
}

export async function fetchVaultInterestRate(vaultId: bigint): Promise<number | null> {
	const key = `vaults:rate:${vaultId}`;
	const cached = getCached<number>(key, TTL.VAULTS);
	if (cached !== null) return cached;

	try {
		const result = await publicActor.get_vault_interest_rate(vaultId);
		if ('Ok' in result) {
			return setCache(key, result.Ok);
		}
		return null;
	} catch (err) {
		console.error('[explorerService] fetchVaultInterestRate failed:', err);
		return null;
	}
}

/** Fetch all vaults owned by a principal using get_vaults(Some(principal)). */
export async function fetchVaultsByOwner(principal: Principal): Promise<any[]> {
	const key = `vaults:owner:${principal.toText()}`;
	const cached = getCached<any[]>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_vaults([principal]);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchVaultsByOwner failed:', err);
		return [];
	}
}

export async function fetchVaultHistory(vaultId: bigint): Promise<any[]> {
	const key = `vaults:history:${vaultId}`;
	const cached = getCached<any[]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_vault_history(vaultId);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchVaultHistory failed:', err);
		return [];
	}
}

// ── Events ───────────────────────────────────────────────────────────────────

export async function fetchEventCount(): Promise<bigint> {
	const key = 'events:count';
	const cached = getCached<bigint>(key, TTL.EVENTS);
	if (cached !== null) return cached;

	try {
		const result = await publicActor.get_event_count();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchEventCount failed:', err);
		return 0n;
	}
}

/**
 * Fetch events with their global indices using the filtered endpoint.
 * Returns { total, events: Array<[bigint, Event]> }.
 *
 * get_events_filtered takes {start, length} where:
 *   - start = page number (0-indexed)
 *   - length = page size
 * It excludes AccrueInterest events automatically.
 */
export async function fetchEvents(
	page: bigint,
	pageSize: bigint
): Promise<{ total: bigint; events: [bigint, any][] }> {
	const key = `events:filtered:${page}:${pageSize}`;
	const cached = getCached<{ total: bigint; events: [bigint, any][] }>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_events_filtered({
			start: page,
			length: pageSize
		});
		const data = { total: result.total, events: result.events ?? [] };
		return setCache(key, data);
	} catch (err) {
		console.error('[explorerService] fetchEvents failed:', err);
		return { total: 0n, events: [] };
	}
}

export async function fetchEventsByPrincipal(principal: Principal): Promise<[bigint, any][]> {
	const key = `events:principal:${principal.toText()}`;
	const cached = getCached<[bigint, any][]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_events_by_principal(principal);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchEventsByPrincipal failed:', err);
		return [];
	}
}

// ── Liquidations ─────────────────────────────────────────────────────────────

// Note: get_liquidation_records / get_liquidation_record_count don't exist in the backend.
// Liquidation events are accessed via the event log (fetchEvents) filtered by event type,
// and via the stability pool's getLiquidationHistory.

/** Fetch vaults that are currently liquidatable. */
export async function fetchLiquidatableVaults(): Promise<any[]> {
	const key = 'liquidations:liquidatable';
	const cached = getCached<any[]>(key, TTL.LIQUIDATIONS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_liquidatable_vaults();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchLiquidatableVaults failed:', err);
		return [];
	}
}

export async function fetchBotStats(): Promise<any | null> {
	const key = 'liquidations:botstats';
	const cached = getCached<any>(key, TTL.LIQUIDATIONS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_bot_stats();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchBotStats failed:', err);
		return null;
	}
}

// ── Treasury ─────────────────────────────────────────────────────────────────

export async function fetchTreasuryStats(): Promise<any | null> {
	const key = 'treasury:stats';
	const cached = getCached<any>(key, TTL.TREASURY);
	if (cached) return cached;

	try {
		const result = await publicActor.get_treasury_stats();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchTreasuryStats failed:', err);
		return null;
	}
}

export async function fetchInterestSplit(): Promise<any | null> {
	const key = 'treasury:interestsplit';
	const cached = getCached<any>(key, TTL.TREASURY);
	if (cached) return cached;

	try {
		const result = await publicActor.get_interest_split();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchInterestSplit failed:', err);
		return null;
	}
}

// ── Snapshots ────────────────────────────────────────────────────────────────

export async function fetchSnapshotCount(): Promise<bigint> {
	const key = 'snapshots:count';
	const cached = getCached<bigint>(key, TTL.SNAPSHOTS);
	if (cached !== null) return cached;

	try {
		const result = await publicActor.get_snapshot_count();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchSnapshotCount failed:', err);
		return 0n;
	}
}

export async function fetchSnapshots(start: bigint, length: bigint): Promise<any[]> {
	const key = `snapshots:range:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.SNAPSHOTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_protocol_snapshots({ start, length });
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchSnapshots failed:', err);
		return [];
	}
}

/**
 * Fetch all snapshots by batching requests. Useful for charting.
 * Uses a batch size of 100 to avoid response-size limits.
 */
export async function fetchAllSnapshots(batchSize = 100n): Promise<any[]> {
	const key = 'snapshots:all';
	const cached = getCached<any[]>(key, TTL.SNAPSHOTS);
	if (cached) return cached;

	try {
		const total = await fetchSnapshotCount();
		if (total === 0n) return setCache(key, []);

		const batches: Promise<any[]>[] = [];
		for (let start = 0n; start < total; start += batchSize) {
			const len = start + batchSize > total ? total - start : batchSize;
			batches.push(fetchSnapshots(start, len));
		}

		const results = await Promise.all(batches);
		const all = results.flat();
		return setCache(key, all);
	} catch (err) {
		console.error('[explorerService] fetchAllSnapshots failed:', err);
		return [];
	}
}

// ── Stability Pool ───────────────────────────────────────────────────────────

export async function fetchStabilityPoolStatus(): Promise<any | null> {
	const key = 'pool:stability:status';
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await stabilityPoolService.getPoolStatus();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchStabilityPoolStatus failed:', err);
		return null;
	}
}

export async function fetchStabilityPoolLiquidations(limit?: number): Promise<any[]> {
	const key = `pool:stability:liquidations:${limit ?? 'all'}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await stabilityPoolService.getLiquidationHistory(limit);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchStabilityPoolLiquidations failed:', err);
		return [];
	}
}

export async function fetchStabilityPoolEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:stability:events:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await stabilityPoolService.getPoolEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchStabilityPoolEvents failed:', err);
		return [];
	}
}

export async function fetchStabilityPoolEventCount(): Promise<bigint> {
	const key = 'pool:stability:eventcount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await stabilityPoolService.getPoolEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchStabilityPoolEventCount failed:', err);
		return 0n;
	}
}

// ── 3Pool ────────────────────────────────────────────────────────────────────

export async function fetchThreePoolStatus(): Promise<any | null> {
	const key = 'pool:3pool:status';
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getPoolStatus();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolStatus failed:', err);
		return null;
	}
}

export async function fetchSwapEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:3pool:swaps:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getSwapEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchSwapEvents failed:', err);
		return [];
	}
}

export async function fetchSwapEventCount(): Promise<bigint> {
	const key = 'pool:3pool:swapcount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await threePoolService.getSwapEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchSwapEventCount failed:', err);
		return 0n;
	}
}

// ── AMM ─────────────────────────────────────────────────────────────────────

export async function fetchAmmPools(): Promise<any[]> {
	const key = 'pool:amm:pools';
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await ammService.getPools();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmPools failed:', err);
		return [];
	}
}

export async function fetchAmmSwapEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:amm:swaps:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await ammService.getSwapEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmSwapEvents failed:', err);
		return [];
	}
}

export async function fetchAmmSwapEventCount(): Promise<bigint> {
	const key = 'pool:amm:swapcount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await ammService.getSwapEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmSwapEventCount failed:', err);
		return 0n;
	}
}

export async function fetchAmmLiquidityEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:amm:liquidity:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await ammService.getLiquidityEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmLiquidityEvents failed:', err);
		return [];
	}
}

export async function fetchAmmLiquidityEventCount(): Promise<bigint> {
	const key = 'pool:amm:liquiditycount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await ammService.getLiquidityEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmLiquidityEventCount failed:', err);
		return 0n;
	}
}

export async function fetchAmmAdminEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:amm:admin:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await ammService.getAdminEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmAdminEvents failed:', err);
		return [];
	}
}

export async function fetchAmmAdminEventCount(): Promise<bigint> {
	const key = 'pool:amm:admincount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await ammService.getAdminEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmAdminEventCount failed:', err);
		return 0n;
	}
}

// ── 3Pool Liquidity & Admin Events ──────────────────────────────────────────

/**
 * Fetch 3Pool v2 liquidity events newest-first.
 * `limit` = max events, `offset` = skip this many most-recent. Matches the canister's v2 contract.
 * Callers that want "all events newest-first" should pass (totalCount, 0n).
 */
export async function fetch3PoolLiquidityEvents(limit: bigint, offset: bigint): Promise<any[]> {
	const key = `pool:3pool:liquidity:${limit}:${offset}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getLiquidityEvents(limit, offset);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolLiquidityEvents failed:', err);
		return [];
	}
}

export async function fetch3PoolLiquidityEventCount(): Promise<bigint> {
	const key = 'pool:3pool:liquiditycount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await threePoolService.getLiquidityEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolLiquidityEventCount failed:', err);
		return 0n;
	}
}

export async function fetch3PoolAdminEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:3pool:admin:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getAdminEvents(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolAdminEvents failed:', err);
		return [];
	}
}

export async function fetch3PoolAdminEventCount(): Promise<bigint> {
	const key = 'pool:3pool:admincount';
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await threePoolService.getAdminEventCount();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolAdminEventCount failed:', err);
		return 0n;
	}
}

// ── Single DEX Event Fetch ─────────────────────────────────────────────────

export type DexEventSource = '3pool_swap' | 'amm_swap' | 'amm_liquidity' | 'amm_admin' | '3pool_liquidity' | '3pool_admin' | 'stability_pool';

/**
 * Fetch a single event from a non-backend source by its id.
 *
 * Canister pagination semantics differ across sources: 3pool's v2 liquidity
 * endpoint is newest-first (offset = skip-from-newest), while AMM / SP / 3pool
 * v1 endpoints are oldest-first where `start` happens to equal the id because
 * ids are assigned sequentially. Rather than hand-craft each code path, we
 * fetch the full event log (reusing the cache populated by list views) and
 * find the matching id client-side. Current event counts are small (~tens),
 * and fetches are cached, so this is cheap.
 */
export async function fetchDexEvent(source: DexEventSource, id: number): Promise<any | null> {
	const key = `dex:event:${source}:${id}`;
	const cached = getCached<any>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const all = await fetchAllDexEvents(source);
		const match = all.find((e: any) => Number(e.id ?? 0) === id) ?? null;
		if (match) return setCache(key, match);
		return null;
	} catch (err) {
		console.error(`[explorerService] fetchDexEvent(${source}, ${id}) failed:`, err);
		return null;
	}
}

/** Return the total count of events for a non-backend source. Used to gate "Next" navigation. */
export async function fetchDexEventCount(source: DexEventSource): Promise<bigint> {
	switch (source) {
		case '3pool_swap':       return fetchSwapEventCount();
		case 'amm_swap':         return fetchAmmSwapEventCount();
		case 'amm_liquidity':    return fetchAmmLiquidityEventCount();
		case 'amm_admin':        return fetchAmmAdminEventCount();
		case '3pool_liquidity':  return fetch3PoolLiquidityEventCount();
		case '3pool_admin':      return fetch3PoolAdminEventCount();
		case 'stability_pool':   return fetchStabilityPoolEventCount();
		default:                 return 0n;
	}
}

/** Fetch every event from a non-backend source (count → full fetch). Cached via the per-range helpers. */
export async function fetchAllDexEvents(source: DexEventSource): Promise<any[]> {
	switch (source) {
		case '3pool_swap': {
			const count = await fetchSwapEventCount();
			return Number(count) > 0 ? fetchSwapEvents(0n, count) : [];
		}
		case 'amm_swap': {
			const count = await fetchAmmSwapEventCount();
			return Number(count) > 0 ? fetchAmmSwapEvents(0n, count) : [];
		}
		case 'amm_liquidity': {
			const count = await fetchAmmLiquidityEventCount();
			return Number(count) > 0 ? fetchAmmLiquidityEvents(0n, count) : [];
		}
		case 'amm_admin': {
			const count = await fetchAmmAdminEventCount();
			return Number(count) > 0 ? fetchAmmAdminEvents(0n, count) : [];
		}
		case '3pool_liquidity': {
			const count = await fetch3PoolLiquidityEventCount();
			return Number(count) > 0 ? fetch3PoolLiquidityEvents(count, 0n) : [];
		}
		case '3pool_admin': {
			const count = await fetch3PoolAdminEventCount();
			return Number(count) > 0 ? fetch3PoolAdminEvents(0n, count) : [];
		}
		case 'stability_pool': {
			const count = await fetchStabilityPoolEventCount();
			return Number(count) > 0 ? fetchStabilityPoolEvents(0n, count) : [];
		}
		default:
			return [];
	}
}

// ── icUSD Holders ──────────────────────────────────────────────────────

export interface TokenHolder {
	account: string;       // "principal" or "principal:subaccount_hex"
	principal: string;
	subaccount: string | null;
	balance: bigint;
	balanceNumber: number; // for sorting in DataTable
}

function accountToKey(owner: Principal, subaccount?: Uint8Array | number[] | null): string {
	if (!subaccount || subaccount.length === 0) return owner.toText();
	const bytes = subaccount instanceof Uint8Array ? subaccount : new Uint8Array(subaccount);
	// Skip all-zero default subaccounts
	if (bytes.every(b => b === 0)) return owner.toText();
	const hex = Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
	return `${owner.toText()}:${hex}`;
}

let _icusdLedgerActor: IcusdLedgerService | null = null;

function getIcusdLedgerActor(): IcusdLedgerService {
	if (_icusdLedgerActor) return _icusdLedgerActor;
	const agent = new HttpAgent({ host: CONFIG.host });
	if (CONFIG.isLocal) agent.fetchRootKey().catch(() => {});
	_icusdLedgerActor = Actor.createActor<IcusdLedgerService>(CONFIG.icusd_ledgerIDL as any, {
		agent,
		canisterId: CANISTER_IDS.ICUSD_LEDGER,
	});
	return _icusdLedgerActor;
}

let _icusdIndexActor: IcusdIndexService | null = null;

function getIcusdIndexActor(): IcusdIndexService {
	if (_icusdIndexActor) return _icusdIndexActor;
	const agent = new HttpAgent({ host: CONFIG.host });
	if (CONFIG.isLocal) agent.fetchRootKey().catch(() => {});
	_icusdIndexActor = Actor.createActor<IcusdIndexService>(CONFIG.icusdIndexIDL as any, {
		agent,
		canisterId: CANISTER_IDS.ICUSD_INDEX,
	});
	return _icusdIndexActor;
}

/**
 * Fetch all icUSD holders by replaying ledger transactions.
 * Returns holders sorted by balance descending.
 */
export async function fetchIcusdHolders(): Promise<{ holders: TokenHolder[]; totalSupply: bigint; txCount: bigint }> {
	const key = 'holders:icusd';
	const cached = getCached<{ holders: TokenHolder[]; totalSupply: bigint; txCount: bigint }>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const ledger = getIcusdLedgerActor();
		const balances = new Map<string, { principal: string; subaccount: string | null; balance: bigint }>();

		function credit(owner: Principal, subaccount: any, amount: bigint) {
			const k = accountToKey(owner, subaccount?.[0] ?? null);
			const existing = balances.get(k);
			if (existing) {
				existing.balance += amount;
			} else {
				const sub = subaccount?.[0] ?? null;
				const subHex = sub ? Array.from(sub instanceof Uint8Array ? sub : new Uint8Array(sub)).map((b: number) => b.toString(16).padStart(2, '0')).join('') : null;
				balances.set(k, { principal: owner.toText(), subaccount: subHex, balance: amount });
			}
		}

		function debit(owner: Principal, subaccount: any, amount: bigint) {
			const k = accountToKey(owner, subaccount?.[0] ?? null);
			const existing = balances.get(k);
			if (existing) {
				existing.balance -= amount;
			}
		}

		// Fetch all transactions in batches
		const BATCH_SIZE = 2000n;
		let start = 0n;
		let totalTxCount = 0n;

		while (true) {
			const resp = await ledger.get_transactions({ start, length: BATCH_SIZE });
			totalTxCount = resp.log_length;
			const txs = resp.transactions;
			if (txs.length === 0) break;

			for (const tx of txs) {
				const mint = tx.mint[0];
				if (mint) {
					credit(mint.to.owner, mint.to.subaccount, mint.amount);
				}
				const burn = tx.burn[0];
				if (burn) {
					debit(burn.from.owner, burn.from.subaccount, burn.amount);
				}
				const xfer = tx.transfer[0];
				if (xfer) {
					debit(xfer.from.owner, xfer.from.subaccount, xfer.amount);
					credit(xfer.to.owner, xfer.to.subaccount, xfer.amount);
					const fee = xfer.fee[0];
					if (fee !== undefined) {
						debit(xfer.from.owner, xfer.from.subaccount, fee);
					}
				}
				// approve transactions don't affect balances
			}

			start += BigInt(txs.length);
			if (start >= totalTxCount) break;
		}

		// Filter out zero/negative balances and sort descending
		const holders: TokenHolder[] = [];
		let totalSupply = 0n;
		for (const [account, info] of balances) {
			if (info.balance > 0n) {
				totalSupply += info.balance;
				holders.push({
					account,
					principal: info.principal,
					subaccount: info.subaccount,
					balance: info.balance,
					balanceNumber: Number(info.balance) / 1e8,
				});
			}
		}
		holders.sort((a, b) => (b.balance > a.balance ? 1 : b.balance < a.balance ? -1 : 0));

		const result = { holders, totalSupply, txCount: totalTxCount };
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchIcusdHolders failed:', err);
		return { holders: [], totalSupply: 0n, txCount: 0n };
	}
}

// ── 3USD Holders ───────────────────────────────────────────────────────

const THREEUSD_INDEX = 'jagpu-pyaaa-aaaap-qtm6q-cai';

let _threeusdIndexActor: IcusdIndexService | null = null;

function getThreeusdIndexActor(): IcusdIndexService {
	if (_threeusdIndexActor) return _threeusdIndexActor;
	const agent = new HttpAgent({ host: CONFIG.host });
	if (CONFIG.isLocal) agent.fetchRootKey().catch(() => {});
	// Same WASM/IDL as icusd_index (both are ic-icrc1-index-ng)
	_threeusdIndexActor = Actor.createActor<IcusdIndexService>(CONFIG.icusdIndexIDL as any, {
		agent,
		canisterId: THREEUSD_INDEX,
	});
	return _threeusdIndexActor;
}

/**
 * Extract all unique account keys from ICRC-3 Value blocks.
 * Accounts are encoded as Array([Blob(principal_bytes)]) or
 * Array([Blob(principal_bytes), Blob(subaccount_bytes)]).
 */
function extractAccountsFromBlocks(blocks: any[]): Set<string> {
	const accounts = new Set<string>();

	function extractFromValue(val: any): void {
		if (!val) return;
		if ('Map' in val) {
			const map = val.Map as [string, any][];
			for (const [key, v] of map) {
				if (key === 'to' || key === 'from' || key === 'spender') {
					const acct = parseAccountValue(v);
					if (acct) accounts.add(acct);
				} else {
					extractFromValue(v);
				}
			}
		} else if ('Array' in val) {
			// Don't recurse into account arrays (they contain Blobs, not Maps)
		}
	}

	function parseAccountValue(val: any): string | null {
		if (!val || !('Array' in val)) return null;
		const arr = val.Array as any[];
		if (arr.length === 0) return null;
		const firstBlob = arr[0];
		if (!firstBlob || !('Blob' in firstBlob)) return null;
		const bytes = firstBlob.Blob instanceof Uint8Array
			? firstBlob.Blob
			: new Uint8Array(firstBlob.Blob);
		try {
			const principal = Principal.fromUint8Array(bytes);
			// Check for subaccount
			if (arr.length > 1 && arr[1] && 'Blob' in arr[1]) {
				const subBytes = arr[1].Blob instanceof Uint8Array
					? arr[1].Blob
					: new Uint8Array(arr[1].Blob);
				if (!subBytes.every((b: number) => b === 0)) {
					const hex = Array.from(subBytes).map((b) => (b as number).toString(16).padStart(2, '0')).join('');
					return `${principal.toText()}:${hex}`;
				}
			}
			return principal.toText();
		} catch {
			return null;
		}
	}

	for (const block of blocks) {
		extractFromValue(block);
	}
	return accounts;
}

/**
 * Fetch all 3USD holders by scanning index blocks and querying balances.
 * Returns holders sorted by balance descending.
 */
export async function fetchThreeUsdHolders(): Promise<{ holders: TokenHolder[]; totalSupply: bigint; txCount: bigint }> {
	const key = 'holders:3usd';
	const cached = getCached<{ holders: TokenHolder[]; totalSupply: bigint; txCount: bigint }>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const index = getThreeusdIndexActor();

		// Get block count
		const status = await index.status();
		const blockCount = status.num_blocks_synced;
		if (blockCount === 0n) {
			return setCache(key, { holders: [], totalSupply: 0n, txCount: 0n });
		}

		// Fetch all blocks (tiny dataset, ~125 blocks)
		const resp = await index.get_blocks({ start: 0n, length: blockCount });
		const uniqueAccounts = extractAccountsFromBlocks(resp.blocks);

		// Query icrc1_balance_of for each account via the index (which proxies to ledger)
		const holders: TokenHolder[] = [];
		let totalSupply = 0n;

		const balancePromises = Array.from(uniqueAccounts).map(async (acctKey) => {
			const parts = acctKey.split(':');
			const principalText = parts[0];
			const subaccountHex = parts[1] || null;

			let subaccount: number[] | null = null;
			if (subaccountHex) {
				subaccount = [];
				for (let i = 0; i < subaccountHex.length; i += 2) {
					subaccount.push(parseInt(subaccountHex.substring(i, i + 2), 16));
				}
			}

			const account = {
				owner: Principal.fromText(principalText),
				subaccount: subaccount ? [new Uint8Array(subaccount)] : [],
			};

			const balance = await index.icrc1_balance_of(account as any);
			return { acctKey, principalText, subaccountHex, balance };
		});

		const results = await Promise.all(balancePromises);

		for (const { acctKey, principalText, subaccountHex, balance } of results) {
			if (balance > 0n) {
				totalSupply += balance;
				holders.push({
					account: acctKey,
					principal: principalText,
					subaccount: subaccountHex,
					balance,
					balanceNumber: Number(balance) / 1e8,
				});
			}
		}

		holders.sort((a, b) => (b.balance > a.balance ? 1 : b.balance < a.balance ? -1 : 0));

		const result = { holders, totalSupply, txCount: blockCount };
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreeUsdHolders failed:', err);
		return { holders: [], totalSupply: 0n, txCount: 0n };
	}
}
