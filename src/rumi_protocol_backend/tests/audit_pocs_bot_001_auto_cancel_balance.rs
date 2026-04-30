//! Wave-11 BOT-001: auto-cancel collateral-return verification — Layer 1 unit tests.
//!
//! Layer 1 covers the pure-data shape of the new `BotClaimReconciliationNeeded`
//! event variant: CBOR round-trip, type-filter classification, vault-id
//! matching, principal involvement, timestamp surfacing, and
//! `EventTypeFilter::BotClaimReconciliationNeeded` round-trip.
//!
//! The auto-cancel guard itself sits inside `check_vaults` and requires a
//! canister context (icrc1_balance_of inter-canister call). That path is
//! fenced in `audit_pocs_bot_001_auto_cancel_balance_pic.rs`.

use candid::Principal;
use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::EventTypeFilter;

const TS: u64 = 1_700_000_000_000_000_000;
const VAULT_ID: u64 = 42;
const OBSERVED: u64 = 1_000_000;
const REQUIRED: u64 = 99_990_000;

fn sample_event() -> Event {
    Event::BotClaimReconciliationNeeded {
        vault_id: VAULT_ID,
        observed_balance: OBSERVED,
        required_balance: REQUIRED,
        timestamp: TS,
    }
}

#[test]
fn bot_001_event_round_trips_cbor() {
    let event = sample_event();
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&event, &mut bytes).expect("encode");
    let decoded: Event = ciborium::de::from_reader(bytes.as_slice()).expect("decode");

    match decoded {
        Event::BotClaimReconciliationNeeded {
            vault_id,
            observed_balance,
            required_balance,
            timestamp,
        } => {
            assert_eq!(vault_id, VAULT_ID);
            assert_eq!(observed_balance, OBSERVED);
            assert_eq!(required_balance, REQUIRED);
            assert_eq!(timestamp, TS);
        }
        other => panic!("expected BotClaimReconciliationNeeded, got {:?}", other),
    }
}

#[test]
fn bot_001_event_classifies_to_dedicated_filter() {
    assert_eq!(
        sample_event().type_filter(),
        EventTypeFilter::BotClaimReconciliationNeeded
    );
}

#[test]
fn bot_001_event_is_vault_related_for_matching_vault() {
    let event = sample_event();
    assert!(event.is_vault_related(&VAULT_ID));
    assert!(!event.is_vault_related(&(VAULT_ID + 1)));
    assert!(!event.is_vault_related(&0));
}

#[test]
fn bot_001_event_does_not_involve_principal() {
    let event = sample_event();
    let principal = Principal::from_slice(&[1, 2, 3, 4]);
    assert!(!event.involves_principal(&principal));
    assert!(!event.involves_principal(&Principal::anonymous()));
}

#[test]
fn bot_001_event_surfaces_timestamp() {
    assert_eq!(sample_event().timestamp_ns(), Some(TS));
}

#[test]
fn bot_001_event_admin_label_is_none() {
    // BotClaimReconciliationNeeded has its own dedicated `EventTypeFilter`,
    // so it must NOT be classified as Admin and therefore admin_label is None.
    assert!(sample_event().admin_label().is_none());
}

#[test]
fn bot_001_event_type_filter_round_trips_cbor() {
    let filter = EventTypeFilter::BotClaimReconciliationNeeded;
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&filter, &mut bytes).expect("encode");
    let decoded: EventTypeFilter = ciborium::de::from_reader(bytes.as_slice()).expect("decode");
    assert_eq!(decoded, EventTypeFilter::BotClaimReconciliationNeeded);
}

#[test]
fn bot_001_event_size_e8s_usd_is_none() {
    // BOT-001 is informational (no monetary magnitude). The size facet must
    // treat it as "passes" by returning None.
    assert_eq!(sample_event().size_e8s_usd(1_000_000_000), None);
}

#[test]
fn bot_001_event_no_collateral_token() {
    // The event carries vault_id, not collateral_type. The collateral_token
    // helper should resolve to None unless the caller's vault_lookup map has
    // an entry. With an empty map: None.
    let lookup = std::collections::HashMap::new();
    assert_eq!(sample_event().collateral_token(&lookup), None);
}
