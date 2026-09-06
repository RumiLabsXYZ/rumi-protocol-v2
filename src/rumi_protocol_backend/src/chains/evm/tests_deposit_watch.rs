use super::deposit_watch::{advance_cursor_and_prune, apply_burn_to_state, credit_deposit_to_state, BurnApplyError};
use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainState;
use crate::chains::monad::evm_rpc::BurnLog;
use crate::chains::supply::SupplyInvariantError;
use crate::chains::vault::{LiquidationTier, PendingLiquidationV1};
use candid::Principal;

fn seeded() -> MultiChainState {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(), collateral_amount_native: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    s
}

#[test]
fn credit_deposit_increments_collateral() {
    let mut s = seeded();
    credit_deposit_to_state(&mut s, 1, 5_000_000_000_000_000_000).expect("credit");
    assert_eq!(s.chain_vaults[&1].collateral_amount_native, 5_000_000_000_000_000_000);
}

#[test]
fn burn_decrements_supply_and_debt_preserving_invariant() {
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 10_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 10_000_000_000);
    let total_debt = 10_000_000_000u128;
    let burn = BurnLog { vault_id: 1, amount_e8s: 4_000_000_000, tx_hash: "0xb".into(), block_number: 110 };
    apply_burn_to_state(&mut s, &burn, total_debt).expect("burn");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 6_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 6_000_000_000);
}

#[test]
fn burn_exceeding_debt_is_rejected_without_mutation() {
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 1_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 1_000_000_000);
    let burn = BurnLog { vault_id: 1, amount_e8s: 9_999_999_999, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 1_000_000_000);
    // Over-repay is a PERMANENT-INVALID burn → InvalidBurn (skippable).
    assert!(matches!(res, Err(BurnApplyError::InvalidBurn(_))), "got {res:?}");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 1_000_000_000); // unchanged
    assert_eq!(s.chain_supplies[&ChainId(10143)], 1_000_000_000); // unchanged
}

#[test]
fn burn_for_unknown_vault_is_rejected_as_invalid() {
    let mut s = seeded();
    let burn = BurnLog { vault_id: 999, amount_e8s: 1, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 0);
    // Unknown vault is a PERMANENT-INVALID burn → InvalidBurn (skippable).
    assert!(matches!(res, Err(BurnApplyError::InvalidBurn(_))), "got {res:?}");
}

#[test]
fn burn_returns_supply_invariant_when_already_halted_without_mutation() {
    // A burn whose amount equals the vault debt and matches the supply would
    // normally apply cleanly, but with the self-check halt already set,
    // apply_supply_delta returns HaltedAfterSelfCheckFailure → the typed error
    // is SupplyInvariant (HALT-CLASS, not skippable). Both fields stay untouched.
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 5_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 5_000_000_000);
    s.invariant_halted = true;
    let burn = BurnLog { vault_id: 1, amount_e8s: 4_000_000_000, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 5_000_000_000);
    assert!(
        matches!(
            res,
            Err(BurnApplyError::SupplyInvariant(SupplyInvariantError::HaltedAfterSelfCheckFailure))
        ),
        "got {res:?}"
    );
    // No-mutation-on-rejection: both fields unchanged even on the halt path.
    assert_eq!(s.chain_vaults[&1].debt_e8s, 5_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 5_000_000_000);
}

#[test]
fn burn_returns_supply_invariant_on_supply_divergence_without_mutation() {
    // debt exists and the per-vault debt check passes, but chain_supplies does
    // NOT match total_debt at call time → apply_supply_delta returns Divergence
    // → SupplyInvariant (HALT-CLASS). Confirms a halt-class failure is correctly
    // classified (NOT InvalidBurn) and leaves both fields untouched.
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 4_000_000_000;
    // Deliberately mismatched supply (3e9) vs the total_debt we pass (4e9).
    s.chain_supplies.insert(ChainId(10143), 3_000_000_000);
    let burn = BurnLog { vault_id: 1, amount_e8s: 1_000_000_000, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 4_000_000_000);
    assert!(
        matches!(res, Err(BurnApplyError::SupplyInvariant(SupplyInvariantError::Divergence { .. }))),
        "got {res:?}"
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 4_000_000_000); // unchanged
    assert_eq!(s.chain_supplies[&ChainId(10143)], 3_000_000_000); // unchanged
}

#[test]
fn advance_cursor_and_prune_sets_cursor_and_drops_keys_at_or_below_finalized() {
    use std::collections::BTreeSet;
    let mut s = seeded();
    // Seed processed_burn_keys at three blocks: 100, 150, 250.
    for b in [100u64, 150, 250] {
        let mut set = BTreeSet::new();
        set.insert(format!("0xtx{b}:0"));
        s.processed_burn_keys.insert(b, set);
    }

    advance_cursor_and_prune(&mut s, ChainId(10143), 200);

    // Cursor advanced to finalized.
    assert_eq!(s.last_observed_block.get(&ChainId(10143)).copied(), Some(200));
    // Keys at block <= 200 pruned (100, 150 gone); keys above 200 retained (250).
    assert!(!s.processed_burn_keys.contains_key(&100), "block 100 pruned");
    assert!(!s.processed_burn_keys.contains_key(&150), "block 150 pruned");
    assert!(s.processed_burn_keys.contains_key(&250), "block 250 > finalized retained");
}

#[test]
fn backstop_scans_only_on_supply_drop_and_no_inflight_mint() {
    use super::deposit_watch::backstop_should_scan;
    // On-chain BELOW recorded + no mint in flight -> an unsubmitted burn -> scan.
    assert!(backstop_should_scan(900, 1_000, false));
    // Equal -> in sync, nothing for the sweep to find -> skip.
    assert!(!backstop_should_scan(1_000, 1_000, false));
    // On-chain ABOVE recorded -> a mint EXCESS (e.g. an RPC-false-negative mint
    // that landed but was never credited), NOT a burn -> skip. Scanning here
    // would repeat every tick forever; this is the bug the backstop fix closes.
    assert!(!backstop_should_scan(1_100, 1_000, false));
    // Below but a mint is in flight -> stay cheap (submit_burn_proof + the
    // post-confirm tick reconcile) -> skip.
    assert!(!backstop_should_scan(900, 1_000, true));
    // Equal + in-flight -> skip.
    assert!(!backstop_should_scan(1_000, 1_000, true));
    // Zero/zero -> skip.
    assert!(!backstop_should_scan(0, 0, false));
}

#[test]
fn no_debt_fast_path_stays_active_with_pending_chain_burn() {
    use super::deposit_watch::burn_watch_can_skip_for_no_supply_obligation;

    assert!(
        burn_watch_can_skip_for_no_supply_obligation(0, 0),
        "zero debt and zero pending burn has no foreign-supply obligation"
    );
    assert!(
        !burn_watch_can_skip_for_no_supply_obligation(0, 42),
        "pending_chain_burn means IC-side icUSD was burned but foreign representation remains outstanding"
    );
    assert!(
        !burn_watch_can_skip_for_no_supply_obligation(42, 0),
        "live debt means user burns can still repay a vault"
    );
}

// ─── F-02 (2026-06-18 audit, report.md / findings.json) ──────────────────────
//
// F-02 claims `processed_burn_keys` can grow unbounded during a stalled cursor
// (a deep reorg regression, or a `reorg_halted` chain awaiting operator
// `clear_reorg_halt`). The two tests below establish the actual bound of
// `run_observer`'s own per-burn loop (deposit_watch.rs ~848-976) and
// `advance_cursor_and_prune` (~1004-1014), the only insertion/eviction sites
// owned by this file.
//
// Analysis result:
//   - A pure RPC/probe stall (the #338 TooFewCycles shape) means
//     `fetch_block_numbers` errors and `run_observer` returns BEFORE the
//     per-burn loop or `advance_cursor_and_prune` ever run (deposit_watch.rs
//     ~623-629) — zero insertions, zero growth, trivially bounded.
//   - A `DeferredLiquidation` head-of-line stall (mid-liquidation burn blocks
//     the range) DOES let the loop run repeatedly across ticks without the
//     cursor advancing, but it is bounded: `deferred_liquidation_stall_...`
//     below proves the key set does not grow across re-scans of the same
//     range, and that the deferred burn applies exactly once (no double-
//     decrement) once the blocker clears.
//   - A `reorg_halted` stall is different: `run_observer`'s `should_skip`
//     check (~383-390) returns before EVEN reaching `fetch_block_numbers`, so
//     this file's own loop never runs and never inserts a key while halted.
//     `chains/evm/burn_proof.rs::apply_receipt_burns_to_state` inserts into
//     this SAME `processed_burn_keys` map via the independent
//     `submit_burn_proof` notify path — it is the PRIMARY burn-detection path
//     (poll is off by default per Phase 1c/#214-#215). `keys_ahead_of_cursor_
//     are_retained_...` below proves this file's own pruning correctly
//     refuses to evict keys ahead of a pinned cursor (required for dedup
//     correctness: evicting a still-re-scannable key would let a resumed
//     re-scan replay/double-decrement it) — that retention behavior stands
//     regardless of the notify path's own gating. The notify path's residual
//     — that it was not gated on `reorg_halted`, so it could insert keys
//     during a halt this file's own loop never runs — is NOW CLOSED in
//     `chains/evm/burn_proof.rs`: `apply_receipt_burns_to_state` re-checks
//     `reorg_halted` as its first statement, inside the same `mutate_state`
//     closure with no `.await` between the check and the mutation, so it
//     refuses (no insertion, no mutation) while halted; a cheap pre-await
//     check in `verify_and_apply_burn_proof` additionally fails fast before
//     spending outcall cycles.

#[test]
fn deferred_liquidation_stall_bounds_processed_burn_keys_across_rescans() {
    // Mirrors run_observer's per-burn loop body (dedup-check, apply, key
    // record, halt-vs-continue classification) without the async RPC layer,
    // so the head-of-line bound can be proven deterministically.
    fn run_tick(s: &mut MultiChainState, burns: &[BurnLog]) -> bool {
        let mut burn_ok = true;
        for burn in burns {
            let key = format!("{}:0", burn.tx_hash);
            let already_processed = s
                .processed_burn_keys
                .get(&burn.block_number)
                .map(|set| set.contains(&key))
                .unwrap_or(false);
            if already_processed {
                continue;
            }
            let current_total = s.total_chain_vault_debt_e8s();
            match apply_burn_to_state(s, burn, current_total) {
                Ok(()) => {
                    s.processed_burn_keys.entry(burn.block_number).or_default().insert(key);
                }
                Err(BurnApplyError::InvalidBurn(_)) => {
                    s.processed_burn_keys.entry(burn.block_number).or_default().insert(key);
                }
                Err(BurnApplyError::SupplyInvariant(_)) | Err(BurnApplyError::DeferredLiquidation) => {
                    burn_ok = false;
                    break;
                }
            }
        }
        burn_ok
    }

    let mut s = seeded();
    s.chain_vaults.insert(2, ChainVaultV1 {
        vault_id: 2, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc2".into(), collateral_amount_native: 0, debt_e8s: 1_000_000_000,
        mint_recipient: "0xr2".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,
    });
    {
        let v1 = s.chain_vaults.get_mut(&1).unwrap();
        v1.debt_e8s = 1_000_000_000;
        v1.pending_liquidation = Some(PendingLiquidationV1 {
            op_id: 1,
            debt_to_clear_e8s: 200_000_000,
            collateral_reserved_native: 1,
            tier: LiquidationTier::Bot,
            started_at_ns: 0,
        });
    }
    s.chain_supplies.insert(ChainId(10143), 2_000_000_000);

    // Block order: vault2 (ok) -> vault1 (mid-liquidation, blocks) -> vault2 (never reached while blocked).
    let burns = vec![
        BurnLog { vault_id: 2, amount_e8s: 300_000_000, tx_hash: "0xa".into(), block_number: 101 },
        BurnLog { vault_id: 1, amount_e8s: 200_000_000, tx_hash: "0xb".into(), block_number: 102 },
        BurnLog { vault_id: 2, amount_e8s: 400_000_000, tx_hash: "0xc".into(), block_number: 103 },
    ];

    let key_count = |s: &MultiChainState| s.processed_burn_keys.values().map(|set| set.len()).sum::<usize>();

    // Tick 1: blocks at burn[1] (vault1 mid-liquidation). Only burn[0] is keyed.
    assert!(!run_tick(&mut s, &burns), "burn_ok false: DeferredLiquidation stopped the range");
    assert_eq!(key_count(&s), 1, "only the pre-blocker burn is keyed");

    // Ticks 2..5: the cursor does NOT advance (burn_ok was false), so
    // run_observer would re-scan this exact same range every tick. The dedup
    // set must NOT grow across these repeats -- this IS the F-02 bound for
    // the DeferredLiquidation stall shape.
    for i in 0..4 {
        assert!(!run_tick(&mut s, &burns), "tick {i}: still blocked");
        assert_eq!(key_count(&s), 1, "tick {i}: stall does not grow the dedup set");
    }
    assert_eq!(s.chain_vaults[&1].debt_e8s, 1_000_000_000, "vault1 debt untouched while deferred");
    assert_eq!(s.chain_vaults[&2].debt_e8s, 700_000_000, "vault2's first burn applied exactly once across all stalled re-scans");

    // The liquidation clears; the next re-scan resolves the blocker AND
    // reaches the remaining burn in the same tick.
    s.chain_vaults.get_mut(&1).unwrap().pending_liquidation = None;
    assert!(run_tick(&mut s, &burns), "range now fully resolved");
    assert_eq!(key_count(&s), 3, "all three burns keyed exactly once");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 800_000_000, "vault1 debt decremented exactly once (no double-apply from earlier stalled ticks)");
    assert_eq!(s.chain_vaults[&2].debt_e8s, 300_000_000, "vault2 both its burns applied exactly once");
}

#[test]
fn keys_ahead_of_cursor_are_retained_across_repeated_stalled_prune_calls() {
    // F-02 (see the block comment above): `advance_cursor_and_prune` only ever
    // discharges keys at block <= the NEW cursor, and it is only ever invoked
    // from within `run_observer`, which never runs the burn-watch path while
    // `reorg_halted[chain]` is set. This test proves this file's pruning
    // correctly REFUSES to evict keys ahead of a pinned (non-advancing)
    // cursor -- the only sound discharge condition, since evicting a
    // still-re-scannable key would let a later resumed re-scan replay it
    // (fund-safety, not just memory growth). That retention behavior is
    // independent of, and unaffected by, the notify path's own gating: the
    // scenario this test simulates -- external insertions landing ahead of a
    // frozen cursor -- is what `chains/evm/burn_proof.rs`'s notify-path
    // `apply_receipt_burns_to_state` NOW REFUSES to do while `reorg_halted`
    // (it re-checks the flag as its first statement, in the same
    // `mutate_state` closure as the mutation, no `.await` in between), which
    // closed the F-02 residual growth path this file could not bound on its
    // own.
    let mut s = seeded();
    s.last_observed_block.insert(ChainId(10143), 500);

    // Simulate several independent submit_burn_proof notify-path insertions
    // landing for blocks AHEAD of the frozen cursor while the observer sits
    // reorg-halted (unbounded in principle: as many as there are real burns +
    // proof submissions during the halt).
    for (i, block) in [501u64, 900, 5_000, 50_000].into_iter().enumerate() {
        let mut set = std::collections::BTreeSet::new();
        set.insert(format!("0xnotify{i}:0"));
        s.processed_burn_keys.insert(block, set);
    }
    let before = s.processed_burn_keys.len();
    assert_eq!(before, 4);

    // The observer's own pruning, even called repeatedly with the cursor
    // pinned at its last known value (the reorg-halted no-progress case),
    // must never touch a future-relative-to-cursor key.
    for i in 0..10 {
        advance_cursor_and_prune(&mut s, ChainId(10143), 500);
        assert_eq!(
            s.processed_burn_keys.len(),
            before,
            "iteration {i}: no cursor progress while halted -> nothing ahead of the cursor is safely prunable"
        );
    }
}
