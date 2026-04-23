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
	OhlcResponse,
	VolatilityResponse,
	PriceSeriesResponse,
	ThreePoolSeriesResponse,
	CycleSeriesResponse,
	TopHoldersResponse,
	TopCounterpartiesResponse,
	TopSpDepositorsResponse,
	AdminEventBreakdownResponse,
	TokenFlowResponse,
	PoolRoutesResponse,
	AddressValueSeriesResponse,
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

// ── OHLC candlestick data ───────────────────────────────────────────────────

export async function fetchOhlc(
	collateral: Principal,
	bucketSecs?: bigint,
	fromTs?: bigint,
	toTs?: bigint,
	limit?: number
): Promise<OhlcResponse | null> {
	const key = `analytics:ohlc:${collateral.toText()}:${bucketSecs ?? 3600n}:${fromTs ?? 0n}:${toTs ?? 0n}:${limit ?? 500}`;
	const cached = getCached<OhlcResponse>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_ohlc({
			collateral,
			bucket_secs: bucketSecs !== undefined ? [bucketSecs] : [],
			from_ts: fromTs !== undefined ? [fromTs] : [],
			to_ts: toTs !== undefined ? [toTs] : [],
			limit: limit !== undefined ? [limit] : [],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchOhlc failed:', err);
		return null;
	}
}

// ── Volatility ──────────────────────────────────────────────────────────────

export async function fetchVolatility(
	collateral: Principal,
	windowSecs?: bigint
): Promise<VolatilityResponse | null> {
	const key = `analytics:volatility:${collateral.toText()}:${windowSecs ?? 86400n}`;
	const cached = getCached<VolatilityResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_volatility({
			collateral,
			window_secs: windowSecs !== undefined ? [windowSecs] : [],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchVolatility failed:', err);
		return null;
	}
}

// ── Fast price snapshots (5-min) ────────────────────────────────────────────

export async function fetchPriceSeries(limit = 500): Promise<PriceSeriesResponse['rows']> {
	const key = `analytics:price_series:${limit}`;
	const cached = getCached<PriceSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_price_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchPriceSeries failed:', err);
		return [];
	}
}

// ── Cycle balance series (hourly per-canister snapshots) ────────────────────

export async function fetchCycleSeries(limit = 500): Promise<CycleSeriesResponse['rows']> {
	const key = `analytics:cycle_series:${limit}`;
	const cached = getCached<CycleSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_cycle_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchCycleSeries failed:', err);
		return [];
	}
}

// ── Fast 3pool snapshots (5-min) ────────────────────────────────────────────

export async function fetchThreePoolSeries(limit = 500): Promise<ThreePoolSeriesResponse['rows']> {
	const key = `analytics:three_pool_series:${limit}`;
	const cached = getCached<ThreePoolSeriesResponse['rows']>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_three_pool_series(rangeQuery(undefined, undefined, limit));
		return setCache(key, result.rows);
	} catch (err) {
		console.error('[analyticsService] fetchThreePoolSeries failed:', err);
		return [];
	}
}

// ── Top holders ─────────────────────────────────────────────────────────────

const EMPTY_TOP_HOLDERS = (token: Principal): TopHoldersResponse => ({
	token,
	total_holders: 0,
	total_supply_e8s: 0n,
	generated_at_ns: 0n,
	rows: [],
	source: 'unsupported',
});

/**
 * Returns the top holders of a token, ranked by descending balance, with each
 * holder's share of supply in basis points.
 *
 * For tokens not tracked by analytics (collateral assets, etc.) the response
 * has `total_holders: 0` and `source: "unsupported"` — callers should render
 * an empty state instead of breaking. The 60s TTL matches the canister-side
 * cache window so cascading reloads stay cheap.
 */
export async function fetchTopHolders(
	token: Principal,
	limit = 50
): Promise<TopHoldersResponse> {
	const key = `analytics:top_holders:${token.toText()}:${limit}`;
	const cached = getCached<TopHoldersResponse>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_top_holders({
			token,
			limit: [limit],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTopHolders failed:', err);
		return EMPTY_TOP_HOLDERS(token);
	}
}

// ── Top counterparties ──────────────────────────────────────────────────────

const EMPTY_TOP_COUNTERPARTIES = (principal: Principal): TopCounterpartiesResponse => ({
	principal,
	window_ns: 0n,
	generated_at_ns: 0n,
	rows: [],
});

/**
 * Ranks the principals a target address most interacts with across vault
 * events (redeemers, liquidators), swaps (pool principals as counterparties),
 * 3pool liquidity provides, and stability-pool participation.
 *
 * Window defaults to 30 days on the canister side when `windowNs` is omitted.
 * Limit is clamped canister-side to [1, 200] (default 50).
 */
export async function fetchTopCounterparties(
	principal: Principal,
	windowNs?: bigint,
	limit = 10
): Promise<TopCounterpartiesResponse> {
	const key = `analytics:top_counterparties:${principal.toText()}:${windowNs ?? 'default'}:${limit}`;
	const cached = getCached<TopCounterpartiesResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_top_counterparties({
			principal,
			window_ns: windowNs !== undefined ? [windowNs] : [],
			limit: [limit],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTopCounterparties failed:', err);
		return EMPTY_TOP_COUNTERPARTIES(principal);
	}
}

// ── Top stability-pool depositors ──────────────────────────────────────────

const EMPTY_TOP_SP_DEPOSITORS: TopSpDepositorsResponse = {
	window_ns: 0n,
	generated_at_ns: 0n,
	rows: [],
};

/**
 * Ranks stability-pool participants by total deposits inside the window,
 * with all-time net balance surfaced separately so the leaderboard can show
 * both flow and standing position.
 */
export async function fetchTopSpDepositors(
	windowNs?: bigint,
	limit = 20
): Promise<TopSpDepositorsResponse> {
	const key = `analytics:top_sp_depositors:${windowNs ?? 'default'}:${limit}`;
	const cached = getCached<TopSpDepositorsResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_top_sp_depositors({
			window_ns: windowNs !== undefined ? [windowNs] : [],
			limit: [limit],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTopSpDepositors failed:', err);
		return EMPTY_TOP_SP_DEPOSITORS;
	}
}

// ── Admin event breakdown ──────────────────────────────────────────────────

const EMPTY_ADMIN_BREAKDOWN: AdminEventBreakdownResponse = {
	window_ns: 0n,
	generated_at_ns: 0n,
	labels: [],
};

/**
 * Returns per-label counts of admin/setter events within the window so the
 * Explorer activity page can expand its Admin chip into labeled sub-facets.
 * The canister caches these for 5 minutes (admin events are rare).
 */
export async function fetchAdminEventBreakdown(
	windowNs?: bigint
): Promise<AdminEventBreakdownResponse> {
	const key = `analytics:admin_breakdown:${windowNs ?? 'default'}`;
	const cached = getCached<AdminEventBreakdownResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_admin_event_breakdown({
			window_ns: windowNs !== undefined ? [windowNs] : [],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchAdminEventBreakdown failed:', err);
		return EMPTY_ADMIN_BREAKDOWN;
	}
}

// ── Token flow (Sankey edges) ──────────────────────────────────────────────

const EMPTY_TOKEN_FLOW: TokenFlowResponse = {
	window_ns: 0n,
	generated_at_ns: 0n,
	edges: [],
};

/**
 * Ranks token→token edges by USD volume across 3pool + AMM swaps. Drives the
 * Protocol → Overview Sankey and per-token flow strips. Canister caches for
 * 60s per (window, min_volume, limit) tuple.
 */
export async function fetchTokenFlow(
	windowNs?: bigint,
	minVolumeUsdE8s?: bigint,
	limit = 50
): Promise<TokenFlowResponse> {
	const key = `analytics:token_flow:${windowNs ?? 'default'}:${minVolumeUsdE8s ?? '0'}:${limit}`;
	const cached = getCached<TokenFlowResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_token_flow({
			window_ns: windowNs !== undefined ? [windowNs] : [],
			min_volume_usd_e8s: minVolumeUsdE8s !== undefined ? [minVolumeUsdE8s] : [],
			limit: [limit],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchTokenFlow failed:', err);
		return EMPTY_TOKEN_FLOW;
	}
}

// ── Pool routes (single-hop + multi-hop sequences through a pool) ──────────

const EMPTY_POOL_ROUTES = (poolId: string): PoolRoutesResponse => ({
	pool_id: poolId,
	window_ns: 0n,
	generated_at_ns: 0n,
	routes: [],
});

/**
 * Enumerates routes passing through a given pool, ranked by USD volume.
 * `poolId` accepts "3pool", the 3pool canister principal text, or an AMM
 * `principal_lo_principal_hi` pool_id. Routes are ordered token sequences —
 * length-2 for single-hop, length-3 for two-hop reconstructed via the
 * 3pool-liquidity + AMM pairing.
 */
export async function fetchPoolRoutes(
	poolId: string,
	windowNs?: bigint,
	limit = 10
): Promise<PoolRoutesResponse> {
	const key = `analytics:pool_routes:${poolId}:${windowNs ?? 'default'}:${limit}`;
	const cached = getCached<PoolRoutesResponse>(key, TTL.AGGREGATE);
	if (cached) return cached;

	try {
		const result = await getActor().get_pool_routes({
			pool_id: poolId,
			window_ns: windowNs !== undefined ? [windowNs] : [],
			limit: [limit],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchPoolRoutes failed:', err);
		return EMPTY_POOL_ROUTES(poolId);
	}
}

// ── Address value series (portfolio value over time) ───────────────────────

const EMPTY_ADDRESS_VALUE_SERIES = (principal: Principal): AddressValueSeriesResponse => ({
	principal,
	window_ns: 0n,
	resolution_ns: 0n,
	generated_at_ns: 0n,
	points: [],
	approximate_sources: [],
});

/**
 * Portfolio value over time for a given principal, stacked by source.
 *
 * Defaults (canister-side): 90-day window, 1-day resolution. v1 approximates
 * the icUSD/3USD bands with the current ledger balance projected from the
 * `firstseen_ns` timestamp — the UI should surface that via the
 * `approximate_sources` field. Backend caches responses for 5 minutes per
 * (principal, window, resolution) tuple.
 */
export async function fetchAddressValueSeries(
	principal: Principal,
	windowNs?: bigint,
	resolutionNs?: bigint
): Promise<AddressValueSeriesResponse> {
	const key = `analytics:address_value:${principal.toText()}:${windowNs ?? 'default'}:${resolutionNs ?? 'default'}`;
	const cached = getCached<AddressValueSeriesResponse>(key, TTL.SERIES);
	if (cached) return cached;

	try {
		const result = await getActor().get_address_value_series({
			principal,
			window_ns: windowNs !== undefined ? [windowNs] : [],
			resolution_ns: resolutionNs !== undefined ? [resolutionNs] : [],
		});
		return setCache(key, result);
	} catch (err) {
		console.error('[analyticsService] fetchAddressValueSeries failed:', err);
		return EMPTY_ADDRESS_VALUE_SERIES(principal);
	}
}
