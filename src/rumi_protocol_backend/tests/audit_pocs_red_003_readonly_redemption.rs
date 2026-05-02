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
