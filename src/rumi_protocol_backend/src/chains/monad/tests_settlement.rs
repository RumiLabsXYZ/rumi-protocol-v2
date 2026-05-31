use super::settlement::{confirm_mint_in_state, select_next_op, OpAction};
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind, SettlementOpStatus};
use candid::Principal;

fn vault_pending(s: &mut MultiChainStateV2, vault_id: u64, pending: u128) {
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_e18: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: pending,
        status: ChainVaultStatus::MintPending, opened_at_ns: 0,
    });
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
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000); // 100 icUSD pending
    // PRE-mint total_chain_vault_debt_e8s() == 0 (vault debt_e8s is still 0).
    // confirm_mint_in_state adds observed_e8s internally to get the post-mint total.
    confirm_mint_in_state(&mut s, ChainId(10143), 1, 10_000_000_000, 0).expect("confirm");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 10_000_000_000);
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Open));
    assert_eq!(s.chain_supplies[&ChainId(10143)], 10_000_000_000);
}

#[test]
fn confirm_mint_rejects_amount_mismatch() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000);
    // Observed amount differs from pending: reject (caught before any supply mutation), do not mutate.
    let res = confirm_mint_in_state(&mut s, ChainId(10143), 1, 9_999_999_999, 0);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 10_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 0);
}

#[test]
fn confirm_mint_unknown_vault_rejected() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    assert!(confirm_mint_in_state(&mut s, ChainId(10143), 999, 1, 0).is_err());
}

#[test]
fn confirm_mint_second_vault_uses_running_total() {
    // Two vaults: first already confirmed (debt 100e8, supply 100e8). Confirming the
    // second (pending 50e8) must pass PRE-mint total = 100e8; helper computes 150e8.
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 10_000_000_000);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_e18: 0, debt_e8s: 10_000_000_000,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
    });
    vault_pending(&mut s, 2, 5_000_000_000);
    let pre_total = s.total_chain_vault_debt_e8s(); // == 10e8
    confirm_mint_in_state(&mut s, ChainId(10143), 2, 5_000_000_000, pre_total).expect("confirm 2nd");
    assert_eq!(s.chain_vaults[&2].debt_e8s, 5_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 15_000_000_000);
}
