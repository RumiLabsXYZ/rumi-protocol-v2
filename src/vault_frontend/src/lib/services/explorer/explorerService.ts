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

export async function fetchProtocolMode(): Promise<any | null> {
	const key = 'status:mode';
	const cached = getCached<any>(key, TTL.STATUS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_protocol_mode();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchProtocolMode failed:', err);
		return null;
	}
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

export async function fetchCollateralPrices(): Promise<[Principal, number][]> {
	const key = 'collateral:prices';
	const cached = getCached<[Principal, number][]>(key, TTL.COLLATERAL);
	if (cached) return cached;

	try {
		const result = await publicActor.get_latest_collateral_prices();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchCollateralPrices failed:', err);
		return [];
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

export async function fetchVault(vaultId: bigint): Promise<any | null> {
	const key = `vaults:${vaultId}`;
	const cached = getCached<any>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_vault(vaultId);
		if ('Ok' in result) {
			return setCache(key, result.Ok);
		}
		console.warn('[explorerService] fetchVault error result:', result.Err);
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

export async function fetchVaultsByOwner(principal: Principal): Promise<bigint[]> {
	const key = `vaults:owner:${principal.toText()}`;
	const cached = getCached<bigint[]>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_vaults_by_principal(principal);
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
 * Returns tuples of [global_index, Event].
 * Pass an empty event_types array to get all events except AccrueInterest.
 */
export async function fetchEvents(
	start: bigint,
	length: bigint,
	eventTypes: string[] = []
): Promise<[bigint, any][]> {
	const key = `events:filtered:${start}:${length}:${eventTypes.join(',')}`;
	const cached = getCached<[bigint, any][]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_events_filtered({
			start,
			length,
			event_types: eventTypes
		});
		if ('Ok' in result) {
			return setCache(key, result.Ok);
		}
		console.warn('[explorerService] fetchEvents error result:', result);
		return [];
	} catch (err) {
		console.error('[explorerService] fetchEvents failed:', err);
		return [];
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

export async function fetchLiquidationRecords(start: bigint, length: bigint): Promise<any[]> {
	const key = `liquidations:records:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.LIQUIDATIONS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_liquidation_records(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchLiquidationRecords failed:', err);
		return [];
	}
}

export async function fetchLiquidationCount(): Promise<bigint> {
	const key = 'liquidations:count';
	const cached = getCached<bigint>(key, TTL.LIQUIDATIONS);
	if (cached !== null) return cached;

	try {
		const result = await publicActor.get_liquidation_record_count();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchLiquidationCount failed:', err);
		return 0n;
	}
}

export async function fetchPendingLiquidations(): Promise<any[]> {
	const key = 'liquidations:pending';
	const cached = getCached<any[]>(key, TTL.LIQUIDATIONS);
	if (cached) return cached;

	try {
		const result = await publicActor.get_pending_liquidations();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchPendingLiquidations failed:', err);
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
		const result = await publicActor.get_protocol_snapshot_count();
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
		const result = await publicActor.get_protocol_snapshots(start, length);
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
