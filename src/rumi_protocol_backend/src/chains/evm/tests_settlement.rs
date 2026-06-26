use super::settlement::{
    claim_liquidation_swap_submit_in_state, ClaimLiquidationSwapSubmitError,
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

fn queued_liquidation_swap(s: &mut MultiChainState, vault_id: u64) -> u64 {
    s.settlement_queues
        .entry(ChainId(71))
        .or_default()
        .enqueue(SettlementOp::new(
            SettlementOpKind::LiquidationSwap {
                vault_id,
                collateral_in_native: 100,
                min_usdc_out_native: 0,
                debt_to_clear_e8s: 50,
                router: "0x1111111111111111111111111111111111111111".into(),
                pair: "0x3333333333333333333333333333333333333333".into(),
                path: vec![
                    "0x4444444444444444444444444444444444444444".into(),
                    "0x5555555555555555555555555555555555555555".into(),
                ],
                reserve_recipient: "0x5555555555555555555555555555555555555555".into(),
                deadline_secs: 180,
            },
            format!("liq-{vault_id}"),
            0,
        ))
        .expect("enqueue swap")
}

fn marked_bot_liquidation(s: &mut MultiChainState, vault_id: u64, op_id: u64) {
    use crate::chains::vault::{LiquidationTier, PendingLiquidationV1};

    s.chain_vaults.insert(
        vault_id,
        ChainVaultV1 {
            vault_id,
            owner: Principal::anonymous(),
            collateral_chain: ChainId(71),
            custody_address: "0xc".into(),
            collateral_amount_native: 100,
            debt_e8s: 50,
            mint_recipient: "0xr".into(),
            pending_mint_e8s: 0,
            status: ChainVaultStatus::Open,
            opened_at_ns: 0,
            owner_evm: None,
            last_interest_accrual_ns: 0,
            pending_interest_mint_e8s: 0,
            pending_liquidation: Some(PendingLiquidationV1 {
                op_id,
                debt_to_clear_e8s: 50,
                collateral_reserved_native: 100,
                tier: LiquidationTier::Bot,
                started_at_ns: 0,
            }),
        },
    );
    s.bot_pending_chain_vaults.insert(vault_id, 0);
}

#[test]
fn claim_liquidation_swap_submit_marks_queued_op_inflight_before_broadcast() {
    let mut s = MultiChainState::default();
    let op_id = queued_liquidation_swap(&mut s, 7);
    marked_bot_liquidation(&mut s, 7, op_id);

    claim_liquidation_swap_submit_in_state(
        &mut s,
        ChainId(71),
        op_id,
        7,
        42,
        "0xabc".into(),
        9,
    )
    .expect("claim submit");

    let op = s.settlement_queues.get(&ChainId(71)).unwrap().pending.get(&op_id).unwrap();
    assert_eq!(op.last_tx_hash.as_deref(), Some("0xabc"));
    assert_eq!(op.submit_nonce, Some(9));
    assert!(matches!(
        op.status,
        SettlementOpStatus::Inflight { tries: 1, last_attempt_ns: 42 }
    ));
}

#[test]
fn claim_liquidation_swap_submit_rejects_if_observer_already_cleared_marker() {
    let mut s = MultiChainState::default();
    let op_id = queued_liquidation_swap(&mut s, 7);

    let err = claim_liquidation_swap_submit_in_state(
        &mut s,
        ChainId(71),
        op_id,
        7,
        42,
        "0xabc".into(),
        9,
    )
    .unwrap_err();

    assert_eq!(err, ClaimLiquidationSwapSubmitError::MissingMarker);
    let op = s.settlement_queues.get(&ChainId(71)).unwrap().pending.get(&op_id).unwrap();
    assert!(matches!(op.status, SettlementOpStatus::Queued));
    assert!(op.last_tx_hash.is_none());
    assert!(op.submit_nonce.is_none());
}

#[test]
fn claim_liquidation_swap_submit_rejects_live_inflight_op() {
    let mut s = MultiChainState::default();
    let op_id = queued_liquidation_swap(&mut s, 7);
    marked_bot_liquidation(&mut s, 7, op_id);
    s.settlement_queues
        .get_mut(&ChainId(71))
        .unwrap()
        .pending
        .get_mut(&op_id)
        .unwrap()
        .mark_inflight(41);

    let err = claim_liquidation_swap_submit_in_state(
        &mut s,
        ChainId(71),
        op_id,
        7,
        42,
        "0xabc".into(),
        9,
    )
    .unwrap_err();

    assert_eq!(err, ClaimLiquidationSwapSubmitError::NotQueued);
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

// ─── Increment 4 / Task 5a: SP chain-vault absorb state transition ───
mod sp_absorb_tests {
    use super::super::settlement::apply_sp_chain_liquidation_absorb_in_state;
    use crate::chains::config::ChainId;
    use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use crate::chains::multi_chain_state::MultiChainState;
    use candid::Principal;

    const CFX: ChainId = ChainId(1030);
    const E8: u128 = 100_000_000;
    const E18: u128 = 1_000_000_000_000_000_000;

    fn open_sp_attempted_vault(
        s: &mut MultiChainState,
        vault_id: u64,
        debt_e8s: u128,
        collateral_native: u128,
    ) {
        s.chain_vaults.insert(
            vault_id,
            ChainVaultV1 {
                vault_id,
                owner: Principal::anonymous(),
                collateral_chain: CFX,
                custody_address: "0x00000000000000000000000000000000000000c7".into(),
                collateral_amount_native: collateral_native,
                debt_e8s,
                mint_recipient: "0x00000000000000000000000000000000000000d7".into(),
                pending_mint_e8s: 0,
                status: ChainVaultStatus::Open,
                opened_at_ns: 0,
                owner_evm: None,
                last_interest_accrual_ns: 0,
                pending_interest_mint_e8s: 0,
                pending_liquidation: None,
            },
        );
        s.chain_supplies.insert(CFX, debt_e8s);
        s.sp_attempted_chain_vaults.insert(vault_id);
    }

    #[test]
    fn sp_absorb_moves_debt_to_pending_burn_and_reserves_claim_collateral() {
        let mut s = MultiChainState::default();
        open_sp_attempted_vault(&mut s, 7, 100 * E8, 1_000 * E18);

        let result = apply_sp_chain_liquidation_absorb_in_state(
            &mut s,
            CFX,
            7,
            100 * E8,
            50_000_000, // $0.50/CFX
            18,
            1_200,
        )
        .expect("sp absorb ok");

        assert_eq!(result.actual_burned_e8s, 100 * E8);
        assert_eq!(result.collateral_seized_native, 224 * E18);
        let v = s.chain_vaults.get(&7).unwrap();
        assert_eq!(v.debt_e8s, 0, "debt cleared by the IC-side SP burn");
        assert_eq!(v.collateral_amount_native, 776 * E18, "claim collateral reserved out of borrower balance");
        assert_eq!(*s.pending_chain_burn_e8s.get(&CFX).unwrap(), 100 * E8);
        assert_eq!(*s.chain_supplies.get(&CFX).unwrap(), 100 * E8, "foreign-chain supply accounting unchanged");
        assert!(v.pending_liquidation.is_none(), "SP absorb does not create a fake foreign-burn marker");

        let claim = s.chain_liquidation_claims.get(&7).expect("claim record");
        assert_eq!(claim.vault_id, 7);
        assert_eq!(claim.chain, CFX);
        assert_eq!(claim.custody_address, "0x00000000000000000000000000000000000000c7");
        assert_eq!(claim.seized_native_total, 224 * E18);
        assert_eq!(claim.paid_native, 0);
        assert!(claim.paid_within_seized());

        assert_eq!(
            *s.chain_supplies.get(&CFX).unwrap(),
            s.total_chain_vault_debt_e8s() + s.total_reserve_backing_e8s() + s.total_pending_chain_burn_e8s()
        );
    }

    #[test]
    fn sp_absorb_rejects_stale_overburn_without_mutation() {
        let mut s = MultiChainState::default();
        open_sp_attempted_vault(&mut s, 8, 80 * E8, 200 * E18);

        let before = s.clone();
        let err = apply_sp_chain_liquidation_absorb_in_state(
            &mut s,
            CFX,
            8,
            100 * E8,
            100_000_000, // $1.00/CFX
            18,
            1_200,
        )
        .unwrap_err();

        assert!(err.contains("does not match live debt"), "error should reject stale SP burn amount: {err}");
        assert_eq!(s.chain_vaults.get(&8).unwrap().debt_e8s, 80 * E8);
        assert_eq!(s.chain_vaults.get(&8).unwrap().collateral_amount_native, 200 * E18);
        assert_eq!(s.pending_chain_burn_e8s.get(&CFX), before.pending_chain_burn_e8s.get(&CFX));
        assert_eq!(s.chain_liquidation_claims.get(&8), before.chain_liquidation_claims.get(&8));
        assert_eq!(s.chain_supplies.get(&CFX), before.chain_supplies.get(&CFX));
    }

    #[test]
    fn sp_absorb_rejects_without_bot_escalation_no_mutation() {
        let mut s = MultiChainState::default();
        open_sp_attempted_vault(&mut s, 9, 100 * E8, 1_000 * E18);
        s.sp_attempted_chain_vaults.remove(&9);

        let err = apply_sp_chain_liquidation_absorb_in_state(&mut s, CFX, 9, 100 * E8, 50_000_000, 18, 1_200)
            .unwrap_err();

        assert!(err.contains("sp_attempted"), "error should name the missing escalation gate: {err}");
        let v = s.chain_vaults.get(&9).unwrap();
        assert_eq!(v.debt_e8s, 100 * E8);
        assert_eq!(v.collateral_amount_native, 1_000 * E18);
        assert!(s.pending_chain_burn_e8s.get(&CFX).is_none());
        assert!(s.chain_liquidation_claims.get(&9).is_none());
    }
}

// ─── Increment 4 / Task 6: enqueue SP claim payout from reserved CFX ───
mod chain_claim_tests {
    use super::super::settlement::claim_chain_collateral_in_state;
    use crate::chains::config::ChainId;
    use crate::chains::multi_chain_state::{ChainLiqClaimV1, MultiChainState};
    use crate::chains::settlement_queue::SettlementOpKind;
    use candid::Principal;

    const CFX: ChainId = ChainId(71);
    const E18: u128 = 1_000_000_000_000_000_000;

    fn valid_evm_address(a: &str) -> bool {
        a.starts_with("0x") && a.len() == 42
    }

    fn claimant() -> Principal {
        Principal::from_slice(&[7u8; 29])
    }

    fn state_with_claim() -> MultiChainState {
        let mut s = MultiChainState::default();
        s.chain_liquidation_claims.insert(7, ChainLiqClaimV1 {
            vault_id: 7,
            chain: CFX,
            custody_address: "0x00000000000000000000000000000000000000c7".into(),
            seized_native_total: 10 * E18,
            paid_native: 2 * E18,
        });
        s
    }

    #[test]
    fn claim_chain_collateral_enqueues_payout_and_marks_paid() {
        let mut s = state_with_claim();
        let dest = "0x0000000000000000000000000000000000000abc".to_string();

        let op_id = claim_chain_collateral_in_state(&mut s, 7, claimant(), 3 * E18, dest.clone(), 42, valid_evm_address)
            .expect("claim enqueued");

        let claim = s.chain_liquidation_claims.get(&7).unwrap();
        assert_eq!(claim.paid_native, 5 * E18, "claim is deducted before async payout");
        let op = s.settlement_queues.get(&CFX).unwrap().pending.get(&op_id).unwrap();
        match &op.kind {
            SettlementOpKind::ChainCollateralPayout { recipient, amount_e18, vault_id, claimant: c } => {
                assert_eq!(recipient, &dest);
                assert_eq!(*amount_e18, 3 * E18);
                assert_eq!(*vault_id, 7);
                assert_eq!(*c, claimant());
            }
            other => panic!("expected ChainCollateralPayout, got {other:?}"),
        }
    }

    #[test]
    fn claim_chain_collateral_rejects_duplicate_without_double_marking_paid() {
        let mut s = state_with_claim();
        let dest = "0x0000000000000000000000000000000000000abc".to_string();
        claim_chain_collateral_in_state(&mut s, 7, claimant(), 3 * E18, dest.clone(), 42, valid_evm_address)
            .expect("first claim enqueued");

        let err = claim_chain_collateral_in_state(&mut s, 7, claimant(), 3 * E18, dest, 43, valid_evm_address)
            .unwrap_err();

        assert!(err.contains("Duplicate"), "expected duplicate idempotency error, got {err}");
        assert_eq!(s.chain_liquidation_claims.get(&7).unwrap().paid_native, 5 * E18);
        assert_eq!(s.settlement_queues.get(&CFX).unwrap().pending.len(), 1);
    }

    #[test]
    fn claim_chain_collateral_validates_destination_before_mutation() {
        let mut s = state_with_claim();

        let err = claim_chain_collateral_in_state(&mut s, 7, claimant(), 3 * E18, "not-evm".into(), 42, valid_evm_address)
            .unwrap_err();

        assert!(err.contains("invalid EVM address"), "unexpected error: {err}");
        assert_eq!(s.chain_liquidation_claims.get(&7).unwrap().paid_native, 2 * E18);
        assert!(s.settlement_queues.get(&CFX).is_none());
    }
}
