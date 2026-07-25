//! Stability-pool audit fences (2026-06-05-d67e100):
//!
//! SP-102: a liquidation snapshots depositor balances, awaits the backend, then
//!   apportions the burn against the LIVE set. deposit/withdraw/claim ran across
//!   that window, letting a withdrawer escape their share. The fix is a per-pool
//!   reentrancy guard (`SpLiquidationGuard`): the liquidation holds it across the
//!   await; deposit/withdraw/claim reject (`SystemBusy`) while it is held.
//!
//! VER-002 / SP-110: the SP recorded the REQUESTED draw (3USD path) and only the
//!   base ckStable debt (ckStable path) as consumed, ignoring the realized
//!   amount the backend actually pulled (the 3USD writedown is capped + excess
//!   refunded; the ckStable pull includes a repay-fee surcharge). The fix records
//!   the realized amount (`liquidated_debt` / `stable_pulled_e6s`).
//!
//! SP-104: a failed `icrc1_fee` query fell back to fee=0, over-crediting gains.
//!   The fix uses a conservative fallback so we under- rather than over-credit.
//!
//! Source fences (the end-to-end paths need a PocketIC + failing-ledger harness;
//! see the report). They FAIL on pre-fix source.

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
fn sp_102_liquidation_entries_hold_reentrancy_guard() {
    let src = read("src/liquidation.rs");
    for header in [
        "pub async fn notify_liquidatable_vaults(",
        "pub async fn execute_liquidation(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("SpLiquidationGuard::new"),
            "liquidation entry `{}` must hold the per-pool SpLiquidationGuard across its await \
             so deposit/withdraw/claim cannot race the burn apportionment (audit SP-102).",
            header
        );
    }
}

/// Idioms a balance-mutating entry may use to consult the SP-102 liquidation
/// guard. Originally every entry called `pool_guard::liquidation_in_progress()`
/// inline; `006342d` ("Inc8 retry-safe SP chain absorb") factored that into
/// `pool_balance_mutation_blocked()` / `ensure_pool_balance_mutation_allowed()`,
/// which OR the guard with the pending-chain-absorb check. Accepting any of the
/// three keeps this fence from bit-rotting on a refactor; the companion test
/// below asserts the indirection still reaches the guard. Pre-fix source
/// contained none of these, so the fence still fails there.
const GUARD_IDIOMS: [&str; 3] = [
    "liquidation_in_progress",
    "pool_balance_mutation_blocked",
    "ensure_pool_balance_mutation_allowed",
];

#[test]
fn sp_102_balance_ops_reject_during_liquidation() {
    let src = read("src/deposits.rs");
    for header in [
        "pub async fn deposit(",
        "pub async fn withdraw(",
        "pub async fn claim_collateral(",
        "pub async fn claim_all_collateral(",
        "pub async fn deposit_as_3usd(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            GUARD_IDIOMS.iter().any(|idiom| body.contains(idiom)),
            "balance-mutating entry `{}` must reject while a liquidation is apportioning, \
             via one of {:?} (audit SP-102).",
            header,
            GUARD_IDIOMS,
        );
    }
}

/// The guard checks above may be indirections, so assert the helpers they route
/// through still consult `pool_guard::liquidation_in_progress()` and still
/// reject with `SystemBusy`. Without this, the fence could be satisfied by a
/// helper that had been changed to always return false.
#[test]
fn sp_102_balance_mutation_helpers_route_to_the_liquidation_guard() {
    let src = read("src/lib.rs");
    let blocked = fn_body(&src, "fn pool_balance_mutation_blocked(");
    assert!(
        blocked.contains("liquidation_in_progress"),
        "pool_balance_mutation_blocked() must consult pool_guard::liquidation_in_progress() \
         (audit SP-102).",
    );
    let ensure = fn_body(&src, "fn ensure_pool_balance_mutation_allowed(");
    assert!(
        ensure.contains("pool_balance_mutation_blocked") && ensure.contains("SystemBusy"),
        "ensure_pool_balance_mutation_allowed() must reject with SystemBusy when \
         pool_balance_mutation_blocked() (audit SP-102).",
    );
}

#[test]
fn ver_002_sp_110_records_realized_not_requested() {
    let src = read("src/liquidation.rs");
    // 3USD path (VER-002): must derive consumption from the realized liquidated_debt.
    assert!(
        src.contains("success.liquidated_debt < icusd_equiv_e8s"),
        "the 3USD reserves path must record the REALIZED 3USD (from liquidated_debt), \
         not the requested draw (audit VER-002).",
    );
    // ckStable path (SP-110): must use the backend's actual stable pulled.
    assert!(
        src.contains("stable_pulled_e6s"),
        "the ckStable path must debit the TOTAL stable pulled (base + repay-fee surcharge) \
         via stable_pulled_e6s (audit SP-110).",
    );
}

#[test]
fn sp_104_fee_query_failure_does_not_fall_back_to_zero() {
    let src = read("src/liquidation.rs");
    assert!(
        src.contains("FALLBACK_COLLATERAL_FEE_E8S"),
        "a failed icrc1_fee query must use a conservative non-zero fallback, not fee=0 \
         (which over-credits gains) (audit SP-104).",
    );
}
