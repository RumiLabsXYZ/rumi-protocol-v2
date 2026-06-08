use super::supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};
use super::config::{ChainConfigV3, ChainId, ChainStatus, GasStrategy};
use super::multi_chain_state::MultiChainStateV4;

fn fixture_state() -> MultiChainStateV4 {
    let mut s = MultiChainStateV4::default();
    s.chain_configs.insert(
        ChainId(101),
        ChainConfigV3 {
            chain_id: ChainId(101),
            display_name: "TestChain".into(),
            rpc_endpoints: vec![],
            finality_depth: 1,
            gas_strategy: GasStrategy::NotApplicable,
            chain_native_decimals: 18,
            registered_at_ns: 0,
            status: ChainStatus::Registered,
            burn_watch_poll_enabled: false,
            min_quorum_providers: None,
        },
    );
    s.chain_supplies.insert(ChainId(101), 0);
    s
}

#[test]
fn increase_supply_preserves_invariant() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1_000),
        /* total_debt_e8s = */ 1_000,
    );
    assert!(res.is_ok());
    assert_eq!(s.chain_supplies[&ChainId(101)], 1_000);
}

#[test]
fn decrease_supply_below_zero_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(500),
        /* total_debt_e8s = */ 0,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Underflow { .. })));
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}

#[test]
fn decrease_to_exact_zero_keeps_entry_for_audit() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 50);
    apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(50),
        /* total_debt_e8s = */ 0,
    ).expect("decrease to zero");
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    assert!(s.chain_supplies.contains_key(&ChainId(101)));
}

#[test]
fn unknown_chain_id_is_rejected() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(999),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(res, Err(SupplyInvariantError::UnknownChain(_))));
}

#[test]
fn invariant_halted_blocks_every_mutation() {
    let mut s = fixture_state();
    s.invariant_halted = true;
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(res, Err(SupplyInvariantError::HaltedAfterSelfCheckFailure)));
}

#[test]
fn divergence_from_total_debt_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(50),
        /* total_debt_e8s = */ 200,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Divergence { .. })));
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}
