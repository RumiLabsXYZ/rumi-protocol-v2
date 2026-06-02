//! Task F3: focused tests for the GENERALIZED `chains::vault` seam.
//!
//! These prove the two parameters hoisted out of the Monad-hardcoded helper work
//! independently of Monad: `address_validator: fn(&str) -> bool` and
//! `price_symbol: &str`. They call `chains::vault::open_chain_vault_in_state`
//! DIRECTLY (not via the Monad wrapper) with a NON-`"MON"` price symbol (`"SOL"`)
//! and an injected validator, documenting the seam that Task 6 (Solana) drives.
//!
//! The exhaustive open/verify/withdraw/close behavior is already covered by the
//! Monad suites (`monad::tests_open_vault`, `monad::tests_withdraw`,
//! `monad::tests_chain_vault`), which exercise the SAME code through the wrappers;
//! this file only asserts the parameterization itself.

use super::config::{ChainId, GasStrategy, RegisterChainArg};
use super::multi_chain_state::MultiChainStateV3;
use super::vault::{open_chain_vault_in_state, OpenVaultError};
use candid::Principal;

/// A non-Monad chain id (Solana's SLIP-44 coin type) so nothing here can lean on
/// Monad's 10143.
const CHAIN: ChainId = ChainId(501);
/// $150.00 as e8 (a plausible SOL price).
const PRICE_150_USD_E8: u64 = 150_0000_0000;
/// 1 SOL in lamports (9 decimals).
const ONE_SOL: u128 = 1_000_000_000;

/// Validator stub that accepts ONLY the literal "good-address". Lets a test prove
/// the injected `address_validator` is the gate (independent of any EVM/Solana
/// format rules).
fn only_good(addr: &str) -> bool {
    addr == "good-address"
}

/// Register chain 501 with 9-decimal native units and set its manual `"SOL"`
/// price. Mirrors what register_chain + a manual price override do.
fn setup(price_e8: u64) -> MultiChainStateV3 {
    let mut s = MultiChainStateV3::default();
    let arg = RegisterChainArg {
        chain_id: CHAIN,
        display_name: "SolanaDevnet".into(),
        // register_chain_in_state requires at least one URL; the value is
        // irrelevant to these pure-state tests (no RPC is ever made).
        rpc_endpoints: vec!["https://rpc".into()],
        finality_depth: 0,
        gas_strategy: GasStrategy::SolanaPriorityFee {
            lamports_per_cu_ceiling: 10_000,
        },
        chain_native_decimals: 9,
    };
    crate::chains::admin::register_chain_in_state(&mut s, arg, 0).expect("register chain");
    s.manual_prices.insert((CHAIN, "SOL".into()), price_e8);
    s
}

// 1. A vault opens through the generalized helper when the injected validator
//    accepts the recipient AND the (chain, "SOL") manual price is set. Proves the
//    price_symbol seam reads a NON-"MON" key and the validator seam is honored.
#[test]
fn open_succeeds_with_injected_validator_and_sol_price_symbol() {
    let mut s = setup(PRICE_150_USD_E8);
    // 100 SOL ($15_000) collateral, 100 icUSD ($100) debt -> CR way above 130%.
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        Principal::anonymous(),
        "custody".into(), // custody is never validated
        100 * ONE_SOL,
        100_00000000, // 100 icUSD intended mint
        "good-address".into(),
        only_good, // injected validator accepts "good-address"
        "SOL",     // NON-"MON" price symbol
        13_000,
        12345,
        7,
    );
    assert!(res.is_ok(), "open should succeed via the generalized seam: {res:?}");
    let v = s.chain_vaults.get(&7).expect("vault 7 created");
    assert_eq!(v.collateral_amount_native, 100 * ONE_SOL);
    assert_eq!(v.pending_mint_e8s, 100_00000000);
    assert_eq!(v.mint_recipient, "good-address");
    // open enqueues nothing (open-then-verify).
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0);
}

// 2. The SAME inputs but a recipient the injected validator REJECTS -> the
//    generalized helper returns InvalidAddress and creates nothing. Proves the
//    address_validator parameter (not a hardcoded EVM check) is the gate.
#[test]
fn open_rejected_when_injected_validator_rejects() {
    let mut s = setup(PRICE_150_USD_E8);
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        Principal::anonymous(),
        "custody".into(),
        100 * ONE_SOL,
        100_00000000,
        "bad-address".into(), // validator rejects anything != "good-address"
        only_good,
        "SOL",
        13_000,
        12345,
        7,
    );
    assert!(matches!(res, Err(OpenVaultError::InvalidAddress(_))), "got {res:?}");
    assert!(s.chain_vaults.is_empty(), "no vault on a rejected address");
    assert_eq!(s.settlement_queues[&CHAIN].pending_len(), 0);
}

// 3. With the validator accepting but the price key looked up under a DIFFERENT
//    symbol than the one set, the helper returns NoPrice. Proves price_symbol is
//    really the lookup key: state has (chain,"SOL") but we ask for "WSOL".
#[test]
fn open_no_price_when_symbol_key_absent() {
    let mut s = setup(PRICE_150_USD_E8); // sets (chain, "SOL")
    let res = open_chain_vault_in_state(
        &mut s,
        CHAIN,
        Principal::anonymous(),
        "custody".into(),
        100 * ONE_SOL,
        100_00000000,
        "good-address".into(),
        only_good,
        "WSOL", // no manual price under this symbol
        13_000,
        12345,
        7,
    );
    assert!(matches!(res, Err(OpenVaultError::NoPrice)), "got {res:?}");
    assert!(s.chain_vaults.is_empty());
}
