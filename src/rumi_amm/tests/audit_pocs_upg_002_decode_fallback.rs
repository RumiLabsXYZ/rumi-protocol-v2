//! UPG-002 regression fence: rumi_amm post_upgrade must not trap when the
//! snapshot blob fails to decode against any known schema version.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/raw-pass-results/upgrade-safety.json (UPG-002).
//!
//! Before the Wave-6 fix, `src/rumi_amm/src/state.rs::load_from_stable_memory`
//! tried V-current..V1 in sequence, but the final V1 fallback used
//! `Decode!(...).expect(...)` which traps on failure. A trap in post_upgrade
//! bricks the canister.
//!
//! The fix extracts the version-walking logic into `try_decode_state`, which
//! returns `None` if every known version fails. The caller (`load_from_stable_memory`)
//! then logs a CRITICAL diagnostic and falls back to `AmmState::default()`. AMM
//! positions are reconstructable from underlying ledger balances, so empty
//! fallback is a defensible last resort here (versus stability_pool, where
//! empty fallback zeroes depositor positions and is therefore the absolute
//! last resort).
//!
//! These unit tests exercise `try_decode_state` directly:
//! 1. Valid encoded state decodes round-trip.
//! 2. Corrupt bytes return None (no trap, no panic).
//! 3. Truncated and empty bytes return None.

use candid::Encode;
use rumi_amm::state::{try_decode_state, AmmState};

#[test]
fn upg_002_valid_state_decodes_round_trip() {
    let original = AmmState::default();
    let bytes = Encode!(&original).expect("encode of default AmmState should succeed");

    let decoded = try_decode_state(&bytes);
    assert!(
        decoded.is_some(),
        "UPG-002: valid encoded AmmState must decode via try_decode_state",
    );
}

#[test]
fn upg_002_corrupt_bytes_return_none_no_trap() {
    let corrupt = vec![0xffu8; 1024];

    let decoded = try_decode_state(&corrupt);
    assert!(
        decoded.is_none(),
        "UPG-002: corrupt bytes must return None (caller falls back to empty), not panic or trap",
    );
}

#[test]
fn upg_002_truncated_bytes_return_none_no_trap() {
    let original = AmmState::default();
    let bytes = Encode!(&original).expect("encode should succeed");

    let truncated = &bytes[..bytes.len() / 2];

    let decoded = try_decode_state(truncated);
    assert!(
        decoded.is_none(),
        "UPG-002: truncated bytes must return None, not panic or trap",
    );
}

#[test]
fn upg_002_empty_bytes_return_none_no_trap() {
    let decoded = try_decode_state(&[]);
    assert!(
        decoded.is_none(),
        "UPG-002: empty bytes must return None, not panic or trap",
    );
}
