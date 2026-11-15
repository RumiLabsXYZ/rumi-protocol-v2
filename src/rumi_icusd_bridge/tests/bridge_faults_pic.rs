//! BB-09b-2: fault and adversarial scenarios for the ICP icUSD <-> Base icUSD escrow bridge.
//!
//! Every scenario runs the real bridge wasm in PocketIC (real threshold ECDSA from the II
//! subnet), the real ICRC ledger wasm (or `flaky_ledger` where a test says so) and a local
//! anvil node running the real `BridgedIcUSD` bytecode. The three provider URLs are a
//! passthrough to that one anvil unless a test installs a fault hook.
//!
//! # Evidence labels (read before trusting an assertion)
//!
//! * REAL (anvil): transactions signed by the bridge canister's own tECDSA minter key that
//!   anvil executes, the receipts, logs, balances and `finalized` heights read back through
//!   a `FaultAction::Pass` provider, and anvil snapshot/revert (a genuine chain reorg on the
//!   local node).
//! * MOCKED: anything a test installs through `router.set_fault` other than `Pass`
//!   (`ReplaceBody`, `Reject`, `HttpStatus`, `DelayOnce`). `ForwardThenReject` is half and
//!   half: the request REALLY reaches anvil and executes, only the answer is lost.
//! * SIMULATED ACTORS: `impersonate_send` and the `contract_*_as_admin` helpers have anvil
//!   impersonate an address whose key the test does not hold (the bridge's own admin or
//!   minter address) to model a compromised key or an unrelated spender. Every test that
//!   uses it says so. They are test-only and never touch the bridge canister.
//!
//! Each test's doc comment lists, in a `Real:` / `Mocked:` / `Simulated:` block, which parts of
//! it fall in which class.
//!
//! Build once, then run:
//!
//! ```text
//! cargo build --offline --release --target wasm32-unknown-unknown \
//!     -p rumi_icusd_bridge -p evm_rpc_relay -p flaky_ledger
//! POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge \
//!     --test bridge_faults_pic -- --test-threads=1
//! ```

mod support;

use candid::utils::ArgumentEncoder;
use candid::{encode_args, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIc, WasmResult};
use serde_json::{json, Value};
use support::anvil::{parse_hex_u64, Anvil, Endpoint};
use support::bridge::*;
use support::contract::{
    bytes32_from_u64, logs_with_topic, word_addr, word_u128, word_u64, BridgedIcUsd, DeployArgs, SEL_BRIDGE_OUT_COUNT,
    TOPIC_BRIDGE_OUT,
};
use support::ledger;
use support::outcalls::{FaultAction, Served, PROVIDER_A, PROVIDER_B, PROVIDER_C};

fn e8(n: u64) -> u64 {
    n * E8
}

fn world() -> BridgeWorld {
    BridgeWorld::setup(SetupOptions::default())
}

// ---------------------------------------------------------------------------
// Fault-hook helpers
// ---------------------------------------------------------------------------

fn method_of(req: &Value) -> &str {
    req["method"].as_str().unwrap_or("")
}

fn ok_body(result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()
}

/// The canister's mint simulation is the only `eth_call` that carries a `from` field.
fn is_simulation(req: &Value) -> bool {
    method_of(req) == "eth_call" && req["params"][0]["from"].is_string()
}

fn calldata_of(req: &Value) -> String {
    req["params"][0]["data"].as_str().unwrap_or("").to_lowercase()
}

fn is_bridge_out_count_call(req: &Value) -> bool {
    method_of(req) == "eth_call" && calldata_of(req).starts_with(&format!("0x{}", hex::encode(SEL_BRIDGE_OUT_COUNT)))
}

/// What anvil itself answers to `req` (used to build a subtly wrong answer).
fn upstream_json(ep: &Endpoint, req: &Value) -> Value {
    let (_, body) = ep.post(req.to_string().as_bytes()).expect("anvil answers the forwarded request");
    serde_json::from_slice(&body).expect("anvil body is JSON")
}

/// MOCKED: anvil's block answer with the `hash` replaced by `salt` repeated 32 times.
fn tamper_block_hash(ep: &Endpoint, req: &Value, salt: u8) -> FaultAction {
    let mut v = upstream_json(ep, req);
    if let Some(h) = v["result"].get_mut("hash") {
        *h = json!(format!("0x{}", hex::encode([salt; 32])));
    }
    FaultAction::ReplaceBody(v.to_string())
}

/// MOCKED: anvil's `bridgeOutCount()` answer plus one.
fn tamper_bridge_out_count(ep: &Endpoint, req: &Value) -> FaultAction {
    let v = upstream_json(ep, req);
    let real = u128::from_str_radix(v["result"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    FaultAction::ReplaceBody(ok_body(json!(format!("0x{:064x}", real + 1))))
}

fn count_served(w: &BridgeWorld, provider: &str, served: Served) -> usize {
    w.router.log().iter().filter(|r| r.provider == provider && r.served == served).count()
}

fn send_raw_count(w: &BridgeWorld, provider: &str) -> usize {
    w.router.count(provider, "eth_sendRawTransaction")
}

/// Every DISTINCT raw transaction the canister broadcast, in order of first appearance.
fn broadcast_raws(w: &BridgeWorld) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in w.router.log() {
        if r.rpc_methods.iter().any(|m| m == "eth_sendRawTransaction") {
            let raw = r.request["params"][0].as_str().expect("raw tx param").to_lowercase();
            if !out.contains(&raw) {
                out.push(raw);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// A tiny RLP reader, so replacement assertions look at the bytes that were really broadcast
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Rlp {
    Bytes(Vec<u8>),
    List(Vec<Rlp>),
}

fn be(bytes: &[u8]) -> usize {
    bytes.iter().fold(0usize, |a, b| (a << 8) | *b as usize)
}

fn rlp_items(mut buf: &[u8]) -> Vec<Rlp> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let (item, n) = rlp_decode(buf);
        out.push(item);
        buf = &buf[n..];
    }
    out
}

fn rlp_decode(buf: &[u8]) -> (Rlp, usize) {
    let b = buf[0];
    match b {
        0x00..=0x7f => (Rlp::Bytes(vec![b]), 1),
        0x80..=0xb7 => {
            let n = (b - 0x80) as usize;
            (Rlp::Bytes(buf[1..1 + n].to_vec()), 1 + n)
        }
        0xb8..=0xbf => {
            let ll = (b - 0xb7) as usize;
            let n = be(&buf[1..1 + ll]);
            (Rlp::Bytes(buf[1 + ll..1 + ll + n].to_vec()), 1 + ll + n)
        }
        0xc0..=0xf7 => {
            let n = (b - 0xc0) as usize;
            (Rlp::List(rlp_items(&buf[1..1 + n])), 1 + n)
        }
        _ => {
            let ll = (b - 0xf7) as usize;
            let n = be(&buf[1..1 + ll]);
            (Rlp::List(rlp_items(&buf[1 + ll..1 + ll + n])), 1 + ll + n)
        }
    }
}

/// The fields of a signed EIP-1559 transaction that the replacement scenarios compare.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RawTx {
    nonce: u64,
    max_priority_fee: u128,
    max_fee: u128,
    gas_limit: u64,
    to: Vec<u8>,
    data: Vec<u8>,
    hash: String,
}

fn decode_raw_tx(raw_hex: &str) -> RawTx {
    let raw = hex::decode(raw_hex.trim_start_matches("0x")).expect("raw tx hex");
    assert_eq!(raw[0], 0x02, "EIP-1559 envelope");
    let (Rlp::List(items), _) = rlp_decode(&raw[1..]) else { panic!("tx payload is not an RLP list") };
    let bytes = |i: usize| -> Vec<u8> {
        match &items[i] {
            Rlp::Bytes(b) => b.clone(),
            Rlp::List(_) => panic!("field {i} is a list"),
        }
    };
    let num = |i: usize| bytes(i).iter().fold(0u128, |a, b| (a << 8) | *b as u128);
    RawTx {
        nonce: num(1) as u64,
        max_priority_fee: num(2),
        max_fee: num(3),
        gas_limit: num(4) as u64,
        to: bytes(5),
        data: bytes(7),
        hash: hex0x(&keccak(&raw)),
    }
}

// ---------------------------------------------------------------------------
// Snapshot helper for "nothing moved" assertions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    deposits: Vec<(DepositStatus, Option<BaseTxStage>)>,
    buckets: [u128; 7],
    processed: u64,
    cursor: u64,
    releases: usize,
    user_balance: u128,
    escrow: u128,
}

fn fingerprint(w: &BridgeWorld, deposit_ids: &[u64], release_owner: Principal) -> Fingerprint {
    let info = w.bridge_info();
    Fingerprint {
        deposits: deposit_ids
            .iter()
            .map(|id| {
                let d = w.get_deposit(*id).expect("deposit");
                (d.status, d.mint_stage)
            })
            .collect(),
        buckets: w.supply_audit().buckets(),
        processed: info.observer.processed_count,
        cursor: info.observer.cursor_block,
        releases: w.list_releases_for_owner(release_owner, None, 100).len(),
        user_balance: w.user_balance(release_owner),
        escrow: w.escrow_balance(),
    }
}

// ---------------------------------------------------------------------------
// Contract helpers
// ---------------------------------------------------------------------------

fn minter_role() -> [u8; 32] {
    keccak(b"MINTER_ROLE")
}

fn sel(sig: &str) -> Vec<u8> {
    keccak(sig.as_bytes())[..4].to_vec()
}

/// `grantRole(bytes32,address)`.
fn grant_role_calldata(role: [u8; 32], account: &str) -> Vec<u8> {
    let mut d = sel("grantRole(bytes32,address)");
    d.extend_from_slice(&role);
    d.extend_from_slice(&word_addr(account));
    d
}

/// foundry's debt-backed `IcUSD` (Monad/Conflux), NOT the bridge token.
struct DebtIcUsd {
    address: String,
}

impl DebtIcUsd {
    fn deploy(anvil: &Anvil, deployer: &str, admin: &str, minter: &str) -> DebtIcUsd {
        let artifact = support::pic::workspace_root().join("foundry/out/IcUSD.sol/IcUSD.json");
        let text = std::fs::read_to_string(&artifact)
            .unwrap_or_else(|e| panic!("read {} (run forge build --offline in foundry/): {e}", artifact.display()));
        let json: Value = serde_json::from_str(&text).expect("artifact JSON");
        let mut data = hex::decode(json["bytecode"]["object"].as_str().expect("bytecode").trim_start_matches("0x"))
            .expect("bytecode hex");
        data.extend_from_slice(&word_addr(admin));
        data.extend_from_slice(&word_addr(minter));
        let tx = anvil.send_tx_from_unlocked(deployer, None, &data, 0);
        let rc = anvil.receipt(&tx).expect("deploy receipt");
        assert_eq!(rc["status"], "0x1", "IcUSD deployment reverted: {rc}");
        DebtIcUsd { address: rc["contractAddress"].as_str().expect("contractAddress").to_lowercase() }
    }

    /// `mint(address,uint256,uint64,uint64)` from `minter`.
    fn mint(&self, anvil: &Anvil, minter: &str, to: &str, amount: u128, vault_id: u64, op_id: u64) {
        let mut d = sel("mint(address,uint256,uint64,uint64)");
        d.extend_from_slice(&word_addr(to));
        d.extend_from_slice(&word_u128(amount));
        d.extend_from_slice(&word_u64(vault_id));
        d.extend_from_slice(&word_u64(op_id));
        let h = anvil.send_tx_from_unlocked(minter, Some(&self.address), &d, 0);
        assert!(anvil.tx_succeeded(&h), "IcUSD.mint reverted");
    }

    /// `burn(uint256,uint64)` (the repay burn) from `from`; returns the tx hash.
    fn burn(&self, anvil: &Anvil, from: &str, amount: u128, vault_id: u64) -> String {
        let mut d = sel("burn(uint256,uint64)");
        d.extend_from_slice(&word_u128(amount));
        d.extend_from_slice(&word_u64(vault_id));
        let h = anvil.send_tx_from_unlocked(from, Some(&self.address), &d, 0);
        assert!(anvil.tx_succeeded(&h), "IcUSD.burn reverted");
        h
    }
}

/// Raise the fee caps so a same-nonce replacement has headroom. anvil answers
/// `eth_maxPriorityFeePerGas` with 1 gwei, ten times the default 0.1 gwei priority cap, and
/// its base fee keeps `2 * base + priority` above the default 2 gwei cap, so under the
/// DEFAULT caps the lane correctly refuses to "replace" (it cannot raise either fee) and
/// only rebroadcasts. Production Base fees sit far below the default caps.
fn give_fee_headroom(w: &mut BridgeWorld) {
    let mut g = w.bridge_info().gas;
    g.max_fee_per_gas_wei = Nat::from(50_000_000_000u64);
    g.max_priority_fee_per_gas_wei = Nat::from(5_000_000_000u64);
    let r: BridgeResult<()> = w.admin_call("admin_set_gas", (g,));
    r.expect("admin_set_gas");
}

/// The cheap idle observer leaves its cursor behind the finalized head until its hourly full
/// pass, and the exact-supply check (C3) is only claimed when the cursor has caught up. Jump
/// an hour, let the canister run, then reconcile.
fn settle_observer_and_reconcile(w: &mut BridgeWorld) -> SupplyAudit {
    w.advance_secs(3_700);
    w.ticks(2);
    w.reconcile()
}

fn notes_containing(w: &BridgeWorld, needle: &str) -> Vec<String> {
    w.audit_log_all().into_iter().map(|e| e.note).filter(|n| n.contains(needle)).collect()
}

// ===========================================================================
// 1. Lost reply after a broadcast that executed
// ===========================================================================

/// The broadcast executes on anvil but every provider answer is lost. The canister must keep
/// the one signed attempt, find its receipt, and deliver exactly once at the same nonce.
///
/// Real: the mint transaction is signed by the canister's tECDSA key and EXECUTES on anvil
/// (REAL side effect of `ForwardThenReject`); receipts, nonce and balances are anvil's.
/// Mocked: the loss of the `eth_sendRawTransaction` answers on all three providers.
/// Simulated: nothing.
#[test]
fn lost_reply_after_executed_broadcast_delivers_exactly_once_without_new_nonce() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    w.router.set_fault(|_, req| {
        if method_of(req) == "eth_sendRawTransaction" {
            FaultAction::ForwardThenReject
        } else {
            FaultAction::Pass
        }
    });
    let a = e8(25);
    let dep = w.approve_and_deposit(user, a, &recipient);
    assert_eq!(dep.status, DepositStatus::Escrowed, "{dep:?}");
    let id = dep.id;

    // The mint executed on anvil even though the canister saw only errors.
    let dep = w.wait_mint_included(id);
    assert_eq!(w.minter_nonce(), 1, "the mint tx really executed on anvil");
    assert_eq!(dep.mint_tx_hashes.len(), 1, "exactly one recorded attempt: {dep:?}");
    let hash = dep.mint_tx_hashes[0].clone();
    assert_eq!(w.receipt(&hash).expect("receipt on anvil")["status"], "0x1");
    for p in [PROVIDER_A, PROVIDER_B, PROVIDER_C] {
        assert!(send_raw_count(&w, p) >= 1, "{p} was asked to broadcast");
        assert!(count_served(&w, p, Served::RealResponseLost) >= 1, "{p}: the answer was lost after forwarding");
    }

    // Time passes, the lane keeps polling: still one attempt, still one nonce used.
    w.ticks(4);
    let dep = w.get_deposit(id).unwrap();
    assert_eq!(dep.mint_tx_hashes, vec![hash.clone()], "no second signature: {dep:?}");
    assert_eq!(w.minter_nonce(), 1);

    w.mine_finalize();
    let dep = w.wait_delivered(id);
    assert_eq!(dep.delivered.as_ref().unwrap().tx_hash, hash);
    assert_eq!(dep.mint_tx_hashes, vec![hash.clone()]);
    assert_eq!(w.minter_nonce(), 1, "exactly one Base transaction was ever sent");
    assert_eq!(w.base_balance(&recipient), a as u128);
    assert_eq!(w.base_total_supply(), a as u128);
    let (mint_id, to, amount) = w.bridge_mint_of(&hash);
    assert_eq!((hex0x(&mint_id), to.as_str(), amount), (dep.mint_id.clone(), recipient.as_str(), a as u128));
    assert!(w.base_mint_consumed(&dep.mint_id));

    // Every broadcast, on every provider, carried the identical signed bytes.
    let raws = broadcast_raws(&w);
    assert_eq!(raws.len(), 1, "one signed transaction only: {raws:?}");
    assert_eq!(decode_raw_tx(&raws[0]).hash, hash, "the persisted hash is the hash of the broadcast bytes");
    assert_eq!(notes_containing(&w, "signed: nonce").len(), 1, "one signing event in the audit log");

    let audit = settle_observer_and_reconcile(&mut w);
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), a as u128);
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    assert!(w.bridge_info().halted.is_none());
}

// ===========================================================================
// 2. Replacement at the same nonce
// ===========================================================================

/// The first attempt never reaches the chain; after `replace_after_secs` the lane signs a
/// strictly-higher-fee Replacement at the SAME nonce; once providers heal exactly one attempt
/// is mined and the deposit is delivered exactly once.
///
/// Real: signing, the nonce checks (`eth_getTransactionCount`), the eventual execution on
/// anvil and the delivery; the broadcast bytes are decoded from the provider request log.
/// Mocked: `Reject` of every `eth_sendRawTransaction` (the request is NOT forwarded to
/// anvil) until the test clears the fault.
/// Simulated: nothing. Fee caps are raised with the real `admin_set_gas` (see
/// `give_fee_headroom` for why the default caps cannot bump on anvil).
#[test]
fn unreachable_first_attempt_is_replaced_at_same_nonce_with_higher_fees_and_delivers_once() {
    let mut w = world();
    give_fee_headroom(&mut w);
    let (user, recipient) = (w.user, w.holder(0));
    let replace_after = w.bridge_info().gas.replace_after_secs;
    w.router.set_fault(|_, req| {
        if method_of(req) == "eth_sendRawTransaction" {
            FaultAction::Reject
        } else {
            FaultAction::Pass
        }
    });
    let a = e8(40);
    let dep = w.approve_and_deposit(user, a, &recipient);
    let id = dep.id;
    w.tick_until("first attempt signed", 20, |w| {
        w.get_deposit(id).map(|d| !d.mint_tx_hashes.is_empty()).unwrap_or(false)
    });
    assert_eq!(w.get_deposit(id).unwrap().mint_tx_hashes.len(), 1);
    assert_eq!(w.minter_nonce(), 0, "the first attempt never reached anvil");
    assert_eq!(w.base_total_supply(), 0);

    // Younger than replace_after_secs: the same bytes are rebroadcast, never replaced.
    w.ticks(6);
    assert_eq!(w.get_deposit(id).unwrap().mint_tx_hashes.len(), 1, "no replacement yet");
    assert!(send_raw_count(&w, PROVIDER_A) >= 2, "the same bytes were rebroadcast");
    assert_eq!(broadcast_raws(&w).len(), 1);

    // Older than replace_after_secs and the chain still shows the nonce unused: replace.
    w.advance_secs(replace_after + 5);
    w.tick();
    let dep = w.get_deposit(id).unwrap();
    assert!(dep.mint_tx_hashes.len() >= 2, "a Replacement was signed: {dep:?}\n{}", w.dump());
    let raws = broadcast_raws(&w);
    assert_eq!(raws.len(), dep.mint_tx_hashes.len(), "every attempt was broadcast");
    let txs: Vec<RawTx> = raws.iter().map(|r| decode_raw_tx(r)).collect();
    assert_eq!(txs.iter().map(|t| t.hash.clone()).collect::<Vec<_>>(), dep.mint_tx_hashes, "views match the bytes");
    for pair in txs.windows(2) {
        assert_eq!(pair[1].nonce, pair[0].nonce, "SAME nonce: {pair:?}");
        assert_eq!(pair[1].nonce, 0);
        assert!(pair[1].max_fee > pair[0].max_fee, "max fee strictly higher: {pair:?}");
        assert!(pair[1].max_priority_fee > pair[0].max_priority_fee, "priority fee strictly higher: {pair:?}");
        assert!(
            pair[1].max_fee >= pair[0].max_fee + pair[0].max_fee.div_ceil(10),
            "a node needs at least +10%: {pair:?}"
        );
        assert_eq!((pair[1].to.clone(), pair[1].data.clone(), pair[1].gas_limit), (pair[0].to.clone(), pair[0].data.clone(), pair[0].gas_limit));
    }
    assert_eq!(w.minter_nonce(), 0, "still nothing on anvil");
    assert!(!notes_containing(&w, "(Replacement) signed").is_empty(), "audit log records the replacement");

    // Providers heal. The newest attempt is rebroadcast (30 s spacing) and mined.
    w.router.clear_fault();
    let dep = w.wait_mint_included(id);
    let mined: Vec<&String> = dep.mint_tx_hashes.iter().filter(|h| w.receipt(h).is_some()).collect();
    assert_eq!(mined.len(), 1, "exactly one attempt was mined: {dep:?}");
    assert_eq!(w.minter_nonce(), 1);
    w.mine_finalize();
    let dep = w.wait_delivered(id);
    assert_eq!(dep.delivered.as_ref().unwrap().tx_hash, *mined[0]);
    assert_eq!(w.minter_nonce(), 1, "exactly one Base transaction executed");
    assert_eq!(w.base_balance(&recipient), a as u128);
    assert_eq!(w.base_total_supply(), a as u128);
    w.bridge_mint_of(&dep.delivered.as_ref().unwrap().tx_hash);
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.outstanding(), a as u128);
    assert!(w.bridge_info().halted.is_none());
}

// ===========================================================================
// 3. Reorgs before finality
// ===========================================================================

/// The mint is mined, the canister reports it Included, then the block vanishes (anvil
/// `evm_revert`). Before finality nothing is settled; once the finalized head passes the old
/// height the lane sees the receipt is gone, returns the tx to pending and rebroadcasts the
/// SAME bytes; the re-mined tx is finalized and delivered exactly once.
///
/// Real: signing, execution, snapshot/revert of the local chain (a genuine reorg on anvil),
/// receipts and finality.
/// Mocked: nothing (no fault hook).
/// Simulated: nothing.
#[test]
fn mint_reorged_before_finality_is_rebroadcast_same_bytes_and_delivered_once() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    let a = e8(30);
    let dep = w.approve_and_deposit(user, a, &recipient);
    let id = dep.id;
    assert_eq!(w.minter_nonce(), 0);
    let snap = w.anvil.snapshot(); // before any tick can broadcast the mint

    let dep = w.wait_mint_included(id);
    let hash = dep.mint_tx_hashes[0].clone();
    let old_block = match dep.mint_stage {
        Some(BaseTxStage::Included { block_number }) => block_number,
        other => panic!("expected Included, got {other:?}"),
    };
    assert_eq!(w.minter_nonce(), 1);
    assert_eq!(w.get_deposit(id).unwrap().status, DepositStatus::MintInFlight);
    assert_eq!(w.supply_audit().outstanding(), 0, "included is not delivered");

    // Reorg: the block, the receipt and the mint disappear from anvil.
    assert!(w.anvil.revert(&snap), "evm_revert succeeded");
    assert!(w.receipt(&hash).is_none(), "receipt vanished");
    assert_eq!(w.minter_nonce(), 0);
    assert_eq!(w.base_total_supply(), 0);
    // Not final yet (finalized head is below the old block): the canister still holds Included.
    w.ticks(2);
    assert!(matches!(w.get_deposit(id).unwrap().mint_stage, Some(BaseTxStage::Included { .. })));
    assert_eq!(w.supply_audit().outstanding(), 0);

    // The chain grows until the finalized head reaches the old height: the lane notices.
    let sends_before = send_raw_count(&w, PROVIDER_A);
    w.mine(old_block.saturating_sub(w.anvil.block_number("latest")) + 3);
    assert!(w.anvil.block_number("finalized") >= old_block);
    w.tick_until("reorg noticed", 10, |w| !notes_containing(w, "reorg before finality").is_empty());
    assert!(send_raw_count(&w, PROVIDER_A) > sends_before, "the same bytes were rebroadcast");

    // The re-broadcast tx is mined again; finality completes; delivered once.
    let dep = w.wait_mint_included(id);
    assert_eq!(dep.mint_tx_hashes, vec![hash.clone()], "same attempt, no new signature: {dep:?}");
    assert_eq!(w.minter_nonce(), 1);
    w.mine_finalize();
    let dep = w.wait_delivered(id);
    assert_eq!(dep.delivered.as_ref().unwrap().tx_hash, hash);
    assert_eq!(dep.mint_tx_hashes.len(), 1);
    assert_eq!(w.minter_nonce(), 1, "exactly one Base transaction survived");
    assert_eq!(w.base_balance(&recipient), a as u128);
    assert_eq!(w.base_total_supply(), a as u128);
    assert_eq!(broadcast_raws(&w).len(), 1, "only ever one signed transaction");
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), a as u128);
    assert!(w.bridge_info().halted.is_none());
}

/// A holder's `bridgeOut` is mined but reverted off the chain before it ever becomes final.
/// The canister must never create a release for it (and a user hint cannot change that).
///
/// Real: the deposit and delivery, the `bridgeOut` execution on anvil, the snapshot/revert reorg,
/// the observer reads.
/// Mocked: nothing. Simulated: nothing (the holder is an anvil unlocked account).
#[test]
fn bridge_out_reverted_before_finality_never_creates_a_release() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    let a = e8(50);
    w.deposit_and_deliver(user, a, &recipient);
    let before = w.reconcile();
    let user_before = w.user_balance(user);
    let escrow_before = w.escrow_balance();

    let snap = w.anvil.snapshot();
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&recipient, e8(20) as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    assert_eq!(w.base_balance(&recipient), (a - e8(20)) as u128, "burned on the (soon reorged) chain");
    w.notify_hint(user, &out.tx_hash).expect("hint");
    w.ticks(3);
    assert!(w.get_release(&out.transfer_id).is_none(), "not final: no release");

    assert!(w.anvil.revert(&snap));
    assert!(w.receipt(&out.tx_hash).is_none());
    assert_eq!(w.base_bridge_out_count(), 0);
    assert_eq!(w.base_balance(&recipient), a as u128, "the burn never happened on the surviving chain");
    w.mine(8); // everything that exists is now final
    w.ticks(8);
    assert!(w.get_release(&out.transfer_id).is_none(), "a reorged burn never settles");
    assert!(w.list_releases_for_owner(user, None, 10).is_empty());
    assert_eq!(w.user_balance(user), user_before);
    assert_eq!(w.escrow_balance(), escrow_before);
    let after = w.reconcile();
    assert_eq!(after.buckets(), before.buckets());
    assert_eq!(after.base_supply_within_bound, Some(true));
    assert_eq!(w.bridge_info().observer.processed_count, 0);
    assert!(w.bridge_info().halted.is_none());
}

// ===========================================================================
// 4. Inconsistent providers
// ===========================================================================

/// With one liar of three, 2-of-3 quorum keeps the bridge moving. With two providers lying
/// differently, nothing is delivered and nothing is recorded (state frozen, observer error
/// visible). After the providers heal each settlement happens exactly once.
///
/// Real: all chain execution (deposit mints, bridgeOuts) and the anvil data the honest provider
/// returns; the settlements after healing.
/// Mocked: provider-c's finalized block hash, `bridgeOutCount` and `eth_getLogs` answers
/// (phase A); provider-b's and provider-c's differing finalized block hashes (phase B). Every
/// forged answer is a `ReplaceBody` built from anvil's real answer with one field changed.
/// Simulated: nothing.
#[test]
fn provider_disagreement_keeps_quorum_progress_then_freezes_then_settles_exactly_once() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    let ep = w.anvil.endpoint();

    // --- Phase A: only provider-c lies (different hash, count+1, hides logs). 2-of-3 holds. ---
    {
        let ep = ep.clone();
        w.router.set_fault(move |provider, req| {
            if provider != PROVIDER_C {
                return FaultAction::Pass;
            }
            match method_of(req) {
                "eth_getBlockByNumber" => tamper_block_hash(&ep, req, 0xC0),
                "eth_getLogs" => FaultAction::ReplaceBody(ok_body(json!([]))),
                _ if is_bridge_out_count_call(req) => tamper_bridge_out_count(&ep, req),
                _ => FaultAction::Pass,
            }
        });
    }
    let a = e8(60);
    let d1 = w.deposit_and_deliver(user, a, &recipient);
    assert_eq!(d1.status, DepositStatus::Delivered);
    let user_before = w.user_balance(user);
    let (out1, rel1) = w.bridge_out_and_release(&recipient, e8(10) as u128, user);
    assert_eq!(rel1.status, ReleaseStatus::Released);
    assert_eq!(w.user_balance(user) - user_before, (e8(10) - LEDGER_FEE) as u128);
    assert!(count_served(&w, PROVIDER_C, Served::MockedBody) > 0, "provider-c really served forged answers");
    assert_eq!(count_served(&w, PROVIDER_A, Served::MockedBody), 0);
    assert!(w.bridge_info().halted.is_none());

    // --- Phase B: b and c lie with DIFFERENT finalized hashes: no quorum on the finalized head. ---
    {
        let ep = ep.clone();
        w.router.set_fault(move |provider, req| {
            let salt = match provider {
                PROVIDER_B => 0xB0,
                PROVIDER_C => 0xC0,
                _ => return FaultAction::Pass,
            };
            if method_of(req) == "eth_getBlockByNumber" {
                tamper_block_hash(&ep, req, salt)
            } else {
                FaultAction::Pass
            }
        });
    }
    let d2 = w.approve_and_deposit(user, e8(30), &recipient);
    assert_eq!(d2.status, DepositStatus::Escrowed);
    let d2 = w.wait_mint_included(d2.id); // receipts are not forged, so inclusion is seen
    let nonce = w.fresh_nonce();
    let out2 = w.bridge_out_from(&recipient, e8(15) as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(out2.success);
    w.mine_finalize(); // both are final on anvil, but the canister cannot establish that
    let frozen = fingerprint(&w, &[d2.id], user);
    w.ticks(8);
    let still = fingerprint(&w, &[d2.id], user);
    assert_eq!(still, frozen, "no state moved while the finalized head has no quorum");
    assert_eq!(still.deposits[0].0, DepositStatus::MintInFlight);
    assert!(matches!(still.deposits[0].1, Some(BaseTxStage::Included { .. })));
    assert!(w.get_release(&out2.transfer_id).is_none(), "no release recorded");
    let info = w.bridge_info();
    let err = info.observer.last_error.clone().expect("observer reports the inconsistency");
    assert!(err.to_lowercase().contains("quorum"), "observer error names the missing quorum: {err}");
    assert!(info.halted.is_none(), "disagreement alone is not a halt");
    assert_eq!(info.observer.processed_count, 1, "only the first bridge-out is processed");

    // --- Phase C: heal. Everything settles, exactly once. ---
    let user_pre_c = w.user_balance(user);
    w.router.clear_fault();
    let d2 = w.wait_delivered(d2.id);
    assert_eq!(d2.status, DepositStatus::Delivered);
    let rel2 = w.wait_released(&out2.transfer_id);
    assert_eq!(rel2.status, ReleaseStatus::Released);
    w.ticks(3);
    let releases = w.list_releases_for_owner(user, None, 100);
    assert_eq!(releases.len(), 2, "one release per bridge-out, no duplicates: {releases:?}");
    assert_eq!(
        w.user_balance(user),
        user_pre_c + (e8(15) - LEDGER_FEE) as u128,
        "release 2 paid out exactly once (bridge-out amount minus the ledger fee)"
    );
    assert_eq!(w.minter_nonce(), 2, "two deposits, two mints");
    assert_eq!(w.base_total_supply(), (a - e8(10) + e8(30) - e8(15)) as u128);
    let info = w.bridge_info();
    assert!(info.observer.last_error.is_none(), "observer recovered: {:?}", info.observer.last_error);
    assert_eq!(info.observer.processed_count, 2);
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_within_bound, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    assert_ne!(out1.transfer_id, out2.transfer_id);
    assert_eq!(rel1.transfer_id, out1.transfer_id);
}

// ===========================================================================
// 5. Finalized revert, then refund
// ===========================================================================

/// The contract's mint is paused (so the real mint tx REVERTS on anvil) while the canister's
/// preflight simulation is forced to look healthy. After finality the deposit is held with a
/// revert reason and does not auto-retry; the owner refunds it and gets A minus the ledger
/// fee; Base supply never moved.
///
/// Real: the reverting mint execution (status 0x0 on anvil, nonce consumed), finality, the
/// `mintConsumed` check, the ledger refund and all balances.
/// Mocked: the preflight simulation `eth_call` (the one carrying a `from`) answered with a
/// successful empty result on all three providers.
/// Simulated: `pauseMint()` executed on the contract by anvil impersonating the bridge's
/// admin address (a pauser acting on the contract).
#[test]
fn finalized_reverted_mint_is_held_then_owner_refund_returns_amount_minus_ledger_fee() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    w.contract_pause_mint_as_admin();
    assert!(w.contract_mint_paused());
    w.router.set_fault(|_, req| {
        if is_simulation(req) {
            FaultAction::ReplaceBody(ok_body(json!("0x")))
        } else {
            FaultAction::Pass
        }
    });
    let a = e8(35);
    let user_start = w.user_balance(user);
    let dep = w.approve_and_deposit(user, a, &recipient);
    let id = dep.id;
    assert_eq!(dep.status, DepositStatus::Escrowed);

    let dep = w.wait_mint_included(id);
    assert_eq!(dep.mint_tx_hashes.len(), 1);
    let hash = dep.mint_tx_hashes[0].clone();
    assert_eq!(w.receipt(&hash).unwrap()["status"], "0x0", "the mint REVERTED on anvil");
    assert_eq!(w.minter_nonce(), 1, "the reverted tx still consumed the nonce");
    for p in [PROVIDER_A, PROVIDER_B, PROVIDER_C] {
        assert!(count_served(&w, p, Served::MockedBody) >= 1, "{p}: simulation was forced to succeed");
    }
    w.mine_finalize();
    let dep = w.wait_deposit(id, "MintHeld after the finalized revert", |d| matches!(d.status, DepositStatus::MintHeld { .. }));
    match &dep.status {
        DepositStatus::MintHeld { reason } => assert!(reason.to_lowercase().contains("revert"), "reason: {reason}"),
        other => panic!("{other:?}"),
    }
    assert!(dep.mint_stage.is_none());
    // A finalized revert does not auto-retry (it would only burn gas again).
    w.ticks(6);
    assert!(matches!(w.get_deposit(id).unwrap().status, DepositStatus::MintHeld { .. }));
    assert_eq!(w.minter_nonce(), 1, "no second mint attempt");
    assert_eq!(w.base_total_supply(), 0);
    assert!(!w.base_mint_consumed(&dep.mint_id));
    assert_eq!(w.escrow_balance(), a as u128, "backing is still in escrow");

    // Non-owner cannot refund; the owner can.
    let other = w.user2;
    assert_eq!(w.request_refund(other, id).unwrap_err(), BridgeError::NotOwner);
    let before_refund = w.user_balance(user);
    let refunded = w.request_refund(user, id).expect("owner refund");
    let refunded = if refunded.status == DepositStatus::Refunded {
        refunded
    } else {
        w.wait_deposit(id, "Refunded", |d| d.status == DepositStatus::Refunded)
    };
    assert_eq!(refunded.status, DepositStatus::Refunded);
    assert_eq!(refunded.refund_amount_e8s, Some(a - LEDGER_FEE));
    assert_eq!(w.user_balance(user) - before_refund, (a - LEDGER_FEE) as u128, "exactly A minus the ledger fee");
    assert_eq!(
        user_start - w.user_balance(user),
        3 * LEDGER_FEE as u128,
        "net loss is the three ledger fees only: approve, pull and refund"
    );
    assert_eq!(w.escrow_balance(), 0);
    assert_eq!(w.base_total_supply(), 0, "Base supply unchanged");
    assert!(!w.base_mint_consumed(&dep.mint_id), "the mint id was never consumed");
    assert_eq!(w.minter_nonce(), 1);
    let audit = settle_observer_and_reconcile(&mut w);
    assert_eq!(audit.buckets(), [0; 7], "{audit:?}");
    assert_eq!(audit.coverage_ok, Some(true));
    assert_eq!(audit.base_supply_exact_match, Some(true));
    assert!(w.bridge_info().halted.is_none());
    // And it cannot be refunded twice.
    assert!(w.request_refund(user, id).is_err());
    assert_eq!(w.user_balance(user) - before_refund, (a - LEDGER_FEE) as u128);
}

// ===========================================================================
// 6. Unauthorized Base supply halts the bridge
// ===========================================================================

/// Base supply appears that the canister never minted. Reconciliation halts the bridge:
/// new deposits are refused, a later bridge-out of the unbacked tokens is recorded but never
/// released, and the halt cannot be cleared while the excess exists.
///
/// Real: the deposit and delivery, the unauthorized mint, the bridge-out burn, finality and the
/// canister's reconciliation reads, all executed on anvil.
/// Mocked: nothing.
/// Simulated: anvil impersonates the bridge's admin address to `grantRole(MINTER_ROLE,
/// attacker)` (a compromised admin key); the "attacker" is an anvil unlocked account that
/// then mints directly.
#[test]
fn unauthorized_base_supply_halts_bridge_blocks_deposits_and_releases() {
    let mut w = world();
    let (user, user2, recipient, attacker) = (w.user, w.user2, w.holder(0), w.holder(3));
    let a = e8(100);
    w.deposit_and_deliver(user, a, &recipient);
    let clean = w.reconcile();
    assert_eq!(clean.base_supply_within_bound, Some(true));
    assert!(w.bridge_info().halted.is_none());

    // Compromised admin grants MINTER_ROLE; the attacker mints 40 unbacked tokens.
    let grant = w.impersonate_send(&w.base_addrs.admin.clone(), &w.token.address.clone(), &grant_role_calldata(minter_role(), &attacker));
    assert!(w.anvil.tx_succeeded(&grant), "grantRole by the (impersonated) admin");
    let mint_tx = w.token.mint_from_unlocked(&w.anvil, &attacker, &attacker, e8(40) as u128, bytes32_from_u64(0xBAD));
    assert!(w.anvil.tx_succeeded(&mint_tx));
    assert_eq!(w.base_total_supply(), e8(140) as u128);
    w.mine_finalize();

    let audit = w.reconcile();
    assert_eq!(audit.base_supply_within_bound, Some(false), "{audit:?}");
    assert_eq!(audit.base_total_supply_e8s.as_ref().map(nat_u128), Some(e8(140) as u128));
    assert_eq!(audit.outstanding(), a as u128);
    let info = w.bridge_info();
    let reason = info.halted.clone().expect("bridge halted");
    // Both the cumulative-mint guard ("exceed the bridge's minted") and the supply bound
    // ("exceeds outstanding") fire on this excess; the cumulative one reports first.
    assert!(reason.contains("exceed"), "halt reason: {reason}");

    // New deposits are refused.
    w.approve_bridge(user2, e8(5));
    let args = w.deposit_args(e8(5), &recipient);
    match w.bridge_to_base(user2, args) {
        Err(BridgeError::Halted(r)) => assert!(r.contains("exceed"), "{r}"),
        other => panic!("expected Halted, got {other:?}"),
    }

    // The attacker burns the unbacked tokens toward user2.
    let escrow_before = w.escrow_balance();
    let user2_before = w.user_balance(user2);
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&attacker, e8(30) as u128, user2.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    w.mine_finalize();
    w.ticks(8);
    if let Some(r) = w.get_release(&out.transfer_id) {
        assert_ne!(r.status, ReleaseStatus::Released, "a halted bridge must not release: {r:?}");
        assert!(r.release_block_index.is_none());
    }
    assert_eq!(w.user_balance(user2), user2_before, "no funds reached the attacker's destination");
    assert_eq!(w.escrow_balance(), escrow_before, "escrow untouched");
    assert!(w.bridge_info().halted.is_some());

    // Clearing needs fresh checks to pass; the excess (Base supply 110 vs accounted 70) remains.
    let cleared: BridgeResult<SupplyAudit> = w.admin_call("admin_clear_halt", ());
    match cleared {
        Err(BridgeError::Halted(r)) => assert!(!r.is_empty()),
        other => panic!("expected admin_clear_halt to fail, got {other:?}"),
    }
    assert!(w.bridge_info().halted.is_some(), "still halted");
    assert_eq!(w.user_balance(user2), user2_before);
}

/// A mint this canister did not make, burned again at once, must not be paid out of escrow.
/// The periodic supply bound cannot see it (the burn leaves Base `totalSupply` looking
/// normal), so the contract counts every mint cumulatively (`totalMinted()`) and the observer
/// reads that counter at the finalized head BEFORE it records any burn. It finds Base has
/// minted 40 more than the canister did (plus nothing in flight), halts the bridge and
/// records nothing: the attacker's burn is never released and escrow stays untouched.
///
/// Real: everything executes on anvil and the real ledger. Mocked: nothing.
/// Simulated: anvil impersonates the bridge's admin address to `grantRole(MINTER_ROLE,
/// attacker)` (a compromised admin key); the attacker is an anvil unlocked account.
#[test]
fn unauthorized_mint_then_burn_before_reconcile_must_not_pay_out_of_escrow() {
    let mut w = world();
    let (user, user2, recipient, attacker) = (w.user, w.user2, w.holder(0), w.holder(3));
    w.deposit_and_deliver(user, e8(100), &recipient);
    let clean = w.reconcile(); // the last reconcile before the attack
    assert_eq!(clean.base_supply_within_bound, Some(true));
    assert!(w.bridge_info().halted.is_none());
    let user2_before = w.user_balance(user2);
    let escrow_before = w.escrow_balance();

    let grant = w.impersonate_send(&w.base_addrs.admin.clone(), &w.token.address.clone(), &grant_role_calldata(minter_role(), &attacker));
    assert!(w.anvil.tx_succeeded(&grant));
    let mint_tx = w.token.mint_from_unlocked(&w.anvil, &attacker, &attacker, e8(40) as u128, bytes32_from_u64(0xBAD));
    assert!(w.anvil.tx_succeeded(&mint_tx));
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&attacker, e8(40) as u128, user2.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    // The burn hides the mint from totalSupply: only the cumulative counter shows it.
    assert_eq!(w.base_total_supply(), e8(100) as u128, "supply looks normal again");
    w.mine_finalize();
    w.ticks(6); // well inside the 30 minute reconcile cadence
    let paid = w.user_balance(user2) - user2_before;
    let reason = w.bridge_info().halted.clone().expect("the observer halted the bridge before any reconcile");
    assert!(reason.contains("Base cumulative mints") && reason.contains("exceed"), "halt reason: {reason}");
    assert_eq!((paid, w.escrow_balance()), (0, escrow_before), "nothing may be paid out of escrow; halt: {reason}");
    if let Some(r) = w.get_release(&out.transfer_id) {
        assert_ne!(r.status, ReleaseStatus::Released, "the unbacked burn must not be released: {r:?}");
        assert!(r.release_block_index.is_none());
    }

    // The late reconcile does not undo the halt, and nothing is ever paid.
    let audit = w.reconcile();
    assert!(w.bridge_info().halted.is_some(), "still halted after reconcile: {audit:?}");
    w.ticks(4);
    assert_eq!((w.user_balance(user2) - user2_before, w.escrow_balance()), (0, escrow_before));
}

// ===========================================================================
// 7. Unexplained nonce use halts the lane
// ===========================================================================

/// The canister's first attempt never lands, but an unrelated transaction consumes the minter
/// nonce on the chain and finalizes. After `replace_after_secs` the minter lane halts, nothing
/// is re-signed or rebroadcast, and the deposit's backing stays in `pending_mint`.
///
/// Real: signing, the nonce queries and the lane's halt decision; the unrelated transaction
/// mined and finalized on anvil.
/// Mocked: `Reject` of every `eth_sendRawTransaction` (so the canister's attempt never reaches
/// anvil), cleared late only to show nothing is rebroadcast.
/// Simulated: anvil impersonates the bridge's own minter address to send an unrelated
/// transaction at the minter's nonce (models a leaked or misused key).
#[test]
fn unexplained_nonce_consumption_halts_minter_lane_and_parks_the_mint() {
    let mut w = world();
    let (user, recipient) = (w.user, w.holder(0));
    let replace_after = w.bridge_info().gas.replace_after_secs;
    w.router.set_fault(|_, req| {
        if method_of(req) == "eth_sendRawTransaction" {
            FaultAction::Reject
        } else {
            FaultAction::Pass
        }
    });
    let a = e8(20);
    let dep = w.approve_and_deposit(user, a, &recipient);
    let id = dep.id;
    w.tick_until("first attempt signed", 20, |w| {
        w.get_deposit(id).map(|d| !d.mint_tx_hashes.is_empty()).unwrap_or(false)
    });
    assert_eq!(w.minter_nonce(), 0);

    // An unrelated tx from the minter address takes nonce 0 and becomes final.
    let minter = w.base_addrs.minter.clone();
    let stranger = w.holder(1);
    let h = w.impersonate_send(&minter, &stranger, &[]);
    assert!(w.anvil.tx_succeeded(&h));
    w.mine_finalize();
    assert_eq!(w.minter_nonce(), 1);
    assert!(w.bridge_info().lane(LaneKind::Minter).halted.is_none(), "not detected before the age threshold");

    w.advance_secs(replace_after + 10);
    w.tick();
    let info = w.bridge_info();
    let halted = info.lane(LaneKind::Minter).halted.clone().expect("minter lane halted");
    assert!(halted.contains("consumed"), "halt reason: {halted}");
    let dep = w.get_deposit(id).unwrap();
    assert_eq!(dep.status, DepositStatus::MintInFlight);
    assert_eq!(dep.mint_tx_hashes.len(), 1, "no replacement was signed: {dep:?}");
    let audit = w.supply_audit();
    assert_eq!(nat_u128(&audit.pending_mint_e8s), a as u128, "the backing stays in pending_mint");
    assert_eq!(audit.outstanding(), 0);
    assert!(!notes_containing(&w, "lane HALT").is_empty(), "halt is in the audit log");

    // Nothing more is signed or sent, even once the providers accept broadcasts again.
    let sends = [PROVIDER_A, PROVIDER_B, PROVIDER_C].map(|p| send_raw_count(&w, p));
    w.router.clear_fault();
    w.ticks(10);
    w.advance_secs(replace_after * 2);
    w.ticks(2);
    let dep = w.get_deposit(id).unwrap();
    assert_eq!(dep.status, DepositStatus::MintInFlight);
    assert_eq!(dep.mint_tx_hashes.len(), 1, "still one attempt: {dep:?}");
    assert_eq!([PROVIDER_A, PROVIDER_B, PROVIDER_C].map(|p| send_raw_count(&w, p)), sends, "no further broadcast");
    assert_eq!(broadcast_raws(&w).len(), 1);
    assert_eq!(w.minter_nonce(), 1, "the canister's mint never executed");
    assert_eq!(w.base_total_supply(), 0);
    assert!(w.bridge_info().lane(LaneKind::Minter).halted.is_some());
    assert_eq!(nat_u128(&w.supply_audit().pending_mint_e8s), a as u128);
    // The halt is not clearable while the parked tx is outstanding.
    let r: BridgeResult<()> = w.admin_call("admin_clear_lane_halt", (LaneKind::Minter,));
    assert!(matches!(r, Err(BridgeError::InvalidState(_))), "{r:?}");
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "escrow still covers the pending mint: {audit:?}");
}

// ===========================================================================
// 8. Cross-contract and cross-purpose replay
// ===========================================================================

/// `BridgeOut` events from another contract (same bytecode, same event signature, naming a
/// real principal) and a `Burn` from the debt-backed `IcUSD` must not create releases. A
/// genuine bridge-out in the same block range still settles exactly once.
///
/// Real: both foreign contracts are deployed on anvil and really emit their events; the
/// observer reads logs through anvil; the genuine bridge-out and its release.
/// Mocked: nothing.
/// Simulated: nothing (anvil unlocked accounts act as admin/minter/holder of the FOREIGN
/// contracts only; they hold no authority over the bound contract).
#[test]
fn foreign_contract_and_debt_backed_burns_never_create_releases() {
    let mut w = world();
    let (user, user2, recipient) = (w.user, w.user2, w.holder(0));
    let a = e8(80);
    w.deposit_and_deliver(user, a, &recipient);
    let base_audit = w.reconcile();
    let user_before = w.user_balance(user);
    let user2_before = w.user_balance(user2);

    // Foreign BridgedIcUSD naming `user2` as the ICP destination.
    let (admin2, minter2) = (w.anvil.accounts[1].clone(), w.anvil.accounts[2].clone());
    let foreign = BridgedIcUsd::deploy(&w.anvil, &w.deployer.clone(), &DeployArgs::permissive(&admin2, &minter2));
    assert_ne!(foreign.address, w.token.address);
    foreign.mint_from_unlocked(&w.anvil, &minter2, &recipient, e8(50) as u128, bytes32_from_u64(77));
    let foreign_tx = foreign.bridge_out(&w.anvil, &recipient, e8(10) as u128, user2.as_slice(), [0u8; 32], bytes32_from_u64(0x501));
    let rc = w.receipt(&foreign_tx).unwrap();
    assert_eq!(rc["status"], "0x1");
    assert_eq!(logs_with_topic(&rc, &foreign.address, TOPIC_BRIDGE_OUT).len(), 1, "a real BridgeOut event exists on chain");
    assert_eq!(foreign.bridge_out_count(&w.anvil), 1);
    assert_eq!(w.base_bridge_out_count(), 0, "the bound contract saw nothing");

    // Debt-backed IcUSD burn (repay path) by the same holder.
    let debt = DebtIcUsd::deploy(&w.anvil, &w.deployer.clone(), &admin2, &minter2);
    debt.mint(&w.anvil, &minter2, &recipient, e8(50) as u128, 1, 1);
    let burn_tx = debt.burn(&w.anvil, &recipient, e8(10) as u128, 1);
    let burn_topic = hex0x(&keccak(b"Burn(uint256,address,uint256)"));
    let burn_logs = logs_with_topic(&w.receipt(&burn_tx).unwrap(), &debt.address, &burn_topic).len();
    assert_eq!(burn_logs, 1, "a real Burn event exists on chain");

    // Hostile hints for both transactions are harmless.
    w.notify_hint(user2, &foreign_tx).expect("hint");
    w.notify_hint(user2, &burn_tx).expect("hint");
    w.mine_finalize();
    let frozen = fingerprint(&w, &[], user2);
    w.advance_secs(3_700); // force a full observer pass (hourly)
    w.ticks(8);
    let mut after = fingerprint(&w, &[], user2);
    // The observer really scanned past the foreign events (its cursor moved to the finalized
    // head); everything else, releases and buckets included, is unchanged.
    let burn_block = parse_hex_u64(&w.receipt(&burn_tx).unwrap()["blockNumber"]);
    assert!(after.cursor >= burn_block, "cursor {} must have passed the foreign events at {burn_block}", after.cursor);
    after.cursor = frozen.cursor;
    assert_eq!(after, frozen, "nothing moved");
    assert_eq!(w.supply_audit().buckets(), base_audit.buckets(), "buckets unchanged");
    assert!(w.list_releases_for_owner(user2, None, 10).is_empty());
    assert!(w.list_releases_for_owner(user, None, 10).is_empty());
    assert_eq!(w.user_balance(user2), user2_before);
    assert_eq!(w.user_balance(user), user_before);
    assert_eq!(w.bridge_info().observer.processed_count, 0);
    assert!(w.bridge_info().halted.is_none());

    // A genuine bridge-out in the same neighbourhood still settles exactly once.
    let nonce = w.fresh_nonce();
    let out = w.bridge_out_from(&recipient, e8(25) as u128, user.as_slice(), [0u8; 32], nonce);
    assert!(out.success);
    w.mine_finalize();
    let rel = w.wait_released(&out.transfer_id);
    assert_eq!(rel.amount_e8s, e8(25));
    w.ticks(3);
    assert_eq!(w.list_releases_for_owner(user, None, 10).len(), 1);
    assert!(w.list_releases_for_owner(user2, None, 10).is_empty(), "foreign event naming user2 never released");
    assert_eq!(w.user_balance(user) - user_before, (e8(25) - LEDGER_FEE) as u128);
    assert_eq!(w.user_balance(user2), user2_before);
    assert_eq!(w.bridge_info().observer.processed_count, 1);
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), (a - e8(25)) as u128);
}

// ===========================================================================
// 9 and 10. Lost reply after a successful ledger pull (flaky_ledger)
// ===========================================================================

fn flaky_update(pic: &PocketIc, ledger_id: Principal, method: &str, args: impl ArgumentEncoder) {
    match pic
        .update_call(ledger_id, Principal::anonymous(), method, encode_args(args).expect("encode"))
        .unwrap_or_else(|e| panic!("flaky_ledger {method} failed: {e}"))
    {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(m) => panic!("flaky_ledger {method} rejected: {m}"),
    }
}

/// Install `flaky_ledger` with the harness fee and the planned balances.
fn flaky_ledger(pic: &PocketIc, plan: &LedgerPlan) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 100_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_args(()).expect("encode init"), None);
    flaky_update(pic, id, "set_fee", (Nat::from(LEDGER_FEE),));
    for (who, balance) in &plan.balances {
        flaky_update(pic, id, "mint", (Account { owner: *who, subaccount: None }, Nat::from(*balance)));
    }
    id
}

/// Number of `2xfer` (icrc2_transfer_from) blocks in the flaky ledger's ICRC-3 log.
fn transfer_from_blocks(w: &BridgeWorld) -> usize {
    let res = ledger::icrc3_get_blocks(&w.pic, w.ledger, 0, 100);
    res.blocks
        .iter()
        .filter(|b| match &b.block {
            ICRC3Value::Map(m) => matches!(m.get("btype"), Some(ICRC3Value::Text(t)) if t == "2xfer"),
            _ => false,
        })
        .count()
}

fn flaky_world() -> BridgeWorld {
    BridgeWorld::setup_with_ledger(SetupOptions::default(), flaky_ledger)
}

/// `bridge_to_base` where the ledger commits the pull but the reply is lost (phantom failure).
/// Returns the world, the deposit id and the user's balance before the pull.
fn phantom_pull(w: &mut BridgeWorld, amount: u64, recipient: &str) -> (u64, u128) {
    let user = w.user;
    flaky_update(&w.pic, w.ledger, "set_phantom_failures", (1u32,));
    let before = w.user_balance(user);
    let dep = w.approve_and_deposit(user, amount, recipient);
    (dep.id, before)
}

/// The ledger executes `icrc2_transfer_from` but the reply is an error (phantom failure). The
/// first reply must not be read as "failed": the deposit is unresolved (`PullUnknown`), the
/// funds really moved, and the timer resends the IDENTICAL tuple; `Duplicate` proves success.
///
/// Real: the bridge wasm, the Base mint and delivery on anvil, the escrow coverage reads.
/// Mocked: the ledger (`flaky_ledger` test canister, not the production ledger wasm): it
/// commits the pull and returns a `GenericError` once (`set_phantom_failures(1)`).
/// Simulated: nothing.
///
/// `flaky_ledger` has no `icrc1_minting_account` and no `icrc1_total_supply`, so this test
/// stops before any release (a release is refused until the minting account is known).
#[test]
fn lost_pull_reply_resends_identical_tuple_and_duplicate_proves_single_pull() {
    let mut w = flaky_world();
    let (user, recipient) = (w.user, w.holder(0));
    let a = e8(45);
    let (id, user_before) = phantom_pull(&mut w, a, &recipient);

    // The reply was lost: unresolved, but the funds really moved exactly once.
    let dep = w.get_deposit(id).unwrap();
    assert_eq!(dep.status, DepositStatus::PullUnknown, "{dep:?}");
    assert!(dep.pull_block_index.is_none());
    assert_eq!(user_before - w.user_balance(user), (a + LEDGER_FEE) as u128, "the ledger really moved the funds");
    assert_eq!(w.escrow_balance(), a as u128);
    assert_eq!(transfer_from_blocks(&w), 1);
    let pre = w.supply_audit();
    assert_eq!(nat_u128(&pre.reserved_pulls_e8s), a as u128, "still reserved, not yet pending_mint: {pre:?}");
    assert_eq!(nat_u128(&pre.pending_mint_e8s), 0);
    assert_eq!(pre.outstanding(), 0);

    // The timer resends the same tuple; the ledger answers Duplicate; that proves the pull.
    let dep = w.wait_deposit(id, "Escrowed", |d| d.status != DepositStatus::PullUnknown);
    assert!(matches!(dep.status, DepositStatus::Escrowed | DepositStatus::MintInFlight | DepositStatus::Delivered), "{dep:?}");
    assert!(dep.pull_block_index.is_some(), "the duplicate_of block is the pull block");
    assert_eq!(user_before - w.user_balance(user), (a + LEDGER_FEE) as u128, "debited exactly once");
    assert_eq!(w.escrow_balance(), a as u128);
    assert_eq!(transfer_from_blocks(&w), 1, "the ledger log still has a single pull block");
    let audit = w.supply_audit();
    assert_eq!(nat_u128(&audit.reserved_pulls_e8s), 0);

    // The deposit continues to delivery on Base exactly once.
    w.wait_mint_included(id);
    w.mine_finalize();
    let dep = w.wait_delivered(id);
    assert_eq!(dep.mint_tx_hashes.len(), 1);
    assert_eq!(w.base_balance(&recipient), a as u128);
    assert_eq!(w.base_total_supply(), a as u128);
    assert_eq!(w.minter_nonce(), 1);
    assert_eq!(user_before - w.user_balance(user), (a + LEDGER_FEE) as u128);
    assert_eq!(transfer_from_blocks(&w), 1);
    let audit = settle_observer_and_reconcile(&mut w);
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), a as u128);
    assert_eq!(audit.escrow(), Some(a as u128));
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    assert!(w.bridge_info().halted.is_none());
}

/// Same lost pull reply, but the canister is restarted while the pull is unresolved:
/// first through stop/upgrade/start (heap wiped, stable state kept), then through a plain
/// stop/start. After each restart the reconciliation completes exactly once.
///
/// Real: the bridge wasm upgrade path and stable-memory persistence (PocketIC), the Base side.
/// Mocked: the ledger as in `lost_pull_reply_resends_identical_tuple_and_duplicate_proves_single_pull`.
/// Simulated: nothing.
#[test]
fn restart_with_unresolved_pull_completes_reconciliation_exactly_once() {
    let mut w = flaky_world();
    let (user, recipient) = (w.user, w.holder(0));

    // --- Leg 1: stop, upgrade (same wasm, post_upgrade(null)), start ---
    let a1 = e8(12);
    let (id1, before1) = phantom_pull(&mut w, a1, &recipient);
    let dep = w.get_deposit(id1).unwrap();
    assert_eq!(dep.status, DepositStatus::PullUnknown);
    let balance_unresolved = w.user_balance(user);
    assert_eq!(before1 - balance_unresolved, (a1 + LEDGER_FEE) as u128);
    w.upgrade_bridge_same_wasm();
    let dep = w.get_deposit(id1).unwrap();
    assert_eq!(dep.status, DepositStatus::PullUnknown, "the unresolved pull survived the upgrade: {dep:?}");
    assert_eq!(dep.amount_e8s, a1);
    assert_eq!(w.user_balance(user), balance_unresolved);
    assert_eq!(nat_u128(&w.supply_audit().reserved_pulls_e8s), a1 as u128, "reservation survived");
    let dep = w.wait_deposit(id1, "pull resolved", |d| d.status != DepositStatus::PullUnknown);
    assert!(!matches!(dep.status, DepositStatus::PullFailed { .. }), "{dep:?}");
    assert!(dep.pull_block_index.is_some());
    assert_eq!(before1 - w.user_balance(user), (a1 + LEDGER_FEE) as u128, "debited exactly once after restart");
    assert_eq!(transfer_from_blocks(&w), 1);

    // --- Leg 2: plain stop/start of the canister ---
    let a2 = e8(7);
    let (id2, before2) = phantom_pull(&mut w, a2, &recipient);
    assert_eq!(w.get_deposit(id2).unwrap().status, DepositStatus::PullUnknown);
    w.pump_idle();
    w.pic.stop_canister(w.bridge, None).expect("stop bridge");
    w.pic.start_canister(w.bridge, None).expect("start bridge");
    assert_eq!(w.get_deposit(id2).unwrap().status, DepositStatus::PullUnknown);
    let dep = w.wait_deposit(id2, "pull resolved", |d| d.status != DepositStatus::PullUnknown);
    assert!(!matches!(dep.status, DepositStatus::PullFailed { .. }), "{dep:?}");
    assert_eq!(before2 - w.user_balance(user), (a2 + LEDGER_FEE) as u128, "second pull debited exactly once");
    assert_eq!(transfer_from_blocks(&w), 2, "two pulls, two ledger blocks, no duplicates");

    // Both deposits finish on Base; the books balance.
    w.wait_mint_included(id1);
    w.wait_mint_included(id2);
    w.mine_finalize();
    w.wait_delivered(id1);
    w.wait_delivered(id2);
    assert_eq!(w.minter_nonce(), 2);
    assert_eq!(w.base_balance(&recipient), (a1 + a2) as u128);
    assert_eq!(w.escrow_balance(), (a1 + a2) as u128);
    let audit = settle_observer_and_reconcile(&mut w);
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), (a1 + a2) as u128);
    assert_eq!(audit.base_supply_exact_match, Some(true), "{audit:?}");
    assert!(w.bridge_info().halted.is_none());
}
