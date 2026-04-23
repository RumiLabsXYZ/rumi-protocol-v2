//! Edge-weighted flow aggregation over swap + 3pool liquidity events.
//!
//! Powers two endpoints that share a collector pass:
//! - `get_token_flow` — aggregate per-swap (token_in → token_out) edges
//!   denominated in USD. Drives the Protocol → Overview Sankey.
//! - `get_pool_routes` — for a given pool, enumerate the routes (token
//!   sequences) that pass through it. Single-hop routes surface as length-2
//!   sequences; two-hop routes reconstruct via the 3pool-liquidity + AMM-swap
//!   pairing used on the Activity page (same caller, ±10s timestamp, one leg
//!   involves 3USD).

use candid::Principal;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::{state, storage, types};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Default lookback when callers omit `window_ns`: 30 days. Matches PR #93.
const DEFAULT_WINDOW_NS: u64 = 30 * 86_400 * NANOS_PER_SEC;

/// TTL for cached flow responses. Flow aggregation walks up to tens of
/// thousands of swap + liquidity events and denominates each leg in USD;
/// repeat callers (Sankey re-renders, window flips) deserve to skip that work
/// for a minute.
const FLOW_CACHE_TTL_NS: u64 = 60 * NANOS_PER_SEC;

/// Default and max edge/route counts. Matches the other top-N endpoints so the
/// Explorer can ask the same questions consistently.
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// Safety cap on per-source event loads. Matches PR #93 (MAX_EVENT_LOAD).
const MAX_EVENT_LOAD: usize = 50_000;

/// Max timestamp delta (10 seconds) between a 3pool liquidity event and the
/// AMM swap it pairs with for multi-hop reconstruction. Mirrors the frontend
/// constant in `/explorer/activity/+page.svelte`.
const MULTI_HOP_MAX_GAP_NS: u64 = 10_000_000_000;

/// Literal pool-id alias for the 3pool. Accepted alongside the 3pool canister
/// principal text so callers can use either form.
const THREE_POOL_ALIAS: &str = "3pool";

// ─── Token metadata ────────────────────────────────────────────────────────

/// ckUSDT ledger canister. 6 decimals, $1 stablecoin.
const CKUSDT_LEDGER: &str = "cngnf-vqaaa-aaaar-qag4q-cai";
/// ckUSDC ledger canister. 6 decimals, $1 stablecoin.
const CKUSDC_LEDGER: &str = "xevnm-gaaaa-aaaar-qafnq-cai";

fn ckusdt_principal() -> Principal {
    Principal::from_text(CKUSDT_LEDGER).expect("ckusdt principal parse")
}
fn ckusdc_principal() -> Principal {
    Principal::from_text(CKUSDC_LEDGER).expect("ckusdc principal parse")
}

/// Token decimals lookup. Covers the 3pool + AMM token set. Unknown tokens
/// default to 8, matching `AMM_TOKENS` fallback in `ammService.ts`.
fn token_decimals(token: Principal, icusd_ledger: Principal, three_pool: Principal) -> u8 {
    if token == icusd_ledger || token == three_pool {
        return 8;
    }
    if token == ckusdt_principal() || token == ckusdc_principal() {
        return 6;
    }
    // ICP and the non-3pool collateral set are all 8-decimal on IC today.
    8
}

/// Whether a token is a $1-pegged stablecoin for pricing purposes. 3USD is
/// treated as $1 here since v1 uses spot prices and its virtual price sits
/// within a few bps of 1.
fn is_stablecoin(token: Principal, icusd_ledger: Principal, three_pool: Principal) -> bool {
    token == icusd_ledger
        || token == three_pool
        || token == ckusdt_principal()
        || token == ckusdc_principal()
}

/// Fetch the most recent price snapshot without loading the full log. `None`
/// when the log is empty (fresh canister, no fast-collector pull yet).
fn latest_price_snapshot() -> Option<storage::fast::FastPriceSnapshot> {
    let n = storage::fast::fast_prices::len();
    if n == 0 {
        return None;
    }
    storage::fast::fast_prices::get(n - 1)
}

/// Return the latest USD price for `token` observed in the fast-price log.
/// `None` when the token was never priced (e.g. a newly listed collateral).
fn latest_price_usd(
    token: Principal,
    latest_snapshot: Option<&storage::fast::FastPriceSnapshot>,
) -> Option<f64> {
    let snap = latest_snapshot?;
    for (p, price, _sym) in &snap.prices {
        if *p == token && *price > 0.0 {
            return Some(*price);
        }
    }
    None
}

/// Convert a raw token amount to USD e8s using `price_per_unit` (USD per 1
/// whole token) and the token's native decimal count. Returns 0 when price or
/// decimals push the result out of range (saturating).
fn to_usd_e8s(raw_amount: u64, decimals: u8, price_usd: f64) -> u64 {
    if raw_amount == 0 || price_usd <= 0.0 {
        return 0;
    }
    let price_e8s = (price_usd * 1e8) as u128;
    if price_e8s == 0 {
        return 0;
    }
    let scaled = (raw_amount as u128).saturating_mul(price_e8s);
    let divisor = 10u128.pow(decimals as u32);
    if divisor == 0 {
        return 0;
    }
    (scaled / divisor).min(u64::MAX as u128) as u64
}

/// USD e8s volume for one leg of a swap.
///
/// 3pool swaps come in pre-normalized to 8-decimal e8s (see the tailer); since
/// all three pool assets are $1 stables, `amount_in` is already USD e8s.
///
/// AMM swaps store raw native amounts. We prefer the stablecoin leg when
/// either side is a stable (exact $1, no price lookup). Otherwise we look up
/// the latest spot price for the input token. If that's also unavailable we
/// try the output side as a fallback. v1 approximation matches PR #91's
/// `size_e8s_usd` shape.
pub fn swap_usd_e8s(
    source: &storage::events::SwapSource,
    token_in: Principal,
    token_out: Principal,
    amount_in: u64,
    amount_out: u64,
    icusd_ledger: Principal,
    three_pool: Principal,
    latest_snapshot: Option<&storage::fast::FastPriceSnapshot>,
) -> u64 {
    match source {
        storage::events::SwapSource::ThreePool => {
            // Tailer already normalized to 8-decimal e8s and all 3pool assets
            // are $1 stables.
            amount_in
        }
        storage::events::SwapSource::Amm => {
            let in_stable = is_stablecoin(token_in, icusd_ledger, three_pool);
            let out_stable = is_stablecoin(token_out, icusd_ledger, three_pool);
            if in_stable {
                let d = token_decimals(token_in, icusd_ledger, three_pool);
                return to_usd_e8s(amount_in, d, 1.0);
            }
            if out_stable {
                let d = token_decimals(token_out, icusd_ledger, three_pool);
                return to_usd_e8s(amount_out, d, 1.0);
            }
            if let Some(p) = latest_price_usd(token_in, latest_snapshot) {
                let d = token_decimals(token_in, icusd_ledger, three_pool);
                return to_usd_e8s(amount_in, d, p);
            }
            if let Some(p) = latest_price_usd(token_out, latest_snapshot) {
                let d = token_decimals(token_out, icusd_ledger, three_pool);
                return to_usd_e8s(amount_out, d, p);
            }
            0
        }
    }
}

// ─── Pool identity helpers ────────────────────────────────────────────────

/// Lexicographically-sorted "a_b" pool-id string, matching rumi_amm's
/// `make_pool_id`. Used both to attribute an AMM swap to its pool and to
/// compare against a caller-supplied `pool_id`.
fn amm_pool_id(token_a: Principal, token_b: Principal) -> String {
    let a = token_a.to_text();
    let b = token_b.to_text();
    if a <= b {
        format!("{}_{}", a, b)
    } else {
        format!("{}_{}", b, a)
    }
}

/// Whether `query_pool_id` refers to the 3pool. Accepts the "3pool" alias or
/// the 3pool canister principal text.
fn is_three_pool_query(query_pool_id: &str, three_pool: Principal) -> bool {
    query_pool_id == THREE_POOL_ALIAS || query_pool_id == three_pool.to_text()
}

// ─── Cache ─────────────────────────────────────────────────────────────────

thread_local! {
    static TOKEN_FLOW_CACHE: RefCell<HashMap<(u64, u64, u32), (u64, types::TokenFlowResponse)>> =
        RefCell::new(HashMap::new());
    static POOL_ROUTES_CACHE: RefCell<HashMap<(String, u64, u32), (u64, types::PoolRoutesResponse)>> =
        RefCell::new(HashMap::new());
}

/// Whether a cached entry stamped at `cached_at_ns` is fresh relative to
/// `now_ns` under a `ttl_ns` lifetime. Extracted for unit tests (same shape
/// as `cache_is_fresh` in `queries::live`).
pub fn cache_is_fresh(cached_at_ns: u64, now_ns: u64, ttl_ns: u64) -> bool {
    now_ns.saturating_sub(cached_at_ns) < ttl_ns
}

fn resolve_window_ns(window_ns: Option<u64>) -> u64 {
    match window_ns {
        Some(0) | None => DEFAULT_WINDOW_NS,
        Some(w) => w,
    }
}

fn resolve_limit(limit: Option<u32>) -> u32 {
    let raw = limit.unwrap_or(DEFAULT_LIMIT);
    if raw == 0 { DEFAULT_LIMIT } else { raw }.clamp(1, MAX_LIMIT)
}

// ─── get_token_flow ────────────────────────────────────────────────────────

pub fn get_token_flow(query: types::TokenFlowQuery) -> types::TokenFlowResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let limit = resolve_limit(query.limit);
    let min_volume = query.min_volume_usd_e8s.unwrap_or(0);
    let now = ic_cdk::api::time();
    let cache_key = (window_ns, min_volume, limit);

    if let Some((ts, resp)) = TOKEN_FLOW_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        if cache_is_fresh(ts, now, FLOW_CACHE_TTL_NS) {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);
    let swaps = storage::events::evt_swaps::range(from, now, MAX_EVENT_LOAD);

    // Latest price snapshot for AMM non-stable leg pricing.
    let latest_snap = latest_price_snapshot();
    let (icusd_ledger, three_pool) = state::read_state(|s| (s.sources.icusd_ledger, s.sources.three_pool));

    let edges = compute_token_flow(
        &swaps,
        latest_snap.as_ref(),
        icusd_ledger,
        three_pool,
        min_volume,
        limit as usize,
    );

    let resp = types::TokenFlowResponse {
        window_ns,
        generated_at_ns: now,
        edges,
    };
    TOKEN_FLOW_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, resp.clone()));
    });
    resp
}

/// Aggregate per-edge (token_in, token_out) volume + swap count. Sort by
/// `volume_usd_e8s` descending (count ties broken by count desc, then token
/// principal order for determinism), apply `min_volume` and `limit`.
pub fn compute_token_flow(
    swaps: &[storage::events::AnalyticsSwapEvent],
    latest_snap: Option<&storage::fast::FastPriceSnapshot>,
    icusd_ledger: Principal,
    three_pool: Principal,
    min_volume_usd_e8s: u64,
    limit: usize,
) -> Vec<types::TokenFlowEdge> {
    let mut agg: HashMap<(Principal, Principal), (u64, u64)> = HashMap::new();
    for e in swaps {
        let vol = swap_usd_e8s(
            &e.source,
            e.token_in,
            e.token_out,
            e.amount_in,
            e.amount_out,
            icusd_ledger,
            three_pool,
            latest_snap,
        );
        let entry = agg.entry((e.token_in, e.token_out)).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(vol);
        entry.1 = entry.1.saturating_add(1);
    }

    let mut rows: Vec<types::TokenFlowEdge> = agg
        .into_iter()
        .filter(|(_, (vol, _))| *vol >= min_volume_usd_e8s)
        .map(|((from_token, to_token), (volume_usd_e8s, swap_count))| types::TokenFlowEdge {
            from_token,
            to_token,
            volume_usd_e8s,
            swap_count,
        })
        .collect();

    rows.sort_by(|a, b| {
        b.volume_usd_e8s
            .cmp(&a.volume_usd_e8s)
            .then(b.swap_count.cmp(&a.swap_count))
            .then(a.from_token.to_text().cmp(&b.from_token.to_text()))
            .then(a.to_token.to_text().cmp(&b.to_token.to_text()))
    });
    rows.truncate(limit);
    rows
}

// ─── get_pool_routes ───────────────────────────────────────────────────────

pub fn get_pool_routes(query: types::PoolRoutesQuery) -> types::PoolRoutesResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let limit = resolve_limit(query.limit);
    let now = ic_cdk::api::time();
    let cache_key = (query.pool_id.clone(), window_ns, limit);

    if let Some((ts, resp)) = POOL_ROUTES_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        if cache_is_fresh(ts, now, FLOW_CACHE_TTL_NS) {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);
    let swaps = storage::events::evt_swaps::range(from, now, MAX_EVENT_LOAD);
    let liquidity = storage::events::evt_liquidity::range(from, now, MAX_EVENT_LOAD);

    let latest_snap = latest_price_snapshot();
    let (icusd_ledger, three_pool) = state::read_state(|s| (s.sources.icusd_ledger, s.sources.three_pool));

    let routes = compute_pool_routes(
        &query.pool_id,
        &swaps,
        &liquidity,
        latest_snap.as_ref(),
        icusd_ledger,
        three_pool,
        limit as usize,
    );

    let resp = types::PoolRoutesResponse {
        pool_id: query.pool_id,
        window_ns,
        generated_at_ns: now,
        routes,
    };
    POOL_ROUTES_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, resp.clone()));
    });
    resp
}

/// Reconstruct routes that pass through the queried pool.
///
/// Single-hop: any swap attributable to the pool contributes a length-2
/// `[token_in, token_out]` route. All 3pool swaps are 3pool single-hops; AMM
/// swaps are attributed to the AMM pool whose `make_pool_id(token_in,
/// token_out)` matches.
///
/// Two-hop: iterate 3pool liquidity events (Add or RemoveOneCoin) and pair
/// with an AMM swap by (same caller, |Δts| ≤ 10s, one leg = 3USD). This is
/// the same shape used by the frontend's `mergeMultiHopEvents` in
/// `/explorer/activity/+page.svelte`, kept inline here since it only depends
/// on event rows. The AMM swap leg of the pair is removed from the single-hop
/// set so we don't double-count it.
#[allow(clippy::too_many_arguments)]
pub fn compute_pool_routes(
    pool_id: &str,
    swaps: &[storage::events::AnalyticsSwapEvent],
    liquidity: &[storage::events::AnalyticsLiquidityEvent],
    latest_snap: Option<&storage::fast::FastPriceSnapshot>,
    icusd_ledger: Principal,
    three_pool: Principal,
    limit: usize,
) -> Vec<types::PoolRoute> {
    use storage::events::{LiquidityAction, SwapSource};

    let target_is_three_pool = is_three_pool_query(pool_id, three_pool);

    // Route accumulator keyed by the ordered token sequence.
    let mut agg: HashMap<Vec<Principal>, (u64, u64, u32)> = HashMap::new();
    let mut bump = |route: Vec<Principal>, vol: u64, hops: u32| {
        let entry = agg.entry(route).or_insert((0, 0, hops));
        entry.0 = entry.0.saturating_add(vol);
        entry.1 = entry.1.saturating_add(1);
        entry.2 = hops;
    };

    // --- Two-hop pairing pass ------------------------------------------------
    //
    // Mark AMM swaps that participate in a pair so the single-hop pass below
    // skips them. Keyed by the global swap identity (source_event_id + source)
    // so we don't re-consume the same AMM row on the second matching liquidity
    // event (mirrors the frontend's `mergedSet`).
    // Keyed by source_event_id alone since only AMM swaps can be consumed
    // and `consumed_amm` already guards by source (see the match below).
    let mut consumed_amm: std::collections::HashSet<u64> =
        std::collections::HashSet::new();

    for liq in liquidity {
        let is_add = matches!(liq.action, LiquidityAction::Add);
        let is_remove_one = matches!(liq.action, LiquidityAction::RemoveOneCoin);
        if !is_add && !is_remove_one {
            continue;
        }
        for amm in swaps {
            if amm.source != SwapSource::Amm {
                continue;
            }
            if consumed_amm.contains(&amm.source_event_id) {
                continue;
            }
            if amm.caller != liq.caller {
                continue;
            }
            let gap = amm.timestamp_ns.max(liq.timestamp_ns) - amm.timestamp_ns.min(liq.timestamp_ns);
            if gap > MULTI_HOP_MAX_GAP_NS {
                continue;
            }
            let amm_involves_3usd = amm.token_in == three_pool || amm.token_out == three_pool;
            if !amm_involves_3usd {
                continue;
            }

            // Identify the stablecoin leg on the 3pool side and build the
            // ordered route. For Add: stablecoin is the non-zero slot in
            // `amounts`. For RemoveOneCoin: `coin_index` names the slot.
            let stable_idx = if is_add {
                liq.amounts.iter().position(|&a| a > 0)
            } else {
                liq.coin_index.map(|c| c as usize)
            };
            let Some(stable_idx) = stable_idx else { continue };
            let Some(stable_token) = three_pool_token_at(stable_idx) else { continue };

            let amm_pool_text = amm_pool_id(amm.token_in, amm.token_out);
            let touches_target = target_is_three_pool || amm_pool_text == pool_id;
            if !touches_target {
                continue;
            }

            let route_tokens: Vec<Principal> = if is_add && amm.token_in == three_pool {
                // stablecoin → 3USD → other_token
                vec![stable_token, three_pool, amm.token_out]
            } else if is_remove_one && amm.token_out == three_pool {
                // other_token → 3USD → stablecoin
                vec![amm.token_in, three_pool, stable_token]
            } else {
                // Directional mismatch (e.g. Add paired with 3USD → stable
                // on AMM, which we treat as two independent legs instead).
                continue;
            };

            let vol = swap_usd_e8s(
                &amm.source,
                amm.token_in,
                amm.token_out,
                amm.amount_in,
                amm.amount_out,
                icusd_ledger,
                three_pool,
                latest_snap,
            );

            consumed_amm.insert(amm.source_event_id);
            bump(route_tokens, vol, 2);
            break;
        }
    }

    // --- Single-hop pass -----------------------------------------------------

    for e in swaps {
        let attributed_pool = match e.source {
            SwapSource::ThreePool => THREE_POOL_ALIAS.to_string(),
            SwapSource::Amm => amm_pool_id(e.token_in, e.token_out),
        };
        let touches_target = match e.source {
            SwapSource::ThreePool => target_is_three_pool,
            SwapSource::Amm => attributed_pool == pool_id,
        };
        if !touches_target {
            continue;
        }
        if matches!(e.source, SwapSource::Amm) && consumed_amm.contains(&e.source_event_id) {
            continue;
        }
        let vol = swap_usd_e8s(
            &e.source,
            e.token_in,
            e.token_out,
            e.amount_in,
            e.amount_out,
            icusd_ledger,
            three_pool,
            latest_snap,
        );
        bump(vec![e.token_in, e.token_out], vol, 1);
    }

    let mut rows: Vec<types::PoolRoute> = agg
        .into_iter()
        .map(|(route, (volume_usd_e8s, swap_count, hops))| types::PoolRoute {
            route,
            volume_usd_e8s,
            swap_count,
            avg_hop_count: hops,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.volume_usd_e8s
            .cmp(&a.volume_usd_e8s)
            .then(b.swap_count.cmp(&a.swap_count))
            .then(a.avg_hop_count.cmp(&b.avg_hop_count))
            .then_with(|| {
                a.route
                    .iter()
                    .map(|p| p.to_text())
                    .collect::<Vec<_>>()
                    .cmp(&b.route.iter().map(|p| p.to_text()).collect::<Vec<_>>())
            })
    });
    rows.truncate(limit);
    rows
}

/// Mainnet 3pool token at index `i` (0=icUSD, 1=ckUSDT, 2=ckUSDC). Mirrors the
/// constant used by the 3pool swap tailer.
fn three_pool_token_at(i: usize) -> Option<Principal> {
    const TOKENS: [&str; 3] = [
        "t6bor-paaaa-aaaap-qrd5q-cai",
        "cngnf-vqaaa-aaaar-qag4q-cai",
        "xevnm-gaaaa-aaaar-qafnq-cai",
    ];
    TOKENS.get(i).and_then(|t| Principal::from_text(t).ok())
}

/// Invalidate cached entries. Wired up to swap/liquidity tailers when we want
/// readers to see fresh data sooner than the TTL. Currently only called from
/// tests; the 60s TTL is acceptable for the Sankey's refresh cadence.
#[allow(dead_code)]
pub fn invalidate_caches() {
    TOKEN_FLOW_CACHE.with(|c| c.borrow_mut().clear());
    POOL_ROUTES_CACHE.with(|c| c.borrow_mut().clear());
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::events::{
        AnalyticsLiquidityEvent, AnalyticsSwapEvent, LiquidityAction, SwapSource,
    };

    fn p(s: &str) -> Principal {
        Principal::from_text(s).unwrap()
    }

    // Well-known principals we can reuse across tests. These match the
    // mainnet canister ids referenced throughout the codebase.
    fn icp_ledger() -> Principal { p("ryjl3-tyaaa-aaaaa-aaaba-cai") }
    fn icusd_ledger() -> Principal { p("t6bor-paaaa-aaaap-qrd5q-cai") }
    fn three_pool() -> Principal { p("fohh4-yyaaa-aaaap-qtkpa-cai") }
    fn ckusdt() -> Principal { ckusdt_principal() }
    fn ckusdc() -> Principal { ckusdc_principal() }
    fn actor(n: u8) -> Principal {
        // 29-byte principal seeded by n so we can generate distinct callers.
        let mut bytes = [0u8; 29];
        bytes[0] = n;
        Principal::from_slice(&bytes)
    }

    fn amm_swap(
        id: u64,
        ts: u64,
        caller: Principal,
        tin: Principal,
        tout: Principal,
        ain: u64,
        aout: u64,
    ) -> AnalyticsSwapEvent {
        AnalyticsSwapEvent {
            timestamp_ns: ts,
            source: SwapSource::Amm,
            source_event_id: id,
            caller,
            token_in: tin,
            token_out: tout,
            amount_in: ain,
            amount_out: aout,
            fee: 0,
        }
    }

    fn three_pool_swap(
        id: u64,
        ts: u64,
        caller: Principal,
        tin: Principal,
        tout: Principal,
        ain_e8s: u64,
        aout_e8s: u64,
    ) -> AnalyticsSwapEvent {
        AnalyticsSwapEvent {
            timestamp_ns: ts,
            source: SwapSource::ThreePool,
            source_event_id: id,
            caller,
            token_in: tin,
            token_out: tout,
            amount_in: ain_e8s,
            amount_out: aout_e8s,
            fee: 0,
        }
    }

    fn add_liquidity(
        id: u64,
        ts: u64,
        caller: Principal,
        amounts: Vec<u64>,
        lp_amount: u64,
    ) -> AnalyticsLiquidityEvent {
        AnalyticsLiquidityEvent {
            timestamp_ns: ts,
            source_event_id: id,
            caller,
            action: LiquidityAction::Add,
            amounts,
            lp_amount,
            coin_index: None,
            fee: None,
        }
    }

    fn remove_one_coin(
        id: u64,
        ts: u64,
        caller: Principal,
        amounts: Vec<u64>,
        lp_amount: u64,
        coin_index: u8,
    ) -> AnalyticsLiquidityEvent {
        AnalyticsLiquidityEvent {
            timestamp_ns: ts,
            source_event_id: id,
            caller,
            action: LiquidityAction::RemoveOneCoin,
            amounts,
            lp_amount,
            coin_index: Some(coin_index),
            fee: None,
        }
    }

    fn price_snap(prices: Vec<(Principal, f64)>) -> storage::fast::FastPriceSnapshot {
        storage::fast::FastPriceSnapshot {
            timestamp_ns: 1,
            prices: prices.into_iter().map(|(p, v)| (p, v, String::new())).collect(),
        }
    }

    // ---- USD conversion ----

    #[test]
    fn usd_e8s_three_pool_swap_is_pass_through() {
        // 3pool swap with amount_in already normalized to e8s ($10 swap).
        let e = three_pool_swap(1, 0, actor(1), icusd_ledger(), ckusdt(), 1_000_000_000, 999_000_000);
        let usd = swap_usd_e8s(
            &e.source,
            e.token_in,
            e.token_out,
            e.amount_in,
            e.amount_out,
            icusd_ledger(),
            three_pool(),
            None,
        );
        assert_eq!(usd, 1_000_000_000);
    }

    #[test]
    fn usd_e8s_amm_prefers_stablecoin_leg() {
        // ICP -> ckUSDC (6d). amount_out = 5_000_000 = $5.00 in e8s = 500_000_000.
        let e = amm_swap(1, 0, actor(1), icp_ledger(), ckusdc(), 100_000_000, 5_000_000);
        let usd = swap_usd_e8s(
            &e.source,
            e.token_in,
            e.token_out,
            e.amount_in,
            e.amount_out,
            icusd_ledger(),
            three_pool(),
            Some(&price_snap(vec![(icp_ledger(), 4.99)])),
        );
        assert_eq!(usd, 500_000_000);
    }

    #[test]
    fn usd_e8s_amm_falls_back_to_spot_price_when_no_stable_leg() {
        // Hypothetical ICP -> ckBTC (8d each). Neither is a stable; should
        // price the input leg via the snapshot.
        let ckbtc = actor(100);
        let e = amm_swap(1, 0, actor(1), icp_ledger(), ckbtc, 200_000_000, 42_000);
        let usd = swap_usd_e8s(
            &e.source,
            e.token_in,
            e.token_out,
            e.amount_in,
            e.amount_out,
            icusd_ledger(),
            three_pool(),
            Some(&price_snap(vec![(icp_ledger(), 6.50)])),
        );
        // 2 ICP * $6.50 = $13.00 → 1_300_000_000 e8s.
        assert_eq!(usd, 1_300_000_000);
    }

    // ---- token flow ----

    #[test]
    fn token_flow_aggregates_edges_and_sorts_desc() {
        let snap = price_snap(vec![(icp_ledger(), 5.0)]);
        let swaps = vec![
            // Two icUSD → ckUSDT swaps, $5 + $3 = $8 total, 2 swaps
            three_pool_swap(1, 100, actor(1), icusd_ledger(), ckusdt(), 500_000_000, 499_000_000),
            three_pool_swap(2, 200, actor(2), icusd_ledger(), ckusdt(), 300_000_000, 299_000_000),
            // One ICP → ckUSDC on AMM, 1 ICP * $5 = $5 (via stablecoin leg)
            amm_swap(3, 300, actor(3), icp_ledger(), ckusdc(), 100_000_000, 5_000_000),
        ];
        let edges = compute_token_flow(&swaps, Some(&snap), icusd_ledger(), three_pool(), 0, 10);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].from_token, icusd_ledger());
        assert_eq!(edges[0].to_token, ckusdt());
        assert_eq!(edges[0].volume_usd_e8s, 800_000_000);
        assert_eq!(edges[0].swap_count, 2);
        assert_eq!(edges[1].from_token, icp_ledger());
        assert_eq!(edges[1].to_token, ckusdc());
        assert_eq!(edges[1].volume_usd_e8s, 500_000_000);
        assert_eq!(edges[1].swap_count, 1);
    }

    #[test]
    fn token_flow_respects_min_volume_threshold() {
        let swaps = vec![
            three_pool_swap(1, 100, actor(1), icusd_ledger(), ckusdt(), 1_000_000_000, 999_000_000), // $10
            three_pool_swap(2, 200, actor(2), icusd_ledger(), ckusdc(), 50_000_000, 49_000_000),     // $0.50
        ];
        let edges = compute_token_flow(&swaps, None, icusd_ledger(), three_pool(), 100_000_000, 10);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_token, ckusdt());
    }

    #[test]
    fn token_flow_window_is_handled_by_caller() {
        // compute_token_flow is a pure aggregator; windowing happens at the
        // storage::range layer before the slice reaches us. This test just
        // confirms that an empty slice yields an empty response.
        let edges = compute_token_flow(&[], None, icusd_ledger(), three_pool(), 0, 10);
        assert!(edges.is_empty());
    }

    #[test]
    fn token_flow_truncates_to_limit() {
        let mut swaps = Vec::new();
        for i in 1..=5u64 {
            // Distinct to_tokens so the aggregator produces 5 edges ranked by
            // volume descending.
            swaps.push(three_pool_swap(
                i,
                100 * i,
                actor(1),
                icusd_ledger(),
                if i % 2 == 0 { ckusdt() } else { ckusdc() },
                i * 100_000_000,
                i * 99_000_000,
            ));
        }
        let edges = compute_token_flow(&swaps, None, icusd_ledger(), three_pool(), 0, 2);
        assert_eq!(edges.len(), 2);
    }

    // ---- pool routes ----

    #[test]
    fn pool_routes_three_pool_single_hop() {
        let swaps = vec![
            three_pool_swap(1, 100, actor(1), icusd_ledger(), ckusdt(), 500_000_000, 499_000_000),
            three_pool_swap(2, 200, actor(2), icusd_ledger(), ckusdt(), 300_000_000, 299_000_000),
            three_pool_swap(3, 300, actor(3), ckusdc(), icusd_ledger(), 100_000_000, 99_000_000),
        ];
        let routes = compute_pool_routes(
            "3pool",
            &swaps,
            &[],
            None,
            icusd_ledger(),
            three_pool(),
            10,
        );
        assert_eq!(routes.len(), 2);
        // Top edge: icUSD -> ckUSDT, 2 swaps, $8.
        assert_eq!(routes[0].route, vec![icusd_ledger(), ckusdt()]);
        assert_eq!(routes[0].volume_usd_e8s, 800_000_000);
        assert_eq!(routes[0].swap_count, 2);
        assert_eq!(routes[0].avg_hop_count, 1);
    }

    #[test]
    fn pool_routes_amm_single_hop_only_for_matching_pool_id() {
        // Two AMM swaps across two different pools. Query the second pool;
        // only its swap should surface.
        let swaps = vec![
            amm_swap(1, 100, actor(1), icp_ledger(), ckusdc(), 100_000_000, 5_000_000),
            amm_swap(2, 200, actor(2), icusd_ledger(), ckusdt(), 200_000_000, 199_000_000),
        ];
        let pool_id = amm_pool_id(icusd_ledger(), ckusdt());
        let routes = compute_pool_routes(
            &pool_id,
            &swaps,
            &[],
            None,
            icusd_ledger(),
            three_pool(),
            10,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, vec![icusd_ledger(), ckusdt()]);
        assert_eq!(routes[0].swap_count, 1);
    }

    #[test]
    fn pool_routes_reconstruct_two_hop_via_3pool_add_plus_amm_swap() {
        // User deposits $5 ckUSDT into 3pool (coin index 1), gets 5 * 1e8 3USD,
        // then swaps 3USD → ICP on AMM. Within ±10s gap.
        let user = actor(1);
        let snap = price_snap(vec![(icp_ledger(), 5.0)]);
        let liquidity = vec![add_liquidity(10, 1_000, user, vec![0, 5_000_000, 0], 500_000_000)];
        let swaps = vec![
            amm_swap(20, 1_500, user, three_pool(), icp_ledger(), 500_000_000, 100_000_000),
        ];
        let routes = compute_pool_routes(
            "3pool",
            &swaps,
            &liquidity,
            Some(&snap),
            icusd_ledger(),
            three_pool(),
            10,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, vec![ckusdt(), three_pool(), icp_ledger()]);
        assert_eq!(routes[0].avg_hop_count, 2);
        assert_eq!(routes[0].swap_count, 1);
        // Volume comes from the AMM leg: 500_000_000 3USD = $5 in e8s (stable).
        assert_eq!(routes[0].volume_usd_e8s, 500_000_000);
    }

    #[test]
    fn pool_routes_reconstruct_two_hop_via_amm_swap_plus_3pool_remove_one_coin() {
        // User swaps ICP → 3USD on AMM, then burns 3USD for ckUSDC via
        // RemoveOneCoin.
        let user = actor(2);
        let snap = price_snap(vec![(icp_ledger(), 5.0)]);
        let swaps = vec![
            amm_swap(30, 2_000, user, icp_ledger(), three_pool(), 100_000_000, 500_000_000),
        ];
        let liquidity = vec![remove_one_coin(
            40,
            2_500,
            user,
            vec![0, 0, 5_000_000],
            500_000_000,
            2,
        )];
        let routes = compute_pool_routes(
            "3pool",
            &swaps,
            &liquidity,
            Some(&snap),
            icusd_ledger(),
            three_pool(),
            10,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, vec![icp_ledger(), three_pool(), ckusdc()]);
        assert_eq!(routes[0].avg_hop_count, 2);
    }

    #[test]
    fn pool_routes_skip_pairs_whose_timestamps_diverge_beyond_threshold() {
        let user = actor(3);
        // Liquidity at t=1_000, AMM at t=1_000 + 10s + 1ns → above the cap.
        let liquidity = vec![add_liquidity(10, 1_000, user, vec![5_000_000, 0, 0], 500_000_000)];
        let swaps = vec![
            amm_swap(
                20,
                1_000 + MULTI_HOP_MAX_GAP_NS + 1,
                user,
                three_pool(),
                icp_ledger(),
                500_000_000,
                100_000_000,
            ),
        ];
        let routes = compute_pool_routes(
            "3pool",
            &swaps,
            &liquidity,
            Some(&price_snap(vec![(icp_ledger(), 5.0)])),
            icusd_ledger(),
            three_pool(),
            10,
        );
        // No multi-hop reconstructed → the AMM swap surfaces on its own pool
        // only (ICP/3USD). Since the query was "3pool", nothing matches at
        // the pool level.
        assert!(routes.is_empty());
    }

    #[test]
    fn pool_routes_amm_multihop_surfaces_on_amm_pool_query_too() {
        // The same ICP↔3USD AMM pool query should see the multi-hop route as
        // well (not just the 3pool query). This confirms target matching
        // checks both target_is_three_pool and the amm pool_id.
        let user = actor(4);
        let snap = price_snap(vec![(icp_ledger(), 5.0)]);
        let liquidity = vec![add_liquidity(10, 1_000, user, vec![5_000_000, 0, 0], 500_000_000)];
        let swaps = vec![
            amm_swap(20, 1_500, user, three_pool(), icp_ledger(), 500_000_000, 100_000_000),
        ];
        let amm_pool = amm_pool_id(three_pool(), icp_ledger());
        let routes = compute_pool_routes(
            &amm_pool,
            &swaps,
            &liquidity,
            Some(&snap),
            icusd_ledger(),
            three_pool(),
            10,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, vec![icusd_ledger(), three_pool(), icp_ledger()]);
        assert_eq!(routes[0].avg_hop_count, 2);
    }

    // ---- cache ----

    #[test]
    fn cache_freshness_crosses_ttl() {
        let cached = 1_000_000_000u64;
        let ttl = FLOW_CACHE_TTL_NS;
        assert!(cache_is_fresh(cached, cached, ttl));
        assert!(cache_is_fresh(cached, cached + ttl - 1, ttl));
        assert!(!cache_is_fresh(cached, cached + ttl, ttl));
        assert!(!cache_is_fresh(cached, cached + ttl + 1, ttl));
        // Clock skew: now < cached → treated as fresh via saturating_sub.
        assert!(cache_is_fresh(cached + 5, cached, ttl));
    }
}
