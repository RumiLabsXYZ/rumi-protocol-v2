use super::config::{ChainConfigV3, ChainId, ChainStatus, GasStrategy};
use super::multi_chain_state::MultiChainState;
use super::supply::{
    apply_debt_to_pending_burn_shift, apply_pending_burn_to_supply_shift, apply_supply_delta,
    check_invariant, settle_pending_chain_burn, settle_reserve_burn, BackingSettlementError,
    PendingBurnShiftError, PendingBurnSupplyShiftError, SupplyDelta, SupplyInvariantError,
};

fn fixture_state() -> MultiChainState {
    let mut s = MultiChainState::default();
    s.chain_configs.insert(
        ChainId(101),
        ChainConfigV3 {
            chain_id: ChainId(101),
            display_name: "TestChain".into(),
            rpc_endpoints: vec![],
            finality_depth: 1,
            gas_strategy: GasStrategy::NotApplicable,
            chain_native_decimals: 18,
            registered_at_ns: 0,
            status: ChainStatus::Registered,
            burn_watch_poll_enabled: false,
            min_quorum_providers: None,
        },
    );
    s.chain_supplies.insert(ChainId(101), 0);
    s
}

#[test]
fn increase_supply_preserves_invariant() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1_000),
        /* total_debt_e8s = */ 1_000,
    );
    assert!(res.is_ok());
    assert_eq!(s.chain_supplies[&ChainId(101)], 1_000);
}

#[test]
fn decrease_supply_below_zero_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(500),
        /* total_debt_e8s = */ 0,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Underflow { .. })));
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}

#[test]
fn decrease_to_exact_zero_keeps_entry_for_audit() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 50);
    apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(50),
        /* total_debt_e8s = */ 0,
    )
    .expect("decrease to zero");
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    assert!(s.chain_supplies.contains_key(&ChainId(101)));
}

#[test]
fn unknown_chain_id_is_rejected() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(999),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(res, Err(SupplyInvariantError::UnknownChain(_))));
}

#[test]
fn invariant_halted_blocks_every_mutation() {
    let mut s = fixture_state();
    s.invariant_halted = true;
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(
        res,
        Err(SupplyInvariantError::HaltedAfterSelfCheckFailure)
    ));
}

#[test]
fn divergence_from_total_debt_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(50),
        /* total_debt_e8s = */ 200,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Divergence { .. })));
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}

// Task 12: the post_upgrade migration stamp sets last_interest_accrual_ns = now
// for any vault that decoded with 0 (an existing vault from a pre-field
// snapshot), and leaves already-stamped vaults alone. Idempotent.
#[test]
fn stamp_sets_accrual_start_only_for_unstamped_vaults() {
    use super::vault::{ChainVaultStatus, ChainVaultV1};
    use candid::Principal;
    let mk = |id: u64, last: u64| ChainVaultV1 {
        vault_id: id,
        owner: Principal::anonymous(),
        collateral_chain: ChainId(71),
        custody_address: "0xc".into(),
        collateral_amount_native: 0,
        debt_e8s: 100,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: last,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,
    };
    let mut s = MultiChainState::default();
    s.chain_vaults.insert(1, mk(1, 0)); // unstamped (pre-field snapshot)
    s.chain_vaults.insert(2, mk(2, 5)); // already stamped
    super::supply::stamp_chain_interest_accrual_start(&mut s, 12_345);
    assert_eq!(
        s.chain_vaults[&1].last_interest_accrual_ns, 12_345,
        "unstamped vault gets now"
    );
    assert_eq!(
        s.chain_vaults[&2].last_interest_accrual_ns, 5,
        "already-stamped untouched"
    );
    // Idempotent: a second run does not re-stamp the now-stamped vault.
    super::supply::stamp_chain_interest_accrual_start(&mut s, 99_999);
    assert_eq!(
        s.chain_vaults[&1].last_interest_accrual_ns, 12_345,
        "idempotent"
    );
}

// ─── Increment 1 / Task 3: unified supply invariant (debt + reserve + pending-burn)
//
// spec 5.2: sum(chain_supplies) == total_chain_vault_debt_e8s()
//                                + total_reserve_backing_e8s()      (bot/PSM path)
//                                + total_pending_chain_burn_e8s()   (SP path, pre-burn)
// With all-zero reserve/pending (Increment 1) this reduces to the old
// `supply == debt`, so it is behavior-preserving on the live snapshot.

const CHAIN: ChainId = ChainId(101);

/// An Open vault on CHAIN with the given realized debt and pending interest.
fn vault_with(debt_e8s: u128, pending_interest_e8s: u128) -> super::vault::ChainVaultV1 {
    use super::vault::ChainVaultStatus;
    use candid::Principal;
    super::vault::ChainVaultV1 {
        vault_id: 1,
        owner: Principal::anonymous(),
        collateral_chain: CHAIN,
        custody_address: "0xc".into(),
        collateral_amount_native: 0,
        debt_e8s,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: pending_interest_e8s,
        pending_liquidation: None,
    }
}

// (a) The reserve term is part of the RHS: when the bot shifts debt -> reserve,
// chain_supplies stays put while debt drops, and the invariant still holds. The
// OLD bare-debt check would FALSE-HALT here (sum 100 != debt 70).
#[test]
fn invariant_holds_with_nonzero_reserve_backing() {
    let mut s = fixture_state();
    s.chain_supplies.insert(CHAIN, 100); // 100 icUSD circulating
    s.reserve_backing_e8s.insert(CHAIN, 30); // 30 backed by bot-held USDC reserve
                                             // debt component = 70; generalized RHS = 70 + 30 + 0 = 100 == supply.
    assert_eq!(check_invariant(&s, 70), Ok(()));
}

// (b) The pending-burn term is part of the RHS: the SP burns IC-side first and
// moves the amount into pending_chain_burn_e8s; chain_supplies drops only when the
// eSpace burn confirms. The invariant must hold during that window.
#[test]
fn invariant_holds_with_nonzero_pending_chain_burn() {
    let mut s = fixture_state();
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 30);
    assert_eq!(check_invariant(&s, 70), Ok(()));
}

// (c) HIGH #1: pending_interest_mint_e8s mints NEW foreign icUSD only when it
// CONFIRMS, so it is NOT yet in chain_supplies and must NOT appear on the RHS.
// Here a vault carries 70 realized debt + 50 pending interest, and 30 of debt has
// shifted to bot reserve, so supply = 100 (= 70 + 30). The 50 pending interest is
// excluded from BOTH total_chain_vault_debt_e8s() and the RHS.
#[test]
fn invariant_excludes_pending_interest_mint_from_rhs() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(70, 50));
    s.chain_supplies.insert(CHAIN, 100);
    s.reserve_backing_e8s.insert(CHAIN, 30);
    let debt = s.total_chain_vault_debt_e8s();
    assert_eq!(
        debt, 70,
        "total debt counts only realized debt_e8s, excludes pending interest"
    );
    // RHS = 70 + 30 + 0 = 100 == supply. If the 50 pending interest leaked onto the
    // RHS it would be 150 != 100 and this would FALSE-HALT.
    assert_eq!(check_invariant(&s, debt), Ok(()));
}

// (d) HIGH #2: apply_supply_delta enforces the GENERALIZED equality — it accepts a
// delta that preserves sum == debt + reserve + pending_burn and rejects (no
// mutation) one that breaks it.
#[test]
fn apply_supply_delta_enforces_generalized_rhs() {
    let mut s = fixture_state();
    // Balanced start: supply 100 == debt 70 + reserve 30.
    s.chain_supplies.insert(CHAIN, 100);
    s.reserve_backing_e8s.insert(CHAIN, 30);
    // A mint of +10 with debt now 80: 110 == 80 + 30 -> accept.
    assert_eq!(
        apply_supply_delta(&mut s, CHAIN, SupplyDelta::Increase(10), 80),
        Ok(())
    );
    assert_eq!(s.chain_supplies[&CHAIN], 110);
    // A delta that breaks the generalized equality is rejected, no mutation:
    // +5 while still claiming debt 80 -> 115 != 80 + 30 = 110 -> Divergence.
    assert_eq!(
        apply_supply_delta(&mut s, CHAIN, SupplyDelta::Increase(5), 80),
        Err(SupplyInvariantError::Divergence {
            sum_after: 115,
            total_debt: 110
        })
    );
    assert_eq!(s.chain_supplies[&CHAIN], 110, "no mutation on rejection");
}

// Behavior preservation: with all-zero reserve/pending the generalized RHS is
// byte-identical to the old `supply == debt` (this is what makes the staging
// upgrade a no-op behaviorally).
#[test]
fn invariant_reduces_to_bare_debt_when_reserve_and_pending_zero() {
    let mut s = fixture_state();
    s.chain_supplies.insert(CHAIN, 70);
    assert_eq!(check_invariant(&s, 70), Ok(()));
    assert_eq!(
        check_invariant(&s, 71),
        Err(SupplyInvariantError::Divergence {
            sum_after: 70,
            total_debt: 71
        })
    );
}

// ─── Increment 1 / Task 4: apply_debt_to_reserve_shift (the reserve-term gate) ───

use super::supply::{apply_debt_to_reserve_shift, ReserveShiftError};

// The bot path moves debt -> reserve WITHOUT burning icUSD: chain_supplies stays
// put, debt drops, reserve rises by the same amount, and the unified invariant
// stays balanced by construction.
#[test]
fn reserve_shift_moves_debt_to_reserve_and_preserves_invariant() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    // pre: 100 == 100 (debt) + 0 (reserve) + 0 (pending)
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));

    // Bot clears 40 of debt into reserve, realizing 45_000000000000000000 wei USDC
    // (incl. the 12% penalty surplus over the 40 cleared).
    apply_debt_to_reserve_shift(&mut s, CHAIN, 1, 40, 45_000_000_000_000_000_000)
        .expect("reserve shift");

    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 60,
        "vault debt reduced by cleared amount"
    );
    assert_eq!(
        s.reserve_backing_e8s[&CHAIN], 40,
        "reserve_backing credited the cleared debt"
    );
    assert_eq!(
        s.reserve_usdc_native[&CHAIN], 45_000_000_000_000_000_000,
        "realized USDC recorded (NOT on the invariant RHS)"
    );
    assert_eq!(
        s.chain_supplies[&CHAIN], 100,
        "chain_supplies UNCHANGED (no burn)"
    );
    // post: 100 == 60 (debt) + 40 (reserve) + 0 (pending) -> invariant still holds
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));
}

#[test]
fn reserve_shift_rejects_clearing_more_than_debt() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    assert_eq!(
        apply_debt_to_reserve_shift(&mut s, CHAIN, 1, 150, 1),
        Err(ReserveShiftError::ClearExceedsDebt {
            cleared_e8s: 150,
            vault_debt_e8s: 100
        })
    );
    // No mutation on rejection.
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);
    assert!(s.reserve_backing_e8s.get(&CHAIN).copied().unwrap_or(0) == 0);
    assert!(s.reserve_usdc_native.get(&CHAIN).copied().unwrap_or(0) == 0);
}

#[test]
fn reserve_shift_rejects_unknown_vault() {
    let mut s = fixture_state();
    assert_eq!(
        apply_debt_to_reserve_shift(&mut s, CHAIN, 99, 10, 10),
        Err(ReserveShiftError::UnknownVault(99))
    );
}

#[test]
fn reserve_shift_blocked_while_invariant_halted() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.invariant_halted = true;
    assert_eq!(
        apply_debt_to_reserve_shift(&mut s, CHAIN, 1, 40, 1),
        Err(ReserveShiftError::Halted)
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no mutation while halted");
}

// ─── Increment 4 / Task 1: the SP-path supply helpers ───
//
// The SP fallback BURNS IC-native icUSD, so unlike the bot/PSM path it moves debt
// into pending_chain_burn_e8s (RHS term-3), NOT reserve_backing_e8s (term-2). Two
// guarded mutations, mirroring apply_debt_to_reserve_shift / apply_supply_delta:
//   (1) absorb time: apply_debt_to_pending_burn_shift moves debt -> pending_burn,
//       chain_supplies UNCHANGED (foreign representation remains outstanding).
//   (2) later manual reconciliation: apply_pending_burn_to_supply_shift drops
//       pending_burn AND chain_supplies together (foreign supply retired).
// Both conserve the unified RHS by construction.

// (1a) absorb: debt -> pending_burn, supply unchanged, invariant preserved.
#[test]
fn pending_burn_shift_moves_debt_to_pending_and_preserves_invariant() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    // pre: 100 == 100 (debt) + 0 (reserve) + 0 (pending)
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));

    // SP burns 40 of IC icUSD and absorbs 40 of debt.
    apply_debt_to_pending_burn_shift(&mut s, CHAIN, 1, 40).expect("pending-burn shift");

    assert_eq!(
        s.chain_vaults[&1].debt_e8s, 60,
        "vault debt reduced by absorbed amount"
    );
    assert_eq!(
        s.pending_chain_burn_e8s[&CHAIN], 40,
        "pending_chain_burn booked the absorbed debt"
    );
    assert_eq!(
        s.chain_supplies[&CHAIN], 100,
        "chain_supplies UNCHANGED (foreign representation remains outstanding)"
    );
    // post: 100 == 60 (debt) + 0 (reserve) + 40 (pending) -> invariant still holds
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));
}

#[test]
fn pending_burn_shift_rejects_clearing_more_than_debt() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    assert_eq!(
        apply_debt_to_pending_burn_shift(&mut s, CHAIN, 1, 150),
        Err(PendingBurnShiftError::ClearExceedsDebt {
            cleared_e8s: 150,
            vault_debt_e8s: 100,
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no mutation on rejection");
    assert_eq!(
        s.pending_chain_burn_e8s.get(&CHAIN).copied().unwrap_or(0),
        0
    );
}

#[test]
fn pending_burn_shift_rejects_unknown_vault() {
    let mut s = fixture_state();
    assert_eq!(
        apply_debt_to_pending_burn_shift(&mut s, CHAIN, 99, 10),
        Err(PendingBurnShiftError::UnknownVault(99))
    );
}

#[test]
fn pending_burn_shift_rejects_wrong_chain() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0)); // vault is on CHAIN (101)
    s.chain_supplies.insert(CHAIN, 100);
    assert_eq!(
        apply_debt_to_pending_burn_shift(&mut s, ChainId(202), 1, 40),
        Err(PendingBurnShiftError::WrongChain {
            vault_chain: CHAIN,
            requested: ChainId(202),
        })
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no mutation on rejection");
}

#[test]
fn pending_burn_shift_blocked_while_invariant_halted() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(100, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.invariant_halted = true;
    assert_eq!(
        apply_debt_to_pending_burn_shift(&mut s, CHAIN, 1, 40),
        Err(PendingBurnShiftError::Halted)
    );
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100, "no mutation while halted");
}

// (2a) reconcile: pending_burn AND chain_supplies both drop, invariant preserved.
#[test]
fn pending_burn_to_supply_drops_both_and_preserves_invariant() {
    let mut s = fixture_state();
    // State after an SP absorb of 40: debt 60, pending_burn 40, supply still 100.
    s.chain_vaults.insert(1, vault_with(60, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 40);
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));

    // The operator has retired 40 of the foreign representation.
    apply_pending_burn_to_supply_shift(&mut s, CHAIN, 40).expect("pending-burn -> supply shift");

    assert_eq!(
        s.pending_chain_burn_e8s[&CHAIN], 0,
        "pending_chain_burn drained"
    );
    assert_eq!(
        s.chain_supplies[&CHAIN], 60,
        "chain_supplies dropped (foreign representation retired)"
    );
    // post: 60 == 60 (debt) + 0 (reserve) + 0 (pending) -> invariant still holds
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));
}

#[test]
fn pending_burn_to_supply_rejects_amount_exceeding_pending() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(60, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 40);
    assert_eq!(
        apply_pending_burn_to_supply_shift(&mut s, CHAIN, 50),
        Err(PendingBurnSupplyShiftError::PendingBurnUnderflow {
            chain: CHAIN,
            booked: 40,
            attempted: 50,
        })
    );
    assert_eq!(
        s.pending_chain_burn_e8s[&CHAIN], 40,
        "no mutation on rejection"
    );
    assert_eq!(s.chain_supplies[&CHAIN], 100, "no mutation on rejection");
}

#[test]
fn pending_burn_to_supply_blocked_while_invariant_halted() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(60, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 40);
    s.invariant_halted = true;
    assert_eq!(
        apply_pending_burn_to_supply_shift(&mut s, CHAIN, 40),
        Err(PendingBurnSupplyShiftError::Halted)
    );
    assert_eq!(
        s.pending_chain_burn_e8s[&CHAIN], 40,
        "no mutation while halted"
    );
    assert_eq!(s.chain_supplies[&CHAIN], 100, "no mutation while halted");
}

// ─── Increment 5 / Task 1: manual foreign-burn reconciliation ───

#[test]
fn pending_chain_burn_settlement_reduces_pending_and_supply() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(70, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 30);
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));

    settle_pending_chain_burn(&mut s, CHAIN, 10).expect("settle pending burn");

    assert_eq!(s.chain_supplies[&CHAIN], 90);
    assert_eq!(s.pending_chain_burn_e8s[&CHAIN], 20);
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));
}

#[test]
fn pending_chain_burn_settlement_rejects_over_settlement_without_mutation() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(70, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.pending_chain_burn_e8s.insert(CHAIN, 30);

    assert_eq!(
        settle_pending_chain_burn(&mut s, CHAIN, 31),
        Err(BackingSettlementError::BackingUnderflow {
            chain: CHAIN,
            current: 30,
            attempted_decrease: 31,
        })
    );
    assert_eq!(s.chain_supplies[&CHAIN], 100);
    assert_eq!(s.pending_chain_burn_e8s[&CHAIN], 30);
}

#[test]
fn reserve_burn_settlement_reduces_reserve_and_supply_but_keeps_usdc_books() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(70, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.reserve_backing_e8s.insert(CHAIN, 30);
    s.reserve_usdc_native
        .insert(CHAIN, 45_000_000_000_000_000_000);
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));

    settle_reserve_burn(&mut s, CHAIN, 10).expect("settle reserve burn");

    assert_eq!(s.chain_supplies[&CHAIN], 90);
    assert_eq!(s.reserve_backing_e8s[&CHAIN], 20);
    assert_eq!(s.reserve_usdc_native[&CHAIN], 45_000_000_000_000_000_000);
    assert_eq!(check_invariant(&s, s.total_chain_vault_debt_e8s()), Ok(()));
}

#[test]
fn reserve_burn_settlement_rejects_over_settlement_without_mutation() {
    let mut s = fixture_state();
    s.chain_vaults.insert(1, vault_with(70, 0));
    s.chain_supplies.insert(CHAIN, 100);
    s.reserve_backing_e8s.insert(CHAIN, 30);
    s.reserve_usdc_native
        .insert(CHAIN, 45_000_000_000_000_000_000);

    assert_eq!(
        settle_reserve_burn(&mut s, CHAIN, 31),
        Err(BackingSettlementError::BackingUnderflow {
            chain: CHAIN,
            current: 30,
            attempted_decrease: 31,
        })
    );
    assert_eq!(s.chain_supplies[&CHAIN], 100);
    assert_eq!(s.reserve_backing_e8s[&CHAIN], 30);
    assert_eq!(s.reserve_usdc_native[&CHAIN], 45_000_000_000_000_000_000);
}
