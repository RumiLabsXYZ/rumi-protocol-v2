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
//   - set_fail_fee_query(bool)        make icrc1_fee trap
//   - set_phantom_failures(u32)       next N transfers commit but return GenericError
//                                     (simulates "ledger committed, reply lost")
//   - set_minter(Option<Principal>)   transfers from this caller produce fee-free 1mint blocks
//   - set_too_old_after_phantom_mint(u32) next exact retry of a phantom mint returns TooOld
//   - set_bad_fee_failures(u32)       next N transfers return BadFee with set_fee value
//   - mint(Account, Nat)              mint tokens to any account (no auth)
//   - reset_dedup()                   clear the dedup map (for explicit test isolation)
//
// Dedup behaviour matches ICRC-1: if a transfer with identical
// (caller, from_subaccount, to, amount, fee, memo, created_at_time) lands
// twice while still in the dedup window (no time advancement here), the
// second call returns Duplicate { duplicate_of }.

use candid::{CandidType, Nat, Principal};
use ic_cdk::{init, query, update};
use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
use icrc_ledger_types::icrc3::archive::{GetArchivesArgs, GetArchivesResult};
use icrc_ledger_types::icrc3::blocks::{BlockWithId, GetBlocksRequest, GetBlocksResult};
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
    /// ICRC-3 history is passive test evidence. Every successful ledger
    /// operation appends exactly one block at its normal zero-based index.
    blocks: Vec<BlockWithId>,
    fee: u128,
    fail_fee_query: bool,
    fail_transfers: bool,
    fail_transfer_from: bool,
    /// Next N transfers commit but return a transient error (simulates lost reply).
    phantom_failures_remaining: u32,
    /// Next N ICRC-1 transfers commit but lose their reply, without consuming
    /// the fault on an earlier ICRC-2 transfer_from in the same saga.
    phantom_icrc1_failures_remaining: u32,
    /// Optional caller whose default-account ICRC-1 transfers produce `1mint`
    /// blocks. This is a fixture-only approximation of the configured minter.
    minter: Option<Principal>,
    /// Phantom mint tuples and their committed block indexes, used to model a
    /// typed TooOld reply on an exact retry without applying another mint.
    phantom_mint_dedup: BTreeMap<DedupKey, u64>,
    too_old_after_phantom_mint_remaining: u32,
    too_old_before_mint_remaining: u32,
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

fn nat_to_u128(n: &Nat) -> u128 {
    n.0.clone().try_into().unwrap_or(0)
}

fn take_too_old_after_phantom_mint(state: &mut LedgerState, key: &DedupKey, duplicate_of: u64) -> bool {
    if state.phantom_mint_dedup.get(key) == Some(&duplicate_of)
        && state.too_old_after_phantom_mint_remaining > 0
    {
        state.too_old_after_phantom_mint_remaining -= 1;
        true
    } else {
        false
    }
}

fn account_key(owner: Principal, subaccount: Option<[u8; 32]>) -> Account {
    Account { owner, subaccount }
}

fn append_block(state: &mut LedgerState, block: ICRC3Value) -> u64 {
    let id = state.block_index;
    state.block_index = state.block_index.saturating_add(1);
    let mut block = match block {
        ICRC3Value::Map(block) => block,
        _ => unreachable!("test ledger blocks are maps"),
    };
    // ICRC-3 requires a block-level timestamp and permits an optional hash of
    // the preceding block. Keep this fixture's log schema-valid so tests do
    // not accidentally exercise a more permissive decoder than production.
    block.insert("ts".into(), ICRC3Value::Nat(Nat::from(ic_cdk::api::time())));
    if let Some(previous) = state.blocks.last() {
        block.insert(
            "phash".into(),
            ICRC3Value::Blob(previous.block.clone().hash().to_vec().into()),
        );
    }
    state.blocks.push(BlockWithId {
        id: Nat::from(id),
        block: ICRC3Value::Map(block),
    });
    id
}

fn account_value(account: &Account) -> ICRC3Value {
    let mut parts = vec![ICRC3Value::Blob(account.owner.as_slice().to_vec().into())];
    if let Some(subaccount) = account.subaccount {
        parts.push(ICRC3Value::Blob(subaccount.to_vec().into()));
    }
    ICRC3Value::Array(parts)
}

fn transfer_block(
    btype: &str,
    op: &str,
    from: Option<&Account>,
    to: &Account,
    amount: u128,
    fee: u128,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
    spender: Option<&Account>,
) -> ICRC3Value {
    let mut tx = ICRC3Map::new();
    tx.insert("op".into(), ICRC3Value::Text(op.into()));
    if let Some(from) = from {
        tx.insert("from".into(), account_value(from));
    }
    tx.insert("to".into(), account_value(to));
    tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(amount)));
    tx.insert("fee".into(), ICRC3Value::Nat(Nat::from(fee)));
    if let Some(memo) = memo {
        tx.insert("memo".into(), ICRC3Value::Blob(memo.to_vec().into()));
    }
    if let Some(created_at_time) = created_at_time {
        tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(created_at_time)));
    }
    if let Some(spender) = spender {
        tx.insert("spender".into(), account_value(spender));
    }
    let mut block = ICRC3Map::new();
    block.insert("btype".into(), ICRC3Value::Text(btype.into()));
    block.insert("tx".into(), ICRC3Value::Map(tx));
    ICRC3Value::Map(block)
}

fn approve_block(from: &Account, spender: &Account, amount: u128) -> ICRC3Value {
    let mut tx = ICRC3Map::new();
    tx.insert("op".into(), ICRC3Value::Text("approve".into()));
    tx.insert("from".into(), account_value(from));
    tx.insert("spender".into(), account_value(spender));
    tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(amount)));
    let mut block = ICRC3Map::new();
    block.insert("btype".into(), ICRC3Value::Text("1approve".into()));
    block.insert("tx".into(), ICRC3Value::Map(tx));
    ICRC3Value::Map(block)
}

fn mint_block(to: &Account, amount: u128) -> ICRC3Value {
    let mut tx = ICRC3Map::new();
    tx.insert("op".into(), ICRC3Value::Text("mint".into()));
    tx.insert("to".into(), account_value(to));
    tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(amount)));
    let mut block = ICRC3Map::new();
    block.insert("btype".into(), ICRC3Value::Text("1mint".into()));
    block.insert("tx".into(), ICRC3Value::Map(tx));
    ICRC3Value::Map(block)
}

fn borrow_mint_block(
    to: &Account,
    amount: u128,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
) -> ICRC3Value {
    let mut block = match mint_block(to, amount) {
        ICRC3Value::Map(block) => block,
        _ => unreachable!("mint blocks are maps"),
    };
    if let Some(ICRC3Value::Map(tx)) = block.get_mut("tx") {
        if let Some(memo) = memo {
            tx.insert("memo".into(), ICRC3Value::Blob(memo.to_vec().into()));
        }
        if let Some(timestamp) = created_at_time {
            tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(timestamp)));
        }
    }
    ICRC3Value::Map(block)
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
fn icrc1_fee() -> Nat {
    STATE.with(|s| {
        let state = s.borrow();
        if state.fail_fee_query {
            ic_cdk::trap("injected icrc1_fee query failure");
        }
        Nat::from(state.fee)
    })
}

/// Pool-status shim for the backend's CL-07 refund fixture. This canister is
/// selected only in kill-switch tests, which reject after the 3USD pull and
/// before ICRC-3 proof verification; success tests use real rumi_3pool.
#[derive(CandidType, Serialize)]
struct TestPoolStatus {
    virtual_price: Nat,
}

#[query]
fn get_pool_status() -> TestPoolStatus {
    TestPoolStatus {
        virtual_price: Nat::from(1_000_000_000_000_000_000u128),
    }
}

#[update]
fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
    let caller = ic_cdk::caller();
    STATE.with(|s| {
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

        if state.bad_fee_failures_remaining > 0 {
            state.bad_fee_failures_remaining -= 1;
            return Err(TransferError::BadFee {
                expected_fee: Nat::from(state.fee),
            });
        }

        let from = account_key(caller, args.from_subaccount);
        let amount = nat_to_u128(&args.amount);
        let fee = args.fee.as_ref().map(nat_to_u128);
        let is_mint = state.minter == Some(caller) && args.from_subaccount.is_none();
        let dedup_key = args.created_at_time.map(|created_at_time| DedupKey {
            caller,
            from_subaccount: args.from_subaccount,
            to: args.to.clone(),
            amount,
            fee,
            memo: args.memo.clone(),
            created_at_time,
        });

        // Fixture mode for proving recovery when TooOld arrives before any
        // mint commit. This is deliberately independent of the dedup map.
        if is_mint && state.too_old_before_mint_remaining > 0 {
            state.too_old_before_mint_remaining -= 1;
            return Err(TransferError::TooOld);
        }

        // Dedup check (only when created_at_time is provided, matching ICRC-1).
        if let Some(key) = dedup_key.as_ref() {
            if let Some(prev_block) = state.dedup.get(key).copied() {
                if is_mint && take_too_old_after_phantom_mint(&mut state, key, prev_block) {
                    return Err(TransferError::TooOld);
                }
                return Err(TransferError::Duplicate {
                    duplicate_of: Nat::from(prev_block),
                });
            }
        }

        let expected_fee = if is_mint { 0 } else { state.fee };
        if fee.is_some_and(|quoted| quoted != expected_fee) {
            return Err(TransferError::BadFee { expected_fee: Nat::from(expected_fee) });
        }

        // Balance check (against the caller's debit, not the to-account).
        let balance = state.balances.get(&from).copied().unwrap_or(0);
        if !is_mint && amount + state.fee > balance {
            return Err(TransferError::InsufficientFunds {
                balance: Nat::from(balance),
            });
        }

        // Commit balances and append the same exact transfer to the passive
        // ICRC-3 fixture log used by positive-proof tests.
        let charged_fee = expected_fee;
        if !is_mint {
            *state.balances.entry(from.clone()).or_insert(0) -= amount + charged_fee;
        }
        *state.balances.entry(args.to.clone()).or_insert(0) += amount;
        let block = if is_mint {
            borrow_mint_block(
                &args.to,
                amount,
                args.memo.as_deref(),
                args.created_at_time,
            )
        } else {
            transfer_block(
                "1xfer",
                "xfer",
                Some(&from),
                &args.to,
                amount,
                charged_fee,
                args.memo.as_deref(),
                args.created_at_time,
                None,
            )
        };
        let landed_block = append_block(&mut state, block);

        if let Some(key) = dedup_key.as_ref() {
            state.dedup.insert(key.clone(), landed_block);
        }

        // Phantom-failure mode: the transfer committed above but we return an
        // error to the caller, simulating a lost reply.
        if state.phantom_failures_remaining > 0 {
            state.phantom_failures_remaining -= 1;
            if is_mint {
                if let Some(key) = dedup_key.as_ref() {
                    state.phantom_mint_dedup.insert(key.clone(), landed_block);
                }
            }
            return Err(TransferError::GenericError {
                error_code: Nat::from(998u64),
                message: "Injected phantom failure (transfer committed, reply lost)".to_string(),
            });
        }
        if state.phantom_icrc1_failures_remaining > 0 {
            state.phantom_icrc1_failures_remaining -= 1;
            if is_mint {
                if let Some(key) = dedup_key.as_ref() {
                    state.phantom_mint_dedup.insert(key.clone(), landed_block);
                }
            }
            return Err(TransferError::GenericError {
                error_code: Nat::from(997u64),
                message: "Injected ICRC-1 phantom failure (transfer committed, reply lost)"
                    .to_string(),
            });
        }

        Ok(Nat::from(landed_block))
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

        state
            .allowances
            .insert((from.clone(), spender.clone()), amount);
        let landed_block = append_block(&mut state, approve_block(&from, &spender, amount));
        Ok(Nat::from(landed_block))
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

        if fee.is_some_and(|quoted| quoted != state.fee) {
            return Err(TransferFromError::BadFee { expected_fee: Nat::from(state.fee) });
        }

        let allowance = state
            .allowances
            .get(&(from.clone(), spender_account.clone()))
            .copied()
            .unwrap_or(0);
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

        if let Some(a) = state
            .allowances
            .get_mut(&(from.clone(), spender_account.clone()))
        {
            *a -= amount;
        }

        let charged_fee = state.fee;
        *state.balances.entry(from.clone()).or_insert(0) -= amount + charged_fee;
        *state.balances.entry(args.to.clone()).or_insert(0) += amount;
        let landed_block = append_block(
            &mut state,
            transfer_block(
                "2xfer",
                "xfer",
                Some(&from),
                &args.to,
                amount,
                charged_fee,
                args.memo.as_deref(),
                args.created_at_time,
                Some(&spender_account),
            ),
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
                message: "Injected phantom failure (transfer_from committed, reply lost)"
                    .to_string(),
            });
        }

        Ok(Nat::from(landed_block))
    })
}

// ─── ICRC-3 (unarchived, fixture-sized history) ───

#[query]
fn icrc3_get_blocks(args: Vec<GetBlocksRequest>) -> GetBlocksResult {
    const MAX_BLOCKS_PER_RESPONSE: usize = 100;
    STATE.with(|s| {
        let state = s.borrow();
        let mut blocks = Vec::new();
        for request in args {
            let Ok(start) = u64::try_from(request.start.0) else {
                continue;
            };
            let Ok(length) = u64::try_from(request.length.0) else {
                continue;
            };
            let length = length.min((MAX_BLOCKS_PER_RESPONSE - blocks.len()) as u64);
            let Some(end) = start.checked_add(length) else {
                continue;
            };
            for block_index in start..end.min(state.blocks.len() as u64) {
                if let Some(block) = state.blocks.get(block_index as usize) {
                    blocks.push(block.clone());
                }
            }
            if blocks.len() == MAX_BLOCKS_PER_RESPONSE {
                break;
            }
        }
        GetBlocksResult {
            log_length: Nat::from(state.blocks.len() as u64),
            blocks,
            archived_blocks: vec![],
        }
    })
}

#[query]
fn icrc3_get_archives(_args: GetArchivesArgs) -> GetArchivesResult {
    vec![]
}

// ─── Test Control Methods ───

/// Mint tokens to any account (no auth — test only).
#[update]
fn mint(account: Account, amount: Nat) {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let amt = nat_to_u128(&amount);
        *state.balances.entry(account.clone()).or_insert(0) += amt;
        let block = mint_block(&account, amt);
        append_block(&mut state, block);
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

/// Make icrc1_fee trap to exercise fail-closed fee admission.
#[update]
fn set_fail_fee_query(fail: bool) {
    STATE.with(|s| s.borrow_mut().fail_fee_query = fail);
}

/// Next N transfers commit (state mutates, dedup record is written) and then
/// return a GenericError. Simulates the IC reply-loss case the audit covers.
#[update]
fn set_phantom_failures(n: u32) {
    STATE.with(|s| s.borrow_mut().phantom_failures_remaining = n);
}

/// Next N ICRC-1 transfers commit and lose their reply. ICRC-2 transfer_from
/// calls do not consume this fault, allowing precise multi-leg saga tests.
#[update]
fn set_phantom_icrc1_failures(n: u32) {
    STATE.with(|s| s.borrow_mut().phantom_icrc1_failures_remaining = n);
}

/// Configure the caller whose default-account ICRC-1 transfers produce
/// fee-free `1mint` blocks. `None` restores ordinary transfer behavior.
#[update]
fn set_minter(minter: Option<Principal>) {
    STATE.with(|s| s.borrow_mut().minter = minter);
}

/// The next N exact retries of a previously phantom-committed mint return the
/// typed `TooOld` error. The original mint block and balance remain untouched;
/// this is a simulated response, not evidence of real ledger expiry behavior.
#[update]
fn set_too_old_after_phantom_mint(n: u32) {
    STATE.with(|s| s.borrow_mut().too_old_after_phantom_mint_remaining = n);
}

/// The next N configured minter calls return typed TooOld before changing
/// balances, dedup state, or the ICRC-3 log.
#[update]
fn set_too_old_before_mint(n: u32) {
    STATE.with(|s| s.borrow_mut().too_old_before_mint_remaining = n);
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
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.dedup.clear();
        state.phantom_mint_dedup.clear();
    });
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

#[cfg(test)]
mod borrow_mint_retry_tests {
    use super::*;

    #[test]
    fn simulated_too_old_is_limited_to_the_exact_phantom_mint() {
        let caller = Principal::self_authenticating(b"fixture-minter");
        let owner = Principal::self_authenticating(b"mint-recipient");
        let key = DedupKey {
            caller,
            from_subaccount: None,
            to: Account {
                owner,
                subaccount: None,
            },
            amount: 42,
            fee: None,
            memo: Some(vec![1, 2, 3]),
            created_at_time: 123,
        };
        let mut other_key = key.clone();
        other_key.amount += 1;

        let mut state = LedgerState {
            too_old_after_phantom_mint_remaining: 1,
            ..LedgerState::default()
        };
        state.balances.insert(key.to.clone(), 42);
        state.dedup.insert(key.clone(), 7);
        state.phantom_mint_dedup.insert(key.clone(), 7);

        assert!(!take_too_old_after_phantom_mint(&mut state, &other_key, 7));
        assert!(!take_too_old_after_phantom_mint(&mut state, &key, 8));
        assert_eq!(state.too_old_after_phantom_mint_remaining, 1);
        assert!(take_too_old_after_phantom_mint(&mut state, &key, 7));
        assert!(!take_too_old_after_phantom_mint(&mut state, &key, 7));
        assert_eq!(state.too_old_after_phantom_mint_remaining, 0);
        assert_eq!(state.balances.get(&key.to), Some(&42));
        assert!(state.blocks.is_empty());
    }
}
