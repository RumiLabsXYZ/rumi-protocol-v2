use crate::event::Event;
use crate::logs::TRACE_XRC;
use crate::numeric::UsdIcp;
use crate::state::{mutate_state, read_state, CollateralStatus, State};
use crate::Decimal;
use crate::Mode;
use candid::Principal;
use ic_canister_log::log;
use ic_xrc_types::GetExchangeRateResult;
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
/// status. Returns true for `Active` and `Paused` (both still allow
/// price-sensitive operations such as liquidation), false for the soft-
/// delist statuses (`Frozen`, `Sunset`, `Deprecated`) where every code
/// path that would consume a fresh price is already blocked or read-
/// only.
///
/// Used by the per-collateral 300s timer closures registered in
/// `setup_timers()` and `add_collateral_token` so that wound-down
/// collateral no longer burns ~1B cycles per tick on a useless XRC
/// call.
pub fn collateral_needs_periodic_price_refresh(status: CollateralStatus) -> bool {
    match status {
        CollateralStatus::Active | CollateralStatus::Paused => true,
        CollateralStatus::Frozen
        | CollateralStatus::Sunset
        | CollateralStatus::Deprecated => false,
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
pub fn should_fetch_collateral_price(
    state: &crate::state::State,
    ledger_id: &Principal,
) -> bool {
    state
        .get_collateral_status(ledger_id)
        .map(collateral_needs_periodic_price_refresh)
        .unwrap_or(false)
}

/// Wave-9d DOS-011: registers the recurring per-collateral XRC price
/// timer with the status-check gate baked in. Used by both
/// `setup_timers()` (re-registering after upgrade) and
/// `add_collateral_token` (registering for a brand-new collateral).
///
/// The timer keeps firing every `FETCHING_ICP_RATE_INTERVAL`
/// regardless of status — that's free. The gate runs synchronously
/// INSIDE the closure (before any `ic_cdk::spawn`): if the collateral
/// is wound down, we early-return before allocating an async task and
/// before the (~1B cycle) XRC call.
pub fn register_collateral_price_timer(ledger_id: Principal) {
    ic_cdk_timers::set_timer_interval(FETCHING_ICP_RATE_INTERVAL, move || {
        let go = read_state(|s| should_fetch_collateral_price(s, &ledger_id));
        if !go {
            log!(
                TRACE_XRC,
                "[register_collateral_price_timer] skipping fetch for {} (wound-down or removed)",
                ledger_id
            );
            return;
        }
        ic_cdk::spawn(crate::management::fetch_collateral_price(ledger_id));
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
                let num_sources = exchange_rate_result
                    .metadata
                    .base_asset_num_received_rates as u32;
                let floor = read_state(|s| s.min_xrc_sources_used);
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
                    // Skip the price-update branch but keep the rest of the
                    // tick (cached price stays valid; CDP-01 counter NOT
                    // touched because the XRC call itself succeeded).
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
                    let accepted = mutate_state(|s| {
                        s.check_price_sanity_band(&icp_ct, rate_f64)
                    });
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
                            mutate_state(|s| s.mode = Mode::ReadOnly);
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
    // Accrue interest on all vaults before checking vault health.
    // This ensures liquidation decisions use up-to-date debt balances.
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        let now = ic_cdk::api::time();
        mutate_state(|s| crate::event::record_accrue_interest(s, now));
        // Harvest accrued interest from vaults into pending distribution map.
        // This zeroes per-vault accrued_interest so interest won't be double-counted
        // if a repayment happens before the next tick.
        mutate_state(|s| s.harvest_accrued_interest());
    }
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        crate::check_vaults().await;
    }

    // Drain any pending treasury interest/collateral accumulated from sync liquidations
    crate::treasury::drain_pending_treasury_interest().await;
    crate::treasury::drain_pending_treasury_collateral().await;

    // Flush accumulated interest to pools/treasury when threshold is reached
    crate::treasury::flush_pending_interest().await;

    // Wave-9b DOS-006/-007: refresh both aggregate query snapshots.
    // Runs after `check_vaults` (which already iterates every vault),
    // `drain_pending_treasury_*`, and `flush_pending_interest` so the
    // snapshot reflects the post-tick view of totals + accrued
    // interest. Unconditional — keeping the cache warm even in
    // ReadOnly mode lets `get_protocol_status` and `get_treasury_stats`
    // serve from cache while the protocol is paused.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.refresh_aggregate_snapshots(now));
}

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
        let needs_refresh = read_state(|s| {
            match s.get_collateral_config(collateral_type) {
                Some(config) => match config.last_price_timestamp {
                    None => true,
                    Some(ts) => {
                        let age = ic_cdk::api::time().saturating_sub(ts);
                        age > PRICE_FRESHNESS_THRESHOLD_NANOS
                    }
                },
                None => true,
            }
        });

        if needs_refresh {
            log!(
                TRACE_XRC,
                "[ensure_fresh_price_for] Price stale for {}, fetching on-demand",
                collateral_type
            );
            crate::management::fetch_collateral_price(*collateral_type).await;
        }

        // Verify we have a price now
        let has_price = read_state(|s| {
            s.get_collateral_config(collateral_type)
                .and_then(|c| c.last_price)
                .is_some()
        });
        if has_price {
            Ok(())
        } else {
            Err(crate::ProtocolError::GenericError(format!(
                "No price available for collateral {}",
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
        read_state(|s| {
            s.check_price_not_too_old()
        })?;
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
                        / Decimal::from_u64(
                            10_u64.pow(exchange_rate_result.metadata.decimals),
                        )
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