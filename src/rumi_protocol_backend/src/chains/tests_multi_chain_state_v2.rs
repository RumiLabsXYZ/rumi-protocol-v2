use super::multi_chain_state::{MultiChainState, MultiChainStateV1, MultiChainStateV2};
use super::config::ChainId;
use super::supply::migrate_multi_chain_state;

#[test]
fn v1_cbor_snapshot_decodes_into_v2_without_wiping_state() {
    // Regression for the Task-3 state-wipe bug: a populated V1-shaped CBOR
    // snapshot (the shape Phase 1a wrote to stable memory) MUST decode into
    // MultiChainStateV2 with the four V1 fields preserved and the new-in-V2
    // fields defaulted to empty. This is the exact ciborium decode path
    // load_state_from_stable() runs on upgrade. WITHOUT field-level
    // #[serde(default)] on the new V2 fields this decode fails with
    // "missing field `chain_vaults`", which on a real canister silently
    // wipes multi_chain state via the event-replay fallback.
    use super::config::{ChainConfigV1, ChainStatus, GasStrategy};
    use super::settlement_queue::SettlementQueueV1;

    let mut v1 = MultiChainStateV1::default();
    v1.chain_configs.insert(ChainId(10143), ChainConfigV1 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 500 },
        chain_native_decimals: 18,
        registered_at_ns: 123,
        status: ChainStatus::Registered,
    });
    v1.chain_supplies.insert(ChainId(10143), 777);
    v1.settlement_queues.insert(ChainId(10143), SettlementQueueV1::default());
    v1.invariant_halted = true;

    // Encode as V1 (the bytes Phase 1a wrote), decode as V2 (the new shape).
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v1, &mut buf).expect("cbor encode V1");
    let v2: MultiChainStateV2 =
        ciborium::de::from_reader(buf.as_slice()).expect("V1 snapshot MUST decode into V2");

    // V1 fields preserved:
    assert_eq!(v2.chain_supplies.get(&ChainId(10143)), Some(&777u128));
    assert!(v2.chain_configs.contains_key(&ChainId(10143)));
    assert!(v2.settlement_queues.contains_key(&ChainId(10143)));
    assert!(v2.invariant_halted);
    // New fields defaulted to empty:
    assert!(v2.chain_vaults.is_empty());
    assert!(v2.chain_contracts.is_empty());
    assert!(v2.manual_prices.is_empty());
    assert!(v2.last_observed_block.is_empty());
    assert!(v2.hot_wallet_balance_e18.is_empty());
    assert!(v2.reorg_halted.is_empty());
    assert!(v2.reorg_suspect_streak.is_empty());
    // C-1: the burn-watch idempotency set must default to empty from a V1 blob
    // that has no such key (its absence must NOT trip the state-wipe fallback).
    assert!(v2.processed_burn_keys.is_empty());
}

#[test]
fn v2_cbor_round_trip_preserves_processed_burn_keys() {
    // C-1 state-wipe defense: a populated V2 snapshot (with processed_burn_keys)
    // must survive a ciborium encode→decode round-trip with the new field intact.
    // This is the exact path load_state_from_stable() runs on a V2→V2 upgrade.
    use std::collections::BTreeSet;

    let mut v2 = MultiChainStateV2::default();
    v2.chain_supplies.insert(ChainId(10143), 6_000_000_000);
    let mut keys = BTreeSet::new();
    keys.insert("0xgoodburn:1:4000000000".to_string());
    keys.insert("0xpoison:1:100000000000".to_string());
    v2.processed_burn_keys.insert(1_000_300, keys);
    v2.processed_burn_keys
        .insert(1_000_301, BTreeSet::from(["0xother:2:50".to_string()]));

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v2, &mut buf).expect("cbor encode V2");
    let decoded: MultiChainStateV2 =
        ciborium::de::from_reader(buf.as_slice()).expect("V2 snapshot round-trips");

    assert_eq!(decoded.chain_supplies.get(&ChainId(10143)), Some(&6_000_000_000u128));
    assert_eq!(decoded.processed_burn_keys.len(), 2);
    let block = decoded.processed_burn_keys.get(&1_000_300).expect("block 1_000_300 present");
    assert!(block.contains("0xgoodburn:1:4000000000"));
    assert!(block.contains("0xpoison:1:100000000000"));
    assert_eq!(block.len(), 2);
    assert!(decoded
        .processed_burn_keys
        .get(&1_000_301)
        .expect("block 1_000_301 present")
        .contains("0xother:2:50"));
}

#[test]
fn v2_default_is_empty() {
    let s = MultiChainStateV2::default();
    assert!(s.chain_configs.is_empty());
    assert!(s.chain_supplies.is_empty());
    assert!(s.chain_vaults.is_empty());
    assert!(s.chain_contracts.is_empty());
    assert!(s.manual_prices.is_empty());
    assert!(s.last_observed_block.is_empty());
    assert!(s.hot_wallet_balance_e18.is_empty());
    assert!(s.reorg_halted.is_empty());
    assert!(s.reorg_suspect_streak.is_empty());
    assert!(s.processed_burn_keys.is_empty());
    assert_eq!(s.total_supply_all_chains_e8s(), 0u128);
}

#[test]
fn migration_preserves_v1_fields_and_defaults_new_ones() {
    let mut v1 = MultiChainStateV1::default();
    v1.chain_supplies.insert(ChainId(10143), 12345);
    v1.invariant_halted = true;
    let v2 = migrate_multi_chain_state(v1);
    assert_eq!(v2.chain_supplies.get(&ChainId(10143)), Some(&12345u128));
    assert!(v2.invariant_halted);
    assert!(v2.chain_vaults.is_empty());
    assert!(v2.chain_contracts.is_empty());
    assert!(v2.processed_burn_keys.is_empty());
}

#[test]
fn active_alias_points_at_v2() {
    fn _check(x: MultiChainState) -> MultiChainStateV2 { x }
}

#[test]
fn chain_vault_debt_total_sums_only_chain_vaults() {
    use super::monad::chain_vault::{ChainVaultV1, ChainVaultStatus};

    let mut s = MultiChainStateV2::default();
    assert_eq!(s.total_chain_vault_debt_e8s(), 0);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xa".into(),
        collateral_amount_e18: 0,
        debt_e8s: 7_000_000_000,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
    });
    s.chain_vaults.insert(2, ChainVaultV1 {
        vault_id: 2,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xb".into(),
        collateral_amount_e18: 0,
        debt_e8s: 3_000_000_000,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
    });
    assert_eq!(s.total_chain_vault_debt_e8s(), 10_000_000_000);
}
