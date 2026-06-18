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
use super::multi_chain_state::MultiChainStateV4;
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
fn setup(price_e8: u64) -> MultiChainStateV4 {
    let mut s = MultiChainStateV4::default();
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
        min_quorum_providers: None,
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

// ─── M2: borrow + nonce + per-owner cap ───────────────────────────────────────

use super::vault::{borrow_chain_vault_in_state, BorrowError};
use super::evm::settlement::confirm_mint_in_state;

/// Insert an Open vault (confirmed collateral + debt, no mint in flight) owned by
/// `owner`, and seed the chain supply to match its debt so the invariant holds.
fn insert_open_vault(s: &mut MultiChainStateV4, owner: Principal, vault_id: u64, collateral: u128, debt: u128) {
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner, collateral_chain: CHAIN, custody_address: "custody".into(),
        collateral_amount_native: collateral, debt_e8s: debt, mint_recipient: "good-address".into(),
        pending_mint_e8s: 0, status: ChainVaultStatus::Open, opened_at_ns: 0,
        owner_evm: Some("0xowner".into()),
    });
    *s.chain_supplies.entry(CHAIN).or_default() += debt;
}

#[test]
fn borrow_open_vault_enqueues_mint_and_reserves_pending() {
    let mut s = setup(PRICE_150_USD_E8);
    // 100 SOL ($15000) collateral, 100 icUSD debt; borrow 50 more → CR fine.
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 1);
    assert_eq!(r, Ok(()));
    let v = s.chain_vaults.get(&7).unwrap();
    assert_eq!(v.pending_mint_e8s, 50_00000000, "borrow reserves the additional as pending");
    assert_eq!(v.debt_e8s, 100_00000000, "confirmed debt unchanged until mint observed");
    assert_eq!(s.settlement_queues.get(&CHAIN).unwrap().pending_len(), 1, "one Mint op enqueued");
}

#[test]
fn borrow_then_confirm_preserves_supply_invariant() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 1).unwrap();
    let pre = s.total_chain_vault_debt_e8s();
    confirm_mint_in_state(&mut s, CHAIN, 7, 50_00000000, pre).expect("confirm");
    let v = s.chain_vaults.get(&7).unwrap();
    assert_eq!(v.debt_e8s, 150_00000000, "pending moved into confirmed debt");
    assert_eq!(v.pending_mint_e8s, 0);
    assert_eq!(s.chain_supplies.get(&CHAIN).copied().unwrap(), 150_00000000, "supply == total debt");
    assert_eq!(s.chain_supplies.get(&CHAIN).copied().unwrap(), s.total_chain_vault_debt_e8s());
}

#[test]
fn borrow_rejects_when_mint_in_flight() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    s.chain_vaults.get_mut(&7).unwrap().pending_mint_e8s = 1; // a mint already pending
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 1);
    assert_eq!(r, Err(BorrowError::MintInFlight));
}

#[test]
fn borrow_rejects_non_open_vault() {
    use super::monad::chain_vault::ChainVaultStatus;
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    s.chain_vaults.get_mut(&7).unwrap().status = ChainVaultStatus::AwaitingDeposit;
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 1);
    assert_eq!(r, Err(BorrowError::WrongStatus { status: ChainVaultStatus::AwaitingDeposit }));
}

#[test]
fn borrow_rejects_below_min_cr() {
    let mut s = setup(PRICE_150_USD_E8);
    // 1 SOL ($150) collateral, 100 icUSD debt (CR 150%); borrow 50 more → new debt
    // 150 icUSD, CR = 150/150 = 100% < 130% → reject.
    insert_open_vault(&mut s, Principal::anonymous(), 7, ONE_SOL, 100_00000000);
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 1);
    assert!(matches!(r, Err(BorrowError::BelowMinCr { .. })), "got {r:?}");
    assert_eq!(s.chain_vaults.get(&7).unwrap().pending_mint_e8s, 0, "no mutation on reject");
}

#[test]
fn borrow_rejects_zero_debt_and_bad_recipient() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    assert_eq!(borrow_chain_vault_in_state(&mut s, 7, 0, "good-address".into(), only_good, "SOL", 13_000, 1), Err(BorrowError::ZeroDebt));
    assert_eq!(borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "bad".into(), only_good, "SOL", 13_000, 1), Err(BorrowError::InvalidAddress("bad".into())));
}

#[test]
fn nonce_consume_is_monotonic_and_rejects_replay() {
    let mut s = MultiChainStateV4::default();
    let owner = Principal::from_slice(&[5, 5, 5]);
    assert_eq!(s.expected_evm_nonce(&owner), 0);
    assert_eq!(s.consume_evm_nonce(&owner, 0), Ok(()));
    assert_eq!(s.expected_evm_nonce(&owner), 1);
    assert_eq!(s.consume_evm_nonce(&owner, 0), Err(1), "replay of nonce 0 rejected");
    assert_eq!(s.consume_evm_nonce(&owner, 5), Err(1), "out-of-order rejected");
    assert_eq!(s.consume_evm_nonce(&owner, 1), Ok(()));
    // A different owner has an independent sequence.
    let other = Principal::from_slice(&[6, 6, 6]);
    assert_eq!(s.consume_evm_nonce(&other, 0), Ok(()));
}

#[test]
fn per_owner_cap_counts_non_terminal_only() {
    use super::monad::chain_vault::ChainVaultStatus;
    let mut s = MultiChainStateV4::default();
    let owner = Principal::from_slice(&[9, 9, 9]);
    insert_open_vault(&mut s, owner, 1, 0, 0);
    insert_open_vault(&mut s, owner, 2, 0, 0);
    s.chain_vaults.get_mut(&2).unwrap().status = ChainVaultStatus::AwaitingDeposit;
    insert_open_vault(&mut s, owner, 3, 0, 0);
    s.chain_vaults.get_mut(&3).unwrap().status = ChainVaultStatus::Closed; // terminal, not counted
    // A different owner's vault is not counted.
    insert_open_vault(&mut s, Principal::anonymous(), 4, 0, 0);
    assert_eq!(s.count_owner_active_vaults(&owner), 2);
}
