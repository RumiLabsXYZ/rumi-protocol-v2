//! Direct-state tests for the chain-admin mutations. The full update-endpoint
//! flow (caller check, event recording, traps) is exercised in PocketIC under
//! Task 12.

use super::config::{
    effective_min_quorum_providers, ChainAdminError, ChainConfigV3, ChainId, ChainStatus,
    GasStrategy, RegisterChainArg, UpdateChainConfigArg, DEFAULT_MIN_QUORUM_PROVIDERS,
};
use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use super::multi_chain_state::MultiChainStateV5;
use crate::chains::admin::{delete_chain_in_state, disable_chain_in_state, register_chain_in_state, update_chain_config_in_state};
use candid::Principal;

fn arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(101),
        display_name: "Monad".into(),
        rpc_endpoints: vec!["https://rpc.example".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 200 },
        chain_native_decimals: 18,
        min_quorum_providers: None,
    }
}

fn config_arg_999() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(999),
        display_name: "ScratchChain".into(),
        rpc_endpoints: vec!["https://rpc.scratch".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 200 },
        chain_native_decimals: 18,
        min_quorum_providers: None,
    }
}

fn dummy_vault(vault_id: u64, chain: ChainId) -> ChainVaultV1 {
    ChainVaultV1 {
        vault_id,
        owner: Principal::anonymous(),
        collateral_chain: chain,
        custody_address: "0x0000000000000000000000000000000000000000".into(),
        collateral_amount_native: 0,
        debt_e8s: 0,
        mint_recipient: "0x0000000000000000000000000000000000000000".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::AwaitingDeposit,
        opened_at_ns: 0,
    }
}

#[test]
fn register_chain_inserts_config_and_zero_supply() {
    let mut s = MultiChainStateV5::default();
    register_chain_in_state(&mut s, arg(), 1_700_000_000_000_000_000).expect("register");
    assert!(s.chain_configs.contains_key(&ChainId(101)));
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    assert!(s.settlement_queues.contains_key(&ChainId(101)));
    let cfg = &s.chain_configs[&ChainId(101)];
    assert!(matches!(cfg.status, ChainStatus::Registered));
    // Phase 1c default: a freshly registered chain has the emergency poll-scan OFF.
    assert!(!cfg.burn_watch_poll_enabled);
    // Note: not unused -- `ChainConfigV3` is brought into scope to assert the type alias.
    let _: &ChainConfigV3 = cfg;
    // Phase 1d default: the per-chain quorum-provider floor override is unset (None).
    assert_eq!(cfg.min_quorum_providers, None);
}

#[test]
fn register_chain_rejects_duplicates() {
    let mut s = MultiChainStateV5::default();
    register_chain_in_state(&mut s, arg(), 0).expect("first");
    let err = register_chain_in_state(&mut s, arg(), 0).expect_err("duplicate");
    assert!(matches!(err, ChainAdminError::ChainAlreadyRegistered(ChainId(101))));
}

#[test]
fn register_chain_rejects_empty_rpc_endpoints() {
    let mut s = MultiChainStateV5::default();
    let mut a = arg();
    a.rpc_endpoints = vec![];
    let err = register_chain_in_state(&mut s, a, 0).expect_err("empty endpoints");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}

#[test]
fn register_chain_rejects_out_of_range_decimals() {
    let mut s = MultiChainStateV5::default();
    // 0 would make the CR native-scale 1 (collateral treated as whole units),
    // inflating every CR check and admitting under-collateralized opens.
    let mut zero = arg();
    zero.chain_native_decimals = 0;
    let err = register_chain_in_state(&mut s, zero, 0).expect_err("zero decimals");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    assert!(!s.chain_configs.contains_key(&ChainId(101)), "no partial insert on reject");

    // Absurdly large decimals are also rejected.
    let mut huge = arg();
    huge.chain_native_decimals = 200;
    let err = register_chain_in_state(&mut s, huge, 0).expect_err("huge decimals");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));

    // The valid EVM (18) and Solana (9) values still register.
    let mut sol = arg();
    sol.chain_id = ChainId(102);
    sol.chain_native_decimals = 9;
    register_chain_in_state(&mut s, sol, 0).expect("9 decimals (Solana) ok");
    assert_eq!(s.chain_configs[&ChainId(102)].chain_native_decimals, 9);
}

#[test]
fn register_chain_enforces_evm_finality_floor() {
    let mut s = MultiChainStateV5::default();
    // EVM chain with finality_depth 0 is rejected.
    let mut a = arg(); // EvmEip1559 gas strategy
    a.finality_depth = 0;
    let err = register_chain_in_state(&mut s, a, 0).expect_err("evm finality 0");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    assert!(!s.chain_configs.contains_key(&ChainId(101)), "no partial insert on reject");

    // A Solana-style chain (non-EVM gas) MAY use finality_depth 0 (reads at the
    // `finalized` commitment).
    let mut sol = arg();
    sol.chain_id = ChainId(202);
    sol.chain_native_decimals = 9;
    sol.gas_strategy = GasStrategy::SolanaPriorityFee { lamports_per_cu_ceiling: 10_000 };
    sol.finality_depth = 0;
    register_chain_in_state(&mut s, sol, 0).expect("solana finality 0 ok");
    assert_eq!(s.chain_configs[&ChainId(202)].finality_depth, 0);
}

#[test]
fn disable_chain_flips_status_and_preserves_supply() {
    let mut s = MultiChainStateV5::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    s.chain_supplies.insert(ChainId(101), 999);
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    assert!(matches!(s.chain_configs[&ChainId(101)].status, ChainStatus::Disabled));
    assert_eq!(s.chain_supplies[&ChainId(101)], 999);
}

#[test]
fn set_chain_config_updates_supplied_fields_only() {
    let mut s = MultiChainStateV5::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    let original_name = s.chain_configs[&ChainId(101)].display_name.clone();
    let update = UpdateChainConfigArg {
        display_name: None,
        rpc_endpoints: Some(vec!["https://new.example".into()]),
        finality_depth: Some(5),
        gas_strategy: None,
        min_quorum_providers: None,
    };
    update_chain_config_in_state(&mut s, ChainId(101), update).expect("update");
    assert_eq!(s.chain_configs[&ChainId(101)].display_name, original_name);
    assert_eq!(s.chain_configs[&ChainId(101)].rpc_endpoints.len(), 1);
    assert_eq!(s.chain_configs[&ChainId(101)].finality_depth, 5);
}

#[test]
fn set_chain_config_rejects_unknown_chain() {
    let mut s = MultiChainStateV5::default();
    let err = update_chain_config_in_state(
        &mut s,
        ChainId(404),
        UpdateChainConfigArg::default(),
    ).expect_err("unknown chain");
    assert!(matches!(err, ChainAdminError::ChainNotRegistered(_)));
}

#[test]
fn delete_chain_removes_zero_supply_chain() {
    let mut s = MultiChainStateV5::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    // Populate EVERY per-chain map so the purge can be observed.
    s.chain_contracts.insert(c, "0xabc".into());
    s.manual_prices.insert((c, "MON".to_string()), 2_0000_0000);
    s.last_observed_block.insert(c, 42);
    s.hot_wallet_balance_e18.insert(c, 1_000);
    s.reorg_halted.insert(c, true);
    s.reorg_suspect_streak.insert(c, 2);
    // An unrelated chain's manual_prices entry must SURVIVE the delete.
    s.manual_prices.insert((ChainId(7), "MON".to_string()), 3_0000_0000);

    delete_chain_in_state(&mut s, c).expect("delete");

    assert!(!s.chain_configs.contains_key(&c), "chain_configs retained");
    assert!(!s.chain_supplies.contains_key(&c), "chain_supplies retained");
    assert!(!s.settlement_queues.contains_key(&c), "settlement_queues retained");
    assert!(!s.chain_contracts.contains_key(&c), "chain_contracts retained");
    assert!(!s.last_observed_block.contains_key(&c), "last_observed_block retained");
    assert!(!s.hot_wallet_balance_e18.contains_key(&c), "hot_wallet_balance_e18 retained");
    assert!(!s.reorg_halted.contains_key(&c), "reorg_halted retained");
    assert!(!s.reorg_suspect_streak.contains_key(&c), "reorg_suspect_streak retained");
    assert!(!s.manual_prices.contains_key(&(c, "MON".to_string())), "manual_prices retained");
    // The unrelated chain's price survives.
    assert_eq!(s.manual_prices[&(ChainId(7), "MON".to_string())], 3_0000_0000);
}

#[test]
fn delete_chain_refuses_when_supply_nonzero() {
    let mut s = MultiChainStateV5::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    s.chain_supplies.insert(c, 1);
    let err = delete_chain_in_state(&mut s, c).expect_err("nonzero supply");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    // No partial delete: the chain is STILL registered with its supply intact.
    assert!(s.chain_configs.contains_key(&c), "chain dropped despite refusal");
    assert_eq!(s.chain_supplies[&c], 1);
}

#[test]
fn delete_chain_refuses_when_open_vaults_reference_it() {
    let mut s = MultiChainStateV5::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    s.chain_vaults.insert(1, dummy_vault(1, c));
    let err = delete_chain_in_state(&mut s, c).expect_err("referencing vault");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    // No partial delete: the chain is STILL registered and the vault remains.
    assert!(s.chain_configs.contains_key(&c), "chain dropped despite refusal");
    assert!(s.chain_vaults.contains_key(&1));
}

#[test]
fn delete_chain_unknown_is_rejected() {
    let mut s = MultiChainStateV5::default();
    let err = delete_chain_in_state(&mut s, ChainId(404)).expect_err("unknown chain");
    assert!(matches!(err, ChainAdminError::ChainNotRegistered(ChainId(404))));
}
