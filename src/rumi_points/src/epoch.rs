//! Weekly epoch driver (spec Section 7, implementation plan Phase 5).
//!
//! PHASE 5 SCOPE (skeleton only here). Once per week the driver:
//!   1. Derives two intra-epoch snapshot times via the commit-reveal seed
//!      (`snapshot_seed::derive_snapshot_times`).
//!   2. Captures per-principal balances at each snapshot into a transient buffer.
//!   3. Accrues `dollar_days = active_value * multiplier * period / day` per
//!      position, takes `min(snapshot_a, snapshot_b)` (closes end-of-epoch
//!      sniping), and adds to `total_points`.
//!   4. For matched ckUSDC+ckUSDT: `2 * min(USDC, USDT)` at 5x, remainder at 3x
//!      (the dust-gaming fix).
//!   5. For open repayment windows: `amount * elapsed_days_in_window * 5`,
//!      truncated at season end.
//!   6. Appends `PointEntry` rows and one `EpochSummary`, advances
//!      `last_epoch_processed`, and closes the seed epoch.
//!
//! None of the multiplier / snapshot / min() math is implemented in Phase 1.

#![allow(dead_code)] // Phase 5 surface.

use std::cell::RefCell;
use std::time::Duration;

use candid::{Nat, Principal};
use ic_cdk::api::call::RejectionCode;
use ic_cdk_timers::TimerId;
use icrc_ledger_types::icrc1::account::Account;

use crate::accrual::{self, RawSnapshot};
use crate::events::SourceId;
use crate::snapshot_seed::{sha256, SeedError, SeedManager};
use crate::source_types::balances;
use crate::state;
use crate::types::{AssetType, EpochSummary, OpenEpoch};
use crate::valuation::SnapshotPrices;

/// Length of one epoch. A week, expressed in nanoseconds (IC time unit).
pub const EPOCH_DURATION_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

/// Bounds of epoch `index`: `[season_start + index*EPOCH, min(start + EPOCH,
/// season_end)]`. The last epoch is partial (truncated at season end).
pub fn epoch_bounds(index: u64, season_start_ns: u64, season_end_ns: u64) -> (u64, u64) {
    let start = season_start_ns.saturating_add(index.saturating_mul(EPOCH_DURATION_NS));
    let end = start.saturating_add(EPOCH_DURATION_NS).min(season_end_ns);
    (start, end)
}

/// What the periodic driver should do on this tick (state machine, spec Section 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverAction {
    /// Nothing to do yet (waiting for a snapshot time, the epoch end, or the
    /// season start / next epoch).
    Idle,
    /// Open the next epoch (index >= 1 only; epoch 0 is bootstrapped by the admin
    /// `start_season`, which provides the secret seed S0).
    Start,
    CaptureA,
    CaptureB,
    Close,
}

/// Decide the driver's action. Pure: the caller (the timer tick) reads
/// `now`/season/open-epoch from state, then performs the returned action. Assumes
/// the driver is enabled (the tick checks that before calling).
pub fn next_action(
    open: &Option<OpenEpoch>,
    now: u64,
    season_start: u64,
    season_end: u64,
    current_index: u64,
) -> DriverAction {
    match open {
        Some(oe) => {
            if !oe.a_complete {
                step_when(now >= oe.snapshot_a_ns, DriverAction::CaptureA)
            } else if !oe.b_complete {
                step_when(now >= oe.snapshot_b_ns, DriverAction::CaptureB)
            } else {
                step_when(now >= oe.epoch_end_ns, DriverAction::Close)
            }
        }
        // Epoch 0 is operator-bootstrapped (it needs the secret seed); the driver
        // only auto-starts epochs >= 1, whose seed is pre-loaded by the prior close.
        None if current_index == 0 => DriverAction::Idle,
        None => {
            let (start, _) = epoch_bounds(current_index, season_start, season_end);
            step_when(start < season_end && now >= start, DriverAction::Start)
        }
    }
}

fn step_when(ready: bool, action: DriverAction) -> DriverAction {
    if ready {
        action
    } else {
        DriverAction::Idle
    }
}

thread_local! {
    /// The live epoch-driver timer (transient; re-registered in `post_upgrade`).
    static EPOCH_TIMER: RefCell<Option<TimerId>> = RefCell::new(None);
}

/// Principals captured per driver tick. Season-1 scale fits one tick; larger
/// seasons span several (the cursor in `OpenEpoch` resumes between ticks).
const CAPTURE_CHUNK: u64 = 100;

type CallResult<T> = Result<T, (RejectionCode, String)>;

#[derive(Clone, Copy)]
enum Snapshot {
    A,
    B,
}

/// The 3USD/ICP AMM pool, oriented so `reserve_3usd` is the 3USD leg regardless of
/// the pool's `token_a`/`token_b` order.
#[derive(Clone, Debug, PartialEq)]
pub struct AmmPool {
    pub pool_id: String,
    pub reserve_3usd: u128,
    pub reserve_icp: u128,
    pub total_lp: u128,
}

/// Pick the 3USD/ICP pool from `pools` and orient its reserves. `None` if absent.
pub fn pick_amm_pool(
    pools: &[balances::PoolInfo],
    threeusd: Principal,
    icp: Principal,
) -> Option<AmmPool> {
    pools.iter().find_map(|p| {
        let pair = [p.token_a, p.token_b];
        if !(pair.contains(&threeusd) && pair.contains(&icp)) {
            return None;
        }
        // Orient so reserve_3usd is the 3USD leg regardless of token order.
        let (reserve_3usd, reserve_icp) = if p.token_a == threeusd {
            (p.reserve_a, p.reserve_b)
        } else {
            (p.reserve_b, p.reserve_a)
        };
        Some(AmmPool {
            pool_id: p.pool_id.clone(),
            reserve_3usd,
            reserve_icp,
            total_lp: p.total_lp_shares,
        })
    })
}

/// Why `start_season` was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartSeasonError {
    /// `now` is outside `[season_start, season_end)`.
    SeasonInactive,
    /// Epoch 0 is already open or past (the season was already started).
    AlreadyStarted,
    /// The provided seed did not match the committed `H0`.
    Seed(SeedError),
}

// ── Timer (mirrors the poll timer: OFF by default, re-registered post-upgrade) ──

pub fn setup_epoch_timer() {
    EPOCH_TIMER.with(|t| {
        if let Some(id) = t.borrow_mut().take() {
            ic_cdk_timers::clear_timer(id);
        }
    });
    if state::epoch_driver_enabled() {
        let interval = Duration::from_secs(state::epoch_driver_interval_secs());
        let id = ic_cdk_timers::set_timer_interval(interval, || {
            ic_cdk::spawn(async {
                epoch_driver_tick().await;
            });
        });
        EPOCH_TIMER.with(|t| *t.borrow_mut() = Some(id));
    }
}

// ── Bootstrap: the admin opens epoch 0 with the secret seed S0 ──

pub fn start_season(initial_seed: [u8; 32], now: u64) -> Result<(), StartSeasonError> {
    let (season_start, season_end) = state::season_bounds();
    if now < season_start || now >= season_end {
        return Err(StartSeasonError::SeasonInactive);
    }
    if state::current_epoch_index() != 0 || state::get_open_epoch().is_some() {
        return Err(StartSeasonError::AlreadyStarted);
    }
    open_new_epoch(0, Some(initial_seed), now).map_err(StartSeasonError::Seed)
}

/// Derive the epoch's seed + snapshot times, install a fresh `OpenEpoch`, and clear
/// the snapshot buffer. Epoch 0 passes `Some(S0)`; epochs >= 1 pass `None` (the
/// pre-loaded `current_seed` is used).
fn open_new_epoch(index: u64, seed_arg: Option<[u8; 32]>, _now: u64) -> Result<(), SeedError> {
    let (season_start, season_end) = state::season_bounds();
    let (start, end) = epoch_bounds(index, season_start, season_end);
    let (a_ns, b_ns) =
        state::with_state_mut(|s| SeedManager::start_epoch(&mut s.snapshot_seed, start, end, seed_arg))?;
    state::snapshot_buffer_clear();
    state::set_open_epoch(Some(OpenEpoch {
        epoch_index: index,
        epoch_start_ns: start,
        epoch_end_ns: end,
        snapshot_a_ns: a_ns,
        snapshot_b_ns: b_ns,
        a_cursor: None,
        a_complete: false,
        b_cursor: None,
        b_complete: false,
    }));
    Ok(())
}

// ── Periodic driver tick ──

/// The timer callback: run a tick only while the driver is enabled.
pub async fn epoch_driver_tick() {
    if state::epoch_driver_enabled() {
        run_tick().await;
    }
}

/// One state-machine step, regardless of the enabled flag (admin `force_epoch_tick`
/// and the E2E drive this directly). The single-tick guard still applies.
pub async fn run_tick() {
    if !state::try_begin_epoch() {
        return; // a tick is already in flight
    }
    let now = ic_cdk::api::time();
    let (season_start, season_end) = state::season_bounds();
    let open = state::get_open_epoch();
    let index = state::current_epoch_index();
    match next_action(&open, now, season_start, season_end, index) {
        DriverAction::Idle => {}
        DriverAction::Start => {
            if let Err(e) = open_new_epoch(index, None, now) {
                ic_cdk::println!("[epoch] start of epoch {} failed: {:?}", index, e);
            }
        }
        DriverAction::CaptureA => capture(Snapshot::A).await,
        DriverAction::CaptureB => capture(Snapshot::B).await,
        DriverAction::Close => close_current_epoch(now),
    }
    state::end_epoch_guard();
}

// ── Snapshot capture (one chunk per tick) ──

async fn capture(which: Snapshot) {
    let mut open = match state::get_open_epoch() {
        Some(o) => o,
        None => return,
    };
    let ctx = match fetch_context().await {
        Some(c) => c,
        None => return, // a snapshot-wide source was unreachable; retry next tick
    };
    let cursor = match which {
        Snapshot::A => open.a_cursor,
        Snapshot::B => open.b_cursor,
    };
    let chunk = state::registered_chunk_after(cursor, CAPTURE_CHUNK);
    for p in &chunk {
        if state::is_excluded(p) {
            continue;
        }
        let raw = fetch_raw_snapshot(*p, &ctx).await;
        let weights = accrual::snapshot_weights(&accrual::build_snapshot_inputs(&raw, &ctx.prices));
        match which {
            Snapshot::A => state::snapshot_buffer_put(*p, weights),
            Snapshot::B => state::snapshot_buffer_merge_min(*p, weights),
        }
    }
    // A short chunk means the registered set is exhausted: this snapshot is done.
    let done = (chunk.len() as u64) < CAPTURE_CHUNK;
    let last = chunk.last().copied();
    match which {
        Snapshot::A => {
            open.a_cursor = if done { None } else { last };
            open.a_complete = done;
        }
        Snapshot::B => {
            open.b_cursor = if done { None } else { last };
            open.b_complete = done;
        }
    }
    state::set_open_epoch(Some(open));
}

// ── Epoch close ──

fn close_current_epoch(now: u64) {
    let open = match state::get_open_epoch() {
        Some(o) => o,
        None => return,
    };
    let stats =
        state::run_close_accrual(open.epoch_index, open.epoch_start_ns, open.epoch_end_ns, now);
    let summary = EpochSummary {
        epoch_index: open.epoch_index,
        epoch_start_ns: open.epoch_start_ns,
        epoch_end_ns: open.epoch_end_ns,
        total_points_all: stats.total_points_all,
        points_accrued_this_epoch: stats.points_accrued,
        active_principals: stats.active_principals,
        registered_principals: stats.registered_principals,
        snapshot_a_ns: open.snapshot_a_ns,
        snapshot_b_ns: open.snapshot_b_ns,
    };
    let hash = summary_hash(&summary);
    match state::with_state_mut(|s| {
        SeedManager::close_epoch(
            &mut s.snapshot_seed,
            open.epoch_index,
            open.snapshot_a_ns,
            open.snapshot_b_ns,
            now,
            hash,
        )
    }) {
        Ok(revealed) => state::append_revealed_seed(revealed),
        // Unreachable in the live flow (`current_seed` is always `Some` once an
        // epoch is open). If it ever happens, trap to roll the whole close back
        // atomically rather than advance the index with a broken (reused) seed.
        Err(e) => ic_cdk::trap(&format!(
            "[epoch] seed close of epoch {} failed ({:?}); halting to avoid a broken seed chain",
            open.epoch_index, e
        )),
    }
    state::append_epoch_summary(summary);
    state::advance_epoch_index();
    state::set_open_epoch(None);
}

/// Deterministic hash binding the next seed to this epoch's chain state (spike 0.3).
fn summary_hash(s: &EpochSummary) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&s.epoch_index.to_le_bytes());
    buf.extend_from_slice(&s.total_points_all.to_le_bytes());
    buf.extend_from_slice(&s.registered_principals.to_le_bytes());
    buf.extend_from_slice(&s.points_accrued_this_epoch.to_le_bytes());
    buf.extend_from_slice(&s.snapshot_a_ns.to_le_bytes());
    buf.extend_from_slice(&s.snapshot_b_ns.to_le_bytes());
    sha256(&[&buf])
}

// ── Inter-canister fetch helpers (validated by the PocketIC E2E) ──

/// Snapshot-wide values fetched once per capture (not per principal), including
/// the resolved source-canister ids so the per-principal pass does not re-read them.
struct SnapshotContext {
    prices: SnapshotPrices,
    amm: Option<AmmPool>,
    icusd_ledger: Principal,
    threeusd_ledger: Principal,
    backend: Principal,
    threepool: Principal,
    sp: Option<Principal>,
    amm_canister: Principal,
}

async fn fetch_context() -> Option<SnapshotContext> {
    let backend = state::get_source_canister(SourceId::Backend.tag())?;
    let threepool = state::get_source_canister(SourceId::ThreePool.tag())?;
    let amm_canister = state::get_source_canister(SourceId::Amm.tag())?;
    let sp = state::get_source_canister(SourceId::StabilityPool.tag());
    let icp_rate = fetch_icp_rate(backend).await?;
    let virtual_price = fetch_virtual_price(threepool).await?;
    let threeusd = state::get_asset_ledger(AssetType::ThreeUsd)?;
    let icp = state::get_asset_ledger(AssetType::Icp)?;
    let icusd = state::get_asset_ledger(AssetType::IcUsd)?;
    let amm = fetch_amm_pool(amm_canister, threeusd, icp).await;
    Some(SnapshotContext {
        prices: SnapshotPrices { icp_rate, virtual_price },
        amm,
        icusd_ledger: icusd,
        threeusd_ledger: threeusd,
        backend,
        threepool,
        sp,
        amm_canister,
    })
}

async fn fetch_raw_snapshot(p: Principal, ctx: &SnapshotContext) -> RawSnapshot {
    let vault_debt = fetch_vault_debt(ctx.backend, p).await;
    let wallet_3usd = fetch_wallet_3usd(ctx.threepool, p).await;
    let (sp_icusd, sp_3usd) = match ctx.sp {
        Some(c) => fetch_sp_position(c, p, ctx.icusd_ledger, ctx.threeusd_ledger).await,
        None => (0, 0),
    };
    let amm_user_lp = match &ctx.amm {
        Some(pool) => fetch_amm_lp(ctx.amm_canister, &pool.pool_id, p).await,
        None => 0,
    };

    let (recorded_icusd, recorded_usdc, recorded_usdt) = state::recorded_3pool_composition(&p);
    let (amm_total_lp, amm_reserve_3usd, amm_reserve_icp) = match &ctx.amm {
        Some(pool) => (pool.total_lp, pool.reserve_3usd, pool.reserve_icp),
        None => (0, 0, 0),
    };
    RawSnapshot {
        vault_debt,
        recorded_icusd,
        recorded_usdc,
        recorded_usdt,
        wallet_3usd,
        sp_icusd,
        sp_3usd,
        amm_user_lp,
        amm_total_lp,
        amm_reserve_3usd,
        amm_reserve_icp,
    }
}

async fn fetch_icp_rate(backend: Principal) -> Option<f64> {
    let res: CallResult<(balances::ProtocolStatus,)> =
        ic_cdk::call(backend, "get_protocol_status", ()).await;
    match res {
        Ok((s,)) if s.last_icp_rate.is_finite() && s.last_icp_rate > 0.0 => Some(s.last_icp_rate),
        Ok((s,)) => {
            // A corrupt (non-finite / non-positive) oracle rate aborts the whole
            // capture to retry next tick, rather than valuing ICP positions wrong.
            ic_cdk::println!("[epoch] get_protocol_status returned bad icp_rate {}; aborting capture", s.last_icp_rate);
            None
        }
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_protocol_status failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_virtual_price(threepool: Principal) -> Option<u128> {
    let res: CallResult<(balances::PoolStatus,)> =
        ic_cdk::call(threepool, "get_pool_status", ()).await;
    match res {
        Ok((s,)) => Some(s.virtual_price),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_pool_status failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_amm_pool(amm: Principal, threeusd: Principal, icp: Principal) -> Option<AmmPool> {
    let res: CallResult<(Vec<balances::PoolInfo>,)> = ic_cdk::call(amm, "get_pools", ()).await;
    match res {
        Ok((pools,)) => pick_amm_pool(&pools, threeusd, icp),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_pools failed: {:?} {}", c, m);
            None
        }
    }
}

async fn fetch_vault_debt(backend: Principal, p: Principal) -> u128 {
    let res: CallResult<(Vec<balances::CandidVault>,)> =
        ic_cdk::call(backend, "get_vaults", (Some(p),)).await;
    match res {
        Ok((vaults,)) => vaults
            .iter()
            .fold(0u128, |acc, v| acc.saturating_add(v.borrowed_icusd_amount as u128)),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_vaults failed: {:?} {}", c, m);
            0
        }
    }
}

async fn fetch_wallet_3usd(threepool: Principal, p: Principal) -> u128 {
    let account = Account { owner: p, subaccount: None };
    let res: CallResult<(Nat,)> = ic_cdk::call(threepool, "icrc1_balance_of", (account,)).await;
    match res {
        Ok((bal,)) => nat_to_u128(&bal),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] icrc1_balance_of failed: {:?} {}", c, m);
            0
        }
    }
}

async fn fetch_sp_position(
    sp: Principal,
    p: Principal,
    icusd: Principal,
    threeusd: Principal,
) -> (u128, u128) {
    let res: CallResult<(Option<balances::UserStabilityPosition>,)> =
        ic_cdk::call(sp, "get_user_position", (Some(p),)).await;
    match res {
        Ok((Some(pos),)) => {
            let bal = |l: &Principal| pos.stablecoin_balances.get(l).copied().unwrap_or(0) as u128;
            (bal(&icusd), bal(&threeusd))
        }
        Ok((None,)) => (0, 0),
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_user_position failed: {:?} {}", c, m);
            (0, 0)
        }
    }
}

async fn fetch_amm_lp(amm: Principal, pool_id: &str, p: Principal) -> u128 {
    let res: CallResult<(u128,)> =
        ic_cdk::call(amm, "get_lp_balance", (pool_id.to_string(), p)).await;
    match res {
        Ok((lp,)) => lp,
        Err((c, m)) => {
            ic_cdk::println!("[epoch] get_lp_balance failed: {:?} {}", c, m);
            0
        }
    }
}

/// candid `Nat` -> `u128`, saturating (balances never exceed `u128` in practice).
fn nat_to_u128(n: &Nat) -> u128 {
    let digits: String = n.to_string().chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u128>().unwrap_or(u128::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: u64 = EPOCH_DURATION_NS;

    #[test]
    fn epoch_bounds_full_partial_and_offset() {
        // Full epochs from season start 0.
        assert_eq!(epoch_bounds(0, 0, 100 * E), (0, E));
        assert_eq!(epoch_bounds(1, 0, 100 * E), (E, 2 * E));
        // Season-start offset carries through.
        assert_eq!(epoch_bounds(0, 1_000, 1_000 + 100 * E), (1_000, 1_000 + E));
        // The last epoch is truncated at season end.
        assert_eq!(epoch_bounds(1, 0, E + 100), (E, E + 100));
    }

    fn oe(a_complete: bool, b_complete: bool) -> OpenEpoch {
        OpenEpoch {
            epoch_index: 0,
            epoch_start_ns: 0,
            epoch_end_ns: 1_000,
            snapshot_a_ns: 100,
            snapshot_b_ns: 500,
            a_cursor: None,
            a_complete,
            b_cursor: None,
            b_complete,
        }
    }

    #[test]
    fn open_epoch_captures_a_then_b_then_closes() {
        assert_eq!(next_action(&Some(oe(false, false)), 99, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(false, false)), 100, 0, 2_000, 0), DriverAction::CaptureA);
        assert_eq!(next_action(&Some(oe(true, false)), 499, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(true, false)), 500, 0, 2_000, 0), DriverAction::CaptureB);
        assert_eq!(next_action(&Some(oe(true, true)), 999, 0, 2_000, 0), DriverAction::Idle);
        assert_eq!(next_action(&Some(oe(true, true)), 1_000, 0, 2_000, 0), DriverAction::Close);
    }

    #[test]
    fn epoch_zero_is_idle_until_start_season_opens_it() {
        // No open epoch and index 0: the driver waits for the operator bootstrap.
        assert_eq!(next_action(&None, u64::MAX, 0, 2_000, 0), DriverAction::Idle);
    }

    #[test]
    fn subsequent_epoch_starts_when_due_and_in_season() {
        // Index 1, now at epoch-1 start, well inside the season -> Start.
        assert_eq!(next_action(&None, E, 0, 100 * E, 1), DriverAction::Start);
        // Before epoch-1 start -> Idle.
        assert_eq!(next_action(&None, E - 1, 0, 100 * E, 1), DriverAction::Idle);
        // Next epoch's start is at/after season end -> season over -> Idle.
        assert_eq!(next_action(&None, u64::MAX, 0, E, 1), DriverAction::Idle);
    }

    use crate::source_types::balances::PoolInfo;

    fn pr(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }
    fn pool(id: &str, ta: Principal, tb: Principal, ra: u128, rb: u128, lp: u128) -> PoolInfo {
        PoolInfo {
            pool_id: id.into(),
            token_a: ta,
            token_b: tb,
            reserve_a: ra,
            reserve_b: rb,
            total_lp_shares: lp,
        }
    }

    #[test]
    fn pick_amm_pool_orients_reserves_when_token_a_is_3usd() {
        let (three, icp) = (pr(1), pr(2));
        let got = pick_amm_pool(&[pool("x", three, icp, 100, 200, 50)], three, icp).unwrap();
        assert_eq!(
            got,
            AmmPool { pool_id: "x".into(), reserve_3usd: 100, reserve_icp: 200, total_lp: 50 }
        );
    }

    #[test]
    fn pick_amm_pool_orients_reserves_when_token_b_is_3usd() {
        let (three, icp) = (pr(1), pr(2));
        let got = pick_amm_pool(&[pool("y", icp, three, 200, 100, 50)], three, icp).unwrap();
        assert_eq!(got.reserve_3usd, 100);
        assert_eq!(got.reserve_icp, 200);
    }

    #[test]
    fn pick_amm_pool_none_when_pair_absent() {
        let (three, icp) = (pr(1), pr(2));
        assert!(pick_amm_pool(&[pool("z", pr(3), pr(4), 1, 1, 1)], three, icp).is_none());
    }
}
