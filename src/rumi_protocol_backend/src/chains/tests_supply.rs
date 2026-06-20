use super::supply::{apply_supply_delta, check_invariant, SupplyDelta, SupplyInvariantError};
use super::config::{ChainConfigV3, ChainId, ChainStatus, GasStrategy};
use super::multi_chain_state::MultiChainState;

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
    ).expect("decrease to zero");
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
    assert!(matches!(res, Err(SupplyInvariantError::HaltedAfterSelfCheckFailure)));
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
        pending_liquidation: None,    };
    let mut s = MultiChainState::default();
    s.chain_vaults.insert(1, mk(1, 0)); // unstamped (pre-field snapshot)
    s.chain_vaults.insert(2, mk(2, 5)); // already stamped
    super::supply::stamp_chain_interest_accrual_start(&mut s, 12_345);
    assert_eq!(s.chain_vaults[&1].last_interest_accrual_ns, 12_345, "unstamped vault gets now");
    assert_eq!(s.chain_vaults[&2].last_interest_accrual_ns, 5, "already-stamped untouched");
    // Idempotent: a second run does not re-stamp the now-stamped vault.
    super::supply::stamp_chain_interest_accrual_start(&mut s, 99_999);
    assert_eq!(s.chain_vaults[&1].last_interest_accrual_ns, 12_345, "idempotent");
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
    assert_eq!(debt, 70, "total debt counts only realized debt_e8s, excludes pending interest");
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
    assert_eq!(apply_supply_delta(&mut s, CHAIN, SupplyDelta::Increase(10), 80), Ok(()));
    assert_eq!(s.chain_supplies[&CHAIN], 110);
    // A delta that breaks the generalized equality is rejected, no mutation:
    // +5 while still claiming debt 80 -> 115 != 80 + 30 = 110 -> Divergence.
    assert_eq!(
        apply_supply_delta(&mut s, CHAIN, SupplyDelta::Increase(5), 80),
        Err(SupplyInvariantError::Divergence { sum_after: 115, total_debt: 110 })
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
    assert_eq!(check_invariant(&s, 71), Err(SupplyInvariantError::Divergence { sum_after: 70, total_debt: 71 }));
}
