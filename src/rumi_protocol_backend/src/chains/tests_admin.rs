//! Direct-state tests for the chain-admin mutations. The full update-endpoint
//! flow (caller check, event recording, traps) is exercised in PocketIC under
//! Task 12.

use super::config::{ChainAdminError, ChainConfigV1, ChainId, ChainStatus, GasStrategy, RegisterChainArg, UpdateChainConfigArg};
use super::multi_chain_state::MultiChainStateV2;
use crate::chains::admin::{disable_chain_in_state, register_chain_in_state, update_chain_config_in_state};

fn arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(101),
        display_name: "Monad".into(),
        rpc_endpoints: vec!["https://rpc.example".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 200 },
        chain_native_decimals: 18,
    }
}

#[test]
fn register_chain_inserts_config_and_zero_supply() {
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, arg(), 1_700_000_000_000_000_000).expect("register");
    assert!(s.chain_configs.contains_key(&ChainId(101)));
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    assert!(s.settlement_queues.contains_key(&ChainId(101)));
    let cfg = &s.chain_configs[&ChainId(101)];
    assert!(matches!(cfg.status, ChainStatus::Registered));
    // Note: not unused -- `ChainConfigV1` is brought into scope to assert the type alias.
    let _: &ChainConfigV1 = cfg;
}

#[test]
fn register_chain_rejects_duplicates() {
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, arg(), 0).expect("first");
    let err = register_chain_in_state(&mut s, arg(), 0).expect_err("duplicate");
    assert!(matches!(err, ChainAdminError::ChainAlreadyRegistered(ChainId(101))));
}

#[test]
fn register_chain_rejects_empty_rpc_endpoints() {
    let mut s = MultiChainStateV2::default();
    let mut a = arg();
    a.rpc_endpoints = vec![];
    let err = register_chain_in_state(&mut s, a, 0).expect_err("empty endpoints");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}

#[test]
fn disable_chain_flips_status_and_preserves_supply() {
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    s.chain_supplies.insert(ChainId(101), 999);
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    assert!(matches!(s.chain_configs[&ChainId(101)].status, ChainStatus::Disabled));
    assert_eq!(s.chain_supplies[&ChainId(101)], 999);
}

#[test]
fn set_chain_config_updates_supplied_fields_only() {
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    let original_name = s.chain_configs[&ChainId(101)].display_name.clone();
    let update = UpdateChainConfigArg {
        display_name: None,
        rpc_endpoints: Some(vec!["https://new.example".into()]),
        finality_depth: Some(5),
        gas_strategy: None,
    };
    update_chain_config_in_state(&mut s, ChainId(101), update).expect("update");
    assert_eq!(s.chain_configs[&ChainId(101)].display_name, original_name);
    assert_eq!(s.chain_configs[&ChainId(101)].rpc_endpoints.len(), 1);
    assert_eq!(s.chain_configs[&ChainId(101)].finality_depth, 5);
}

#[test]
fn set_chain_config_rejects_unknown_chain() {
    let mut s = MultiChainStateV2::default();
    let err = update_chain_config_in_state(
        &mut s,
        ChainId(404),
        UpdateChainConfigArg::default(),
    ).expect_err("unknown chain");
    assert!(matches!(err, ChainAdminError::ChainNotRegistered(_)));
}
