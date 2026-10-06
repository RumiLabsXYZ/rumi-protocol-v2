// Flaky Ledger — a minimal ICRC-1/ICRC-2 token canister with configurable failure injection.
//
// Supports just enough of the ICRC spec for the Rumi test suite plus the
// audit_pocs Wave-3 regression fences (ICRC-001/002/003/004/005):
//   - icrc1_transfer (with dedup based on created_at_time)
//   - icrc2_approve
//   - icrc2_transfer_from (with dedup)
//   - icrc1_balance_of
//   - icrc1_fee
//
// Control methods (test-only):
//   - set_fail_transfers(bool)        all icrc1_transfer calls return GenericError
//   - set_fail_transfer_from(bool)    all icrc2_transfer_from calls return GenericError
//   - set_fee(Nat)                    update the ledger fee returned by icrc1_fee
//   - set_phantom_failures(u32)       next N transfers commit but return GenericError
//                                     (simulates "ledger committed, reply lost")
//   - set_bad_fee_failures(u32)       next N transfers return BadFee with set_fee value
//   - mint(Account, Nat)              mint tokens to any account (no auth)
//   - reset_dedup()                   clear the dedup map (for explicit test isolation)
//
// Dedup behaviour matches ICRC-1: if a transfer with identical
// (caller, from_subaccount, to, amount, fee, memo, created_at_time) lands
// twice while still in the dedup window (no time advancement here), the
// second call returns Duplicate { duplicate_of }.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
use icrc_ledger_types::icrc3::blocks::{BlockWithId, GetBlocksRequest, GetBlocksResult};
use icrc_ledger_types::icrc1::account::Account as IcrcAccount;
use num_traits::ToPrimitive;
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

/// The deduplication tuple used by ICRC-1/2 ledgers. Two calls with identical
/// tuples within the dedup window collapse into a single block; the second
/// returns Duplicate { duplicate_of }.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DedupKey {
    caller: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    fee: Option<u128>,
    memo: Option<Vec<u8>>,
    created_at_time: u64,
}

#[derive(Default)]
struct LedgerState {
    balances: BTreeMap<Account, u128>,
    allowances: BTreeMap<(Account, Account), u128>,
    block_index: u64,
    blocks: Vec<ICRC3Value>,
    minting_account: Option<Account>,
    fee: u128,
    fail_transfers: bool,
    fail_transfer_from: bool,
    /// Next N transfers commit but return a transient error (simulates lost reply).
    phantom_failures_remaining: u32,
    /// Next matching caller's committed transfer traps after an inter-canister await.
    trap_after_commit_for_caller: Option<Principal>,
    /// Next N transfers return TooOld before changing balances.
    too_old_failures_remaining: u32,
    /// Next N transfers return BadFee with the current fee value.
    bad_fee_failures_remaining: u32,
    /// Recent transfers keyed by their dedup tuple. Retained until reset_dedup().
    dedup: BTreeMap<DedupKey, u64>,
    /// When set, icrc1_transfer rejects with GenericError if the caller
    /// matches. Used by audit_pocs_bot_002 to fail the bot's outbound
    /// return-collateral transfer without breaking the protocol's bot_claim
    /// transfer (the protocol calls icrc1_transfer with itself as caller).
    fail_transfers_for_caller: Option<Principal>,
    /// When set, icrc1_balance_of returns 0 for the matching account owner.
    /// Used by audit_pocs_bot_002 to force the protocol's BOT-001b cancel
    /// gate to reject (the gate compares icrc1_balance_of(protocol) against
    /// `claim.collateral_amount - fee`, and a 0 reading deterministically
    /// fails the >= check) without having to engineer a specific
    /// post-claim-and-return on-ledger balance.
    fake_zero_balance_for: Option<Principal>,
}

thread_local! {
    static STATE: RefCell<LedgerState> = RefCell::new(LedgerState::default());
}

fn icrc_account(owner: Principal, subaccount: Option<[u8; 32]>) -> IcrcAccount {
    IcrcAccount { owner, subaccount }
}

fn account_value(account: &IcrcAccount) -> ICRC3Value {
    let mut parts = vec![ICRC3Value::Blob(account.owner.as_slice().to_vec().into())];
    if let Some(subaccount) = account.subaccount {
        parts.push(ICRC3Value::Blob(subaccount.to_vec().into()));
    }
    ICRC3Value::Array(parts)
}

fn push_block(
    state: &mut LedgerState,
    btype: &str,
    op: &str,
    from: Option<IcrcAccount>,
    to: Option<IcrcAccount>,
    amount: u128,
    fee: Option<u128>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
    spender: Option<IcrcAccount>,
) {
    let mut tx: ICRC3Map = BTreeMap::new();
    tx.insert("op".into(), ICRC3Value::Text(op.into()));
    if let Some(account) = from { tx.insert("from".into(), account_value(&account)); }
    if let Some(account) = to { tx.insert("to".into(), account_value(&account)); }
    tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(amount)));
    if let Some(fee) = fee { tx.insert("fee".into(), ICRC3Value::Nat(Nat::from(fee))); }
    if let Some(memo) = memo { tx.insert("memo".into(), ICRC3Value::Blob(memo.into())); }
    if let Some(time) = created_at_time { tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(time))); }
    if let Some(account) = spender { tx.insert("spender".into(), account_value(&account)); }
    let mut block = ICRC3Map::new();
    block.insert("btype".into(), ICRC3Value::Text(btype.into()));
    block.insert("tx".into(), ICRC3Value::Map(tx));
    state.blocks.push(ICRC3Value::Map(block));
}

#[query]
fn icrc3_get_blocks(args: Vec<GetBlocksRequest>) -> GetBlocksResult {
    STATE.with(|s| {
        let state = s.borrow();
        let mut blocks = Vec::new();
        for request in args {
            let Some(start) = request.start.0.to_u64() else { continue };
            let Some(length) = request.length.0.to_u64() else { continue };
            let end = start.saturating_add(length).min(state.blocks.len() as u64);
            for index in start..end {
                blocks.push(BlockWithId { id: Nat::from(index), block: state.blocks[index as usize].clone() });
            }
        }
        GetBlocksResult { log_length: Nat::from(state.blocks.len() as u64), blocks, archived_blocks: vec![] }
    })
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
        if let Some(target) = state.fake_zero_balance_for {
            if account.owner == target {
                return Nat::from(0u64);
            }
        }
        Nat::from(state.balances.get(&account).copied().unwrap_or(0))
    })
}

#[query]
fn icrc1_minting_account() -> Option<Account> {
    STATE.with(|s| s.borrow().minting_account.clone())
}

#[query]
fn icrc1_fee() -> Nat {
    STATE.with(|s| Nat::from(s.borrow().fee))
}

#[update]
async fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
    let caller = ic_cdk::caller();
    let mut trap_after_commit = false;
    let transfer_result = STATE.with(|s| {
        let mut state = s.borrow_mut();

        if state.fail_transfers {
            return Err(TransferError::GenericError {
                error_code: Nat::from(999u64),
                message: "Injected failure: transfers disabled".to_string(),
            });
        }

        if let Some(target) = state.fail_transfers_for_caller {
            if caller == target {
                return Err(TransferError::GenericError {
                    error_code: Nat::from(997u64),
                    message: format!(
                        "Injected failure: transfers from caller {} disabled",
                        target
                    ),
                });
            }
        }

        let is_mint = state.minting_account.as_ref().is_some_and(|minting| {
            minting.owner == caller && args.from_subaccount.is_none()
        });
        let is_burn = !is_mint && state.minting_account.as_ref() == Some(&args.to);

        if let Some(requested_fee) = args.fee.as_ref().map(nat_to_u128) {
            let expected_fee = if is_burn { 0 } else { state.fee };
            if requested_fee != expected_fee {
                return Err(TransferError::BadFee {
                    expected_fee: Nat::from(expected_fee),
                });
            }
        }

        if state.bad_fee_failures_remaining > 0 {
            state.bad_fee_failures_remaining -= 1;
            return Err(TransferError::BadFee {
                expected_fee: Nat::from(state.fee),
            });
        }

        let from = account_key(caller, args.from_subaccount);
        let amount = nat_to_u128(&args.amount);
        let fee = args.fee.as_ref().map(nat_to_u128);

        // Dedup check (only when created_at_time is provided, matching ICRC-1).
        if let Some(t) = args.created_at_time {
            let key = DedupKey {
                caller,
                from_subaccount: args.from_subaccount,
                to: args.to.clone(),
                amount,
                fee,
                memo: args.memo.clone(),
                created_at_time: t,
            };
            if let Some(prev_block) = state.dedup.get(&key).copied() {
                return Err(TransferError::Duplicate {
                    duplicate_of: Nat::from(prev_block),
                });
            }
        }

        if state.too_old_failures_remaining > 0 {
            state.too_old_failures_remaining -= 1;
            return Err(TransferError::TooOld);
        }

        // Minting-account transfers create tokens; transfers to the configured
        // minting account burn the source amount without a fee. All other
        // transfers debit the caller and credit the recipient.
        if !is_mint {
            let balance = state.balances.get(&from).copied().unwrap_or(0);
            let debit = amount + if is_burn { 0 } else { state.fee };
            if debit > balance {
                return Err(TransferError::InsufficientFunds {
                    balance: Nat::from(balance),
                });
            }
        }

        if is_mint {
            *state.balances.entry(args.to.clone()).or_insert(0) += amount;
        } else {
            let debit = amount + if is_burn { 0 } else { state.fee };
            *state.balances.entry(from.clone()).or_insert(0) -= debit;
            if !is_burn {
                *state.balances.entry(args.to.clone()).or_insert(0) += amount;
            }
        }
        let landed_block = state.block_index;
        state.block_index += 1;
        let (btype, op, block_from, block_to, block_fee) = if is_mint {
            ("1mint", "mint", None, Some(icrc_account(args.to.owner, args.to.subaccount)), None)
        } else if is_burn {
            ("1burn", "burn", Some(icrc_account(caller, args.from_subaccount)), None, None)
        } else {
            ("1xfer", "xfer", Some(icrc_account(caller, args.from_subaccount)), Some(icrc_account(args.to.owner, args.to.subaccount)), Some(state.fee))
        };
        push_block(&mut state, btype, op, block_from, block_to, amount, block_fee, args.memo.clone(), args.created_at_time, None);

        if let Some(t) = args.created_at_time {
            let key = DedupKey {
                caller,
                from_subaccount: args.from_subaccount,
                to: args.to,
                amount,
                fee,
                memo: args.memo,
                created_at_time: t,
            };
            state.dedup.insert(key, landed_block);
        }

        if state.trap_after_commit_for_caller == Some(caller) {
            state.trap_after_commit_for_caller = None;
            trap_after_commit = true;
        }

        // Phantom-failure mode: the transfer committed above but we return an
        // error to the caller, simulating a lost reply.
        if state.phantom_failures_remaining > 0 {
            state.phantom_failures_remaining -= 1;
            return Err(TransferError::GenericError {
                error_code: Nat::from(998u64),
                message: "Injected phantom failure (transfer committed, reply lost)".to_string(),
            });
        }

        Ok(Nat::from(landed_block))
    });

    if trap_after_commit {
        // Await a self-call after the state changes have crossed an IC await
        // boundary, then trap so the caller receives a transport reject while
        // the committed ledger mutation remains visible.
        let _: Result<(), _> = ic_cdk::call(ic_cdk::id(), "yield_after_commit", ()).await;
        ic_cdk::trap("Injected trap after committed transfer");
    }

    transfer_result
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

        state.allowances.insert((from.clone(), spender.clone()), amount);

        let index = state.block_index;
        state.block_index += 1;
        push_block(
            &mut state, "2approve", "approve", Some(icrc_account(from.owner, from.subaccount)),
            None, amount, None, args.memo.clone(), args.created_at_time, Some(icrc_account(spender.owner, spender.subaccount)),
        );
        Ok(Nat::from(index))
    })
}

#[update]
fn icrc2_transfer_from(args: TransferFromArgs) -> Result<Nat, TransferFromError> {
    STATE.with(|s| {
        let mut state = s.borrow_mut();

        if state.fail_transfer_from {
            return Err(TransferFromError::GenericError {
                error_code: Nat::from(999u64),
                message: "Injected failure: transfer_from disabled".to_string(),
            });
        }

        if state.bad_fee_failures_remaining > 0 {
            state.bad_fee_failures_remaining -= 1;
            return Err(TransferFromError::BadFee {
                expected_fee: Nat::from(state.fee),
            });
        }

        let spender = ic_cdk::caller();
        let spender_account = account_key(spender, args.spender_subaccount);
        let from = args.from.clone();
        let amount = nat_to_u128(&args.amount);
        let fee = args.fee.as_ref().map(nat_to_u128);

        // Dedup keyed on the spender (caller of transfer_from) plus the args.
        if let Some(t) = args.created_at_time {
            let key = DedupKey {
                caller: spender,
                from_subaccount: from.subaccount,
                to: args.to.clone(),
                amount,
                fee,
                memo: args.memo.clone(),
                created_at_time: t,
            };
            if let Some(prev_block) = state.dedup.get(&key).copied() {
                return Err(TransferFromError::Duplicate {
                    duplicate_of: Nat::from(prev_block),
                });
            }
        }

        let allowance = state.allowances.get(&(from.clone(), spender_account.clone())).copied().unwrap_or(0);
        if amount > allowance {
            return Err(TransferFromError::InsufficientAllowance {
                allowance: Nat::from(allowance),
            });
        }

        let balance = state.balances.get(&from).copied().unwrap_or(0);
        if amount + state.fee > balance {
            return Err(TransferFromError::InsufficientFunds {
                balance: Nat::from(balance),
            });
        }

        if let Some(a) = state.allowances.get_mut(&(from.clone(), spender_account)) {
            *a -= amount;
        }

        *state.balances.entry(from.clone()).or_insert(0) -= amount + state.fee;
        *state.balances.entry(args.to.clone()).or_insert(0) += amount;
        let landed_block = state.block_index;
        state.block_index += 1;
        let transfer_fee = state.fee;
        push_block(
            &mut state, "2xfer", "xfer", Some(icrc_account(from.owner, from.subaccount)),
            Some(icrc_account(args.to.owner, args.to.subaccount)), amount, Some(transfer_fee),
            args.memo.clone(), args.created_at_time, Some(icrc_account(spender, args.spender_subaccount)),
        );

        if let Some(t) = args.created_at_time {
            let key = DedupKey {
                caller: spender,
                from_subaccount: from.subaccount,
                to: args.to,
                amount,
                fee,
                memo: args.memo,
                created_at_time: t,
            };
            state.dedup.insert(key, landed_block);
        }

        if state.phantom_failures_remaining > 0 {
            state.phantom_failures_remaining -= 1;
            return Err(TransferFromError::GenericError {
                error_code: Nat::from(998u64),
                message: "Injected phantom failure (transfer_from committed, reply lost)".to_string(),
            });
        }

        Ok(Nat::from(landed_block))
    })
}

// ─── Test Control Methods ───

/// Mint tokens to any account (no auth — test only).
#[update]
fn mint(account: Account, amount: Nat) {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let amt = nat_to_u128(&amount);
        *state.balances.entry(account.clone()).or_insert(0) += amt;
        state.block_index += 1;
        push_block(&mut state, "1mint", "mint", None, Some(icrc_account(account.owner, account.subaccount)), amt, None, None, None, None);
    });
}

/// When true, all icrc1_transfer calls return GenericError before committing.
#[update]
fn set_fail_transfers(fail: bool) {
    STATE.with(|s| s.borrow_mut().fail_transfers = fail);
}

/// When true, all icrc2_transfer_from calls return GenericError before committing.
#[update]
fn set_fail_transfer_from(fail: bool) {
    STATE.with(|s| s.borrow_mut().fail_transfer_from = fail);
}

/// Update the ledger fee returned by icrc1_fee and used in BadFee responses.
#[update]
fn set_fee(fee: Nat) {
    STATE.with(|s| s.borrow_mut().fee = nat_to_u128(&fee));
}

/// Next N transfers commit (state mutates, dedup record is written) and then
/// return a GenericError. Simulates the IC reply-loss case the audit covers.
#[update]
fn set_phantom_failures(n: u32) {
    STATE.with(|s| s.borrow_mut().phantom_failures_remaining = n);
}

/// Compatibility name used by the burn-reconciliation PocketIC fixture.
#[update]
fn set_phantom_transfer_failures(n: u32) {
    set_phantom_failures(n);
}

/// Cause the next transfer from `caller` to trap after committing and yielding.
#[update]
fn set_trap_after_commit_for_caller(caller: Option<Principal>) {
    STATE.with(|s| s.borrow_mut().trap_after_commit_for_caller = caller);
}

/// Self-call target used to create an await boundary after a committed transfer.
#[update]
fn yield_after_commit() {}

/// Make the next N transfers return TooOld before any balance mutation.
#[update]
fn set_too_old_failures(n: u32) {
    STATE.with(|s| s.borrow_mut().too_old_failures_remaining = n);
}

/// Test-only ICRC-1 minting-account configuration.
#[update]
fn set_minting_account(account: Option<Account>) {
    STATE.with(|s| s.borrow_mut().minting_account = account);
}

/// Next N transfers return BadFee { expected_fee = current fee } before
/// committing, regardless of the fee the caller submitted.
#[update]
fn set_bad_fee_failures(n: u32) {
    STATE.with(|s| s.borrow_mut().bad_fee_failures_remaining = n);
}

/// Wipe the dedup map. Tests that want explicit isolation between scenarios
/// can call this to start fresh without redeploying the canister.
#[update]
fn reset_dedup() {
    STATE.with(|s| s.borrow_mut().dedup.clear());
}

/// When `Some(p)`, `icrc1_transfer` rejects with `GenericError` if the
/// caller principal equals `p`. Set to `None` to clear. Lets a test fail
/// transfers from a specific canister (e.g., the liquidation bot) without
/// breaking transfers from other callers (e.g., the protocol's bot_claim
/// transfer to the bot).
#[update]
fn set_fail_transfers_for_caller(target: Option<Principal>) {
    STATE.with(|s| s.borrow_mut().fail_transfers_for_caller = target);
}

/// When `Some(p)`, `icrc1_balance_of` returns 0 for any account whose
/// owner equals `p`. Set to `None` to clear. Lets a test deterministically
/// fail the protocol's BOT-001b cancel gate (which compares
/// `icrc1_balance_of(protocol)` against `claim.collateral_amount - fee`)
/// without having to engineer the exact post-claim-and-return on-ledger
/// balance.
#[update]
fn set_fake_zero_balance_for(target: Option<Principal>) {
    STATE.with(|s| s.borrow_mut().fake_zero_balance_for = target);
}
