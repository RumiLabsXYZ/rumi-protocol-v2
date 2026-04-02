/**
 * explorerService.ts — Unified data service for the explorer.
 *
 * Wraps all canister calls (backend, stability pool, 3pool) with
 * TTL-based Map caching so multiple components sharing the same data
 * within a short window don't re-fetch.
 */

import { Principal } from '@dfinity/principal';
import { publicActor } from '$services/protocol/apiClient';
import { stabilityPoolService } from '$services/stabilityPoolService';
import { threePoolService } from '$services/threePoolService';
import { ammService } from '$services/ammService';

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

export async function fetch3PoolLiquidityEvents(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:3pool:liquidity:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getLiquidityEvents(start, length);
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
