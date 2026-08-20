use crate::event::Event;
use crate::logs::{INFO, TRACE_XRC};
use crate::numeric::UsdIcp;
use crate::state::{mutate_state, read_state, CollateralStatus, State};
use crate::Decimal;
use crate::Mode;
use candid::Principal;
use ic_canister_log::log;
use ic_xrc_types::{Asset, AssetClass, GetExchangeRateRequest, GetExchangeRateResult};
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal_macros::dec;
use std::time::Duration;

/// Wave-14a CDP-14: minimum number of CEX sources that must contribute to
/// an XRC `metadata.num_sources_used` for the protocol to accept the
/// resulting price. A single-source aggregation is cheaper to manipulate
/// than one drawn from multiple venues; the Wave-5 sanity band catches
/// implausible *values* but not the *thinness* of the underlying
/// aggregation.
pub const MIN_XRC_SOURCES: u32 = 3;

/// Wave-14a CDP-14: pure helper. Returns true iff the sample's source
/// count meets the protocol-configured floor. `min_required == 0` is a
/// kill switch that disables the gate entirely (operator setting if XRC
/// aggregation degrades industry-wide).
pub fn xrc_metadata_meets_source_floor(num_sources_used: u32, min_required: u32) -> bool {
    if min_required == 0 {
        return true;
    }
    num_sources_used >= min_required
}

/// Wave-14a CDP-01: maximum number of consecutive XRC fetch failures the
/// protocol will tolerate before falling back to ReadOnly. The 300-second
/// poll cadence and the 10-minute hard staleness gate cover the slow-
/// degradation case; this counter trips on the fast-degradation case
/// where multiple consecutive ticks fail in close succession (cycle
/// pressure, XRC outage, network partition).
pub const MAX_CONSECUTIVE_XRC_FAILURES: u64 = 3;

/// Wave-14a CDP-01: record an XRC fetch failure. Increments the
/// consecutive-failure counter; if it reaches the threshold and the
/// protocol is still in `GeneralAvailability`, switches to `ReadOnly`,
/// marks the trip as oracle-triggered (so a later success can auto-clear
/// it), and returns the `OracleCircuitBreaker` event for the caller to
/// persist.
pub fn note_xrc_failure(state: &mut State) -> Option<Event> {
    note_xrc_failure_at(state, ic_cdk::api::time())
}

/// Pure-state variant taking an explicit `now_ns` for tests. Production
/// callers go through `note_xrc_failure`.
pub fn note_xrc_failure_at(state: &mut State, now_ns: u64) -> Option<Event> {
    state.consecutive_xrc_failures = state.consecutive_xrc_failures.saturating_add(1);

    if state.consecutive_xrc_failures < MAX_CONSECUTIVE_XRC_FAILURES {
        return None;
    }

    if state.mode != Mode::GeneralAvailability {
        // Already ReadOnly (operator-set or oracle-set). Don't emit an
        // event each subsequent failure — the trip is already in effect.
        return None;
    }

    state.mode = Mode::ReadOnly;
    state.mode_triggered_by_oracle = true;
    Some(Event::OracleCircuitBreaker {
        consecutive_failures: state.consecutive_xrc_failures,
        timestamp: now_ns,
    })
}

/// Wave-14a CDP-01: record an XRC fetch success. Resets the consecutive-
/// failure counter to 0. If ReadOnly was triggered by the oracle path,
/// clears it back to `GeneralAvailability`. Operator-set ReadOnly is
/// preserved.
pub fn note_xrc_success(state: &mut State) {
    state.consecutive_xrc_failures = 0;

    if state.mode == Mode::ReadOnly && state.mode_triggered_by_oracle {
        state.mode = Mode::GeneralAvailability;
        state.mode_triggered_by_oracle = false;
    }
}

/// Wave-9d DOS-011: classifies whether a collateral type's periodic
/// background XRC price refresh is still useful given its lifecycle
/// status. Returns true for `Active`, `Paused`, and `Sunset`: all three can
/// still liquidate outstanding debt and therefore require a current price.
/// `Frozen` and `Deprecated` have no remaining price-consuming operation.
///
/// Used by the per-collateral 300s timer closures registered in
/// `setup_timers()` and `add_collateral_token` so fully disabled collateral
/// no longer burns ~1B cycles per tick on a useless XRC call.
pub fn collateral_needs_periodic_price_refresh(status: CollateralStatus) -> bool {
    match status {
        CollateralStatus::Active | CollateralStatus::Paused | CollateralStatus::Sunset => true,
        CollateralStatus::Frozen | CollateralStatus::Deprecated => false,
    }
}

/// Wave-9d DOS-011: pure-state gate used by the per-collateral price
/// timer closure. Returns true if the closure should call XRC for this
/// collateral, false to early-return without burning the ~1B cycles of
/// an XRC fetch. Exposed as a module function so unit tests can pin the
/// composition (status lookup + classification) without spawning a
/// canister. Returns false for unknown collateral so a stale closure
/// (collateral removed entirely from `collateral_configs`) doesn't keep
/// hitting XRC for a deleted ledger.
pub fn should_fetch_collateral_price(state: &crate::state::State, ledger_id: &Principal) -> bool {
    match state.get_collateral_status(ledger_id) {
        Some(CollateralStatus::Sunset) => !state.is_retired_sunset_collateral(ledger_id),
        Some(status) => collateral_needs_periodic_price_refresh(status),
        None => false,
    }
}

/// Spawn a collateral XRC fetch only while its lifecycle still consumes a
/// price. Both immediate post-upgrade refreshes and recurring timers use this
/// boundary so fully retired Sunset collateral cannot bypass the cycle gate.
pub fn spawn_collateral_price_fetch_if_needed(ledger_id: Principal) {
    let should_fetch = read_state(|state| should_fetch_collateral_price(state, &ledger_id));
    if should_fetch {
        ic_cdk::spawn(crate::management::fetch_collateral_price(ledger_id));
    } else {
        log!(
            TRACE_XRC,
            "[collateral_price_fetch] skipping {} (fully retired, disabled, or removed)",
            ledger_id
        );
    }
}

thread_local! {
    /// 2026-07-03 cycle-burn optimization: the `TimerId` of the background
    /// price-refresh timer for each collateral, so
    /// `set_collateral_price_fetch_interval_secs` can clear + re-register a
    /// single collateral's timer in place (mirroring the ICP / settlement /
    /// observer timers in `main`). NOT persisted: timers never survive an
    /// upgrade, and `setup_timers` re-registers each collateral's timer from
    /// `State::collateral_price_fetch_interval_secs` on every start.
    static COLLATERAL_PRICE_TIMER_IDS: std::cell::RefCell<
        std::collections::BTreeMap<Principal, ic_cdk_timers::TimerId>,
    > = std::cell::RefCell::new(std::collections::BTreeMap::new());
}

/// 2026-07-03: fallback background price-fetch cadence (seconds) for a
/// collateral with no entry in `State::collateral_price_fetch_interval_secs`.
/// Equal to the historical hardcoded 300s, so an unconfigured collateral
/// behaves exactly as it did before per-collateral cadences existed.
pub const DEFAULT_COLLATERAL_PRICE_FETCH_SECS: u64 = 300;

/// 2026-07-03: resolve the effective background price-fetch cadence for a
/// collateral. Returns the per-collateral override when set, else the 300s
/// default. A stored 0 (should never happen — the setter rejects it) or any
/// value below 60s is floored to protect against a busy-loop timer.
pub fn collateral_price_fetch_secs(state: &State, ledger_id: &Principal) -> u64 {
    match state
        .collateral_price_fetch_interval_secs
        .get(ledger_id)
        .copied()
    {
        None | Some(0) => DEFAULT_COLLATERAL_PRICE_FETCH_SECS,
        Some(secs) => secs.max(60),
    }
}

/// Wave-9d DOS-011: registers the recurring per-collateral XRC price
/// timer with the status-check gate baked in. Used by both
/// `setup_timers()` (re-registering after upgrade) and
/// `add_collateral_token` (registering for a brand-new collateral).
///
/// 2026-07-03: the interval is now per-collateral (resolved via
/// `collateral_price_fetch_secs`, default 300s) rather than a fixed
/// `FETCHING_ICP_RATE_INTERVAL`, and the timer id is tracked in
/// `COLLATERAL_PRICE_TIMER_IDS` so a re-register (setter, or repeated
/// `setup_timers`) clears the prior timer first and never leaks a duplicate
/// interval timer for the same collateral.
///
/// The timer keeps firing on its cadence regardless of status — that's free.
/// The gate runs synchronously INSIDE the closure (before any `ic_cdk::spawn`):
/// if the collateral is wound down, we early-return before allocating an async
/// task and before the (~1B cycle) XRC call.
pub fn register_collateral_price_timer(ledger_id: Principal) {
    // Clear any prior timer for this collateral so a re-register never leaks a
    // second interval timer (which would double this collateral's fetch burn).
    COLLATERAL_PRICE_TIMER_IDS.with(|cell| {
        if let Some(old) = cell.borrow_mut().remove(&ledger_id) {
            ic_cdk_timers::clear_timer(old);
        }
    });
    let secs = read_state(|s| collateral_price_fetch_secs(s, &ledger_id));
    let new_id = ic_cdk_timers::set_timer_interval(Duration::from_secs(secs), move || {
        spawn_collateral_price_fetch_if_needed(ledger_id);
    });
    COLLATERAL_PRICE_TIMER_IDS.with(|cell| {
        cell.borrow_mut().insert(ledger_id, new_id);
    });
}

/// How often to passively fetch ICP price from XRC (background polling).
/// Each XRC call costs ~1B cycles. At 60s = ~$58/month, at 300s = ~$12/month.
/// Price-sensitive operations will fetch on-demand if the cached price is older
/// than `PRICE_FRESHNESS_THRESHOLD_NANOS` (60s as of Wave-5 F-004), so this
/// timer is just a lazy background refresh for display/query purposes.
pub const FETCHING_ICP_RATE_INTERVAL: Duration = Duration::from_secs(300);

/// Maximum age (in nanoseconds) of a cached price before a price-sensitive
/// operation triggers an on-demand XRC fetch.
///
/// Audit Wave-5 F-004: bumped from 30s to 60s. The XRC `timestamp` field is
/// the CEX bar time, populated as `(now - XRC_MARGIN_SEC=60)`, so a freshly
/// fetched price already starts 60s "old" from this canister's clock. A 30s
/// threshold therefore meant every call refetched, defeating the cache and
/// burning roughly 300M cycles/day. 60s matches the stables-side threshold
/// and lets bursts of activity within the same fetch window hit the cache.
pub const PRICE_FRESHNESS_THRESHOLD_NANOS: u64 = 60 * 1_000_000_000;

pub async fn fetch_icp_rate() {
    let _guard = match crate::guard::FetchXrcGuard::new() {
        Some(guard) => guard,
        None => return,
    };

    // Wave-14a CDP-01: track whether the call succeeded for the
    // consecutive-failure counter. We set this to true on the success
    // path AFTER source-floor / sanity-band acceptance.
    let mut xrc_call_succeeded = false;

    match crate::management::fetch_icp_price().await {
        Ok(call_result) => match call_result {
            GetExchangeRateResult::Ok(exchange_rate_result) => {
                // Wave-14a CDP-14: refuse to consume a price that is below
                // the configured aggregation floor. Cached price stays in
                // place; the staleness gate (or the CDP-01 failure counter
                // below) will eventually fire if the condition persists.
                // Source-floor rejection is its own signal class (the call
                // succeeded but the aggregation is too thin), tracked via
                // OracleSourceCountInsufficient rather than the
                // consecutive-failure counter.
                // ic-xrc-types calls this field `base_asset_num_received_rates`
                // (the number of CEXs that actually returned a rate for the
                // base asset — for ICP/USD that's the ICP-side aggregation).
                let num_sources =
                    exchange_rate_result.metadata.base_asset_num_received_rates as u32;
                // Wave-14a CDP-14 follow-up: resolve the per-collateral
                // override (defaults to the global floor when unset).
                let floor = read_state(|s| {
                    let icp_ct = s.icp_collateral_type();
                    s.get_collateral_config(&icp_ct)
                        .map(|c| c.effective_min_xrc_sources(s.min_xrc_sources_used))
                        .unwrap_or(s.min_xrc_sources_used)
                });
                if !xrc_metadata_meets_source_floor(num_sources, floor) {
                    log!(
                        TRACE_XRC,
                        "[FetchPrice] rejecting ICP rate {}: only {} XRC sources (floor {})",
                        exchange_rate_result.rate,
                        num_sources,
                        floor
                    );
                    let icp_ct = read_state(|s| s.icp_collateral_type());
                    crate::storage::record_event(&Event::OracleSourceCountInsufficient {
                        collateral_type: icp_ct,
                        num_sources,
                        min_required: floor,
                        timestamp: ic_cdk::api::time(),
                    });
                    // Skip the price-update branch. `xrc_call_succeeded`
                    // stays false so the CDP-01 counter treats this as a
                    // failure (sustained thin-aggregation should trip ReadOnly).
                } else {
                    let rate = Decimal::from_u64(exchange_rate_result.rate).unwrap()
                        / Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals))
                            .unwrap();
                    let ts_nanos = exchange_rate_result.timestamp * 1_000_000_000;

                    // Wave-5 LIQ-007 / ORACLE-009: gate every accepted price through the
                    // sanity band. Pre-Wave-5 the ReadOnly latch fired on `rate < $0.01`
                    // before any sanity check, so a single sub-$0.01 XRC blip could
                    // freeze the protocol. Now we (1) reject samples older than the
                    // stored timestamp, (2) apply the sanity band, then (3) only latch
                    // ReadOnly when a sub-$0.01 sample was actually accepted.
                    let should_update = read_state(|s| match s.last_icp_timestamp {
                        Some(last_ts) => last_ts < ts_nanos,
                        None => true,
                    });
                    if !should_update {
                        log!(
                        TRACE_XRC,
                        "[FetchPrice] ICP rate {rate} skipped: timestamp {} not newer than stored",
                        exchange_rate_result.timestamp
                    );
                    } else {
                        let icp_ct = read_state(|s| s.icp_collateral_type());
                        let rate_f64 = rate.to_f64().unwrap_or(0.0);
                        let accepted =
                            mutate_state(|s| s.check_price_sanity_band(&icp_ct, rate_f64));
                        if !accepted {
                            log!(
                            TRACE_XRC,
                            "[FetchPrice] rejecting outlier ICP rate {rate} (sanity band); awaiting confirmation"
                        );
                        } else {
                            if rate < dec!(0.01) {
                                log!(
                                TRACE_XRC,
                                "[FetchPrice] CONFIRMED sub-$0.01 ICP rate {rate}, switching to ReadOnly at timestamp: {}",
                                exchange_rate_result.timestamp
                            );
                                mutate_state(|s| {
                                    s.mode = Mode::ReadOnly;
                                    s.mode_triggered_by_oracle = false;
                                });
                            }
                            log!(
                                TRACE_XRC,
                                "[FetchPrice] fetched new ICP rate: {rate} with timestamp: {}",
                                exchange_rate_result.timestamp
                            );
                            mutate_state(|s| {
                                s.set_icp_rate(UsdIcp::from(rate), Some(ts_nanos));
                                let icp_ct = s.icp_collateral_type();
                                crate::event::record_price_update(icp_ct, rate, ts_nanos);
                            });
                            xrc_call_succeeded = true;
                        }
                    }
                } // end of Wave-14a CDP-14 source-floor `else` block
            }
            GetExchangeRateResult::Err(error) => ic_canister_log::log!(
                TRACE_XRC,
                "[FetchPrice] failed to call XRC canister with error: {error:?}"
            ),
        },
        Err(error) => ic_canister_log::log!(
            TRACE_XRC,
            "[FetchPrice] failed to call XRC canister with error: {error}"
        ),
    }

    // Wave-14a CDP-01: feed the success/failure into the consecutive-
    // failure counter. A successful price update resets the counter and
    // clears any oracle-triggered ReadOnly. Any non-success path (call
    // Err, GetExchangeRateResult::Err, or source-floor rejection) is
    // treated as a failure for circuit-breaker purposes; sustained
    // failures across `MAX_CONSECUTIVE_XRC_FAILURES` ticks trip ReadOnly
    // and emit `OracleCircuitBreaker`.
    let oracle_event = mutate_state(|s| {
        if xrc_call_succeeded {
            note_xrc_success(s);
            None
        } else {
            note_xrc_failure(s)
        }
    });
    if let Some(ev) = oracle_event {
        crate::storage::record_event(&ev);
    }
    if let Some(last_icp_rate) = read_state(|s| s.last_icp_rate) {
        mutate_state(|s| s.update_total_collateral_ratio_and_mode(last_icp_rate));
    }
    // Wave-14b CDP-12: the post-fetch interest / treasury / vault-check work
    // moved out of this function and into separate, independently scheduled
    // timers. See `interest_and_treasury_tick` (Timer B) and
    // `vault_check_tick` (Timer C) below, wired up in `setup_timers`. A trap
    // anywhere in fetch_icp_rate (Timer A) no longer skips the downstream
    // bookkeeping silently.
}

/// Wave-14b CDP-12 Timer B: per-tick maintenance work that does NOT need a
/// fresh XRC sample. Runs interest accrual + harvest + treasury drains +
/// flush. Skipped under `Mode::ReadOnly` for the same reason the chained
/// version was: no new debt is being created, no new fees to drain.
///
/// Cheap in cycles: pure in-memory state walks plus three short
/// inter-canister calls to the icUSD ledger via the treasury module.
pub async fn interest_and_treasury_tick() {
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        let now = ic_cdk::api::time();
        mutate_state(|s| crate::event::record_accrue_interest(s, now));
        // Harvest accrued interest from vaults into pending distribution map.
        // Zeroes per-vault accrued_interest so interest won't double-count
        // if a repayment happens before the next tick.
        mutate_state(|s| s.harvest_accrued_interest());
    }

    // Drain pending treasury interest/collateral accumulated from sync
    // liquidations.
    crate::treasury::drain_pending_treasury_interest().await;
    crate::treasury::drain_pending_treasury_collateral().await;

    // Flush accumulated interest to pools/treasury when threshold is reached.
    crate::treasury::flush_pending_interest().await;
    crate::treasury::flush_pending_stability_pool_interest_notifications().await;
    crate::treasury::flush_pending_amm1_donations().await;

    // Phase 1b foreign-chain-only supply-invariant self-check. Runs on every
    // Timer B tick (default cadence 60s) per spec Section 3. On drift, halt
    // new debt issuance + supply mutations and flip the protocol into
    // ReadOnly. Manual recovery requires `clear_invariant_halt` (Phase 1b
    // operational tooling) plus a developer-gated mode flip.
    //
    // The invariant is: sum(chain_supplies) == sum(chain_vault.debt_e8s).
    // ICP-native debt (total_borrowed_icusd_amount, which sums only
    // vault_id_to_vaults) is a SEPARATE pool and must NOT be part of this
    // check — unification to a single global total is a Phase 2 task.
    // Using total_borrowed_icusd_amount here would cause a false halt on the
    // very first Monad mint (chain_supplies[Monad] > 0, ICP-side debt == 0
    // on the staging canister → divergence detected → protocol halts).
    //
    // In Phase 1a chain_supplies is empty and total_chain_vault_debt_e8s is
    // also 0, so the check always passes. The invariant becomes live once
    // Phase 1b ships the first confirmed Monad mint.
    let chain_debt_e8s: u128 = read_state(|s| s.multi_chain.total_chain_vault_debt_e8s());
    let check_outcome =
        read_state(|s| crate::chains::supply::check_invariant(&s.multi_chain, chain_debt_e8s));
    if let Err(err) = check_outcome {
        let now = ic_cdk::api::time();
        let (sum, td) = match err {
            crate::chains::supply::SupplyInvariantError::Divergence {
                sum_after,
                total_debt,
            } => (sum_after, total_debt),
            _ => (0u128, chain_debt_e8s),
        };
        mutate_state(|s| {
            s.multi_chain.invariant_halted = true;
            if matches!(s.mode, Mode::GeneralAvailability) {
                s.mode = Mode::ReadOnly;
                s.mode_triggered_by_oracle = false;
            }
        });
        crate::storage::record_event(&Event::SupplyInvariantSelfCheckFailed {
            sum_chain_supplies_e8s: sum,
            total_debt_e8s: td,
            timestamp: now,
        });
        log!(
            INFO,
            "[supply_invariant] FAILED: sum={} total_debt={}; halting and flipping to ReadOnly",
            sum,
            td
        );
    }
}

/// Wave-14b CDP-12 Timer C: vault health sweep + aggregate-snapshot refresh.
/// Runs `check_vaults` (which dispatches to bot / SP and handles partial
/// liquidations) and then refreshes the cached query snapshots so
/// `get_protocol_status` / `get_treasury_stats` serve fresh totals.
///
/// `check_vaults` is gated on `mode != ReadOnly`; the snapshot refresh is
/// unconditional (keeping the cache warm in ReadOnly lets queries continue
/// to serve from cache while the protocol is paused).
pub async fn vault_check_tick() {
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        crate::check_vaults().await;
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| s.refresh_aggregate_snapshots(now));
}

/// Wave-14b CDP-12: cadence for the interest / treasury maintenance timer
/// (Timer B). Cheaper than Timer A's XRC fetch, so 60s is comfortable.
pub const INTEREST_AND_TREASURY_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Wave-14b CDP-12: cadence for the vault-check timer (Timer C). Matches
/// the legacy 300s `check_vaults` cadence.
pub const VAULT_CHECK_TICK_INTERVAL: Duration = Duration::from_secs(300);

/// Ensures the price for the given collateral type is fresh enough for
/// a price-sensitive operation. ICP uses its own dedicated path; other
/// collateral types use the generic fetch_collateral_price.
pub async fn ensure_fresh_price_for(
    collateral_type: &candid::Principal,
) -> Result<(), crate::ProtocolError> {
    let icp_ledger = read_state(|s| s.icp_collateral_type());
    if *collateral_type == icp_ledger {
        ensure_fresh_price().await
    } else {
        let needs_refresh = read_state(|s| match s.get_collateral_config(collateral_type) {
            Some(config) => match config.last_price_timestamp {
                None => true,
                Some(ts) => {
                    let age = ic_cdk::api::time().saturating_sub(ts);
                    age > PRICE_FRESHNESS_THRESHOLD_NANOS
                }
            },
            None => true,
        });

        if needs_refresh {
            log!(
                TRACE_XRC,
                "[ensure_fresh_price_for] Price stale for {}, fetching on-demand",
                collateral_type
            );
            crate::management::fetch_collateral_price(*collateral_type).await;
        }

        // Fail CLOSED if, after the (best-effort) refresh, the cached price is
        // missing OR still older than the hard staleness ceiling.
        //
        // ORACLE-001 / VER-001 (audit 2026-06-05): this previously checked only
        // `last_price.is_some()`. `fetch_collateral_price` is best-effort — on a
        // down feed (XRC source-count rejection, LST canister error, CoinGecko
        // failure) it returns WITHOUT updating the cache. So a stale cached
        // `last_price` passed the gate, letting mint/withdraw originate against
        // an arbitrarily old non-ICP price. We now enforce the same 10-minute
        // hard ceiling the ICP path applies via `check_price_not_too_old`. nICP
        // (LstWrapped) inherits the underlying ICP timestamp (see
        // `fetch_collateral_price`), so this also correctly rejects an nICP
        // price derived from a stale ICP rate.
        const MAX_NON_ICP_PRICE_AGE_NANOS: u64 = 10 * 60 * 1_000_000_000;
        let now = ic_cdk::api::time();
        let fresh = read_state(|s| match s.get_collateral_config(collateral_type) {
            Some(c) => match (c.last_price, c.last_price_timestamp) {
                (Some(price), Some(ts)) if price.is_finite() && price > 0.0 => {
                    now.saturating_sub(ts) <= MAX_NON_ICP_PRICE_AGE_NANOS
                }
                _ => false,
            },
            None => false,
        });
        if fresh {
            Ok(())
        } else {
            Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                "No fresh price available for collateral {} (stale or missing after on-demand refresh)",
                collateral_type
            )))
        }
    }
}

/// Ensures the ICP price is fresh enough for a price-sensitive operation.
/// If the cached price is older than PRICE_FRESHNESS_THRESHOLD_NANOS (30s),
/// fetches a fresh price from XRC before returning.
/// Returns Ok(()) if a fresh-enough price is available, Err if fetch fails
/// and no cached price exists.
pub async fn ensure_fresh_price() -> Result<(), crate::ProtocolError> {
    let needs_refresh = read_state(|s| {
        match s.last_icp_timestamp {
            None => true, // No price at all, definitely need one
            Some(ts) => {
                let age = ic_cdk::api::time().saturating_sub(ts);
                age > PRICE_FRESHNESS_THRESHOLD_NANOS
            }
        }
    });

    if needs_refresh {
        log!(
            TRACE_XRC,
            "[ensure_fresh_price] Cached price is stale (>30s), fetching on-demand"
        );
        fetch_icp_rate().await;

        // After fetch, verify we actually have a price now
        read_state(|s| s.check_price_not_too_old())?;
    }

    Ok(())
}

/// Depeg thresholds: reject ckstable operations if the stablecoin price
/// is outside the safe band. Per HANDOFF.md: "reject if rate < $0.95 or > $1.05"
const DEPEG_LOWER_BOUND: Decimal = dec!(0.95);
const DEPEG_UPPER_BOUND: Decimal = dec!(1.05);

/// Maximum age for cached ckstable prices before re-fetching.
/// More lenient than ICP (60s vs 30s) since stablecoin prices move slowly.
const STABLE_PRICE_FRESHNESS_NANOS: u64 = 60 * 1_000_000_000;

/// Ensures the given ckstable token is not depegged before allowing an operation.
/// On-demand only — fetches from XRC if cached price is stale or missing.
/// Returns Ok(()) if the price is within the safe band [$0.95, $1.05].
pub async fn ensure_stable_not_depegged(
    token_type: &crate::StableTokenType,
) -> Result<(), crate::ProtocolError> {
    let (symbol, needs_refresh) = read_state(|s| {
        let now = ic_cdk::api::time();
        match token_type {
            crate::StableTokenType::CKUSDT => {
                let stale = match s.last_ckusdt_timestamp {
                    None => true,
                    Some(ts) => now.saturating_sub(ts) > STABLE_PRICE_FRESHNESS_NANOS,
                };
                ("USDT".to_string(), stale)
            }
            crate::StableTokenType::CKUSDC => {
                let stale = match s.last_ckusdc_timestamp {
                    None => true,
                    Some(ts) => now.saturating_sub(ts) > STABLE_PRICE_FRESHNESS_NANOS,
                };
                ("USDC".to_string(), stale)
            }
        }
    });

    if needs_refresh {
        log!(
            TRACE_XRC,
            "[ensure_stable_not_depegged] Fetching fresh {} price from XRC",
            symbol
        );

        match crate::management::fetch_stable_price(&symbol).await {
            Ok(call_result) => match call_result {
                GetExchangeRateResult::Ok(exchange_rate_result) => {
                    let rate = Decimal::from_u64(exchange_rate_result.rate).unwrap()
                        / Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals))
                            .unwrap();

                    log!(
                        TRACE_XRC,
                        "[ensure_stable_not_depegged] {} rate: {} at timestamp: {}",
                        symbol,
                        rate,
                        exchange_rate_result.timestamp
                    );

                    let ts_nanos = exchange_rate_result.timestamp * 1_000_000_000;
                    mutate_state(|s| match token_type {
                        crate::StableTokenType::CKUSDT => {
                            s.last_ckusdt_rate = Some(rate);
                            s.last_ckusdt_timestamp = Some(ts_nanos);
                        }
                        crate::StableTokenType::CKUSDC => {
                            s.last_ckusdc_rate = Some(rate);
                            s.last_ckusdc_timestamp = Some(ts_nanos);
                        }
                    });
                }
                GetExchangeRateResult::Err(error) => {
                    log!(
                        TRACE_XRC,
                        "[ensure_stable_not_depegged] XRC error for {}: {:?}",
                        symbol,
                        error
                    );
                    return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                        "Cannot verify {} price: XRC returned error {:?}",
                        symbol, error
                    )));
                }
            },
            Err(error) => {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] Failed to call XRC for {}: {}",
                    symbol,
                    error
                );
                return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                    "Cannot verify {} price: {}",
                    symbol, error
                )));
            }
        }
    }

    // Now check the cached price against depeg thresholds
    let rate = read_state(|s| match token_type {
        crate::StableTokenType::CKUSDT => s.last_ckusdt_rate,
        crate::StableTokenType::CKUSDC => s.last_ckusdc_rate,
    });

    match rate {
        Some(price) => {
            if price < DEPEG_LOWER_BOUND || price > DEPEG_UPPER_BOUND {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] DEPEG DETECTED: {} at ${}, outside [{}, {}]",
                    symbol,
                    price,
                    DEPEG_LOWER_BOUND,
                    DEPEG_UPPER_BOUND
                );
                Err(crate::ProtocolError::GenericError(format!(
                    "{} appears to be depegged (current price: ${:.4}). \
                     Operations with this token are suspended until the price \
                     returns to the ${:.2}–${:.2} range.",
                    symbol, price, DEPEG_LOWER_BOUND, DEPEG_UPPER_BOUND
                )))
            } else {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] {} price ${} is within safe band",
                    symbol,
                    price
                );
                Ok(())
            }
        }
        None => Err(crate::ProtocolError::TemporarilyUnavailable(format!(
            "No {} price available. Cannot verify peg safety.",
            symbol
        ))),
    }
}

// ─── De-scaffold pass (2026-08-20): XRC-sourced chains-rail price feed ───────
//
// The chains-liquidation rail (chains/mod.rs) needs a live collateral price
// for each configured chain, the same way every ICP-native collateral does.
// Rather than reinvent price fetching, this reuses the existing XRC
// `PriceSource::Xrc` pattern (`management.rs`) and the source-floor gate
// (`xrc_metadata_meets_source_floor` above), writing accepted prices through
// `MultiChainState::set_manual_price` — the SAME internal path
// `set_manual_collateral_price` (main.rs) uses, so the existing fail-closed
// staleness gate (`chains::liquidation::fresh_chain_price_e8`) needs no
// changes at all.
//
// CYCLE-NEUTRALITY (the whole point of this section): the work set is
// derived from state, not from a fixed chain list. A chain only enters the
// set when it is BOTH (a) registered (`ChainStatus::Registered`) AND (b) has
// an operator-set liquidation config row (`chain_liquidation_configs`). On
// today's prod state neither condition holds for any chain, so
// `chains_needing_price_feed` returns empty and `fetch_chains_prices` (below)
// returns before making a single XRC call. `chains_needing_price_feed_is_empty_test`
// pins this directly; every other XRC-call site in this file already proves
// its own gate is a synchronous, pre-spawn check (see
// `should_fetch_collateral_price` above) and `fetch_chains_prices` follows
// the identical shape: read the gate, and only THEN loop.
use crate::chains::config::{ChainId, ChainStatus};

/// Pure gate: which `(ChainId, native_symbol)` pairs currently need a price.
/// A chain qualifies only when it is `Registered` in `multi_chain.chain_configs`
/// AND carries a `chain_liquidation_configs` row (regardless of that row's
/// `enabled` flag: staging a config while disabled should still keep the
/// price warm so flipping `enabled` later doesn't start on a stale/missing
/// price). The symbol comes from the compile-time `evm_chain_config` (the
/// same table `chains/evm/mod.rs` uses for `native_symbol`); a chain with no
/// EVM config (e.g. a future non-EVM chain, or an unknown id) is skipped
/// rather than guessed at.
pub fn chains_needing_price_feed(state: &State) -> Vec<(ChainId, String)> {
    state
        .multi_chain
        .chain_configs
        .iter()
        .filter(|(_, cfg)| matches!(cfg.status, ChainStatus::Registered))
        .filter(|(id, _)| state.multi_chain.chain_liquidation_configs.contains_key(id))
        .filter_map(|(id, _)| {
            crate::chains::evm::evm_chain_config(*id).map(|c| (*id, c.native_symbol.to_string()))
        })
        .collect()
}

/// XRC cycles cost per `get_exchange_rate` call. Matches the collateral-price
/// path (`management.rs::fetch_collateral_price`'s `XRC_CALL_COST_CYCLES`).
const CHAINS_XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;

/// Margin (seconds) subtracted from `now` when asking XRC for a rate, so the
/// request doesn't land in the future relative to the CEX aggregation window.
/// Matches `management.rs`'s `XRC_MARGIN_SEC`.
const CHAINS_XRC_MARGIN_SEC: u64 = 60;

/// Fetch a fresh USD price for every `(chain, symbol)` currently returned by
/// `chains_needing_price_feed` and write accepted ones through
/// `MultiChainState::set_manual_price` (the same internal path
/// `set_manual_collateral_price` uses).
///
/// CYCLE-NEUTRALITY: the gate is read synchronously, before any `await` or
/// XRC call. An empty work set (today's prod state) returns immediately
/// having made zero XRC calls — see `chains_needing_price_feed` and its
/// `empty_on_default_state_zero_xrc_calls_on_prod_today` test above.
pub async fn fetch_chains_prices() {
    let work = read_state(chains_needing_price_feed);
    if work.is_empty() {
        return;
    }
    for (chain, symbol) in work {
        fetch_one_chain_price(chain, symbol).await;
    }
}

/// Fetch + apply a single chain's price. Best-effort, mirroring
/// `fetch_collateral_price`: any failure (call error, XRC error, thin
/// source count, non-finite/non-positive rate) logs and returns WITHOUT
/// writing anything. The existing fail-closed staleness gate
/// (`chains::liquidation::fresh_chain_price_e8`) is what actually protects
/// liquidations from a stale or missing price; this function never needs to
/// touch it. No retries beyond the next timer tick.
async fn fetch_one_chain_price(chain: ChainId, symbol: String) {
    let xrc_principal = read_state(|s| s.xrc_principal);
    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - CHAINS_XRC_MARGIN_SEC;
    let args = GetExchangeRateRequest {
        base_asset: Asset {
            symbol: symbol.clone(),
            class: AssetClass::Cryptocurrency,
        },
        quote_asset: Asset {
            symbol: "USD".to_string(),
            class: AssetClass::FiatCurrency,
        },
        timestamp: Some(timestamp_sec),
    };

    let res: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args,),
        CHAINS_XRC_CALL_COST_CYCLES,
    )
    .await;

    match res {
        Ok((GetExchangeRateResult::Ok(rate_result),)) => {
            // Wave-14a CDP-14-style source-floor gate, applied identically
            // to the collateral XRC path: a thin aggregation is cheaper to
            // manipulate than one drawn from multiple venues.
            let num_sources = rate_result.metadata.base_asset_num_received_rates as u32;
            let floor = read_state(|s| s.min_xrc_sources_used);
            if !xrc_metadata_meets_source_floor(num_sources, floor) {
                log!(
                    TRACE_XRC,
                    "[fetch_chains_prices] rejecting {:?}/{} rate {}: only {} XRC sources (floor {})",
                    chain,
                    symbol,
                    rate_result.rate,
                    num_sources,
                    floor
                );
                return;
            }

            let decimals = rust_decimal::Decimal::from_u64(10_u64.pow(rate_result.metadata.decimals));
            let rate_dec = rust_decimal::Decimal::from_u64(rate_result.rate);
            let rate_f64 = match (rate_dec, decimals) {
                (Some(r), Some(d)) if !d.is_zero() => (r / d).to_f64(),
                _ => None,
            };
            let Some(rate_f64) = rate_f64 else {
                log!(
                    TRACE_XRC,
                    "[fetch_chains_prices] {:?}/{} rate {} (decimals {}) failed to decode",
                    chain,
                    symbol,
                    rate_result.rate,
                    rate_result.metadata.decimals
                );
                return;
            };
            if !rate_f64.is_finite() || rate_f64 <= 0.0 {
                log!(
                    TRACE_XRC,
                    "[fetch_chains_prices] {:?}/{} rejecting non-finite/non-positive rate {}",
                    chain,
                    symbol,
                    rate_f64
                );
                return;
            }

            let price_e8_f64 = (rate_f64 * 100_000_000.0).round();
            if !price_e8_f64.is_finite() || price_e8_f64 < 1.0 || price_e8_f64 > u64::MAX as f64 {
                log!(
                    TRACE_XRC,
                    "[fetch_chains_prices] {:?}/{} price_e8 {} out of representable range",
                    chain,
                    symbol,
                    price_e8_f64
                );
                return;
            }
            let price_e8 = price_e8_f64 as u64;

            let now = ic_cdk::api::time();
            mutate_state(|s| {
                s.multi_chain
                    .set_manual_price(chain, symbol.clone(), price_e8, now);
            });
            log!(
                TRACE_XRC,
                "[fetch_chains_prices] {:?}/{} price_e8={} at {}",
                chain,
                symbol,
                price_e8,
                now
            );
        }
        Ok((GetExchangeRateResult::Err(error),)) => {
            log!(
                TRACE_XRC,
                "[fetch_chains_prices] XRC error for {:?}/{}: {:?}",
                chain,
                symbol,
                error
            );
        }
        Err((code, msg)) => {
            log!(
                TRACE_XRC,
                "[fetch_chains_prices] call error for {:?}/{}: {:?} {}",
                chain,
                symbol,
                code,
                msg
            );
        }
    }
}

#[cfg(test)]
mod cycle_cadence_tests {
    use super::{
        collateral_needs_periodic_price_refresh, collateral_price_fetch_secs,
        should_fetch_collateral_price, DEFAULT_COLLATERAL_PRICE_FETCH_SECS,
    };
    use crate::state::{CollateralStatus, State};
    use crate::vault::Vault;
    use crate::{InitArg, ICUSD};
    use candid::Principal;

    fn ckbtc() -> Principal {
        Principal::from_text("mxzaz-hqaaa-aaaar-qaada-cai").unwrap()
    }

    fn configured_state() -> State {
        State::from(InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        })
    }

    fn vault(id: u64, collateral_type: Principal, collateral: u64, debt: u64) -> Vault {
        Vault {
            owner: Principal::anonymous(),
            vault_id: id,
            collateral_amount: collateral,
            borrowed_icusd_amount: ICUSD::new(debt),
            collateral_type,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        }
    }

    #[test]
    fn absent_collateral_falls_back_to_default() {
        // A collateral with no entry keeps the historical 300s cadence, so a
        // legacy snapshot (empty map) behaves exactly as before this field.
        let s = State::default();
        assert_eq!(
            collateral_price_fetch_secs(&s, &ckbtc()),
            DEFAULT_COLLATERAL_PRICE_FETCH_SECS
        );
    }

    #[test]
    fn override_is_honored() {
        let mut s = State::default();
        s.collateral_price_fetch_interval_secs.insert(ckbtc(), 1800);
        assert_eq!(collateral_price_fetch_secs(&s, &ckbtc()), 1800);
    }

    #[test]
    fn zero_falls_back_and_sub_60_is_floored() {
        let mut s = State::default();
        // A stored 0 (the setter rejects it, but a corrupt value must not
        // busy-loop) falls back to the default.
        s.collateral_price_fetch_interval_secs.insert(ckbtc(), 0);
        assert_eq!(
            collateral_price_fetch_secs(&s, &ckbtc()),
            DEFAULT_COLLATERAL_PRICE_FETCH_SECS
        );
        // Any other sub-60 value is floored to 60s.
        s.collateral_price_fetch_interval_secs.insert(ckbtc(), 5);
        assert_eq!(collateral_price_fetch_secs(&s, &ckbtc()), 60);
    }

    #[test]
    fn sunset_price_refresh_tracks_true_retirement() {
        let mut state = configured_state();
        let collateral = state.icp_collateral_type();
        state
            .collateral_configs
            .get_mut(&collateral)
            .unwrap()
            .status = CollateralStatus::Sunset;

        assert!(
            !should_fetch_collateral_price(&state, &collateral),
            "fully retired Sunset collateral must stop burning XRC cycles"
        );

        state.open_vault(vault(999, collateral, 100_000_000, 100_000_000));
        assert!(should_fetch_collateral_price(&state, &collateral));

        let vault = state.vault_id_to_vaults.get_mut(&999).unwrap();
        vault.borrowed_icusd_amount = ICUSD::new(0);
        assert!(
            should_fetch_collateral_price(&state, &collateral),
            "debt-free Sunset vault still needs a fresh withdrawal/liquidation price"
        );

        state
            .vault_id_to_vaults
            .get_mut(&999)
            .unwrap()
            .collateral_amount = 0;
        assert!(
            should_fetch_collateral_price(&state, &collateral),
            "an open Sunset vault remains in wind-down until close"
        );

        state.close_vault(999);
        assert!(!should_fetch_collateral_price(&state, &collateral));
    }

    #[test]
    fn active_collateral_refresh_behavior_is_unchanged() {
        let state = configured_state();
        let collateral = state.icp_collateral_type();

        assert!(collateral_needs_periodic_price_refresh(
            CollateralStatus::Active
        ));
        assert!(should_fetch_collateral_price(&state, &collateral));
    }
}

#[cfg(test)]
mod chains_price_feed_tests {
    use super::chains_needing_price_feed;
    use crate::chains::config::{ChainConfigV3, ChainId, ChainStatus, GasStrategy};
    use crate::chains::liquidation_config::{ChainLiquidationConfigV1, DexKind};
    use crate::state::State;

    const CFX_TESTNET: ChainId = ChainId(71);
    const CFX_MAINNET: ChainId = ChainId(1030);
    // Not present in `evm_chain_config` (chains/evm/mod.rs): used to prove an
    // unknown/non-EVM chain id is skipped rather than guessed at.
    const UNKNOWN_CHAIN: ChainId = ChainId(999);

    fn registered_chain_config(id: ChainId) -> ChainConfigV3 {
        ChainConfigV3 {
            chain_id: id,
            display_name: "test chain".to_string(),
            rpc_endpoints: vec!["https://example.invalid".to_string()],
            finality_depth: 400,
            gas_strategy: GasStrategy::EvmEip1559 {
                max_priority_fee_gwei: 1,
                max_fee_gwei_ceiling: 100,
            },
            chain_native_decimals: 18,
            registered_at_ns: 0,
            status: ChainStatus::Registered,
            burn_watch_poll_enabled: false,
            min_quorum_providers: None,
        }
    }

    // `enabled: false` on purpose: a staged-but-disabled config row must
    // still count for the price feed (see `chains_needing_price_feed` doc).
    fn liquidation_config_row(enabled: bool) -> ChainLiquidationConfigV1 {
        ChainLiquidationConfigV1 {
            dex: DexKind::UniswapV2,
            router: String::new(),
            factory: String::new(),
            pair: String::new(),
            collateral_token: String::new(),
            settle_stable_token: String::new(),
            slippage_cap_bps: 250,
            restore_target_cr_e4: 15_500,
            enabled,
            max_swap_value_e8s: 0,
            max_price_age_ns: 0,
            max_dex_oracle_divergence_bps: 0,
            fee_bps: 0,
            settle_stable_decimals: 0,
            deadline_secs: 0,
        }
    }

    #[test]
    fn empty_on_default_state_zero_xrc_calls_on_prod_today() {
        // The cycle-neutrality guarantee: on today's prod state (no chain
        // registered, no liquidation config row), the work set MUST be
        // empty. `fetch_chains_prices` reads exactly this gate before
        // making any XRC call, so this test pins the "no configured chains
        // => no XRC calls" requirement at the pure-function level.
        let state = State::default();
        assert!(chains_needing_price_feed(&state).is_empty());
    }

    #[test]
    fn registered_chain_without_liquidation_config_row_is_excluded() {
        let mut state = State::default();
        state
            .multi_chain
            .chain_configs
            .insert(CFX_MAINNET, registered_chain_config(CFX_MAINNET));
        // No `chain_liquidation_configs` row inserted.
        assert!(
            chains_needing_price_feed(&state).is_empty(),
            "a registered chain with no liquidation config row must not burn XRC cycles"
        );
    }

    #[test]
    fn liquidation_config_row_without_registration_is_excluded() {
        let mut state = State::default();
        state
            .multi_chain
            .chain_liquidation_configs
            .insert(CFX_MAINNET, liquidation_config_row(true));
        // No `chain_configs` entry inserted at all (never registered).
        assert!(chains_needing_price_feed(&state).is_empty());
    }

    #[test]
    fn disabled_chain_status_is_excluded_even_with_config_row() {
        let mut state = State::default();
        let mut cfg = registered_chain_config(CFX_MAINNET);
        cfg.status = ChainStatus::Disabled;
        state.multi_chain.chain_configs.insert(CFX_MAINNET, cfg);
        state
            .multi_chain
            .chain_liquidation_configs
            .insert(CFX_MAINNET, liquidation_config_row(true));
        assert!(chains_needing_price_feed(&state).is_empty());
    }

    #[test]
    fn registered_plus_configured_chain_is_included_with_its_native_symbol() {
        let mut state = State::default();
        state
            .multi_chain
            .chain_configs
            .insert(CFX_MAINNET, registered_chain_config(CFX_MAINNET));
        state
            .multi_chain
            .chain_liquidation_configs
            .insert(CFX_MAINNET, liquidation_config_row(true));
        assert_eq!(
            chains_needing_price_feed(&state),
            vec![(CFX_MAINNET, "CFX".to_string())]
        );
    }

    #[test]
    fn a_disabled_but_staged_liquidation_config_row_still_counts() {
        // enabled=false: an operator staging the DEX wiring before flipping
        // the kill switch should still keep the price warm.
        let mut state = State::default();
        state
            .multi_chain
            .chain_configs
            .insert(CFX_MAINNET, registered_chain_config(CFX_MAINNET));
        state
            .multi_chain
            .chain_liquidation_configs
            .insert(CFX_MAINNET, liquidation_config_row(false));
        assert_eq!(
            chains_needing_price_feed(&state),
            vec![(CFX_MAINNET, "CFX".to_string())]
        );
    }

    #[test]
    fn chain_unknown_to_evm_chain_config_is_excluded() {
        let mut state = State::default();
        state
            .multi_chain
            .chain_configs
            .insert(UNKNOWN_CHAIN, registered_chain_config(UNKNOWN_CHAIN));
        state
            .multi_chain
            .chain_liquidation_configs
            .insert(UNKNOWN_CHAIN, liquidation_config_row(true));
        assert!(
            chains_needing_price_feed(&state).is_empty(),
            "a chain with no compile-time EVM config must not be guessed at"
        );
    }

    #[test]
    fn multiple_registered_configured_chains_are_all_included() {
        let mut state = State::default();
        for id in [CFX_TESTNET, CFX_MAINNET] {
            state
                .multi_chain
                .chain_configs
                .insert(id, registered_chain_config(id));
            state
                .multi_chain
                .chain_liquidation_configs
                .insert(id, liquidation_config_row(true));
        }
        let mut work = chains_needing_price_feed(&state);
        work.sort();
        assert_eq!(
            work,
            vec![
                (CFX_TESTNET, "CFX".to_string()),
                (CFX_MAINNET, "CFX".to_string()),
            ]
        );
    }
}
