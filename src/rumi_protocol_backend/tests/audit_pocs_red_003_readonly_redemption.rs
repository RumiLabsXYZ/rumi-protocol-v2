//! Wave-9 RED-003: redemption endpoints must respect protocol mode.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/redemption-peg.json`
//!     finding RED-003.
//!
//! # What the bug was
//!
//! `validate_mode()` (defined at `src/main.rs::validate_mode`) gates
//! state-mutating endpoints to `Mode::GeneralAvailability` /
//! `Mode::Recovery` and rejects when `state.mode == ReadOnly`. It is
//! called from `borrow_from_vault`, `withdraw_partial_collateral`,
//! `add_margin`, `open_vault_with_deposit`, and `open_vault_and_borrow`.
//!
//! `redeem_collateral` and `redeem_reserves` did NOT call it. ReadOnly
//! is the auto-latched state when total collateral ratio drops below
//! 100% (Wave-1 latch) or, post-Wave-8e, when the deficit account
//! crosses its admin-configured threshold. Allowing redemption while
//! ReadOnly meant a redeemer could continue extracting collateral from
//! a protocol that was already insolvent, deepening the bad-debt
//! position.
//!
//! # How this file tests the fix
//!
//! The fix is a one-line change at the two redemption entry points in
//! `src/main.rs`. This file pins:
//!
//!   1. The state-side invariant that `Mode::ReadOnly` is a distinct
//!      variant detectable by the caller.
//!   2. A structural fence reading `src/main.rs` directly: both
//!      `redeem_collateral` and `redeem_reserves` MUST contain a
//!      `validate_mode()?` call after the fix lands. The structural
//!      fence will FAIL on pre-fix main and PASS post-fix.
//!
//! # RED-101 regression (audit 2026-06-03-0c3ceb4): the ICP path was open
//!
//! The Wave-9 fix gated only the two #[update] endpoints above. The
//! sibling endpoint `redeem_icp` (main.rs) called only `validate_call()`
//! and then `vault::redeem_icp`, a thin wrapper over
//! `vault::redeem_collateral`, and NEITHER performed a `Mode::ReadOnly`
//! check. ICP is the tier-1, highest-TVL collateral and the first asset
//! returned by `get_collateral_types_by_redemption_priority`, so the
//! primary redemption path the gate was meant to close was the one still
//! open: a holder could redeem ICP at oracle face value while the protocol
//! was latched ReadOnly (insolvent), deepening the bad-debt position.
//!
//! The root-cause fix moves the gate into the shared vault-module entry
//! points so every present/future redemption surface is covered by
//! construction: `vault::redeem_collateral` AND `vault::redeem_reserves`
//! reject in `Mode::ReadOnly` at the top (after the per-caller guard,
//! before any icUSD is pulled), returning the same `TemporarilyUnavailable`
//! that `main.rs::validate_mode()` produces. The endpoint-level
//! `validate_mode()?` checks are kept as defense-in-depth, including a new
//! one on `redeem_icp`. The `red_101_*` structural fences below pin all of
//! it. A behavioral end-to-end fence lives in
//! `audit_pocs_liq_005_deficit_account_pic.rs`
//! (`red_101_pic_redeem_icp_rejects_in_readonly`), which reuses that file's
//! deficit-threshold ReadOnly-latch fixture to call `redeem_icp` on a live
//! canister latched ReadOnly and assert it rejects.

use rumi_protocol_backend::state::{Mode, State};
use std::path::PathBuf;

/// Reads `src/main.rs` from the package root for source-level structural
/// assertions. `CARGO_MANIFEST_DIR` always points to the package root
/// (`src/rumi_protocol_backend/`), so resolving `src/main.rs` from there
/// is robust to test invocation from any working directory.
fn read_main_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Reads `src/vault.rs` for the vault-module structural assertions. The
/// shared redemption gate lives here (not just at the Candid entry layer),
/// so RED-101's fences read this file directly. Same `CARGO_MANIFEST_DIR`
/// anchoring as `read_main_rs`.
fn read_vault_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/vault.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Slice the body of a free `async fn` named `fn_name` from its declaration
/// up to the next free function declaration (column-0 `async fn ` or `fn `).
/// `main.rs` declares free functions only at column 0 and never nests them,
/// so this is sufficient for entry-point bodies.
fn function_body_slice<'a>(source: &'a str, fn_name: &str) -> &'a str {
    let header = format!("async fn {}(", fn_name);
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("function `{}` not found in main.rs", fn_name));
    let after_header = start + header.len();
    let next_async = source[after_header..]
        .find("\nasync fn ")
        .map(|i| after_header + i);
    let next_fn = source[after_header..]
        .find("\nfn ")
        .map(|i| after_header + i);
    let end = match (next_async, next_fn) {
        (Some(a), Some(f)) => a.min(f),
        (Some(a), None) => a,
        (None, Some(f)) => f,
        (None, None) => source.len(),
    };
    &source[start..end]
}

/// Slice the body of a `pub async fn` named `fn_name` in `vault.rs`, from its
/// declaration up to the next column-0 item. `vault.rs` declares free items at
/// column 0 as `pub async fn` / `pub fn` / `async fn` / `fn`; nested helpers
/// are indented, so anchoring the end on a newline-prefixed column-0 item
/// header isolates exactly one function body. (The `main.rs` slicer keys off
/// bare `async fn`, which would over-run a `pub async fn` boundary here.)
fn vault_function_body_slice<'a>(source: &'a str, fn_name: &str) -> &'a str {
    let header = format!("pub async fn {}(", fn_name);
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("function `{}` not found in vault.rs", fn_name));
    let after_header = start + header.len();
    let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
        .iter()
        .filter_map(|marker| source[after_header..].find(marker).map(|i| after_header + i))
        .min()
        .unwrap_or(source.len());
    &source[start..end]
}

#[test]
fn red_003_mode_readonly_is_a_distinct_state_variant() {
    // Sanity fence: the `ReadOnly` variant exists, is the rejection
    // state validate_mode checks against, and is distinct from the
    // two pass-through variants.
    let mut state = State::default();
    assert_eq!(state.mode, Mode::GeneralAvailability);
    assert_ne!(state.mode, Mode::ReadOnly);
    assert_ne!(state.mode, Mode::Recovery);

    state.mode = Mode::ReadOnly;
    assert_eq!(state.mode, Mode::ReadOnly);

    state.mode = Mode::Recovery;
    assert_eq!(state.mode, Mode::Recovery);
}

#[test]
fn red_003_main_rs_redeem_collateral_calls_validate_mode() {
    let main_rs = read_main_rs();
    let body = function_body_slice(&main_rs, "redeem_collateral");
    assert!(
        body.contains("validate_mode()?"),
        "main.rs::redeem_collateral entry point must call `validate_mode()?` \
         to gate redemption on protocol mode (audit RED-003). The fix mirrors \
         the existing pattern at borrow_from_vault, open_vault_with_deposit, \
         and open_vault_and_borrow.\n\nInspected body:\n{}",
        body,
    );
}

#[test]
fn red_003_main_rs_redeem_reserves_calls_validate_mode() {
    let main_rs = read_main_rs();
    let body = function_body_slice(&main_rs, "redeem_reserves");
    assert!(
        body.contains("validate_mode()?"),
        "main.rs::redeem_reserves entry point must call `validate_mode()?` \
         to gate reserve redemption on protocol mode (audit RED-003). \
         Reserve-redemption spillover walks the same vault cr-index as \
         redeem_collateral, so the same gate applies.\n\nInspected body:\n{}",
        body,
    );
}

// ── RED-101 (2026-06-03-0c3ceb4): close the redeem_icp ReadOnly bypass ──────
//
// These fences extend the RED-003 ones to the ICP redemption path that the
// per-endpoint Wave-9 fix missed. They cover both the new defense-in-depth
// endpoint check AND the root-cause vault-module gate.

#[test]
fn red_101_main_rs_redeem_icp_calls_validate_mode() {
    let main_rs = read_main_rs();
    let body = function_body_slice(&main_rs, "redeem_icp");
    assert!(
        body.contains("validate_mode()?"),
        "main.rs::redeem_icp endpoint must call `validate_mode()?` immediately \
         after `validate_call().await?`, matching redeem_collateral (audit \
         RED-101, regression of RED-003). redeem_icp is a distinct #[update] \
         surface that reaches the same collateral-seizing path \
         (vault::redeem_icp -> vault::redeem_collateral -> \
         record_redemption_on_vaults); without this gate a redeemer can \
         extract ICP at face value while the protocol is latched \
         ReadOnly.\n\nInspected body:\n{}",
        body,
    );
}

#[test]
fn red_101_vault_redeem_collateral_gates_readonly() {
    let vault_rs = read_vault_rs();
    let body = vault_function_body_slice(&vault_rs, "redeem_collateral");
    assert!(
        body.contains("Mode::ReadOnly"),
        "vault::redeem_collateral must reject when `state.mode == Mode::ReadOnly` \
         at the shared internal entry point (after the per-caller guard, before \
         pulling icUSD) so every redemption surface (redeem_icp, \
         redeem_collateral) is gated by construction (audit RED-101). The \
         per-endpoint validate_mode() in main.rs lives in the Candid entry layer; \
         gating here closes any entry point (present or future) that forgets \
         it.\n\nInspected body:\n{}",
        body,
    );
}

#[test]
fn red_101_vault_redeem_reserves_gates_readonly() {
    let vault_rs = read_vault_rs();
    let body = vault_function_body_slice(&vault_rs, "redeem_reserves");
    assert!(
        body.contains("Mode::ReadOnly"),
        "vault::redeem_reserves must also reject when `state.mode == Mode::ReadOnly` \
         at the vault-module boundary. Its spillover branch calls \
         record_redemption_on_vaults DIRECTLY (it does not route through \
         redeem_collateral), so the redeem_collateral gate does not cover it. \
         Keep the endpoint-level validate_mode() AND gate here so the reserve \
         path is covered by construction too (audit RED-101).\n\nInspected \
         body:\n{}",
        body,
    );
}
