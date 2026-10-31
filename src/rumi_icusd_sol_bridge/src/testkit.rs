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
}

type SendHook = Box<dyn Fn(&[u8])>;

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
        self.landed.borrow_mut().insert(first, MockTx { wire: wire.to_vec(), failed, slot });
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
        self.landed.borrow_mut().insert(first, MockTx { wire: wire.to_vec(), failed: false, slot });
        self.by_address.borrow_mut().entry(*mentions).or_default().push(first);
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
        Ok(self.accounts.borrow().get(address).map(|(owner, data)| AccountSnapshot { owner: *owner, data: data.clone() }))
    }

    async fn get_transaction(&self, signature: &Signature) -> RpcResult<Option<LandedTx>> {
        match self.fault("get_transaction") {
            Some(Fault::Inconsistent) => return Err(RpcFailure::Inconsistent),
            Some(Fault::Error(e)) => return Err(RpcFailure::Error(e)),
            Some(Fault::Null) => return Ok(None),
            None => {}
        }
        Ok(self.landed.borrow().get(signature).map(|t| LandedTx { slot: t.slot, wire: t.wire.clone(), failed: t.failed }))
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
        Ok(self.tip_slot.get())
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

#[derive(Default)]
pub struct MockSigner {
    pub calls: RefCell<Vec<SignCall>>,
    pub script: RefCell<VecDeque<Scripted>>,
    /// When set, the same (role, message) signed twice gives two different signatures.
    pub nondeterministic: Cell<bool>,
    pub on_call: RefCell<Option<SignHook>>,
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
        self.calls.borrow_mut().push(call);
        match self.script.borrow_mut().pop_front() {
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
// The fixture
// ---------------------------------------------------------------------------

/// A configured instance: role keys, a bound mint, registered and verified nonce slots whose
/// accounts exist on the mock chain with distinct values.
pub struct Fixture {
    pub chain: MockChain,
    pub signer: MockSigner,
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
            c.keys = Some(RoleKeys { fee_payer: FEE_PAYER, mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER });
            c.mint = Some(MINT);
        });
        let fixture = Fixture { chain: MockChain::new(), signer: MockSigner::new() };
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
