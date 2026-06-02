//! Event ingestion (spike 0.2, `2026-05-07-spike-0.2-event-ingestion-gaps.md`).
//!
//! PHASE 2 SCOPE (skeleton only here). Ingestion is PULL-BASED: `rumi_points`
//! polls each source canister's existing `get_*_events(start, length)` query
//! endpoints on a timer, processes new events idempotently keyed by event id,
//! advances a per-source cursor, and updates `PrincipalState`. No upstream
//! changes are required to start (the one needed change, `repayment_asset` on
//! the backend `RepayToVault` event, is a SEPARATE branch and gates Phase 5, not
//! Phase 1; do not touch the backend here).
//!
//! Sources and cursors (spike 0.2):
//!   - rumi_protocol_backend  get_events                 (vault mint/repay/close/liquidate/redeem)
//!   - rumi_3pool             get_liquidity_events_v2    (v2 ONLY; v1 lacks per-asset breakdown)
//!   - rumi_stability_pool    get_pool_events            (deposit/withdraw/liquidation draw)
//!   - rumi_amm               get_amm_liquidity_events   (LP add/remove)
//!
//! Each handler resolves the principal, auto-registers it on first sight
//! (Phase 3), updates active deposits / repayment events, and writes a
//! `PointEntry` audit row. Registration is gated to the season window by a
//! `pre_season_active` flag so pre-June-1 test traffic does not accrue.

#![allow(dead_code)] // Phase 2 surface.

use candid::Principal;

/// Per-source ingestion cursor (last event id processed). Phase 2 persists these
/// in stable state; Phase 1 only declares the shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCursors {
    pub backend_event_id: u64,
    pub three_pool_liquidity_event_id: u64,
    pub stability_pool_event_id: u64,
    pub amm_liquidity_event_id: u64,
}

/// Poll every source once, process new events idempotently, advance cursors.
/// Registered on a ~60s timer in Phase 2 (`setup_timers`).
pub async fn poll_all_sources() {
    unimplemented!("Phase 2: pull-based ingestion across the four source canisters");
}

/// Handle one decoded backend event for `caller`. Auto-registers on first sight,
/// then mutates the principal's deposit / repayment state and writes a ledger row.
pub fn handle_backend_event(_caller: Principal /* , event: BackendEvent */) {
    unimplemented!("Phase 2");
}

pub fn handle_three_pool_event(_caller: Principal /* , event: LiquidityEventV2 */) {
    unimplemented!("Phase 2");
}

pub fn handle_stability_pool_event(_caller: Principal /* , event: PoolEvent */) {
    unimplemented!("Phase 2");
}

pub fn handle_amm_event(_caller: Principal /* , event: AmmLiquidityEvent */) {
    unimplemented!("Phase 2");
}
