use super::multi_chain_state::{MultiChainState, MultiChainStateV1, MultiChainStateV2};
use super::config::ChainId;
use super::supply::migrate_multi_chain_state;

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
}

#[test]
fn active_alias_points_at_v2() {
    fn _check(x: MultiChainState) -> MultiChainStateV2 { x }
}
