//! Solana devnet configuration defaults.
//!
//! The deployed icUSD SPL mint address is NOT a static here; it lives in
//! runtime state (`MultiChainState.chain_contracts`) so it survives upgrades,
//! exactly like the Monad icUSD contract.

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Internal multi-chain key for Solana. Solana has no EVM-style numeric chain
/// id, so we use its SLIP-44 coin type (501) as a stable, mnemonic internal key.
/// The actual network (devnet/mainnet) is selected by the RPC cluster in
/// `sol_rpc`, not by this number.
pub const SOLANA_CHAIN_ID: ChainId = ChainId(501);

/// icUSD is 8 decimals on every chain (1 base unit == 1 e8s).
pub const SOLANA_ICUSD_DECIMALS: u8 = 8;

/// Native SOL is 9 decimals (lamports).
pub const SOL_NATIVE_DECIMALS: u8 = 9;

/// Minimum collateral ratio (e4: 13000 == 130.00%) required to open a Solana
/// chain vault. Checked against DECLARED collateral at open time. Matches Monad's
/// `MONAD_MIN_CR_E4` (130%) for launch; per-collateral configurability is a later
/// refinement (Phase 2 unifies the foreign-chain and ICP-native CDP parameter
/// models). Passed to `chains::vault::open_chain_vault_in_state` by the Solana
/// vault endpoints (Task 6).
pub const SOLANA_MIN_CR_E4: u64 = 13_000;

/// Threshold-Ed25519 key name. `test_key_1` is the mainnet test key (Ed25519 has
/// NO local dfx key); switch to `key_1` for the production rollout. Derived
/// addresses differ per key, so the SPL mint authority must be derived with this
/// exact key.
pub fn solana_schnorr_key_name() -> String {
    "test_key_1".to_string()
}

/// Default registration payload for Solana devnet. `rpc_endpoints` is left empty
/// because the SOL RPC canister addresses devnet via `RpcSources::Default(Devnet)`
/// (built-in providers), not per-URL like the Monad EVM path.
pub fn solana_default_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: SOLANA_CHAIN_ID,
        display_name: "SolanaDevnet".to_string(),
        rpc_endpoints: vec![],
        // Solana finality is a commitment level (`finalized`), not block depth.
        // We keep depth 0 and read at `finalized`; see sol_rpc.rs.
        finality_depth: 0,
        gas_strategy: GasStrategy::SolanaPriorityFee {
            lamports_per_cu_ceiling: 10_000,
        },
        chain_native_decimals: SOL_NATIVE_DECIMALS,
        // M-05 (QUORUM-2): not consulted for Solana — the SOL-RPC canister
        // enforces its own Equality/Threshold consensus and rejects
        // `Inconsistent` responses. Kept `None` for shape completeness.
        min_quorum_providers: None,
    }
}
