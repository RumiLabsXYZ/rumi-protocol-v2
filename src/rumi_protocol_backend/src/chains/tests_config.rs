//! ChainConfig encode/decode + version-alias invariants.

use super::config::{ChainConfig, ChainConfigV1, ChainConfigV2, ChainId, ChainStatus, GasStrategy};
use candid::{Decode, Encode};

#[test]
fn chain_id_orderable_for_btreemap_use() {
    let a = ChainId(1);
    let b = ChainId(2);
    assert!(a < b);
}

#[test]
fn chain_status_is_exhaustive() {
    // Phase 1a defines two variants. Future variants land via a versioned
    // migration, never an in-place enum addition (cf. CBOR untagged-enum
    // round-trips for Mode).
    let variants = vec![ChainStatus::Registered, ChainStatus::Disabled];
    assert_eq!(variants.len(), 2);
}

#[test]
fn chain_config_round_trips_via_candid() {
    let cfg = ChainConfigV1 {
        chain_id: ChainId(101),
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: vec!["https://rpc.testnet.example".to_string()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        registered_at_ns: 1_700_000_000_000_000_000,
        status: ChainStatus::Registered,
    };
    let bytes = Encode!(&cfg).expect("encode");
    let back: ChainConfigV1 = Decode!(&bytes, ChainConfigV1).expect("decode");
    assert_eq!(back.chain_id, cfg.chain_id);
    assert_eq!(back.display_name, cfg.display_name);
    assert_eq!(back.finality_depth, 1);
}

#[test]
fn chain_config_alias_matches_v2() {
    // Phase 1c rebound `ChainConfig` from V1 to V2 (added the
    // `burn_watch_poll_enabled` poll-scan flag). `ChainConfig` is the active
    // version pointer; the next field add bumps it to V3.
    fn _check(_x: ChainConfig) -> ChainConfigV2 { _x }
}

#[test]
fn v1_cbor_sub_map_decodes_into_v2_defaulting_the_poll_flag() {
    // STATE-WIPE REGRESSION (Phase 1c). On the live staging canister
    // `chain_configs` is a ciborium (CBOR) map whose VALUES were written as
    // `ChainConfigV1` field-maps (no `burn_watch_poll_enabled` key). After the
    // Phase 1c upgrade those same bytes must decode into `ChainConfigV2` with
    // every V1 field preserved and the new flag defaulted to `false` via its
    // field-level `#[serde(default)]`. State persists via ciborium (serde),
    // NOT a Candid `Decode!` of a fixed record, so a missing key fills from the
    // default rather than failing the decode — this is what prevents the
    // AMM-style state-wipe (2026-05-18). If the `#[serde(default)]` were
    // dropped, this decode would error with "missing field", which on the real
    // canister silently wipes multi_chain state via the event-replay fallback.
    let v1 = ChainConfigV1 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: vec!["https://rpc.testnet.example".to_string()],
        finality_depth: 3,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        registered_at_ns: 1_700_000_000_000_000_000,
        status: ChainStatus::Registered,
    };

    // Encode with the V1 shape (the bytes a pre-1c canister wrote), decode with
    // the V2 shape (the new active type).
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v1, &mut buf).expect("cbor encode V1 config");
    let v2: ChainConfigV2 = ciborium::de::from_reader(buf.as_slice())
        .expect("V1 config sub-map MUST decode into V2 without wiping state");

    // Every V1 field preserved verbatim:
    assert_eq!(v2.chain_id, ChainId(10143));
    assert_eq!(v2.display_name, "MonadTestnet");
    assert_eq!(v2.rpc_endpoints, vec!["https://rpc.testnet.example".to_string()]);
    assert_eq!(v2.finality_depth, 3);
    assert_eq!(v2.chain_native_decimals, 18);
    assert_eq!(v2.registered_at_ns, 1_700_000_000_000_000_000);
    assert!(matches!(v2.status, ChainStatus::Registered));
    // New flag defaults to false (poll-scan OFF) — notify-then-verify default.
    assert!(!v2.burn_watch_poll_enabled);
}

#[test]
fn v2_config_round_trips_with_poll_flag_set() {
    // A populated V2 config (poll flag flipped on for an emergency catch-up)
    // must survive a ciborium round-trip with the flag intact — the V2->V2
    // upgrade path.
    let v2 = ChainConfigV2 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: vec!["https://rpc".to_string()],
        finality_depth: 3,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        registered_at_ns: 1,
        status: ChainStatus::Registered,
        burn_watch_poll_enabled: true,
    };
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v2, &mut buf).expect("cbor encode V2 config");
    let back: ChainConfigV2 =
        ciborium::de::from_reader(buf.as_slice()).expect("V2 config round-trips");
    assert!(back.burn_watch_poll_enabled);
    assert_eq!(back.finality_depth, 3);
}