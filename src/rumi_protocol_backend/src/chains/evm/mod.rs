//! Shared EVM rail. Home for the chain-agnostic EVM modules (moved here in
//! Task 2) plus the per-chain compile-time config that the generic logic reads.

pub mod adapter;
pub mod burn_proof;
pub mod deposit_watch;
pub mod evm_rpc;
pub mod hardening;
pub mod settlement;
pub mod tecdsa;
pub mod tx;

#[cfg(test)]
mod tests_adapter;

#[cfg(test)]
mod tests_burn_proof;

#[cfg(test)]
mod tests_deposit_watch;

#[cfg(test)]
mod tests_evm_rpc;

#[cfg(test)]
mod tests_hardening;

#[cfg(test)]
mod tests_settlement;

#[cfg(test)]
mod tests_tecdsa;

#[cfg(test)]
mod tests_tx;

use crate::chains::config::ChainId;

/// How a chain's "finalized" height is determined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalityMode {
    /// finalized_height = latest - depth, probed at a specific block number.
    /// Monad: single-slot finality, depth 1.
    FixedDepth(u32),
    /// Query the chain's `finalized` block tag. Conflux: the PoW Tree-Graph
    /// layer can reorg beyond a small depth; the PoS `finalized` tag is the
    /// real irreversibility guarantee.
    FinalizedTag,
}

/// Compile-time per-chain EVM parameters that are NOT in the persisted
/// `ChainConfigV3` (rpc_endpoints, gas_strategy, finality_depth,
/// min_quorum_providers stay there). Collateral risk params are NOT here; they
/// live per-collateral in `ChainCollateralConfig` (Task 5).
#[derive(Clone, Copy, Debug)]
pub struct EvmChainConfig {
    pub chain_id: ChainId,
    pub ecdsa_key_name: &'static str,
    pub finality: FinalityMode,
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
            finality: FinalityMode::FixedDepth(1),
            getlogs_max_range: 100,
            native_decimals: 18,
            native_symbol: "MON",
        }),
        71 => Some(EvmChainConfig {
            chain_id: ChainId(71),
            ecdsa_key_name: "test_key_1",
            finality: FinalityMode::FinalizedTag,
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
    fn monad_config_is_fixed_depth_window_100() {
        let c = evm_chain_config(ChainId(10143)).expect("monad known");
        assert!(matches!(c.finality, FinalityMode::FixedDepth(1)));
        assert_eq!(c.getlogs_max_range, 100);
        assert_eq!(c.native_symbol, "MON");
    }

    #[test]
    fn conflux_testnet_is_finalized_tag_window_1000() {
        let c = evm_chain_config(ChainId(71)).expect("conflux known");
        assert!(matches!(c.finality, FinalityMode::FinalizedTag));
        assert_eq!(c.getlogs_max_range, 1000);
        assert_eq!(c.native_symbol, "CFX");
    }

    #[test]
    fn unknown_chain_is_none() {
        assert!(evm_chain_config(ChainId(999)).is_none());
    }
}
