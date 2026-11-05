//! Isolated PocketIC smoke for the test_endpoints-gated native-XLM address open.
//!
//! Requires the backend Wasm at the conventional release target path to have
//! been built with `--features test_endpoints`. PocketIC's II subnet must
//! provision `test_key_1` for the threshold-Ed25519 public-key call.

use candid::{
    decode_one, encode_args, encode_one, CandidType, Deserialize, Encode, Nat, Principal,
};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

#[derive(CandidType, Deserialize)]
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

#[derive(CandidType, Deserialize)]
enum ProtocolArg {
    Init(ProtocolInitArg),
}

#[derive(CandidType, Deserialize)]
struct XlmVaultOpenInfo {
    vault_id: u64,
    custody_address: String,
    network_passphrase: String,
    expires_at_ns: u64,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum XlmPendingDepositStatus {
    Pending,
    Open,
    Expired,
    Cancelled,
}

#[derive(CandidType, Deserialize, Debug)]
struct XlmPendingDepositView {
    vault_id: u64,
    custody_address: String,
    network_passphrase: String,
    opened_at_ns: u64,
    expires_at_ns: u64,
    status: XlmPendingDepositStatus,
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
    #[serde(rename = "Upgrade")]
    Upgrade(Option<()>),
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

fn developer() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Success,
    /// Provider B reports a different deposit amount: no 2-of-3 agreement on the
    /// full deposit list is possible when A, B and C all differ.
    Disagree,
}

const TX_HASH: &str = "ef0fe04ac3c7de7228ca2598886059868ad05c224a041e8b2d9ee2a8a9dd6894";
const TX_LEDGER: u64 = 1_000;
const RESERVE_LEDGER: u64 = 1_005;

fn iso(pic: &PocketIc, back_secs: u64) -> String {
    let secs = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - back_secs;
    // civil-from-days (Howard Hinnant)
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Raw Horizon-shaped body for `url`; provider "b" deviates under `Disagree`.
fn horizon_body(url: &str, custody: &str, scenario: Scenario, closed_at: &str) -> String {
    // Keyed by the three REAL compiled-in testnet provider base URLs. Ankr is
    // the deviating provider under `Disagree`; Ankr's base carries a path prefix.
    const SDF: &str = "https://horizon-testnet.stellar.org";
    const ANKR: &str = "https://rpc.ankr.com/http/stellar_testnet_horizon";
    const TATUM: &str = "https://stellar-testnet.gateway.tatum.io";
    let (base, rest) = [SDF, ANKR, TATUM]
        .iter()
        .find_map(|b| {
            url.strip_prefix(b)
                .filter(|r| r.is_empty() || r.starts_with('/'))
                .map(|r| (*b, r.trim_start_matches('/')))
        })
        .unwrap_or_else(|| panic!("unexpected provider URL {url}"));
    let host_b = base == ANKR;
    let amount = if scenario == Scenario::Disagree && host_b { "50.0000000" } else { "100.0000000" };
    let op_id = ((TX_LEDGER << 32) | 1).to_string();
    if rest.is_empty() {
        r#"{"network_passphrase":"Test SDF Network ; September 2015"}"#.to_string()
    } else if rest.starts_with("ledgers") {
        format!(
            r#"{{"_embedded":{{"records":[{{"sequence":{RESERVE_LEDGER},"base_reserve_in_stroops":5000000,"closed_at":"{closed_at}"}}]}}}}"#
        )
    } else if rest.starts_with("accounts/") {
        format!(
            r#"{{"account_id":"{custody}","sequence":"4294967296000","last_modified_ledger":{TX_LEDGER},"account_last_modified_ledger":{TX_LEDGER},"subentry_count":0,"num_sponsoring":0,"num_sponsored":0,"balances":[{{"asset_type":"native","balance":"100.0000000","selling_liabilities":"0.0000000"}}]}}"#
        )
    } else if rest.contains("/operations") {
        format!(
            r#"{{"_embedded":{{"records":[{{"id":"{op_id}","paging_token":"{op_id}","transaction_hash":"{TX_HASH}","transaction_successful":true,"type":"create_account","account":"{custody}","starting_balance":"{amount}"}}]}}}}"#
        )
    } else if rest.starts_with("transactions/") {
        format!(r#"{{"hash":"{TX_HASH}","successful":true,"ledger":{TX_LEDGER}}}"#)
    } else {
        panic!("unexpected Horizon URL {url}")
    }
}

struct Env {
    pic: PocketIc,
    backend: Principal,
    ledger: Principal,
    vault_id: u64,
    custody: String,
}

fn boot() -> Env {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();
    let backend = pic.create_canister();
    pic.add_cycles(backend, 100_000_000_000_000);
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 100_000_000_000_000);
    let ledger_init = LedgerInitArgs {
        minting_account: Account { owner: backend, subaccount: None },
        fee_collector_account: None,
        transfer_fee: Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "Test icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1_000,
            trigger_threshold: 2_000,
            controller_id: backend,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(ledger, ledger_wasm(), encode_one(LedgerArg::Init(ledger_init)).unwrap(), None);
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: 10_000,
        developer_principal: developer(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(backend, backend_wasm(), encode_args((init,)).unwrap(), None);
    for _ in 0..5 {
        pic.tick();
    }
    let reply = pic
        .update_call(backend, developer(), "open_xlm_vault", encode_args(()).unwrap())
        .expect("open_xlm_vault");
    let opened: Result<XlmVaultOpenInfo, candid::Reserved> = match reply {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(m) => panic!("{m}"),
    };
    let opened = opened.expect("open");
    Env { pic, backend, ledger, vault_id: opened.vault_id, custody: opened.custody_address }
}

/// Submit confirm_xlm_deposit and answer every parked outcall with Horizon JSON.
fn confirm(env: &Env, scenario: Scenario) -> Result<u64, String> {
    let pic = &env.pic;
    let msg = pic
        .submit_call(
            env.backend,
            developer(),
            "confirm_xlm_deposit",
            encode_args((env.vault_id, TX_HASH.to_string())).unwrap(),
        )
        .expect("submit confirm");
    let closed_at = iso(pic, 10);
    for _ in 0..120 {
        pic.tick();
        for req in pic.get_canister_http() {
            let body = horizon_body(&req.url, &env.custody, scenario, &closed_at);
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: req.subnet_id,
                request_id: req.request_id,
                response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
                    status: 200,
                    headers: vec![],
                    body: body.into_bytes(),
                }),
                additional_responses: vec![],
            });
        }
    }
    match pic.await_call(msg).expect("await confirm") {
        WasmResult::Reply(b) => {
            let r: Result<u64, candid::Reserved> = decode_one(&b).expect("decode confirm");
            match r {
                Ok(v) => Ok(v),
                Err(_) => Err(candid::IDLArgs::from_bytes(&b).map(|a| a.to_string()).unwrap_or_default()),
            }
        }
        WasmResult::Reject(m) => Err(m),
    }
}

fn vault_count(env: &Env) -> u64 {
    match env
        .pic
        .query_call(env.backend, Principal::anonymous(), "get_vault_count", Encode!().unwrap())
        .unwrap()
    {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(m) => panic!("{m}"),
    }
}

#[derive(CandidType, Deserialize, Debug)]
struct VaultView {
    owner: Principal,
    borrowed_icusd_amount: u64,
    icp_margin_amount: u64,
    vault_id: u64,
    collateral_amount: u64,
    collateral_type: Principal,
    accrued_interest: u64,
}

/// `get_vaults(opt owner)` query: the owner-scoped vault list.
fn vaults_of(env: &Env, owner: Principal) -> Vec<VaultView> {
    match env
        .pic
        .query_call(env.backend, owner, "get_vaults", encode_one(Some(owner)).unwrap())
        .unwrap()
    {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(m) => panic!("{m}"),
    }
}

fn pending_count(env: &Env) -> usize {
    match env
        .pic
        .query_call(env.backend, developer(), "get_my_xlm_pending_deposits", encode_args(()).unwrap())
        .unwrap()
    {
        WasmResult::Reply(b) => decode_one::<Vec<XlmPendingDepositView>>(&b).unwrap().len(),
        WasmResult::Reject(m) => panic!("{m}"),
    }
}

#[test]
fn confirm_xlm_deposit_success_replay_and_disagreement() {
    // Same synthetic key the backend uses for native XLM
    // (`rumi_protocol_backend::state::xlm_collateral_principal()`).
    let xlm = Principal::from_slice(b"rumi-xlm-native");

    // (b) provider disagreement: no vault, pending row stays.
    let env = boot();
    assert!(confirm(&env, Scenario::Disagree).is_err(), "disagreement must fail");
    assert_eq!(vault_count(&env), 0);
    assert!(vaults_of(&env, developer()).is_empty(), "no vault for owner after disagreement");
    assert_eq!(pending_count(&env), 1);

    // (a) success on the same canister after providers agree again.
    // 100 XLM balance - 1 XLM minimum (2 x 0.5 base reserve) - 10_000 fee budget.
    let credited = confirm(&env, Scenario::Success).expect("agreeing providers must credit");
    assert_eq!(credited, 989_990_000);
    assert_eq!(vault_count(&env), 1);
    assert_eq!(pending_count(&env), 0, "pending row consumed");
    let vaults = vaults_of(&env, developer());
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0].vault_id, env.vault_id);
    assert_eq!(vaults[0].owner, developer());
    assert_eq!(vaults[0].collateral_type, xlm);
    assert_eq!(vaults[0].collateral_amount, 989_990_000);
    assert_eq!(vaults[0].borrowed_icusd_amount, 0);

    // (c) replay of the same hash: error, still exactly one vault, unchanged.
    assert!(confirm(&env, Scenario::Success).is_err(), "replay must fail");
    assert_eq!(vault_count(&env), 1);
    let after = vaults_of(&env, developer());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].owner, developer());
    assert_eq!(after[0].collateral_type, xlm);
    assert_eq!(after[0].collateral_amount, 989_990_000);
    assert_eq!(after[0].borrowed_icusd_amount, 0);

    // (d) test-build-only: register XLM Active, mock price, borrow through the
    // normal endpoint as the vault owner.
    let ledger = env.ledger;
    let call = |sender: Principal, method: &str, args: Vec<u8>| -> Vec<u8> {
        match env.pic.update_call(env.backend, sender, method, args).unwrap() {
            WasmResult::Reply(b) => b,
            WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
        }
    };
    let reg_bytes = call(developer(), "dev_register_xlm_collateral", encode_args(()).unwrap());
    let reg: Result<(), candid::Reserved> = decode_one(&reg_bytes).unwrap();
    reg.unwrap_or_else(|_| panic!("register xlm: {}", candid::IDLArgs::from_bytes(&reg_bytes).unwrap()));
    let price: Result<String, candid::Reserved> = decode_one(&call(
        developer(),
        "dev_set_collateral_price",
        encode_args((xlm, 0.10f64)).unwrap(),
    ))
    .unwrap();
    price.expect("set price");

    #[derive(CandidType, Deserialize)]
    struct VaultArg {
        vault_id: u64,
        amount: u64,
    }
    let bal = |who: Principal| -> u64 {
        let r = env
            .pic
            .query_call(
                ledger,
                Principal::anonymous(),
                "icrc1_balance_of",
                Encode!(&Account { owner: who, subaccount: None }).unwrap(),
            )
            .unwrap();
        match r {
            WasmResult::Reply(b) => {
                let n: Nat = decode_one(&b).unwrap();
                n.0.try_into().unwrap()
            }
            WasmResult::Reject(m) => panic!("{m}"),
        }
    };
    let before = bal(developer());
    // 98.999 XLM at $0.10 = ~$9.90 collateral; 5 icUSD debt is well inside limits.
    let borrow_amount: u64 = 5 * 100_000_000;
    let r: Result<candid::Reserved, candid::Reserved> = decode_one(&call(
        developer(),
        "borrow_from_vault",
        Encode!(&VaultArg { vault_id: env.vault_id, amount: borrow_amount }).unwrap(),
    ))
    .unwrap();
    assert!(r.is_ok(), "in-limit XLM borrow must succeed");
    let v = vaults_of(&env, developer());
    assert_eq!(v[0].borrowed_icusd_amount, borrow_amount, "debt = borrowed amount");
    let minted = bal(developer()) - before;
    let fee = borrow_amount - minted;
    eprintln!("XLM borrow: debt={} minted={} fee_deducted={}", v[0].borrowed_icusd_amount, minted, fee);
    assert!(minted > 0 && minted <= borrow_amount, "minted net of borrow fee");
    assert!(fee <= borrow_amount / 50, "fee is a small borrowing fee (<=2%)");

    // (e) over-limit borrow: 50 icUSD against ~$9.90 collateral must fail, unchanged.
    let bal_mid = bal(developer());
    let r: Result<candid::Reserved, candid::Reserved> = decode_one(&call(
        developer(),
        "borrow_from_vault",
        Encode!(&VaultArg { vault_id: env.vault_id, amount: 50 * 100_000_000 }).unwrap(),
    ))
    .unwrap();
    assert!(r.is_err(), "over-limit borrow must be rejected");
    let v2 = vaults_of(&env, developer());
    assert_eq!(v2[0].borrowed_icusd_amount, borrow_amount, "debt unchanged");
    assert_eq!(bal(developer()), bal_mid, "icUSD balance unchanged");
}
