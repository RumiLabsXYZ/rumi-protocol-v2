//! Portfolio value series per address. Reconstructs a principal's total
//! USD-denominated value at regular timestamps across the tracked window by
//! replaying each position type's event history and pricing each position
//! with the historical price observed closest to the sample timestamp.
//!
//! Sources reconstructed historically:
//!   - vault collateral (USD value, priced per collateral_type)
//!   - stability-pool deposit (icUSD principal, $1-pegged)
//!   - 3pool LP balance (virtual-priced from Fast3PoolSnapshot)
//!
//! Sources approximated in v1 (no per-principal event log available):
//!   - icUSD ledger balance (current balance projected from firstseen)
//!   - 3USD ledger balance (same)
//!   - AMM LP positions (no analytics event log for AMM liquidity yet)
//!
//! The `approximate_sources` field in the response flags these so the UI can
//! surface a caveat. Follow-ups: add an ICRC-3 per-delta log and an AMM
//! liquidity tailer to promote those sources to full historical reconstruction.
//!
//! Performance: per-query cost is dominated by loading up to 50k events per
//! source type. The response is cached for 5 minutes per (principal, window,
//! resolution) tuple.

use candid::Principal;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::{state, storage, types};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Default lookback when callers omit `window_ns`: 90 days.
pub const DEFAULT_WINDOW_NS: u64 = 90 * 86_400 * NANOS_PER_SEC;

/// Default sample resolution: 1 day (so a 90-day window yields ~90 points).
pub const DEFAULT_RESOLUTION_NS: u64 = 86_400 * NANOS_PER_SEC;

/// Hard lower bound on resolution. The fast-price collector runs every 5
/// minutes, so points spaced tighter than that repeat the same price anyway.
pub const MIN_RESOLUTION_NS: u64 = 5 * 60 * NANOS_PER_SEC;

/// Absolute cap on points returned in a single response. Keeps the response
/// size bounded regardless of (window, resolution) combination.
pub const MAX_POINTS: usize = 730;

/// TTL for cached responses. This query walks per-source event logs for a
/// specific principal and scans the price log once, so repeat renders within
/// the same viewing session should reuse the result.
pub const CACHE_TTL_NS: u64 = 5 * 60 * NANOS_PER_SEC;

/// Safety cap on per-source event loads. Mirrors PR #93 and PR #96.
const MAX_EVENT_LOAD: usize = 50_000;

/// ICP ledger canister id. The dominant collateral type on mainnet; we special-
/// case nothing here, just document where the price lookup for ICP originates.
#[allow(dead_code)]
const ICP_LEDGER: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";

/// Source identifiers exposed in the response breakdown. The frontend uses
/// these as keys for its colour palette and legend.
pub const SRC_ICUSD: &str = "icusd";
pub const SRC_THREEUSD: &str = "threeusd";
pub const SRC_VAULT_COLLATERAL: &str = "vault_collateral";
pub const SRC_SP_DEPOSIT: &str = "sp_deposit";
pub const SRC_3POOL_LP: &str = "three_pool_lp";

thread_local! {
    static ADDRESS_VALUE_CACHE: RefCell<HashMap<(Principal, u64, u64), (u64, types::AddressValueSeriesResponse)>> =
        RefCell::new(HashMap::new());
}

/// Whether a cached entry stamped at `cached_at_ns` is fresh relative to
/// `now_ns` under a `ttl_ns` lifetime. Shares the shape used elsewhere so cache
/// unit tests stay consistent across modules.
pub fn cache_is_fresh(cached_at_ns: u64, now_ns: u64, ttl_ns: u64) -> bool {
    now_ns.saturating_sub(cached_at_ns) < ttl_ns
}

fn resolve_window_ns(window_ns: Option<u64>) -> u64 {
    match window_ns {
        Some(0) | None => DEFAULT_WINDOW_NS,
        Some(w) => w,
    }
}

fn resolve_resolution_ns(resolution_ns: Option<u64>) -> u64 {
    let raw = match resolution_ns {
        Some(0) | None => DEFAULT_RESOLUTION_NS,
        Some(r) => r,
    };
    raw.max(MIN_RESOLUTION_NS)
}

/// Invalidate cached entries for a given principal. Kept exported for parity
/// with other query modules even though current callers rely on the TTL alone.
#[allow(dead_code)]
pub fn invalidate_cache(principal: Principal) {
    ADDRESS_VALUE_CACHE.with(|c| {
        c.borrow_mut().retain(|(p, _, _), _| *p != principal);
    });
}

// ─── Entry point ────────────────────────────────────────────────────────────

pub fn get_address_value_series(query: types::AddressValueSeriesQuery) -> types::AddressValueSeriesResponse {
    let window_ns = resolve_window_ns(query.window_ns);
    let resolution_ns = resolve_resolution_ns(query.resolution_ns);
    let now = ic_cdk::api::time();
    let cache_key = (query.principal, window_ns, resolution_ns);

    if let Some((ts, resp)) = ADDRESS_VALUE_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        if cache_is_fresh(ts, now, CACHE_TTL_NS) {
            return resp;
        }
    }

    let from = now.saturating_sub(window_ns);

    // Load all event sources once. We load across the full log for vault
    // events because a vault Opened BEFORE the window still contributes
    // collateral during the window — we need that state to reconstruct it.
    // SP and 3pool LP balances work the same way: you can still hold a
    // position you acquired years ago, and events before `from` are needed
    // to reconstruct the running balance.
    let vault_evs = storage::events::evt_vaults::range(0, now, MAX_EVENT_LOAD);
    let liq_evs = storage::events::evt_liquidations::range(0, now, MAX_EVENT_LOAD);
    let sp_evs = storage::events::evt_stability::range(0, now, MAX_EVENT_LOAD);
    let liquidity_evs = storage::events::evt_liquidity::range(0, now, MAX_EVENT_LOAD);
    let price_snaps = storage::fast::fast_prices::range(0, now, MAX_EVENT_LOAD);
    let three_pool_snaps = storage::fast::fast_3pool::range(0, now, MAX_EVENT_LOAD);

    let (icusd_ledger, three_pool) = state::read_state(|s| (s.sources.icusd_ledger, s.sources.three_pool));
    let icusd_balance = storage::balance_tracker::all_balances(storage::balance_tracker::Token::IcUsd)
        .into_iter()
        .filter(|(acct, _)| acct.owner == query.principal)
        .fold(0u64, |acc, (_, bal)| acc.saturating_add(bal));
    let threeusd_balance = storage::balance_tracker::all_balances(storage::balance_tracker::Token::ThreeUsd)
        .into_iter()
        .filter(|(acct, _)| acct.owner == query.principal)
        .fold(0u64, |acc, (_, bal)| acc.saturating_add(bal));
    let icusd_firstseen = storage::balance_tracker::get_firstseen(
        storage::balance_tracker::Token::IcUsd,
        &storage::balance_tracker::Account { owner: query.principal, subaccount: None },
    );
    let threeusd_firstseen = storage::balance_tracker::get_firstseen(
        storage::balance_tracker::Token::ThreeUsd,
        &storage::balance_tracker::Account { owner: query.principal, subaccount: None },
    );

    let points = compute_address_value_series(
        query.principal,
        from,
        now,
        resolution_ns,
        &vault_evs,
        &liq_evs,
        &sp_evs,
        &liquidity_evs,
        &price_snaps,
        &three_pool_snaps,
        icusd_balance,
        icusd_firstseen,
        threeusd_balance,
        threeusd_firstseen,
        icusd_ledger,
        three_pool,
    );

    let approximate_sources = vec![
        SRC_ICUSD.to_string(),
        SRC_THREEUSD.to_string(),
        // AMM LP is absent from the output entirely in v1; no source string
        // surfaces here because the breakdown never contains an "amm_lp"
        // entry. Documented in the module header so the UI can set
        // expectations without reading the backend.
    ];

    let resp = types::AddressValueSeriesResponse {
        principal: query.principal,
        window_ns,
        resolution_ns,
        generated_at_ns: now,
        points,
        approximate_sources,
    };
    ADDRESS_VALUE_CACHE.with(|c| {
        c.borrow_mut().insert(cache_key, (now, resp.clone()));
    });
    resp
}

// ─── Pure computation ──────────────────────────────────────────────────────

/// Sample timestamps from `[from, to]` inclusive, stepping by `resolution_ns`.
/// The last point always lands on `to` so the chart ends at "now" even when
/// resolution doesn't evenly divide the window.
pub fn sample_timestamps(from: u64, to: u64, resolution_ns: u64) -> Vec<u64> {
    if to <= from || resolution_ns == 0 {
        return vec![to];
    }
    let mut points = Vec::new();
    let mut t = from;
    while t < to && points.len() < MAX_POINTS {
        points.push(t);
        t = t.saturating_add(resolution_ns);
    }
    // Ensure the trailing sample is exactly `to`.
    if points.last().copied() != Some(to) {
        if points.len() >= MAX_POINTS {
            // Replace the last slot rather than exceed MAX_POINTS.
            let idx = points.len() - 1;
            points[idx] = to;
        } else {
            points.push(to);
        }
    }
    points
}

#[allow(clippy::too_many_arguments)]
pub fn compute_address_value_series(
    principal: Principal,
    from_ns: u64,
    to_ns: u64,
    resolution_ns: u64,
    vault_events: &[storage::events::AnalyticsVaultEvent],
    liquidation_events: &[storage::events::AnalyticsLiquidationEvent],
    stability_events: &[storage::events::AnalyticsStabilityEvent],
    liquidity_3pool_events: &[storage::events::AnalyticsLiquidityEvent],
    price_snaps: &[storage::fast::FastPriceSnapshot],
    three_pool_snaps: &[storage::fast::Fast3PoolSnapshot],
    icusd_current_balance_e8s: u64,
    icusd_firstseen_ns: Option<u64>,
    threeusd_current_balance_e8s: u64,
    threeusd_firstseen_ns: Option<u64>,
    icusd_ledger: Principal,
    three_pool: Principal,
) -> Vec<types::AddressValuePoint> {
    let timestamps = sample_timestamps(from_ns, to_ns, resolution_ns);

    // Build per-source timelines, each a sorted list of (ts, running_value).
    // At query time we binary-search / linear-scan each timeline for the value
    // at the largest ts <= sample_ts.
    let vault_timeline = build_vault_collateral_timeline(principal, vault_events, liquidation_events);
    let sp_timeline = build_sp_deposit_timeline(principal, stability_events);
    let three_pool_lp_timeline = build_three_pool_lp_timeline(principal, liquidity_3pool_events);

    let mut points = Vec::with_capacity(timestamps.len());
    for ts in timestamps {
        let mut breakdown = Vec::with_capacity(5);
        let mut total: u64 = 0;

        // icUSD ledger balance (approximate: current, from firstseen onward).
        let icusd_value = project_stable_balance(
            ts,
            icusd_current_balance_e8s,
            icusd_firstseen_ns,
        );
        if icusd_value > 0 {
            breakdown.push(types::AddressValueSourceBreakdown {
                source: SRC_ICUSD.to_string(),
                value_usd_e8s: icusd_value,
            });
            total = total.saturating_add(icusd_value);
        }

        // 3USD ledger balance (approximate: current, from firstseen onward).
        // v1 prices 3USD at $1 even though its virtual price floats a few bps
        // above that. Matches the flow aggregator's `is_stablecoin` treatment.
        let threeusd_value = project_stable_balance(
            ts,
            threeusd_current_balance_e8s,
            threeusd_firstseen_ns,
        );
        if threeusd_value > 0 {
            breakdown.push(types::AddressValueSourceBreakdown {
                source: SRC_THREEUSD.to_string(),
                value_usd_e8s: threeusd_value,
            });
            total = total.saturating_add(threeusd_value);
        }

        // Vault collateral (per-token, priced at historical spot).
        let vault_state_at = lookup_timeline_at(&vault_timeline, ts);
        let vault_usd = price_vault_collateral_at(
            vault_state_at,
            ts,
            price_snaps,
        );
        if vault_usd > 0 {
            breakdown.push(types::AddressValueSourceBreakdown {
                source: SRC_VAULT_COLLATERAL.to_string(),
                value_usd_e8s: vault_usd,
            });
            total = total.saturating_add(vault_usd);
        }

        // Stability pool deposit (icUSD principal, $1-pegged).
        let sp_balance = lookup_timeline_scalar_at(&sp_timeline, ts);
        if sp_balance > 0 {
            breakdown.push(types::AddressValueSourceBreakdown {
                source: SRC_SP_DEPOSIT.to_string(),
                value_usd_e8s: sp_balance,
            });
            total = total.saturating_add(sp_balance);
        }

        // 3pool LP balance × historical virtual price.
        let lp_amount = lookup_timeline_scalar_at(&three_pool_lp_timeline, ts);
        if lp_amount > 0 {
            let vp = virtual_price_at(ts, three_pool_snaps);
            let lp_value = apply_virtual_price(lp_amount, vp);
            if lp_value > 0 {
                breakdown.push(types::AddressValueSourceBreakdown {
                    source: SRC_3POOL_LP.to_string(),
                    value_usd_e8s: lp_value,
                });
                total = total.saturating_add(lp_value);
            }
        }

        let _ = icusd_ledger;
        let _ = three_pool;

        points.push(types::AddressValuePoint {
            ts_ns: ts,
            value_usd_e8s: total,
            breakdown,
        });
    }

    points
}

// ─── Vault collateral reconstruction ───────────────────────────────────────

/// Per-token collateral balance held by a principal across every active vault,
/// captured at each event timestamp that changes it. The vault key in the
/// inner map is the `collateral_type` (principal); the value is raw e8s.
pub type VaultCollateralState = HashMap<Principal, u64>;

/// Timeline entries share a full state snapshot to make price application at
/// a sample timestamp cheap (no re-summation per token per point).
pub fn build_vault_collateral_timeline(
    principal: Principal,
    vault_events: &[storage::events::AnalyticsVaultEvent],
    liquidation_events: &[storage::events::AnalyticsLiquidationEvent],
) -> Vec<(u64, VaultCollateralState)> {
    use storage::events::VaultEventKind;

    // vault_id -> (owner, collateral_type, collateral_e8s, is_ours)
    let mut vaults: HashMap<u64, VaultRecord> = HashMap::new();
    // Merge-sort vault + liquidation events by timestamp.
    let mut merged: Vec<MergedVaultEvent> = Vec::new();
    merged.extend(vault_events.iter().map(|e| MergedVaultEvent::Vault(e.clone())));
    merged.extend(liquidation_events.iter().map(|e| MergedVaultEvent::Liquidation(e.clone())));
    merged.sort_by_key(|e| e.timestamp_ns());

    let mut timeline: Vec<(u64, VaultCollateralState)> = Vec::new();
    for e in &merged {
        let mut changed = false;
        match e {
            MergedVaultEvent::Vault(v) => {
                match v.event_kind {
                    VaultEventKind::Opened => {
                        let is_ours = v.owner == principal;
                        vaults.insert(v.vault_id, VaultRecord {
                            owner: v.owner,
                            collateral_type: v.collateral_type,
                            collateral_e8s: if is_ours { v.amount } else { 0 },
                            is_ours,
                        });
                        changed = is_ours;
                    }
                    VaultEventKind::CollateralWithdrawn
                    | VaultEventKind::PartialCollateralWithdrawn
                    | VaultEventKind::WithdrawAndClose => {
                        if let Some(r) = vaults.get_mut(&v.vault_id) {
                            if r.is_ours {
                                r.collateral_e8s = r.collateral_e8s.saturating_sub(v.amount);
                                changed = true;
                            }
                        }
                    }
                    VaultEventKind::Closed => {
                        if let Some(r) = vaults.get_mut(&v.vault_id) {
                            if r.is_ours && r.collateral_e8s > 0 {
                                r.collateral_e8s = 0;
                                changed = true;
                            }
                        }
                    }
                    // Borrowed, Repaid, DustForgiven, Redeemed: no per-vault
                    // collateral-amount delta we can attribute to an owner.
                    // Redeemed events are emitted without a vault_id today
                    // (owner-level summary); see sources::backend for details.
                    _ => {}
                }
            }
            MergedVaultEvent::Liquidation(l) => {
                use storage::events::LiquidationKind;
                if let Some(r) = vaults.get_mut(&l.vault_id) {
                    if r.is_ours {
                        match l.liquidation_kind {
                            LiquidationKind::Full => {
                                if r.collateral_e8s > 0 {
                                    r.collateral_e8s = 0;
                                    changed = true;
                                }
                            }
                            LiquidationKind::Partial => {
                                if r.collateral_e8s > 0 && l.collateral_amount > 0 {
                                    r.collateral_e8s =
                                        r.collateral_e8s.saturating_sub(l.collateral_amount);
                                    changed = true;
                                }
                            }
                            // Redistribution: no owner-facing balance change.
                            LiquidationKind::Redistribution => {}
                        }
                    }
                }
            }
        }
        if changed {
            timeline.push((e.timestamp_ns(), state_snapshot(&vaults, principal)));
        }
    }
    timeline
}

#[derive(Clone, Debug)]
struct VaultRecord {
    #[allow(dead_code)]
    owner: Principal,
    collateral_type: Principal,
    collateral_e8s: u64,
    is_ours: bool,
}

enum MergedVaultEvent {
    Vault(storage::events::AnalyticsVaultEvent),
    Liquidation(storage::events::AnalyticsLiquidationEvent),
}

impl MergedVaultEvent {
    fn timestamp_ns(&self) -> u64 {
        match self {
            MergedVaultEvent::Vault(v) => v.timestamp_ns,
            MergedVaultEvent::Liquidation(l) => l.timestamp_ns,
        }
    }
}

fn state_snapshot(
    vaults: &HashMap<u64, VaultRecord>,
    principal: Principal,
) -> VaultCollateralState {
    let mut out: VaultCollateralState = HashMap::new();
    for r in vaults.values() {
        if r.is_ours && r.collateral_e8s > 0 && r.owner == principal {
            *out.entry(r.collateral_type).or_insert(0) =
                out.get(&r.collateral_type).copied().unwrap_or(0).saturating_add(r.collateral_e8s);
        }
    }
    out
}

/// Linear scan by timestamp descending — cheap for our expected N (a handful
/// of event-triggered snapshots per active vault). For high-volume addresses
/// a binary search would be an easy optimization.
pub fn lookup_timeline_at(
    timeline: &[(u64, VaultCollateralState)],
    ts: u64,
) -> VaultCollateralState {
    let mut last: Option<&VaultCollateralState> = None;
    for (event_ts, state) in timeline {
        if *event_ts > ts {
            break;
        }
        last = Some(state);
    }
    last.cloned().unwrap_or_default()
}

// ─── SP deposits + 3pool LP reconstruction ─────────────────────────────────

/// Scalar timeline: each entry is `(ts, running_balance_e8s)` with balance
/// already aggregated — no per-vault structure needed.
pub fn build_sp_deposit_timeline(
    principal: Principal,
    stability_events: &[storage::events::AnalyticsStabilityEvent],
) -> Vec<(u64, u64)> {
    use storage::events::StabilityAction;

    let mut running: u64 = 0;
    let mut timeline: Vec<(u64, u64)> = Vec::new();
    for e in stability_events {
        if e.caller != principal {
            continue;
        }
        let prev = running;
        match e.action {
            StabilityAction::Deposit => running = running.saturating_add(e.amount),
            StabilityAction::Withdraw => running = running.saturating_sub(e.amount),
            // ClaimReturns pays out collateral gains, leaving the icUSD
            // principal position unchanged. Skip for portfolio-value purposes.
            StabilityAction::ClaimReturns => {}
        }
        if running != prev {
            timeline.push((e.timestamp_ns, running));
        }
    }
    timeline
}

/// 3pool LP holdings reconstructed from Add / Remove / RemoveOneCoin events.
/// Donate events don't mint LP tokens, so they're skipped.
pub fn build_three_pool_lp_timeline(
    principal: Principal,
    liquidity_events: &[storage::events::AnalyticsLiquidityEvent],
) -> Vec<(u64, u64)> {
    use storage::events::LiquidityAction;

    let mut running: u64 = 0;
    let mut timeline: Vec<(u64, u64)> = Vec::new();
    for e in liquidity_events {
        if e.caller != principal {
            continue;
        }
        let prev = running;
        match e.action {
            LiquidityAction::Add => running = running.saturating_add(e.lp_amount),
            LiquidityAction::Remove | LiquidityAction::RemoveOneCoin => {
                running = running.saturating_sub(e.lp_amount)
            }
            LiquidityAction::Donate => {}
        }
        if running != prev {
            timeline.push((e.timestamp_ns, running));
        }
    }
    timeline
}

/// Latest balance at or before `ts` for a scalar timeline. Returns 0 if the
/// timeline is empty or every entry is strictly after `ts`.
pub fn lookup_timeline_scalar_at(timeline: &[(u64, u64)], ts: u64) -> u64 {
    let mut last: u64 = 0;
    for (event_ts, value) in timeline {
        if *event_ts > ts {
            break;
        }
        last = *value;
    }
    last
}

// ─── Pricing helpers ───────────────────────────────────────────────────────

/// Latest USD price for `token` recorded at or before `ts`. Returns `None`
/// when no snapshot covers the range.
pub fn price_usd_at(
    token: Principal,
    ts: u64,
    snapshots: &[storage::fast::FastPriceSnapshot],
) -> Option<f64> {
    let mut latest: Option<f64> = None;
    for snap in snapshots {
        if snap.timestamp_ns > ts {
            break;
        }
        for (p, price, _sym) in &snap.prices {
            if *p == token && *price > 0.0 {
                latest = Some(*price);
            }
        }
    }
    latest
}

/// 3pool virtual_price closest to (≤) `ts`, converted from e18 to a f64
/// multiplier on the LP share value. Defaults to 1.0 when no snapshot covers
/// the range (bootstrap period, or principal's first activity predates any
/// recorded Fast3PoolSnapshot).
pub fn virtual_price_at(
    ts: u64,
    snapshots: &[storage::fast::Fast3PoolSnapshot],
) -> f64 {
    let mut latest: Option<u128> = None;
    for snap in snapshots {
        if snap.timestamp_ns > ts {
            break;
        }
        latest = Some(snap.virtual_price);
    }
    match latest {
        Some(vp) if vp > 0 => (vp as f64) / 1e18,
        _ => 1.0,
    }
}

/// Apply an 18-decimal-ish virtual price multiplier to an 8-decimal LP amount.
/// Result is clamped into u64 to avoid overflow surprises on degenerate inputs.
pub fn apply_virtual_price(lp_amount_e8s: u64, virtual_price: f64) -> u64 {
    if lp_amount_e8s == 0 || virtual_price <= 0.0 {
        return 0;
    }
    let scaled = (lp_amount_e8s as f64) * virtual_price;
    if !scaled.is_finite() || scaled < 0.0 {
        return 0;
    }
    scaled.min(u64::MAX as f64) as u64
}

/// Price every collateral type in `state` at `ts` and sum. Tokens without an
/// in-range price are skipped (their contribution is 0 rather than an error).
pub fn price_vault_collateral_at(
    state: VaultCollateralState,
    ts: u64,
    snapshots: &[storage::fast::FastPriceSnapshot],
) -> u64 {
    let mut total: u64 = 0;
    for (token, amount) in state {
        if amount == 0 { continue; }
        let price = match price_usd_at(token, ts, snapshots) {
            Some(p) => p,
            None => continue,
        };
        // Collateral tokens on IC are 8-decimal across the current set.
        // Other decimal counts would require a per-token lookup; mirroring
        // flow.rs::token_decimals(). Keeping 8 here matches the mainnet set
        // (ICP, BOB, EXE, nICP, ckBTC, ckETH, ckXAUT all use 8).
        let price_e8s = (price * 1e8) as u128;
        let scaled = (amount as u128).saturating_mul(price_e8s);
        let divisor = 10u128.pow(8);
        let contribution = (scaled / divisor).min(u64::MAX as u128) as u64;
        total = total.saturating_add(contribution);
    }
    total
}

/// v1 stable-balance projection: return `current_balance_e8s` for any sample
/// ts at or after `firstseen_ns`, otherwise 0. Used for icUSD and 3USD whose
/// per-principal history isn't yet in a stable log. Documented as an
/// approximation in the response's `approximate_sources` field.
pub fn project_stable_balance(
    ts: u64,
    current_balance_e8s: u64,
    firstseen_ns: Option<u64>,
) -> u64 {
    match firstseen_ns {
        Some(fs) if ts >= fs => current_balance_e8s,
        // No firstseen entry means the balance tracker never saw this
        // principal. That's the case for fresh principals but also for the
        // 5-minute race where balance was applied before firstseen set.
        // Falling back to current balance for ts within 5 minutes of now is
        // too cute; treat as 0 to keep the approximation conservative.
        _ => 0,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::events::{
        AnalyticsLiquidationEvent, AnalyticsLiquidityEvent, AnalyticsStabilityEvent,
        AnalyticsVaultEvent, LiquidationKind, LiquidityAction, StabilityAction, VaultEventKind,
    };
    use crate::storage::fast::{Fast3PoolSnapshot, FastPriceSnapshot};

    fn p(byte: u8) -> Principal {
        Principal::from_slice(&[byte; 29])
    }
    fn icp_ledger() -> Principal { Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap() }
    fn icusd_ledger() -> Principal { Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap() }
    fn three_pool() -> Principal { Principal::from_text("fohh4-yyaaa-aaaap-qtkpa-cai").unwrap() }

    fn vault_event(
        id: u64, ts: u64, owner: Principal, kind: VaultEventKind, coll_type: Principal, amount: u64,
    ) -> AnalyticsVaultEvent {
        AnalyticsVaultEvent {
            timestamp_ns: ts,
            source_event_id: 0,
            vault_id: id,
            owner,
            event_kind: kind,
            collateral_type: coll_type,
            amount,
        }
    }
    fn liq_event(vault_id: u64, ts: u64, kind: LiquidationKind, coll_amount: u64) -> AnalyticsLiquidationEvent {
        AnalyticsLiquidationEvent {
            timestamp_ns: ts,
            source_event_id: 0,
            vault_id,
            collateral_type: Principal::anonymous(),
            collateral_amount: coll_amount,
            debt_amount: 0,
            liquidation_kind: kind,
        }
    }
    fn sp_event(ts: u64, caller: Principal, action: StabilityAction, amount: u64) -> AnalyticsStabilityEvent {
        AnalyticsStabilityEvent {
            timestamp_ns: ts,
            source_event_id: 0,
            caller,
            action,
            amount,
        }
    }
    fn lp_event(ts: u64, caller: Principal, action: LiquidityAction, lp_amount: u64) -> AnalyticsLiquidityEvent {
        AnalyticsLiquidityEvent {
            timestamp_ns: ts,
            source_event_id: 0,
            caller,
            action,
            amounts: vec![],
            lp_amount,
            coin_index: None,
            fee: None,
        }
    }
    fn price_snap(ts: u64, prices: Vec<(Principal, f64)>) -> FastPriceSnapshot {
        FastPriceSnapshot {
            timestamp_ns: ts,
            prices: prices.into_iter().map(|(p, v)| (p, v, String::new())).collect(),
        }
    }
    fn three_pool_snap(ts: u64, virtual_price: u128) -> Fast3PoolSnapshot {
        Fast3PoolSnapshot {
            timestamp_ns: ts,
            balances: vec![],
            virtual_price,
            lp_total_supply: 0,
            decimals: vec![],
        }
    }

    // ─── Sampling ────────────────────────────────────────────────────────

    #[test]
    fn sample_timestamps_stepping() {
        let stamps = sample_timestamps(0, 300, 100);
        // 0, 100, 200, 300 — inclusive of both endpoints.
        assert_eq!(stamps, vec![0, 100, 200, 300]);
    }

    #[test]
    fn sample_timestamps_ragged_window_still_ends_on_to() {
        let stamps = sample_timestamps(0, 250, 100);
        assert_eq!(stamps, vec![0, 100, 200, 250]);
    }

    #[test]
    fn sample_timestamps_empty_window_returns_endpoint() {
        let stamps = sample_timestamps(500, 500, 100);
        assert_eq!(stamps, vec![500]);
    }

    // ─── Vault collateral ─────────────────────────────────────────────────

    #[test]
    fn vault_timeline_tracks_opened_and_withdrawals() {
        let user = p(1);
        let events = vec![
            vault_event(1, 100, user, VaultEventKind::Opened, icp_ledger(), 1_000_000_000),
            vault_event(1, 200, user, VaultEventKind::PartialCollateralWithdrawn, icp_ledger(), 300_000_000),
            vault_event(1, 300, user, VaultEventKind::WithdrawAndClose, icp_ledger(), 700_000_000),
        ];
        let timeline = build_vault_collateral_timeline(user, &events, &[]);
        // Three snapshots — one per changing event.
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline[0].1.get(&icp_ledger()).copied(), Some(1_000_000_000));
        assert_eq!(timeline[1].1.get(&icp_ledger()).copied(), Some(700_000_000));
        assert!(timeline[2].1.get(&icp_ledger()).copied().unwrap_or(0) == 0
            || timeline[2].1.is_empty());
    }

    #[test]
    fn vault_timeline_ignores_other_principals_vaults() {
        let me = p(1);
        let other = p(2);
        let events = vec![
            vault_event(1, 100, other, VaultEventKind::Opened, icp_ledger(), 9_999),
            vault_event(2, 200, me, VaultEventKind::Opened, icp_ledger(), 1_000),
        ];
        let timeline = build_vault_collateral_timeline(me, &events, &[]);
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].0, 200);
        assert_eq!(timeline[0].1.get(&icp_ledger()).copied(), Some(1_000));
    }

    #[test]
    fn vault_timeline_applies_full_liquidation() {
        let me = p(1);
        let vault_evs = vec![
            vault_event(7, 100, me, VaultEventKind::Opened, icp_ledger(), 10_000_000),
        ];
        let liq_evs = vec![
            liq_event(7, 300, LiquidationKind::Full, 0),
        ];
        let timeline = build_vault_collateral_timeline(me, &vault_evs, &liq_evs);
        assert_eq!(timeline.len(), 2);
        // Post-liquidation snapshot: no collateral.
        assert_eq!(timeline[1].1.get(&icp_ledger()).copied().unwrap_or(0), 0);
    }

    // ─── SP deposits ─────────────────────────────────────────────────────

    #[test]
    fn sp_timeline_running_sum_across_deposits_and_withdrawals() {
        let me = p(1);
        let events = vec![
            sp_event(100, me, StabilityAction::Deposit, 1_000),
            sp_event(150, p(2), StabilityAction::Deposit, 500),  // other user, ignored
            sp_event(200, me, StabilityAction::Deposit, 500),
            sp_event(300, me, StabilityAction::Withdraw, 200),
            sp_event(400, me, StabilityAction::ClaimReturns, 10), // no balance change
        ];
        let timeline = build_sp_deposit_timeline(me, &events);
        assert_eq!(timeline, vec![(100, 1_000), (200, 1_500), (300, 1_300)]);
    }

    // ─── 3pool LP ────────────────────────────────────────────────────────

    #[test]
    fn three_pool_lp_timeline_handles_add_and_remove() {
        let me = p(1);
        let events = vec![
            lp_event(100, me, LiquidityAction::Add, 1_000),
            lp_event(200, me, LiquidityAction::RemoveOneCoin, 300),
            lp_event(300, me, LiquidityAction::Remove, 500),
            lp_event(400, me, LiquidityAction::Donate, 999),  // no LP minted
        ];
        let timeline = build_three_pool_lp_timeline(me, &events);
        assert_eq!(timeline, vec![(100, 1_000), (200, 700), (300, 200)]);
    }

    // ─── Historical pricing ──────────────────────────────────────────────

    #[test]
    fn price_usd_at_picks_latest_at_or_before_ts() {
        let snaps = vec![
            price_snap(100, vec![(icp_ledger(), 5.0)]),
            price_snap(200, vec![(icp_ledger(), 6.0)]),
            price_snap(300, vec![(icp_ledger(), 7.0)]),
        ];
        assert_eq!(price_usd_at(icp_ledger(), 50, &snaps), None);
        assert_eq!(price_usd_at(icp_ledger(), 150, &snaps), Some(5.0));
        assert_eq!(price_usd_at(icp_ledger(), 200, &snaps), Some(6.0));
        assert_eq!(price_usd_at(icp_ledger(), 500, &snaps), Some(7.0));
    }

    #[test]
    fn vault_collateral_value_moves_with_price_even_when_balance_is_constant() {
        let me = p(1);
        let vault_evs = vec![
            vault_event(1, 100, me, VaultEventKind::Opened, icp_ledger(), 1_000_000_000),
        ];
        let snaps = vec![
            price_snap(100, vec![(icp_ledger(), 5.0)]),
            price_snap(2_000, vec![(icp_ledger(), 10.0)]),
        ];
        let timeline = build_vault_collateral_timeline(me, &vault_evs, &[]);
        let state_at_t1 = lookup_timeline_at(&timeline, 500);
        let state_at_t2 = lookup_timeline_at(&timeline, 2_500);

        let v1 = price_vault_collateral_at(state_at_t1, 500, &snaps);
        let v2 = price_vault_collateral_at(state_at_t2, 2_500, &snaps);

        // 10 ICP * $5 = $50; 10 ICP * $10 = $100. Values are e8s.
        assert_eq!(v1, 5_000_000_000);
        assert_eq!(v2, 10_000_000_000);
    }

    // ─── Series composition ─────────────────────────────────────────────

    #[test]
    fn full_series_icusd_only_holder() {
        // Stable-balance holder with $100 icUSD, firstseen at t=50.
        let me = p(1);
        let points = compute_address_value_series(
            me,
            0, 400, 100,
            &[], &[], &[], &[],
            &[],
            &[],
            100_000_000_000u64, // $100 in e8s
            Some(50),
            0, None,
            icusd_ledger(), three_pool(),
        );
        // Points at t=0, 100, 200, 300, 400.
        assert_eq!(points.len(), 5);
        // At t=0 (before firstseen), balance is 0.
        assert_eq!(points[0].value_usd_e8s, 0);
        assert!(points[0].breakdown.is_empty());
        // At t=100 and later, full balance registered.
        for p in &points[1..] {
            assert_eq!(p.value_usd_e8s, 100_000_000_000);
            assert_eq!(p.breakdown.len(), 1);
            assert_eq!(p.breakdown[0].source, SRC_ICUSD);
        }
    }

    #[test]
    fn full_series_multi_source_principal() {
        // User opens a vault at t=100 with 10 ICP, deposits 50 icUSD to SP at
        // t=200, and provides 3pool LP for 20 tokens at t=300. Check that
        // each timestamp's breakdown carries all three sources in the right
        // amounts.
        let me = p(1);
        let vault_evs = vec![
            vault_event(1, 100, me, VaultEventKind::Opened, icp_ledger(), 1_000_000_000),
        ];
        let sp_evs = vec![
            sp_event(200, me, StabilityAction::Deposit, 50_00_000_000),
        ];
        let lp_evs = vec![
            lp_event(300, me, LiquidityAction::Add, 20_00_000_000),
        ];
        let prices = vec![price_snap(100, vec![(icp_ledger(), 5.0)])];
        let tp_snaps = vec![three_pool_snap(300, 1_020_000_000_000_000_000)]; // vp = 1.02

        let points = compute_address_value_series(
            me,
            0, 400, 100,
            &vault_evs, &[], &sp_evs, &lp_evs,
            &prices, &tp_snaps,
            0, None,
            0, None,
            icusd_ledger(), three_pool(),
        );
        // t=0: nothing yet.
        assert_eq!(points[0].value_usd_e8s, 0);
        // t=100: only vault collateral. 10 ICP * $5 = $50 = 5_000_000_000 e8s.
        let t100 = &points[1];
        assert_eq!(t100.breakdown.len(), 1);
        assert_eq!(t100.breakdown[0].source, SRC_VAULT_COLLATERAL);
        assert_eq!(t100.value_usd_e8s, 5_000_000_000);
        // t=200: vault + SP. 50 e8s icUSD == $50.
        let t200 = &points[2];
        assert_eq!(t200.breakdown.len(), 2);
        assert_eq!(t200.value_usd_e8s, 5_000_000_000 + 50_00_000_000);
        // t=300: vault + SP + LP. LP = 20e8 * 1.02 ≈ 20.4e8.
        let t300 = &points[3];
        assert_eq!(t300.breakdown.len(), 3);
        let lp_src = t300.breakdown.iter().find(|b| b.source == SRC_3POOL_LP).unwrap();
        // 20_00_000_000 * 1.02 = 20_40_000_000.
        assert!((lp_src.value_usd_e8s as i64 - 20_40_000_000).abs() <= 1);
    }

    #[test]
    fn full_series_vault_only_address_with_price_drift() {
        // Stable collateral balance (10 ICP opened at t=100), but price rises
        // from $5 at t=100 to $10 by t=300. Portfolio value must rise even
        // though no balance events occurred after t=100.
        let me = p(1);
        let vault_evs = vec![
            vault_event(1, 100, me, VaultEventKind::Opened, icp_ledger(), 1_000_000_000),
        ];
        let prices = vec![
            price_snap(100, vec![(icp_ledger(), 5.0)]),
            price_snap(300, vec![(icp_ledger(), 10.0)]),
        ];
        let points = compute_address_value_series(
            me,
            0, 400, 100,
            &vault_evs, &[], &[], &[],
            &prices, &[],
            0, None, 0, None,
            icusd_ledger(), three_pool(),
        );
        assert_eq!(points.len(), 5);
        assert_eq!(points[1].value_usd_e8s, 5_000_000_000);  // $50 @ t=100
        assert_eq!(points[2].value_usd_e8s, 5_000_000_000);  // still $5 @ t=200
        assert_eq!(points[3].value_usd_e8s, 10_000_000_000); // $100 @ t=300
        assert_eq!(points[4].value_usd_e8s, 10_000_000_000); // still $10 @ t=400
    }

    #[test]
    fn full_series_inactive_principal_yields_zero_points() {
        let me = p(1);
        let points = compute_address_value_series(
            me,
            0, 200, 100,
            &[], &[], &[], &[],
            &[], &[],
            0, None, 0, None,
            icusd_ledger(), three_pool(),
        );
        for p in &points {
            assert_eq!(p.value_usd_e8s, 0);
            assert!(p.breakdown.is_empty());
        }
    }

    // ─── Cache ────────────────────────────────────────────────────────────

    #[test]
    fn cache_freshness_crosses_ttl_boundary() {
        let cached = 1_000_000_000u64;
        assert!(cache_is_fresh(cached, cached, CACHE_TTL_NS));
        assert!(cache_is_fresh(cached, cached + CACHE_TTL_NS - 1, CACHE_TTL_NS));
        assert!(!cache_is_fresh(cached, cached + CACHE_TTL_NS, CACHE_TTL_NS));
        assert!(!cache_is_fresh(cached, cached + CACHE_TTL_NS + 1, CACHE_TTL_NS));
        // Defensive: skew with now < cached → saturating_sub yields 0, treated fresh.
        assert!(cache_is_fresh(cached + 1_000, cached, CACHE_TTL_NS));
    }
}
