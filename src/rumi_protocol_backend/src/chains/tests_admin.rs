//! Direct-state tests for the chain-admin mutations. The full update-endpoint
//! flow (caller check, event recording, traps) is exercised in PocketIC under
//! Task 12.

use super::config::{
    effective_min_quorum_providers, ChainAdminError, ChainConfigV3, ChainId, ChainStatus,
    GasStrategy, RegisterChainArg, UpdateChainConfigArg, DEFAULT_MIN_QUORUM_PROVIDERS,
};
use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use super::multi_chain_state::MultiChainState;
use crate::chains::admin::{
    delete_chain_in_state, disable_chain_in_state, enable_chain_in_state, register_chain_in_state,
    update_chain_config_in_state,
};
use crate::chains::liquidation_config::{ChainLiquidationConfigV1, DexKind};
use crate::chains::settlement_queue::SettlementQueueV1;
use candid::Principal;

fn arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(101),
        display_name: "Monad".into(),
        rpc_endpoints: vec!["https://rpc.example".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 200,
        },
        chain_native_decimals: 18,
        min_quorum_providers: None,
    }
}

/// M3 (security review, 2026-08-20): a minimal liquidation config row for the
/// "populate every map" delete_chain test. Values are irrelevant; only
/// presence/absence in `chain_liquidation_configs` is being asserted.
fn m3_liq_config() -> super::liquidation_config::ChainLiquidationConfigV1 {
    use super::liquidation_config::{ChainLiquidationConfigV1, DexKind};
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: "0x1111111111111111111111111111111111111111".into(),
        factory: "0x2222222222222222222222222222222222222222".into(),
        pair: "0x3333333333333333333333333333333333333333".into(),
        collateral_token: "0x4444444444444444444444444444444444444444".into(),
        settle_stable_token: "0x5555555555555555555555555555555555555555".into(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: true,
        max_swap_value_e8s: 2_000 * 100_000_000,
        max_price_age_ns: 1_800_000_000_000,
        max_dex_oracle_divergence_bps: 500,
        fee_bps: 25,
        settle_stable_decimals: 18,
        deadline_secs: 180,
    }
}

/// A real EVM chain id (Conflux mainnet, per `chains::evm::evm_chain_config`)
/// so `xrc::pair_is_xrc_managed`, which resolves the native symbol from that
/// compile-time table, actually recognizes it. `config_arg_999`'s
/// `ChainId(999)` is deliberately NOT a known EVM chain for its own tests.
fn config_arg_cfx_mainnet() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(1030),
        display_name: "Conflux mainnet".into(),
        rpc_endpoints: vec!["https://evm.confluxrpc.com".into()],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    }
}

/// M3 (security review, 2026-08-20): the purge regression the security
/// reviewer asked for. Stages a liquidation config row (making the chain
/// XRC-managed), deletes the chain, re-registers the SAME chain id, and
/// asserts the stale row did NOT silently re-attach: `chain_is_xrc_managed`
/// is false again, so a pusher (or, pre-fix, a bot-swap path re-armed with
/// stale DEX wiring) would find the "fresh" chain unmanaged rather than
/// inheriting the old config.
#[test]
fn delete_chain_purges_liquidation_config_no_silent_reattach_on_reregister() {
    let mut s = MultiChainState::default();
    let cfx = ChainId(1030);
    register_chain_in_state(&mut s, config_arg_cfx_mainnet(), 0).expect("register");
    s.chain_liquidation_configs.insert(cfx, m3_liq_config());
    assert!(
        crate::xrc::chain_is_xrc_managed(&wrap(&s), cfx),
        "precondition: chain must be XRC-managed before delete"
    );

    delete_chain_in_state(&mut s, cfx).expect("delete");
    assert!(
        !s.chain_liquidation_configs.contains_key(&cfx),
        "chain_liquidation_configs row must not survive delete_chain"
    );

    register_chain_in_state(&mut s, config_arg_cfx_mainnet(), 1).expect("re-register");
    assert!(
        !s.chain_liquidation_configs.contains_key(&cfx),
        "re-registering must not resurrect the deleted config row"
    );
    assert!(
        !crate::xrc::chain_is_xrc_managed(&wrap(&s), cfx),
        "the re-registered chain must NOT be XRC-managed (no stale config row re-attached); \
         pre-fix, a stale enabled:true row would have re-armed the bot swap path with old DEX \
         wiring the moment this id was re-registered"
    );
    assert!(
        !crate::xrc::pair_is_xrc_managed(&wrap(&s), cfx, "CFX"),
        "manual price control (pusher included) must be available again for the fresh chain"
    );
}

/// Wraps a `MultiChainState` in the outer `State` shape `xrc::chain_is_xrc_managed`
/// / `pair_is_xrc_managed` read, without pulling in a full `State::default()`
/// construction path irrelevant to this test.
fn wrap(multi_chain: &MultiChainState) -> crate::state::State {
    let mut state = crate::state::State::default();
    state.multi_chain = multi_chain.clone();
    state
}

fn config_arg_999() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(999),
        display_name: "ScratchChain".into(),
        rpc_endpoints: vec!["https://rpc.scratch".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 200,
        },
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
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,
    }
}

#[test]
fn register_chain_inserts_config_and_zero_supply() {
    let mut s = MultiChainState::default();
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
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("first");
    let err = register_chain_in_state(&mut s, arg(), 0).expect_err("duplicate");
    assert!(matches!(
        err,
        ChainAdminError::ChainAlreadyRegistered(ChainId(101))
    ));
}

#[test]
fn register_chain_rejects_empty_rpc_endpoints() {
    let mut s = MultiChainState::default();
    let mut a = arg();
    a.rpc_endpoints = vec![];
    let err = register_chain_in_state(&mut s, a, 0).expect_err("empty endpoints");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}

#[test]
fn register_chain_rejects_out_of_range_decimals() {
    let mut s = MultiChainState::default();
    // 0 would make the CR native-scale 1 (collateral treated as whole units),
    // inflating every CR check and admitting under-collateralized opens.
    let mut zero = arg();
    zero.chain_native_decimals = 0;
    let err = register_chain_in_state(&mut s, zero, 0).expect_err("zero decimals");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    assert!(
        !s.chain_configs.contains_key(&ChainId(101)),
        "no partial insert on reject"
    );

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
    let mut s = MultiChainState::default();
    // EVM chain with finality_depth 0 is rejected.
    let mut a = arg(); // EvmEip1559 gas strategy
    a.finality_depth = 0;
    let err = register_chain_in_state(&mut s, a, 0).expect_err("evm finality 0");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    assert!(
        !s.chain_configs.contains_key(&ChainId(101)),
        "no partial insert on reject"
    );

    // A Solana-style chain (non-EVM gas) MAY use finality_depth 0 (reads at the
    // `finalized` commitment).
    let mut sol = arg();
    sol.chain_id = ChainId(202);
    sol.chain_native_decimals = 9;
    sol.gas_strategy = GasStrategy::SolanaPriorityFee {
        lamports_per_cu_ceiling: 10_000,
    };
    sol.finality_depth = 0;
    register_chain_in_state(&mut s, sol, 0).expect("solana finality 0 ok");
    assert_eq!(s.chain_configs[&ChainId(202)].finality_depth, 0);
}

#[test]
fn disable_chain_flips_status_and_preserves_supply() {
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    s.chain_supplies.insert(ChainId(101), 999);
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    assert!(matches!(
        s.chain_configs[&ChainId(101)].status,
        ChainStatus::Disabled
    ));
    assert_eq!(s.chain_supplies[&ChainId(101)], 999);
}

#[test]
fn set_chain_config_updates_supplied_fields_only() {
    let mut s = MultiChainState::default();
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
    let mut s = MultiChainState::default();
    let err = update_chain_config_in_state(&mut s, ChainId(404), UpdateChainConfigArg::default())
        .expect_err("unknown chain");
    assert!(matches!(err, ChainAdminError::ChainNotRegistered(_)));
}

#[test]
fn delete_chain_removes_zero_supply_chain() {
    let mut s = MultiChainState::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    // Populate EVERY per-chain map so the purge can be observed.
    s.chain_contracts.insert(c, "0xabc".into());
    s.manual_prices.insert((c, "MON".to_string()), 2_0000_0000);
    s.manual_price_set_at_ns.insert((c, "MON".to_string()), 123);
    s.last_observed_block.insert(c, 42);
    s.hot_wallet_balance_e18.insert(c, 1_000);
    s.reorg_halted.insert(c, true);
    s.reorg_suspect_streak.insert(c, 2);
    s.chain_bad_debt_e8s.insert(c, 77);
    s.chain_bad_debt_circuit_threshold_e8s.insert(c, 100);
    s.chain_bad_debt_circuit_tripped_at_ns.insert(c, 456);
    // M3 (security review, 2026-08-20): chain_liquidation_configs was missing
    // from the purge list; add it to this "populate every map" test so a
    // future regression here fails loudly instead of silently.
    s.chain_liquidation_configs.insert(c, m3_liq_config());
    // An unrelated chain's manual_prices entry must SURVIVE the delete.
    s.manual_prices
        .insert((ChainId(7), "MON".to_string()), 3_0000_0000);
    s.manual_price_set_at_ns
        .insert((ChainId(7), "MON".to_string()), 456);

    delete_chain_in_state(&mut s, c).expect("delete");

    assert!(!s.chain_configs.contains_key(&c), "chain_configs retained");
    assert!(
        !s.chain_supplies.contains_key(&c),
        "chain_supplies retained"
    );
    assert!(
        !s.settlement_queues.contains_key(&c),
        "settlement_queues retained"
    );
    assert!(
        !s.chain_contracts.contains_key(&c),
        "chain_contracts retained"
    );
    assert!(
        !s.last_observed_block.contains_key(&c),
        "last_observed_block retained"
    );
    assert!(
        !s.hot_wallet_balance_e18.contains_key(&c),
        "hot_wallet_balance_e18 retained"
    );
    assert!(!s.reorg_halted.contains_key(&c), "reorg_halted retained");
    assert!(
        !s.reorg_suspect_streak.contains_key(&c),
        "reorg_suspect_streak retained"
    );
    assert!(
        !s.chain_bad_debt_e8s.contains_key(&c),
        "chain_bad_debt_e8s retained"
    );
    assert!(
        !s.chain_bad_debt_circuit_threshold_e8s.contains_key(&c),
        "chain_bad_debt_circuit_threshold_e8s retained"
    );
    assert!(
        !s.chain_bad_debt_circuit_tripped_at_ns.contains_key(&c),
        "chain_bad_debt_circuit_tripped_at_ns retained"
    );
    assert!(
        !s.chain_liquidation_configs.contains_key(&c),
        "chain_liquidation_configs retained (M3 regression)"
    );
    assert!(
        !s.manual_prices.contains_key(&(c, "MON".to_string())),
        "manual_prices retained"
    );
    assert!(
        !s.manual_price_set_at_ns
            .contains_key(&(c, "MON".to_string())),
        "manual_price_set_at_ns leaked (paired-map divergence)"
    );
    // The unrelated chain's price + timestamp survive.
    assert_eq!(
        s.manual_prices[&(ChainId(7), "MON".to_string())],
        3_0000_0000
    );
    assert_eq!(
        s.manual_price_set_at_ns[&(ChainId(7), "MON".to_string())],
        456
    );
}

#[test]
fn delete_chain_refuses_when_supply_nonzero() {
    let mut s = MultiChainState::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    s.chain_supplies.insert(c, 1);
    let err = delete_chain_in_state(&mut s, c).expect_err("nonzero supply");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    // No partial delete: the chain is STILL registered with its supply intact.
    assert!(
        s.chain_configs.contains_key(&c),
        "chain dropped despite refusal"
    );
    assert_eq!(s.chain_supplies[&c], 1);
}

#[test]
fn delete_chain_refuses_when_open_vaults_reference_it() {
    let mut s = MultiChainState::default();
    let c = ChainId(999);
    register_chain_in_state(&mut s, config_arg_999(), 0).expect("register");
    s.chain_vaults.insert(1, dummy_vault(1, c));
    let err = delete_chain_in_state(&mut s, c).expect_err("referencing vault");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
    // No partial delete: the chain is STILL registered and the vault remains.
    assert!(
        s.chain_configs.contains_key(&c),
        "chain dropped despite refusal"
    );
    assert!(s.chain_vaults.contains_key(&1));
}

#[test]
fn delete_chain_unknown_is_rejected() {
    let mut s = MultiChainState::default();
    let err = delete_chain_in_state(&mut s, ChainId(404)).expect_err("unknown chain");
    assert!(matches!(
        err,
        ChainAdminError::ChainNotRegistered(ChainId(404))
    ));
}

// ── enable_chain: the recovery half of the emergency risk stop ──────────────

fn liq_config_row() -> ChainLiquidationConfigV1 {
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: "0x1111111111111111111111111111111111111111".into(),
        factory: "0x2222222222222222222222222222222222222222".into(),
        pair: "0x3333333333333333333333333333333333333333".into(),
        collateral_token: "0x4444444444444444444444444444444444444444".into(),
        settle_stable_token: "0x5555555555555555555555555555555555555555".into(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: true,
        max_swap_value_e8s: 2_000 * 100_000_000,
        max_price_age_ns: 1_800_000_000_000,
        max_dex_oracle_divergence_bps: 500,
        fee_bps: 25,
        settle_stable_decimals: 18,
        deadline_secs: 180,
    }
}

#[test]
fn enable_chain_flips_disabled_back_to_registered() {
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    assert!(matches!(
        s.chain_configs[&ChainId(101)].status,
        ChainStatus::Disabled
    ));

    enable_chain_in_state(&mut s, ChainId(101)).expect("enable");
    assert!(matches!(
        s.chain_configs[&ChainId(101)].status,
        ChainStatus::Registered
    ));
    assert!(
        s.chain_is_registered(ChainId(101)),
        "the shared predicate every risk gate reads must agree"
    );
}

#[test]
fn enable_chain_rejects_an_unknown_chain() {
    let mut s = MultiChainState::default();
    let err = enable_chain_in_state(&mut s, ChainId(404)).expect_err("unknown chain");
    assert!(matches!(
        err,
        ChainAdminError::ChainNotRegistered(ChainId(404))
    ));
    assert!(
        s.chain_configs.is_empty(),
        "enable_chain must never create a chain"
    );
}

#[test]
fn enable_chain_rejects_an_already_registered_chain() {
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    let err = enable_chain_in_state(&mut s, ChainId(101)).expect_err("already Registered");
    match err {
        ChainAdminError::InvalidConfig(msg) => {
            assert!(msg.contains("already Registered"), "msg={msg}");
        }
        other => panic!("expected InvalidConfig, got {other:?}"),
    }
    // A refused enable is inert: the chain is untouched.
    assert!(matches!(
        s.chain_configs[&ChainId(101)].status,
        ChainStatus::Registered
    ));
}

#[test]
fn disable_then_enable_preserves_every_per_chain_state_entry() {
    // The whole point of enable_chain over delete + re-register: a live chain
    // with vaults, supply, a bound contract, an observer cursor, prices and
    // liquidation wiring must come back EXACTLY as it went in. delete_chain
    // cannot even be attempted here (nonzero supply + vaults), which is why
    // this transition had to exist.
    let mut s = MultiChainState::default();
    let c = ChainId(101);
    register_chain_in_state(&mut s, arg(), 1_700_000_000_000_000_000).expect("register");

    s.chain_supplies.insert(c, 100 * 100_000_000);
    s.chain_vaults.insert(1, dummy_vault(1, c));
    s.chain_contracts.insert(c, "0xabc".into());
    s.manual_prices.insert((c, "MON".to_string()), 15_000_000);
    s.manual_price_set_at_ns
        .insert((c, "MON".to_string()), 1_700_000_000_000_000_000);
    s.last_observed_block.insert(c, 42);
    s.hot_wallet_balance_e18.insert(c, 1_000);
    s.chain_bad_debt_e8s.insert(c, 77);
    s.chain_bad_debt_circuit_threshold_e8s.insert(c, 100);
    s.chain_liquidation_configs.insert(c, liq_config_row());
    s.settlement_queues.insert(c, SettlementQueueV1::default());
    let registered_at_ns = s.chain_configs[&c].registered_at_ns;

    disable_chain_in_state(&mut s, c).expect("disable");
    enable_chain_in_state(&mut s, c).expect("enable");

    let cfg = &s.chain_configs[&c];
    assert!(matches!(cfg.status, ChainStatus::Registered));
    assert_eq!(cfg.registered_at_ns, registered_at_ns, "registration time");
    assert_eq!(cfg.display_name, "Monad", "display_name");
    assert_eq!(cfg.rpc_endpoints, vec!["https://rpc.example".to_string()]);
    assert_eq!(cfg.finality_depth, 1, "finality_depth");
    assert_eq!(cfg.chain_native_decimals, 18, "chain_native_decimals");
    assert_eq!(cfg.min_quorum_providers, None, "min_quorum_providers");
    assert!(!cfg.burn_watch_poll_enabled, "burn_watch_poll_enabled");

    assert_eq!(s.chain_supplies[&c], 100 * 100_000_000, "supply");
    assert!(s.chain_vaults.contains_key(&1), "vaults");
    assert_eq!(s.chain_contracts[&c], "0xabc", "bound contract");
    assert_eq!(s.manual_prices[&(c, "MON".to_string())], 15_000_000, "price");
    assert_eq!(
        s.manual_price_set_at_ns[&(c, "MON".to_string())],
        1_700_000_000_000_000_000,
        "price timestamp must NOT be refreshed by an enable"
    );
    assert_eq!(s.last_observed_block[&c], 42, "observer cursor");
    assert_eq!(s.hot_wallet_balance_e18[&c], 1_000, "hot wallet balance");
    assert_eq!(s.chain_bad_debt_e8s[&c], 77, "bad debt");
    assert_eq!(s.chain_bad_debt_circuit_threshold_e8s[&c], 100, "threshold");
    assert!(
        s.chain_liquidation_configs.contains_key(&c),
        "liquidation config row"
    );
    assert!(s.settlement_queues.contains_key(&c), "settlement queue");
}

#[test]
fn enable_chain_is_idempotent_only_in_the_sense_that_a_repeat_is_refused() {
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    enable_chain_in_state(&mut s, ChainId(101)).expect("first enable");
    // A second call is a VISIBLE error, not a silent success: an operator who
    // re-runs the command must not read "ok" as confirmation that a fresh
    // recovery just happened.
    let err = enable_chain_in_state(&mut s, ChainId(101)).expect_err("second enable");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}

#[test]
fn disable_enable_can_be_cycled_repeatedly() {
    let mut s = MultiChainState::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    for round in 0..3 {
        disable_chain_in_state(&mut s, ChainId(101)).unwrap_or_else(|e| panic!("disable {round}: {e:?}"));
        assert!(!s.chain_is_registered(ChainId(101)));
        enable_chain_in_state(&mut s, ChainId(101)).unwrap_or_else(|e| panic!("enable {round}: {e:?}"));
        assert!(s.chain_is_registered(ChainId(101)));
    }
}
