//! 3pool audit fences (2026-06-05-d67e100):
//!
//! 3P-01/02/03 (SAT-001/002): swap / add_liquidity / remove_liquidity /
//!   remove_one_coin pulled or debited user funds, then on a failed ledger
//!   transfer returned early with NO refund and NO recovery record — stranding
//!   the funds (the rumi_amm handles this; the 3pool did not). The fix refunds
//!   or records a pending claim on every failure path, recoverable via
//!   `claim_pending`.
//!
//! SAT-007: the 3USD token rejected EVERY same-owner transfer
//!   (`caller == to.owner`), breaking the "send 3USD with Internet Identity"
//!   flow. Balances are keyed by owner only, so a same-owner transfer is a
//!   net-zero no-op; the over-broad guard is removed.
//!
//! SAT-006: `icrc3_get_blocks` capped each range to the log length but not the
//!   total across an unbounded `args` vector; the fix bounds the total response.
//!
//! These are source-level fences (the behavioral paths need a failing-ledger
//! PocketIC harness; see report for the end-to-end repro). They FAIL on pre-fix
//! source.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn fn_body<'a>(src: &'a str, header: &'a str) -> &'a str {
    let start = src.find(header).unwrap_or_else(|| panic!("`{}` not found", header));
    let after = start + header.len();
    let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn ", "\n#[update]", "\n#[query]"]
        .iter()
        .filter_map(|m| src[after..].find(m).map(|i| after + i))
        .min()
        .unwrap_or(src.len());
    &src[start..end]
}

#[test]
fn p3_recovery_value_moving_paths_record_pending_claim() {
    let src = read("src/lib.rs");
    for header in [
        "pub async fn swap(",
        "pub async fn add_liquidity(",
        "pub async fn remove_liquidity(",
        "pub async fn remove_one_coin(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("record_pending_claim"),
            "3pool entry `{}` must refund or record_pending_claim on a failed transfer so user \
             funds are recoverable, not stranded (audit 3P-01/02/03).",
            header
        );
    }
    // The recovery endpoint and its query must exist.
    assert!(src.contains("pub async fn claim_pending("), "claim_pending recovery endpoint missing");
    assert!(src.contains("pub fn get_pending_claims("), "get_pending_claims query missing");
}

#[test]
fn p3_withdraw_admin_fees_holds_pool_guard() {
    // 3P-04: concurrent admin withdrawals could double-pay accrued fees.
    let src = read("src/admin.rs");
    let body = fn_body(&src, "pub async fn withdraw_admin_fees(");
    assert!(
        body.contains("PoolGuard::new"),
        "withdraw_admin_fees must hold the PoolGuard so concurrent calls cannot double-pay fees \
         (audit 3P-04)."
    );
}

#[test]
fn sat_007_3usd_allows_same_owner_transfer() {
    // The over-broad "cannot transfer to self" rejection must be gone.
    let src = read("src/icrc_token.rs");
    assert!(
        !src.contains("cannot transfer to self"),
        "3USD must not reject same-owner transfers (the Internet Identity send bug); balances are \
         owner-keyed so a same-owner transfer is a safe net-zero no-op (audit SAT-007)."
    );
}

#[test]
fn sat_006_icrc3_get_blocks_is_bounded() {
    let src = read("src/icrc3.rs");
    assert!(
        src.contains("MAX_GET_BLOCKS_RESPONSE"),
        "icrc3_get_blocks must cap the total blocks returned across all args (audit SAT-006)."
    );
}
