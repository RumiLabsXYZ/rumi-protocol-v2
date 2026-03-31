// Flaky Ledger — a minimal ICRC-1/ICRC-2 token canister with configurable failure injection.
//
// Supports just enough of the ICRC spec for the Rumi AMM test suite:
//   - icrc1_transfer
//   - icrc2_approve
//   - icrc2_transfer_from
//   - icrc1_balance_of
//
// Control methods (test-only):
//   - set_fail_transfers(bool)    — make all icrc1_transfer calls fail
//   - set_fail_transfer_from(bool) — make all icrc2_transfer_from calls fail
//   - mint(Account, Nat)          — mint tokens to an account (no auth check)

use candid::{CandidType, Nat, Principal};
use ic_cdk::{init, query, update};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;

// ─── Types matching ICRC-1/ICRC-2 ───

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct TransferArg {
    pub from_subaccount: Option<[u8; 32]>,
    pub to: Account,
    pub amount: Nat,
    pub fee: Option<Nat>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Serialize)]
pub enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct TransferFromArgs {
    pub spender_subaccount: Option<[u8; 32]>,
    pub from: Account,
    pub to: Account,
    pub amount: Nat,
    pub fee: Option<Nat>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Serialize)]
pub enum TransferFromError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    InsufficientAllowance { allowance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct ApproveArgs {
    pub from_subaccount: Option<[u8; 32]>,
    pub spender: Account,
    pub amount: Nat,
    pub expected_allowance: Option<Nat>,
    pub expires_at: Option<u64>,
    pub fee: Option<Nat>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Serialize)]
pub enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

// ─── State ───

#[derive(Default)]
struct LedgerState {
    balances: BTreeMap<Account, u128>,
    // allowances: (from, spender) -> amount
    allowances: BTreeMap<(Account, Account), u128>,
    block_index: u64,
    // Failure injection flags
    fail_transfers: bool,
    fail_transfer_from: bool,
}

thread_local! {
    static STATE: RefCell<LedgerState> = RefCell::new(LedgerState::default());
}

fn nat_to_u128(n: &Nat) -> u128 {
    n.0.clone().try_into().unwrap_or(0)
}

fn account_key(owner: Principal, subaccount: Option<[u8; 32]>) -> Account {
    Account { owner, subaccount }
}

// ─── Init ───

#[init]
fn init() {}

// ─── ICRC-1 ───

#[query]
fn icrc1_balance_of(account: Account) -> Nat {
    STATE.with(|s| {
        let state = s.borrow();
        Nat::from(state.balances.get(&account).copied().unwrap_or(0))
    })
}

#[update]
fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
    STATE.with(|s| {
        let mut state = s.borrow_mut();

        // Check failure injection
        if state.fail_transfers {
            return Err(TransferError::GenericError {
                error_code: Nat::from(999u64),
                message: "Injected failure: transfers disabled".to_string(),
            });
        }

        let caller = ic_cdk::caller();
        let from = account_key(caller, args.from_subaccount);
        let balance = state.balances.get(&from).copied().unwrap_or(0);
        let amount = nat_to_u128(&args.amount);

        if amount > balance {
            return Err(TransferError::InsufficientFunds {
                balance: Nat::from(balance),
            });
        }

        // Debit from
        *state.balances.entry(from).or_insert(0) -= amount;

        // Credit to
        *state.balances.entry(args.to).or_insert(0) += amount;

        state.block_index += 1;
        Ok(Nat::from(state.block_index))
    })
}

// ─── ICRC-2 ───

#[update]
fn icrc2_approve(args: ApproveArgs) -> Result<Nat, ApproveError> {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let caller = ic_cdk::caller();
        let from = account_key(caller, args.from_subaccount);
        let spender = args.spender;
        let amount = nat_to_u128(&args.amount);

        state.allowances.insert((from, spender), amount);

        state.block_index += 1;
        Ok(Nat::from(state.block_index))
    })
}

#[update]
fn icrc2_transfer_from(args: TransferFromArgs) -> Result<Nat, TransferFromError> {
    STATE.with(|s| {
        let mut state = s.borrow_mut();

        // Check failure injection
        if state.fail_transfer_from {
            return Err(TransferFromError::GenericError {
                error_code: Nat::from(999u64),
                message: "Injected failure: transfer_from disabled".to_string(),
            });
        }

        let spender = ic_cdk::caller();
        let spender_account = account_key(spender, args.spender_subaccount);
        let from = args.from.clone();
        let amount = nat_to_u128(&args.amount);

        // Check allowance
        let allowance = state.allowances.get(&(from.clone(), spender_account)).copied().unwrap_or(0);
        if amount > allowance {
            return Err(TransferFromError::InsufficientAllowance {
                allowance: Nat::from(allowance),
            });
        }

        // Check balance
        let balance = state.balances.get(&from).copied().unwrap_or(0);
        if amount > balance {
            return Err(TransferFromError::InsufficientFunds {
                balance: Nat::from(balance),
            });
        }

        // Deduct allowance
        if let Some(a) = state.allowances.get_mut(&(from.clone(), account_key(spender, args.spender_subaccount))) {
            *a -= amount;
        }

        // Debit from
        *state.balances.entry(from).or_insert(0) -= amount;

        // Credit to
        *state.balances.entry(args.to).or_insert(0) += amount;

        state.block_index += 1;
        Ok(Nat::from(state.block_index))
    })
}

// ─── Test Control Methods ───

/// Mint tokens to any account (no auth — test only).
#[update]
fn mint(account: Account, amount: Nat) {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let amt = nat_to_u128(&amount);
        *state.balances.entry(account).or_insert(0) += amt;
    });
}

/// When true, all icrc1_transfer calls will fail.
#[update]
fn set_fail_transfers(fail: bool) {
    STATE.with(|s| {
        s.borrow_mut().fail_transfers = fail;
    });
}

/// When true, all icrc2_transfer_from calls will fail.
#[update]
fn set_fail_transfer_from(fail: bool) {
    STATE.with(|s| {
        s.borrow_mut().fail_transfer_from = fail;
    });
}
