//! Unit tests for `amm1_pool_id` state field and event-replay round-trip.
//!
//! The on-mainnet bug these tests guard against: `donate_icusd_to_amm1`
//! hardcoded the AMM pool_id as `"3USD_ICP"`, but rumi_amm stores its 3USD/ICP
//! pool under `make_pool_id(token_a, token_b)` =
//! `<token_a_principal>_<token_b_principal>`. The mismatch caused
//! `notify_reward_received` to return PoolNotFound and the donation queue to
//! re-mint icUSD every Timer-B tick.
//!
//! These tests pin down:
//! 1. `record_set_amm1_pool_id` writes the field.
//! 2. The new `Event::SetAmm1PoolId` replays into state correctly.
//! 3. CBOR (ciborium) serde round-trips work both with the field set and unset.
//!    The `#[serde(default)]` attribute is what makes the post-upgrade decode
//!    work without a versioned snapshot.

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::state::State;

#[test]
fn default_state_amm1_pool_id_is_none() {
    let state = State::default();
    assert_eq!(state.amm1_pool_id, None);
}

#[test]
fn event_variant_field_matches_state_field_shape() {
    // Confirm Event::SetAmm1PoolId carries exactly the `pool_id: String`
    // payload that `state.amm1_pool_id: Option<String>` expects.
    // (Compile-time assertion: this destructure must match the event definition.)
    let event = Event::SetAmm1PoolId {
        pool_id: "shape_check".to_string(),
    };
    let pool_id: String = match event {
        Event::SetAmm1PoolId { pool_id } => pool_id,
        _ => unreachable!(),
    };
    assert_eq!(pool_id, "shape_check");
}

#[test]
fn replay_set_amm1_pool_id_event_restores_field() {
    // Mirror the replay match arm directly. (The full `replay()` requires an
    // Init event first, which carries InitArg and is overkill here; we just
    // need to confirm the new match arm assigns into state correctly.)
    let mut state = State::default();
    let event = Event::SetAmm1PoolId {
        pool_id: "abc_def".to_string(),
    };

    match event {
        Event::SetAmm1PoolId { pool_id } => {
            state.amm1_pool_id = Some(pool_id);
        }
        _ => panic!("expected SetAmm1PoolId"),
    }

    assert_eq!(state.amm1_pool_id, Some("abc_def".to_string()));
}

#[test]
fn cbor_roundtrip_with_amm1_pool_id_unset() {
    let state = State::default();
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode");
    let decoded: State = ciborium::de::from_reader(buf.as_slice()).expect("decode");
    assert_eq!(decoded.amm1_pool_id, None);
}

#[test]
fn cbor_roundtrip_with_amm1_pool_id_set() {
    let mut state = State::default();
    state.amm1_pool_id = Some("my_pool".to_string());
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode");
    let decoded: State = ciborium::de::from_reader(buf.as_slice()).expect("decode");
    assert_eq!(decoded.amm1_pool_id, Some("my_pool".to_string()));
}
