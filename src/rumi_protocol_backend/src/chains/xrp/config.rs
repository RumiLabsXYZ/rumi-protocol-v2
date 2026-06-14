//! Native-XRP chain configuration defaults (mirrors `chains::solana::config`).
//!
//! XRP is a foreign COLLATERAL chain: a user funds a threshold-derived XRPL
//! custody address, the protocol verifies the deposit and mints icUSD on the IC
//! (icUSD is IC-native — there is NO icUSD token on the XRPL, so the adapter's
//! `sign_mint`/`sign_burn` are `NotImplemented`). On close/withdraw the canister
//! builds + threshold-signs an XRPL `Payment` back to the user.

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Internal multi-chain key for the XRP Ledger. XRP has no EVM-style numeric
/// chain id, so we use its SLIP-44 coin type (144) as a stable, mnemonic internal
/// key — the same convention Solana uses (501). Mainnet vs testnet is selected by
/// the rippled RPC URL in `xrp_rpc`, not by this number.
pub const XRP_CHAIN_ID: ChainId = ChainId(144);

/// icUSD is 8 decimals on every chain (1 base unit == 1 e8s).
pub const XRP_ICUSD_DECIMALS: u8 = 8;

/// Native XRP is 6 decimals: 1 XRP = 1_000_000 drops. (Drops are the on-wire
/// integer unit; `Amount`/`Fee`/`Balance` in `xrp_rpc` are all drops.)
pub const XRP_NATIVE_DECIMALS: u8 = 6;

/// Minimum collateral ratio (e4: 13000 == 130.00%) to open an XRP chain vault.
/// Matches `SOLANA_MIN_CR_E4` / `MONAD_MIN_CR_E4` for launch; per-collateral
/// configurability is a later refinement.
pub const XRP_MIN_CR_E4: u64 = 13_000;

/// Threshold-Ed25519 key name. XRPL Ed25519 reuses the SAME threshold Schnorr
/// Ed25519 key as Solana (`test_key_1` is the mainnet test key — Ed25519 has no
/// local dfx key); switch to `key_1` for production. The XRP rail keeps its keys
/// distinct from Solana's by using a DIFFERENT derivation path (the chain-id 144
/// prefix), not a different key name — see `ted25519`.
pub fn xrp_schnorr_key_name() -> String {
    "test_key_1".to_string()
}

/// Default registration payload for the XRP Ledger.
///
/// `rpc_endpoints` is left empty: `xrp_rpc` selects the public rippled cluster by
/// key name (mainnet for `key_1`, the altnet testnet otherwise), like Solana's
/// built-in providers. `finality_depth` is 0 — XRPL has no block-depth finality;
/// a validated ledger IS final, and reads use `ledger_index: "validated"`.
/// `min_quorum_providers` is `None`: like Solana, XRP does not use the EVM quorum
/// floor (Phase 1 is single-cluster; multi-node agreement is Phase 2).
pub fn xrp_default_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: XRP_CHAIN_ID,
        display_name: "XrpTestnet".to_string(),
        rpc_endpoints: vec![],
        finality_depth: 0,
        // XRPL fees are fixed drops in Phase 1 (see xrp_rpc / adapter), not a
        // priority-bid model; no EVM/Solana fee strategy applies.
        gas_strategy: GasStrategy::NotApplicable,
        chain_native_decimals: XRP_NATIVE_DECIMALS,
        min_quorum_providers: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_register_arg_is_well_formed() {
        let arg = xrp_default_register_arg();
        assert_eq!(arg.chain_id, XRP_CHAIN_ID);
        assert_eq!(arg.chain_native_decimals, 6);
        assert_eq!(arg.finality_depth, 0);
        assert!(arg.rpc_endpoints.is_empty());
        assert!(matches!(arg.gas_strategy, GasStrategy::NotApplicable));
    }

    #[test]
    fn xrp_chain_id_is_distinct_from_solana() {
        // SLIP-44: XRP = 144, Solana = 501. A collision would alias derivation
        // paths and merge the two chains' keys/state.
        assert_eq!(XRP_CHAIN_ID.0, 144);
        assert_ne!(XRP_CHAIN_ID.0, crate::chains::solana::config::SOLANA_CHAIN_ID.0);
    }
}
