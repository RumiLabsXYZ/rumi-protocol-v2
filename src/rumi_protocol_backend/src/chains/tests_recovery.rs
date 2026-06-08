//! Pure-state tests for the on-chain-verified recovery helpers (audit M-08 /
//! RECOV-01 and M-09 / RECOV-02). The async verification orchestrators
//! (`resolve_stuck_settlement_op_verified`, `recover_stuck_chain_vault_verified`)
//! make EVM-RPC quorum calls and are covered by PocketIC; here we exercise the
//! pure reversal + precheck + transition helpers they delegate to, plus the
//! state-machine guards that prevent releasing collateral / reversing a landed
//! mint.

use super::config::ChainId;
use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use super::multi_chain_state::MultiChainStateV4;
use super::recovery::{
    apply_recover_vault_in_state, apply_resolve_reversal_in_state,
    precheck_recover_vault_in_state, RecoveryError,
};
use super::settlement_queue::{SettlementOp, SettlementOpKind, SettlementOpStatus};
use candid::Principal;

const CHAIN: ChainId = ChainId(10143);

fn vault(vault_id: u64, status: ChainVaultStatus, pending: u128, collateral: u128) -> ChainVaultV1 {
    ChainVaultV1 {
        vault_id,
        owner: Principal::anonymous(),
        collateral_chain: CHAIN,
        custody_address: "0xcustody".into(),
        collateral_amount_native: collateral,
        debt_e8s: 0,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: pending,
        status,
        opened_at_ns: 0,
    }
}

fn inflight_op(op_id: u64, kind: SettlementOpKind, tx: Option<&str>) -> SettlementOp {
    let mut op = SettlementOp::new(kind, format!("key-{op_id}"), 0);
    op.op_id = op_id;
    op.status = SettlementOpStatus::Inflight { tries: 1, last_attempt_ns: 0 };
    op.last_tx_hash = tx.map(|s| s.to_string());
    op
}

fn state_with_op(op: SettlementOp) -> MultiChainStateV4 {
    let mut s = MultiChainStateV4::default();
    let mut q = super::settlement_queue::SettlementQueueV1::default();
    let id = op.op_id;
    q.pending.insert(id, op);
    q.tail = id + 1;
    s.settlement_queues.insert(CHAIN, q);
    s
}

// ── M-08: apply_resolve_reversal_in_state ─────────────────────────────────────

#[test]
fn resolve_reversal_mint_clears_pending_and_marks_failed() {
    let mut s = state_with_op(inflight_op(
        0,
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 5, vault_id: 1 },
        Some("0xtx"),
    ));
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 5, 0));

    let did = apply_resolve_reversal_in_state(&mut s, CHAIN, 0, 100);
    assert!(did, "first reversal applies");
    // Pending cleared, status stays MintPending (Design B: no debt was counted).
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 0);
    assert_eq!(s.chain_vaults[&1].status, ChainVaultStatus::MintPending);
    // Op marked Failed.
    assert!(matches!(
        s.settlement_queues[&CHAIN].pending[&0].status,
        SettlementOpStatus::Failed { .. }
    ));
}

#[test]
fn resolve_reversal_withdrawal_restores_collateral_and_reopens() {
    let mut s = state_with_op(inflight_op(
        0,
        SettlementOpKind::NativeWithdrawal { recipient: "0xr".into(), amount_e18: 1_000, vault_id: 1 },
        Some("0xtx"),
    ));
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::Closing, 0, 0));

    let did = apply_resolve_reversal_in_state(&mut s, CHAIN, 0, 100);
    assert!(did);
    // Reserved collateral added back; Closing -> Open.
    assert_eq!(s.chain_vaults[&1].collateral_amount_native, 1_000);
    assert_eq!(s.chain_vaults[&1].status, ChainVaultStatus::Open);
}

#[test]
fn resolve_reversal_is_idempotent_cas() {
    // A second reversal (op already Failed) is a no-op and does NOT double-credit
    // the withdrawal collateral (the CAS guard prevents 2x amount_e18).
    let mut s = state_with_op(inflight_op(
        0,
        SettlementOpKind::NativeWithdrawal { recipient: "0xr".into(), amount_e18: 1_000, vault_id: 1 },
        Some("0xtx"),
    ));
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::Closing, 0, 0));

    assert!(apply_resolve_reversal_in_state(&mut s, CHAIN, 0, 100));
    let second = apply_resolve_reversal_in_state(&mut s, CHAIN, 0, 200);
    assert!(!second, "second reversal is a no-op (op no longer Inflight)");
    assert_eq!(
        s.chain_vaults[&1].collateral_amount_native, 1_000,
        "collateral credited exactly once"
    );
}

// ── M-09: precheck + transition ───────────────────────────────────────────────

#[test]
fn precheck_recover_returns_terminal_mint_tx_hashes() {
    // A terminal (Failed) Mint op for the vault carries a tx hash the async path
    // must re-verify on-chain.
    let mut s = MultiChainStateV4::default();
    let mut q = super::settlement_queue::SettlementQueueV1::default();
    let mut op = inflight_op(
        0,
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 5, vault_id: 1 },
        Some("0xmint_tx"),
    );
    op.status = SettlementOpStatus::Failed { reason: "x".into(), failed_ns: 1 };
    q.pending.insert(0, op);
    q.tail = 1;
    s.settlement_queues.insert(CHAIN, q);
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 0, 9));

    let hashes = precheck_recover_vault_in_state(&s, CHAIN, 1).expect("precheck ok");
    assert_eq!(hashes, vec!["0xmint_tx".to_string()]);
}

#[test]
fn precheck_recover_rejects_live_mint_op() {
    let mut s = state_with_op(inflight_op(
        0,
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 5, vault_id: 1 },
        Some("0xtx"),
    )); // Inflight == live
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 0, 9));
    let err = precheck_recover_vault_in_state(&s, CHAIN, 1).expect_err("live mint");
    assert!(matches!(err, RecoveryError::LiveMintOp(1)));
}

#[test]
fn precheck_recover_rejects_nonzero_pending_or_wrong_status() {
    let mut s = MultiChainStateV4::default();
    // Nonzero pending => not recoverable.
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 7, 9));
    assert!(matches!(
        precheck_recover_vault_in_state(&s, CHAIN, 1),
        Err(RecoveryError::NotRecoverable(_))
    ));
    // Wrong status (Open) => not recoverable.
    s.chain_vaults.insert(2, vault(2, ChainVaultStatus::Open, 0, 9));
    assert!(matches!(
        precheck_recover_vault_in_state(&s, CHAIN, 2),
        Err(RecoveryError::NotRecoverable(_))
    ));
}

#[test]
fn precheck_recover_rejects_wrong_chain_and_unknown() {
    let mut s = MultiChainStateV4::default();
    let mut v = vault(1, ChainVaultStatus::MintPending, 0, 9);
    v.collateral_chain = ChainId(999);
    s.chain_vaults.insert(1, v);
    assert!(matches!(
        precheck_recover_vault_in_state(&s, CHAIN, 1),
        Err(RecoveryError::WrongChain { vault_id: 1, .. })
    ));
    assert!(matches!(
        precheck_recover_vault_in_state(&s, CHAIN, 42),
        Err(RecoveryError::UnknownVault(42))
    ));
}

#[test]
fn apply_recover_vault_flips_mintpending_to_open() {
    let mut s = MultiChainStateV4::default();
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 0, 9));
    apply_recover_vault_in_state(&mut s, CHAIN, 1).expect("recover");
    assert_eq!(s.chain_vaults[&1].status, ChainVaultStatus::Open);
}

#[test]
fn apply_recover_vault_rechecks_guards_at_commit() {
    // A live mint op enqueued between precheck and commit must block the flip
    // (defense-in-depth re-check inside apply_recover_vault_in_state).
    let mut s = state_with_op(inflight_op(
        0,
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 5, vault_id: 1 },
        Some("0xtx"),
    ));
    s.chain_vaults.insert(1, vault(1, ChainVaultStatus::MintPending, 0, 9));
    let err = apply_recover_vault_in_state(&mut s, CHAIN, 1).expect_err("live mint at commit");
    assert!(matches!(err, RecoveryError::LiveMintOp(1)));
    // Vault NOT flipped.
    assert_eq!(s.chain_vaults[&1].status, ChainVaultStatus::MintPending);
}
