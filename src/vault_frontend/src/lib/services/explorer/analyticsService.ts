/**
 * analyticsService.ts — Typed fetch functions for the rumi_analytics canister.
 *
 * Uses TTL-based Map caching to avoid redundant canister calls within a
 * short window. All calls are anonymous (no wallet required).
 */

import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import type { _SERVICE as AnalyticsService } from '$declarations/rumi_analytics/rumi_analytics.did';
import type {
	ProtocolSummary,
	TvlSeriesResponse,
	VaultSeriesResponse,
	StabilitySeriesResponse,
	SwapSeriesResponse,
	FeeSeriesResponse,
	LiquidationSeriesResponse,
	HolderSeriesResponse,
	TwapResponse,
	ApyResponse,
	PegStatus,
	TradeActivityResponse,
	CollectorHealth,
} from '$declarations/rumi_analytics/rumi_analytics.did';

// ── TTL constants (ms) ───────────────────────────────────────────────────────

const TTL = {
	SUMMARY: 15_000,
	SERIES: 60_000,
	AGGREGATE: 30_000,
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
export function invalidateAnalyticsCache(prefix?: string): void {
	if (!prefix) {
		cache.clear();
		return;
	}
	for (const key of cache.keys()) {
		if (key.startsWith(prefix)) cache.delete(key);
	}
}

// ── Actor (lazy, anonymous) ──────────────────────────────────────────────────

let _actor: AnalyticsService | null = null;

function getActor(): AnalyticsService {
	if (_actor) return _actor;

	const agent = new HttpAgent({ host: 'https://icp-api.io' });

	_actor = Actor.createActor<AnalyticsService>(CONFIG.analyticsIDL, {
		agent,
		canisterId: CANISTER_IDS.ANALYTICS,
	});

	return _actor;
}

// ── RangeQuery helper ────────────────────────────────────────────────────────

function rangeQuery(from?: bigint, to?: bigint, limit?: number) {
	return {
		from_ts: from !== undefined ? [from] : [],
		to_ts: to !== undefined ? [to] : [],
		limit: limit !== undefined ? [limit] : [],
		offset: [] as [],
	};
}

// ── Protocol summary ─────────────────────────────────────────────────────────

export async function fetchProtocolSummary(): Promise<ProtocolSummary | null> {
	const key = 'analytics:summary';
	const cached = getCached<ProtocolSummary>(key, TTL.SUMMARY);
	if (cached) return cached;

	try {
		const result = await getActor().get_protocol_summary();
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchProtocolSummary failed:', err);
		return null;
	}
}

// ── Time-series fetchers ─────────────────────────────────────────────────────

export async function fetchTvlSeries(limit = 365): Promise<TvlSeriesResponse['rows']> {
	const key = `analytics:tvl:${limit}`;
	const cached = getCached<TvlSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_tvl_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchTvlSeries failed:', err);
		return [];
	}
}

export async function fetchVaultSeries(limit = 365): Promise<VaultSeriesResponse['rows']> {
	const key = `analytics:vault:${limit}`;
	const cached = getCached<VaultSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_vault_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchVaultSeries failed:', err);
		return [];
	}
}

export async function fetchStabilitySeries(limit = 365): Promise<StabilitySeriesResponse['rows']> {
	const key = `analytics:stability:${limit}`;
	const cached = getCached<StabilitySeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_stability_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchStabilitySeries failed:', err);
		return [];
	}
}

export async function fetchSwapSeries(limit = 365): Promise<SwapSeriesResponse['rows']> {
	const key = `analytics:swap:${limit}`;
	const cached = getCached<SwapSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_swap_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchSwapSeries failed:', err);
		return [];
	}
}

export async function fetchFeeSeries(limit = 365): Promise<FeeSeriesResponse['rows']> {
	const key = `analytics:fee:${limit}`;
	const cached = getCached<FeeSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_fee_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchFeeSeries failed:', err);
		return [];
	}
}

export async function fetchLiquidationSeries(
	limit = 365
): Promise<LiquidationSeriesResponse['rows']> {
	const key = `analytics:liquidation:${limit}`;
	const cached = getCached<LiquidationSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_liquidation_series(
			rangeQuery(undefined, undefined, limit)
		);
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchLiquidationSeries failed:', err);
		return [];
	}
}

export async function fetchHolderSeries(
	token: Principal,
	limit = 365
): Promise<HolderSeriesResponse['rows']> {
	const key = `analytics:holder:${token.toText()}:${limit}`;
	const cached = getCached<HolderSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_holder_series(
			rangeQuery(undefined, undefined, limit),
			token
		);
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchHolderSeries failed:', err);
		return [];
	}
}

// ── Aggregate / derived metrics ───────────────────────────────────────────────

export async function fetchTwap(windowSecs = 3600): Promise<TwapResponse | null> {
	const key = `analytics:twap:${windowSecs}`;
	const cached = getCached<TwapResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_twap({ window_secs: [BigInt(windowSecs)] });
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTwap failed:', err);
		return null;
	}
}

export async function fetchApys(windowDays = 7): Promise<ApyResponse | null> {
	const key = `analytics:apys:${windowDays}`;
	const cached = getCached<ApyResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_apys({ window_days: [windowDays] });
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchApys failed:', err);
		return null;
	}
}

export async function fetchPegStatus(): Promise<PegStatus | null> {
	const key = 'analytics:peg';
	const cached = getCached<PegStatus>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_peg_status();
		const value = result.length > 0 ? result[0] : null;
		if (value) setCache(key, value);
		return value ?? null;
	} catch (err) {
		console.error('[analyticsService] fetchPegStatus failed:', err);
		return null;
	}
}

export async function fetchTradeActivity(windowSecs = 86400): Promise<TradeActivityResponse | null> {
	const key = `analytics:trade_activity:${windowSecs}`;
	const cached = getCached<TradeActivityResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_trade_activity({ window_secs: [BigInt(windowSecs)] });
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTradeActivity failed:', err);
		return null;
	}
}

export async function fetchCollectorHealth(): Promise<CollectorHealth | null> {
	const key = 'analytics:collector_health';
	const cached = getCached<CollectorHealth>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_collector_health();
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchCollectorHealth failed:', err);
		return null;
	}
}
