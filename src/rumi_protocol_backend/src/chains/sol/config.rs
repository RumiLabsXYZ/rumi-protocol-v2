//! Native-SOL-collateral chain configuration and the key/cluster coupling
//! (mirrors `chains::xrp::config`).
//!
//! SOL is a foreign COLLATERAL chain, like XRP: a user funds a per-vault
//! threshold-Ed25519 Solana address, the protocol verifies the deposit and
//! mints icUSD on the IC (icUSD is IC-native — there is no icUSD token on
//! Solana on this rail). On withdraw/liquidation/redemption the canister
//! builds + threshold-signs a durable-nonce SOL transfer back out.
//!
//! ## Key name / cluster coupling (design doc §2)
//! XRP's most important custody-safety property is that the Schnorr key name
//! is runtime `State`, and the RPC network is DERIVED from it, so custody
//! derivation and the network being read can never disagree. `chains::solana`
//! (the dormant SPL-mint rail) has neither of these — its key name and cluster
//! are both hardcoded constants — which is fine for a dormant devnet rail but
//! unusable for real collateral. `chains::sol` copies XRP's pattern exactly:
//! `is_sol_production_key_name` is the SINGLE predicate that decides both the
//! Schnorr `key_id()` (see `ted25519.rs`) and the RPC cluster (`sol_cluster`
//! below), so there is no way for the two to desync.

use crate::chains::config::ChainId;
use crate::chains::solana::sol_rpc::SolanaCluster;

/// Internal multi-chain key for Solana (SLIP-44 coin type 501). Intentionally
/// the SAME numeric value as the dormant `chains::solana::config::SOLANA_CHAIN_ID`
/// — see `ted25519::sol_custody_derivation_path`'s doc comment for why an
/// identical chain id does not create a derivation-path collision: the two
/// rails' path VECTORS differ in shape (this rail's paths carry an explicit
/// `b"collateral"` role-label element that the dormant rail's paths do not).
pub const SOL_CHAIN_ID: ChainId = ChainId(501);

/// Native SOL is 9 decimals (lamports). Matches `chains::solana::config::SOL_NATIVE_DECIMALS`.
pub const SOL_NATIVE_DECIMALS: u8 = 9;

/// Production threshold-Ed25519 key name.
pub const SOL_PRODUCTION_SCHNORR_KEY_NAME: &str = "key_1";

/// Non-production threshold-Ed25519 key name used by default.
pub const SOL_TEST_SCHNORR_KEY_NAME: &str = "test_key_1";

/// True only for the management-canister production Schnorr key. Drives BOTH
/// `ted25519::key_id()` and `sol_cluster()` — see the module doc comment.
pub fn is_sol_production_key_name(name: &str) -> bool {
    name == SOL_PRODUCTION_SCHNORR_KEY_NAME
}

/// Threshold-Ed25519 key name for the native-SOL-collateral rail, read from
/// runtime `State`. `set_sol_schnorr_key_name` (developer-only, Phase 2)
/// refuses to change this once any SOL custody state exists, mirroring
/// `chains::xrp`'s `validate_xrp_schnorr_key_change`.
pub fn sol_schnorr_key_name() -> String {
    crate::read_state(|s| s.sol_schnorr_key_name.clone())
}

/// The SOL RPC cluster to talk to, derived from the CONFIGURED key name (not a
/// separate setting): the production key implies mainnet-beta, any other key
/// implies devnet. One function, one decision — `chains::sol::rpc` calls this
/// on every request rather than reading a second, independently-settable flag,
/// so custody addresses (derived under the configured key) and the network
/// being queried can never point at different clusters.
pub fn sol_cluster() -> SolanaCluster {
    if is_sol_production_key_name(&sol_schnorr_key_name()) {
        SolanaCluster::Mainnet
    } else {
        SolanaCluster::Devnet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sol_chain_id_matches_slip44_and_equals_dormant_rail_numerically() {
        // Intentional: see the SOL_CHAIN_ID doc comment. The numeric equality is
        // safe because the derivation PATH SHAPE differs (ted25519 tests assert
        // the actual non-collision).
        assert_eq!(SOL_CHAIN_ID.0, 501);
        assert_eq!(SOL_CHAIN_ID.0, crate::chains::solana::config::SOLANA_CHAIN_ID.0);
    }

    #[test]
    fn key_names_are_explicit_and_default_to_test_key() {
        assert_eq!(SOL_PRODUCTION_SCHNORR_KEY_NAME, "key_1");
        assert_eq!(SOL_TEST_SCHNORR_KEY_NAME, "test_key_1");
        assert_eq!(
            crate::state::State::default().sol_schnorr_key_name,
            SOL_TEST_SCHNORR_KEY_NAME
        );
        assert!(is_sol_production_key_name(SOL_PRODUCTION_SCHNORR_KEY_NAME));
        assert!(!is_sol_production_key_name(SOL_TEST_SCHNORR_KEY_NAME));
        assert!(!is_sol_production_key_name("key_10"));
    }

    #[test]
    fn cluster_coupling_follows_key_name() {
        let mut state = crate::state::State::default();
        state.sol_schnorr_key_name = SOL_TEST_SCHNORR_KEY_NAME.to_string();
        crate::state::replace_state(state);
        assert!(matches!(sol_cluster(), SolanaCluster::Devnet));

        let mut state = crate::state::State::default();
        state.sol_schnorr_key_name = SOL_PRODUCTION_SCHNORR_KEY_NAME.to_string();
        crate::state::replace_state(state);
        assert!(matches!(sol_cluster(), SolanaCluster::Mainnet));
    }
}
