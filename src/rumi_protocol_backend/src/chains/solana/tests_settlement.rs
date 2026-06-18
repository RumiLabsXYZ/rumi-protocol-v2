//! Pure unit tests for the Solana settlement worker (Task 8).
//!
//! Mirrors `chains::monad::tests_settlement` where applicable. The async
//! `run_settlement` worker is covered by the Task 9 PocketIC test (the mock SOL
//! RPC + signing-subnet round trip); here we cover only the pure pieces:
//!
//! - the REUSE of the chain-agnostic `confirm_mint_in_state` / `select_next_op`
//!   helpers on a SOLANA-flavored `MultiChainStateV4` (a Solana mint confirm
//!   moves pending -> debt, increments `chain_supplies`, and flips the vault to
//!   `Open` - identical invariant to Monad, exercised here against the Solana
//!   chain id so the reuse is proven, not assumed);
//! - the `tx::first_signature_base58` local-signature derivation the submit path
//!   relies on for durable-nonce idempotency.

use crate::chains::config::ChainId;
use crate::chains::monad::settlement::{confirm_mint_in_state, select_next_op, OpAction};
use crate::chains::multi_chain_state::MultiChainStateV4;
use crate::chains::settlement_queue::{
    SettlementOp, SettlementOpKind, SettlementOpStatus, SettlementQueueV1,
};
use crate::chains::vault::{ChainVaultStatus, ChainVaultV1};
use candid::Principal;

/// Solana devnet chain id (mirrors `solana::config::SOLANA_CHAIN_ID`). Hardcoded
/// here so the test does not depend on the config constant staying 501.
const SOL: ChainId = ChainId(501);

/// Insert a Solana `MintPending` vault with the given pending mint amount.
/// `collateral_amount_native` is in lamports (Solana's 9-decimal base unit); a
/// confirm does not read it, but we set a realistic value for clarity.
fn solana_vault_pending(s: &mut MultiChainStateV4, vault_id: u64, pending_e8s: u128) {
    s.chain_vaults.insert(
        vault_id,
        ChainVaultV1 {
            vault_id,
            owner: Principal::anonymous(),
            collateral_chain: SOL,
            custody_address: "So11111111111111111111111111111111111111112".into(),
            collateral_amount_native: 2_000_000_000, // 2 SOL in lamports
            debt_e8s: 0,
            mint_recipient: "So11111111111111111111111111111111111111112".into(),
            pending_mint_e8s: pending_e8s,
            status: ChainVaultStatus::MintPending,
            opened_at_ns: 0,
            last_interest_accrual_ns: 0,
            pending_interest_mint_e8s: 0,
        },
    );
}

#[test]
fn solana_confirm_mint_moves_pending_to_debt_and_increments_supply() {
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(SOL, 0);
    solana_vault_pending(&mut s, 1, 10_000_000_000); // 100 icUSD pending (e8s)
                                                     // PRE-mint total_chain_vault_debt_e8s() == 0 (vault debt_e8s is still 0).
    confirm_mint_in_state(&mut s, SOL, 1, 10_000_000_000, 0).expect("confirm");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 10_000_000_000);
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Open));
    assert_eq!(s.chain_supplies[&SOL], 10_000_000_000);
}

#[test]
fn solana_confirm_mint_rejects_amount_mismatch_no_mutation() {
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(SOL, 0);
    solana_vault_pending(&mut s, 1, 10_000_000_000);
    // Observed != pending: reject before any supply mutation; nothing changes.
    let res = confirm_mint_in_state(&mut s, SOL, 1, 9_999_999_999, 0);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 10_000_000_000);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::MintPending));
    assert_eq!(s.chain_supplies[&SOL], 0);
}

#[test]
fn solana_confirm_mint_unknown_vault_rejected() {
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(SOL, 0);
    assert!(confirm_mint_in_state(&mut s, SOL, 999, 1, 0).is_err());
}

#[test]
fn solana_confirm_mint_second_vault_uses_running_total() {
    // Two Solana vaults: the first already confirmed (debt 100e8, supply 100e8);
    // confirming the second (pending 50e8) must pass PRE-mint total = 100e8, and
    // the helper computes the post-mint total 150e8 internally.
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(SOL, 10_000_000_000);
    s.chain_vaults.insert(
        1,
        ChainVaultV1 {
            vault_id: 1,
            owner: Principal::anonymous(),
            collateral_chain: SOL,
            custody_address: "So11111111111111111111111111111111111111112".into(),
            collateral_amount_native: 2_000_000_000,
            debt_e8s: 10_000_000_000,
            mint_recipient: "So11111111111111111111111111111111111111112".into(),
            pending_mint_e8s: 0,
            status: ChainVaultStatus::Open,
            opened_at_ns: 0,
            last_interest_accrual_ns: 0,
            pending_interest_mint_e8s: 0,
        },
    );
    solana_vault_pending(&mut s, 2, 5_000_000_000);
    let pre_total = s.total_chain_vault_debt_e8s(); // == 10e8
    confirm_mint_in_state(&mut s, SOL, 2, 5_000_000_000, pre_total).expect("confirm 2nd");
    assert_eq!(s.chain_vaults[&2].debt_e8s, 5_000_000_000);
    assert_eq!(s.chain_supplies[&SOL], 15_000_000_000);
}

// ─── select_next_op reuse on a Solana queue ──────────────────────────────────

fn solana_mint_op(key: &str, vault_id: u64) -> SettlementOp {
    SettlementOp::new(
        SettlementOpKind::Mint {
            recipient: "So11111111111111111111111111111111111111112".into(),
            amount_e8s: 10_000_000_000,
            vault_id,
        },
        key.into(),
        0,
    )
}

#[test]
fn solana_select_next_op_submits_queued_then_confirms_inflight() {
    let mut q = SettlementQueueV1::default();
    let id0 = q.enqueue(solana_mint_op("sol-k0", 1)).unwrap();
    let _id1 = q.enqueue(solana_mint_op("sol-k1", 2)).unwrap();

    // All Queued: the lowest op_id is selected for Submit.
    match select_next_op(&q) {
        Some((oid, OpAction::Submit)) => assert_eq!(oid, id0),
        other => panic!("expected Submit of op 0, got {other:?}"),
    }

    // Put op0 Inflight: now only the Confirm of op0 is actionable (one-in-flight).
    q.pending.get_mut(&id0).unwrap().status =
        SettlementOpStatus::Inflight { tries: 1, last_attempt_ns: 0 };
    match select_next_op(&q) {
        Some((oid, OpAction::Confirm)) => assert_eq!(oid, id0),
        other => panic!("expected Confirm of inflight op 0, got {other:?}"),
    }
}

// ─── first_signature_base58 (the submit path's idempotency primitive) ────────

#[test]
fn solana_first_signature_base58_is_the_first_signature() {
    use crate::chains::solana::tx::{assemble_wire_tx, first_signature_base58};
    let sig = [42u8; 64];
    let wire = assemble_wire_tx(sig, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let got = first_signature_base58(&wire).expect("derive signature");
    assert_eq!(got, bs58::encode(sig).into_string());
}
