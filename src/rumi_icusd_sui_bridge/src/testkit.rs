//! Test doubles for the ICP-side sagas (host unit tests only): a simulated icUSD ledger behind the
//! [`IcpLedger`] trait and a fixture that stands up a configured instance. Nothing here touches a
//! network or signs for real. The MockLedger is the Solana testkit's (4394b472): it replays the
//! ledger's 24 hour deduplication window, fee check, block log and ICRC-3 search, so the
//! persist-before-send, same-tuple and absence-proof rules run end to end.

use crate::state;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};

pub const SECOND: u64 = 1_000_000_000;

/// Seconds to nanoseconds.
pub fn secs(s: u64) -> u64 {
    s * SECOND
}

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// Suspends the awaiting task exactly once.
#[derive(Default)]
pub struct YieldOnce(bool);

impl std::future::Future for YieldOnce {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}

/// Poll a future once, to leave it suspended at its first await that does not complete.
pub fn poll_once<F: std::future::Future>(future: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    use std::sync::Arc;
    use std::task::{Context, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    future.poll(&mut Context::from_waker(&waker))
}


// ---------------------------------------------------------------------------
// The icUSD ledger
// ---------------------------------------------------------------------------

use crate::icp::IcpLedger;
use crate::ledger::{CallOutcome, LedgerRejection, SearchOutcome};
use crate::state::{LedgerOp, LedgerOpKind};
use icrc_ledger_types::icrc1::account::Account;

type AccountKey = (candid::Principal, [u8; 32]);
type Allowance = (u128, Option<u64>);

fn norm(a: &Account) -> AccountKey {
    (a.owner, a.subaccount.unwrap_or([0u8; 32]))
}

/// What the ledger stores of a transfer: the tuple its deduplication and an ICRC-3 search match on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockBlock {
    pub kind: LedgerOpKind,
    pub from: Account,
    pub to: Account,
    pub spender: Option<Account>,
    pub amount: u64,
    pub fee: u64,
    pub memo: Vec<u8>,
    pub created_at_time: u64,
}

/// An injected failure for the next `execute` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerFault {
    /// The transfer executes and the reply is lost (an ambiguous outcome).
    ExecuteThenLoseReply,
    /// The call fails ambiguously and nothing executes.
    AmbiguousNoEffect,
    /// A typed rejection; nothing executes.
    Reject(LedgerRejection),
}

type ExecHook = Box<dyn Fn(&LedgerOp)>;

/// A simulated icUSD ledger: balances, ICRC-2 allowances, a block log, the 24 hour deduplication
/// window of ICRC-1 (a repeated identical tuple returns the original block), fee checking and
/// ICRC-3 search over the log. `execute` can be made to lose its reply or fail.
pub struct MockLedger {
    pub fee: Cell<u64>,
    balances: RefCell<BTreeMap<AccountKey, u128>>,
    allowances: RefCell<BTreeMap<(AccountKey, AccountKey), Allowance>>,
    pub blocks: RefCell<Vec<MockBlock>>,
    faults: RefCell<VecDeque<LedgerFault>>,
    /// Every tuple handed to `execute`, in order (before any fault applies).
    pub sent: RefCell<Vec<LedgerOp>>,
    pub minting: RefCell<Option<Account>>,
    /// Called at the top of `execute`, before anything happens: lets a test look at the stored state
    /// at the moment of the send.
    pub on_execute: RefCell<Option<ExecHook>>,
    /// Make `search` fail with this error.
    pub search_error: RefCell<Option<String>>,
    pub search_calls: RefCell<Vec<(u64, u64)>>,
    pub fee_calls: Cell<u32>,
    /// Every call whose memo is listed fails ambiguously and never executes, on every attempt: a
    /// transfer whose outcome stays unresolved for as long as the test needs.
    pub ambiguous_memos: RefCell<Vec<Vec<u8>>>,
}

impl Default for MockLedger {
    fn default() -> Self {
        MockLedger::new()
    }
}

impl MockLedger {
    pub fn new() -> MockLedger {
        MockLedger {
            fee: Cell::new(10_000),
            balances: RefCell::default(),
            allowances: RefCell::default(),
            blocks: RefCell::default(),
            faults: RefCell::default(),
            sent: RefCell::default(),
            minting: RefCell::new(Some(Account { owner: candid::Principal::from_slice(&[0x4d; 10]), subaccount: None })),
            on_execute: RefCell::new(None),
            search_error: RefCell::new(None),
            search_calls: RefCell::default(),
            fee_calls: Cell::new(0),
            ambiguous_memos: RefCell::default(),
        }
    }

    pub fn credit(&self, a: &Account, amount: u128) {
        *self.balances.borrow_mut().entry(norm(a)).or_insert(0) += amount;
    }

    /// Remove funds (the escrow lost them): a deficit the monitors must catch.
    pub fn debit(&self, a: &Account, amount: u128) {
        let mut balances = self.balances.borrow_mut();
        let entry = balances.entry(norm(a)).or_insert(0);
        *entry = entry.saturating_sub(amount);
    }

    pub fn balance(&self, a: &Account) -> u128 {
        self.balances.borrow().get(&norm(a)).copied().unwrap_or(0)
    }

    pub fn approve(&self, from: &Account, spender: &Account, amount: u128, expires_at: Option<u64>) {
        self.allowances.borrow_mut().insert((norm(from), norm(spender)), (amount, expires_at));
    }

    pub fn allowance_of(&self, from: &Account, spender: &Account) -> u128 {
        self.allowances.borrow().get(&(norm(from), norm(spender))).map(|a| a.0).unwrap_or(0)
    }

    /// Fill the log with `n` unrelated blocks (so a search window starts above zero).
    pub fn preload_blocks(&self, n: u64) {
        for i in 0..n {
            self.blocks.borrow_mut().push(MockBlock {
                kind: LedgerOpKind::Sweep,
                from: Account { owner: candid::Principal::from_slice(&[0xee; 10]), subaccount: None },
                to: Account { owner: candid::Principal::from_slice(&[0xef; 10]), subaccount: None },
                spender: None,
                amount: 1,
                fee: 0,
                memo: i.to_be_bytes().to_vec(),
                created_at_time: 1,
            });
        }
    }

    pub fn push_fault(&self, f: LedgerFault) {
        self.faults.borrow_mut().push_back(f);
    }

    pub fn sent_count(&self) -> usize {
        self.sent.borrow().len()
    }

    fn block_of(op: &LedgerOp, fee: u64) -> MockBlock {
        MockBlock {
            kind: op.kind,
            from: op.from,
            to: op.to,
            spender: op.spender,
            amount: op.amount_e8s,
            fee,
            memo: op.memo.clone(),
            created_at_time: op.created_at_time,
        }
    }

    fn same_tuple(a: &MockBlock, b: &MockBlock) -> bool {
        a.kind == b.kind
            && norm(&a.from) == norm(&b.from)
            && norm(&a.to) == norm(&b.to)
            && a.spender.map(|x| norm(&x)) == b.spender.map(|x| norm(&x))
            && a.amount == b.amount
            && a.memo == b.memo
            && a.created_at_time == b.created_at_time
    }

    /// Apply the transfer, or return why it is refused. Mirrors the ledger's checks in its order.
    fn apply(&self, op: &LedgerOp) -> CallOutcome {
        let now = crate::env::now_ns();
        let fee = self.fee.get();
        // Window.
        if op.created_at_time.saturating_add(crate::ledger::TX_WINDOW_NS + crate::ledger::PERMITTED_DRIFT_NS) < now {
            return CallOutcome::Rejected(LedgerRejection::TooOld);
        }
        if op.created_at_time > now.saturating_add(crate::ledger::PERMITTED_DRIFT_NS) {
            return CallOutcome::Rejected(LedgerRejection::CreatedInFuture);
        }
        // Deduplication.
        let candidate = Self::block_of(op, if op.kind == LedgerOpKind::Pull { fee } else { op.fee_e8s.unwrap_or(fee) });
        if let Some(pos) = self.blocks.borrow().iter().position(|b| Self::same_tuple(b, &candidate)) {
            return CallOutcome::Accepted { block_index: pos as u64 };
        }
        // Fee (explicit on transfers).
        if op.kind != LedgerOpKind::Pull && op.fee_e8s != Some(fee) {
            return CallOutcome::Rejected(LedgerRejection::BadFee { expected_fee: fee });
        }
        let need = op.amount_e8s as u128 + fee as u128;
        if op.kind == LedgerOpKind::Pull {
            let spender = op.spender.expect("a pull has a spender");
            let allowance = self.allowances.borrow().get(&(norm(&op.from), norm(&spender))).copied();
            let (allowed, expires) = allowance.unwrap_or((0, None));
            let live = expires.map(|e| e > now).unwrap_or(true);
            if !live || allowed < need {
                return CallOutcome::Rejected(LedgerRejection::InsufficientAllowance { allowance: if live { allowed } else { 0 } });
            }
        }
        let balance = self.balance(&op.from);
        if balance < need {
            return CallOutcome::Rejected(LedgerRejection::InsufficientFunds { balance });
        }
        *self.balances.borrow_mut().entry(norm(&op.from)).or_insert(0) -= need;
        *self.balances.borrow_mut().entry(norm(&op.to)).or_insert(0) += op.amount_e8s as u128;
        if op.kind == LedgerOpKind::Pull {
            let spender = op.spender.expect("a pull has a spender");
            if let Some(a) = self.allowances.borrow_mut().get_mut(&(norm(&op.from), norm(&spender))) {
                a.0 -= need;
            }
        }
        self.blocks.borrow_mut().push(candidate);
        CallOutcome::Accepted { block_index: self.blocks.borrow().len() as u64 - 1 }
    }
}

impl IcpLedger for MockLedger {
    async fn execute(&self, op: &LedgerOp) -> CallOutcome {
        if let Some(hook) = self.on_execute.borrow().as_ref() {
            hook(op);
        }
        self.sent.borrow_mut().push(op.clone());
        if self.ambiguous_memos.borrow().contains(&op.memo) {
            return CallOutcome::Ambiguous("the call failed (this memo always does)".into());
        }
        match self.faults.borrow_mut().pop_front() {
            Some(LedgerFault::ExecuteThenLoseReply) => {
                let _ = self.apply(op);
                CallOutcome::Ambiguous("the reply was lost".into())
            }
            Some(LedgerFault::AmbiguousNoEffect) => CallOutcome::Ambiguous("the call failed".into()),
            Some(LedgerFault::Reject(r)) => CallOutcome::Rejected(r),
            None => self.apply(op),
        }
    }

    async fn search(&self, op: &LedgerOp, from_block: u64, max_blocks: u64, now_ns: u64) -> SearchOutcome {
        self.search_calls.borrow_mut().push((from_block, max_blocks));
        if let Some(e) = self.search_error.borrow().clone() {
            return SearchOutcome::Error(e);
        }
        let blocks = self.blocks.borrow();
        let tip = blocks.len() as u64;
        let end = tip.min(from_block.saturating_add(max_blocks));
        let wanted = Self::block_of(op, op.fee_e8s.unwrap_or(0));
        for i in from_block..end {
            if Self::same_tuple(&blocks[i as usize], &wanted) {
                return SearchOutcome::Found { block_index: i };
            }
        }
        crate::ledger::conclude_scan(op.created_at_time, from_block, end, tip, now_ns)
    }

    async fn fee(&self) -> Result<u64, String> {
        self.fee_calls.set(self.fee_calls.get() + 1);
        Ok(self.fee.get())
    }

    async fn balance_of(&self, account: Account) -> Result<u128, String> {
        Ok(self.balance(&account))
    }

    async fn allowance(&self, account: Account, spender: Account) -> Result<(u128, Option<u64>), String> {
        Ok(self.allowances.borrow().get(&(norm(&account), norm(&spender))).copied().unwrap_or((0, None)))
    }

    async fn log_length(&self) -> Result<u64, String> {
        Ok(self.blocks.borrow().len() as u64)
    }

    async fn minting_account(&self) -> Result<Option<Account>, String> {
        Ok(*self.minting.borrow())
    }
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// A configured instance: network, gate binding, role keys, runtime config with deposits enabled and
/// the ledger facts known, behind a mock ledger.
pub struct Fixture {
    pub ledger: MockLedger,
}

impl Fixture {
    pub fn new() -> Self {
        state::reset_for_tests();
        crate::env::test_env::set_now_ns(secs(1_000));
        state::mutate_core(|c| {
            c.initialized = true;
            c.network = crate::ids::SuiNetwork::Testnet;
            c.chain_identifier = crate::ids::TESTNET_CHAIN_IDENTIFIER;
            c.instance_salt = [0x5a; 32];
            c.binding = Some(state::StoredBinding { package: [0x70; 32], gate: [0x6a; 32], mint_cap: [0x6d; 32], admin_cap: [0x6e; 32] });
            c.roles = vec![
                state::StoredRoleKey { role: crate::ids::SuiRole::Treasury, public_key: [0x11; 32] },
                state::StoredRoleKey { role: crate::ids::SuiRole::Gas, public_key: [0x22; 32] },
                state::StoredRoleKey { role: crate::ids::SuiRole::UpgradeCustody, public_key: [0x33; 32] },
            ];
        });
        state::mutate_runtime(|c| {
            c.initialized = true;
            c.config.icusd_ledger = candid::Principal::from_slice(&[0x1c; 10]);
            c.config.admins = vec![candid::Principal::from_slice(&[3; 10])];
            c.config.pauses.deposits = false;
        });
        Fixture { ledger: MockLedger::new() }
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Fixture::new()
    }
}
