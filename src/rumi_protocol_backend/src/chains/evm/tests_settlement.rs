use super::settlement::{
    confirm_interest_mint_in_state, confirm_mint_in_state, fundable_withdrawal_value,
    select_next_op, OpAction,
};
use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainState;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind, SettlementOpStatus};
use candid::Principal;

fn vault_pending(s: &mut MultiChainState, vault_id: u64, pending: u128) {
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_native: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: pending,
        status: ChainVaultStatus::MintPending, opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
}

#[test]
fn select_next_op_prefers_queued_then_inflight() {
    let mut q = crate::chains::settlement_queue::SettlementQueueV1::default();
    let op = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 10, vault_id: 1 },
        "k0".into(), 0);
    let id = q.enqueue(op).unwrap();
    // Queued -> action Submit.
    match select_next_op(&q) {
        Some((oid, OpAction::Submit)) => assert_eq!(oid, id),
        other => panic!("expected Submit, got {other:?}"),
    }
}

#[test]
fn select_next_op_confirms_inflight_before_submitting_new() {
    let mut q = crate::chains::settlement_queue::SettlementQueueV1::default();
    let op0 = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 10, vault_id: 1 }, "k0".into(), 0);
    let id0 = q.enqueue(op0).unwrap();
    let op1 = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 20, vault_id: 2 }, "k1".into(), 0);
    let _id1 = q.enqueue(op1).unwrap();
    // Put op0 inflight; with something inflight, only Confirm of op0 is actionable.
    q.pending.get_mut(&id0).unwrap().status = SettlementOpStatus::Inflight { tries: 1, last_attempt_ns: 0 };
    match select_next_op(&q) {
        Some((oid, OpAction::Confirm)) => assert_eq!(oid, id0),
        other => panic!("expected Confirm of inflight op, got {other:?}"),
    }
}

#[test]
fn confirm_mint_moves_pending_to_debt_and_increments_supply() {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000); // 100 icUSD pending
    // PRE-mint total_chain_vault_debt_e8s() == 0 (vault debt_e8s is still 0).
    // confirm_mint_in_state adds observed_e8s internally to get the post-mint total.
    confirm_mint_in_state(&mut s, ChainId(10143), 1, 10_000_000_000, 0, 7_777).expect("confirm");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 10_000_000_000);
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Open));
    assert_eq!(s.chain_supplies[&ChainId(10143)], 10_000_000_000);
    assert_eq!(
        s.chain_vaults[&1].last_interest_accrual_ns, 7_777,
        "interest accrual window is stamped when the vault goes Open at mint-confirm"
    );
}

/// An Open vault with confirmed `debt` and an interest mint of `pending` in
/// flight (`last_interest_accrual_ns = 1_000`).
fn vault_interest_pending(s: &mut MultiChainState, vault_id: u64, debt: u128, pending: u128) {
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner: Principal::anonymous(), collateral_chain: ChainId(71),
        custody_address: "0xc".into(), collateral_amount_native: 1_400_000_000_000_000_000_000,
        debt_e8s: debt, mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
    owner_evm: None,
        last_interest_accrual_ns: 1_000,
        pending_interest_mint_e8s: pending,
        pending_liquidation: None,    });
}

#[test]
fn confirm_interest_mint_grows_debt_and_supply_equally() {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(71), 100 * 100_000_000); // 100 icUSD principal minted
    vault_interest_pending(&mut s, 1, 100 * 100_000_000, 2 * 100_000_000); // 2 icUSD pending
    let pre = s.total_chain_vault_debt_e8s(); // 100e8
    confirm_interest_mint_in_state(&mut s, ChainId(71), 1, 2 * 100_000_000, 5_000, pre)
        .expect("confirm");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 102 * 100_000_000, "debt += interest");
    assert_eq!(s.chain_vaults[&1].pending_interest_mint_e8s, 0, "pending cleared");
    assert_eq!(
        s.chain_vaults[&1].last_interest_accrual_ns, 5_000,
        "accrual window advanced to the harvest snapshot time"
    );
    assert_eq!(
        s.chain_supplies[&ChainId(71)],
        102 * 100_000_000,
        "supply grows equally -> invariant gap stays 0"
    );
}

#[test]
fn confirm_interest_mint_rejects_amount_mismatch_no_mutation() {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(71), 100 * 100_000_000);
    vault_interest_pending(&mut s, 1, 100 * 100_000_000, 2 * 100_000_000);
    let pre = s.total_chain_vault_debt_e8s();
    let err = confirm_interest_mint_in_state(&mut s, ChainId(71), 1, 3 * 100_000_000, 5_000, pre)
        .unwrap_err();
    assert!(err.contains("observed"), "mismatch error mentions observed/pending: {err}");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100 * 100_000_000, "debt untouched on reject");
    assert_eq!(s.chain_vaults[&1].pending_interest_mint_e8s, 2 * 100_000_000, "pending untouched");
    assert_eq!(s.chain_supplies[&ChainId(71)], 100 * 100_000_000, "supply untouched");
}

#[test]
fn confirm_mint_rejects_amount_mismatch() {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000);
    // Observed amount differs from pending: reject (caught before any supply mutation), do not mutate.
    let res = confirm_mint_in_state(&mut s, ChainId(10143), 1, 9_999_999_999, 0, 0);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 10_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 0);
}

#[test]
fn confirm_mint_unknown_vault_rejected() {
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    assert!(confirm_mint_in_state(&mut s, ChainId(10143), 999, 1, 0, 0).is_err());
}

#[test]
fn confirm_mint_second_vault_uses_running_total() {
    // Two vaults: first already confirmed (debt 100e8, supply 100e8). Confirming the
    // second (pending 50e8) must pass PRE-mint total = 100e8; helper computes 150e8.
    let mut s = MultiChainState::default();
    s.chain_supplies.insert(ChainId(10143), 10_000_000_000);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_native: 0, debt_e8s: 10_000_000_000,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    vault_pending(&mut s, 2, 5_000_000_000);
    let pre_total = s.total_chain_vault_debt_e8s(); // == 10e8
    confirm_mint_in_state(&mut s, ChainId(10143), 2, 5_000_000_000, pre_total, 0).expect("confirm 2nd");
    assert_eq!(s.chain_vaults[&2].debt_e8s, 5_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 15_000_000_000);
}

#[test]
fn fundable_withdrawal_value_nets_gas_only_when_balance_is_tight() {
    let amount = 20_000_000_000_000_000_000u128; // 20 native (full close)
    let max_fee = 40_000_000_000u128; // 40 gwei ceiling
    let gas_reserve = 21_000u128 * max_fee; // gas_limit * max_fee

    // Full close: custody holds EXACTLY the requested amount -> value is netted
    // down by the worst-case gas so the tx can still pay for itself.
    assert_eq!(
        fundable_withdrawal_value(amount, amount, max_fee),
        amount - gas_reserve
    );

    // Partial withdrawal leaving a buffer >= gas: the full requested amount is
    // sent (custody keeps the rest, which covers gas).
    let bigger_balance = amount + 1_000_000_000_000_000_000; // 21 native
    assert_eq!(
        fundable_withdrawal_value(amount, bigger_balance, max_fee),
        amount
    );

    // Degenerate: balance below the gas reserve -> saturates to 0 (never panics
    // / underflows), so the worker sends a 0-value tx rather than trapping.
    assert_eq!(fundable_withdrawal_value(amount, gas_reserve / 2, max_fee), 0);
}

// ─── Increment 3 / Task 7: apply_liquidation_settlement_in_state (Phase 2) ───
mod phase2_tests {
    use super::super::settlement::apply_liquidation_settlement_in_state;
    use crate::chains::config::ChainId;
    use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use crate::chains::multi_chain_state::MultiChainState;
    use crate::chains::vault::{LiquidationTier, PendingLiquidationV1};
    use candid::Principal;

    const CFX: ChainId = ChainId(71);
    const E8: u128 = 100_000_000;
    const E18: u128 = 1_000_000_000_000_000_000;

    /// Insert an Open vault mid-liquidation (collateral already reserved in Phase 1)
    /// with a Bot marker, and seed chain_supplies so the unified invariant holds.
    fn marked(
        s: &mut MultiChainState,
        vault_id: u64,
        debt_e8s: u128,
        collateral_remaining: u128,
        debt_to_clear: u128,
        collateral_reserved: u128,
    ) {
        s.chain_vaults.insert(
            vault_id,
            ChainVaultV1 {
                vault_id,
                owner: Principal::anonymous(),
                collateral_chain: CFX,
                custody_address: "0xc".into(),
                collateral_amount_native: collateral_remaining,
                debt_e8s,
                mint_recipient: "0xr".into(),
                pending_mint_e8s: 0,
                status: ChainVaultStatus::Open,
                opened_at_ns: 0,
                owner_evm: None,
                last_interest_accrual_ns: 0,
                pending_interest_mint_e8s: 0,
                pending_liquidation: Some(PendingLiquidationV1 {
                    op_id: 9,
                    debt_to_clear_e8s: debt_to_clear,
                    collateral_reserved_native: collateral_reserved,
                    tier: LiquidationTier::Bot,
                    started_at_ns: 0,
                }),
            },
        );
        *s.chain_supplies.entry(CFX).or_default() += debt_e8s;
        s.bot_pending_chain_vaults.insert(vault_id, 0);
    }

    #[test]
    fn shifts_debt_to_reserve_supply_unchanged() {
        let mut s = MultiChainState::default();
        marked(&mut s, 7, 100 * E8, 500 * E18, 67 * E8, 839 * E18);
        // realized 75 USDC (18-dec) -> 75e8 >= debt_to_clear 67e8.
        apply_liquidation_settlement_in_state(&mut s, CFX, 7, 9, 75 * E18, 18).expect("phase2 ok");
        let v = s.chain_vaults.get(&7).unwrap();
        assert_eq!(v.debt_e8s, 33 * E8, "debt -= actual_cleared");
        assert_eq!(*s.reserve_backing_e8s.get(&CFX).unwrap(), 67 * E8, "backing += actual_cleared");
        assert_eq!(*s.reserve_usdc_native.get(&CFX).unwrap(), 75 * E18, "usdc += realized");
        assert_eq!(*s.chain_supplies.get(&CFX).unwrap(), 100 * E8, "supply UNCHANGED (PSM)");
        assert!(v.pending_liquidation.is_none(), "marker cleared");
        assert!(s.chain_bad_debt_e8s.get(&CFX).is_none(), "no shortfall");
        // Unified invariant: supply == debt + reserve + pending_burn.
        assert_eq!(
            *s.chain_supplies.get(&CFX).unwrap(),
            s.total_chain_vault_debt_e8s() + s.total_reserve_backing_e8s() + s.total_pending_chain_burn_e8s()
        );
    }

    #[test]
    fn shortfall_clamps_and_records_bad_debt() {
        let mut s = MultiChainState::default();
        marked(&mut s, 7, 100 * E8, 500 * E18, 67 * E8, 839 * E18);
        // realized only 60 USDC -> 60e8 < debt_to_clear 67e8.
        apply_liquidation_settlement_in_state(&mut s, CFX, 7, 9, 60 * E18, 18).expect("phase2 ok");
        assert_eq!(s.chain_vaults.get(&7).unwrap().debt_e8s, 40 * E8);
        assert_eq!(*s.reserve_backing_e8s.get(&CFX).unwrap(), 60 * E8, "backing capped at realized USD");
        assert_eq!(*s.chain_bad_debt_e8s.get(&CFX).unwrap(), 7 * E8, "shortfall recorded");
        assert_eq!(*s.chain_supplies.get(&CFX).unwrap(), 100 * E8, "supply unchanged");
    }

    #[test]
    fn closes_vault_when_drained() {
        let mut s = MultiChainState::default();
        // collateral fully reserved in Phase 1 (remaining 0); clearing all debt drains it.
        marked(&mut s, 7, 100 * E8, 0, 100 * E8, 1_400 * E18);
        apply_liquidation_settlement_in_state(&mut s, CFX, 7, 9, 112 * E18, 18).expect("phase2 ok");
        let v = s.chain_vaults.get(&7).unwrap();
        assert_eq!(v.debt_e8s, 0);
        assert_eq!(v.status, ChainVaultStatus::Closed, "drained vault closes");
    }

    #[test]
    fn rejects_op_mismatch_no_mutation() {
        let mut s = MultiChainState::default();
        marked(&mut s, 7, 100 * E8, 500 * E18, 67 * E8, 839 * E18);
        // confirming op 99 != marker op 9.
        assert!(apply_liquidation_settlement_in_state(&mut s, CFX, 7, 99, 75 * E18, 18).is_err());
        assert_eq!(s.chain_vaults.get(&7).unwrap().debt_e8s, 100 * E8, "no mutation on reject");
        assert!(s.reserve_backing_e8s.get(&CFX).is_none());
    }
}
