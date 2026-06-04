//! ORACLE-001 (audit 2026-06-03-0c3ceb4): mint/withdraw must gate on a fresh
//! per-collateral price.
//!
//! Audit pass:
//!   `audit-reports/2026-06-03-0c3ceb4/raw-pass-results/01-oracle-cr-mint.json`
//!   finding ORACLE-001.
//!
//! # The bug
//!
//! `validate_call()` only refreshes the ICP price. The prior audit (RED-001 /
//! LIQ-006) retrofitted a per-collateral freshness gate
//! (`xrc::ensure_fresh_price_for`) onto the redemption and liquidation paths,
//! but the debt-increasing / collateral-decreasing endpoints were never
//! updated. So for a non-ICP collateral whose background price fetch has been
//! failing, a borrower could mint icUSD (or withdraw collateral) priced against
//! an arbitrarily stale `last_price` — over-minting against a feed that has
//! since crashed.
//!
//! # The fix these fences pin
//!
//! Every such endpoint must refresh the relevant collateral price first:
//!   * by-vault-id endpoints (`borrow_from_vault`, `withdraw_collateral`,
//!     `withdraw_partial_collateral`) use `validate_freshness_for_vault`;
//!   * the open-* endpoints (`open_vault_and_borrow`, `open_vault_with_deposit`)
//!     take the collateral directly, so they use
//!     `validate_freshness_for_collateral`.
//!
//! Both helpers ultimately call `xrc::ensure_fresh_price_for`. The structural
//! fences below read `src/main.rs` and assert each endpoint body invokes a
//! freshness gate; they FAIL on pre-fix main and PASS post-fix.

use std::path::PathBuf;

fn read_main_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Slice a free `async fn` body from its declaration to the next top-level item.
fn fn_body<'a>(source: &'a str, fn_name: &str) -> &'a str {
    let header = format!("async fn {}(", fn_name);
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("function `{}` not found in main.rs", fn_name));
    let after = start + header.len();
    let end = ["\nasync fn ", "\nfn ", "\npub async fn ", "\npub fn "]
        .iter()
        .filter_map(|m| source[after..].find(m).map(|i| after + i))
        .min()
        .unwrap_or(source.len());
    &source[start..end]
}

/// A freshness gate is any of the per-collateral refresh helpers.
fn gates_freshness(body: &str) -> bool {
    body.contains("validate_freshness_for_vault")
        || body.contains("validate_freshness_for_collateral")
        || body.contains("ensure_fresh_price_for")
}

#[test]
fn oracle_001_borrow_from_vault_gates_freshness() {
    let m = read_main_rs();
    let body = fn_body(&m, "borrow_from_vault");
    assert!(
        gates_freshness(body),
        "borrow_from_vault must refresh the vault collateral price before minting \
         (audit ORACLE-001). Use validate_freshness_for_vault(arg.vault_id).await?.\n\n{}",
        body
    );
}

#[test]
fn oracle_001_open_vault_and_borrow_gates_freshness() {
    let m = read_main_rs();
    let body = fn_body(&m, "open_vault_and_borrow");
    assert!(
        gates_freshness(body),
        "open_vault_and_borrow must refresh the collateral price before minting \
         (audit ORACLE-001).\n\n{}",
        body
    );
}

#[test]
fn oracle_001_open_vault_with_deposit_gates_freshness() {
    let m = read_main_rs();
    let body = fn_body(&m, "open_vault_with_deposit");
    assert!(
        gates_freshness(body),
        "open_vault_with_deposit must refresh the collateral price before minting \
         (audit ORACLE-001).\n\n{}",
        body
    );
}

#[test]
fn oracle_001_withdraw_collateral_gates_freshness() {
    let m = read_main_rs();
    let body = fn_body(&m, "withdraw_collateral");
    assert!(
        gates_freshness(body),
        "withdraw_collateral must refresh the vault collateral price before \
         releasing collateral (audit ORACLE-001).\n\n{}",
        body
    );
}

#[test]
fn oracle_001_withdraw_partial_collateral_gates_freshness() {
    let m = read_main_rs();
    let body = fn_body(&m, "withdraw_partial_collateral");
    assert!(
        gates_freshness(body),
        "withdraw_partial_collateral must refresh the vault collateral price \
         before releasing collateral (audit ORACLE-001).\n\n{}",
        body
    );
}
