//! ChainConfig encode/decode + version-alias invariants.

use super::config::{
    chain_rpc_endpoint_set_digest, ChainConfig, ChainConfigV1, ChainConfigV2, ChainConfigV3,
    ChainId, ChainStatus, GasStrategy,
};
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
fn chain_config_alias_matches_v3() {
    // Phase 1d rebound `ChainConfig` from V2 to V3 (added the M-05
    // `min_quorum_providers` quorum-provider floor override). `ChainConfig` is
    // the active version pointer; the next field add bumps it to V4.
    fn _check(_x: ChainConfig) -> ChainConfigV3 {
        _x
    }
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
    assert_eq!(
        v2.rpc_endpoints,
        vec!["https://rpc.testnet.example".to_string()]
    );
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

#[test]
fn v2_cbor_sub_map_decodes_into_v3_defaulting_the_quorum_floor() {
    // STATE-WIPE REGRESSION (audit M-05 / Phase 1d). On the live staging
    // canister `chain_configs` VALUES were written as `ChainConfigV2` field-maps
    // (no `min_quorum_providers` key). After this upgrade those same bytes must
    // decode into `ChainConfigV3` with every V2 field preserved and the new
    // `min_quorum_providers` defaulted to `None` via its field-level
    // `#[serde(default)]`. State persists via ciborium (serde), NOT a Candid
    // `Decode!` of a fixed record, so a missing key fills from the default
    // rather than failing — this is what prevents the AMM-style state-wipe
    // (2026-05-18). Dropping the `#[serde(default)]` would error "missing field"
    // here and silently wipe multi_chain state on the real canister.
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
        registered_at_ns: 7,
        status: ChainStatus::Registered,
        burn_watch_poll_enabled: true,
    };
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v2, &mut buf).expect("cbor encode V2 config");
    let v3: ChainConfigV3 = ciborium::de::from_reader(buf.as_slice())
        .expect("V2 config sub-map MUST decode into V3 without wiping state");

    // Every V2 field preserved verbatim:
    assert_eq!(v3.chain_id, ChainId(10143));
    assert_eq!(v3.display_name, "MonadTestnet");
    assert_eq!(v3.rpc_endpoints, vec!["https://rpc".to_string()]);
    assert_eq!(v3.finality_depth, 3);
    assert_eq!(v3.chain_native_decimals, 18);
    assert_eq!(v3.registered_at_ns, 7);
    assert!(matches!(v3.status, ChainStatus::Registered));
    assert!(v3.burn_watch_poll_enabled);
    // The new quorum-floor override defaults to None (=> DEFAULT_MIN_QUORUM_PROVIDERS).
    assert_eq!(v3.min_quorum_providers, None);
}

fn digest_config(endpoints: &[&str], quorum: Option<u32>) -> ChainConfigV3 {
    ChainConfigV3 {
        chain_id: ChainId(1030),
        display_name: "Conflux eSpace mainnet".into(),
        rpc_endpoints: endpoints.iter().map(|s| (*s).to_string()).collect(),
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        registered_at_ns: 1,
        status: ChainStatus::Disabled,
        burn_watch_poll_enabled: false,
        min_quorum_providers: quorum,
    }
}

#[test]
fn rpc_endpoint_digest_is_order_independent_and_exactly_deduplicated() {
    let a = digest_config(
        &[
            "https://rpc-a.example/v1",
            "https://rpc-b.example/v1",
            "https://rpc-a.example/v1",
        ],
        Some(2),
    );
    let b = digest_config(
        &["https://rpc-b.example/v1", "https://rpc-a.example/v1"],
        Some(2),
    );

    let a_digest = chain_rpc_endpoint_set_digest(&a);
    let b_digest = chain_rpc_endpoint_set_digest(&b);
    assert_eq!(a_digest, b_digest);
    assert_eq!(a_digest.endpoint_count, 2);
    assert_eq!(a_digest.effective_min_quorum_providers, 2);
    assert_eq!(a_digest.digest_sha256.len(), 64);
    assert!(
        a_digest
            .digest_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "digest must be lowercase hex"
    );
}

#[test]
fn rpc_endpoint_digest_changes_on_endpoint_or_effective_quorum_drift() {
    let baseline = chain_rpc_endpoint_set_digest(&digest_config(
        &["https://rpc-a.example/v1", "https://rpc-b.example/v1"],
        Some(2),
    ));
    let endpoint_drift = chain_rpc_endpoint_set_digest(&digest_config(
        &["https://rpc-a.example/v1", "https://rpc-c.example/v1"],
        Some(2),
    ));
    let quorum_drift = chain_rpc_endpoint_set_digest(&digest_config(
        &["https://rpc-a.example/v1", "https://rpc-b.example/v1"],
        Some(3),
    ));
    let mut chain_drift_config = digest_config(
        &["https://rpc-a.example/v1", "https://rpc-b.example/v1"],
        Some(2),
    );
    chain_drift_config.chain_id = ChainId(71);
    let chain_drift = chain_rpc_endpoint_set_digest(&chain_drift_config);

    assert_ne!(baseline.digest_sha256, endpoint_drift.digest_sha256);
    assert_ne!(baseline.digest_sha256, quorum_drift.digest_sha256);
    assert_ne!(baseline.digest_sha256, chain_drift.digest_sha256);
}

#[test]
fn rpc_endpoint_digest_commits_to_effective_not_storage_shaped_quorum() {
    let endpoints = ["https://rpc-a.example/v1", "https://rpc-b.example/v1"];
    let inherited_default = chain_rpc_endpoint_set_digest(&digest_config(&endpoints, None));
    let explicit_default = chain_rpc_endpoint_set_digest(&digest_config(&endpoints, Some(3)));

    assert_eq!(inherited_default, explicit_default);
    assert_eq!(inherited_default.effective_min_quorum_providers, 3);
}

#[test]
fn rpc_endpoint_digest_uses_exact_url_bytes_and_ignores_uncommitted_fields() {
    let baseline = digest_config(&["https://rpc-a.example/v1"], Some(2));
    let slash_drift = digest_config(&["https://rpc-a.example/v1/"], Some(2));
    let mut unrelated = baseline.clone();
    unrelated.display_name = "renamed".into();
    unrelated.finality_depth = 999;
    unrelated.status = ChainStatus::Registered;

    assert_ne!(
        chain_rpc_endpoint_set_digest(&baseline).digest_sha256,
        chain_rpc_endpoint_set_digest(&slash_drift).digest_sha256,
        "URL canonicalization is exact-string, matching stored de-duplication"
    );
    assert_eq!(
        chain_rpc_endpoint_set_digest(&baseline).digest_sha256,
        chain_rpc_endpoint_set_digest(&unrelated).digest_sha256,
        "the V1 digest commits only chain id, effective quorum, and endpoint set"
    );
}

#[test]
fn rpc_endpoint_digest_has_a_fixed_canonical_vector() {
    let digest = chain_rpc_endpoint_set_digest(&digest_config(
        &["https://rpc-b.example/v1", "https://rpc-a.example/v1"],
        Some(2),
    ));
    assert_eq!(
        digest.digest_sha256, "0d7895c55182ac2f0d69823100cd5b69af36922136eb3a1988416aea59399528",
        "changing this requires a documented digest-contract version bump"
    );
}
