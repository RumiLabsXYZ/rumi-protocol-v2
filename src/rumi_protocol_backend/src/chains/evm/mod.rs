//! Shared EVM rail. Home for the chain-agnostic EVM modules (moved here in
//! Task 2) plus the per-chain compile-time config that the generic logic reads.

pub mod adapter;
pub mod burn_proof;
pub mod deposit_watch;
pub mod eip712;
pub mod evm_rpc;
pub mod hardening;
pub mod settlement;
pub mod settlement_proof;
pub mod tecdsa;
pub mod tx;

pub mod conflux;

#[cfg(test)]
mod tests_adapter;

#[cfg(test)]
mod tests_burn_proof;

#[cfg(test)]
mod tests_deposit_watch;

#[cfg(test)]
mod tests_eip712;

#[cfg(test)]
mod tests_evm_rpc;

#[cfg(test)]
mod tests_hardening;

#[cfg(test)]
mod tests_settlement;

#[cfg(test)]
mod tests_settlement_proof;

#[cfg(test)]
mod tests_tecdsa;

#[cfg(test)]
mod tests_tx;

use crate::chains::config::ChainId;

/// Compile-time per-chain EVM parameters that are NOT in the persisted
/// `ChainConfigV3` (rpc_endpoints, gas_strategy, finality_depth,
/// min_quorum_providers stay there). Collateral risk params are NOT here; they
/// live per-collateral in `ChainCollateralConfig` (Task 5).
///
/// Finality is intentionally NOT modeled here: it is handled by the existing
/// consensus-safe specific-block probe keyed on `ChainConfigV3.finality_depth`
/// (Monad uses 1; Conflux uses a large depth to match its deep PoW/PoS
/// finalization). A volatile `finalized` block tag is deliberately avoided
/// because a moving value breaks IC HTTPS-outcall consensus across replicas.
#[derive(Clone, Copy, Debug)]
pub struct EvmChainConfig {
    pub chain_id: ChainId,
    pub ecdsa_key_name: &'static str,
    /// Max toBlock - fromBlock span for a single eth_getLogs query (NOT the per-tick scan window). Monad provider caps at 100; Conflux eSpace at 1000.
    pub getlogs_max_range: u64,
    pub native_decimals: u8,
    pub native_symbol: &'static str,
}

/// Look up the compile-time EVM config for a chain id. `None` for non-EVM or
/// unknown chains.
pub fn evm_chain_config(chain: ChainId) -> Option<EvmChainConfig> {
    match chain.0 {
        10143 => Some(EvmChainConfig {
            chain_id: ChainId(10143),
            ecdsa_key_name: "test_key_1",
            getlogs_max_range: 100,
            native_decimals: 18,
            native_symbol: "MON",
        }),
        71 => Some(EvmChainConfig {
            chain_id: ChainId(71),
            ecdsa_key_name: "test_key_1",
            getlogs_max_range: 1000,
            native_decimals: 18,
            native_symbol: "CFX",
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chains::config::ChainId;

    #[test]
    fn monad_config_getlogs_range_100() {
        let c = evm_chain_config(ChainId(10143)).expect("monad known");
        assert_eq!(c.getlogs_max_range, 100);
        assert_eq!(c.native_symbol, "MON");
        assert_eq!(c.ecdsa_key_name, "test_key_1");
    }

    #[test]
    fn conflux_testnet_getlogs_range_1000() {
        let c = evm_chain_config(ChainId(71)).expect("conflux known");
        assert_eq!(c.getlogs_max_range, 1000);
        assert_eq!(c.native_symbol, "CFX");
        assert_eq!(c.native_decimals, 18);
    }

    #[test]
    fn unknown_chain_is_none() {
        assert!(evm_chain_config(ChainId(999)).is_none());
    }
}
