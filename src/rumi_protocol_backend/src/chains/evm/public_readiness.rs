//! Authoritative, bounded Conflux-mainnet public risk gate.
//!
//! This module is shared by ingress, observer, and settlement so the anonymous
//! readiness query cannot drift from enforcement. Every check is fixed-size or
//! a bounded map lookup; it never scans vaults, burn proofs, or settlement ops.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::chains::collateral_config::{
    chain_collateral_config, ChainCollateralConfig, ChainDebtConfigV1,
};
use crate::chains::config::{
    effective_min_quorum_providers, ChainConfigV3, ChainId, ChainStatus, GasStrategy,
};
use crate::chains::evm::conflux::config::{
    CONFLUX_MAINNET_CHAIN_ID, CONFLUX_MAINNET_FINALITY_DEPTH,
};
use crate::chains::evm::{evm_chain_config, evm_rpc, hardening};
use crate::chains::liquidation_config::{ChainLiquidationConfigV1, DexKind};
use crate::state::{Mode, State};
use crate::ProtocolError;

pub const CONFLUX_MAINNET_PUBLIC_ICUSD_CONTRACT: &str =
    "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff";
/// Reviewed production EVM-RPC canister trust anchor. Keep this independent
/// from the wrapper's configurable/default resolution so a future default
/// change cannot silently redefine chain-1030 readiness.
pub const CONFLUX_MAINNET_EXPECTED_EVM_RPC_PRINCIPAL_TEXT: &str = "7hfb6-caaaa-aaaar-qadga-cai";
pub const CONFLUX_MAINNET_EXPECTED_ECDSA_KEY_NAME: &str = "key_1";
pub const CONFLUX_MAINNET_PUBLIC_MIN_DEBT_E8S: u128 = 10_000_000;
pub const CONFLUX_MAINNET_PUBLIC_DEBT_CEILING_E8S: u128 = 50_000_000_000;
/// Reviewed chain-1030 launch threshold: trip the circuit once cumulative
/// realized bad debt reaches one minimum-size vault debt (0.10 icUSD).
pub const CONFLUX_MAINNET_BAD_DEBT_CIRCUIT_THRESHOLD_E8S: u128 = 10_000_000;

// Reviewed Swappi V2 WCFX/USDC route. Router/factory are from Swappi's public
// contract registry; the pair is the factory's on-chain getPair(WCFX, USDC)
// result. All comparisons are case-insensitive EVM-address comparisons.
pub const CONFLUX_MAINNET_SWAPPI_ROUTER: &str = "0x62b0873055bf896dd869e172119871ac24aea305";
pub const CONFLUX_MAINNET_SWAPPI_FACTORY: &str = "0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5";
pub const CONFLUX_MAINNET_WCFX_USDC_PAIR: &str = "0x0736b3384531cda2f545f5449e84c6c6bcd6f01b";
pub const CONFLUX_MAINNET_WCFX: &str = "0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b";
pub const CONFLUX_MAINNET_USDC: &str = "0x6963efed0ab40f6c3d7bda44a05dcf1437c44372";

pub const CONFLUX_MAINNET_LIQUIDATION_SLIPPAGE_BPS: u16 = 250;
pub const CONFLUX_MAINNET_LIQUIDATION_RESTORE_TARGET_CR_E4: u64 = 15_500;
pub const CONFLUX_MAINNET_LIQUIDATION_MAX_SWAP_VALUE_E8S: u128 = 200_000_000_000;
pub const CONFLUX_MAINNET_LIQUIDATION_MAX_PRICE_AGE_NS: u64 = 1_800_000_000_000;
pub const CONFLUX_MAINNET_LIQUIDATION_MAX_DIVERGENCE_BPS: u32 = 500;
pub const CONFLUX_MAINNET_LIQUIDATION_FEE_BPS: u16 = 25;
pub const CONFLUX_MAINNET_LIQUIDATION_SETTLE_DECIMALS: u8 = 18;
pub const CONFLUX_MAINNET_LIQUIDATION_DEADLINE_SECS: u64 = 180;

/// The observer normally ticks every 30 seconds. A five-minute proof window
/// tolerates transient RPC failures while still preventing an old gas balance
/// from authorizing a new mint. Pre-V7 snapshots have no timestamp and fail
/// closed until a successful refresh.
pub const HOT_WALLET_BALANCE_MAX_AGE_NS: u64 = 300_000_000_000;

/// Bound defensive URL de-duplication even if a hand-restored snapshot bypassed
/// setter validation.
pub const MAX_PUBLIC_READINESS_RPC_ENDPOINTS: usize = 16;

pub fn expected_evm_rpc_principal() -> candid::Principal {
    candid::Principal::from_text(CONFLUX_MAINNET_EXPECTED_EVM_RPC_PRINCIPAL_TEXT)
        .expect("static Conflux mainnet EVM-RPC principal is valid")
}

pub fn bounded_distinct_rpc_endpoint_count(config: Option<&ChainConfigV3>) -> (u32, bool) {
    let Some(config) = config else {
        return (0, true);
    };
    if config.rpc_endpoints.len() > MAX_PUBLIC_READINESS_RPC_ENDPOINTS {
        return (
            u32::try_from(config.rpc_endpoints.len()).unwrap_or(u32::MAX),
            false,
        );
    }
    let distinct: BTreeSet<&str> = config.rpc_endpoints.iter().map(String::as_str).collect();
    (u32::try_from(distinct.len()).unwrap_or(u32::MAX), true)
}

pub fn expected_debt_config() -> ChainDebtConfigV1 {
    ChainDebtConfigV1 {
        min_vault_debt_e8s: CONFLUX_MAINNET_PUBLIC_MIN_DEBT_E8S,
        debt_ceiling_e8s: Some(CONFLUX_MAINNET_PUBLIC_DEBT_CEILING_E8S),
    }
}

pub fn effective_debt_config(state: &State, chain: ChainId) -> Option<ChainDebtConfigV1> {
    chain_collateral_config(chain).map(|base| {
        state
            .multi_chain
            .chain_debt_configs
            .get(&chain)
            .copied()
            .unwrap_or_else(|| ChainDebtConfigV1::from_collateral_config(base))
    })
}

pub fn debt_config_matches_expected(state: &State, chain: ChainId) -> bool {
    chain == CONFLUX_MAINNET_CHAIN_ID
        && effective_debt_config(state, chain) == Some(expected_debt_config())
}

pub fn collateral_config_matches_expected(config: ChainCollateralConfig) -> bool {
    config.min_cr_e4 == 15_000
        && config.borrow_threshold_e4 == 15_000
        && config.liquidation_penalty_bps == 1_200
        && config.borrowing_fee_bps == 30
        && config.interest_apr_bps == 200
        && config.min_vault_debt_e8s == CONFLUX_MAINNET_PUBLIC_MIN_DEBT_E8S
        && config.recovery_target_cr_e4 == CONFLUX_MAINNET_LIQUIDATION_RESTORE_TARGET_CR_E4
        && config.debt_ceiling_e8s == Some(CONFLUX_MAINNET_PUBLIC_DEBT_CEILING_E8S)
        && config.liquidation_threshold_e4 == 13_300
}

pub fn liquidation_config_matches_expected(config: &ChainLiquidationConfigV1) -> bool {
    config.dex == DexKind::UniswapV2
        && config
            .router
            .eq_ignore_ascii_case(CONFLUX_MAINNET_SWAPPI_ROUTER)
        && config
            .factory
            .eq_ignore_ascii_case(CONFLUX_MAINNET_SWAPPI_FACTORY)
        && config
            .pair
            .eq_ignore_ascii_case(CONFLUX_MAINNET_WCFX_USDC_PAIR)
        && config
            .collateral_token
            .eq_ignore_ascii_case(CONFLUX_MAINNET_WCFX)
        && config
            .settle_stable_token
            .eq_ignore_ascii_case(CONFLUX_MAINNET_USDC)
        && config.slippage_cap_bps == CONFLUX_MAINNET_LIQUIDATION_SLIPPAGE_BPS
        && config.restore_target_cr_e4 == CONFLUX_MAINNET_LIQUIDATION_RESTORE_TARGET_CR_E4
        && config.enabled
        && config.max_swap_value_e8s == CONFLUX_MAINNET_LIQUIDATION_MAX_SWAP_VALUE_E8S
        && config.max_price_age_ns == CONFLUX_MAINNET_LIQUIDATION_MAX_PRICE_AGE_NS
        && config.max_dex_oracle_divergence_bps == CONFLUX_MAINNET_LIQUIDATION_MAX_DIVERGENCE_BPS
        && config.fee_bps == CONFLUX_MAINNET_LIQUIDATION_FEE_BPS
        && config.settle_stable_decimals == CONFLUX_MAINNET_LIQUIDATION_SETTLE_DECIMALS
        && config.deadline_secs == CONFLUX_MAINNET_LIQUIDATION_DEADLINE_SECS
}

fn canonical_liquidation_config(config: &ChainLiquidationConfigV1) -> String {
    let dex = match config.dex {
        DexKind::UniswapV2 => "uniswap-v2",
    };
    format!(
        "v1|{dex}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        config.router.to_ascii_lowercase(),
        config.factory.to_ascii_lowercase(),
        config.pair.to_ascii_lowercase(),
        config.collateral_token.to_ascii_lowercase(),
        config.settle_stable_token.to_ascii_lowercase(),
        config.slippage_cap_bps,
        config.restore_target_cr_e4,
        config.enabled,
        config.max_swap_value_e8s,
        config.max_price_age_ns,
        config.max_dex_oracle_divergence_bps,
        config.fee_bps,
        config.settle_stable_decimals,
        config.deadline_secs,
    )
}

pub fn liquidation_config_digest(config: &ChainLiquidationConfigV1) -> String {
    hex::encode(Sha256::digest(
        canonical_liquidation_config(config).as_bytes(),
    ))
}

pub fn expected_liquidation_config() -> ChainLiquidationConfigV1 {
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: CONFLUX_MAINNET_SWAPPI_ROUTER.to_string(),
        factory: CONFLUX_MAINNET_SWAPPI_FACTORY.to_string(),
        pair: CONFLUX_MAINNET_WCFX_USDC_PAIR.to_string(),
        collateral_token: CONFLUX_MAINNET_WCFX.to_string(),
        settle_stable_token: CONFLUX_MAINNET_USDC.to_string(),
        slippage_cap_bps: CONFLUX_MAINNET_LIQUIDATION_SLIPPAGE_BPS,
        restore_target_cr_e4: CONFLUX_MAINNET_LIQUIDATION_RESTORE_TARGET_CR_E4,
        enabled: true,
        max_swap_value_e8s: CONFLUX_MAINNET_LIQUIDATION_MAX_SWAP_VALUE_E8S,
        max_price_age_ns: CONFLUX_MAINNET_LIQUIDATION_MAX_PRICE_AGE_NS,
        max_dex_oracle_divergence_bps: CONFLUX_MAINNET_LIQUIDATION_MAX_DIVERGENCE_BPS,
        fee_bps: CONFLUX_MAINNET_LIQUIDATION_FEE_BPS,
        settle_stable_decimals: CONFLUX_MAINNET_LIQUIDATION_SETTLE_DECIMALS,
        deadline_secs: CONFLUX_MAINNET_LIQUIDATION_DEADLINE_SECS,
    }
}

pub fn expected_liquidation_config_digest() -> String {
    liquidation_config_digest(&expected_liquidation_config())
}

pub fn hot_wallet_balance_is_fresh(state: &State, chain: ChainId, now_ns: u64) -> bool {
    state
        .multi_chain
        .hot_wallet_balance_refreshed_at_ns
        .get(&chain)
        .copied()
        .filter(|timestamp| *timestamp > 0)
        .map(|timestamp| now_ns.saturating_sub(timestamp) <= HOT_WALLET_BALANCE_MAX_AGE_NS)
        .unwrap_or(false)
}

/// Stable machine-readable blocker codes for chain 1030. Non-mainnet chains
/// return one descriptive status blocker; enforcement bypasses them below.
pub fn conflux_mainnet_public_risk_blockers(
    state: &State,
    chain: ChainId,
    now_ns: u64,
) -> Vec<&'static str> {
    if chain != CONFLUX_MAINNET_CHAIN_ID {
        return vec!["not_conflux_mainnet_public_chain"];
    }

    let mut blockers = Vec::with_capacity(32);
    let config = state.multi_chain.chain_configs.get(&chain);
    match config {
        None => blockers.push("chain_not_configured"),
        Some(config) if config.status != ChainStatus::Registered => blockers.push("chain_disabled"),
        Some(_) => {}
    }

    let evm_config = evm_chain_config(chain);
    let collateral_config = chain_collateral_config(chain);
    let persisted_shape_matches = config
        .map(|config| {
            config.chain_id == chain
                && config.chain_native_decimals == 18
                && matches!(config.gas_strategy, GasStrategy::EvmEip1559 { .. })
        })
        .unwrap_or(true);
    let compile_time_shape_matches = evm_config
        .map(|config| {
            config.chain_id == chain
                && config.native_symbol == "CFX"
                && config.native_decimals == 18
        })
        .unwrap_or(false)
        && collateral_config
            .map(collateral_config_matches_expected)
            .unwrap_or(false);
    if !persisted_shape_matches || !compile_time_shape_matches {
        blockers.push("conflux_mainnet_config_mismatch");
    }

    match state.multi_chain.chain_contracts.get(&chain) {
        None => blockers.push("icusd_contract_not_bound"),
        Some(contract) if !contract.eq_ignore_ascii_case(CONFLUX_MAINNET_PUBLIC_ICUSD_CONTRACT) => {
            blockers.push("icusd_contract_mismatch")
        }
        Some(_) => {}
    }

    if evm_rpc::evm_rpc_principal_in_state(state) != expected_evm_rpc_principal() {
        blockers.push("evm_rpc_principal_mismatch");
    }
    if state.chains_ecdsa_key_name != CONFLUX_MAINNET_EXPECTED_ECDSA_KEY_NAME {
        blockers.push("chains_ecdsa_key_mismatch");
    }

    let (rpc_endpoint_count, rpc_endpoint_configuration_bounded) =
        bounded_distinct_rpc_endpoint_count(config);
    if !rpc_endpoint_configuration_bounded {
        blockers.push("rpc_endpoint_configuration_too_large");
    }
    if rpc_endpoint_count < 2 {
        blockers.push("rpc_distinct_endpoints_insufficient");
    }
    let rpc_floor = config.map(effective_min_quorum_providers).unwrap_or(0);
    let rpc_effective_agreement_requirement = rpc_floor.max(rpc_endpoint_count / 2 + 1);
    if rpc_effective_agreement_requirement < 2 {
        blockers.push("rpc_agreement_below_two");
    }
    if rpc_endpoint_count < rpc_effective_agreement_requirement {
        blockers.push("rpc_agreement_unsatisfiable");
    }
    if config.map(|config| config.finality_depth) != Some(CONFLUX_MAINNET_FINALITY_DEPTH) {
        blockers.push("finality_depth_mismatch");
    }
    if !debt_config_matches_expected(state, chain) {
        blockers.push("debt_config_mismatch");
    }

    let liquidation_config = state.multi_chain.chain_liquidation_configs.get(&chain);
    match liquidation_config {
        None => blockers.push("liquidation_config_missing"),
        Some(config) if !config.enabled => blockers.push("liquidation_disabled"),
        Some(config) if !liquidation_config_matches_expected(config) => {
            blockers.push("liquidation_config_mismatch")
        }
        Some(_) => {}
    }

    let max_price_age_ns = liquidation_config.map(|config| config.max_price_age_ns);
    match (
        state.multi_chain.get_manual_price(chain, "CFX"),
        max_price_age_ns,
    ) {
        (None, _) => blockers.push("collateral_price_missing"),
        (Some((0, _)), _) => blockers.push("collateral_price_zero"),
        (Some((_, 0)), _) => blockers.push("collateral_price_timestamp_missing"),
        (_, None | Some(0)) => blockers.push("price_freshness_limit_missing"),
        (Some((_, set_at_ns)), Some(max_age_ns))
            if now_ns.saturating_sub(set_at_ns) > max_age_ns =>
        {
            blockers.push("collateral_price_stale")
        }
        _ => {}
    }

    if state.mode != Mode::GeneralAvailability {
        blockers.push("protocol_mode_not_general_availability");
    }
    if state.frozen {
        blockers.push("protocol_frozen");
    }
    if state.multi_chain.invariant_halted {
        blockers.push("supply_invariant_halted");
    }
    if state
        .multi_chain
        .reorg_halted
        .get(&chain)
        .copied()
        .unwrap_or(false)
    {
        blockers.push("reorg_halted");
    }
    match state
        .multi_chain
        .chain_bad_debt_circuit_threshold_e8s
        .get(&chain)
        .copied()
    {
        None | Some(0) => blockers.push("bad_debt_circuit_not_configured"),
        Some(threshold) if threshold != CONFLUX_MAINNET_BAD_DEBT_CIRCUIT_THRESHOLD_E8S => {
            blockers.push("bad_debt_circuit_threshold_mismatch")
        }
        Some(_) => {}
    }
    if state.multi_chain.chain_bad_debt_circuit_tripped(chain) {
        blockers.push("bad_debt_circuit_tripped");
    }
    if state
        .multi_chain
        .last_observed_block
        .get(&chain)
        .copied()
        .unwrap_or(0)
        == 0
    {
        blockers.push("burn_cursor_unseeded");
    }
    match state
        .multi_chain
        .hot_wallet_balance_e18
        .get(&chain)
        .copied()
    {
        None => blockers.push("hot_wallet_balance_unknown"),
        Some(balance) if !hardening::hot_wallet_ok(balance) => {
            blockers.push("hot_wallet_balance_low")
        }
        Some(_) => {}
    }
    match state
        .multi_chain
        .hot_wallet_balance_refreshed_at_ns
        .get(&chain)
        .copied()
    {
        None | Some(0) => blockers.push("hot_wallet_balance_timestamp_unknown"),
        Some(_) if !hot_wallet_balance_is_fresh(state, chain, now_ns) => {
            blockers.push("hot_wallet_balance_stale")
        }
        Some(_) => {}
    }
    blockers
}

pub fn gate_error(blockers: &[&str]) -> ProtocolError {
    let message = format!(
        "conflux mainnet public risk gate blocked: {}",
        blockers.join(",")
    );
    let configuration_failure = blockers.iter().any(|reason| {
        matches!(
            *reason,
            "chain_not_configured"
                | "chain_disabled"
                | "conflux_mainnet_config_mismatch"
                | "icusd_contract_not_bound"
                | "icusd_contract_mismatch"
                | "evm_rpc_principal_mismatch"
                | "chains_ecdsa_key_mismatch"
                | "rpc_endpoint_configuration_too_large"
                | "rpc_distinct_endpoints_insufficient"
                | "rpc_agreement_below_two"
                | "rpc_agreement_unsatisfiable"
                | "finality_depth_mismatch"
                | "debt_config_mismatch"
                | "liquidation_config_missing"
                | "liquidation_disabled"
                | "liquidation_config_mismatch"
                | "bad_debt_circuit_not_configured"
                | "bad_debt_circuit_threshold_mismatch"
        )
    });
    if configuration_failure {
        ProtocolError::EvmAuth(message)
    } else {
        ProtocolError::TemporarilyUnavailable(message)
    }
}

pub fn enforce_conflux_mainnet_public_risk_gate(
    state: &State,
    chain: ChainId,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    if chain != CONFLUX_MAINNET_CHAIN_ID {
        return Ok(());
    }
    let blockers = conflux_mainnet_public_risk_blockers(state, chain, now_ns);
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(gate_error(&blockers))
    }
}
