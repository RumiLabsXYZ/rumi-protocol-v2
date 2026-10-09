//! Stability-pool audit fences (2026-06-09-e49ed10):
//!
//! IC-S-001: `deposit_as_3usd` refunds were best-effort: the GROSS amount was
//!   sent with fee:None (the ledger debits amount+fee, drifting the pool one
//!   fee below its tracked deposits per refund) and a failed refund was
//!   DISCARDED, stranding the user's pulled tokens with no record. Refunds
//!   query the current `icrc1_fee`, send the net amount with an explicit fee,
//!   and persist a pending-refund record recoverable via `claim_pending_refund`
//!   / `get_pending_refunds`
//!   (mirroring rumi_3pool's pending-claims pattern).
//!
//! AR-S-002: `opt_in_collateral` / `opt_out_collateral` were the only
//!   synchronous permissionless mutations NOT gated on the SP liquidation
//!   guard, so an opt-out landing across a liquidation's await window changed
//!   the apportionment denominator (escape-the-burn + aggregate drift above
//!   the ledger). The fix gates both on
//!   `pool_guard::liquidation_in_progress()` (SystemBusy), the SP-102 idiom.
//!
//! SP claim transport ambiguity: a lost ICRC-1 reply restored already-paid
//!   collateral gains. The fix persists the exact payout tuple before dispatch,
//!   holds ambiguous outcomes, retries the identical tuple, and fails closed
//!   when `icrc1_fee` cannot be queried.
//!
//! Source fences (the end-to-end paths need a PocketIC + failing-ledger
//! harness); state-level regression tests live in `src/state.rs`
//! (`ic_s_001_*`, `ar_s_002_opt_out_mid_liquidation_escapes_burn`). They FAIL
//! on pre-fix source.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn fn_body<'a>(src: &'a str, header: &'a str) -> &'a str {
    let start = src
        .find(header)
        .unwrap_or_else(|| panic!("`{}` not found", header));
    let after = start + header.len();
    let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
        .iter()
        .filter_map(|m| src[after..].find(m).map(|i| after + i))
        .min()
        .unwrap_or(src.len());
    &src[start..end]
}

#[test]
fn ic_s_001_refund_is_net_of_ledger_fee() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "async fn refund_user(");
    assert!(
        body.contains("current_ledger_transfer_fee"),
        "refund_user must fetch the current ledger fee rather than use a potentially stale cache \
         (audit IC-S-001).",
    );
    let tuple_builder = fn_body(&src, "fn build_pending_refund_attempt(");
    assert!(
        tuple_builder.contains("refund.amount - fee")
            && body.contains("amount: attempt.amount.into()"),
        "refund_user must send the amount NET of the ledger fee, not gross with fee:None \
         (a gross refund debits amount+fee from the pool) (audit IC-S-001).",
    );
    assert!(
        body.contains("fee: Some(attempt.fee.into())"),
        "refund_user must pass the quoted fee explicitly so a concurrent fee change fails with BadFee \
         instead of creating an untracked debit (audit IC-S-001).",
    );
}

#[test]
fn ic_s_001_failed_refund_records_pending_recovery() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "async fn refund_user(");
    assert!(
        body.contains("record_pending_refund"),
        "a failed refund transfer must persist a per-user recovery record, not strand the \
         pulled tokens (audit IC-S-001).",
    );
    assert!(
        !body.contains("let _ = call"),
        "refund_user must not discard the transfer result (audit IC-S-001).",
    );
    let row_journal = body
        .find("record_pending_refund(")
        .expect("refund liability is journaled before setup awaits");
    let exact_attempt = body
        .find("put_pending_refund_attempt(attempt.clone())")
        .expect("exact payout tuple is persisted before dispatch");
    let transfer_dispatch = body
        .find("call(attempt.token_ledger, \"icrc1_transfer\"")
        .expect("refund transfer dispatch exists");
    assert!(
        row_journal < exact_attempt && exact_attempt < transfer_dispatch,
        "the initial compensation must journal the row and exact tuple before a possibly committed transfer reply is awaited",
    );
    assert!(body.contains("attempt.dispatch_started = true"));
}

#[test]
fn ic_s_001_recovery_endpoints_exist_and_are_declared() {
    let lib = read("src/lib.rs");
    assert!(
        lib.contains("pub async fn claim_pending_refund("),
        "the SP must expose a user-callable claim_pending_refund endpoint (audit IC-S-001).",
    );
    assert!(
        lib.contains("pub fn get_pending_refunds("),
        "the SP must expose a get_pending_refunds query (audit IC-S-001).",
    );
    let did = read("stability_pool.did");
    for method in [
        "claim_pending_refund",
        "get_pending_refunds",
        "PendingRefund",
        "RefundClaimNotFound",
    ] {
        assert!(
            did.contains(method),
            "stability_pool.did must declare `{}` (audit IC-S-001).",
            method,
        );
    }
}

#[test]
fn ic_s_001_claim_keeps_record_until_exact_receipt() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "pub async fn claim_pending_refund(");
    assert!(
        !body.contains("take_pending_refund"),
        "claim_pending_refund must keep the liability row during the payout await (audit IC-S-001).",
    );
    assert!(
        body.contains("pending_refund_attempt") && body.contains("update_pending_refund_attempt"),
        "claim_pending_refund must persist and update a stable exact-tuple attempt (audit IC-S-001).",
    );
    assert!(
        body.contains("fetch_icrc3_block") && body.contains("expected_block_index"),
        "claim_pending_refund must verify ledger history before discharging the liability (audit IC-S-001).",
    );
    assert!(
        body.contains("pending_refund_attempt_initializable")
            && body.contains("legacy refund has no durable payout identity"),
        "legacy attemptless refunds must fail closed instead of inventing a fresh payout identity",
    );
    let balance_guard = body
        .find("PoolBalanceAsyncGuard::new()")
        .expect("refund claim must hold the shared-balance async guard");
    let first_await = body.find(".await").expect("claim has ledger awaits");
    assert!(
        balance_guard < first_await,
        "guard must span every claim await"
    );
}

#[test]
fn ar_s_002_opt_endpoints_reject_during_liquidation() {
    let src = read("src/lib.rs");
    for header in ["pub fn opt_out_collateral(", "pub fn opt_in_collateral("] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("liquidation_in_progress"),
            "opt endpoint `{}` changes the apportionment denominator and must reject \
             (SystemBusy) while a liquidation is apportioning (audit AR-S-002).",
            header
        );
    }
}

#[test]
fn sp_claim_requires_live_fee_and_persists_exact_payout_identity() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "pub async fn claim_collateral(");
    assert!(
        body.contains("icrc1_fee"),
        "collateral claims must discover the ledger fee"
    );
    assert!(
        body.contains("claim gains unchanged"),
        "fee-query failure must leave gains unreserved"
    );
    assert!(
        body.contains("prepare_collateral_payout"),
        "claim must persist the debit and exact tuple before dispatch"
    );
    assert!(
        body.contains("begin_outbound_payout_retry"),
        "legacy claim endpoint must retry its held tuple"
    );
    assert!(
        !body.contains("FALLBACK_COLLATERAL_FEE_E8S"),
        "claim must not guess a fee such as the 10,000-unit fallback"
    );
    let transfer = fn_body(&src, "fn outbound_transfer_args(");
    assert!(
        transfer.contains("fee: Some(payout.transfer_fee.into())"),
        "transfer must pin the discovered fee in the immutable tuple"
    );
    assert!(
        transfer.contains("created_at_time: Some(payout.transfer_created_at_time_ns)"),
        "retry must reuse the exact ICRC dedup timestamp"
    );
    let claim_all = fn_body(&src, "pub async fn claim_all_collateral(");
    assert!(claim_all.contains("pending_outbound_payouts"), "claim_all must retry pending rows with zero visible gains");
    assert!(src.contains("verify_collateral_payout_block"), "Duplicate and aged reconciliation need exact ledger evidence");
    assert!(src.contains("native_icp_proof::query_block"), "native ICP claims must use the legacy query_blocks proof path");
    assert!(src.contains("archive-backed ICRC-3 response is unsupported"), "ICRC-3 archive proof must remain held without certified membership");

    let did = read("stability_pool.did");
    assert!(did.contains("claim_collateral : (principal) -> (variant { Ok : nat64; Err : StabilityPoolError });"), "legacy claim_collateral Candid signature must remain unchanged");
    assert!(did.contains("reconcile_collateral_claim : (principal, principal, nat64)"), "exact candidate-block reconciliation must be additive");
}
