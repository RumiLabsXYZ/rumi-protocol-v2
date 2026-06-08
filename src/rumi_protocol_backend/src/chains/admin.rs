//! Pure-state mutation helpers for the chain-admin endpoints. The
//! `#[update]` handlers in `main.rs` call into these after the caller
//! check + event recording. Kept here so unit tests can exercise the
//! state-shape rules without spinning up PocketIC.

use super::config::{
    ChainAdminError, ChainConfigV3, ChainId, ChainStatus, GasStrategy, RegisterChainArg,
    UpdateChainConfigArg,
};
use super::multi_chain_state::MultiChainStateV4;
use super::settlement_queue::SettlementQueueV1;

/// EVM chains need >= 1 confirmation before a block is treated as final. A
/// `finality_depth` of 0 makes `is_block_final(block, 0)` true for any mined
/// block, defeating reorg safety on the burn/settlement-confirm paths. Solana
/// (which reads at the `finalized` commitment) legitimately uses 0, so the floor
/// applies only to EVM gas strategies.
fn is_evm(gas: &GasStrategy) -> bool {
    matches!(gas, GasStrategy::EvmEip1559 { .. } | GasStrategy::EvmLegacy { .. })
}

/// Audit M-04 (QUORUM-1): de-duplicate `rpc_endpoints` by URL, preserving the
/// first occurrence's order. The EVM-RPC quorum (`call_evm_rpc`) tallies
/// agreement by DISTINCT provider; if the same URL appears twice it would be
/// queried (and could vote) twice, letting ONE provider satisfy a >1 quorum on
/// its own. Deduping at registration/config time guarantees the persisted list
/// already contains only distinct providers, so the runtime tally is honest.
/// Comparison is exact-string (operators must supply canonical URLs); this never
/// drops a genuinely distinct endpoint.
fn dedup_endpoints(endpoints: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(endpoints.len());
    for url in endpoints {
        if seen.insert(url.clone()) {
            out.push(url);
        }
    }
    out
}

pub fn register_chain_in_state(
    state: &mut MultiChainStateV4,
    arg: RegisterChainArg,
    now_ns: u64,
) -> Result<ChainConfigV3, ChainAdminError> {
    // M-04 (QUORUM-1): collapse duplicate URLs BEFORE the non-empty check, so a
    // list of nothing but repeats (which would give a false sense of quorum) is
    // rejected as effectively empty.
    let rpc_endpoints = dedup_endpoints(arg.rpc_endpoints);
    if rpc_endpoints.is_empty() {
        return Err(ChainAdminError::InvalidConfig(
            "rpc_endpoints must contain at least one (distinct) URL".into(),
        ));
    }
    // Validate the native-asset decimals. This feeds `collateral_ratio_e4`
    // (10^chain_native_decimals is the divisor that converts native base units
    // to whole units); a wrong value silently mis-scales every CR check for the
    // chain. `0` makes the scale 1 (collateral treated as whole units, CR
    // inflated ~1e9-1e18x -> under-collateralized opens accepted); an absurdly
    // large value underflows CR to 0 (fails-closed). Expected: 18 for EVM, 9 for
    // Solana. Reject anything outside a sane band at registration.
    if arg.chain_native_decimals == 0 || arg.chain_native_decimals > 36 {
        return Err(ChainAdminError::InvalidConfig(format!(
            "chain_native_decimals {} out of range (expected 1..=36; 18 for EVM, 9 for Solana)",
            arg.chain_native_decimals
        )));
    }
    if is_evm(&arg.gas_strategy) && arg.finality_depth == 0 {
        return Err(ChainAdminError::InvalidConfig(
            "finality_depth must be >= 1 for EVM chains (0 treats any mined block as final)".into(),
        ));
    }
    // M-05 (QUORUM-2): a per-chain override of 0 distinct providers is nonsense
    // (it would disable the fail-closed floor); require >= 1. `None` means "use
    // DEFAULT_MIN_QUORUM_PROVIDERS".
    if let Some(0) = arg.min_quorum_providers {
        return Err(ChainAdminError::InvalidConfig(
            "min_quorum_providers must be >= 1 (omit/null for the default floor)".into(),
        ));
    }
    if state.chain_configs.contains_key(&arg.chain_id) {
        return Err(ChainAdminError::ChainAlreadyRegistered(arg.chain_id));
    }
    let cfg = ChainConfigV3 {
        chain_id: arg.chain_id,
        display_name: arg.display_name,
        rpc_endpoints,
        finality_depth: arg.finality_depth,
        gas_strategy: arg.gas_strategy,
        chain_native_decimals: arg.chain_native_decimals,
        registered_at_ns: now_ns,
        status: ChainStatus::Registered,
        // Phase 1c: notify-then-verify is the default; the continuous poll-scan
        // starts OFF and is enabled per-chain only via set_burn_watch_poll_enabled.
        burn_watch_poll_enabled: false,
        // M-05 (QUORUM-2): per-chain quorum-provider floor override (None => default).
        min_quorum_providers: arg.min_quorum_providers,
    };
    state.chain_configs.insert(arg.chain_id, cfg.clone());
    state.chain_supplies.insert(arg.chain_id, 0);
    state.settlement_queues.insert(arg.chain_id, SettlementQueueV1::default());
    Ok(cfg)
}

pub fn disable_chain_in_state(
    state: &mut MultiChainStateV4,
    chain_id: ChainId,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;
    cfg.status = ChainStatus::Disabled;
    Ok(())
}

/// Remove a chain entirely. Only permitted when the chain carries ZERO supply
/// and NO chain_vaults reference it (so deletion cannot orphan debt/collateral).
///
/// Purges the chain from EVERY per-chain map (a stale entry left in any of them
/// would be a silent state leak). All-or-nothing: every rejection path returns
/// before the first mutation, so a refused delete leaves the chain fully intact.
pub fn delete_chain_in_state(
    state: &mut MultiChainStateV4,
    chain_id: ChainId,
) -> Result<(), ChainAdminError> {
    if !state.chain_configs.contains_key(&chain_id) {
        return Err(ChainAdminError::ChainNotRegistered(chain_id));
    }
    let supply = state.chain_supplies.get(&chain_id).copied().unwrap_or(0);
    if supply != 0 {
        return Err(ChainAdminError::InvalidConfig(format!(
            "chain {} has nonzero supply {}",
            chain_id.0, supply
        )));
    }
    if state.chain_vaults.values().any(|v| v.collateral_chain == chain_id) {
        return Err(ChainAdminError::InvalidConfig(format!(
            "chain {} still has vaults",
            chain_id.0
        )));
    }
    // Remove from EVERY per-chain map (a stale entry in any of these is a leak).
    state.chain_configs.remove(&chain_id);
    state.chain_supplies.remove(&chain_id);
    state.settlement_queues.remove(&chain_id);
    state.chain_contracts.remove(&chain_id);
    state.last_observed_block.remove(&chain_id);
    state.hot_wallet_balance_e18.remove(&chain_id);
    state.reorg_halted.remove(&chain_id);
    state.reorg_suspect_streak.remove(&chain_id); // Task-11 reorg-debounce streak
    // manual_prices is keyed by (ChainId, String) — drop all entries for this chain.
    state.manual_prices.retain(|(c, _), _| *c != chain_id);
    Ok(())
}

pub fn update_chain_config_in_state(
    state: &mut MultiChainStateV4,
    chain_id: ChainId,
    update: UpdateChainConfigArg,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;

    // Validate the FULL post-update config before mutating anything
    // (all-or-nothing). rpc_endpoints must stay non-empty, and an EVM chain must
    // keep finality_depth >= 1 even if only one of (gas_strategy, finality_depth)
    // is being changed.
    //
    // M-04 (QUORUM-1): dedup the supplied endpoints FIRST, then check non-empty
    // against the deduped list (a list of nothing but repeats is effectively
    // empty and must be rejected). The deduped list is what gets persisted.
    let deduped_endpoints = update.rpc_endpoints.map(dedup_endpoints);
    if let Some(eps) = &deduped_endpoints {
        if eps.is_empty() {
            return Err(ChainAdminError::InvalidConfig(
                "rpc_endpoints cannot be empty (after de-duplication)".into(),
            ));
        }
    }
    let effective_finality = update.finality_depth.unwrap_or(cfg.finality_depth);
    let effective_is_evm = match &update.gas_strategy {
        Some(g) => is_evm(g),
        None => is_evm(&cfg.gas_strategy),
    };
    if effective_is_evm && effective_finality == 0 {
        return Err(ChainAdminError::InvalidConfig(
            "finality_depth must be >= 1 for EVM chains (0 treats any mined block as final)".into(),
        ));
    }
    // M-05 (QUORUM-2): if the override is being SET (Some(Some(n))), it must be
    // >= 1. Some(None) clears it back to the default; None leaves it unchanged.
    if let Some(Some(0)) = update.min_quorum_providers {
        return Err(ChainAdminError::InvalidConfig(
            "min_quorum_providers must be >= 1 (pass opt null to clear to the default)".into(),
        ));
    }

    if let Some(name) = update.display_name {
        cfg.display_name = name;
    }
    if let Some(eps) = deduped_endpoints {
        cfg.rpc_endpoints = eps;
    }
    if let Some(d) = update.finality_depth {
        cfg.finality_depth = d;
    }
    if let Some(g) = update.gas_strategy {
        cfg.gas_strategy = g;
    }
    // M-05 (QUORUM-2): outer Some => caller addressed the field. Inner Some(n)
    // sets the override; inner None clears it back to the default.
    if let Some(override_value) = update.min_quorum_providers {
        cfg.min_quorum_providers = override_value;
    }
    Ok(())
}
