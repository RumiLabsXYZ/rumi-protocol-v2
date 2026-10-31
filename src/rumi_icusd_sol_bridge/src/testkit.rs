//! Test doubles for the nonce lane (host unit tests only): a simulated Solana chain behind the
//! [`SolRpc`] trait, a scripted signer behind [`TxSigner`], and a fixture that stands up a
//! configured instance. Nothing here touches a network or signs for real; the "signatures" are
//! hashes of the role and the message, which the lane never verifies.

use crate::ids::{Hash32, KeyRole};
use crate::lane::{SignOutcome, TxSigner};
use crate::nonce;
use crate::records::Lane;
use crate::sol_rpc::{
    AccountSnapshot, LandedTx, RpcFailure, RpcResult, SignatureEntry, SignatureStatus, SolRpc,
};
use crate::state::{self, RoleKeys};
use crate::tx::{
    assemble_wire_tx, decode_message, leading_advance_nonce, split_wire_transaction, Pubkey, Signature, SYSTEM_PROGRAM_ID,
};
use sha2::{Digest as _, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};

pub const FEE_PAYER: Pubkey = [0x11; 32];
pub const MINT_AUTHORITY: Pubkey = [0x22; 32];
pub const RETURN_OWNER: Pubkey = [0x33; 32];
pub const MINT: Pubkey = [0x44; 32];
pub const RECIPIENT: Pubkey = [0x77; 32];
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

/// An injected failure for one SOL RPC method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Inconsistent,
    Error(String),
    /// A consistent `null` (only meaningful for `get_account` and `get_transaction`).
    Null,
}

#[derive(Clone, Debug)]
struct MockTx {
    wire: Vec<u8>,
    failed: bool,
    slot: u64,
    meta: crate::sol_rpc::TxMeta,
}

type SendHook = Box<dyn Fn(&[u8])>;
/// Called from `get_account` AFTER the reply was computed and BEFORE it is returned: whatever the
/// hook changes is invisible to this read (the read "happened before"), as for an RPC reply that
/// is still in flight while other messages run.
type AccountReadHook = Box<dyn Fn(&MockChain, &Pubkey)>;
/// Called from `get_slot` AFTER the reply was computed and BEFORE it is returned: a read that is
/// still in flight while another pass stores, signs and sends (the preparation of a bootstrap
/// transaction waits on exactly this read).
type SlotReadHook = Box<dyn Fn(&MockChain)>;

#[derive(Default)]
pub struct MockChain {
    accounts: RefCell<BTreeMap<Pubkey, (Pubkey, Vec<u8>)>>,
    landed: RefCell<BTreeMap<Signature, MockTx>>,
    /// Signatures that mention an address, oldest first.
    by_address: RefCell<BTreeMap<Pubkey, Vec<Signature>>>,
    sent: RefCell<Vec<Vec<u8>>>,
    faults: RefCell<BTreeMap<&'static str, VecDeque<Fault>>>,
    sticky: RefCell<BTreeMap<&'static str, Fault>>,
    calls: RefCell<BTreeMap<&'static str, u32>>,
    next_slot: Cell<u64>,
    /// When set, every sent transaction that is valid against the current nonce lands at once.
    pub auto_land: Cell<bool>,
    pub on_send: RefCell<Option<SendHook>>,
    pub on_account_read: RefCell<Option<AccountReadHook>>,
    pub on_slot_read: RefCell<Option<SlotReadHook>>,
    /// When set, every sent bootstrap transaction (pool account, mint, return ATA) takes effect at once.
    pub auto_apply_creation: Cell<bool>,
    pub tip_slot: Cell<u64>,
    pub blocks: RefCell<BTreeMap<u64, [u8; 32]>>,
}

impl MockChain {
    pub fn new() -> Self {
        let c = MockChain::default();
        c.next_slot.set(1_000);
        c.tip_slot.set(2_000);
        c
    }

    // ---- accounts ----

    pub fn set_account(&self, address: Pubkey, owner: Pubkey, data: Vec<u8>) {
        self.accounts.borrow_mut().insert(address, (owner, data));
    }

    pub fn add_nonce_account(&self, address: Pubkey, authority: &Pubkey, value: &Hash32) {
        self.set_account(address, SYSTEM_PROGRAM_ID, nonce_account_data(1, 1, authority, value));
    }

    pub fn remove_account(&self, address: &Pubkey) {
        self.accounts.borrow_mut().remove(address);
    }

    pub fn nonce_value(&self, address: &Pubkey) -> Option<Hash32> {
        let accounts = self.accounts.borrow();
        let (_, data) = accounts.get(address)?;
        nonce::parse_nonce_data(data).ok().map(|n| n.value)
    }

    /// Advance a nonce account to its successor value without any landed transaction (a provider
    /// that has not indexed the consumer yet, or history that is gone).
    pub fn advance_without_transaction(&self, address: &Pubkey) -> Hash32 {
        let (owner, authority, current) = {
            let accounts = self.accounts.borrow();
            let (owner, data) = accounts.get(address).expect("nonce account exists");
            let n = nonce::parse_nonce_data(data).expect("initialized");
            (*owner, n.authority, n.value)
        };
        let next = successor(&current);
        self.set_account(*address, owner, nonce_account_data(1, 1, &authority, &next));
        next
    }

    // ---- faults and counters ----

    /// The next call of `method` fails this way (one shot, queued in order).
    pub fn fail_next(&self, method: &'static str, fault: Fault) {
        self.faults.borrow_mut().entry(method).or_default().push_back(fault);
    }

    /// Every call of `method` fails this way until cleared.
    pub fn fail_always(&self, method: &'static str, fault: Fault) {
        self.sticky.borrow_mut().insert(method, fault);
    }

    pub fn clear_faults(&self) {
        self.faults.borrow_mut().clear();
        self.sticky.borrow_mut().clear();
    }

    fn fault(&self, method: &'static str) -> Option<Fault> {
        *self.calls.borrow_mut().entry(method).or_insert(0) += 1;
        if let Some(f) = self.faults.borrow_mut().get_mut(method).and_then(|q| q.pop_front()) {
            return Some(f);
        }
        self.sticky.borrow().get(method).cloned()
    }

    pub fn call_count(&self, method: &'static str) -> u32 {
        self.calls.borrow().get(method).copied().unwrap_or(0)
    }

    // ---- sent and landed transactions ----

    pub fn sent(&self) -> Vec<Vec<u8>> {
        self.sent.borrow().clone()
    }

    /// Land a wire transaction if it is a valid durable-nonce transaction against the account's
    /// CURRENT value (the runtime check). The nonce advances; a landed transaction is stored with
    /// `failed` as its execution status. Returns whether it landed.
    pub fn land(&self, wire: &[u8], failed: bool) -> bool {
        let Ok((signatures, message_bytes)) = split_wire_transaction(wire) else { return false };
        let Some(first) = signatures.first().copied() else { return false };
        let Ok(message) = decode_message(message_bytes) else { return false };
        let Some((nonce_account, authority)) = leading_advance_nonce(&message) else { return false };
        let current = match self.nonce_value(&nonce_account) {
            Some(v) => v,
            None => return false,
        };
        if current != message.recent_blockhash {
            return false;
        }
        let owner = self.accounts.borrow().get(&nonce_account).map(|(o, _)| *o).unwrap_or(SYSTEM_PROGRAM_ID);
        self.set_account(nonce_account, owner, nonce_account_data(1, 1, &authority, &successor(&current)));
        let slot = self.next_slot.get();
        self.next_slot.set(slot + 1);
        self.landed.borrow_mut().insert(first, MockTx { wire: wire.to_vec(), failed, slot, meta: Default::default() });
        for key in &message.account_keys {
            self.by_address.borrow_mut().entry(*key).or_default().push(first);
        }
        true
    }

    /// Insert a landed transaction that did NOT consume a nonce (for example a transfer to the
    /// nonce account): it appears in the account's signature list and nothing else changes.
    pub fn land_noise(&self, wire: &[u8], mentions: &Pubkey) {
        let (signatures, _) = split_wire_transaction(wire).expect("wire");
        let first = signatures[0];
        let slot = self.next_slot.get();
        self.next_slot.set(slot + 1);
        self.landed.borrow_mut().insert(first, MockTx { wire: wire.to_vec(), failed: false, slot, meta: Default::default() });
        self.by_address.borrow_mut().entry(*mentions).or_default().push(first);
    }

    /// Land a transaction that is not a nonce consumer (a holder's return, a stray transfer, the
    /// bridge's own burn) with the metadata the RPC would report for it, listed under every address
    /// in `mentions`. Returns its first signature (the transaction id).
    pub fn land_with_meta(&self, wire: &[u8], failed: bool, meta: crate::sol_rpc::TxMeta, mentions: &[Pubkey]) -> Signature {
        let (signatures, _) = split_wire_transaction(wire).expect("wire");
        let first = signatures[0];
        let slot = self.next_slot.get();
        self.next_slot.set(slot + 1);
        self.landed.borrow_mut().insert(first, MockTx { wire: wire.to_vec(), failed, slot, meta });
        for key in mentions {
            self.by_address.borrow_mut().entry(*key).or_default().push(first);
        }
        first
    }

    /// Apply a creation transaction as the runtime would (no signature or fee checks): the pool
    /// account (`CreateAccountWithSeed` + `InitializeNonceAccount`), the mint (`CreateAccount` +
    /// `InitializeMint2`) or the return ATA (`CreateIdempotent`). Returns whether it applied.
    pub fn apply_creation(&self, wire: &[u8]) -> bool {
        let Ok((_, bytes)) = split_wire_transaction(wire) else { return false };
        let Ok(msg) = decode_message(bytes) else { return false };
        let keys = |ix: &crate::tx::CompiledInstruction| msg.instruction_accounts(ix).unwrap_or_default();
        let program = |ix: &crate::tx::CompiledInstruction| msg.program_id(ix).copied().unwrap_or([0xff; 32]);
        let tag_u32 = |d: &[u8]| d.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        match msg.instructions.as_slice() {
            [create, init] if program(create) == SYSTEM_PROGRAM_ID && program(init) == SYSTEM_PROGRAM_ID => {
                if tag_u32(&create.data) == Some(3) && tag_u32(&init.data) == Some(6) {
                    let target = keys(create)[1];
                    let authority: Pubkey = init.data[4..36].try_into().expect("authority");
                    self.add_nonce_account(target, &authority, &successor(&msg.recent_blockhash));
                    return true;
                }
                false
            }
            [create, init] if program(create) == SYSTEM_PROGRAM_ID => {
                if tag_u32(&create.data) == Some(0) && init.data.first() == Some(&20) {
                    let target = keys(create)[1];
                    let owner: Pubkey = create.data[20..52].try_into().expect("owner");
                    let authority: Pubkey = init.data[2..34].try_into().expect("authority");
                    let freeze = (init.data[34] == 1).then(|| <Pubkey>::try_from(&init.data[35..67]).expect("freeze"));
                    self.set_account(target, owner, crate::mint::fixtures::mint_data(Some(&authority), 0, init.data[1], true, freeze.as_ref()));
                    return true;
                }
                false
            }
            [ata] if program(ata) == crate::tx::ATA_PROGRAM_ID && ata.data == [1] => {
                // [funder, ata, wallet, mint, system, token program]
                let k = keys(ata);
                let base = crate::mint::fixtures::token_account_data(&k[3], &k[2], 0, None, 1, None);
                let data = if k[5] == crate::tx::TOKEN_2022_PROGRAM_ID {
                    crate::mint::fixtures::extended_token_account(base, &[(crate::mint::EXT_IMMUTABLE_OWNER, vec![])])
                } else {
                    base
                };
                self.set_account(k[1], k[5], data);
                true
            }
            _ => false,
        }
    }

    pub fn is_landed(&self, signature: &Signature) -> bool {
        self.landed.borrow().contains_key(signature)
    }

    fn memo_of(&self, tx: &MockTx) -> Option<String> {
        let (_, bytes) = split_wire_transaction(&tx.wire).ok()?;
        let message = decode_message(bytes).ok()?;
        message.instructions.iter().find_map(|ix| {
            (message.program_id(ix) == Some(&crate::tx::MEMO_PROGRAM_V3_ID)).then(|| {
                let text = String::from_utf8_lossy(&ix.data).to_string();
                format!("[{}] {text}", text.len())
            })
        })
    }
}

impl SolRpc for MockChain {
    async fn get_account(&self, address: &Pubkey) -> RpcResult<Option<AccountSnapshot>> {
        match self.fault("get_account") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            Some(Fault::Null) => return Ok(None),
            None => {}
        }
        let reply = self.accounts.borrow().get(address).map(|(owner, data)| AccountSnapshot { owner: *owner, data: data.clone() });
        if let Some(hook) = self.on_account_read.borrow_mut().take() {
            // One shot: the hook acts during the first read only.
            hook(self, address);
        }
        Ok(reply)
    }

    async fn get_transaction(&self, signature: &Signature) -> RpcResult<Option<LandedTx>> {
        match self.fault("get_transaction") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            Some(Fault::Null) => return Ok(None),
            None => {}
        }
        Ok(self.landed.borrow().get(signature).map(|t| LandedTx { slot: t.slot, wire: t.wire.clone(), failed: t.failed, meta: t.meta.clone() }))
    }

    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        before: Option<&Signature>,
        until: Option<&Signature>,
        limit: u32,
    ) -> RpcResult<Vec<SignatureEntry>> {
        match self.fault("get_signatures_for_address") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            Some(Fault::Null) => return Ok(vec![]),
            None => {}
        }
        let all: Vec<Signature> = self.by_address.borrow().get(address).cloned().unwrap_or_default();
        // Newest first.
        let mut newest_first: Vec<Signature> = all.into_iter().rev().collect();
        if let Some(b) = before {
            let Some(pos) = newest_first.iter().position(|s| s == b) else {
                return Err(RpcFailure::Error("before signature not found".into()));
            };
            newest_first = newest_first.split_off(pos + 1);
        }
        if let Some(u) = until {
            if let Some(pos) = newest_first.iter().position(|s| s == u) {
                newest_first.truncate(pos);
            }
        }
        newest_first.truncate(limit as usize);
        let landed = self.landed.borrow();
        Ok(newest_first
            .into_iter()
            .filter_map(|sig| {
                landed.get(&sig).map(|t| SignatureEntry { signature: sig, slot: t.slot, failed: t.failed, memo: self.memo_of(t) })
            })
            .collect())
    }

    async fn get_signature_statuses(&self, signatures: &[Signature], _search: bool) -> RpcResult<Vec<Option<SignatureStatus>>> {
        match self.fault("get_signature_statuses") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            Some(Fault::Null) => return Ok(signatures.iter().map(|_| None).collect()),
            None => {}
        }
        let landed = self.landed.borrow();
        Ok(signatures.iter().map(|s| landed.get(s).map(|t| SignatureStatus { slot: t.slot, failed: t.failed })).collect())
    }

    async fn send_transaction(&self, wire: &[u8]) -> RpcResult<Signature> {
        let fault = self.fault("send_transaction");
        self.sent.borrow_mut().push(wire.to_vec());
        if let Some(hook) = self.on_send.borrow().as_ref() {
            hook(wire);
        }
        if self.auto_land.get() {
            self.land(wire, false);
        }
        if self.auto_apply_creation.get() {
            self.apply_creation(wire);
        }
        match fault {
            Some(Fault::Inconsistent) => Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => Err(RpcFailure::Error(e)),
            Some(Fault::Null) | None => {
                let (sigs, _) = split_wire_transaction(wire).map_err(|e| RpcFailure::Error(format!("{e}")))?;
                sigs.first().copied().ok_or_else(|| RpcFailure::Error("no signature".into()))
            }
        }
    }

    async fn get_slot(&self) -> RpcResult<u64> {
        match self.fault("get_slot") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            _ => {}
        }
        let slot = self.tip_slot.get();
        // One-shot, like the account read hook: the hook may call back into the chain.
        let hook = self.on_slot_read.borrow_mut().take();
        if let Some(hook) = hook {
            hook(self);
        }
        Ok(slot)
    }

    async fn get_block_blockhash(&self, slot: u64) -> RpcResult<Option<[u8; 32]>> {
        match self.fault("get_block") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            _ => {}
        }
        Ok(self.blocks.borrow().get(&slot).copied())
    }
}

/// The 80-byte nonce account image.
pub fn nonce_account_data(version: u32, state: u32, authority: &Pubkey, value: &Hash32) -> Vec<u8> {
    let mut d = Vec::with_capacity(80);
    d.extend_from_slice(&version.to_le_bytes());
    d.extend_from_slice(&state.to_le_bytes());
    d.extend_from_slice(authority);
    d.extend_from_slice(value);
    d.extend_from_slice(&5_000u64.to_le_bytes());
    d
}

/// The nonce value after an advance (a stand-in for the runtime's hash chain).
pub fn successor(value: &Hash32) -> Hash32 {
    let mut h = Sha256::new();
    h.update(b"next-nonce");
    h.update(value);
    h.finalize().into()
}

// ---------------------------------------------------------------------------
// The signer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignCall {
    pub role: KeyRole,
    pub message: Vec<u8>,
}

pub enum Scripted {
    Fail(String),
    Unknown(String),
}

type SignHook = Box<dyn Fn(&SignCall)>;
/// Decides the outcome of one specific sign call (by role and message), ahead of the script.
type SignOverride = Box<dyn Fn(&SignCall) -> Option<Scripted>>;

#[derive(Default)]
pub struct MockSigner {
    pub calls: RefCell<Vec<SignCall>>,
    pub script: RefCell<VecDeque<Scripted>>,
    /// When set, the same (role, message) signed twice gives two different signatures.
    pub nondeterministic: Cell<bool>,
    pub on_call: RefCell<Option<SignHook>>,
    /// Fails (or leaves unknown) the sign calls it selects, whatever their position in the script.
    pub override_call: RefCell<Option<SignOverride>>,
    counter: Cell<u64>,
}

impl MockSigner {
    pub fn new() -> Self {
        MockSigner::default()
    }

    pub fn script(&self, s: Scripted) {
        self.script.borrow_mut().push_back(s);
    }

    pub fn call_count(&self) -> usize {
        self.calls.borrow().len()
    }

    /// The signature this signer returns for `(role, message)` when deterministic.
    pub fn signature_for(role: KeyRole, message: &[u8], nonce: u64) -> Signature {
        let mut h = Sha256::new();
        h.update(role.label());
        h.update(message);
        h.update(nonce.to_le_bytes());
        let half: [u8; 32] = h.finalize().into();
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&half);
        sig[32..].copy_from_slice(&half);
        sig
    }
}

impl TxSigner for MockSigner {
    async fn public_key(&self, role: KeyRole) -> Result<Pubkey, String> {
        match role {
            KeyRole::FeePayer => Ok(FEE_PAYER),
            KeyRole::MintAuthority => Ok(MINT_AUTHORITY),
            KeyRole::ReturnOwner => Ok(RETURN_OWNER),
            KeyRole::MintAccount => Ok(MINT),
        }
    }

    async fn sign(&self, role: KeyRole, message: &[u8]) -> SignOutcome {
        let call = SignCall { role, message: message.to_vec() };
        if let Some(hook) = self.on_call.borrow().as_ref() {
            hook(&call);
        }
        let forced = self.override_call.borrow().as_ref().and_then(|f| f(&call));
        self.calls.borrow_mut().push(call);
        match forced.or_else(|| self.script.borrow_mut().pop_front()) {
            Some(Scripted::Fail(e)) => return SignOutcome::Failed(e),
            Some(Scripted::Unknown(e)) => return SignOutcome::Unknown(e),
            None => {}
        }
        let nonce = if self.nondeterministic.get() {
            let n = self.counter.get() + 1;
            self.counter.set(n);
            n
        } else {
            0
        };
        SignOutcome::Signed(Self::signature_for(role, message, nonce))
    }
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

/// A configured instance: role keys, a bound mint, registered and verified nonce slots whose
/// accounts exist on the mock chain with distinct values.
pub struct Fixture {
    pub chain: MockChain,
    pub signer: MockSigner,
    pub ledger: MockLedger,
}

pub fn slot_initial_value(lane: Lane, index: u8) -> Hash32 {
    let mut v = [0u8; 32];
    v[0] = lane as u8 + 1;
    v[1] = index + 1;
    v[2..].fill(0xd0);
    v
}

impl Fixture {
    /// `pool` slots per lane.
    pub fn new(pool: u8) -> Self {
        Fixture::with_lane_settings(crate::types::LaneSettings { nonce_pool_size: pool, ..crate::types::LaneSettings::default() })
    }

    pub fn with_lane_settings(lane: crate::types::LaneSettings) -> Self {
        state::reset_for_tests();
        crate::env::test_env::set_now_ns(secs(1_000));
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.admins = vec![candid::Principal::from_slice(&[3; 10])];
            c.config.lane = lane.clone();
            c.config.token_program = crate::tx::TokenProgram::Token2022;
            c.keys = Some(RoleKeys { fee_payer: FEE_PAYER, mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER, mint_account: Some(MINT) });
            c.mint = Some(MINT);
            c.mint_state = crate::records::MintState::Bound;
        });
        let fixture = Fixture { chain: MockChain::new(), signer: MockSigner::new(), ledger: MockLedger::new() };
        for l in Lane::ALL {
            for i in 0..lane.nonce_pool_size {
                let slot = nonce::register_slot(l, i).expect("register");
                fixture.chain.add_nonce_account(slot.address, &FEE_PAYER, &slot_initial_value(l, i));
                let outcome = block_on(nonce::verify_slot(&fixture.chain, l, i)).expect("verify");
                assert_eq!(outcome, nonce::VerifyOutcome::Verified(slot_initial_value(l, i)));
            }
        }
        fixture
    }

    /// A configured instance with NO keys, no pool and no mint: the state before bootstrap. The
    /// chain is empty; creation transactions take effect when sent.
    pub fn unbootstrapped(pool: u8) -> Self {
        let lane = crate::types::LaneSettings { nonce_pool_size: pool, ..crate::types::LaneSettings::default() };
        state::reset_for_tests();
        crate::env::test_env::set_now_ns(secs(1_000));
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.admins = vec![candid::Principal::from_slice(&[3; 10])];
            c.config.lane = lane;
            c.config.token_program = crate::tx::TokenProgram::Token2022;
        });
        let fixture = Fixture { chain: MockChain::new(), signer: MockSigner::new(), ledger: MockLedger::new() };
        fixture.chain.auto_apply_creation.set(true);
        fixture.chain.blocks.borrow_mut().insert(1_999, [0x5b; 32]);
        fixture.chain.tip_slot.set(2_000);
        fixture
    }

    pub fn profile(&self) -> crate::tx::builders::TxProfile {
        state::read_core(|c| c.tx_profile()).expect("profile")
    }

    /// Submit a mint of `amount` to `RECIPIENT` with the given id byte.
    pub fn mint_request(id_byte: u8, amount: u64) -> crate::lane::SubmitRequest {
        crate::lane::SubmitRequest {
            kind: crate::records::OpKind::Mint,
            payload: crate::records::OpPayload::Mint { recipient_wallet: RECIPIENT, per_operation_cap: 1_000_000_000_000 },
            external_id: [id_byte; 32],
            amount,
        }
    }

    pub fn burn_request(id_byte: u8, amount: u64) -> crate::lane::SubmitRequest {
        crate::lane::SubmitRequest {
            kind: crate::records::OpKind::Burn,
            payload: crate::records::OpPayload::Burn,
            external_id: [id_byte; 32],
            amount,
        }
    }

    pub fn submit(&self, req: crate::lane::SubmitRequest, now: u64) -> Result<u64, crate::lane::SubmitError> {
        block_on(crate::lane::submit_op(&self.chain, &self.signer, req, now))
    }

    pub fn tick(&self, now: u64) -> crate::lane::TickReport {
        block_on(crate::lane::tick(&self.chain, &self.signer, now))
    }

    /// The wire transaction the lane would send for attempt `idx` of op `id` (assembled from the
    /// recorded signatures), without sending it.
    pub fn wire_of(&self, id: u64, idx: usize) -> Vec<u8> {
        let op = state::get_op(id).expect("op");
        let a = &op.attempts[idx];
        assemble_wire_tx(&a.latest_signatures().expect("fully signed"), &a.message).expect("wire")
    }
}

/// A minimal non-terminal (or chosen-state) operation record, for tests of the index and paging.
pub fn bare_op(id: u64, state: crate::records::OpState) -> crate::records::SolOp {
    let mut external = [0u8; 32];
    external[..8].copy_from_slice(&id.to_be_bytes());
    crate::records::SolOp {
        id,
        kind: crate::records::OpKind::Mint,
        payload: crate::records::OpPayload::Burn,
        external_id: external,
        amount: 1,
        state,
        lane: Lane::Mint,
        slot_index: None,
        generation: 0,
        attempts: vec![],
        observed_value: None,
        advance_seen_at_ns: None,
        discovery_cursor: None,
        created_at_ns: 0,
        updated_at_ns: 0,
        outcome_applied: false,
        alarm_raised_at_ns: None,
        note: None,
        non_delivery: vec![],
    }
}
