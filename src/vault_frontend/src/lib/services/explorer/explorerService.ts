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
import type { AmmStatsWindow } from '$services/ammService';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import type { _SERVICE as IcusdLedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';
import type { _SERVICE as IcusdIndexService } from '$declarations/icusd_index/icusd_index.did';
import type { UserStabilityPosition } from '$declarations/rumi_stability_pool/rumi_stability_pool.did';
import { idlFactory as stabilityPoolIDL } from '$declarations/rumi_stability_pool/rumi_stability_pool.did.js';

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

// ── Redemption parameters (live) ────────────────────────────────────────────

export async function fetchRedemptionRate(): Promise<number | null> {
	const key = 'redemption:rate';
	const cached = getCached<number>(key, TTL.STATUS);
	if (cached !== null) return cached;

	try {
		const result = await (publicActor as any).get_redemption_rate();
		return setCache(key, Number(result));
	} catch (err) {
		console.error('[explorerService] fetchRedemptionRate failed:', err);
		return null;
	}
}

export async function fetchRedemptionFeeFloor(): Promise<number | null> {
	const key = 'redemption:floor';
	const cached = getCached<number>(key, TTL.STATUS);
	if (cached !== null) return cached;

	try {
		const result = await (publicActor as any).get_redemption_fee_floor();
		return setCache(key, Number(result));
	} catch (err) {
		console.error('[explorerService] fetchRedemptionFeeFloor failed:', err);
		return null;
	}
}

export async function fetchRedemptionFeeCeiling(): Promise<number | null> {
	const key = 'redemption:ceiling';
	const cached = getCached<number>(key, TTL.STATUS);
	if (cached !== null) return cached;

	try {
		const result = await (publicActor as any).get_redemption_fee_ceiling();
		return setCache(key, Number(result));
	} catch (err) {
		console.error('[explorerService] fetchRedemptionFeeCeiling failed:', err);
		return null;
	}
}

export async function fetchRedemptionTier(collateral: Principal): Promise<number | null> {
	const key = `redemption:tier:${collateral.toText()}`;
	const cached = getCached<number>(key, TTL.COLLATERAL);
	if (cached !== null) return cached;

	try {
		const result = await (publicActor as any).get_redemption_tier(collateral);
		if ('Ok' in result) return setCache(key, Number(result.Ok));
		return null;
	} catch (err) {
		console.error('[explorerService] fetchRedemptionTier failed:', err);
		return null;
	}
}

// ── 3pool rich analytics (state / stats / health / series) ──────────────────

export async function fetchThreePoolState(): Promise<any | null> {
	const key = 'pool:3pool:state';
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getPoolState();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolState failed:', err);
		return null;
	}
}

export async function fetchThreePoolStats(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last24h',
): Promise<any | null> {
	const key = `pool:3pool:stats:${window}`;
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getPoolStats(window);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolStats failed:', err);
		return null;
	}
}

export async function fetchThreePoolHealth(): Promise<any | null> {
	const key = 'pool:3pool:health';
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getPoolHealth();
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolHealth failed:', err);
		return null;
	}
}

export async function fetchThreePoolVolumeSeries(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
	bucketSecs: bigint = 3600n,
): Promise<any[]> {
	const key = `pool:3pool:vol:${window}:${bucketSecs}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getVolumeSeries(window, bucketSecs);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolVolumeSeries failed:', err);
		return [];
	}
}

export async function fetchThreePoolBalanceSeries(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
	bucketSecs: bigint = 3600n,
): Promise<any[]> {
	const key = `pool:3pool:bal:${window}:${bucketSecs}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getBalanceSeries(window, bucketSecs);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolBalanceSeries failed:', err);
		return [];
	}
}

export async function fetchThreePoolVirtualPriceSeries(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
	bucketSecs: bigint = 3600n,
): Promise<any[]> {
	const key = `pool:3pool:vp:${window}:${bucketSecs}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getVirtualPriceSeries(window, bucketSecs);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolVirtualPriceSeries failed:', err);
		return [];
	}
}

export async function fetchThreePoolFeeSeries(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
	bucketSecs: bigint = 3600n,
): Promise<any[]> {
	const key = `pool:3pool:fees:${window}:${bucketSecs}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getFeeSeries(window, bucketSecs);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolFeeSeries failed:', err);
		return [];
	}
}

export async function fetchThreePoolStatsWindow(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
): Promise<any | null> {
	const key = `pool:3pool:stats:${window}`;
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getPoolStats(window);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolStatsWindow failed:', err);
		return null;
	}
}

export async function fetchThreePoolTopLps(n: bigint = 10n): Promise<Array<[Principal, bigint, number]>> {
	const key = `pool:3pool:topLps:${n}`;
	const cached = getCached<Array<[Principal, bigint, number]>>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getTopLps(n);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolTopLps failed:', err);
		return [];
	}
}

export async function fetchThreePoolTopSwappers(
	window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
	n: bigint = 10n,
): Promise<Array<[Principal, bigint, bigint]>> {
	const key = `pool:3pool:topSwappers:${window}:${n}`;
	const cached = getCached<Array<[Principal, bigint, bigint]>>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const result = await threePoolService.getTopSwappers(window, n);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolTopSwappers failed:', err);
		return [];
	}
}

export async function fetchThreePoolSwapEventsV2(start: bigint, length: bigint): Promise<any[]> {
	const key = `pool:3pool:swapEventsV2:${start}:${length}`;
	const cached = getCached<any[]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const result = await threePoolService.getSwapEventsV2(start, length);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreePoolSwapEventsV2 failed:', err);
		return [];
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
 * Server-side filter spec for `fetchEvents`. Each field is optional; when
 * omitted the backend treats it as "no filter on this dimension". Maps 1:1
 * to the backend's `GetEventsArg` opt fields. Built from the activity-page
 * facet bar via `facetsToBackendFilters` in `$utils/eventFacets`.
 */
export interface BackendEventFilters {
	types?: BackendEventTypeFilter[];
	principal?: Principal;
	collateral_token?: Principal;
	time_range?: { start_ns: bigint; end_ns: bigint };
	min_size_e8s?: bigint;
	/**
	 * Narrow `Admin`-type matches to these canonical labels (e.g.
	 * "SetBorrowingFee"). No-op when `Admin` isn't in `types` or when the
	 * list is empty. See backend `Event::admin_label()` for the full set.
	 */
	admin_labels?: string[];
}

/** Mirrors the backend `EventTypeFilter` Candid variant. */
export type BackendEventTypeFilter =
	| 'OpenVault' | 'CloseVault' | 'AdjustVault'
	| 'Borrow' | 'Repay'
	| 'Liquidation' | 'PartialLiquidation'
	| 'Redemption' | 'ReserveRedemption'
	| 'StabilityPoolDeposit' | 'StabilityPoolWithdraw'
	| 'AdminMint' | 'AdminSweepToTreasury'
	| 'Admin' | 'PriceUpdate' | 'AccrueInterest';

function backendFiltersCacheKey(f: BackendEventFilters | undefined): string {
	if (!f) return 'none';
	const parts: string[] = [];
	if (f.types?.length) parts.push(`t=${[...f.types].sort().join(',')}`);
	if (f.principal) parts.push(`p=${f.principal.toText()}`);
	if (f.collateral_token) parts.push(`c=${f.collateral_token.toText()}`);
	if (f.time_range) parts.push(`r=${f.time_range.start_ns}-${f.time_range.end_ns}`);
	if (f.min_size_e8s != null) parts.push(`s=${f.min_size_e8s}`);
	if (f.admin_labels?.length) parts.push(`al=${[...f.admin_labels].sort().join(',')}`);
	return parts.length ? parts.join('|') : 'none';
}

function toCandidVariant(v: BackendEventTypeFilter): Record<string, null> {
	return { [v]: null };
}

/**
 * Fetch events with their global indices using `get_events_filtered`.
 * `page` cursors over the *filtered* result set; `pageSize` capped at 200
 * by the backend. `total` reports the matched count across the entire log.
 *
 * Without `filters`, behavior matches the legacy tail-fetch (hides
 * AccrueInterest/PriceUpdate). With `filters`, all fields AND-combined.
 */
export async function fetchEvents(
	page: bigint,
	pageSize: bigint,
	filters?: BackendEventFilters,
): Promise<{ total: bigint; events: [bigint, any][] }> {
	const filterKey = backendFiltersCacheKey(filters);
	const key = `events:filtered:${page}:${pageSize}:${filterKey}`;
	const cached = getCached<{ total: bigint; events: [bigint, any][] }>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const arg: any = { start: page, length: pageSize };
		// Candid `opt T` is `[]` (none) or `[T]` (some) on the JS side.
		arg.types = filters?.types?.length ? [filters.types.map(toCandidVariant)] : [];
		arg.principal = filters?.principal ? [filters.principal] : [];
		arg.collateral_token = filters?.collateral_token ? [filters.collateral_token] : [];
		arg.time_range = filters?.time_range ? [filters.time_range] : [];
		arg.min_size_e8s = filters?.min_size_e8s != null ? [filters.min_size_e8s] : [];
		arg.admin_labels = filters?.admin_labels?.length ? [filters.admin_labels] : [];

		const result = await publicActor.get_events_filtered(arg);
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

// ── Current SP depositors (SP canister as source of truth) ─────────────────

export type CurrentSpDepositor = {
	principal: Principal;
	position: UserStabilityPosition;
};

let _spActor: any = null;

function getStabilityPoolActor() {
	if (_spActor) return _spActor;
	const agent = new HttpAgent({ host: CONFIG.host ?? 'https://icp-api.io' });
	if (CONFIG.isLocal) agent.fetchRootKey().catch(() => {});
	_spActor = Actor.createActor(stabilityPoolIDL as any, {
		agent,
		canisterId: CANISTER_IDS.STABILITY_POOL,
	});
	return _spActor;
}

/**
 * Returns every principal currently holding a non-zero SP balance, sorted by
 * total USD value descending. The SP canister is the source of truth for both
 * the depositor set and each user's position — using `list_depositor_principals`
 * (added in this PR) instead of the analytics shadow log fixes the case where
 * old depositors are missing from `evt_stability` because their Deposit events
 * predate the analytics tailer or were dropped while the shadow types were
 * stale.
 */
export async function fetchCurrentSpDepositors(): Promise<CurrentSpDepositor[]> {
	const key = 'pool:stability:current_depositors';
	const cached = getCached<CurrentSpDepositor[]>(key, TTL.POOL);
	if (cached) return cached;

	try {
		const sp = getStabilityPoolActor();
		const principals: Principal[] = await sp.list_depositor_principals();
		const positions = await Promise.all(
			principals.map(async (p) => {
				try {
					return await sp.get_user_position([p]);
				} catch (err) {
					console.warn('[fetchCurrentSpDepositors] get_user_position failed for', p.toText(), err);
					return [];
				}
			}),
		);
		const out: CurrentSpDepositor[] = [];
		for (let i = 0; i < principals.length; i++) {
			const maybe = positions[i];
			const pos = Array.isArray(maybe) ? maybe[0] : maybe;
			if (pos && Number(pos.total_usd_value_e8s) > 0) {
				out.push({ principal: principals[i], position: pos as UserStabilityPosition });
			}
		}
		out.sort((a, b) => Number(b.position.total_usd_value_e8s) - Number(a.position.total_usd_value_e8s));
		return setCache(key, out);
	} catch (err) {
		console.error('[explorerService] fetchCurrentSpDepositors failed:', err);
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

// ── AMM analytics (per-pool time series + rankings) ────────────────────────
//
// Mirrors the rumi_3pool analytics wrappers so /e/pool/{id} can branch on
// pool source with minimal frontend code. All responses are cached 30s
// client-side; the canister also caches 60s.

export async function fetchAmmVolumeSeries(
	poolId: string,
	window: AmmStatsWindow = 'Week',
	points: number = 100,
): Promise<any[]> {
	const key = `pool:amm:vol:${poolId}:${window}:${points}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getVolumeSeries(poolId, window, points);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmVolumeSeries failed:', err);
		return [];
	}
}

export async function fetchAmmBalanceSeries(
	poolId: string,
	window: AmmStatsWindow = 'Week',
	points: number = 100,
): Promise<any[]> {
	const key = `pool:amm:bal:${poolId}:${window}:${points}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getBalanceSeries(poolId, window, points);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmBalanceSeries failed:', err);
		return [];
	}
}

export async function fetchAmmFeeSeries(
	poolId: string,
	window: AmmStatsWindow = 'Week',
	points: number = 100,
): Promise<any[]> {
	const key = `pool:amm:fee:${poolId}:${window}:${points}`;
	const cached = getCached<any[]>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getFeeSeries(poolId, window, points);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmFeeSeries failed:', err);
		return [];
	}
}

export async function fetchAmmPoolStats(
	poolId: string,
	window: AmmStatsWindow = 'Week',
): Promise<any | null> {
	const key = `pool:amm:stats:${poolId}:${window}`;
	const cached = getCached<any>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getPoolStats(poolId, window);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmPoolStats failed:', err);
		return null;
	}
}

export async function fetchAmmTopSwappers(
	poolId: string,
	window: AmmStatsWindow = 'Week',
	limit: number = 10,
): Promise<Array<[Principal, bigint, bigint]>> {
	const key = `pool:amm:swappers:${poolId}:${window}:${limit}`;
	const cached = getCached<Array<[Principal, bigint, bigint]>>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getTopSwappers(poolId, window, limit);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmTopSwappers failed:', err);
		return [];
	}
}

export async function fetchAmmTopLps(
	poolId: string,
	limit: number = 10,
): Promise<Array<[Principal, bigint, number]>> {
	const key = `pool:amm:lps:${poolId}:${limit}`;
	const cached = getCached<Array<[Principal, bigint, number]>>(key, TTL.POOL);
	if (cached) return cached;
	try {
		const result = await ammService.getTopLps(poolId, limit);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchAmmTopLps failed:', err);
		return [];
	}
}

export async function fetchAmmSwapEventsByPrincipal(
	poolId: string,
	who: Principal,
	start: bigint,
	length: bigint,
): Promise<any[]> {
	try {
		return await ammService.getSwapEventsByPrincipal(poolId, who, start, length);
	} catch (err) {
		console.error('[explorerService] fetchAmmSwapEventsByPrincipal failed:', err);
		return [];
	}
}

export async function fetchAmmLiquidityEventsByPrincipal(
	poolId: string,
	who: Principal,
	start: bigint,
	length: bigint,
): Promise<any[]> {
	try {
		return await ammService.getLiquidityEventsByPrincipal(poolId, who, start, length);
	} catch (err) {
		console.error('[explorerService] fetchAmmLiquidityEventsByPrincipal failed:', err);
		return [];
	}
}

export async function fetchAmmSwapEventsByTimeRange(
	poolId: string,
	startNs: bigint,
	endNs: bigint,
	limit: bigint,
): Promise<any[]> {
	try {
		return await ammService.getSwapEventsByTimeRange(poolId, startNs, endNs, limit);
	} catch (err) {
		console.error('[explorerService] fetchAmmSwapEventsByTimeRange failed:', err);
		return [];
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

// ── Address page wrappers (LP balances, by-principal events, ICRC-1, subaccounts) ─

// TODO: replace these N+1 call sites with rumi_analytics.get_address_summary(p) once it ships.

/** 3pool LP balance for a principal. Result is LP token units (8 decimals). */
export async function fetch3PoolLpBalance(principal: Principal): Promise<bigint> {
	const key = `address:3pool:lp:${principal.toText()}`;
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await threePoolService.getLpBalance(principal);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolLpBalance failed:', err);
		return 0n;
	}
}

/** AMM LP balance for a given pool + principal. */
export async function fetchAmmLpBalance(poolId: string, principal: Principal): Promise<bigint> {
	const key = `address:amm:lp:${poolId}:${principal.toText()}`;
	const cached = getCached<bigint>(key, TTL.POOL);
	if (cached !== null) return cached;

	try {
		const result = await ammService.getLpBalance(poolId, principal);
		return setCache(key, result);
	} catch (err) {
		console.error(`[explorerService] fetchAmmLpBalance(${poolId}) failed:`, err);
		return 0n;
	}
}

/**
 * 3pool swap events touching a principal (as caller or counterparty).
 * Newest-first. offset skips the N most-recent matches, limit takes the next batch.
 */
export async function fetch3PoolSwapEventsByPrincipal(
	principal: Principal,
	offset: bigint = 0n,
	limit: bigint = 500n,
): Promise<any[]> {
	const key = `address:3pool:swaps:${principal.toText()}:${offset}:${limit}`;
	const cached = getCached<any[]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const actor = await (threePoolService as any).getQueryActor();
		const result = await actor.get_swap_events_by_principal(principal, offset, limit) as any[];
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolSwapEventsByPrincipal failed:', err);
		return [];
	}
}

/** 3pool liquidity events (add / remove) touching a principal. Newest-first. */
export async function fetch3PoolLiquidityEventsByPrincipal(
	principal: Principal,
	offset: bigint = 0n,
	limit: bigint = 500n,
): Promise<any[]> {
	const key = `address:3pool:liq:${principal.toText()}:${offset}:${limit}`;
	const cached = getCached<any[]>(key, TTL.EVENTS);
	if (cached) return cached;

	try {
		const actor = await (threePoolService as any).getQueryActor();
		const result = await actor.get_liquidity_events_by_principal(principal, offset, limit) as any[];
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetch3PoolLiquidityEventsByPrincipal failed:', err);
		return [];
	}
}

/**
 * Lazy actor cache for ICRC-1 balance lookups. Any ICRC-1-compliant ledger
 * works because the IDL we reuse (icusd_ledger) declares the standard
 * `icrc1_balance_of : (Account) -> (nat) query` method that all ICRC-1 ledgers expose.
 */
const _icrc1Actors = new Map<string, IcusdLedgerService>();

function getIcrc1Actor(ledgerPrincipal: string): IcusdLedgerService {
	const cached = _icrc1Actors.get(ledgerPrincipal);
	if (cached) return cached;
	const agent = new HttpAgent({ host: CONFIG.host });
	if (CONFIG.isLocal) agent.fetchRootKey().catch(() => {});
	const actor = Actor.createActor<IcusdLedgerService>(CONFIG.icusd_ledgerIDL as any, {
		agent,
		canisterId: ledgerPrincipal,
	});
	_icrc1Actors.set(ledgerPrincipal, actor);
	return actor;
}

/**
 * Query `icrc1_balance_of(owner)` on any ICRC-1 ledger. Subaccount defaults
 * to the caller's default subaccount (empty opt). Returns 0n on error so
 * callers can safely promise-all across multiple ledgers.
 */
export async function fetchIcrc1BalanceOf(
	ledgerPrincipal: string,
	owner: Principal,
): Promise<bigint> {
	const key = `address:balance:${ledgerPrincipal}:${owner.toText()}`;
	const cached = getCached<bigint>(key, TTL.VAULTS);
	if (cached !== null) return cached;

	try {
		const actor = getIcrc1Actor(ledgerPrincipal);
		const result = await actor.icrc1_balance_of({
			owner,
			subaccount: [],
		} as any);
		return setCache(key, result as bigint);
	} catch (err) {
		console.error(`[explorerService] fetchIcrc1BalanceOf(${ledgerPrincipal}) failed:`, err);
		return 0n;
	}
}

/**
 * Query `icrc1_total_supply()` on any ICRC-1 ledger. Used by entity pages
 * that need the live circulating supply for a token whose ledger is not
 * tracked by analytics. Returns null on error so callers can hide the field.
 */
export async function fetchIcrc1TotalSupply(ledgerPrincipal: string): Promise<bigint | null> {
	const key = `token:total_supply:${ledgerPrincipal}`;
	const cached = getCached<bigint>(key, TTL.VAULTS);
	if (cached !== null) return cached;

	try {
		const actor = getIcrc1Actor(ledgerPrincipal);
		const result = await (actor as any).icrc1_total_supply();
		return setCache(key, result as bigint);
	} catch (err) {
		console.error(`[explorerService] fetchIcrc1TotalSupply(${ledgerPrincipal}) failed:`, err);
		return null;
	}
}

/**
 * List every subaccount that has held a balance under `owner` in the icUSD index.
 * `start` is optional pagination — omit for the first page.
 */
export async function fetchIcusdSubaccounts(
	owner: Principal,
	start?: Uint8Array | number[],
): Promise<Array<Uint8Array | number[]>> {
	const key = `address:icusd:subs:${owner.toText()}`;
	const cached = getCached<Array<Uint8Array | number[]>>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const index = getIcusdIndexActor();
		const result = await index.list_subaccounts({
			owner,
			start: start ? [start] : [],
		} as any);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchIcusdSubaccounts failed:', err);
		return [];
	}
}

/** Same as `fetchIcusdSubaccounts` but for the 3USD (3pool LP) index canister. */
export async function fetchThreeUsdSubaccounts(
	owner: Principal,
	start?: Uint8Array | number[],
): Promise<Array<Uint8Array | number[]>> {
	const key = `address:3usd:subs:${owner.toText()}`;
	const cached = getCached<Array<Uint8Array | number[]>>(key, TTL.VAULTS);
	if (cached) return cached;

	try {
		const index = getThreeusdIndexActor();
		const result = await index.list_subaccounts({
			owner,
			start: start ? [start] : [],
		} as any);
		return setCache(key, result);
	} catch (err) {
		console.error('[explorerService] fetchThreeUsdSubaccounts failed:', err);
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

// ── Treasury holdings ────────────────────────────────────────────────────────

export type TreasuryHolding = {
	symbol: string;
	ledger: string;
	balanceE8s: bigint;
	decimals: number;
	usd: number;
};

/** Ledgers we track for the treasury holdings card. */
const TREASURY_TRACKED_LEDGERS: Array<{
	symbol: string;
	principal: string;
	decimals: number;
	/** usdEachE8s === -1 means use live ICP price */
	usdEachE8s: number;
}> = [
	{ symbol: 'icUSD', principal: CANISTER_IDS.ICUSD_LEDGER, decimals: 8, usdEachE8s: 1 },
	{ symbol: 'ckUSDT', principal: CANISTER_IDS.CKUSDT_LEDGER, decimals: 6, usdEachE8s: 1 },
	{ symbol: 'ckUSDC', principal: CANISTER_IDS.CKUSDC_LEDGER, decimals: 6, usdEachE8s: 1 },
	{ symbol: 'ICP', principal: CANISTER_IDS.ICP_LEDGER, decimals: 8, usdEachE8s: -1 },
];

const TREASURY_PRINCIPAL = Principal.fromText(CANISTER_IDS.TREASURY);

/**
 * Query `icrc1_balance_of` on each tracked ledger using the rumi_treasury
 * principal as the account owner. Returns only tokens with a non-zero balance.
 * icpPriceUsd should be the live USD price of ICP (e.g. from protocolStatus.lastIcpRate).
 */
export async function fetchTreasuryHoldings(icpPriceUsd: number): Promise<TreasuryHolding[]> {
	const key = `treasury:holdings:${Math.floor(icpPriceUsd)}`;
	const cached = getCached<TreasuryHolding[]>(key, TTL.TREASURY);
	if (cached) return cached;

	const balances = await Promise.all(
		TREASURY_TRACKED_LEDGERS.map(async (l) => {
			try {
				const balanceE8s = await fetchIcrc1BalanceOf(l.principal, TREASURY_PRINCIPAL);
				const amount = Number(balanceE8s) / Math.pow(10, l.decimals);
				const usd = l.usdEachE8s === -1 ? amount * icpPriceUsd : amount * l.usdEachE8s;
				return { symbol: l.symbol, ledger: l.principal, balanceE8s, decimals: l.decimals, usd };
			} catch (err) {
				console.error(`[explorerService] fetchTreasuryHoldings ${l.symbol} failed:`, err);
				return { symbol: l.symbol, ledger: l.principal, balanceE8s: 0n, decimals: l.decimals, usd: 0 };
			}
		}),
	);

	const result = balances.filter((b) => b.balanceE8s > 0n);
	return setCache(key, result);
}
