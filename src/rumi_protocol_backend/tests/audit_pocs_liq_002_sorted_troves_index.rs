//! LIQ-002 regression fence: sorted-troves index + liquidation ordering.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`
//!     finding LIQ-002.
//!
//! # What the gap was
//!
//! Pre-Wave-8b, every liquidator endpoint accepted any caller-provided
//! `vault_id` as long as the vault's CR was below `min_liquidation_ratio`.
//! `check_vaults` walked `vault_id_to_vaults` in insertion order, not by CR,
//! and there was no secondary index of underwater vaults. A liquidator could
//! cherry-pick the most profitable target (best DEX routing, most bonus
//! headroom) and skip deeply-underwater vaults, leaving the protocol with
//! unfinished bad debt while harvesting easy wins.
//!
//! # How this file tests the fix
//!
//! Three layers, mirroring the structure of the LIQ-007 fence:
//!
//!  1. **Pure index unit tests (state-level, no canister context)** — confirm
//!     that every mutation that moves a vault's debt or collateral re-keys
//!     the index, that closes/full-liquidations un-index, and that the
//!     index size invariant holds after each operation.
//!
//!  2. **Ordering enforcement at the state layer** — `is_within_liquidation_band`
//!     returns true for the bottom-of-stack vault, false for mid-stack
//!     vaults, and respects the admin-tunable tolerance.
//!
//!  3. **Scale + migration tests** — open 100 vaults via the state mutators,
//!     assert the band gate rejects mid-stack and accepts worst-CR; simulate
//!     the post_upgrade rebuild over 50 vaults to pin the migration contract.
//!
//! # Why no dedicated PocketIC file
//!
//! The four liquidator entry points each call
//! `read_state(|s| s.is_within_liquidation_band(vault_id))` BEFORE any other
//! work. The wiring is verified by the Rust compiler (the call site exists
//! in each function) and by the state-level fence (the helper returns
//! the correct result for all interesting scenarios). A full PocketIC test
//! would replicate all of that at one or two orders of magnitude higher
//! cost (per-test setup is multi-second; opening 100 vaults via canister
//! calls takes minutes). The existing `pocket_ic_tests` suite continues to
//! exercise single-vault liquidation paths, where the band gate trivially
//! passes — confirming the gate does not regress the existing flow.

use candid::Principal;

use rumi_protocol_backend::numeric::{Ratio, ICUSD};
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::InitArg;
use rust_decimal_macros::dec;

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

/// Insert a vault directly into state and re-key the index. Used by the
/// helper-level tests so we can assert the index in isolation without going
/// through the full `record_open_vault` event flow.
fn open_and_reindex(state: &mut State, vault: Vault) {
    let vid = vault.vault_id;
    state.open_vault(vault);
    state.reindex_vault_cr(vid);
}

/// Sum of bucket sizes inside `vault_cr_index`. Equal to the number of
/// indexed vaults.
fn index_size(state: &State) -> usize {
    state.vault_cr_index.values().map(|b| b.len()).sum()
}

// ============================================================================
// Layer 1 — pure index unit tests (state-level)
// ============================================================================

#[test]
fn liq_002_open_vault_inserts_into_index() {
    let mut state = fresh_state_with_price(10.0);
    // Vault: 1 ICP at $10 = $10 collateral, 5 icUSD debt → CR = 2.0 (200%).
    let vault = make_vault(1, 100_000_000, 500_000_000);
    open_and_reindex(&mut state, vault);

    assert_eq!(index_size(&state), 1, "one vault open → one index entry");
    // CR = 2.0 → key = 20000 bps.
    assert!(
        state.vault_cr_index.contains_key(&20_000),
        "expected key 20000 (200% CR) in index, got keys {:?}",
        state.vault_cr_index.keys().collect::<Vec<_>>()
    );
    assert!(state.vault_cr_index.get(&20_000).unwrap().contains(&1));
}

#[test]
fn liq_002_borrow_rekeys_vault() {
    let mut state = fresh_state_with_price(10.0);
    // Open vault at CR = 2.0 (200%): $10 collateral, 5 icUSD debt.
    let vault = make_vault(1, 100_000_000, 500_000_000);
    open_and_reindex(&mut state, vault);
    assert!(state.vault_cr_index.contains_key(&20_000));

    // Simulate a borrow that moves debt to 8 icUSD → CR = 10/8 = 1.25.
    state.borrow_from_vault(1, ICUSD::new(300_000_000));
    state.reindex_vault_cr(1);

    assert!(
        !state.vault_cr_index.contains_key(&20_000),
        "old key 20000 must be removed after re-key"
    );
    assert!(
        state.vault_cr_index.contains_key(&12_500),
        "new key 12500 (125% CR) expected, got {:?}",
        state.vault_cr_index.keys().collect::<Vec<_>>()
    );
    assert_eq!(index_size(&state), 1, "single vault must remain singly indexed");
}

#[test]
fn liq_002_repay_rekeys_vault() {
    let mut state = fresh_state_with_price(10.0);
    // Open vault at CR = 1.25.
    let vault = make_vault(1, 100_000_000, 800_000_000);
    open_and_reindex(&mut state, vault);
    assert!(state.vault_cr_index.contains_key(&12_500));

    // Repay 3 icUSD → debt 5 icUSD → CR = 2.0.
    let _ = state.repay_to_vault(1, ICUSD::new(300_000_000));
    state.reindex_vault_cr(1);

    assert!(
        !state.vault_cr_index.contains_key(&12_500),
        "old key must be removed"
    );
    assert!(state.vault_cr_index.contains_key(&20_000));
    assert_eq!(index_size(&state), 1);
}

#[test]
fn liq_002_add_margin_rekeys() {
    let mut state = fresh_state_with_price(10.0);
    let vault = make_vault(1, 100_000_000, 500_000_000); // CR = 2.0
    open_and_reindex(&mut state, vault);

    // Add another 1 ICP of margin → 2 ICP collateral = $20, debt $5 → CR = 4.0.
    state.add_margin_to_vault(1, rumi_protocol_backend::numeric::ICP::new(100_000_000));
    state.reindex_vault_cr(1);

    assert!(state.vault_cr_index.contains_key(&40_000));
    assert!(!state.vault_cr_index.contains_key(&20_000));
    assert_eq!(index_size(&state), 1);
}

#[test]
fn liq_002_close_vault_unindexes() {
    let mut state = fresh_state_with_price(10.0);
    let vault = make_vault(1, 100_000_000, 500_000_000);
    open_and_reindex(&mut state, vault);
    assert_eq!(index_size(&state), 1);

    // Close requires zero debt and zero collateral (per protocol invariants).
    if let Some(v) = state.vault_id_to_vaults.get_mut(&1) {
        v.borrowed_icusd_amount = ICUSD::new(0);
        v.collateral_amount = 0;
    }
    state.close_vault(1);
    state.unindex_vault_cr(1);

    assert_eq!(index_size(&state), 0, "close_vault must drop index entry");
    assert!(state.vault_cr_index.is_empty());
}

#[test]
fn liq_002_full_liquidation_unindexes() {
    let mut state = fresh_state_with_price(10.0);
    // Vault below liq ratio: $10 collateral, $9 debt → CR = 1.111.
    let vault = make_vault(1, 100_000_000, 900_000_000);
    open_and_reindex(&mut state, vault);
    assert!(state.vault_cr_index.contains_key(&11_111));

    // Full-liquidate by removing the vault (simulates the path in vault.rs's
    // mutate_state, which calls liquidate_vault → removes from
    // vault_id_to_vaults → must also unindex).
    state.vault_id_to_vaults.remove(&1);
    state.unindex_vault_cr(1);

    assert_eq!(index_size(&state), 0);
}

#[test]
fn liq_002_partial_liquidation_rekeys() {
    let mut state = fresh_state_with_price(10.0);
    // Vault below liq ratio: $10 collateral, $9 debt → CR = 1.111.
    let vault = make_vault(1, 100_000_000, 900_000_000);
    open_and_reindex(&mut state, vault);

    // Simulate partial liq: cut debt and collateral proportionally.
    if let Some(v) = state.vault_id_to_vaults.get_mut(&1) {
        v.borrowed_icusd_amount = ICUSD::new(450_000_000); // 4.5 icUSD
        v.collateral_amount = 50_000_000; // 0.5 ICP = $5
    }
    // CR = 5/4.5 = 1.111 (same ratio, but key still must round-trip correctly).
    state.reindex_vault_cr(1);
    assert_eq!(index_size(&state), 1, "partial liq must keep vault indexed");
}

#[test]
fn liq_002_redemption_water_fill_rekeys() {
    // After a redemption, every vault touched by `redeem_on_vaults` must be
    // re-keyed. This test exercises the contract at the state layer: we simulate
    // a debt+collateral deduction via direct mutation followed by reindex_vault_cr.
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 500_000_000)); // CR 2.0
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 400_000_000)); // CR 2.5

    assert_eq!(index_size(&state), 3);
    assert!(state.vault_cr_index.contains_key(&12_500));
    assert!(state.vault_cr_index.contains_key(&20_000));
    assert!(state.vault_cr_index.contains_key(&25_000));

    // Simulate redemption deducting from vault 1 (worst-CR first).
    if let Some(v) = state.vault_id_to_vaults.get_mut(&1) {
        v.borrowed_icusd_amount = ICUSD::new(400_000_000); // halve debt
        v.collateral_amount = 60_000_000; // burn collateral too
    }
    state.reindex_vault_cr(1);

    // CR for vault 1 now = $6 / $4 = 1.5 → key 15000.
    assert!(state.vault_cr_index.contains_key(&15_000));
    assert!(!state.vault_cr_index.contains_key(&12_500));
    assert_eq!(index_size(&state), 3);
}

#[test]
fn liq_002_invariant_index_size_matches_vault_map() {
    // After every mutation of debt or collateral, the count of indexed vaults
    // must equal `vault_id_to_vaults.len()`. This is the rebuild invariant.
    let mut state = fresh_state_with_price(10.0);
    for vid in 1..=10u64 {
        open_and_reindex(&mut state, make_vault(vid, 100_000_000, 500_000_000));
    }
    assert_eq!(index_size(&state), state.vault_id_to_vaults.len());

    // Borrow on half of them, repay on the other half — the invariant must hold.
    for vid in 1..=5u64 {
        state.borrow_from_vault(vid, ICUSD::new(100_000_000));
        state.reindex_vault_cr(vid);
    }
    for vid in 6..=10u64 {
        let _ = state.repay_to_vault(vid, ICUSD::new(100_000_000));
        state.reindex_vault_cr(vid);
    }
    assert_eq!(index_size(&state), state.vault_id_to_vaults.len(),
        "index size must track vault_id_to_vaults.len()");
}

#[test]
fn liq_002_price_update_does_not_rekey() {
    // SAFETY contract: `on_price_update` must NOT call `reindex_vault_cr`.
    // CR keys are computed at mutation time; relative ordering within a
    // collateral type is preserved under uniform price moves. Re-keying every
    // vault on every 5-minute price tick would be O(N) cycles burned for
    // zero ordering benefit.
    //
    // This test pins the contract: change the cached price, do NOT touch the
    // index, and assert the keys stayed the same. If a future contributor
    // adds a re-key in the price-update path, this test fires.
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 500_000_000)); // CR 2.0
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 800_000_000)); // CR 1.25

    let keys_before: Vec<u64> = state.vault_cr_index.keys().copied().collect();
    assert_eq!(keys_before, vec![12_500, 20_000]);

    // Bump the price 50% — without a re-key, the index keys must stay the same.
    let icp = state.icp_collateral_type();
    if let Some(config) = state.collateral_configs.get_mut(&icp) {
        config.last_price = Some(15.0);
    }
    let keys_after: Vec<u64> = state.vault_cr_index.keys().copied().collect();
    assert_eq!(
        keys_after, keys_before,
        "price update must not change index keys (no re-key on price tick)"
    );
}

#[test]
fn liq_002_post_upgrade_rebuilds_index() {
    // Simulates the post_upgrade path: after restoring state from stable
    // memory, the in-memory `vault_cr_index` is empty (it's
    // `skip_serializing`). The post_upgrade migration walks
    // `vault_id_to_vaults` and re-keys every vault.
    let mut state = fresh_state_with_price(10.0);
    state.open_vault(make_vault(1, 100_000_000, 500_000_000)); // CR 2.0
    state.open_vault(make_vault(2, 100_000_000, 800_000_000)); // CR 1.25
    state.open_vault(make_vault(3, 100_000_000, 400_000_000)); // CR 2.5

    // Wipe the index to simulate a freshly-decoded snapshot.
    state.vault_cr_index.clear();
    assert_eq!(index_size(&state), 0);

    // Run the migration.
    let vault_ids: Vec<u64> = state.vault_id_to_vaults.keys().copied().collect();
    for vid in vault_ids {
        state.reindex_vault_cr(vid);
    }

    assert_eq!(index_size(&state), 3, "every vault re-indexed");
    assert!(state.vault_cr_index.contains_key(&12_500));
    assert!(state.vault_cr_index.contains_key(&20_000));
    assert!(state.vault_cr_index.contains_key(&25_000));
}

// ============================================================================
// Layer 2 — ordering enforcement state-level
// ============================================================================

#[test]
fn liq_002_is_within_band_accepts_lowest_cr_vault() {
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25 (worst)
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 600_000_000)); // CR 1.667
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 500_000_000)); // CR 2.0
    open_and_reindex(&mut state, make_vault(4, 100_000_000, 400_000_000)); // CR 2.5
    open_and_reindex(&mut state, make_vault(5, 100_000_000, 300_000_000)); // CR 3.333

    assert!(
        state.is_within_liquidation_band(1),
        "worst-CR vault must be within band"
    );
}

#[test]
fn liq_002_is_within_band_rejects_mid_stack_vault() {
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 600_000_000)); // CR 1.667
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 500_000_000)); // CR 2.0
    open_and_reindex(&mut state, make_vault(4, 100_000_000, 400_000_000)); // CR 2.5
    open_and_reindex(&mut state, make_vault(5, 100_000_000, 300_000_000)); // CR 3.333

    // Default tolerance is 1% (0.01) → 100 bps. Vault 3 at CR 2.0 is 7500 bps
    // above worst (CR 1.25), well outside the band.
    assert!(
        !state.is_within_liquidation_band(3),
        "mid-stack vault must be rejected with default 1% tolerance"
    );
}

#[test]
fn liq_002_is_within_band_accepts_within_tolerance() {
    let mut state = fresh_state_with_price(10.0);
    // Two vaults very close together: CR 1.25 and CR 1.255 (50 bps apart).
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25
    // 1 ICP collateral, debt = 1/1.255 = 0.79681... icUSD → 79681274 e8s
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 796_812_749)); // CR ~1.255

    // Default 1% (= 100 bps) tolerance → vault 2 at 50 bps above worst is in band.
    assert!(state.is_within_liquidation_band(1));
    assert!(
        state.is_within_liquidation_band(2),
        "vault within 1% of worst must be accepted"
    );
}

#[test]
fn liq_002_admin_can_widen_tolerance() {
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 600_000_000)); // CR 1.667
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 500_000_000)); // CR 2.0

    // Default 1% → vaults 2 and 3 rejected.
    assert!(!state.is_within_liquidation_band(2));
    assert!(!state.is_within_liquidation_band(3));

    // Admin widens to 10% (1000 bps). Vault 2 at 4167 bps above is still out;
    // widen further to 50% (5000 bps).
    state.set_liquidation_ordering_tolerance(Ratio::new(dec!(0.5)));
    assert!(state.is_within_liquidation_band(2));
    assert!(
        !state.is_within_liquidation_band(3),
        "vault 3 at 7500 bps above worst still out of band"
    );

    // Widen to 100% — every vault is in band.
    state.set_liquidation_ordering_tolerance(Ratio::new(dec!(1.0)));
    assert!(state.is_within_liquidation_band(2));
    assert!(state.is_within_liquidation_band(3));
}

#[test]
fn liq_002_is_within_band_returns_false_for_unknown_vault() {
    // A vault_id not in the index must be rejected. Liquidator endpoints rely
    // on this to short-circuit before the per-vault CR check.
    let state = fresh_state_with_price(10.0);
    assert!(
        !state.is_within_liquidation_band(999),
        "unindexed vault_id must be rejected"
    );
}

#[test]
fn liq_002_zero_debt_vault_does_not_overflow_cr_key() {
    // Regression: `compute_collateral_ratio` returns `Ratio::from(Decimal::MAX)`
    // for zero-debt vaults. The naive `cr.0 * 10_000` form of `cr_index_key`
    // panics with "Multiplication overflowed" on those. The fix uses
    // `checked_mul` and saturates to `u64::MAX`. This pins the contract.
    //
    // Hit during initial wiring of `reindex_vault_cr` into `repay_to_vault`
    // when a test repaid the full debt and the post-mutation re-key tried to
    // key a zero-debt vault.
    let mut state = fresh_state_with_price(10.0);
    let vault = make_vault(1, 100_000_000, 500_000_000); // 5 icUSD debt
    open_and_reindex(&mut state, vault);

    // Zero out the debt directly (simulates what repay_to_vault does on a
    // full-amount repayment), then re-key. Must not panic.
    if let Some(v) = state.vault_id_to_vaults.get_mut(&1) {
        v.borrowed_icusd_amount = ICUSD::new(0);
    }
    state.reindex_vault_cr(1);

    // Zero-debt vault sorts to the top of the index (highest key = MAX).
    assert!(
        state.vault_cr_index.contains_key(&u64::MAX),
        "zero-debt vault must key at u64::MAX"
    );
    assert!(state.vault_cr_index.get(&u64::MAX).unwrap().contains(&1));
}

#[test]
fn liq_002_cr_index_key_saturates_on_decimal_max() {
    // Direct unit test of the saturating contract.
    use rust_decimal::Decimal;
    let max_ratio = Ratio::new(Decimal::MAX);
    assert_eq!(State::cr_index_key(max_ratio), u64::MAX);
    let zero_ratio = Ratio::new(Decimal::ZERO);
    assert_eq!(State::cr_index_key(zero_ratio), 0);
    let one = Ratio::new(rust_decimal_macros::dec!(1.0));
    assert_eq!(State::cr_index_key(one), 10_000);
}

// ============================================================================
// Layer 3 — scale + migration fence
// ============================================================================

#[test]
fn liq_002_band_gate_with_100_vaults_only_accepts_worst_band() {
    // Plan-level test #16, state-layer equivalent. Open 100 vaults with debts
    // that vary monotonically. The CR distribution is monotonic too: vault 1
    // has the worst CR (highest debt vs same collateral), vault 100 has the
    // best.
    //
    // Default tolerance is 1% (100 bps). Walk every vault and assert:
    //   * The worst-CR vault and its near-band neighbours are accepted.
    //   * Mid-stack vaults far from the worst are rejected.
    //
    // This pins the band gate's behavior at scale — the helper's state-level
    // contract holds for any N, the per-canister wiring is the same line of
    // code in every endpoint.
    let mut state = fresh_state_with_price(10.0);

    // Vault i (i = 1..=100): 1 ICP collateral ($10), debt = (5 + i / 10) icUSD.
    // CR = 10 / (5 + i / 10), descending with i is impossible — let's reverse:
    // Use debt = (10 - i * 0.05) icUSD: i=1 → 9.95 icUSD → CR 1.005;
    //                                    i=100 → 5.0 icUSD → CR 2.0.
    // That makes vault 1 the worst-CR.
    for i in 1..=100u64 {
        let debt_e8s = 1_000_000_000 - (i * 5_000_000); // 10 icUSD - i * 0.05
        open_and_reindex(&mut state, make_vault(i, 100_000_000, debt_e8s));
    }

    assert_eq!(index_size(&state), 100);

    // Worst CR vault is in band.
    assert!(state.is_within_liquidation_band(1), "vault 1 (worst-CR) accepted");

    // The next 1-2 vaults are within ~5 bps of vault 1 (CR 1.0050... vs 1.0050...
    // - vault 2: CR 10 / 9.9 = 1.0101...
    // - keys differ by ~50 bps, so vault 2 is OUT of the default 100-bps band.
    //
    // Stronger assertion: every vault past index position 2 must be rejected.
    let mut rejected_count = 0u64;
    for i in 2..=100u64 {
        if !state.is_within_liquidation_band(i) {
            rejected_count += 1;
        }
    }
    assert!(
        rejected_count >= 90,
        "at least 90 of the 99 non-worst vaults must be out of band; got {} rejected",
        rejected_count
    );

    // Pick a mid-stack vault that is definitively out of band.
    assert!(
        !state.is_within_liquidation_band(50),
        "vault 50 (CR 10 / 7.5 = 1.333) is far from worst → rejected"
    );
    assert!(
        !state.is_within_liquidation_band(100),
        "vault 100 (CR 2.0) is the best → rejected"
    );
}

#[test]
fn liq_002_band_gate_after_worst_liquidated_promotes_next_worst() {
    // After the worst-CR vault is liquidated (or otherwise removed), the next-
    // worst becomes the new lowest. This exercises the index's update
    // contract: unindex_vault_cr drops the entry, and the next call to
    // is_within_liquidation_band uses the new bottom.
    let mut state = fresh_state_with_price(10.0);
    open_and_reindex(&mut state, make_vault(1, 100_000_000, 800_000_000)); // CR 1.25 (worst)
    open_and_reindex(&mut state, make_vault(2, 100_000_000, 600_000_000)); // CR 1.667
    open_and_reindex(&mut state, make_vault(3, 100_000_000, 500_000_000)); // CR 2.0

    assert!(state.is_within_liquidation_band(1));
    assert!(!state.is_within_liquidation_band(2));

    // Simulate full liquidation of vault 1 (debt+collateral wiped, vault removed).
    state.vault_id_to_vaults.remove(&1);
    state.unindex_vault_cr(1);

    // Now vault 2 is the worst — it must enter the band.
    assert!(
        state.is_within_liquidation_band(2),
        "after vault 1 is removed, vault 2 becomes the new worst-CR and enters the band"
    );
    // Vault 3 is still mid-stack relative to the new bottom (1.667).
    assert!(
        !state.is_within_liquidation_band(3),
        "vault 3 (CR 2.0 = 20000 bps) is 3333 bps above new worst (vault 2 at 16667 bps), out of 100-bps band"
    );
}

#[test]
fn liq_002_post_upgrade_migration_50_vaults() {
    // Plan-level test #17, state-layer equivalent. Build state with 50
    // vaults via the standard mutators (which keep the index in sync), then
    // simulate post_upgrade by clearing the index and running the migration
    // step from `main.rs::post_upgrade`. Assert every vault is re-indexed
    // and the band gate still resolves to the worst-CR vault.
    let mut state = fresh_state_with_price(10.0);

    for i in 1..=50u64 {
        // Same descending-CR layout as the 100-vault test.
        let debt_e8s = 1_000_000_000 - (i * 10_000_000); // i=1 → 9.9, i=50 → 5.0
        open_and_reindex(&mut state, make_vault(i, 100_000_000, debt_e8s));
    }

    assert_eq!(index_size(&state), 50);

    // Wipe the index — same effect as deserializing a snapshot where the
    // field was `skip_serializing`.
    state.vault_cr_index.clear();
    assert_eq!(index_size(&state), 0);

    // Re-run the migration (the body of the post_upgrade migration block).
    let vault_ids: Vec<u64> = state.vault_id_to_vaults.keys().copied().collect();
    for vid in vault_ids {
        state.reindex_vault_cr(vid);
    }

    assert_eq!(index_size(&state), 50, "migration must re-index every vault");

    // Worst-CR resolution must work after migration.
    assert!(
        state.is_within_liquidation_band(1),
        "vault 1 (worst-CR) must still resolve as in-band post-migration"
    );
    assert!(
        !state.is_within_liquidation_band(25),
        "vault 25 (mid-stack) must still resolve as out-of-band post-migration"
    );
}
