use super::config::{
    monad_default_register_arg, monad_ecdsa_key_name, MONAD_CHAIN_ID,
    MONAD_ICUSD_DECIMALS,
};
use crate::chains::config::{ChainId, GasStrategy};

#[test]
fn chain_id_is_monad_testnet() {
    assert_eq!(MONAD_CHAIN_ID, ChainId(10143));
}

#[test]
fn default_register_arg_uses_eip1559_and_nonempty_rpc() {
    let arg = monad_default_register_arg();
    assert_eq!(arg.chain_id, ChainId(10143));
    assert!(!arg.rpc_endpoints.is_empty());
    assert!(matches!(arg.gas_strategy, GasStrategy::EvmEip1559 { .. }));
    assert_eq!(arg.chain_native_decimals, 18); // MON has 18 decimals
    assert!(arg.finality_depth >= 1);
}

#[test]
fn icusd_decimals_match_e8s() {
    // IcUSD.sol uses 8 decimals so on-chain amount == e8s 1:1.
    assert_eq!(MONAD_ICUSD_DECIMALS, 8);
}

#[test]
fn key_name_is_test_key_on_staging_default() {
    assert_eq!(monad_ecdsa_key_name(), "test_key_1");
}
