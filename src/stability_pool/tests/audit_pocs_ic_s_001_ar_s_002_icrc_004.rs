//! Stability-pool audit fences (2026-06-09-e49ed10):
//!
//! IC-S-001: `deposit_as_3usd` refunds were best-effort: the GROSS amount was
//!   sent with fee:None (the ledger debits amount+fee, drifting the pool one
//!   fee below its tracked deposits per refund) and a failed refund was
//!   DISCARDED, stranding the user's pulled tokens with no record. The fix
//!   refunds net of the ledger fee (cached `icrc1_fee` with a conservative
//!   fallback, mirroring rumi_3pool::transfers) and persists a pending-refund
//!   record recoverable via `claim_pending_refund` / `get_pending_refunds`
//!   (mirroring rumi_3pool's pending-claims pattern).
//!
//! AR-S-002: `opt_in_collateral` / `opt_out_collateral` were the only
//!   synchronous permissionless mutations NOT gated on the SP liquidation
//!   guard, so an opt-out landing across a liquidation's await window changed
//!   the apportionment denominator (escape-the-burn + aggregate drift above
//!   the ledger). The fix gates both on
//!   `pool_guard::liquidation_in_progress()` (SystemBusy), the SP-102 idiom.
//!
//! ICRC-004 / SP-203: `claim_collateral` fell back to fee=0 when the
//!   `icrc1_fee` query failed, over-crediting the claimant by one ledger fee
//!   (inconsistent with the SP-104 conservative fallback on the liquidation
//!   gains path). The fix applies the same `FALLBACK_COLLATERAL_FEE_E8S`.
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
    let start = src.find(header).unwrap_or_else(|| panic!("`{}` not found", header));
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
        body.contains("refund_ledger_fee"),
        "refund_user must look up the ledger fee (cached icrc1_fee, conservative fallback) \
         (audit IC-S-001).",
    );
    assert!(
        body.contains("amount - fee"),
        "refund_user must send the amount NET of the ledger fee, not gross with fee:None \
         (a gross refund debits amount+fee from the pool) (audit IC-S-001).",
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
    for method in ["claim_pending_refund", "get_pending_refunds", "PendingRefund", "RefundClaimNotFound"] {
        assert!(
            did.contains(method),
            "stability_pool.did must declare `{}` (audit IC-S-001).",
            method,
        );
    }
}

#[test]
fn ic_s_001_claim_removes_record_before_transfer() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "pub async fn claim_pending_refund(");
    let take = body.find("take_pending_refund")
        .expect("claim_pending_refund must remove the record via take_pending_refund (audit IC-S-001)");
    let transfer = body.find("icrc1_transfer")
        .expect("claim_pending_refund must pay out via icrc1_transfer (audit IC-S-001)");
    assert!(
        take < transfer,
        "the record must be removed BEFORE the async transfer so two concurrent claims \
         cannot both pay out (audit IC-S-001).",
    );
    assert!(
        body.contains("put_pending_refund"),
        "a failed payout must re-insert the record so the user can retry (audit IC-S-001).",
    );
}

#[test]
fn ar_s_002_opt_endpoints_reject_during_liquidation() {
    let src = read("src/lib.rs");
    for header in [
        "pub fn opt_out_collateral(",
        "pub fn opt_in_collateral(",
    ] {
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
fn icrc_004_sp_203_claim_collateral_fee_fallback_is_conservative() {
    let src = read("src/deposits.rs");
    let body = fn_body(&src, "pub async fn claim_collateral(");
    assert!(
        body.contains("FALLBACK_COLLATERAL_FEE_E8S"),
        "claim_collateral's icrc1_fee failure path must use the SP-104 conservative \
         fallback, matching the liquidation gains path (audit ICRC-004 / SP-203).",
    );
    assert!(
        !body.contains("Err(_) => 0"),
        "claim_collateral must not fall back to fee=0 on icrc1_fee failure (over-credits \
         the claimant by one ledger fee) (audit ICRC-004 / SP-203).",
    );
}
