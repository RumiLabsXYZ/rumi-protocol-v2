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
//! returns `None` if every known version fails.
//!
//! UPDATE (audit 2026-06-05, SAT-004): the caller `load_from_stable_memory` now
//! TRAPS when every known version fails, instead of wiping to
//! `AmmState::default()`. The old "AMM positions are reconstructable from ledger
//! balances" justification was false — pool reserves are internal-only
//! accounting and the per-LP reward state cannot be reconstructed. A silent
//! wipe of live pools is the 2026-05-18 incident class. `try_decode_state` still
//! returns `None` on undecodable input (the unit-testable contract below); only
//! the caller's reaction changed from wipe to trap.
//!
//! Also added: `AmmStateV5`, a frozen snapshot of the current shape, so the next
//! non-Option field added to `AmmState` decodes via V5 instead of silently
//! falling through to V4 (which would drop `protocol_backend_principal` and all
//! post-V4 state).
//!
//! These unit tests exercise `try_decode_state` directly:
//! 1. Valid encoded state decodes round-trip.
//! 2. Corrupt bytes return None (no trap, no panic).
//! 3. Truncated and empty bytes return None.
//! 4. A fully-populated current state round-trips with post-V4 fields intact.

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

#[test]
fn sat_004_populated_state_preserves_post_v4_fields() {
    // SAT-004: the fields the live AmmState carries beyond V4
    // (protocol_backend_principal, event logs + counters, tvl_samples) must
    // survive an upgrade round-trip. Before the AmmStateV5 snapshot, the next
    // non-Option field added to AmmState would route the decode to V4 and
    // silently reset protocol_backend_principal to None (halting reward
    // distribution) and drop all post-V4 state. This pins the round-trip for
    // the current shape so a broken/ reordered decode is caught.
    let mut state = AmmState::default();
    let backend = candid::Principal::from_text("aaaaa-aa").unwrap();
    state.protocol_backend_principal = Some(backend);
    state.next_swap_event_id = 42;
    state.next_claim_id = 7;

    let bytes = Encode!(&state).expect("encode populated AmmState");
    let decoded = try_decode_state(&bytes).expect("populated state must decode");

    assert_eq!(
        decoded.protocol_backend_principal,
        Some(backend),
        "SAT-004: protocol_backend_principal must survive decode, else reward \
         distribution halts after upgrade",
    );
    assert_eq!(
        decoded.next_swap_event_id, 42,
        "SAT-004: post-V4 counters must survive the decode",
    );
    assert_eq!(decoded.next_claim_id, 7);
}
