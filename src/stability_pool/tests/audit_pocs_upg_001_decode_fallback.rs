//! UPG-001 regression fence: stability pool post_upgrade must not trap when
//! the snapshot blob fails to decode.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/raw-pass-results/upgrade-safety.json (UPG-001).
//!
//! Before the Wave-6 fix, `src/stability_pool/src/state.rs::load_from_stable_memory`
//! used `Decode!(&bytes, StabilityPoolState).expect(...)` which traps the canister
//! on any decode failure. A trap in post_upgrade bricks the canister: it cannot be
//! upgraded, queried, or recovered without a hotfix wasm whose decoder is
//! compatible with the persisted bytes. Recovery via reinstall is destructive
//! (every depositor position is erased).
//!
//! The fix introduces `try_decode_state`, a multi-version fallback chain. Today
//! the chain has a single entry (the current schema, which is structurally tolerant
//! of additive changes via `#[serde(default)]` on every new field). When a
//! non-additive schema change ships, the previous shape is added as a fallback
//! variant; if every known version fails, the loader logs a CRITICAL diagnostic
//! and falls back to empty state rather than trapping. Empty fallback is the last
//! resort because it zeroes depositor positions.
//!
//! These unit tests exercise `try_decode_state` directly:
//! 1. Valid encoded state decodes round-trip.
//! 2. Corrupt bytes return None (no trap, no panic).
//! 3. Empty bytes return None.
//!
//! The end-to-end "post_upgrade does not trap on corruption" path is exercised
//! indirectly: `load_from_stable_memory` calls `try_decode_state` and falls back
//! to `StabilityPoolState::default()` when None is returned. As long as
//! `try_decode_state` is total (never panics), `load_from_stable_memory` is total.

use candid::Encode;
use stability_pool::state::{try_decode_state, StabilityPoolState};

#[test]
fn upg_001_valid_state_decodes_round_trip() {
    let original = StabilityPoolState::default();
    let bytes = Encode!(&original).expect("encode of default state should succeed");

    let decoded = try_decode_state(&bytes);
    assert!(
        decoded.is_some(),
        "UPG-001: valid encoded StabilityPoolState must decode via try_decode_state",
    );
}

#[test]
fn upg_001_corrupt_bytes_return_none_no_trap() {
    let corrupt = vec![0xffu8; 1024];

    let decoded = try_decode_state(&corrupt);
    assert!(
        decoded.is_none(),
        "UPG-001: corrupt bytes must return None (caller falls back to empty), not panic or trap",
    );
}

#[test]
fn upg_001_truncated_bytes_return_none_no_trap() {
    let original = StabilityPoolState::default();
    let bytes = Encode!(&original).expect("encode should succeed");

    let truncated = &bytes[..bytes.len() / 2];

    let decoded = try_decode_state(truncated);
    assert!(
        decoded.is_none(),
        "UPG-001: truncated bytes must return None, not panic or trap",
    );
}

#[test]
fn upg_001_empty_bytes_return_none_no_trap() {
    let decoded = try_decode_state(&[]);
    assert!(
        decoded.is_none(),
        "UPG-001: empty bytes must return None, not panic or trap",
    );
}
