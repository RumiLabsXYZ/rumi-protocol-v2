//! Monad testnet configuration defaults.
//!
//! These feed `register_chain` (Task 22). The deployed icUSD contract address
//! is NOT stored here as a static; it lives in runtime state
//! (MultiChainStateV2.chain_contracts, Task 3) so it survives upgrades.

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Monad testnet chain id (10143 is the published Monad testnet id as of 2026-05).
pub const MONAD_CHAIN_ID: ChainId = ChainId(10143);

/// IcUSD.sol uses 8 decimals so 1 on-chain base unit == 1 e8s.
pub const MONAD_ICUSD_DECIMALS: u8 = 8;

/// MON native gas asset decimals.
pub const MON_NATIVE_DECIMALS: u8 = 18;

/// Candidate Monad testnet RPC endpoints. The EVM RPC canister fans out to
/// these via custom JSON-RPC services. VERIFY these URLs are live at execution
/// time and pick 2-3 with adequate rate limits (spec calls for multi-provider).
pub fn monad_rpc_endpoints() -> Vec<String> {
    vec![
        "https://testnet-rpc.monad.xyz".to_string(),
        // Add 1-2 third-party Monad-testnet endpoints once confirmed available.
    ]
}

/// tECDSA key name. `test_key_1` on staging (mainnet test key); switch to
/// `key_1` for the Phase 2 production rollout. The derived addresses differ
/// per key, so the IcUSD.sol minter must be derived with this exact key.
pub fn monad_ecdsa_key_name() -> String {
    "test_key_1".to_string()
}

/// Default registration payload for Monad testnet.
pub fn monad_default_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: MONAD_CHAIN_ID,
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: monad_rpc_endpoints(),
        // Spec open question: Monad single-slot finality likely means depth 1.
        // Start at 1, verify on testnet, bump via set_chain_config if reorgs seen.
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: MON_NATIVE_DECIMALS,
        // M-05 (QUORUM-2): use the default floor (DEFAULT_MIN_QUORUM_PROVIDERS =
        // 3 distinct providers). NOTE: `monad_rpc_endpoints()` ships only ONE URL
        // today, which is BELOW the floor — the observer/settlement workers fail
        // closed until the operator adds >= 2 more independent endpoints via
        // `set_chain_config` before un-gating permissionless mainnet. This is the
        // intended fail-closed posture, not a regression.
        min_quorum_providers: None,
    }
}
