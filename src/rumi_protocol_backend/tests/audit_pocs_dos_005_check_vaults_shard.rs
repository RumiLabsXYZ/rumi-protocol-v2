//! Wave-9c DoS hardening: shard `check_vaults` to the at-risk CR band
//! (DOS-005).
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` finding DOS-005
//!     (`check_vaults() and accrue_all_vault_interest() iterate all vaults
//!     on every XRC tick`).
//!   * Wave plan: `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//!     §"Wave 9 — DoS hardening".
//!
//! # What the gap is
//!
//! `check_vaults()` runs every 5 minutes on the XRC price tick and
//! currently walks every entry in `vault_cr_index` to identify
//! liquidatable vaults. At current TVL this is fine; at scale the cost
//! grows linearly with vault count and dominates the backend's cycle
//! budget — most vaults walked are deeply healthy and re-checking them
//! is wasted work.
//!
//! # How this file pins the fix
//!
//! Wave-9c reuses the LIQ-002 sorted-troves index (`vault_cr_index`)
//! to walk only vaults whose CR is at or below
//! `max(min_liquidation_ratio across collaterals) +
//! check_vaults_alert_band_bps / 10000`. Vaults far above the band are
//! skipped on band-only ticks. Every Nth tick (default 12 = once per
//! hour at the 5-minute XRC cadence) walks the full index as a safety
//! belt for cross-collateral CR-key drift.
//!
//! Layered fences (mirrors the LIQ-002 file structure):
//!
//!  1. **Constant fences** — defaults pinned at audit-spec values.
//!  2. **Threshold-key resolution** — `check_vaults_alert_threshold_key`
//!     uses the worst (max) `min_liq_ratio` across active collaterals,
//!     not a single hard-coded constant. Multi-collateral protocols
//!     would silently miss vaults if we keyed off the wrong floor.
//!  3. **Tick advance** — `advance_check_vaults_tick` returns the
//!     band-vs-full-sweep decision and rolls `ticks_since_full_sweep`.
//!     Pinned for K=0, K=1, K=3, K=12 cadences.
//!  4. **Scan correctness** — `scan_unhealthy_vaults` returns a vec of
//!     unhealthy vaults plus the visited count and full-sweep flag.
//!     The unhealthy vec must equal what a full sweep would have
//!     produced for any vault inside the band; the visited count fences
//!     the "cycle savings" contract on band-only ticks.
//!  5. **Drift safety belt** — without the periodic full sweep, a
//!     vault whose CR-key is stale-above-threshold (cross-collateral
//!     drift) would be missed. The full sweep finds it within K ticks.
//!  6. **Admin tunables** — both setters change behavior on the next
//!     tick. Setting `full_sweep_every_n_ticks = 1` reverts to
//!     pre-Wave-9c behavior (full sweep every tick).
//!  7. **Upgrade hygiene** — pre-Wave-9c snapshots decode with the
//!     three new fields populated from `serde(default)`. State::default
//!     and `From<InitArg>` agree on the defaults.

use candid::Principal;
use rust_decimal_macros::dec;

use rumi_protocol_backend::numeric::{Ratio, UsdIcp, ICUSD};
use rumi_protocol_backend::state::{State, UnhealthyVaultScan};
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::{
    InitArg, DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS,
    DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS,
};

fn icp_ledger() -> Principal {
    Principal::from_slice(&[10])
}

fn fresh_state_with_price(price: f64) -> State {
    let mut state = State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp_ledger(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(price);
    }
    state
}

fn make_vault(vault_id: u64, collateral_e8s: u64, borrowed_icusd_e8s: u64) -> Vault {
    Vault {
        owner: Principal::anonymous(),
        vault_id,
        collateral_amount: collateral_e8s,
        borrowed_icusd_amount: ICUSD::new(borrowed_icusd_e8s),
        collateral_type: icp_ledger(),
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    }
}

fn open_and_reindex(state: &mut State, vault: Vault) {
    let vid = vault.vault_id;
    state.open_vault(vault);
    state.reindex_vault_cr(vid);
}

fn dummy_rate() -> UsdIcp {
    UsdIcp::from(dec!(1.0))
}

// ============================================================================
// Layer 1 — constant fences
// ============================================================================

#[test]
fn dos_005_default_alert_band_pinned_at_1000_bps() {
    assert_eq!(
        DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS, 1000,
        "Wave-9c DOS-005: default alert band must be 1000 bps (10% headroom). \
         Lowering risks missing vaults that drift toward liquidation between \
         re-keys; raising widens the band and burns extra cycles per tick."
    );
}

#[test]
fn dos_005_default_full_sweep_cadence_pinned_at_12_ticks() {
    assert_eq!(
        DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS, 12,
        "Wave-9c DOS-005: default full-sweep cadence must be 12 ticks \
         (one full sweep per hour at the 5-minute XRC tick cadence). \
         Lowering bounds the worst-case missed-vault window but reduces \
         cycle savings; raising extends the missed window beyond an hour."
    );
}

#[test]
fn dos_005_default_state_initializes_band_and_cadence() {
    let state = fresh_state_with_price(10.0);
    assert_eq!(
        state.check_vaults_alert_band_bps, DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS,
        "fresh state must initialize alert band to default"
    );
    assert_eq!(
        state.check_vaults_full_sweep_every_n_ticks,
        DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS,
        "fresh state must initialize full-sweep cadence to default"
    );
    assert_eq!(
        state.ticks_since_full_sweep, 0,
        "fresh state must start at tick 0 (next tick is tick 1, not full sweep)"
    );
}

// ============================================================================
// Layer 2 — threshold-key resolution
// ============================================================================

#[test]
fn dos_005_threshold_uses_max_min_liq_ratio_plus_band() {
    // ICP default: liquidation_ratio = 1.33, borrow_threshold = 1.5.
    // Default band = 1000 bps = 0.10. In Normal mode, threshold should be
    // 1.33 + 0.10 = 1.43 → key = 14_300.
    let state = fresh_state_with_price(10.0);
    assert_eq!(state.check_vaults_alert_threshold_key(), 14_300);
}

#[test]
fn dos_005_threshold_widens_in_recovery_mode() {
    // Recovery mode: get_min_liquidation_ratio_for returns
    // borrow_threshold_ratio (1.5) instead of liquidation_ratio (1.33).
    // Threshold = 1.5 + 0.10 = 1.60 → key = 16_000.
    let mut state = fresh_state_with_price(10.0);
    state.mode = rumi_protocol_backend::state::Mode::Recovery;
    assert_eq!(state.check_vaults_alert_threshold_key(), 16_000);
}

#[test]
fn dos_005_threshold_widens_when_admin_widens_band() {
    let mut state = fresh_state_with_price(10.0);
    // Default 1000 bps → threshold 14_300.
    assert_eq!(state.check_vaults_alert_threshold_key(), 14_300);
    // Widen to 5000 bps (50%) → threshold 1.33 + 0.50 = 1.83 → key 18_300.
    state.set_check_vaults_alert_band_bps(5000);
    assert_eq!(state.check_vaults_alert_threshold_key(), 18_300);
}

// ============================================================================
// Layer 3 — tick advance / full-sweep cadence
// ============================================================================

#[test]
fn dos_005_advance_tick_with_default_k_runs_band_for_first_11_ticks() {
    let mut state = fresh_state_with_price(10.0);
    // Default K=12: ticks 1..=11 are band-only; tick 12 is full sweep.
    for tick in 1..=11u64 {
        let do_full = state.advance_check_vaults_tick();
        assert!(
            !do_full,
            "tick {} of 12 should be band-only (K=12, default cadence)",
            tick
        );
    }
    let do_full = state.advance_check_vaults_tick();
    assert!(do_full, "tick 12 of 12 should be the full sweep");
    // After full sweep, counter resets — next 11 ticks are band again.
    for tick in 1..=11u64 {
        let do_full = state.advance_check_vaults_tick();
        assert!(
            !do_full,
            "tick {} after full sweep should be band-only",
            tick
        );
    }
    assert!(
        state.advance_check_vaults_tick(),
        "12 ticks past full sweep must trigger another full sweep"
    );
}

#[test]
fn dos_005_advance_tick_with_k_3_runs_band_band_full() {
    let mut state = fresh_state_with_price(10.0);
    state.set_check_vaults_full_sweep_every_n_ticks(3);
    assert!(!state.advance_check_vaults_tick(), "K=3 tick 1: band only");
    assert!(!state.advance_check_vaults_tick(), "K=3 tick 2: band only");
    assert!(state.advance_check_vaults_tick(), "K=3 tick 3: full sweep");
    assert!(!state.advance_check_vaults_tick(), "K=3 tick 4: band only");
    assert!(!state.advance_check_vaults_tick(), "K=3 tick 5: band only");
    assert!(state.advance_check_vaults_tick(), "K=3 tick 6: full sweep");
}

#[test]
fn dos_005_k_zero_means_always_full_sweep() {
    // K=0 disables the optimization (every tick is full sweep =
    // pre-Wave-9c behavior). This is the safe revert path if production
    // reveals an issue.
    let mut state = fresh_state_with_price(10.0);
    state.set_check_vaults_full_sweep_every_n_ticks(0);
    for _ in 0..5 {
        assert!(
            state.advance_check_vaults_tick(),
            "K=0 must full-sweep every tick"
        );
    }
}

#[test]
fn dos_005_k_one_means_always_full_sweep() {
    let mut state = fresh_state_with_price(10.0);
    state.set_check_vaults_full_sweep_every_n_ticks(1);
    for _ in 0..5 {
        assert!(
            state.advance_check_vaults_tick(),
            "K=1 must full-sweep every tick"
        );
    }
}

// ============================================================================
// Layer 4 — scan correctness (the core contract)
// ============================================================================

#[test]
fn dos_005_band_only_tick_skips_above_threshold_vaults() {
    // 50 vaults total: half safely above the band (CR 5.0), half at risk
    // (CR 1.40, just inside the 1.43 default threshold). On a band-only
    // tick, the scanner must visit only the 25 at-risk vaults, not the
    // other 25.
    let mut state = fresh_state_with_price(10.0);

    // 25 at-risk vaults: 1 ICP at $10, debt ~ 7.143 icUSD → CR ~ 1.40
    for i in 1..=25u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 714_000_000));
    }
    // 25 safely-above vaults: 1 ICP at $10, debt = 2 icUSD → CR = 5.0
    for i in 26..=50u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 200_000_000));
    }

    let scan: UnhealthyVaultScan = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert!(!scan.was_full_sweep);
    assert_eq!(
        scan.vaults_visited, 25,
        "band-only tick must visit only the 25 at-risk vaults; got {}",
        scan.vaults_visited
    );
}

#[test]
fn dos_005_full_sweep_tick_visits_every_vault() {
    // Same setup; full-sweep tick must visit all 50.
    let mut state = fresh_state_with_price(10.0);
    for i in 1..=25u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 714_000_000));
    }
    for i in 26..=50u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 200_000_000));
    }

    let scan = state.scan_unhealthy_vaults(dummy_rate(), true);
    assert!(scan.was_full_sweep);
    assert_eq!(scan.vaults_visited, 50);
}

#[test]
fn dos_005_band_only_tick_finds_underwater_vaults_at_index_bottom() {
    // 1 deeply underwater vault (CR 1.10), 5 safe vaults (CR 5.0).
    // Even on a band-only tick, the underwater vault must surface in the
    // unhealthy list — it's at the bottom of the index, well within band.
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 909_090_909)); // CR ~1.10
    for i in 2..=6u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 200_000_000)); // CR 5.0
    }

    let scan = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        scan.unhealthy_vaults.len(),
        1,
        "underwater vault must be found on first band tick"
    );
    assert_eq!(scan.unhealthy_vaults[0].vault_id, 1);
}

#[test]
fn dos_005_band_visit_count_grows_after_user_activity_pushes_vaults_into_band() {
    // Vaults at CR 1.6 (above default 1.43 threshold). Band tick visits 0.
    // After three vaults borrow more icUSD (re-keying their CR into the
    // band), the next band tick visits exactly those three. Pins the
    // contract that the band reflects current re-keyed state.
    let mut state = fresh_state_with_price(10.0);
    for i in 1..=5u64 {
        open_and_reindex(&mut state, make_vault(i, 100_000_000, 625_000_000)); // CR 1.6
    }

    let scan_before = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        scan_before.vaults_visited, 0,
        "tick before borrow: all vaults above band"
    );

    // Three vaults borrow more, dropping CR from 1.6 to ~1.25.
    // Existing debt 6.25 icUSD; borrow another 1.75 icUSD → debt 8 icUSD →
    // CR = 10 / 8 = 1.25 (below 1.33 liquidation_ratio: now liquidatable).
    for vid in 1..=3u64 {
        state.borrow_from_vault(vid, ICUSD::new(175_000_000));
        state.reindex_vault_cr(vid);
    }

    let scan_after = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        scan_after.vaults_visited, 3,
        "tick after borrow: 3 vaults pulled into band"
    );
    assert_eq!(
        scan_after.unhealthy_vaults.len(),
        3,
        "all three are now below liquidation_ratio"
    );
}

#[test]
fn dos_005_band_only_and_full_sweep_agree_on_unhealthy_set_for_in_band_vaults() {
    // The band-only tick must produce the same unhealthy set as a full
    // sweep, *for vaults whose CR-key is inside the band*. This is the
    // "no false negatives within the band" guarantee.
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 909_090_909)); // CR 1.10 (underwater)
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 800_000_000)); // CR 1.25 (underwater)
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 714_000_000)); // CR ~1.40 (in-band, healthy)
    open_and_reindex(&mut state, make_vault(4, 100_000_000, 200_000_000)); // CR 5.0 (above)

    let band = state.scan_unhealthy_vaults(dummy_rate(), false);
    let full = state.scan_unhealthy_vaults(dummy_rate(), true);

    let mut band_ids: Vec<u64> = band.unhealthy_vaults.iter().map(|v| v.vault_id).collect();
    let mut full_ids: Vec<u64> = full.unhealthy_vaults.iter().map(|v| v.vault_id).collect();
    band_ids.sort();
    full_ids.sort();
    assert_eq!(
        band_ids, full_ids,
        "band tick and full sweep must agree on the unhealthy set when no \
         vault drifts above the band threshold"
    );
}

// ============================================================================
// Layer 5 — drift safety belt
// ============================================================================

#[test]
fn dos_005_full_sweep_catches_stale_keyed_drifted_vault_band_misses() {
    // Cross-collateral drift scenario: a vault was healthy when last
    // re-keyed (key well above threshold), but its current CR (recomputed
    // from the cached price) is now underwater. The band tick misses it
    // (key is above threshold); the safety-belt full sweep catches it.
    //
    // This pins the contract that the periodic full sweep is the
    // mitigation for stale-key drift, not just an optimization knob.
    let mut state = fresh_state_with_price(10.0);
    // Vault opened at CR 1.6 (key 16_000) — above default 14_300 threshold.
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 625_000_000));

    // Price drops to $7. Real CR = $7 / $6.25 = 1.12 (deeply underwater),
    // but `vault_cr_index` key stays at 16_000 because the price-update
    // path does NOT re-key (per LIQ-002 SAFETY contract).
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(7.0);
    }

    // Band tick misses the now-underwater vault (its key 16_000 is above
    // the 14_300 threshold).
    let band = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        band.unhealthy_vaults.len(),
        0,
        "band tick misses stale-keyed drifted vault (this is the documented limitation)"
    );

    // Full sweep walks every key, including 16_000, and finds the drifted
    // vault by recomputing CR with the current cached price.
    let full = state.scan_unhealthy_vaults(dummy_rate(), true);
    assert_eq!(
        full.unhealthy_vaults.len(),
        1,
        "full sweep catches drifted vault via real-time CR recompute"
    );
    assert_eq!(full.unhealthy_vaults[0].vault_id, 1);
}

// ============================================================================
// Layer 6 — admin tunables
// ============================================================================

#[test]
fn dos_005_admin_widening_band_visits_more_vaults_next_tick() {
    let mut state = fresh_state_with_price(10.0);
    // Vault at CR 1.6 — outside default 1000 bps band.
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 625_000_000));

    let scan_before = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(scan_before.vaults_visited, 0);

    // Widen to 5000 bps (50%) → threshold 1.83 → key 18_300. Now vault 1
    // (key 16_000) is inside the band.
    state.set_check_vaults_alert_band_bps(5000);
    let scan_after = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        scan_after.vaults_visited, 1,
        "widened band visits the previously-out-of-band vault"
    );
}

#[test]
fn dos_005_admin_lowering_full_sweep_cadence_speeds_up_drift_recovery() {
    let mut state = fresh_state_with_price(10.0);
    // K=2: every other tick is full sweep.
    state.set_check_vaults_full_sweep_every_n_ticks(2);
    assert!(!state.advance_check_vaults_tick());
    assert!(state.advance_check_vaults_tick());
    assert!(!state.advance_check_vaults_tick());
    assert!(state.advance_check_vaults_tick());
}

// ============================================================================
// Layer 7 — upgrade hygiene (serde defaults)
// ============================================================================

#[test]
fn dos_005_state_default_initializes_new_fields() {
    // The `Default` impl on State is used as the serde fallback for
    // missing fields in pre-Wave-9c CBOR snapshots. Each new field must
    // resolve to the audited default.
    let s = State::default();
    assert_eq!(s.check_vaults_alert_band_bps, DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS);
    assert_eq!(
        s.check_vaults_full_sweep_every_n_ticks,
        DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS
    );
    assert_eq!(s.ticks_since_full_sweep, 0);
}

#[test]
fn dos_005_simulated_pre_upgrade_snapshot_decodes_to_defaults() {
    // Simulate a pre-Wave-9c snapshot: the three new fields default via
    // their `serde(default)` annotation. We can't easily round-trip CBOR
    // here without mocking storage, but we can fence the contract by
    // checking that `State::default()` (which IS the serde fallback)
    // produces the same values as a fresh `From<InitArg>`.
    let from_init = fresh_state_with_price(10.0);
    let from_default = State::default();
    assert_eq!(
        from_init.check_vaults_alert_band_bps,
        from_default.check_vaults_alert_band_bps,
        "Default impl must agree with From<InitArg> on alert band"
    );
    assert_eq!(
        from_init.check_vaults_full_sweep_every_n_ticks,
        from_default.check_vaults_full_sweep_every_n_ticks,
        "Default impl must agree with From<InitArg> on full-sweep cadence"
    );
}

// ============================================================================
// Layer 8 — interaction with bot_processing flag
// ============================================================================

#[test]
fn dos_005_bot_processing_vaults_skipped_in_unhealthy_list_but_still_counted_visited() {
    // The pre-Wave-9c logic skipped `bot_processing` vaults via `continue`
    // before classifying. Wave-9c preserves that behavior — bot-claimed
    // vaults are not double-published — but the visit counter still
    // increments because the cycle cost was paid (we read the vault to
    // check the flag).
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 909_090_909)); // CR 1.10 unhealthy
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 800_000_000)); // CR 1.25 unhealthy
    if let Some(v) = state.vault_id_to_vaults.get_mut(&2) {
        v.bot_processing = true;
    }

    let scan = state.scan_unhealthy_vaults(dummy_rate(), false);
    assert_eq!(
        scan.unhealthy_vaults.len(),
        1,
        "bot_processing vault not republished"
    );
    assert_eq!(scan.unhealthy_vaults[0].vault_id, 1);
    assert_eq!(
        scan.vaults_visited, 2,
        "both vaults visited (bot_processing flag read counts as a visit)"
    );
}

// ============================================================================
// Layer 9 — Ratio-based fence using public API
// ============================================================================

#[test]
fn dos_005_ratio_helper_round_trips_band_setter() {
    let mut state = fresh_state_with_price(10.0);
    // 0 bps = no headroom; threshold is exactly the worst min_liq_ratio.
    state.set_check_vaults_alert_band_bps(0);
    assert_eq!(state.check_vaults_alert_threshold_key(), 13_300);
    // 10000 bps (100%) → threshold 1.33 + 1.0 = 2.33 → key 23_300.
    state.set_check_vaults_alert_band_bps(10_000);
    assert_eq!(state.check_vaults_alert_threshold_key(), 23_300);
    // Sanity: setter is idempotent (no Ratio drift).
    state.set_check_vaults_alert_band_bps(1000);
    assert_eq!(state.check_vaults_alert_threshold_key(), 14_300);
}

// Helper: keep the unused-import warning at bay if Ratio isn't used in the
// final assertion list above.
#[allow(dead_code)]
fn _ratio_anchor() -> Ratio {
    Ratio::new(dec!(1.0))
}
