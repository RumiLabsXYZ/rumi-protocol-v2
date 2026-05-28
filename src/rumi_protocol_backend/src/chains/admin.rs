//! Pure-state mutation helpers for the chain-admin endpoints. The
//! `#[update]` handlers in `main.rs` call into these after the caller
//! check + event recording. Kept here so unit tests can exercise the
//! state-shape rules without spinning up PocketIC.

use super::config::{
    ChainAdminError, ChainConfigV1, ChainId, ChainStatus, RegisterChainArg,
    UpdateChainConfigArg,
};
use super::multi_chain_state::MultiChainStateV1;
use super::settlement_queue::SettlementQueueV1;

pub fn register_chain_in_state(
    state: &mut MultiChainStateV1,
    arg: RegisterChainArg,
    now_ns: u64,
) -> Result<ChainConfigV1, ChainAdminError> {
    if arg.rpc_endpoints.is_empty() {
        return Err(ChainAdminError::InvalidConfig(
            "rpc_endpoints must contain at least one URL".into(),
        ));
    }
    if state.chain_configs.contains_key(&arg.chain_id) {
        return Err(ChainAdminError::ChainAlreadyRegistered(arg.chain_id));
    }
    let cfg = ChainConfigV1 {
        chain_id: arg.chain_id,
        display_name: arg.display_name,
        rpc_endpoints: arg.rpc_endpoints,
        finality_depth: arg.finality_depth,
        gas_strategy: arg.gas_strategy,
        chain_native_decimals: arg.chain_native_decimals,
        registered_at_ns: now_ns,
        status: ChainStatus::Registered,
    };
    state.chain_configs.insert(arg.chain_id, cfg.clone());
    state.chain_supplies.insert(arg.chain_id, 0);
    state.settlement_queues.insert(arg.chain_id, SettlementQueueV1::default());
    Ok(cfg)
}

pub fn disable_chain_in_state(
    state: &mut MultiChainStateV1,
    chain_id: ChainId,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;
    cfg.status = ChainStatus::Disabled;
    Ok(())
}

pub fn update_chain_config_in_state(
    state: &mut MultiChainStateV1,
    chain_id: ChainId,
    update: UpdateChainConfigArg,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;
    if let Some(name) = update.display_name {
        cfg.display_name = name;
    }
    if let Some(eps) = update.rpc_endpoints {
        if eps.is_empty() {
            return Err(ChainAdminError::InvalidConfig("rpc_endpoints cannot be empty".into()));
        }
        cfg.rpc_endpoints = eps;
    }
    if let Some(d) = update.finality_depth {
        cfg.finality_depth = d;
    }
    if let Some(g) = update.gas_strategy {
        cfg.gas_strategy = g;
    }
    Ok(())
}
