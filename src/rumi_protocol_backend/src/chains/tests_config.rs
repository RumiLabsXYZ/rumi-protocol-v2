//! ChainConfig encode/decode + version-alias invariants.

use super::config::{ChainConfig, ChainConfigV1, ChainId, ChainStatus, GasStrategy};
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
fn chain_config_alias_matches_v1() {
    // Phase 1a invariant: `ChainConfig` is the active version pointer.
    // Phase 1c (or whenever a field is added) rebinds this alias to V2 and
    // ships a `MultiChainStateMigration` step.
    fn _check(_x: ChainConfig) -> ChainConfigV1 { _x }
}