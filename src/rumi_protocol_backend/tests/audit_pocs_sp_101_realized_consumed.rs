//! SP-101 (audit 2026-06-03-0c3ceb4): the stability pool must debit depositors
//! by the debt the backend ACTUALLY cleared, not the amount it requested.
//!
//! The partial-liquidation backend paths cap the requested draw to the vault's
//! `max_liquidatable_debt`. The SP, however, recorded `actual_consumed = *amount`
//! (the requested draw) and `process_liquidation_gains` debited depositors by
//! that, so on a shallowly-underwater vault the tracked SP aggregate fell by
//! more than the pool actually spent (permanent depositor value destruction,
//! worst on the permissionless `execute_liquidation` path).
//!
//! Fix: the backend reports the realized debt via a new
//! `SuccessWithFee.debt_liquidated_e8s`, and the SP debits by it (converting the
//! icUSD e8s amount to the drawn token's native units for the stable path).
//!
//! These fences read both crates' source. NOTE: end-to-end validation (liquidate
//! a shallow vault and assert the SP aggregate drop == backend cap, and that
//! icrc1_balance_of(pool) == the tracked aggregate) wants a two-canister PocketIC
//! test; that is recommended before the (gated) coordinated backend+SP deploy.

use std::path::PathBuf;

#[test]
fn sp_101_backend_reports_realized_debt() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(
        lib.contains("debt_liquidated_e8s: Option<u64>"),
        "SuccessWithFee must carry debt_liquidated_e8s: Option<u64> so the SP can debit the \
         realized amount (audit SP-101)."
    );
    let vault = std::fs::read_to_string(root.join("src/vault.rs")).unwrap();
    let populated = vault
        .matches("debt_liquidated_e8s: Some(max_liquidatable_debt")
        .count();
    assert!(
        populated >= 2,
        "the SP-called partial-liquidation paths (liquidate_vault_partial and \
         liquidate_vault_partial_with_stable) must set debt_liquidated_e8s = \
         Some(max_liquidatable_debt) (audit SP-101); found {}",
        populated
    );
}

#[test]
fn sp_101_stability_pool_debits_realized_not_requested() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sp = std::fs::read_to_string(root.join("../stability_pool/src/liquidation.rs"))
        .expect("read stability_pool/src/liquidation.rs");
    assert!(
        sp.contains("success.debt_liquidated_e8s"),
        "the SP icUSD/stable liquidation path must debit by success.debt_liquidated_e8s (the \
         backend's realized, capped amount), not the requested draw (audit SP-101)."
    );
    assert!(
        sp.contains("denormalize_from_e8s"),
        "the SP must convert the realized icUSD e8s debt to the drawn token's native units for \
         the non-icUSD (ckUSDT/ckUSDC) path (audit SP-101)."
    );
}
