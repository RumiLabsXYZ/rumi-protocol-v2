use super::multi_chain_state::{
    MultiChainState, MultiChainStateV1, MultiChainStateV2, MultiChainStateV3, MultiChainStateV4,
    MultiChainStateV5, MultiChainStateV6,
};
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
fn active_alias_points_at_v6() {
    fn _check(x: MultiChainState) -> MultiChainStateV6 { x }
}

#[test]
fn v2_cbor_snapshot_decodes_into_v3_without_wiping_state() {
    // STATE-WIPE REGRESSION (Phase 1c). The live staging canister persists a
    // `MultiChainStateV2` whose `chain_configs` values are `ChainConfigV1`
    // field-maps. The Phase 1c upgrade rebinds `MultiChainState` to V3 (whose
    // `chain_configs` value type is `ChainConfigV2`). A populated V2 CBOR
    // snapshot MUST decode into V3 with:
    //   - the eight outer fields carried across by name, and
    //   - each nested config decoding from `ChainConfigV1` into `ChainConfigV2`
    //     with `burn_watch_poll_enabled` supplied by its `#[serde(default)]`.
    // This is the exact ciborium decode path `load_state_from_stable()` runs on
    // upgrade. Without the nested `#[serde(default)]` the decode would fail with
    // "missing field `burn_watch_poll_enabled`", silently wiping multi_chain
    // state on the real canister. (Vault 1 with real debt/supply lives here.)
    use super::config::{ChainConfigV1, ChainStatus, GasStrategy};
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use super::settlement_queue::SettlementQueueV1;
    use std::collections::BTreeSet;

    let mut v2 = MultiChainStateV2::default();
    v2.chain_configs.insert(ChainId(10143), ChainConfigV1 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 3,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 500 },
        chain_native_decimals: 18,
        registered_at_ns: 123,
        status: ChainStatus::Registered,
    });
    v2.chain_supplies.insert(ChainId(10143), 50_000_000);
    v2.settlement_queues.insert(ChainId(10143), SettlementQueueV1::default());
    v2.invariant_halted = false;
    v2.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(),
        collateral_amount_native: 1_000_000_000_000_000_000,
        debt_e8s: 50_000_000,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 99,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    v2.chain_contracts.insert(ChainId(10143), "0xicusd".into());
    v2.last_observed_block.insert(ChainId(10143), 35_136_248);
    v2.processed_burn_keys.insert(35_136_200, BTreeSet::from(["0xtx:0".to_string()]));

    // Encode as V2 (the bytes the live canister wrote), decode as V3 (new shape).
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v2, &mut buf).expect("cbor encode V2");
    let v3: MultiChainStateV3 =
        ciborium::de::from_reader(buf.as_slice()).expect("V2 snapshot MUST decode into V3");

    // Outer state preserved:
    assert_eq!(v3.chain_supplies.get(&ChainId(10143)), Some(&50_000_000u128));
    assert_eq!(v3.chain_vaults.len(), 1);
    assert_eq!(v3.chain_vaults[&1].debt_e8s, 50_000_000);
    assert_eq!(v3.chain_contracts.get(&ChainId(10143)), Some(&"0xicusd".to_string()));
    assert_eq!(v3.last_observed_block.get(&ChainId(10143)), Some(&35_136_248u64));
    assert!(v3.processed_burn_keys.get(&35_136_200).unwrap().contains("0xtx:0"));
    // Nested config migrated V1 -> V2: V1 fields preserved, new flag defaulted off.
    let cfg = v3.chain_configs.get(&ChainId(10143)).expect("config preserved");
    assert_eq!(cfg.finality_depth, 3);
    assert_eq!(cfg.display_name, "MonadTestnet");
    assert!(matches!(cfg.status, ChainStatus::Registered));
    assert!(!cfg.burn_watch_poll_enabled, "poll-scan defaults OFF after upgrade");
    // Total debt still reconciles (no state wipe).
    assert_eq!(v3.total_chain_vault_debt_e8s(), 50_000_000);
    assert_eq!(v3.total_supply_all_chains_e8s(), 50_000_000);
}

#[test]
fn v3_cbor_snapshot_decodes_into_v4_without_wiping_state() {
    // STATE-WIPE REGRESSION (audit M-05 / Phase 1d). The live staging canister
    // persists a `MultiChainStateV3` whose `chain_configs` values are
    // `ChainConfigV2` field-maps. This upgrade rebinds `MultiChainState` to V4
    // (whose `chain_configs` value type is `ChainConfigV3`). A populated V3 CBOR
    // snapshot MUST decode into V4 with:
    //   - the ten outer fields carried across by name, and
    //   - each nested config decoding from `ChainConfigV2` into `ChainConfigV3`
    //     with `min_quorum_providers` supplied by its `#[serde(default)]`.
    // This is the exact ciborium decode path `load_state_from_stable()` runs on
    // upgrade. Without the nested `#[serde(default)]` the decode would fail with
    // "missing field `min_quorum_providers`", silently wiping multi_chain state
    // on the real canister. (Vault 1 with real debt/supply lives here.)
    use super::config::{ChainConfigV2, ChainStatus, GasStrategy};
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use super::settlement_queue::SettlementQueueV1;
    use std::collections::BTreeSet;

    let mut v3 = MultiChainStateV3::default();
    v3.chain_configs.insert(ChainId(10143), ChainConfigV2 {
        chain_id: ChainId(10143),
        display_name: "MonadTestnet".into(),
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 3,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 500 },
        chain_native_decimals: 18,
        registered_at_ns: 123,
        status: ChainStatus::Registered,
        burn_watch_poll_enabled: true,
    });
    v3.chain_supplies.insert(ChainId(10143), 50_000_000);
    v3.settlement_queues.insert(ChainId(10143), SettlementQueueV1::default());
    v3.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(),
        collateral_amount_native: 1_000_000_000_000_000_000,
        debt_e8s: 50_000_000,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 99,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    v3.chain_contracts.insert(ChainId(10143), "0xicusd".into());
    v3.last_observed_block.insert(ChainId(10143), 35_136_248);
    v3.processed_burn_keys.insert(35_136_200, BTreeSet::from(["0xtx:0".to_string()]));

    // Encode as V3 (the bytes the live canister wrote), decode as V4 (new shape).
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v3, &mut buf).expect("cbor encode V3");
    let v4: MultiChainStateV4 =
        ciborium::de::from_reader(buf.as_slice()).expect("V3 snapshot MUST decode into V4");

    // Outer state preserved:
    assert_eq!(v4.chain_supplies.get(&ChainId(10143)), Some(&50_000_000u128));
    assert_eq!(v4.chain_vaults.len(), 1);
    assert_eq!(v4.chain_vaults[&1].debt_e8s, 50_000_000);
    assert_eq!(v4.chain_contracts.get(&ChainId(10143)), Some(&"0xicusd".to_string()));
    assert_eq!(v4.last_observed_block.get(&ChainId(10143)), Some(&35_136_248u64));
    assert!(v4.processed_burn_keys.get(&35_136_200).unwrap().contains("0xtx:0"));
    // Nested config migrated V2 -> V3: V2 fields preserved, new override defaulted None.
    let cfg = v4.chain_configs.get(&ChainId(10143)).expect("config preserved");
    assert_eq!(cfg.finality_depth, 3);
    assert_eq!(cfg.display_name, "MonadTestnet");
    assert!(matches!(cfg.status, ChainStatus::Registered));
    assert!(cfg.burn_watch_poll_enabled, "V2 poll flag carried across");
    assert_eq!(cfg.min_quorum_providers, None, "new quorum floor defaults to None");
    // Total debt still reconciles (no state wipe).
    assert_eq!(v4.total_chain_vault_debt_e8s(), 50_000_000);
    assert_eq!(v4.total_supply_all_chains_e8s(), 50_000_000);
}

#[test]
fn v5_cbor_snapshot_decodes_into_v6_without_wiping_state() {
    // STATE-WIPE REGRESSION (chains-liquidation Increment 1). The live staging
    // canister (kvg63) persists a `MultiChainStateV5`. This increment rebinds the
    // `MultiChainState` alias to V6, which adds four NEW maps/sets
    // (reserve_backing_e8s, reserve_usdc_native, pending_chain_burn_e8s,
    // sp_attempted_chain_vaults). A populated V5 CBOR snapshot MUST decode into V6
    // with:
    //   - every carried field preserved (the four originals + every V2+-added map
    //     incl. the V4 nonces and the V5 price timestamps), and
    //   - each new V6 field supplied by its `#[serde(default)]` => empty.
    // This is the exact ciborium decode path `load_state_from_stable()` runs on
    // upgrade. WITHOUT field-level `#[serde(default)]` on the new V6 fields this
    // decode fails with "missing field `reserve_backing_e8s`", which on the real
    // canister silently WIPES multi_chain state via the event-replay fallback
    // (the 2026-05-18 AMM incident class). Real Vault state lives on kvg63, so a
    // wipe here would destroy live vaults/supply/contracts.
    use super::config::{ChainConfigV3, ChainStatus, GasStrategy};
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use super::settlement_queue::SettlementQueueV1;
    use std::collections::BTreeSet;

    const CFX: ChainId = ChainId(1030);

    let mut v5 = MultiChainStateV5::default();
    v5.chain_configs.insert(CFX, ChainConfigV3 {
        chain_id: CFX,
        display_name: "ConfluxESpace".into(),
        rpc_endpoints: vec!["https://evm.confluxrpc.com".into()],
        finality_depth: 5,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 1, max_fee_gwei_ceiling: 200 },
        chain_native_decimals: 18,
        registered_at_ns: 123,
        status: ChainStatus::Registered,
        burn_watch_poll_enabled: true,
        min_quorum_providers: Some(3),
    });
    v5.chain_supplies.insert(CFX, 100 * 100_000_000); // 100 icUSD in e8s
    v5.settlement_queues.insert(CFX, SettlementQueueV1::default());
    v5.invariant_halted = false;
    v5.chain_vaults.insert(3, ChainVaultV1 {
        vault_id: 3,
        owner: candid::Principal::anonymous(),
        collateral_chain: CFX,
        custody_address: "0xcustody".into(),
        collateral_amount_native: 1_000_000_000_000_000_000,
        debt_e8s: 100 * 100_000_000,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 99,
        owner_evm: Some("0xowner".into()),
        last_interest_accrual_ns: 1_700_000_000_000_000_000,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    v5.chain_contracts.insert(CFX, "0xicusd".into());
    v5.last_observed_block.insert(CFX, 35_136_248);
    v5.processed_burn_keys.insert(35_136_200, BTreeSet::from(["0xtx:0".to_string()]));
    v5.evm_owner_nonces.insert(candid::Principal::anonymous(), 7);
    // Stamp a manual price + its freshness timestamp (the V5-added side-channel).
    v5.set_manual_price(CFX, "CFX".to_string(), 4_800_000, 1_700_000_000_000_000_001);

    // Encode as V5 (the bytes the live canister wrote), decode as V6 (new shape).
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v5, &mut buf).expect("cbor encode V5");
    let v6: MultiChainStateV6 =
        ciborium::de::from_reader(buf.as_slice()).expect("V5 snapshot MUST decode into V6");

    // --- Every carried field survived (NOT wiped) ---
    assert_eq!(v6.chain_supplies, v5.chain_supplies, "chain_supplies carried");
    assert_eq!(v6.chain_supplies.get(&CFX), Some(&(100 * 100_000_000u128)));
    assert_eq!(v6.chain_vaults.len(), 1);
    assert_eq!(v6.chain_vaults[&3].debt_e8s, 100 * 100_000_000);
    assert_eq!(v6.chain_vaults[&3].owner_evm, Some("0xowner".to_string()));
    assert_eq!(v6.chain_vaults[&3].last_interest_accrual_ns, 1_700_000_000_000_000_000);
    assert_eq!(v6.chain_contracts.get(&CFX), Some(&"0xicusd".to_string()));
    assert_eq!(v6.last_observed_block.get(&CFX), Some(&35_136_248u64));
    assert!(v6.processed_burn_keys.get(&35_136_200).unwrap().contains("0xtx:0"));
    assert_eq!(v6.expected_evm_nonce(&candid::Principal::anonymous()), 7, "nonce carried");
    assert_eq!(
        v6.get_manual_price(CFX, "CFX"),
        Some((4_800_000, 1_700_000_000_000_000_001)),
        "manual price + freshness timestamp carried"
    );
    let cfg = v6.chain_configs.get(&CFX).expect("config preserved");
    assert_eq!(cfg.finality_depth, 5);
    assert!(cfg.burn_watch_poll_enabled);
    assert_eq!(cfg.min_quorum_providers, Some(3));

    // --- The four NEW V6 fields defaulted EMPTY (state-wipe defense, not a wipe) ---
    assert!(v6.reserve_backing_e8s.is_empty(), "new field defaulted empty, not wiped");
    assert!(v6.reserve_usdc_native.is_empty());
    assert!(v6.pending_chain_burn_e8s.is_empty());
    assert!(v6.sp_attempted_chain_vaults.is_empty());

    // Total debt still reconciles with supply (the invariant is intact post-decode;
    // with all-zero reserve/pending the generalized RHS reduces to bare debt).
    assert_eq!(v6.total_chain_vault_debt_e8s(), 100 * 100_000_000);
    assert_eq!(v6.total_supply_all_chains_e8s(), 100 * 100_000_000);
}

#[test]
fn v6_cbor_round_trip_preserves_new_liquidation_fields() {
    // STATE-WIPE defense (the forward direction): once V6 is live, a V6 snapshot
    // carrying the new liquidation-engine fields (reserve maps, pending-burn,
    // sp-attempted set, the liquidation config) MUST survive an encode->decode
    // round-trip with every value intact. This is the path a V6->V6 upgrade runs.
    use super::liquidation_config::{ChainLiquidationConfigV1, DexKind};
    use std::collections::BTreeSet;

    const CFX: ChainId = ChainId(1030);

    let mut v6 = MultiChainStateV6::default();
    v6.chain_supplies.insert(CFX, 100);
    v6.reserve_backing_e8s.insert(CFX, 40);
    v6.reserve_usdc_native.insert(CFX, 45_000_000_000_000_000_000);
    v6.pending_chain_burn_e8s.insert(CFX, 15);
    v6.sp_attempted_chain_vaults.insert(7);
    v6.sp_attempted_chain_vaults.insert(9);
    v6.chain_liquidation_configs.insert(CFX, ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: "0x14b2D3bC65e74DAE1030EAFd8ac30c533c976A9b".into(),
        factory: "0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5".into(),
        pair: "0xpair".into(),
        collateral_token: "0x14b2D3bC65e74DAE1030EAFd8ac30c533c976A9b".into(),
        settle_stable_token: "0x6963EfED0aB40F6C3d7BdA44A05dcf1437C44372".into(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: true,
        max_swap_value_e8s: 2_000 * 100_000_000,
        max_price_age_ns: 1_800_000_000_000,
        max_dex_oracle_divergence_bps: 500,
        fee_bps: 25,
        settle_stable_decimals: 18,
        deadline_secs: 180,
    });

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v6, &mut buf).expect("cbor encode V6");
    let decoded: MultiChainStateV6 =
        ciborium::de::from_reader(buf.as_slice()).expect("V6 snapshot round-trips");

    assert_eq!(decoded.reserve_backing_e8s.get(&CFX), Some(&40));
    assert_eq!(decoded.reserve_usdc_native.get(&CFX), Some(&45_000_000_000_000_000_000));
    assert_eq!(decoded.pending_chain_burn_e8s.get(&CFX), Some(&15));
    assert_eq!(decoded.sp_attempted_chain_vaults, BTreeSet::from([7, 9]));
    let cfg = decoded.chain_liquidation_configs.get(&CFX).expect("config survived");
    assert_eq!(cfg.dex, DexKind::UniswapV2);
    assert_eq!(cfg.slippage_cap_bps, 250);
    assert_eq!(cfg.restore_target_cr_e4, 15_500);
    assert!(cfg.enabled);
    assert_eq!(cfg.settle_stable_token, "0x6963EfED0aB40F6C3d7BdA44A05dcf1437C44372");
    assert_eq!(cfg.max_swap_value_e8s, 2_000 * 100_000_000);
    assert_eq!(cfg.max_price_age_ns, 1_800_000_000_000);
    assert_eq!(cfg.max_dex_oracle_divergence_bps, 500);
    assert_eq!(cfg.fee_bps, 25);
    assert_eq!(cfg.settle_stable_decimals, 18);
    assert_eq!(cfg.deadline_secs, 180);
    // The aggregate accessors reflect the round-tripped reserve/pending terms.
    assert_eq!(decoded.total_reserve_backing_e8s(), 40);
    assert_eq!(decoded.total_pending_chain_burn_e8s(), 15);
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
        collateral_amount_native: 0,
        debt_e8s: 7_000_000_000,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    s.chain_vaults.insert(2, ChainVaultV1 {
        vault_id: 2,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xb".into(),
        collateral_amount_native: 0,
        debt_e8s: 3_000_000_000,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    });
    assert_eq!(s.total_chain_vault_debt_e8s(), 10_000_000_000);
}

// Task 12: a ChainVaultV1 snapshot written BEFORE the interest fields existed
// (no `last_interest_accrual_ns` / `pending_interest_mint_e8s` keys) MUST decode
// into the current ChainVaultV1 with those fields defaulting to 0, NOT error
// (which on a real canister wipes multi_chain state via the replay fallback).
// Then the post_upgrade stamp repairs the 0 so the first harvest doesn't bill
// from the unix epoch.
#[test]
fn legacy_chain_vault_decodes_interest_fields_defaulted_then_stamped() {
    use super::vault::{ChainVaultStatus, ChainVaultV1};
    use candid::Principal;
    use serde::Serialize;

    // The exact pre-Task-12 ChainVaultV1 shape (same field names + serde rename).
    #[derive(Serialize)]
    struct LegacyChainVault {
        vault_id: u64,
        owner: Principal,
        collateral_chain: ChainId,
        custody_address: String,
        #[serde(rename = "collateral_amount_e18")]
        collateral_amount_native: u128,
        debt_e8s: u128,
        mint_recipient: String,
        pending_mint_e8s: u128,
        status: ChainVaultStatus,
        opened_at_ns: u64,
    }
    let legacy = LegacyChainVault {
        vault_id: 7,
        owner: Principal::anonymous(),
        collateral_chain: ChainId(71),
        custody_address: "0xcustody".into(),
        collateral_amount_native: 1_234,
        debt_e8s: 555,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 99,
    };

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&legacy, &mut buf).expect("encode legacy vault");
    let decoded: ChainVaultV1 = ciborium::de::from_reader(buf.as_slice())
        .expect("legacy ChainVaultV1 MUST decode into the new shape (no state wipe)");

    // Pre-existing fields preserved exactly:
    assert_eq!(decoded.vault_id, 7);
    assert_eq!(decoded.debt_e8s, 555);
    assert_eq!(decoded.collateral_amount_native, 1_234);
    assert_eq!(decoded.opened_at_ns, 99);
    assert_eq!(decoded.status, ChainVaultStatus::Open);
    // New interest fields defaulted to 0:
    assert_eq!(decoded.last_interest_accrual_ns, 0);
    assert_eq!(decoded.pending_interest_mint_e8s, 0);

    // The post_upgrade stamp repairs the 0 last_interest_accrual_ns.
    let mut s = MultiChainState::default();
    s.chain_vaults.insert(7, decoded);
    super::supply::stamp_chain_interest_accrual_start(&mut s, 1_700_000_000_000_000_000);
    assert_eq!(s.chain_vaults[&7].last_interest_accrual_ns, 1_700_000_000_000_000_000);
    assert_eq!(s.chain_vaults[&7].debt_e8s, 555, "stamp does not touch debt");
}
