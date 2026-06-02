use super::config::*;
use crate::chains::config::ChainId;

#[test]
fn solana_chain_id_is_slip44() {
    assert_eq!(SOLANA_CHAIN_ID, ChainId(501));
}

#[test]
fn solana_decimals_are_correct() {
    assert_eq!(SOL_NATIVE_DECIMALS, 9);
    assert_eq!(SOLANA_ICUSD_DECIMALS, 8);
}

#[test]
fn default_register_arg_matches_constants() {
    let arg = solana_default_register_arg();
    assert_eq!(arg.chain_id, SOLANA_CHAIN_ID);
    assert_eq!(arg.chain_native_decimals, SOL_NATIVE_DECIMALS);
    assert!(arg.rpc_endpoints.is_empty());
}

#[test]
fn key_name_is_test_key_1() {
    assert_eq!(solana_schnorr_key_name(), "test_key_1");
}
