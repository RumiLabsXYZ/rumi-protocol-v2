use super::deposit_watch::{advance_cursor_and_prune, apply_burn_to_state, credit_deposit_to_state, BurnApplyError};
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV3;
use crate::chains::monad::evm_rpc::BurnLog;
use crate::chains::supply::SupplyInvariantError;
use candid::Principal;

fn seeded() -> MultiChainStateV3 {
    let mut s = MultiChainStateV3::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(), collateral_amount_e18: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
    });
    s
}

#[test]
fn credit_deposit_increments_collateral() {
    let mut s = seeded();
    credit_deposit_to_state(&mut s, 1, 5_000_000_000_000_000_000).expect("credit");
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 5_000_000_000_000_000_000);
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
fn supply_gate_skips_only_on_exact_match_and_no_inflight_mint() {
    use super::deposit_watch::can_skip_burn_scan;
    // Equal + no mint in flight -> skip (no unobserved burn possible).
    assert!(can_skip_burn_scan(1_000, 1_000, false));
    // Equal but a mint is in flight -> scan (a mint could mask a burn).
    assert!(!can_skip_burn_scan(1_000, 1_000, true));
    // On-chain below recorded -> a burn happened -> scan.
    assert!(!can_skip_burn_scan(900, 1_000, false));
    // On-chain above recorded (anomaly under sole-minter) -> scan.
    assert!(!can_skip_burn_scan(1_100, 1_000, false));
    // Below + in-flight mint -> scan.
    assert!(!can_skip_burn_scan(900, 1_000, true));
    // Zero/zero, no mint -> skip (degenerate but valid).
    assert!(can_skip_burn_scan(0, 0, false));
}
