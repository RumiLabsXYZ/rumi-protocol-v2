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
use sha2::Digest;
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

fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    rates: std::collections::HashMap<String, u64>,
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
    // XRC mock (same wasm as xrp_native_e2e_pic): validate_call() refreshes the
    // ICP price on demand, which fails against an unroutable XRC.
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 100_000_000_000_000);
    let mut rates = std::collections::HashMap::new();
    rates.insert("ICP/USD".to_string(), 10 * 100_000_000u64);
    pic.install_canister(xrc, xrc_wasm(), encode_one(MockXRC { rates }).unwrap(), None);
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: xrc,
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

    // (f) REPAY through the generic `repay_to_vault` route (no XLM guard on it;
    // the backend is the icUSD minting account, so the transfer_from pull of
    // icUSD into the backend account is a burn).
    let total_supply = || -> u64 {
        match env
            .pic
            .query_call(ledger, Principal::anonymous(), "icrc1_total_supply", Encode!().unwrap())
            .unwrap()
        {
            WasmResult::Reply(b) => {
                let n: Nat = decode_one(&b).unwrap();
                n.0.try_into().unwrap()
            }
            WasmResult::Reject(m) => panic!("{m}"),
        }
    };
    let ledger_fee: u64 = 10_000;
    // Interest accrues on the vault between calls, so debt is checked against a
    // small bounded tolerance (0.01 icUSD) instead of exact equality.
    let tol: u64 = 1_000_000;
    let outsider = Principal::from_slice(&[0xAB, 0xCD, 0x01]);
    let repay_amount: u64 = 2 * 100_000_000;

    // Owner approves the backend to pull the repay amount (approve fee is paid
    // here, so snapshot balance/supply AFTER the approve).
    let approve_args = icrc_ledger_types::icrc2::approve::ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: env.backend, subaccount: None },
        amount: Nat::from(repay_amount + ledger_fee),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let ap: Result<Nat, candid::Reserved> = match env
        .pic
        .update_call(ledger, developer(), "icrc2_approve", Encode!(&approve_args).unwrap())
        .unwrap()
    {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(m) => panic!("icrc2_approve rejected: {m}"),
    };
    assert!(ap.is_ok(), "icrc2_approve must succeed");

    let debt_of = || vaults_of(&env, developer())[0].borrowed_icusd_amount;
    let assert_vault_identity = || {
        let v = vaults_of(&env, developer());
        assert_eq!(v.len(), 1, "still exactly one vault");
        assert_eq!(v[0].vault_id, env.vault_id);
        assert_eq!(v[0].owner, developer(), "owner unchanged");
        assert_eq!(v[0].collateral_type, xlm, "collateral_type unchanged");
        assert_eq!(v[0].collateral_amount, 989_990_000, "collateral_amount unchanged");
    };
    let repay_call = |sender: Principal, amount: u64| -> Result<candid::Reserved, candid::Reserved> {
        decode_one(&call(
            sender,
            "repay_to_vault",
            Encode!(&VaultArg { vault_id: env.vault_id, amount }).unwrap(),
        ))
        .unwrap()
    };

    // (f1) Outsider repay is rejected (CallerNotOwner); nothing mutates.
    let debt_pre = debt_of();
    let owner_bal_pre = bal(developer());
    let outsider_bal_pre = bal(outsider);
    let supply_pre = total_supply();
    assert!(repay_call(outsider, repay_amount).is_err(), "outsider repay must be rejected");
    let debt_post = debt_of();
    // Interest accrual runs before the owner check, so allow a tiny bounded rise.
    assert!(
        debt_post >= debt_pre && debt_post <= debt_pre + tol,
        "outsider repay must not reduce debt: {debt_pre} -> {debt_post}"
    );
    assert_eq!(bal(developer()), owner_bal_pre, "owner balance unchanged by outsider");
    assert_eq!(bal(outsider), outsider_bal_pre, "outsider balance unchanged");
    assert_eq!(total_supply(), supply_pre, "supply unchanged by rejected outsider repay");
    assert_vault_identity();

    // (f2) Invalid repay: 0 icUSD is below min_icusd_amount (0.1) -> AmountTooLow.
    assert!(repay_call(developer(), 0).is_err(), "zero repay must be rejected");
    let debt_post2 = debt_of();
    assert!(
        debt_post2 >= debt_post && debt_post2 <= debt_post + tol,
        "zero repay must not change debt: {debt_post} -> {debt_post2}"
    );
    assert_eq!(bal(developer()), owner_bal_pre, "owner balance unchanged by zero repay");
    assert_eq!(total_supply(), supply_pre, "supply unchanged by zero repay");
    assert_vault_identity();

    // (f3) Owner partial repay of 2 icUSD (3 icUSD remains, above min_vault_debt
    // and outside the 1% full-repay dust window).
    let owner_bal_before = bal(developer());
    let supply_before = total_supply();
    let debt_before = debt_of();
    assert!(repay_call(developer(), repay_amount).is_ok(), "owner partial repay must succeed");
    let owner_debit = owner_bal_before - bal(developer());
    assert!(
        owner_debit >= repay_amount && owner_debit <= repay_amount + ledger_fee,
        "owner debited the repaid amount (burn; at most one ledger fee extra): {owner_debit}"
    );
    let supply_drop = supply_before - total_supply();
    assert!(
        supply_drop >= repay_amount && supply_drop <= repay_amount + ledger_fee,
        "icUSD total supply drops by the burned repay: {supply_drop}"
    );
    let debt_after = debt_of();
    assert!(
        debt_after + repay_amount >= debt_before && debt_after + repay_amount <= debt_before + tol,
        "debt falls by the repaid amount (plus bounded interest): {debt_before} -> {debt_after}"
    );
    assert!(debt_after >= 3 * 100_000_000 && debt_after <= 3 * 100_000_000 + tol);
    assert_eq!(bal(outsider), outsider_bal_pre, "outsider balance still unchanged");
    assert_vault_identity();

    // (g) Test-only partial XLM withdrawal recorded as a durable claim.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    struct XlmClaimTestView {
        claim_id: u64,
        vault_id: u64,
        custody_owner: Principal,
        custody_nonce: u64,
        claimant: Principal,
        gross_stroops: u64,
        destination: String,
        network_id: Vec<u8>,
        reserved_fee_stroops: u64,
    }
    let dest = "GA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJVSGZ".to_string();
    let withdraw_call = |sender: Principal, amount: u64, d: &str| -> Result<u64, ()> {
        let r: Result<u64, candid::Reserved> = decode_one(&call(
            sender,
            "withdraw_xlm_collateral_to_claim",
            encode_args((env.vault_id, amount, d.to_string())).unwrap(),
        ))
        .unwrap();
        r.map_err(|_| ())
    };
    let claims_of = |who: Principal| -> Vec<XlmClaimTestView> {
        match env
            .pic
            .query_call(env.backend, who, "get_my_xlm_claims", encode_args(()).unwrap())
            .unwrap()
        {
            WasmResult::Reply(b) => decode_one(&b).unwrap(),
            WasmResult::Reject(m) => panic!("get_my_xlm_claims rejected: {m}"),
        }
    };
    let w_amount: u64 = 10 * 10_000_000; // 10 XLM
    let owner_bal_g = bal(developer());
    let supply_g = total_supply();
    // Debt may only grow by bounded interest accrual (accrual can run before a rejection).
    let assert_debt_bounded = |before: u64, what: &str| {
        let after = debt_of();
        assert!(after >= before && after <= before + tol, "{what}: debt {before} -> {after} outside [before, before+tol]");
    };
    let assert_no_icrc_movement = |what: &str| {
        assert_eq!(bal(developer()), owner_bal_g, "{what}: owner icUSD balance unchanged");
        assert_eq!(total_supply(), supply_g, "{what}: icUSD supply unchanged");
    };

    // (g1) Outsider withdraw rejected; no claim; vault unchanged.
    let debt_g1 = debt_of();
    assert!(withdraw_call(outsider, w_amount, &dest).is_err(), "outsider withdraw must be rejected");
    assert!(claims_of(outsider).is_empty() && claims_of(developer()).is_empty(), "no claim after outsider");
    assert_vault_identity();
    assert_eq!(vaults_of(&env, developer())[0].collateral_amount, 989_990_000, "g1 collateral unchanged");
    assert_debt_bounded(debt_g1, "g1");
    assert_no_icrc_movement("g1");

    // (g2) Withdrawing all collateral with debt outstanding breaks CR -> rejected.
    let debt_g2 = debt_of();
    assert!(withdraw_call(developer(), 989_990_000, &dest).is_err(), "CR-breaking withdraw must be rejected");
    assert!(claims_of(developer()).is_empty(), "no claim after CR-breaking withdraw");
    assert_vault_identity();
    assert_eq!(vaults_of(&env, developer())[0].collateral_amount, 989_990_000, "g2 collateral unchanged");
    assert_debt_bounded(debt_g2, "g2");
    assert_no_icrc_movement("g2");

    // (g3) Invalid (non-classic) destination rejected before any mutation.
    let debt_g3 = debt_of();
    assert!(withdraw_call(developer(), w_amount, "not-a-stellar-address").is_err(), "bad destination must be rejected");
    assert!(claims_of(developer()).is_empty(), "no claim after bad destination");
    assert_vault_identity();
    assert_eq!(vaults_of(&env, developer())[0].collateral_amount, 989_990_000, "g3 collateral unchanged");
    assert_debt_bounded(debt_g3, "g3");
    assert_no_icrc_movement("g3");

    // (g4) Owner partial withdraw of 10 XLM succeeds and records exactly one claim.
    let debt_g4 = debt_of();
    let claim_id = withdraw_call(developer(), w_amount, &dest).expect("owner partial XLM withdraw");
    assert_debt_bounded(debt_g4, "g4");
    let v = vaults_of(&env, developer());
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].collateral_amount, 989_990_000 - w_amount, "collateral decreases by exactly the withdrawal");
    assert_eq!(v[0].owner, developer(), "owner unchanged");
    assert_eq!(v[0].collateral_type, xlm, "collateral_type unchanged");
    let claims = claims_of(developer());
    assert_eq!(claims.len(), 1, "exactly one claim");
    assert_eq!(claims[0].claim_id, claim_id);
    assert_eq!(claims[0].vault_id, env.vault_id);
    assert_eq!(claims[0].custody_owner, developer(), "custody owner = vault owner");
    assert_eq!(claims[0].custody_nonce, env.vault_id, "custody nonce = source vault id");
    assert_eq!(claims[0].claimant, developer());
    assert_eq!(claims[0].gross_stroops, w_amount);
    assert_eq!(claims[0].destination, dest);
    assert_eq!(claims[0].reserved_fee_stroops, 10_000);
    // Stellar testnet network id = SHA-256("Test SDF Network ; September 2015");
    // the canister's default xlm_stellar_key_name (test_key_1) selects Network::Test.
    let expected_network_id: Vec<u8> =
        sha2::Sha256::digest(b"Test SDF Network ; September 2015").to_vec();
    assert_eq!(claims[0].network_id, expected_network_id, "claim network id = testnet network id");
    assert!(claims_of(outsider).is_empty(), "outsider has no claim");
    assert_no_icrc_movement("g4");
}
