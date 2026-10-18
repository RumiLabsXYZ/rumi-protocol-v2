//! XRPL icUSD lane: LOCAL rehearsal harness against the PUBLIC XRPL TESTNET.
//!
//! This is tooling, not a regression test: both entry points are `#[ignore]`.
//!
//! * `probe_live_outcall` (2-minute feasibility probe): boots a live PocketIC, installs the
//!   testnet FIXTURE backend and checks that a REAL HTTPS outcall to the XRPL testnet
//!   (`account_info` from the issuer-setup step) is performed.
//! * `run_rehearsal_harness`: builds the full local instance (icUSD ledger, XRC mock, ICP
//!   ledger, Internet Identity dev, fixture backend at the exact mainnet canister ids the
//!   vault frontend hard-codes), exposes it on an HTTP gateway, configures + funds the
//!   issuer from the XRPL TESTNET faucet, runs issuer setup to Ready (real signed
//!   AccountSet transactions on testnet), writes `state.json` and then BLOCKS so the
//!   instance stays alive.
//!
//! Nothing here touches IC mainnet, XRPL mainnet or real funds: the fixture artifact is
//! pinned to the XRPL testnet and `test_key_1`; the local PocketIC replica is private.
//!
//! Run (absolute server path; pocket-ic crate 6.0.0 with the repo's 7.0.0 server):
//!   POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_protocol_backend \
//!     --test xrpl_icusd_testnet_rehearsal run_rehearsal_harness -- --ignored --nocapture
//!
//! Environment (all optional):
//!   REHEARSAL_DIR           work dir (default /private/tmp/claude-501/rehearsal)
//!   REHEARSAL_BACKEND_WASM  fixture backend wasm, raw or gzip (default $DIR/backend_fixture.wasm.gz)
//!   REHEARSAL_II_WASM       internet_identity_dev.wasm.gz, the II backend (default $DIR/ii.wasm.gz)
//!   REHEARSAL_II_FRONTEND_WASM  internet_identity_frontend.wasm.gz (default $DIR/ii_frontend.wasm.gz)
//!   REHEARSAL_SERVER_URL    long-lived pocket-ic server, e.g. http://127.0.0.1:4951 (start it with --ttl 86400)
//!   REHEARSAL_PORT          gateway port (default 4950; never 4943)

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Nat, Principal, Reserved};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::chains::xrp::XrplNetwork;
use rumi_protocol_backend::xrpl_icusd::{
    ConfigureXrplIcusdArg, CoverageReport, IssuerPolicy, IssuerSetupStatus, PublicXrplIcusdConfig,
    SetPausesArg, XrplIcusdError,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const E8: u64 = 100_000_000;
const LEDGER_FEE: u64 = 10_000;

// Exact ids the vault frontend hard-codes (config.ts CANISTER_IDS / LOCAL_CANISTER_IDS).
const BACKEND_ID: &str = "tfesu-vyaaa-aaaap-qrd7a-cai";
const ICUSD_ID: &str = "t6bor-paaaa-aaaap-qrd5q-cai";
const ICP_LEDGER_ID: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
/// Mainnet Internet Identity id (the ii subnet's range); the frontend override points at it.
const II_ID: &str = "rdmx6-jaaaa-aaaaa-aaadq-cai";

const TESTNET_RPC: &str = "https://s.altnet.rippletest.net:51234";
const FAUCET: &str = "https://faucet.altnet.rippletest.net/accounts";

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}
fn ctrl() -> Principal {
    Principal::from_slice(&[5, 5, 5, 5, 5, 5, 5, 5, 5])
}
fn treasury() -> Principal {
    Principal::from_slice(&[6, 6, 6, 6, 6, 6, 6, 6, 6])
}

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("REHEARSAL_DIR").unwrap_or_else(|_| "/private/tmp/claude-501/rehearsal".into()))
}
fn port() -> u16 {
    std::env::var("REHEARSAL_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(4950)
}
fn p(s: &str) -> Principal {
    Principal::from_text(s).unwrap()
}

fn log(msg: impl AsRef<str>) {
    println!("[rehearsal {}] {}", chrono_like(), msg.as_ref());
}
fn chrono_like() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}", d.as_secs())
}

// ─── Candid mirrors (same shapes as xrpl_icusd_e2e_pic.rs) ───────────────────

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
    Init(LedgerInitArgs),
}
#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    rates: HashMap<String, u64>,
}

fn read_wasm(env: &str, default_name: &str) -> Vec<u8> {
    let path = std::env::var(env).map(PathBuf::from).unwrap_or_else(|_| dir().join(default_name));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}
fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}
fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

fn ledger_init(minter: Principal, name: &str, fee: u64, balances: Vec<(Account, u64)>) -> Vec<u8> {
    encode_one(LedgerArg::Init(LedgerInitArgs {
        minting_account: Account { owner: minter, subaccount: None },
        fee_collector_account: None,
        transfer_fee: Nat::from(fee),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: name.to_string(),
        token_symbol: name.to_string(),
        metadata: vec![],
        initial_balances: balances.into_iter().map(|(a, n)| (a, Nat::from(n))).collect(),
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1000,
            trigger_threshold: 2000,
            controller_id: ctrl(),
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    }))
    .expect("ledger init")
}

// ─── Local instance ──────────────────────────────────────────────────────────

struct Local {
    pic: PocketIc,
    backend: Principal,
    icusd: Principal,
    icp: Principal,
    xrc: Principal,
    ii: Principal,
    ii_backend: Principal,
    /// Operator account that holds the genesis icUSD used to fund the escrow surplus.
    operator: Principal,
}

/// Create every canister at its id, install, then switch the instance to live mode.
fn build_local(operator_genesis_e8s: u64) -> Local {
    // The crate-spawned server dies after a 60s TTL once the client goes idle, so the
    // long-lived harness connects to a server started separately with a long TTL:
    //   pocket-ic --port 4951 --ttl 86400
    let mut builder = PocketIcBuilder::new().with_nns_subnet().with_ii_subnet().with_application_subnet();
    if let Ok(u) = std::env::var("REHEARSAL_SERVER_URL") {
        builder = builder.with_server_url(u.parse().expect("REHEARSAL_SERVER_URL"));
    }
    let mut pic = builder.build();
    let (backend, icusd, icp, ii) = (p(BACKEND_ID), p(ICUSD_ID), p(ICP_LEDGER_ID), p(II_ID));
    for (name, id) in [("backend", backend), ("icusd", icusd), ("icp ledger", icp), ("ii", ii)] {
        pic.create_canister_with_id(None, None, id)
            .unwrap_or_else(|e| panic!("create_canister_with_id {name} {id}: {e}"));
        pic.add_cycles(id, 100_000_000_000_000);
        log(format!("created {name} at {id} (subnet {:?})", pic.get_subnet(id)));
    }
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 100_000_000_000_000);

    // icUSD: minting account = backend (the lane's mint authority). The operator receives a
    // genesis balance standing in for "the operator's own canonical icUSD" (launch step 5).
    let operator = ctrl();
    pic.install_canister(
        icusd,
        ledger_wasm(),
        ledger_init(backend, "icUSD", LEDGER_FEE, vec![(Account { owner: operator, subaccount: None }, operator_genesis_e8s)]),
        None,
    );
    pic.install_canister(icp, ledger_wasm(), ledger_init(backend, "ICP", LEDGER_FEE, vec![]), None);

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
    pic.install_canister(backend, read_wasm("REHEARSAL_BACKEND_WASM", "backend_fixture.wasm.gz"), encode_args((init,)).unwrap(), None);
    pic.set_controllers(backend, None, vec![ctrl()]).expect("set controllers");

    // Internet Identity (release-2026-05-26-be) is split in two canisters: the DEV backend and
    // the asset-serving frontend. The frontend sits at the mainnet II id (the id the vault
    // frontend override uses in the browser URL); the backend gets any id on the II subnet.
    // dummy_auth is enabled on both so a rehearsal login needs no passkey/captcha.
    let ii_subnet = pic.get_subnet(ii).expect("ii subnet");
    let ii_backend = pic.create_canister_on_subnet(None, None, ii_subnet);
    pic.add_cycles(ii_backend, 100_000_000_000_000);
    let backend_arg = candid_parser::parse_idl_args(
        "(opt record { captcha_config = opt record { max_unsolved_captchas = 50 : nat64; captcha_trigger = variant { Static = variant { CaptchaDisabled } } }; dummy_auth = opt opt record { prompt_for_index = false } })",
    )
    .expect("ii backend arg")
    .to_bytes()
    .expect("ii backend arg bytes");
    pic.install_canister(ii_backend, read_wasm("REHEARSAL_II_WASM", "ii.wasm.gz"), backend_arg, None);
    let frontend_arg = candid_parser::parse_idl_args(&format!(
        "(record {{ backend_canister_id = principal \"{ii_backend}\"; backend_origin = \"http://localhost:{}\"; fetch_root_key = opt true; dev_csp = opt true; dummy_auth = opt opt record {{ prompt_for_index = false }} }})",
        port()
    ))
    .expect("ii frontend arg")
    .to_bytes()
    .expect("ii frontend arg bytes");
    pic.install_canister(ii, read_wasm("REHEARSAL_II_FRONTEND_WASM", "ii_frontend.wasm.gz"), frontend_arg, None);

    for _ in 0..10 {
        pic.tick();
    }
    let url = pic.make_live(Some(port()));
    log(format!("live gateway: {url}"));
    Local { pic, backend, icusd, icp, xrc, ii, ii_backend, operator }
}

impl Local {
    /// Update call that survives slow real HTTPS outcalls. The server answers an ingress
    /// await with "Failed to answer ... after 100 rounds" once 100 (fast, live-mode) rounds
    /// pass, which the client surfaces as a panic; the message is still pending, so wait on
    /// the same message id again until a real deadline.
    fn update(&self, sender: Principal, method: &str, args: Vec<u8>) -> Vec<u8> {
        let msg = self
            .pic
            .submit_call(self.backend, sender, method, args)
            .unwrap_or_else(|e| panic!("submit {method}: {e}"));
        let deadline = Instant::now() + Duration::from_secs(600);
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = loop {
            let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.pic.await_call(msg.clone())));
            match attempt {
                Ok(r) => break r.unwrap_or_else(|e| panic!("await {method}: {e}")),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(500)),
                Err(_) => {
                    std::panic::set_hook(prev_hook);
                    panic!("{method} did not complete within 600s");
                }
            }
        };
        std::panic::set_hook(prev_hook);
        match result {
            WasmResult::Reply(b) => b,
            WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
        }
    }
    fn typed<T: CandidType + for<'a> Deserialize<'a>>(&self, sender: Principal, method: &str, args: Vec<u8>) -> T {
        Decode!(&self.update(sender, method, args), T).unwrap_or_else(|e| panic!("decode {method}: {e}"))
    }
    fn ok_unit(&self, sender: Principal, method: &str, args: Vec<u8>) {
        let r: Result<(), Reserved> = self.typed(sender, method, args);
        r.unwrap_or_else(|_| panic!("{method} returned Err"));
    }
    fn query<T: CandidType + for<'a> Deserialize<'a>>(&self, method: &str, args: Vec<u8>) -> T {
        match self
            .pic
            .query_call(self.backend, Principal::anonymous(), method, args)
            .unwrap_or_else(|e| panic!("query {method}: {e}"))
        {
            WasmResult::Reply(b) => Decode!(&b, T).unwrap_or_else(|e| panic!("decode {method}: {e}")),
            WasmResult::Reject(m) => panic!("query {method} rejected: {m}"),
        }
    }
    fn config(&self) -> PublicXrplIcusdConfig {
        self.query("get_xrpl_icusd_config", Encode!().unwrap())
    }
    fn coverage(&self) -> CoverageReport {
        let r: Result<CoverageReport, XrplIcusdError> = self.typed(ctrl(), "get_xrpl_icusd_coverage", Encode!().unwrap());
        r.expect("coverage")
    }
    fn icusd_balance(&self, acct: &Account) -> u128 {
        match self.pic.query_call(self.icusd, Principal::anonymous(), "icrc1_balance_of", Encode!(acct).unwrap()).unwrap() {
            WasmResult::Reply(b) => u128::try_from(Decode!(&b, Nat).unwrap().0).unwrap(),
            WasmResult::Reject(m) => panic!("{m}"),
        }
    }
    fn configure_testnet_lane(&self) -> PublicXrplIcusdConfig {
        self.ok_unit(dev(), "set_xrp_schnorr_key_name", Encode!(&"test_key_1".to_string()).unwrap());
        self.ok_unit(dev(), "register_xrp_collateral", Encode!().unwrap());
        let cfg: Result<PublicXrplIcusdConfig, XrplIcusdError> = self.typed(
            ctrl(),
            "configure_xrpl_icusd",
            Encode!(&ConfigureXrplIcusdArg {
                network: Some(XrplNetwork::Testnet),
                policy: Some(IssuerPolicy { allow_clawback: false, no_freeze: false }),
                enabled: Some(true),
                outstanding_cap_e8s: Some(1_000 * E8),
                per_operation_cap_e8s: Some(100 * E8),
                rounding_buffer_e8s: Some(E8 / 100),
                max_fee_drops: Some(1_000),
                min_auto_refund_e8s: None,
            })
            .unwrap(),
        );
        cfg.expect("configure_xrpl_icusd")
    }
}

// ─── XRPL testnet helpers (public testnet only) ──────────────────────────────

fn curl_json(args: &[&str]) -> Value {
    let out = Command::new("curl").args(["-sS", "--max-time", "60"]).args(args).output().expect("curl");
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    serde_json::from_str(&s).unwrap_or_else(|_| json!({ "raw": s, "stderr": String::from_utf8_lossy(&out.stderr) }))
}
fn xrpl_rpc(method: &str, params: Value) -> Value {
    let body = json!({ "method": method, "params": [params] }).to_string();
    curl_json(&["-X", "POST", "-H", "Content-Type: application/json", "-d", &body, TESTNET_RPC])
}
fn faucet_fund(address: &str) -> Value {
    let body = json!({ "destination": address }).to_string();
    curl_json(&["-X", "POST", "-H", "Content-Type: application/json", "-d", &body, FAUCET])
}
fn account_balance_drops(address: &str) -> Option<u64> {
    xrpl_rpc("account_info", json!({ "account": address, "ledger_index": "validated" }))["result"]["account_data"]["Balance"]
        .as_str()
        .and_then(|b| b.parse().ok())
}

fn hashes_in(s: &str) -> BTreeSet<String> {
    // 64-char uppercase hex runs.
    let mut out = BTreeSet::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let mut j = i;
        while j < b.len() && (b[j].is_ascii_digit() || ('A'..='F').contains(&b[j])) {
            j += 1;
        }
        if j - i == 64 {
            out.insert(b[i..j].iter().collect());
        }
        i = j.max(i + 1);
    }
    out
}

// ─── Entry points ────────────────────────────────────────────────────────────

/// Feasibility probe: does live PocketIC perform a REAL outcall to the XRPL testnet?
#[test]
#[ignore]
fn probe_live_outcall() {
    std::fs::create_dir_all(dir()).unwrap();
    let l = build_local(10 * E8);
    let cfg = l.configure_testnet_lane();
    let issuer = cfg.issuer_address.clone().expect("issuer address (test_key_1 Ed25519 works)");
    log(format!("issuer {issuer}"));
    let acct: Result<Account, XrplIcusdError> = l.typed(ctrl(), "verify_xrpl_icusd_minting_account", Encode!().unwrap());
    log(format!("minting account verified: {:?}", acct.is_ok()));
    // The setup step performs account_info against the testnet: an unfunded issuer is a
    // clean "account not found" rather than an outcall failure.
    let t = Instant::now();
    let st: Result<IssuerSetupStatus, XrplIcusdError> = l.typed(ctrl(), "run_xrpl_issuer_setup_step", Encode!().unwrap());
    log(format!("setup step after {:?}: {:?}", t.elapsed(), st));
}

#[test]
#[ignore]
fn run_rehearsal_harness() {
    let work = dir();
    std::fs::create_dir_all(&work).unwrap();
    let l = build_local(10 * E8);

    // ── economics: XRP collateral ──
    let xrp = Principal::from_slice(b"rumi-xrp-native");
    let cfg = l.configure_testnet_lane();
    l.ok_unit(dev(), "set_borrowing_fee_curve", Encode!(&Option::<String>::None).unwrap());
    l.ok_unit(dev(), "set_collateral_borrowing_fee", Encode!(&xrp, &0.01f64).unwrap());
    l.ok_unit(dev(), "set_interest_rate", Encode!(&xrp, &0.005f64).unwrap());
    l.ok_unit(dev(), "set_collateral_debt_ceiling", Encode!(&xrp, &(2_000u64 * E8)).unwrap());
    let issuer = cfg.issuer_address.clone().expect("issuer address");
    log(format!("issuer address (derived, test_key_1): {issuer}"));
    assert!(!cfg.ready);

    let acct: Result<Account, XrplIcusdError> = l.typed(ctrl(), "verify_xrpl_icusd_minting_account", Encode!().unwrap());
    let acct = acct.expect("minting account verified");
    assert_eq!(acct.owner, l.backend);
    log("minting account verified (= backend default account)");

    // ── fund the issuer from the XRPL TESTNET faucet ──
    if account_balance_drops(&issuer).map_or(true, |b| b < 20_000_000) {
        let fund = faucet_fund(&issuer);
        log(format!("faucet response: {fund}"));
    } else {
        log("issuer already funded on testnet; skipping faucet");
    }
    let t = Instant::now();
    let issuer_balance = loop {
        if let Some(b) = account_balance_drops(&issuer) {
            break b;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "issuer never appeared funded on testnet");
        std::thread::sleep(Duration::from_secs(4));
    };
    log(format!("issuer funded: {issuer_balance} drops (validated)"));

    // ── fund the escrow surplus with canonical icUSD from the operator account ──
    let cov = l.coverage();
    log(format!("coverage before surplus: {cov:?}"));
    let surplus = (cov.fee_surplus_required_e8s as u64).max(E8) + 2 * E8; // required + headroom
    let escrow = cfg.escrow_account;
    let transfer = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: escrow,
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(surplus),
    };
    match l.pic.update_call(l.icusd, l.operator, "icrc1_transfer", Encode!(&transfer).unwrap()).unwrap() {
        WasmResult::Reply(b) => {
            let r = Decode!(&b, Result<Nat, Reserved>).unwrap();
            assert!(r.is_ok(), "escrow transfer failed");
        }
        WasmResult::Reject(m) => panic!("{m}"),
    }
    log(format!("escrow funded with {surplus} e8s; balance {}", l.icusd_balance(&escrow)));

    // ── issuer setup to Ready (real signed AccountSet txs on testnet) ──
    let mut setup_hashes: BTreeSet<String> = BTreeSet::new();
    let mut transcript: Vec<String> = vec![];
    let t = Instant::now();
    loop {
        let st: Result<IssuerSetupStatus, XrplIcusdError> = l.typed(ctrl(), "run_xrpl_issuer_setup_step", Encode!().unwrap());
        let s = format!("{st:?}");
        log(format!("setup step: {s}"));
        setup_hashes.extend(hashes_in(&s));
        transcript.push(s.clone());
        if let Ok(ok) = &st {
            if format!("{:?}", ok.setup).starts_with("Ready") {
                break;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(900), "issuer setup did not reach Ready in 15 minutes");
        std::thread::sleep(Duration::from_secs(5));
    }
    let cfg = l.config();
    setup_hashes.extend(hashes_in(&format!("{cfg:?}")));
    // The setup status carries only ledger indices, so read the issuer's validated history
    // from the public testnet: every AccountSet it signed is listed there with its hash.
    let hist = xrpl_rpc("account_tx", json!({ "account": issuer, "ledger_index_min": -1, "ledger_index_max": -1, "limit": 50 }));
    for t in hist["result"]["transactions"].as_array().cloned().unwrap_or_default() {
        if t["tx_json"]["TransactionType"] == "AccountSet" || t["tx"]["TransactionType"] == "AccountSet" {
            if let Some(h) = t["hash"].as_str().or_else(|| t["tx"]["hash"].as_str()) {
                setup_hashes.insert(h.to_string());
            }
        }
    }
    assert!(cfg.ready || format!("{:?}", cfg.issuer_setup).starts_with("Ready"), "{:?}", cfg.issuer_setup);

    // ── validate each AccountSet on the public testnet ──
    let mut validated = vec![];
    for h in &setup_hashes {
        let r = xrpl_rpc("tx", json!({ "transaction": h, "binary": false }));
        let res = &r["result"];
        log(format!(
            "tx {h}: type={} validated={} result={}",
            res["TransactionType"], res["validated"], res["meta"]["TransactionResult"]
        ));
        validated.push(json!({
            "hash": h,
            "type": res["TransactionType"],
            "validated": res["validated"],
            "result": res["meta"]["TransactionResult"],
            "ledger_index": res["ledger_index"],
        }));
    }
    let info = xrpl_rpc("account_info", json!({ "account": issuer, "ledger_index": "validated" }));
    let flags = info["result"]["account_data"]["Flags"].clone();
    log(format!("issuer account Flags on testnet: {flags} (DefaultRipple 0x00800000, RequireDest 0x00020000)"));

    // ── unpause issuance (LOCAL instance only) ──
    let r: Result<PublicXrplIcusdConfig, XrplIcusdError> = l.typed(
        ctrl(),
        "set_xrpl_icusd_pauses",
        Encode!(&SetPausesArg { issuance_paused: Some(false), settlement_paused: Some(false) }).unwrap(),
    );
    let cfg = r.expect("unpause");
    assert!(cfg.ready, "lane not ready after unpause: {cfg:?}");

    let cov = l.coverage();
    let url = l.pic.url().map(|u| u.to_string());
    let state = json!({
        "gateway": format!("http://localhost:{}", port()),
        "pocket_ic_url": url,
        "canisters": {
            "rumi_protocol_backend": l.backend.to_text(),
            "icusd_ledger": l.icusd.to_text(),
            "icp_ledger": l.icp.to_text(),
            "xrc_mock": l.xrc.to_text(),
            "internet_identity_frontend": l.ii.to_text(),
            "internet_identity_backend": l.ii_backend.to_text(),
        },
        "ii_url": format!("http://{}.localhost:{}", l.ii, port()),
        "principals": {
            "developer": dev().to_text(),
            "controller_and_operator": ctrl().to_text(),
            "treasury": treasury().to_text(),
        },
        "xrpl": {
            "network": "testnet",
            "rpc": TESTNET_RPC,
            "issuer_address": issuer,
            "issuer_balance_drops_after_faucet": issuer_balance,
            "issuer_flags_validated": flags,
            "issuer_setup_tx_hashes": validated,
            "currency_hex": cfg.currency_hex,
        },
        "escrow": {
            "account_owner": escrow.owner.to_text(),
            "funded_e8s": surplus,
            "balance_e8s": l.icusd_balance(&escrow).to_string(),
            "fee_surplus_required_e8s": cov.fee_surplus_required_e8s.to_string(),
            "coverage_status": format!("{:?}", cov.status),
        },
        "lane_ready": cfg.ready,
        "setup_transcript": transcript,
        "pid": std::process::id(),
    });
    std::fs::write(work.join("state.json"), serde_json::to_string_pretty(&state).unwrap()).unwrap();
    log(format!("READY. state written to {}", work.join("state.json").display()));
    log(format!("gateway http://localhost:{}  backend {}  icusd {}  II {}", port(), l.backend, l.icusd, l.ii));
    log(format!("BLOCKING (pid {}). Kill this process to stop the instance.", std::process::id()));
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
