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
use super::multi_chain_state::MultiChainState;
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
fn setup(price_e8: u64) -> MultiChainState {
    let mut s = MultiChainState::default();
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
        0,
        None,
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
        0,
        None,
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
        0,
        None,
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
fn insert_open_vault(s: &mut MultiChainState, owner: Principal, vault_id: u64, collateral: u128, debt: u128) {
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner, collateral_chain: CHAIN, custody_address: "custody".into(),
        collateral_amount_native: collateral, debt_e8s: debt, mint_recipient: "good-address".into(),
        pending_mint_e8s: 0, status: ChainVaultStatus::Open, opened_at_ns: 0,
    last_interest_accrual_ns: 0,
    pending_interest_mint_e8s: 0,
    pending_liquidation: None,        owner_evm: Some("0xowner".into()),
    });
    *s.chain_supplies.entry(CHAIN).or_default() += debt;
}

#[test]
fn borrow_open_vault_enqueues_mint_and_reserves_pending() {
    let mut s = setup(PRICE_150_USD_E8);
    // 100 SOL ($15000) collateral, 100 icUSD debt; borrow 50 more → CR fine.
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1);
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
    borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1).unwrap();
    let pre = s.total_chain_vault_debt_e8s();
    confirm_mint_in_state(&mut s, CHAIN, 7, 50_00000000, pre, 1).expect("confirm");
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
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1);
    assert_eq!(r, Err(BorrowError::MintInFlight));
}

#[test]
fn borrow_rejects_non_open_vault() {
    use super::monad::chain_vault::ChainVaultStatus;
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    s.chain_vaults.get_mut(&7).unwrap().status = ChainVaultStatus::AwaitingDeposit;
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1);
    assert_eq!(r, Err(BorrowError::WrongStatus { status: ChainVaultStatus::AwaitingDeposit }));
}

#[test]
fn borrow_rejects_below_min_cr() {
    let mut s = setup(PRICE_150_USD_E8);
    // 1 SOL ($150) collateral, 100 icUSD debt (CR 150%); borrow 50 more → new debt
    // 150 icUSD, CR = 150/150 = 100% < 130% → reject.
    insert_open_vault(&mut s, Principal::anonymous(), 7, ONE_SOL, 100_00000000);
    let r = borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1);
    assert!(matches!(r, Err(BorrowError::BelowMinCr { .. })), "got {r:?}");
    assert_eq!(s.chain_vaults.get(&7).unwrap().pending_mint_e8s, 0, "no mutation on reject");
}

#[test]
fn borrow_rejects_zero_debt_and_bad_recipient() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    assert_eq!(borrow_chain_vault_in_state(&mut s, 7, 0, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1), Err(BorrowError::ZeroDebt));
    assert_eq!(borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "bad".into(), only_good, "SOL", 13_000, 0, None, 1), Err(BorrowError::InvalidAddress("bad".into())));
}

// ─── Increment 0: min-debt floor + per-chain debt ceiling ─────────────────────

#[test]
fn open_rejects_debt_below_min_vault_debt() {
    let mut s = setup(PRICE_150_USD_E8);
    // 1 SOL ($150) collateral easily clears CR; debt 0.05 icUSD < the 0.1 floor.
    let r = open_chain_vault_in_state(
        &mut s, CHAIN, Principal::anonymous(), "custody".into(), ONE_SOL,
        5_000_000, // 0.05 icUSD
        "good-address".into(), only_good, "SOL",
        13_000, /*min_vault_debt*/ 10_000_000, /*ceiling*/ None, 12345, 1,
    );
    assert!(matches!(r, Err(OpenVaultError::BelowMinDebt { min_e8s: 10_000_000, .. })), "got {r:?}");
    assert!(s.chain_vaults.is_empty(), "no mutation on rejection");
}

#[test]
fn open_rejects_when_over_debt_ceiling() {
    let mut s = setup(PRICE_150_USD_E8);
    // Seed 900 icUSD of existing chain debt, then open another 200 -> 1100 > 1000.
    insert_open_vault(&mut s, Principal::anonymous(), 1, 100 * ONE_SOL, 900_00000000);
    let r = open_chain_vault_in_state(
        &mut s, CHAIN, Principal::anonymous(), "custody".into(), 100 * ONE_SOL,
        200_00000000, "good-address".into(), only_good, "SOL",
        13_000, 10_000_000, Some(1000_00000000), 12345, 2,
    );
    assert!(
        matches!(r, Err(OpenVaultError::DebtCeilingExceeded { would_be_e8s, ceiling_e8s })
            if would_be_e8s == 1100_00000000 && ceiling_e8s == 1000_00000000),
        "got {r:?}"
    );
    assert!(!s.chain_vaults.contains_key(&2), "no vault created over the ceiling");
}

#[test]
fn open_allows_debt_exactly_at_ceiling() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 1, 100 * ONE_SOL, 800_00000000);
    // 800 + 200 == 1000 == ceiling -> allowed (only strictly over rejects).
    let r = open_chain_vault_in_state(
        &mut s, CHAIN, Principal::anonymous(), "custody".into(), 100 * ONE_SOL,
        200_00000000, "good-address".into(), only_good, "SOL",
        13_000, 10_000_000, Some(1000_00000000), 12345, 2,
    );
    assert_eq!(r, Ok(()), "open at exactly the ceiling is allowed");
}

#[test]
fn borrow_rejects_when_over_debt_ceiling() {
    let mut s = setup(PRICE_150_USD_E8);
    // Vault 7 holds 900 icUSD; borrowing 200 more pushes the chain total to 1100 > 1000.
    insert_open_vault(&mut s, Principal::anonymous(), 7, 1000 * ONE_SOL, 900_00000000);
    let r = borrow_chain_vault_in_state(
        &mut s, 7, 200_00000000, "good-address".into(), only_good, "SOL",
        13_000, 10_000_000, Some(1000_00000000), 1,
    );
    assert!(
        matches!(r, Err(BorrowError::DebtCeilingExceeded { ceiling_e8s, .. }) if ceiling_e8s == 1000_00000000),
        "got {r:?}"
    );
    assert_eq!(s.chain_vaults.get(&7).unwrap().pending_mint_e8s, 0, "no mutation on reject");
}

#[test]
fn nonce_consume_is_monotonic_and_rejects_replay() {
    let mut s = MultiChainState::default();
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
    let mut s = MultiChainState::default();
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

// ─── M2: stale AwaitingDeposit GC ─────────────────────────────────────────────

use super::vault::{prune_stale_awaiting_deposit, AWAITING_DEPOSIT_TTL_NS};

#[test]
fn gc_prunes_only_stale_awaiting_deposit() {
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    // setup() registers CHAIN (status Registered), so its observer is "active"
    // and the GC is allowed to reap stale unfunded vaults on it (finding F).
    let mut s = setup(PRICE_150_USD_E8);
    let mk = |id: u64, st: ChainVaultStatus, opened: u64| ChainVaultV1 {
        vault_id: id, owner: Principal::anonymous(), collateral_chain: CHAIN,
        custody_address: "c".into(), collateral_amount_native: 0, debt_e8s: 0,
        mint_recipient: "r".into(), pending_mint_e8s: 0, status: st, opened_at_ns: opened,
    last_interest_accrual_ns: 0,
    pending_interest_mint_e8s: 0,
    pending_liquidation: None,        owner_evm: None,
    };
    let now = 100 * AWAITING_DEPOSIT_TTL_NS;
    // 1: stale AwaitingDeposit (older than TTL) -> pruned.
    s.chain_vaults.insert(1, mk(1, ChainVaultStatus::AwaitingDeposit, now - AWAITING_DEPOSIT_TTL_NS - 1));
    // 2: young AwaitingDeposit (within TTL) -> kept.
    s.chain_vaults.insert(2, mk(2, ChainVaultStatus::AwaitingDeposit, now - 1));
    // 3: old Open vault -> kept (only AwaitingDeposit is GC'd; a funded vault is safe).
    s.chain_vaults.insert(3, mk(3, ChainVaultStatus::Open, 0));
    // 4: old MintPending -> kept (a mint is in flight; not unfunded).
    s.chain_vaults.insert(4, mk(4, ChainVaultStatus::MintPending, 0));

    let pruned = prune_stale_awaiting_deposit(&mut s, now, AWAITING_DEPOSIT_TTL_NS);
    assert_eq!(pruned, 1);
    assert!(!s.chain_vaults.contains_key(&1), "stale AwaitingDeposit pruned");
    assert!(s.chain_vaults.contains_key(&2), "young AwaitingDeposit kept");
    assert!(s.chain_vaults.contains_key(&3), "Open kept");
    assert!(s.chain_vaults.contains_key(&4), "MintPending kept");
}

#[test]
fn gc_skips_stale_vaults_when_observer_inactive() {
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    let now = 100 * AWAITING_DEPOSIT_TTL_NS;
    let stale = |id: u64| ChainVaultV1 {
        vault_id: id, owner: Principal::anonymous(), collateral_chain: CHAIN,
        custody_address: "c".into(), collateral_amount_native: 0, debt_e8s: 0,
        mint_recipient: "r".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::AwaitingDeposit,
        opened_at_ns: now - AWAITING_DEPOSIT_TTL_NS - 1, owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
        pending_liquidation: None,    };
    // (a) Chain not registered at all -> no observer -> not reaped (would strand
    //     a funded-but-unobserved deposit).
    let mut s = MultiChainState::default();
    s.chain_vaults.insert(1, stale(1));
    assert_eq!(prune_stale_awaiting_deposit(&mut s, now, AWAITING_DEPOSIT_TTL_NS), 0);
    assert!(s.chain_vaults.contains_key(&1), "unregistered-chain vault kept");
    // (b) Chain registered but reorg-halted -> observer halted -> not reaped.
    let mut s = setup(PRICE_150_USD_E8);
    s.reorg_halted.insert(CHAIN, true);
    s.chain_vaults.insert(1, stale(1));
    assert_eq!(prune_stale_awaiting_deposit(&mut s, now, AWAITING_DEPOSIT_TTL_NS), 0);
    assert!(s.chain_vaults.contains_key(&1), "reorg-halted-chain vault kept");
}

// ─── M2 review finding A: collateral release blocked while a borrow mint pends ─

use super::vault::{close_chain_vault_in_state, withdraw_collateral_in_state, WithdrawError};

#[test]
fn withdraw_and_close_reject_while_borrow_mint_in_flight() {
    let mut s = setup(PRICE_150_USD_E8);
    // Open vault, 100 SOL collateral, debt 100e8; borrow 50 more (pending=50e8).
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1).unwrap();
    assert_eq!(s.chain_vaults.get(&7).unwrap().pending_mint_e8s, 50_00000000);
    // Withdraw must reject — releasing collateral now would undercollateralize
    // once the borrow mint confirms (debt would jump 100e8 -> 150e8).
    assert_eq!(
        withdraw_collateral_in_state(&mut s, 7, 1, "good-address".into(), only_good, "SOL", 13_000, 2),
        Err(WithdrawError::MintInFlight)
    );
    assert_eq!(s.settlement_queues.get(&CHAIN).unwrap().pending_len(), 1, "no withdraw enqueued");
}

#[test]
fn close_rejects_while_borrow_mint_in_flight_even_if_debt_zero() {
    let mut s = setup(PRICE_150_USD_E8);
    // A repaid (debt 0) but still-Open vault that borrows: debt stays 0, pending=50e8.
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 0);
    borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 1).unwrap();
    // Close (debt==0) must NOT release collateral while the borrow mint pends.
    assert_eq!(
        close_chain_vault_in_state(&mut s, 7, "good-address".into(), only_good, "SOL", 13_000, 2),
        Err(WithdrawError::MintInFlight)
    );
    assert_eq!(
        s.chain_vaults.get(&7).unwrap().status,
        super::monad::chain_vault::ChainVaultStatus::Open,
        "vault not closed"
    );
}

// ─── Increment 1 / Task 2: pending_liquidation marker write-guards (spec 3.1) ───

use super::vault::{LiquidationTier, PendingLiquidationV1};

/// A Bot-tier liquidation marker for the owner-write-guard tests.
fn liq_marker() -> PendingLiquidationV1 {
    PendingLiquidationV1 {
        op_id: 1,
        debt_to_clear_e8s: 10_00000000,
        collateral_reserved_native: ONE_SOL,
        tier: LiquidationTier::Bot,
        started_at_ns: 100,
    }
}

#[test]
fn withdraw_rejected_while_liquidation_in_flight() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    // A liquidation is mid-flight: the vault's collateral is reserved/handed to a
    // tier. An owner withdraw (even a tiny, CR-safe 1-lamport one) must be rejected
    // so the collateral cannot be double-spent against an in-flight swap/burn.
    s.chain_vaults.get_mut(&7).unwrap().pending_liquidation = Some(liq_marker());
    assert_eq!(
        withdraw_collateral_in_state(&mut s, 7, 1, "good-address".into(), only_good, "SOL", 13_000, 2),
        Err(WithdrawError::LiquidationInFlight)
    );
    assert_eq!(
        s.chain_vaults.get(&7).unwrap().collateral_amount_native,
        100 * ONE_SOL,
        "collateral untouched on rejection"
    );
    assert_eq!(s.settlement_queues.get(&CHAIN).unwrap().pending_len(), 0, "no withdraw enqueued");
}

#[test]
fn borrow_rejected_while_liquidation_in_flight() {
    let mut s = setup(PRICE_150_USD_E8);
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 100_00000000);
    s.chain_vaults.get_mut(&7).unwrap().pending_liquidation = Some(liq_marker());
    assert_eq!(
        borrow_chain_vault_in_state(&mut s, 7, 50_00000000, "good-address".into(), only_good, "SOL", 13_000, 0, None, 2),
        Err(BorrowError::LiquidationInFlight)
    );
    assert_eq!(s.chain_vaults.get(&7).unwrap().pending_mint_e8s, 0, "no borrow reserved on rejection");
    assert_eq!(s.settlement_queues.get(&CHAIN).unwrap().pending_len(), 0, "no mint enqueued");
}

#[test]
fn close_rejected_while_liquidation_in_flight_via_withdraw_delegate() {
    let mut s = setup(PRICE_150_USD_E8);
    // debt 0 but collateral > 0: close delegates to withdraw, which must reject on
    // the marker (the non-degenerate path).
    insert_open_vault(&mut s, Principal::anonymous(), 7, 100 * ONE_SOL, 0);
    s.chain_vaults.get_mut(&7).unwrap().pending_liquidation = Some(liq_marker());
    assert_eq!(
        close_chain_vault_in_state(&mut s, 7, "good-address".into(), only_good, "SOL", 13_000, 2),
        Err(WithdrawError::LiquidationInFlight)
    );
    assert_eq!(
        s.chain_vaults.get(&7).unwrap().status,
        super::monad::chain_vault::ChainVaultStatus::Open,
        "vault not closed while liquidation in flight"
    );
}

#[test]
fn close_rejected_while_liquidation_in_flight_degenerate_zero_collateral() {
    let mut s = setup(PRICE_150_USD_E8);
    // debt 0, collateral 0, Open: close normally short-circuits straight to Closed.
    // The marker must block even that degenerate path — the short-circuit bypasses
    // the withdraw delegate, so the guard must live in close's OWN block.
    insert_open_vault(&mut s, Principal::anonymous(), 7, 0, 0);
    s.chain_vaults.get_mut(&7).unwrap().pending_liquidation = Some(liq_marker());
    assert_eq!(
        close_chain_vault_in_state(&mut s, 7, "good-address".into(), only_good, "SOL", 13_000, 2),
        Err(WithdrawError::LiquidationInFlight)
    );
    assert_eq!(
        s.chain_vaults.get(&7).unwrap().status,
        super::monad::chain_vault::ChainVaultStatus::Open,
        "vault not closed while liquidation in flight"
    );
}
