//! Conflux eSpace configuration.
//!
//! eSpace is EVM-compatible and supports EIP-1559 (since the v2.4.0 hardfork),
//! so the shared EVM tx builder / tECDSA / EVM-RPC path applies unchanged.
//! Finality: Conflux has a PoW Tree-Graph layer plus a PoS finality chain;
//! deep reorgs are possible on the PoW layer, so we rely on the existing
//! consensus-safe specific-block probe with a LARGE `finality_depth` (never a
//! volatile `finalized` tag, which would break IC HTTPS-outcall consensus).

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Conflux eSpace TESTNET chain id (mainnet is 1030).
pub const CONFLUX_TESTNET_CHAIN_ID: ChainId = ChainId(71);
/// Conflux eSpace MAINNET chain id. This is where real Swappi liquidity lives.
pub const CONFLUX_MAINNET_CHAIN_ID: ChainId = ChainId(1030);

/// CFX native gas asset decimals (wei-style, like ETH).
pub const CFX_NATIVE_DECIMALS: u8 = 18;

/// Mainnet finality default used by the gated-launch runbook.
pub const CONFLUX_MAINNET_FINALITY_DEPTH: u32 = 400;
/// Minimum independent-provider quorum floor for mainnet financial reads.
pub const CONFLUX_MAINNET_MIN_QUORUM_PROVIDERS: u32 = 2;

/// Candidate Conflux eSpace TESTNET RPC endpoints. VERIFY live at deploy time.
/// NOTE: all three are Confura (the Conflux Foundation's own service, one
/// operator), so the read quorum is RELAXED to 1 below. A real multi-provider
/// quorum for mainnet needs independent providers (NOWNodes / BlockPi /
/// Validation Cloud) and a raised `min_quorum_providers`.
pub fn conflux_testnet_rpc_endpoints() -> Vec<String> {
    vec![
        "https://evmtestnet.confluxrpc.com".to_string(),
        "https://evmtest.confluxrpc.com".to_string(),
        "https://evmtestnet.confluxrpc.org".to_string(),
    ]
}

/// Default registration payload for Conflux eSpace testnet.
pub fn conflux_testnet_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: CONFLUX_TESTNET_CHAIN_ID,
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: conflux_testnet_rpc_endpoints(),
        // Conflux deep finality: the existing specific-block probe treats a
        // block as final only when buried under `finality_depth` confirmations.
        // 100 is a TESTNET default (~2 min at ~1.25s/block) for fast iteration.
        // MAINNET must raise this to reflect Conflux's documented ~400-block PoS
        // finalization (a security-review parameter).
        finality_depth: 100,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100, // tune from observed eSpace testnet base fee
        },
        chain_native_decimals: CFX_NATIVE_DECIMALS,
        // RELAXED to 1: all configured endpoints are Confura (one operator).
        // Mainnet needs >= 3 independent providers (raise via set_chain_config).
        min_quorum_providers: Some(1),
    }
}

/// Registration payload for Conflux eSpace mainnet.
///
/// Operators must pass independent, deployment-vetted RPC endpoints. This helper
/// deliberately does not carry baked-in provider URLs because the safety property
/// is provider independence, not any specific public endpoint.
pub fn conflux_mainnet_register_arg(rpc_endpoints: Vec<String>) -> RegisterChainArg {
    RegisterChainArg {
        chain_id: CONFLUX_MAINNET_CHAIN_ID,
        display_name: "ConfluxESpaceMainnet".to_string(),
        rpc_endpoints,
        finality_depth: CONFLUX_MAINNET_FINALITY_DEPTH,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 200,
        },
        chain_native_decimals: CFX_NATIVE_DECIMALS,
        min_quorum_providers: Some(CONFLUX_MAINNET_MIN_QUORUM_PROVIDERS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_arg_is_conflux_testnet() {
        let arg = conflux_testnet_register_arg();
        assert_eq!(arg.chain_id, ChainId(71));
        assert_eq!(arg.chain_native_decimals, 18);
        assert_eq!(arg.min_quorum_providers, Some(1));
        assert_eq!(arg.rpc_endpoints.len(), 3);
        assert!(matches!(arg.gas_strategy, GasStrategy::EvmEip1559 { .. }));
        assert!(arg.finality_depth >= 1); // register validation requires >= 1
    }

    #[test]
    fn register_arg_is_conflux_mainnet() {
        let endpoints = vec![
            "https://evm.confluxrpc.com".to_string(),
            "https://conflux-espace.example-rpc-a.invalid".to_string(),
            "https://conflux-espace.example-rpc-b.invalid".to_string(),
        ];
        let arg = conflux_mainnet_register_arg(endpoints.clone());
        assert_eq!(arg.chain_id, ChainId(1030));
        assert_eq!(arg.display_name, "ConfluxESpaceMainnet");
        assert_eq!(arg.rpc_endpoints, endpoints);
        assert_eq!(arg.finality_depth, CONFLUX_MAINNET_FINALITY_DEPTH);
        assert_eq!(arg.chain_native_decimals, CFX_NATIVE_DECIMALS);
        assert_eq!(
            arg.min_quorum_providers,
            Some(CONFLUX_MAINNET_MIN_QUORUM_PROVIDERS)
        );
        assert!(matches!(
            arg.gas_strategy,
            GasStrategy::EvmEip1559 {
                max_priority_fee_gwei: 1,
                max_fee_gwei_ceiling: 200
            }
        ));
    }
}
