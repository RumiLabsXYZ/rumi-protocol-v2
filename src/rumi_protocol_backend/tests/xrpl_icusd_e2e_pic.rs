//! XRPL icUSD lane: ledger-backed PocketIC end-to-end suite (Task 7).
//!
//! Real backend wasm + real ICRC-1/2/3 ledger + real XRC mock + PocketIC's built-in
//! threshold Ed25519 (`key_1`, mainnet-bound lane). The only mocked surface is XRPL
//! HTTPS: a STATEFUL rippled model (`World`) answers the JSON-RPC outcalls with raw
//! rippled shapes (the canister's own transforms reduce them), decodes and
//! signature-verifies every submitted blob, and applies it to a tiny ledger model
//! (issuer flags and sequence, trust-line balances, validated history).
//!
//! Unlike the native-XRP suite this file NEVER degrades silently: if signing or any
//! lifecycle step is unavailable the test panics. A pass means the full path ran.
//!
//! Run (absolute server path, pocket-ic crate 6.0.0 with a 7.0.0 server):
//!   POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_protocol_backend \
//!     --test xrpl_icusd_e2e_pic -- --test-threads=1

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Nat, Principal, Reserved};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::chains::xrp::account_id_from_classic_address;
use rumi_protocol_backend::chains::xrp::issued_amount::{ExactIssuedAmount, ICUSD_CURRENCY_HEX};
use rumi_protocol_backend::chains::xrp::{tx_hash, XrplNetwork};
use rumi_protocol_backend::xrpl_icusd::{
    BorrowToXrplArg, ConfigureXrplIcusdArg, CoverageReport, IssueFromCanonicalArg, IssuerPolicy,
    IssuerSetupStatus, ObserveRepaymentArg, OperationKindTag, OperationPage, OperationView,
    Phase, PrepareRepaymentArg, PublicXrplIcusdConfig, RepaymentIntentView, SetPausesArg,
    XrplIcusdError,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

// ─── Constants ───────────────────────────────────────────────────────────────

const E8: u64 = 100_000_000;
const XRP: u64 = 1_000_000;
const RESERVE_DROPS: u64 = 1_000_000;
/// Ledger transfer fee used by the real icUSD ledger in this suite.
const LEDGER_FEE: u64 = 10_000;
/// A valid classic address used as the XRPL holder / payer.
const HOLDER: &str = "rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo";
/// A second valid classic address (a stranger).
const STRANGER: &str = "rM2a5NiwBDRxoWCTnisrGaGGfmXC2w8FaW";

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}
fn user() -> Principal {
    Principal::from_slice(&[9, 9, 9, 9, 9, 9, 9, 9, 9])
}
fn other_user() -> Principal {
    Principal::from_slice(&[8, 8, 8, 8, 8, 8, 8, 8, 8])
}
fn liquidator() -> Principal {
    Principal::from_slice(&[7, 7, 7, 7, 7, 7, 7, 7, 7])
}
fn treasury() -> Principal {
    Principal::from_slice(&[6, 6, 6, 6, 6, 6, 6, 6, 6])
}
fn ctrl() -> Principal {
    Principal::from_slice(&[5, 5, 5, 5, 5, 5, 5, 5, 5])
}
fn xrp_collateral_principal() -> Principal {
    Principal::from_slice(b"rumi-xrp-native")
}

// ─── Candid mirrors (non-lane types) ─────────────────────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct UpgradeArg {
    mode: Option<Reserved>,
    description: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpVaultOpenInfo {
    vault_id: u64,
    custody_address: String,
    reserve_base_drops: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct CandidVault {
    owner: Principal,
    borrowed_icusd_amount: u64,
    icp_margin_amount: u64,
    vault_id: u64,
    collateral_amount: u64,
    collateral_type: Principal,
    accrued_interest: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct VaultArg {
    vault_id: u64,
    amount: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpClaimView {
    claimant: Principal,
    drops: u64,
    custody_owner: Principal,
    custody_nonce: u64,
    created_at_ns: u64,
}

#[derive(CandidType, Deserialize)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    trigger_threshold: u64,
    controller_id: Principal,
    max_transactions_per_response: Option<u64>,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize)]
struct MetadataValue {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "Nat")]
    nat: Option<Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize)]
struct LedgerInitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
}

#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    rates: HashMap<String, u64>,
}

/// The backend module under test. `RUMI_E2E_BACKEND_WASM` overrides the default
/// `target/.../release/rumi_protocol_backend.wasm`. The IC rejects a code section over
/// 10 MiB, so an artifact that large cannot be installed on PocketIC either (nor on
/// mainnet): the suite fails loudly instead of silently testing something else.
fn backend_wasm() -> Vec<u8> {
    let path = std::env::var("RUMI_E2E_BACKEND_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm"
        )
        .to_string()
    });
    std::fs::read(&path).unwrap_or_else(|e| panic!("read backend wasm {path}: {e}"))
}
fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}
fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}
fn flaky_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}
fn stability_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/stability_pool.wasm").to_vec()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stateful rippled model
// ═══════════════════════════════════════════════════════════════════════════════

fn ledger_hash_of(idx: u32) -> String {
    let mut h = Sha256::new();
    h.update(b"rumi-test-ledger");
    h.update(idx.to_be_bytes());
    hex::encode_upper(h.finalize())
}

fn amount_json(issuer: &str, v: &ExactIssuedAmount) -> Value {
    json!({ "currency": ICUSD_CURRENCY_HEX, "issuer": issuer, "value": v.to_decimal_string() })
}

fn exact(s: &str) -> ExactIssuedAmount {
    ExactIssuedAmount::parse(s).unwrap_or_else(|e| panic!("bad amount {s}: {e}"))
}

fn e8(v: u64) -> ExactIssuedAmount {
    ExactIssuedAmount::from_e8s(v)
}

#[derive(Clone, Debug)]
struct Line {
    limit: ExactIssuedAmount,
    balance: ExactIssuedAmount,
}

#[derive(Clone, Debug)]
struct HistTx {
    hash: String,
    ledger: u32,
    tx_type: &'static str,
    account: String,
    destination: Option<String>,
    tag: Option<u32>,
    flags: u32,
    result: String,
    amount: Option<Value>,
    delivered: Option<Value>,
    has_send_max: bool,
    changes: Vec<Value>,
}

#[derive(Clone, Debug, Default)]
struct Decoded {
    tx_type: u16,
    flags: u32,
    sequence: u32,
    tag: Option<u32>,
    lls: u32,
    set_flag: Option<u32>,
    transfer_rate: Option<u32>,
    account: [u8; 20],
    destination: Option<[u8; 20]>,
    amount: Option<(ExactIssuedAmount, [u8; 20], [u8; 20])>,
    fee_drops: u64,
    pubkey: Vec<u8>,
    signature: Vec<u8>,
    unsigned: Vec<u8>,
}

/// Minimal XRPL STObject decoder for the three transaction shapes the lane signs.
fn decode_blob(blob: &[u8]) -> Decoded {
    let mut d = Decoded::default();
    let mut i = 0usize;
    let mut sig_range: Option<(usize, usize)> = None;
    while i < blob.len() {
        let start = i;
        let b = blob[i];
        i += 1;
        let mut t = b >> 4;
        let mut f = b & 0x0f;
        if t == 0 {
            t = blob[i];
            i += 1;
        }
        if f == 0 {
            f = blob[i];
            i += 1;
        }
        match t {
            1 => {
                let v = u16::from_be_bytes([blob[i], blob[i + 1]]);
                i += 2;
                if f == 2 {
                    d.tx_type = v;
                }
            }
            2 => {
                let v = u32::from_be_bytes(blob[i..i + 4].try_into().unwrap());
                i += 4;
                match f {
                    2 => d.flags = v,
                    4 => d.sequence = v,
                    11 => d.transfer_rate = Some(v),
                    14 => d.tag = Some(v),
                    27 => d.lls = v,
                    33 => d.set_flag = Some(v),
                    _ => {}
                }
            }
            6 => {
                if blob[i] & 0x80 == 0 {
                    let raw = u64::from_be_bytes(blob[i..i + 8].try_into().unwrap());
                    i += 8;
                    if f == 8 {
                        d.fee_drops = raw & 0x3fff_ffff_ffff_ffff;
                    }
                } else {
                    let wire = u64::from_be_bytes(blob[i..i + 8].try_into().unwrap());
                    let cur: [u8; 20] = blob[i + 8..i + 28].try_into().unwrap();
                    let iss: [u8; 20] = blob[i + 28..i + 48].try_into().unwrap();
                    i += 48;
                    if f == 1 {
                        let amt = ExactIssuedAmount::from_wire_u64(wire).expect("wire amount");
                        d.amount = Some((amt, cur, iss));
                    }
                }
            }
            7 => {
                let len = blob[i] as usize;
                i += 1;
                let bytes = blob[i..i + len].to_vec();
                i += len;
                match f {
                    3 => d.pubkey = bytes,
                    4 => {
                        d.signature = bytes;
                        sig_range = Some((start, i));
                    }
                    _ => {}
                }
            }
            8 => {
                let len = blob[i] as usize;
                i += 1;
                let id: [u8; 20] = blob[i..i + len].try_into().expect("account id");
                i += len;
                match f {
                    1 => d.account = id,
                    3 => d.destination = Some(id),
                    _ => {}
                }
            }
            other => panic!("unexpected STObject type {other} field {f} in submitted blob"),
        }
    }
    let (s, e) = sig_range.expect("a signed blob carries TxnSignature");
    d.unsigned = [&blob[..s], &blob[e..]].concat();
    d
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmitMode {
    /// Accept, remember, and (if auto-validate) include in the next ledger.
    Include,
    /// Answer tesSUCCESS but never relay (the blob vanishes).
    Drop,
    /// Answer with an RPC error.
    Error,
}

struct World {
    issuer: String,
    ledger: u32,
    issuer_seq: u32,
    issuer_flags: u32,
    transfer_rate: Option<u32>,
    issuer_balance: u128,
    reserve: u64,
    base_fee: u64,
    holders: BTreeMap<String, Line>,
    history: Vec<HistTx>,
    pending: Vec<(String, Decoded)>,
    submits: Vec<(String, Vec<u8>, Decoded)>,
    auto_validate: bool,
    submit_mode: SubmitMode,
    complete_ledgers: Option<String>,
    custody_balance: u64,
    counts: BTreeMap<String, u32>,
    /// When set, the next `submit` request is NOT answered and `intercepted` flips.
    intercept_submit: bool,
    intercepted: bool,
    next_fake: u64,
}

impl World {
    fn new() -> Self {
        World {
            issuer: String::new(),
            ledger: 80_000_000,
            issuer_seq: 5,
            issuer_flags: 0,
            transfer_rate: None,
            issuer_balance: 200 * XRP as u128,
            reserve: RESERVE_DROPS,
            base_fee: 10,
            holders: BTreeMap::new(),
            history: vec![],
            pending: vec![],
            submits: vec![],
            auto_validate: true,
            submit_mode: SubmitMode::Include,
            complete_ledgers: None,
            custody_balance: 0,
            counts: BTreeMap::new(),
            intercept_submit: false,
            intercepted: false,
            next_fake: 1,
        }
    }

    fn complete(&self) -> String {
        self.complete_ledgers
            .clone()
            .unwrap_or_else(|| format!("1-{}", self.ledger))
    }

    fn fake_hash(&mut self) -> String {
        let n = self.next_fake;
        self.next_fake += 1;
        let mut h = Sha256::new();
        h.update(b"rumi-test-fake-tx");
        h.update(n.to_be_bytes());
        hex::encode_upper(h.finalize())
    }

    fn state_change(&self, holder: &str, prev: &ExactIssuedAmount, fin: &ExactIssuedAmount) -> Value {
        let hid = account_id_from_classic_address(holder).expect("holder id");
        let iid = account_id_from_classic_address(&self.issuer).expect("issuer id");
        let holder_low = hid < iid;
        let (low, high) = if holder_low { (holder, self.issuer.as_str()) } else { (self.issuer.as_str(), holder) };
        let to_low = |a: &ExactIssuedAmount| if holder_low { *a } else { a.negated() };
        json!({ "ModifiedNode": {
            "LedgerEntryType": "RippleState",
            "FinalFields": {
                "Balance": { "currency": ICUSD_CURRENCY_HEX, "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": to_low(fin).to_decimal_string() },
                "LowLimit": { "currency": ICUSD_CURRENCY_HEX, "issuer": low, "value": "0" },
                "HighLimit": { "currency": ICUSD_CURRENCY_HEX, "issuer": high, "value": "0" }
            },
            "PreviousFields": { "Balance": { "value": to_low(prev).to_decimal_string() } }
        }})
    }

    /// Include the decoded tx in a fresh validated ledger. Returns the ledger index.
    fn include(&mut self, hash: &str, d: &Decoded) -> u32 {
        self.ledger += 1;
        let idx = self.ledger;
        let issuer_id = account_id_from_classic_address(&self.issuer).unwrap();
        assert_eq!(d.account, issuer_id, "only issuer-signed blobs are submitted by the lane");
        let mut result = "tesSUCCESS".to_string();
        if d.sequence != self.issuer_seq {
            result = "tefPAST_SEQ".into();
        }
        let issuer = self.issuer.clone();
        match d.tx_type {
            3 => {
                if result == "tesSUCCESS" {
                    if let Some(f) = d.set_flag {
                        self.issuer_flags |= match f {
                            8 => 0x0080_0000,
                            1 => 0x0002_0000,
                            6 => 0x0020_0000,
                            16 => 0x8000_0000,
                            other => panic!("unexpected SetFlag {other}"),
                        };
                    }
                    if let Some(r) = d.transfer_rate {
                        self.transfer_rate = if r == 1_000_000_000 { None } else { Some(r) };
                    }
                    self.issuer_seq += 1;
                    self.issuer_balance -= u128::from(d.fee_drops);
                }
                self.history.push(HistTx {
                    hash: hash.into(),
                    ledger: idx,
                    tx_type: "AccountSet",
                    account: issuer,
                    destination: None,
                    tag: None,
                    flags: d.flags,
                    result,
                    amount: None,
                    delivered: None,
                    has_send_max: false,
                    changes: vec![],
                });
            }
            0 => {
                let dest_id = d.destination.expect("payment destination");
                let dest = self
                    .holders
                    .keys()
                    .find(|a| account_id_from_classic_address(a).unwrap() == dest_id)
                    .cloned();
                let (amt, _, _) = d.amount.expect("payment amount");
                let mut changes = vec![];
                let mut dest_s = String::new();
                if result == "tesSUCCESS" {
                    match dest {
                        Some(h) => {
                            let line = self.holders.get_mut(&h).unwrap();
                            let prev = line.balance;
                            let next = prev.checked_add(&amt).unwrap();
                            if next > line.limit {
                                result = "tecPATH_PARTIAL".into();
                            } else {
                                line.balance = next;
                                changes.push(self.state_change(&h, &prev, &next));
                            }
                            dest_s = h;
                        }
                        None => result = "tecNO_LINE".into(),
                    }
                    if result == "tesSUCCESS" || result.starts_with("tec") {
                        self.issuer_seq += 1;
                        self.issuer_balance -= u128::from(d.fee_drops);
                    }
                }
                self.history.push(HistTx {
                    hash: hash.into(),
                    ledger: idx,
                    tx_type: "Payment",
                    account: issuer.clone(),
                    destination: Some(dest_s),
                    tag: d.tag,
                    flags: d.flags,
                    result: result.clone(),
                    amount: Some(amount_json(&issuer, &amt)),
                    delivered: if result == "tesSUCCESS" { Some(amount_json(&issuer, &amt)) } else { None },
                    has_send_max: false,
                    changes,
                });
            }
            other => panic!("unexpected submitted tx type {other}"),
        }
        idx
    }

    fn validate_pending(&mut self) {
        let pend = std::mem::take(&mut self.pending);
        for (hash, d) in pend {
            self.include(&hash, &d);
        }
    }

    fn tx_json(&self, t: &HistTx) -> (Value, Value) {
        let mut txj = json!({
            "TransactionType": t.tx_type,
            "Account": t.account,
            "Flags": t.flags,
        });
        if let Some(dst) = &t.destination {
            txj["Destination"] = json!(dst);
        }
        if let Some(tag) = t.tag {
            txj["DestinationTag"] = json!(tag);
        }
        if let Some(a) = &t.amount {
            txj["Amount"] = a.clone();
        }
        if t.has_send_max {
            txj["SendMax"] = json!("1");
        }
        let mut meta = json!({
            "TransactionResult": t.result,
            "AffectedNodes": t.changes,
        });
        if let Some(d) = &t.delivered {
            meta["delivered_amount"] = d.clone();
        }
        (txj, meta)
    }

    fn holder_obligation(&self) -> ExactIssuedAmount {
        let mut tot = ExactIssuedAmount::ZERO;
        for l in self.holders.values() {
            tot = tot.checked_add(&l.balance).unwrap();
        }
        tot
    }

    fn handle(&mut self, method: &str, p: &Value) -> Option<Value> {
        *self.counts.entry(method.to_string()).or_default() += 1;
        match method {
            "server_state" => Some(json!({ "result": { "state": { "validated_ledger": { "reserve_base": self.reserve } } } })),
            "fee" => Some(json!({ "result": { "drops": { "base_fee": self.base_fee.to_string() } } })),
            "server_info" => Some(json!({ "result": { "info": {
                "complete_ledgers": self.complete(),
                "validated_ledger": { "seq": self.ledger, "hash": ledger_hash_of(self.ledger) }
            } } })),
            "account_info" => {
                let acct = p.get("account").and_then(Value::as_str).unwrap_or_default();
                if acct == self.issuer {
                    let mut data = json!({
                        "Sequence": self.issuer_seq,
                        "OwnerCount": 0,
                        "Flags": self.issuer_flags,
                        "Balance": self.issuer_balance.to_string(),
                    });
                    if let Some(r) = self.transfer_rate {
                        data["TransferRate"] = json!(r);
                    }
                    Some(json!({ "result": { "account_data": data, "ledger_index": self.ledger, "validated": true } }))
                } else {
                    Some(json!({ "result": {
                        "account_data": { "Sequence": 1, "Balance": self.custody_balance.to_string() },
                        "ledger_index": self.ledger, "ledger_current_index": self.ledger, "validated": true
                    } }))
                }
            }
            "account_objects" => Some(json!({ "result": { "account_objects": [], "validated": true } })),
            "account_lines" => {
                let acct = p.get("account").and_then(Value::as_str).unwrap_or_default().to_string();
                let idx = p.get("ledger_index").and_then(Value::as_u64).map(|n| n as u32).unwrap_or(self.ledger);
                let lines: Vec<Value> = if acct == self.issuer {
                    self.holders
                        .iter()
                        .map(|(h, l)| json!({
                            "account": h,
                            "balance": l.balance.negated().to_decimal_string(),
                            "currency": ICUSD_CURRENCY_HEX,
                            "limit": "0",
                            "limit_peer": l.limit.to_decimal_string(),
                            "no_ripple": false, "no_ripple_peer": false,
                        }))
                        .collect()
                } else {
                    self.holders
                        .get(&acct)
                        .map(|l| vec![json!({
                            "account": self.issuer,
                            "balance": l.balance.to_decimal_string(),
                            "currency": ICUSD_CURRENCY_HEX,
                            "limit": l.limit.to_decimal_string(),
                            "limit_peer": "0",
                            "no_ripple": false, "no_ripple_peer": false,
                        })])
                        .unwrap_or_default()
                };
                Some(json!({ "result": {
                    "account": acct, "ledger_index": idx, "ledger_hash": ledger_hash_of(idx),
                    "validated": true, "lines": lines
                } }))
            }
            "tx" => {
                let hash = p.get("transaction").and_then(Value::as_str).unwrap_or_default().to_ascii_uppercase();
                if let Some(t) = self.history.iter().find(|t| t.hash == hash) {
                    let (txj, meta) = self.tx_json(t);
                    return Some(json!({ "result": {
                        "tx_json": txj, "hash": t.hash, "ledger_index": t.ledger,
                        "ledger_hash": ledger_hash_of(t.ledger), "validated": true, "meta": meta
                    } }));
                }
                if let Some((h, d)) = self.pending.iter().find(|(h, _)| *h == hash) {
                    let _ = d;
                    return Some(json!({ "result": {
                        "tx_json": { "TransactionType": "Payment", "Account": self.issuer },
                        "hash": h, "validated": false, "ledger_index": 0
                    } }));
                }
                Some(json!({ "result": { "error": "txnNotFound", "status": "error" } }))
            }
            "submit" => {
                if self.intercept_submit {
                    self.intercepted = true;
                    return None;
                }
                let blob_hex = p.get("tx_blob").and_then(Value::as_str).unwrap_or_default();
                let blob = hex::decode(blob_hex).expect("submit blob hex");
                let hash = hex::encode_upper(tx_hash(&blob));
                let d = decode_blob(&blob);
                // Verify the threshold signature host-side: the issuer really signed it.
                let msg = [&[0x53, 0x54, 0x58, 0x00][..], &d.unsigned].concat();
                assert_eq!(d.pubkey.len(), 33, "ed25519 signing key carries the 0xED prefix");
                assert_eq!(d.pubkey[0], 0xED);
                let vk = VerifyingKey::from_bytes(d.pubkey[1..].try_into().unwrap()).expect("pubkey");
                let sig = Signature::from_slice(&d.signature).expect("signature bytes");
                vk.verify(&msg, &sig).expect("issuer signature must verify");
                if !self.submits.iter().any(|(h, _, _)| *h == hash) {
                    self.submits.push((hash.clone(), blob, d.clone()));
                }
                match self.submit_mode {
                    SubmitMode::Error => return Some(json!({ "result": { "error": "noNetwork", "status": "error" } })),
                    SubmitMode::Drop => {}
                    SubmitMode::Include => {
                        let known = self.pending.iter().any(|(h, _)| *h == hash)
                            || self.history.iter().any(|t| t.hash == hash);
                        if !known {
                            if self.auto_validate {
                                self.include(&hash, &d);
                            } else {
                                self.pending.push((hash.clone(), d));
                            }
                        }
                    }
                }
                Some(json!({ "result": { "engine_result": "tesSUCCESS", "tx_json": { "hash": hash } } }))
            }
            "account_tx" => {
                let min = p.get("ledger_index_min").and_then(Value::as_u64).unwrap_or(0) as u32;
                let max = p.get("ledger_index_max").and_then(Value::as_u64).unwrap_or(0) as u32;
                let txs: Vec<Value> = self
                    .history
                    .iter()
                    .filter(|t| t.ledger >= min && t.ledger <= max)
                    .map(|t| {
                        let (txj, meta) = self.tx_json(t);
                        json!({
                            "tx_json": txj, "meta": meta, "hash": t.hash, "ledger_index": t.ledger,
                            "ledger_hash": ledger_hash_of(t.ledger), "validated": true
                        })
                    })
                    .collect();
                Some(json!({ "result": {
                    "account": self.issuer, "ledger_index_min": min, "ledger_index_max": max,
                    "transactions": txs, "validated": true
                } }))
            }
            other => panic!("unexpected rippled method {other}: {p}"),
        }
    }
}

/// Handle to the shared world plus the HTTP pump.
#[derive(Clone)]
struct Xrpl {
    w: Rc<RefCell<World>>,
}

#[derive(Clone, Debug)]
struct ReturnOpts {
    holder: String,
    tag: Option<u32>,
    nominal: String,
    delivered: String,
    flags: u32,
    send_max: bool,
}

impl ReturnOpts {
    fn simple(holder: &str, tag: u32, amount: &str) -> Self {
        ReturnOpts {
            holder: holder.into(),
            tag: Some(tag),
            nominal: amount.into(),
            delivered: amount.into(),
            flags: 0,
            send_max: false,
        }
    }
}

impl Xrpl {
    fn new() -> Self {
        Xrpl { w: Rc::new(RefCell::new(World::new())) }
    }

    fn set_issuer(&self, issuer: &str) {
        self.w.borrow_mut().issuer = issuer.to_string();
    }

    fn add_holder(&self, addr: &str, limit: &str) {
        self.w.borrow_mut().holders.insert(
            addr.into(),
            Line { limit: exact(limit), balance: ExactIssuedAmount::ZERO },
        );
    }

    fn holder_balance(&self, addr: &str) -> ExactIssuedAmount {
        self.w.borrow().holders.get(addr).map(|l| l.balance).unwrap_or(ExactIssuedAmount::ZERO)
    }

    fn obligation(&self) -> ExactIssuedAmount {
        self.w.borrow().holder_obligation()
    }

    fn advance_ledgers(&self, n: u32) {
        self.w.borrow_mut().ledger += n;
    }

    fn count(&self, method: &str) -> u32 {
        self.w.borrow().counts.get(method).copied().unwrap_or(0)
    }

    fn total_http(&self) -> u32 {
        self.w.borrow().counts.values().sum()
    }

    fn submits(&self) -> Vec<(String, Vec<u8>, Decoded)> {
        self.w.borrow().submits.clone()
    }

    fn payments_submitted(&self) -> Vec<(String, Decoded)> {
        self.submits().into_iter().filter(|(_, _, d)| d.tx_type == 0).map(|(h, _, d)| (h, d)).collect()
    }

    fn set_submit_mode(&self, m: SubmitMode) {
        self.w.borrow_mut().submit_mode = m;
    }

    fn set_auto_validate(&self, v: bool) {
        self.w.borrow_mut().auto_validate = v;
    }

    fn validate_pending(&self) {
        self.w.borrow_mut().validate_pending();
    }

    fn set_complete_ledgers(&self, s: Option<&str>) {
        self.w.borrow_mut().complete_ledgers = s.map(String::from);
    }

    fn set_intercept_submit(&self, on: bool) {
        let mut w = self.w.borrow_mut();
        w.intercept_submit = on;
        w.intercepted = false;
    }

    fn intercepted(&self) -> bool {
        self.w.borrow().intercepted
    }

    fn ledger(&self) -> u32 {
        self.w.borrow().ledger
    }

    /// A holder-to-issuer payment validated in a fresh ledger. Returns the tx hash.
    fn holder_pays(&self, o: &ReturnOpts) -> String {
        let mut w = self.w.borrow_mut();
        let hash = w.fake_hash();
        w.ledger += 1;
        let idx = w.ledger;
        let issuer = w.issuer.clone();
        let delivered = exact(&o.delivered);
        let mut changes = vec![];
        if let Some(line) = w.holders.get(&o.holder).cloned() {
            let prev = line.balance;
            let next = prev.checked_sub(&delivered).unwrap_or(ExactIssuedAmount::ZERO);
            w.holders.get_mut(&o.holder).unwrap().balance = next;
            changes.push(w.state_change(&o.holder, &prev, &next));
        }
        w.history.push(HistTx {
            hash: hash.clone(),
            ledger: idx,
            tx_type: "Payment",
            account: o.holder.clone(),
            destination: Some(issuer.clone()),
            tag: o.tag,
            flags: o.flags,
            result: "tesSUCCESS".into(),
            amount: Some(amount_json(&issuer, &exact(&o.nominal))),
            delivered: Some(amount_json(&issuer, &delivered)),
            has_send_max: o.send_max,
            changes,
        });
        hash
    }

    /// Answer every parked HTTP request. Returns how many were answered.
    fn serve(&self, pic: &PocketIc) -> usize {
        let reqs = pic.get_canister_http();
        let mut n = 0;
        for req in reqs {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let method = v.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
            let params = v.get("params").and_then(|p| p.get(0)).cloned().unwrap_or(Value::Null);
            let resp = self.w.borrow_mut().handle(&method, &params);
            let Some(resp) = resp else { continue };
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: req.subnet_id,
                request_id: req.request_id,
                response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
                    status: 200,
                    headers: vec![],
                    body: resp.to_string().into_bytes(),
                }),
                additional_responses: vec![],
            });
            n += 1;
        }
        n
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fixture
// ═══════════════════════════════════════════════════════════════════════════════

struct Fx {
    pic: PocketIc,
    backend: Principal,
    icusd: Principal,
    icp: Principal,
    xrc: Principal,
    x: Xrpl,
}

/// Idle ticks (no parked HTTP request) after which a pumped call is considered quiet.
const IDLE_TICKS: u32 = 30;

fn install_ledger(pic: &PocketIc, ledger: Principal, minter: Principal, name: &str) {
    let args = LedgerArg::Init(LedgerInitArgs {
        minting_account: Account { owner: minter, subaccount: None },
        fee_collector_account: None,
        transfer_fee: Nat::from(LEDGER_FEE),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: name.to_string(),
        token_symbol: name.to_string(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1000,
            trigger_threshold: 2000,
            controller_id: minter,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    });
    pic.install_canister(ledger, ledger_wasm(), encode_one(args).expect("ledger init"), None);
}

fn boot() -> Fx {
    boot_with(false)
}

/// `flaky = true` installs the fault-injecting `flaky_ledger` as the icUSD ledger (minting
/// account = backend, burn-aware) so reply loss and `TooOld` can be injected.
fn boot_with(flaky: bool) -> Fx {
    let pic = PocketIcBuilder::new().with_ii_subnet().with_application_subnet().build();
    let mk = |pic: &PocketIc| {
        let c = pic.create_canister();
        pic.add_cycles(c, 100_000_000_000_000);
        c
    };
    let backend = mk(&pic);
    let icusd = mk(&pic);
    let icp = mk(&pic);
    let xrc = mk(&pic);
    if flaky {
        pic.install_canister(icusd, flaky_wasm(), Encode!().unwrap(), None);
        pic.update_call(icusd, Principal::anonymous(), "set_minter", Encode!(&Some(backend)).unwrap())
            .expect("set_minter");
    } else {
        install_ledger(&pic, icusd, backend, "icUSD");
    }
    install_ledger(&pic, icp, backend, "ICP");
    let mut rates = HashMap::new();
    rates.insert("ICP/USD".to_string(), 10 * E8);
    rates.insert("XRP/USD".to_string(), E8 / 2);
    pic.install_canister(xrc, xrc_wasm(), encode_one(MockXRC { rates }).unwrap(), None);
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd,
        icp_ledger_principal: icp,
        fee_e8s: LEDGER_FEE,
        developer_principal: dev(),
        treasury_principal: Some(treasury()),
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(backend, backend_wasm(), encode_args((init,)).unwrap(), None);
    pic.set_controllers(backend, None, vec![ctrl()]).expect("set controllers");
    for _ in 0..5 {
        pic.tick();
    }
    Fx { pic, backend, icusd, icp, xrc, x: Xrpl::new() }
}

impl Fx {
    /// Submit an update and pump HTTP + ticks until it is quiet, then await it.
    fn call(&self, sender: Principal, method: &str, args: Vec<u8>) -> WasmResult {
        let msg = self
            .pic
            .submit_call(self.backend, sender, method, args)
            .unwrap_or_else(|e| panic!("submit {method}: {e}"));
        let mut idle = 0;
        for _ in 0..2000 {
            self.pic.tick();
            if self.x.serve(&self.pic) == 0 {
                idle += 1;
                if idle >= IDLE_TICKS {
                    break;
                }
            } else {
                idle = 0;
            }
        }
        self.pic.await_call(msg).unwrap_or_else(|e| panic!("await {method}: {e}"))
    }

    /// Like `call`, but returns early (leaving the call in flight) once `stop` is true.
    /// Returns the message id and whether `stop` fired.
    fn call_until(
        &self,
        sender: Principal,
        method: &str,
        args: Vec<u8>,
        mut stop: impl FnMut(&Fx) -> bool,
    ) -> (pocket_ic::common::rest::RawMessageId, bool) {
        let msg = self
            .pic
            .submit_call(self.backend, sender, method, args)
            .unwrap_or_else(|e| panic!("submit {method}: {e}"));
        let mut idle = 0;
        for _ in 0..2000 {
            self.pic.tick();
            if stop(self) {
                return (msg, true);
            }
            if self.x.serve(&self.pic) == 0 {
                idle += 1;
                if idle >= IDLE_TICKS {
                    break;
                }
            } else {
                idle = 0;
            }
            if stop(self) {
                return (msg, true);
            }
        }
        (msg, false)
    }

    fn finish(&self, msg: pocket_ic::common::rest::RawMessageId) -> Result<WasmResult, String> {
        // Keep answering HTTP for a bounded number of ticks, then await.
        let mut idle = 0;
        for _ in 0..2000 {
            self.pic.tick();
            if self.x.serve(&self.pic) == 0 {
                idle += 1;
                if idle >= IDLE_TICKS {
                    break;
                }
            } else {
                idle = 0;
            }
        }
        self.pic.await_call(msg).map_err(|e| e.to_string())
    }

    /// Typed pumped call.
    fn lane<T>(&self, sender: Principal, method: &str, args: Vec<u8>) -> T
    where
        T: CandidType + for<'a> Deserialize<'a>,
    {
        match self.call(sender, method, args) {
            WasmResult::Reply(b) => Decode!(&b, T).unwrap_or_else(|e| panic!("decode {method}: {e}")),
            WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
        }
    }

    fn lane_ok(&self, sender: Principal, method: &str, args: Vec<u8>) -> OperationView {
        let r: Result<OperationView, XrplIcusdError> = self.lane(sender, method, args);
        r.unwrap_or_else(|e| panic!("{method} failed: {e:?}"))
    }

    fn query<T>(&self, sender: Principal, method: &str, args: Vec<u8>) -> T
    where
        T: CandidType + for<'a> Deserialize<'a>,
    {
        match self.pic.query_call(self.backend, sender, method, args).expect("query call") {
            WasmResult::Reply(b) => Decode!(&b, T).unwrap_or_else(|e| panic!("decode {method}: {e}")),
            WasmResult::Reject(m) => panic!("query {method} rejected: {m}"),
        }
    }

    /// Advance time and keep servicing timers + HTTP for `ticks` rounds.
    fn run(&self, secs: u64, ticks: u32) {
        if secs > 0 {
            self.pic.advance_time(Duration::from_secs(secs));
        }
        for _ in 0..ticks {
            self.pic.tick();
            self.x.serve(&self.pic);
        }
    }

    /// Step over the per-caller new-request throttle.
    fn gap(&self) {
        self.run(12, 4);
    }

    // ── plain (non-HTTP) updates ──

    fn plain(&self, sender: Principal, method: &str, args: Vec<u8>) -> WasmResult {
        self.pic
            .update_call(self.backend, sender, method, args)
            .unwrap_or_else(|e| panic!("update {method}: {e}"))
    }

    fn plain_ok<T>(&self, sender: Principal, method: &str, args: Vec<u8>) -> T
    where
        T: CandidType + for<'a> Deserialize<'a>,
    {
        match self.plain(sender, method, args) {
            WasmResult::Reply(b) => {
                let r = Decode!(&b, Result<T, Reserved>).unwrap_or_else(|e| panic!("decode {method}: {e}"));
                r.unwrap_or_else(|_| panic!("{method} returned Err"))
            }
            WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
        }
    }

    // ── ledger reads ──

    fn bal(&self, owner: Principal, sub: Option<[u8; 32]>) -> u64 {
        let n: Nat = match self
            .pic
            .query_call(
                self.icusd,
                Principal::anonymous(),
                "icrc1_balance_of",
                Encode!(&Account { owner, subaccount: sub }).unwrap(),
            )
            .expect("balance_of")
        {
            WasmResult::Reply(b) => Decode!(&b, Nat).unwrap(),
            WasmResult::Reject(m) => panic!("{m}"),
        };
        u64::try_from(n.0).unwrap()
    }

    fn supply(&self) -> u64 {
        let n: Nat = match self
            .pic
            .query_call(self.icusd, Principal::anonymous(), "icrc1_total_supply", Encode!().unwrap())
            .expect("supply")
        {
            WasmResult::Reply(b) => Decode!(&b, Nat).unwrap(),
            WasmResult::Reject(m) => panic!("{m}"),
        };
        u64::try_from(n.0).unwrap()
    }

    fn escrow_account(&self) -> Account {
        self.config().escrow_account
    }

    fn escrow_bal(&self) -> u64 {
        let a = self.escrow_account();
        self.bal(a.owner, a.subaccount)
    }

    fn log_length(&self) -> u64 {
        let r = GetBlocksRequest { start: Nat::from(0u8), length: Nat::from(1u8) };
        let res: GetBlocksResult = match self
            .pic
            .query_call(self.icusd, Principal::anonymous(), "icrc3_get_blocks", encode_args((vec![r],)).unwrap())
            .expect("icrc3")
        {
            WasmResult::Reply(b) => candid::decode_one(&b).unwrap(),
            WasmResult::Reject(m) => panic!("{m}"),
        };
        u64::try_from(res.log_length.0).unwrap()
    }

    /// `(op, amount)` of one ledger block (`mint`, `burn`, `xfer`, ...).
    fn block(&self, idx: u64) -> (String, u128) {
        let r = GetBlocksRequest { start: Nat::from(idx), length: Nat::from(1u8) };
        let res: GetBlocksResult = match self
            .pic
            .query_call(self.icusd, Principal::anonymous(), "icrc3_get_blocks", encode_args((vec![r],)).unwrap())
            .expect("icrc3")
        {
            WasmResult::Reply(b) => candid::decode_one(&b).unwrap(),
            WasmResult::Reject(m) => panic!("{m}"),
        };
        let blk = res.blocks.first().expect("block present").block.clone();
        let ICRC3Value::Map(m) = blk else { panic!("block is a map") };
        let tx = match m.get("tx") {
            Some(ICRC3Value::Map(tx)) => tx,
            other => panic!("tx {other:?}"),
        };
        let btype = match tx.get("op") {
            Some(ICRC3Value::Text(t)) => t.clone(),
            other => panic!("op {other:?}"),
        };
        let amt = match tx.get("amt") {
            Some(ICRC3Value::Nat(n)) => u128::try_from(n.0.clone()).unwrap(),
            other => panic!("amt {other:?}"),
        };
        (btype, amt)
    }

    // ── lane reads ──

    fn config(&self) -> PublicXrplIcusdConfig {
        self.query(Principal::anonymous(), "get_xrpl_icusd_config", Encode!().unwrap())
    }

    fn ops(&self, who: Principal) -> Vec<OperationView> {
        let page: OperationPage = self.query(who, "get_my_xrpl_operations", Encode!(&Option::<u64>::None, &50u32).unwrap());
        page.items
    }

    fn op(&self, who: Principal, id: u64) -> OperationView {
        self.ops(who).into_iter().find(|o| o.operation_id == id).unwrap_or_else(|| panic!("op {id} missing"))
    }

    fn vault(&self, who: Principal, id: u64) -> Option<CandidVault> {
        let v: Vec<CandidVault> = self.query(Principal::anonymous(), "get_vaults", Encode!(&Some(who)).unwrap());
        v.into_iter().find(|v| v.vault_id == id)
    }

    fn coverage(&self) -> CoverageReport {
        let r: Result<CoverageReport, XrplIcusdError> = self.lane(ctrl(), "get_xrpl_icusd_coverage", Encode!().unwrap());
        r.expect("coverage")
    }

    // ── token plumbing ──

    fn transfer_icusd(&self, from: Principal, to: Principal, amount: u64) {
        let args = icrc_ledger_types::icrc1::transfer::TransferArg {
            from_subaccount: None,
            to: Account { owner: to, subaccount: None },
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(amount),
        };
        match self.pic.update_call(self.icusd, from, "icrc1_transfer", Encode!(&args).unwrap()).unwrap() {
            WasmResult::Reply(b) => {
                Decode!(&b, Result<Nat, Reserved>).unwrap().unwrap_or_else(|_| panic!("transfer failed"));
            }
            WasmResult::Reject(m) => panic!("{m}"),
        }
    }

    fn mint_icusd(&self, to: Principal, amount: u64) {
        self.transfer_icusd(self.backend, to, amount);
    }

    fn approve(&self, owner: Principal, spender: Principal, amount: u64) {
        let args = ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: spender, subaccount: None },
            amount: Nat::from(amount),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        match self.pic.update_call(self.icusd, owner, "icrc2_approve", Encode!(&args).unwrap()).unwrap() {
            WasmResult::Reply(b) => {
                Decode!(&b, Result<Nat, Reserved>).unwrap().unwrap_or_else(|_| panic!("approve failed"));
            }
            WasmResult::Reject(m) => panic!("{m}"),
        }
    }

    fn set_xrp_price(&self, price_e8: u64) {
        match self
            .pic
            .update_call(
                self.xrc,
                Principal::anonymous(),
                "set_exchange_rate",
                Encode!(&"XRP".to_string(), &"USD".to_string(), &price_e8).unwrap(),
            )
            .unwrap()
        {
            WasmResult::Reject(m) => panic!("set_exchange_rate: {m}"),
            _ => {}
        }
        self.pic.advance_time(Duration::from_secs(120));
        for _ in 0..3 {
            self.pic.tick();
        }
    }

    // ── XRP collateral + vault ──

    fn register_xrp(&self) {
        self.plain_ok::<()>(dev(), "set_xrp_schnorr_key_name", Encode!(&"key_1".to_string()).unwrap());
        self.plain_ok::<()>(dev(), "register_xrp_collateral", Encode!().unwrap());
    }

    fn open_and_fund_vault(&self, owner: Principal, deposit_xrp: u64) -> u64 {
        let reply = self.call(owner, "open_xrp_vault", Encode!().unwrap());
        let open: XrpVaultOpenInfo = match reply {
            WasmResult::Reply(b) => Decode!(&b, Result<XrpVaultOpenInfo, Reserved>)
                .unwrap()
                .unwrap_or_else(|_| panic!("open_xrp_vault failed (threshold Ed25519 must be available)")),
            WasmResult::Reject(m) => panic!("open_xrp_vault rejected: {m}"),
        };
        assert!(open.custody_address.starts_with('r'));
        self.x.w.borrow_mut().custody_balance = deposit_xrp * XRP;
        let credited = match self.call(owner, "confirm_xrp_deposit", Encode!(&open.vault_id).unwrap()) {
            WasmResult::Reply(b) => Decode!(&b, Result<u64, Reserved>)
                .unwrap()
                .unwrap_or_else(|_| panic!("confirm_xrp_deposit failed")),
            WasmResult::Reject(m) => panic!("{m}"),
        };
        assert_eq!(credited, deposit_xrp * XRP - RESERVE_DROPS);
        open.vault_id
    }

    /// Fixture economics: 1% borrowing fee on XRP, zero interest.
    fn zero_interest_one_percent_fee(&self) {
        // No CR-based fee multiplier: fee is exactly 1% of gross.
        self.plain_ok::<()>(dev(), "set_borrowing_fee_curve", Encode!(&Option::<String>::None).unwrap());
        self.plain_ok::<()>(dev(), "set_collateral_borrowing_fee", Encode!(&xrp_collateral_principal(), &0.01f64).unwrap());
        self.plain_ok::<()>(dev(), "set_interest_rate", Encode!(&xrp_collateral_principal(), &0.0f64).unwrap());
    }

    // ── XRPL lane configuration (controller flow) ──

    fn lane_setup(&self) {
        let cfg: Result<PublicXrplIcusdConfig, XrplIcusdError> = self.lane(
            ctrl(),
            "configure_xrpl_icusd",
            Encode!(&ConfigureXrplIcusdArg {
                network: Some(XrplNetwork::Mainnet),
                policy: Some(IssuerPolicy { allow_clawback: false, no_freeze: false }),
                enabled: Some(true),
                outstanding_cap_e8s: Some(1_000_000 * E8),
                per_operation_cap_e8s: Some(1_000_000 * E8),
                rounding_buffer_e8s: Some(0),
                max_fee_drops: Some(1_000),
            })
            .unwrap(),
        );
        let cfg = cfg.expect("configure_xrpl_icusd");
        let issuer = cfg.issuer_address.clone().expect("issuer address derived by threshold Ed25519");
        assert!(issuer.starts_with('r'));
        assert!(!cfg.ready, "not ready before setup");
        self.x.set_issuer(&issuer);
        let acct: Result<Account, XrplIcusdError> = self.lane(ctrl(), "verify_xrpl_icusd_minting_account", Encode!().unwrap());
        assert_eq!(acct.expect("minting account verified").owner, self.backend);
        for step in 0..12 {
            let st: Result<IssuerSetupStatus, XrplIcusdError> = self.lane(ctrl(), "run_xrpl_issuer_setup_step", Encode!().unwrap());
            let st = st.unwrap_or_else(|e| panic!("setup step {step}: {e:?}"));
            if format!("{:?}", st.setup).starts_with("Ready") {
                break;
            }
            self.run(1, 2);
        }
        let cfg = self.config();
        assert!(format!("{:?}", cfg.issuer_setup).starts_with("Ready"), "issuer setup reached Ready: {:?}", cfg.issuer_setup);
        let r: Result<PublicXrplIcusdConfig, XrplIcusdError> = self.lane(
            ctrl(),
            "set_xrpl_icusd_pauses",
            Encode!(&SetPausesArg { issuance_paused: Some(false), settlement_paused: Some(false) }).unwrap(),
        );
        assert!(r.expect("pauses").ready);
    }

    fn borrow_to_xrpl(&self, vault: u64, gross: u64, nonce: u8) -> Result<OperationView, XrplIcusdError> {
        self.lane(
            user(),
            "borrow_to_xrpl",
            Encode!(&BorrowToXrplArg {
                request_nonce: vec![nonce; 32],
                recipient: HOLDER.to_string(),
                network: XrplNetwork::Mainnet,
                vault_id: vault,
                gross_e8s: gross,
            })
            .unwrap(),
        )
    }

    fn prepare_repay(&self, vault: u64, max: u64, full: bool, nonce: u8, top_up: Option<u64>) -> Result<RepaymentIntentView, XrplIcusdError> {
        self.lane(
            user(),
            "prepare_xrpl_repayment",
            Encode!(&PrepareRepaymentArg {
                request_nonce: vec![nonce; 32],
                network: XrplNetwork::Mainnet,
                vault_id: vault,
                max_e8s: max,
                full_close: full,
                payer: HOLDER.to_string(),
                top_up_operation: top_up,
            })
            .unwrap(),
        )
    }

    fn observe(&self, op: u64, hash: &str) -> Result<OperationView, XrplIcusdError> {
        self.lane(
            user(),
            "observe_xrpl_repayment",
            Encode!(&ObserveRepaymentArg { tx_hash: hash.to_string(), operation_id: op }).unwrap(),
        )
    }

    fn resume(&self, op: u64) -> Result<OperationView, XrplIcusdError> {
        self.lane(user(), "resume_xrpl_operation", Encode!(&op).unwrap())
    }

    /// Update call on the (flaky) icUSD ledger's test controls.
    fn flaky(&self, method: &str, args: Vec<u8>) {
        match self.pic.update_call(self.icusd, Principal::anonymous(), method, args).unwrap() {
            WasmResult::Reply(_) => {}
            WasmResult::Reject(m) => panic!("flaky {method}: {m}"),
        }
    }

    /// Upgrade the backend with the same wasm (an `Upgrade` argument).
    fn upgrade_backend(&self) {
        let arg = ProtocolArg::Upgrade(UpgradeArg { mode: None, description: Some("e2e upgrade".into()) });
        self.pic
            .upgrade_canister(self.backend, backend_wasm(), encode_args((arg,)).unwrap(), Some(ctrl()))
            .expect("upgrade backend");
        for _ in 0..3 {
            self.pic.tick();
        }
    }
}

fn phase_is(v: &OperationView, p: Phase) -> bool {
    v.phase == p
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Lane bring-up smoke: configuration, minting-account proof, and issuer setup to
/// Ready through signed AccountSet transactions against the rippled model.
#[test]
fn lane_setup_reaches_ready_with_signed_account_sets() {
    let fx = boot();
    fx.register_xrp();
    fx.lane_setup();
    let cfg = fx.config();
    assert!(cfg.ready);
    assert_eq!(cfg.verified_minting_account.unwrap().owner, fx.backend);
    let w = fx.x.w.borrow();
    // DefaultRipple + RequireDest are on; clawback and NoFreeze stay off (policy).
    assert_eq!(w.issuer_flags & 0x0080_0000, 0x0080_0000);
    assert_eq!(w.issuer_flags & 0x0002_0000, 0x0002_0000);
    assert_eq!(w.issuer_flags & 0x8000_0000, 0);
    assert_eq!(w.issuer_flags & 0x0020_0000, 0);
    assert!(w.submits.iter().all(|(_, _, d)| d.tx_type == 3), "setup submits only AccountSets");
}

// ─── assertion helpers ───────────────────────────────────────────────────────

fn issuer_id(fx: &Fx) -> [u8; 20] {
    account_id_from_classic_address(&fx.config().issuer_address.unwrap()).unwrap()
}

/// The blob really is a Payment of exactly `amount_e8s` icUSD from the issuer to
/// `dest` (our currency + issuer, no tag, no flags).
fn assert_payment(fx: &Fx, d: &Decoded, dest: &str, amount_e8s: u64) {
    assert_eq!(d.tx_type, 0, "Payment");
    assert_eq!(d.account, issuer_id(fx));
    assert_eq!(d.destination, Some(account_id_from_classic_address(dest).unwrap()));
    let (amt, cur, iss) = d.amount.expect("issued Amount");
    assert_eq!(amt, e8(amount_e8s), "exact delivered amount");
    let want_cur = rumi_protocol_backend::chains::xrp::issued_amount::CurrencyCode::icusd().0;
    assert_eq!(cur, want_cur, "icUSD currency code");
    assert_eq!(iss, issuer_id(fx), "issuer is the lane's issuer");
    assert_eq!(d.flags & 0x0002_0000, 0, "never a partial payment");
}

fn expect_phase(v: &OperationView, p: Phase) {
    assert!(phase_is(v, p.clone()), "expected phase {p:?}, got {:?} notes={:?}", v.phase, v.notes);
}

/// Resume an operation (stepping over the throttle) until it reaches `want`.
fn drive_to(fx: &Fx, who: Principal, op: u64, want: Phase) -> OperationView {
    for _ in 0..8 {
        let v = fx.op(who, op);
        if phase_is(&v, want.clone()) {
            return v;
        }
        fx.run(31, 3);
        let _ = fx.resume(op);
    }
    let v = fx.op(who, op);
    panic!("operation {op} did not reach {want:?}: {:?} notes={:?}", v.phase, v.notes);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 1. Full lifecycle (R1-R5)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn full_lifecycle_deposit_borrow_deliver_repay_import_close() {
    let fx = boot();
    fx.register_xrp();
    fx.zero_interest_one_percent_fee();
    let vault = fx.open_and_fund_vault(user(), 500);
    fx.lane_setup();
    fx.x.add_holder(HOLDER, "1000000");
    assert_eq!(fx.escrow_bal(), 0);
    assert_eq!(fx.supply(), 0);

    // ── borrow gross 100 (fee exactly 1) -> escrow 99, debt 100, treasury 1 ──
    let b = fx.borrow_to_xrpl(vault, 100 * E8, 1).expect("borrow_to_xrpl");
    assert_eq!((b.gross_e8s, b.fee_e8s, b.net_e8s), (100 * E8, E8, 99 * E8));
    let borrow_id = b.operation_id;
    let b = drive_to(&fx, user(), borrow_id, Phase::Delivered);
    assert_eq!(fx.escrow_bal(), 99 * E8, "escrow holds exactly the net proceeds");
    assert_eq!(fx.bal(treasury(), None), E8, "fee remainder reached the treasury (no deficit)");
    assert_eq!(fx.supply(), 100 * E8, "ledger supply = debt 100 (escrow 99 + treasury 1)");
    let v = fx.vault(user(), vault).unwrap();
    assert_eq!(v.borrowed_icusd_amount, 100 * E8, "debt is the GROSS amount");
    assert_eq!(b.ic_ledger_blocks.len(), 2, "escrow mint + fee mint");
    let kinds: Vec<(String, u128)> = b.ic_ledger_blocks.iter().map(|i| fx.block(*i)).collect();
    assert!(kinds.contains(&("mint".to_string(), u128::from(99 * E8))), "{kinds:?}");
    assert!(kinds.contains(&("mint".to_string(), u128::from(E8))), "{kinds:?}");
    let pays = fx.x.payments_submitted();
    assert_eq!(pays.len(), 1, "exactly one issuer Payment was signed");
    assert_payment(&fx, &pays[0].1, HOLDER, 99 * E8);
    assert_eq!(b.xrpl_tx_hashes, vec![pays[0].0.clone()]);
    assert_eq!(fx.x.holder_balance(HOLDER), e8(99 * E8));
    let cov = fx.coverage();
    assert_eq!(format!("{:?}", cov.status), "Covered");
    assert_eq!((cov.o_e8s, cov.d_e8s, cov.escrow_balance_e8s), (u128::from(99 * E8), 0, u128::from(99 * E8)));

    // ── repay 40 on XRPL -> escrow 59, debt 60, burn block on the ledger ──
    fx.gap();
    let intent = fx.prepare_repay(vault, 40 * E8, false, 2, None).expect("prepare");
    assert_eq!(intent.payer, HOLDER);
    let h40 = fx.x.holder_pays(&ReturnOpts::simple(HOLDER, intent.destination_tag, "40"));
    let r = fx.observe(intent.operation_id, &h40).expect("observe");
    let r = drive_to(&fx, user(), r.operation_id, Phase::Complete);
    assert_eq!(r.debt_applied_e8s, 40 * E8);
    assert_eq!(fx.escrow_bal(), 59 * E8);
    assert_eq!(fx.vault(user(), vault).unwrap().borrowed_icusd_amount, 60 * E8);
    assert_eq!(fx.supply(), 60 * E8, "40 icUSD retired from supply");
    let burn_block = *r.ic_ledger_blocks.last().expect("burn block");
    assert_eq!(fx.block(burn_block), ("burn".to_string(), u128::from(40 * E8)));
    assert_eq!(fx.x.obligation(), e8(59 * E8));

    // ── import 1 canonical icUSD from a funded fixture holder, deliver it ──
    fx.mint_icusd(other_user(), E8 + 3 * LEDGER_FEE);
    fx.approve(other_user(), fx.backend, 2 * E8);
    fx.gap();
    let imp = fx.lane_ok(
        other_user(),
        "issue_xrpl_from_canonical",
        Encode!(&IssueFromCanonicalArg {
            request_nonce: vec![3; 32],
            recipient: HOLDER.to_string(),
            network: XrplNetwork::Mainnet,
            amount_e8s: E8,
        })
        .unwrap(),
    );
    let imp = drive_to(&fx, other_user(), imp.operation_id, Phase::Delivered);
    assert_eq!(imp.kind, OperationKindTag::Import);
    assert_eq!(fx.escrow_bal(), 60 * E8, "import adds 1 of real backing");
    assert_eq!(fx.vault(user(), vault).unwrap().borrowed_icusd_amount, 60 * E8, "import creates no debt");
    assert_eq!(fx.x.holder_balance(HOLDER), e8(60 * E8));
    let pays = fx.x.payments_submitted();
    assert_eq!(pays.len(), 2);
    assert_payment(&fx, &pays[1].1, HOLDER, E8);

    // ── repay the remaining 60 (zero interest) -> debt 0, escrow 0 ──
    fx.gap();
    let intent = fx.prepare_repay(vault, 60 * E8, true, 4, None).expect("prepare full");
    let h60 = fx.x.holder_pays(&ReturnOpts::simple(HOLDER, intent.destination_tag, "60"));
    let r = fx.observe(intent.operation_id, &h60).expect("observe full");
    let r = drive_to(&fx, user(), r.operation_id, Phase::Complete);
    assert_eq!(r.debt_applied_e8s, 60 * E8);
    let v = fx.vault(user(), vault).unwrap();
    assert_eq!((v.borrowed_icusd_amount, v.accrued_interest), (0, 0), "debt fully repaid");
    assert_eq!(fx.escrow_bal(), 0, "no orphan backing left in escrow");
    assert_eq!(fx.x.obligation(), ExactIssuedAmount::ZERO, "nothing outstanding on XRPL");
    let cov = fx.coverage();
    assert_eq!(format!("{:?}", cov.status), "Covered");
    assert_eq!((cov.o_e8s, cov.h_e8s, cov.r_e8s, cov.escrow_balance_e8s), (0, 0, 0, 0));
    // The only icUSD left in the world is the treasury fee and the fixture holder's change.
    assert_eq!(
        fx.supply(),
        fx.bal(treasury(), None) + fx.bal(other_user(), None),
        "supply = treasury fee + fixture holder remainder (every other unit was retired)"
    );
}

// ─── shared scenario fixture ─────────────────────────────────────────────────

/// Booted fixture with XRP registered, a funded vault, the lane Ready and a trust
/// line for [`HOLDER`]. `interest_apr` is the XRP collateral's interest rate.
fn lane_fx(deposit_xrp: u64, interest_apr: f64) -> (Fx, u64) {
    lane_fx_with(deposit_xrp, interest_apr, false)
}

fn lane_fx_with(deposit_xrp: u64, interest_apr: f64, flaky: bool) -> (Fx, u64) {
    let fx = boot_with(flaky);
    fx.register_xrp();
    fx.zero_interest_one_percent_fee();
    if interest_apr > 0.0 {
        fx.plain_ok::<()>(
            dev(),
            "set_interest_rate",
            Encode!(&xrp_collateral_principal(), &interest_apr).unwrap(),
        );
    }
    let vault = fx.open_and_fund_vault(user(), deposit_xrp);
    fx.lane_setup();
    fx.x.add_holder(HOLDER, "1000000");
    (fx, vault)
}

/// Borrow `gross` into XRPL and drive it to Delivered.
fn borrow_delivered(fx: &Fx, vault: u64, gross: u64, nonce: u8) -> OperationView {
    fx.gap();
    let b = fx.borrow_to_xrpl(vault, gross, nonce).expect("borrow_to_xrpl");
    drive_to(fx, user(), b.operation_id, Phase::Delivered)
}

// ═══════════════════════════════════════════════════════════════════════════════
// 6. Lost response / dedup expiry (flaky icUSD ledger)
// ═══════════════════════════════════════════════════════════════════════════════

/// A mint whose reply is lost: the same persisted nonce is re-sent, the ledger
/// answers Duplicate, and exactly ONE escrow mint exists.
#[test]
fn escrow_mint_reply_lost_same_nonce_reconciles_without_second_mint() {
    let (fx, vault) = lane_fx_with(500, 0.0, true);
    fx.gap();
    fx.flaky("set_trap_icrc1_after_commit", Encode!(&1u32).unwrap());
    let b = fx.borrow_to_xrpl(vault, 100 * E8, 1).expect("borrow_to_xrpl accepts the request");
    let id = b.operation_id;
    // The mint committed on the ledger but the backend never saw the reply.
    assert_eq!(fx.escrow_bal(), 99 * E8, "the mint landed");
    let v = fx.op(user(), id);
    expect_phase(&v, Phase::EscrowMintPending);
    assert_eq!(fx.vault(user(), vault).unwrap().borrowed_icusd_amount, 0, "no debt before proof");
    assert_eq!(fx.log_length(), 1, "exactly one block so far");
    // Reconcile: same nonce -> Duplicate -> proven; debt is committed once.
    let v = drive_to(&fx, user(), id, Phase::Delivered);
    assert_eq!(fx.escrow_bal(), 99 * E8, "no second mint");
    assert_eq!(fx.log_length(), 2, "escrow mint + fee mint only");
    assert_eq!(fx.vault(user(), vault).unwrap().borrowed_icusd_amount, 100 * E8, "debt committed exactly once");
    assert_eq!(v.xrpl_tx_hashes.len(), 1);
    assert_eq!(fx.x.holder_balance(HOLDER), e8(99 * E8));
}

/// The dedup window has passed (simulated `TooOld` on the exact retry): recovery
/// proves the original mint from the ICRC-3 block log; no second effect.
#[test]
fn escrow_mint_too_old_after_loss_is_proven_from_block_scan() {
    let (fx, vault) = lane_fx_with(500, 0.0, true);
    fx.gap();
    fx.flaky("set_trap_icrc1_after_commit", Encode!(&1u32).unwrap());
    fx.flaky("set_too_old_after_phantom_mint", Encode!(&1u32).unwrap());
    let b = fx.borrow_to_xrpl(vault, 100 * E8, 1).expect("borrow_to_xrpl");
    let id = b.operation_id;
    assert_eq!(fx.escrow_bal(), 99 * E8);
    expect_phase(&fx.op(user(), id), Phase::EscrowMintPending);
    let v = drive_to(&fx, user(), id, Phase::Delivered);
    assert_eq!(fx.escrow_bal(), 99 * E8, "TooOld never produced a second mint");
    assert_eq!(fx.log_length(), 2, "still exactly the escrow mint + fee mint");
    assert_eq!(fx.vault(user(), vault).unwrap().borrowed_icusd_amount, 100 * E8);
    assert_eq!(v.ic_ledger_blocks.len(), 2);
}
