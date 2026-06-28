//! Native-XRP collateral: end-to-end PocketIC integration test (P5).
//!
//! This is the capstone proof that a native-XRP CDP works end-to-end through the
//! ICP-native Vault model + the `chains::xrp` rail. It exercises the two pieces of
//! XRP-specific machinery that nothing else covers:
//!
//!   * deposit verification  — `confirm_xrp_deposit` makes DIRECT rippled HTTPS
//!     outcalls (`account_info` + `server_state`) which PocketIC parks until the
//!     test mocks them (native canister-http mocking — a first for this repo;
//!     every other chain test redirects to a mock *canister*).
//!   * claim settlement       — `settle_xrp_claim` threshold-Ed25519 signs an XRPL
//!     Payment (`key_1`, provisioned by PocketIC's II subnet) and makes
//!     `submit` + `tx` outcalls, again mocked here.
//!
//! Happy path: register XRP -> open -> confirm deposit -> borrow -> repay ->
//! withdraw&close -> settle the resulting XRP claim.
//!
//! Liquidation path: open -> confirm deposit -> borrow -> XRP price crashes ->
//! an external liquidator (claim-based) absorbs the vault, producing an XrpClaim
//! for the seized XRP (the automated SP/bot path is excluded for native-XRP by P5,
//! so liquidation is manual/claim-based only).
//!
//! ## tEd25519-in-PocketIC: full vs gated
//!
//! `open_xrp_vault` / `settle_xrp_claim` call the management-canister threshold
//! Schnorr (Ed25519) API with key `key_1`. We boot `.with_ii_subnet()
//! .with_application_subnet()`. If this build cannot provision `key_1`,
//! `open_xrp_vault` errors and the test degrades to a gated subset (the same
//! auto-degrade split the Solana/Monad happy-path tests use).

use candid::{
    encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal, Reserved,
};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::common::rest::{CanisterHttpReply, CanisterHttpResponse, MockCanisterHttpResponse};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use serde_json::{json, Value};
use std::collections::HashMap;

// ─── Locally-mirrored candid types (shapes mirror the backend exactly) ───────

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

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpVaultOpenInfo {
    vault_id: u64,
    custody_address: String,
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

// Minimal views for the frontend-contract test. Candid record subtyping lets us
// decode the full CollateralConfig into just the fields the UI reads.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum CustodyKindView {
    IcrcLedger,
    NativeXrp,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct CollateralConfigView {
    decimals: u8,
    custody_kind: Option<CustodyKindView>,
}

// ─── ICRC-1 ledger init types (mirror ic-icrc1-ledger candid) ────────────────

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
    nat: Option<candid::Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize)]
struct LedgerInitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: candid::Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, candid::Nat)>,
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

// ─── XRC mock init arg (mirror tests/pocket_ic_tests.rs MockXRC) ─────────────

#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    rates: HashMap<String, u64>,
}

// ─── Wasm loaders ────────────────────────────────────────────────────────────

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}
fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}
fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

// ─── Constants ───────────────────────────────────────────────────────────────

const E8: u64 = 100_000_000; // 1 icUSD (8 decimals)
const XRP: u64 = 1_000_000; // 1 XRP in drops (6 decimals)
const RESERVE_DROPS: u64 = 1_000_000; // XRPL base reserve (1 XRP)

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}
fn user() -> Principal {
    Principal::from_slice(&[9, 9, 9, 9, 9, 9, 9, 9, 9])
}
fn liquidator() -> Principal {
    Principal::from_slice(&[7, 7, 7, 7, 7, 7, 7, 7, 7])
}

// The synthetic native-XRP collateral key: Principal::from_slice(b"rumi-xrp-native").
fn xrp_collateral_principal() -> Principal {
    Principal::from_slice(b"rumi-xrp-native")
}

// ─── Call helpers ────────────────────────────────────────────────────────────

fn update_as(
    pic: &PocketIc,
    cid: Principal,
    sender: Principal,
    method: &str,
    args: Vec<u8>,
) -> WasmResult {
    pic.update_call(cid, sender, method, args)
        .unwrap_or_else(|e| panic!("update {method} failed: {e}"))
}

fn query_as<T>(pic: &PocketIc, cid: Principal, sender: Principal, method: &str, args: Vec<u8>) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    match pic
        .query_call(cid, sender, method, args)
        .expect("query call")
    {
        WasmResult::Reply(b) => Decode!(&b, T).expect("decode query reply"),
        WasmResult::Reject(m) => panic!("query {method} rejected: {m}"),
    }
}

/// Decode a `Result<T, ProtocolError>` reply, treating the error arm as `Reserved`
/// (the backend's rich ProtocolError does not subtype-decode into a minimal local
/// enum). Panics on a Reject. Returns the inner Result.
fn decode_result<T>(reply: WasmResult, method: &str) -> Result<T, Reserved>
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<T, Reserved>)
            .unwrap_or_else(|e| panic!("decode {method} Result: {e}")),
        WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
    }
}

// ─── Native rippled HTTPS-outcall pump ───────────────────────────────────────

/// The rippled JSON-RPC method in an outcall request body (`{"method":"...",...}`).
fn rippled_method_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default()
}

/// Drive an async update that makes DIRECT rippled HTTPS outcalls: submit the call,
/// then repeatedly tick and answer every parked canister-http request with a RAW
/// rippled JSON body chosen by `responder` (keyed on the JSON-RPC method). PocketIC
/// applies the canister's `xrp_transform_*` to the body before the parsers see it,
/// so we provide the raw shape. Returns the call's WasmResult.
fn call_with_rippled<F>(
    pic: &PocketIc,
    backend: Principal,
    sender: Principal,
    method: &str,
    args: Vec<u8>,
    responder: F,
) -> WasmResult
where
    F: Fn(&str) -> Value,
{
    let msg = pic
        .submit_call(backend, sender, method, args)
        .unwrap_or_else(|e| panic!("submit_call {method} failed: {e}"));

    // Each sequential outcall parks until mocked; answer one per tick as it appears.
    // Signing (sign_with_schnorr) resolves over a few ticks with nothing to mock.
    // 80 iterations comfortably covers confirm (2 reads) and settle (read+submit+tx
    // + signing); extra ticks after completion are harmless (no pending requests).
    for _ in 0..80 {
        pic.tick();
        for req in pic.get_canister_http() {
            let m = rippled_method_of(&req.body);
            let body = responder(&m).to_string().into_bytes();
            pic.mock_canister_http_response(MockCanisterHttpResponse {
                subnet_id: req.subnet_id,
                request_id: req.request_id,
                response: CanisterHttpResponse::CanisterHttpReply(CanisterHttpReply {
                    status: 200,
                    headers: vec![],
                    body,
                }),
                additional_responses: vec![],
            });
        }
    }
    pic.await_call(msg)
        .unwrap_or_else(|e| panic!("await_call {method} failed: {e}"))
}

// ─── Raw rippled response builders (pre-transform shapes) ────────────────────

/// `account_info` for a FUNDED account holding `balance_drops`.
fn rippled_account_info(balance_drops: u64, sequence: u32, ledger_index: u32) -> Value {
    json!({
        "result": {
            "account_data": {
                "Sequence": sequence,
                "Balance": balance_drops.to_string()
            },
            "ledger_index": ledger_index,
            "ledger_current_index": ledger_index,
            "validated": true
        }
    })
}

/// `server_state` reporting the base reserve (drops).
fn rippled_server_state(reserve_base_drops: u64) -> Value {
    json!({ "result": { "state": { "validated_ledger": { "reserve_base": reserve_base_drops } } } })
}

/// `submit` accepting a Payment (`tesSUCCESS`).
fn rippled_submit_ok() -> Value {
    json!({
        "result": {
            "engine_result": "tesSUCCESS",
            "tx_json": { "hash": "E2E0000000000000000000000000000000000000000000000000000000000001" }
        }
    })
}

/// `tx` reporting a validated Payment delivering `delivered_drops`.
fn rippled_tx_validated(tx_hash: &str, delivered_drops: u64, ledger_index: u32) -> Value {
    json!({
        "result": {
            "validated": true,
            "hash": tx_hash,
            "ledger_index": ledger_index,
            "meta": {
                "TransactionResult": "tesSUCCESS",
                "delivered_amount": delivered_drops.to_string()
            }
        }
    })
}

// ─── Boot: II subnet (tEd25519) + application subnet (backend, ledgers, XRC) ──

struct Env {
    pic: PocketIc,
    backend: Principal,
    icusd: Principal,
    xrc: Principal,
}

fn boot() -> Env {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

    let backend = pic.create_canister();
    pic.add_cycles(backend, 100_000_000_000_000);
    let icusd = pic.create_canister();
    pic.add_cycles(icusd, 100_000_000_000_000);
    let icp = pic.create_canister();
    pic.add_cycles(icp, 100_000_000_000_000);
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 100_000_000_000_000);

    // icUSD ledger: backend is the minter (borrow mints, repay burns). icrc2 on.
    install_ledger(&pic, icusd, backend, "icUSD", "icUSD");
    // ICP ledger: required by init; not exercised by XRP-only flows.
    install_ledger(&pic, icp, backend, "Internet Computer", "ICP");

    // XRC mock: configurable rates. Borrow auto-pulls the XRP price on demand.
    let mut rates = HashMap::new();
    rates.insert("ICP/USD".to_string(), 10 * E8); // $10
    rates.insert("XRP/USD".to_string(), E8 / 2); // $0.50
    pic.install_canister(
        xrc,
        xrc_wasm(),
        encode_one(MockXRC { rates }).expect("encode MockXRC"),
        None,
    );

    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd,
        icp_ledger_principal: icp,
        fee_e8s: 10_000,
        developer_principal: dev(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );

    for _ in 0..5 {
        pic.tick();
    }

    Env {
        pic,
        backend,
        icusd,
        xrc,
    }
}

fn install_ledger(pic: &PocketIc, ledger: Principal, minter: Principal, name: &str, symbol: &str) {
    let args = LedgerArg::Init(LedgerInitArgs {
        minting_account: Account {
            owner: minter,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: name.to_string(),
        token_symbol: symbol.to_string(),
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
    pic.install_canister(
        ledger,
        ledger_wasm(),
        encode_one(args).expect("encode ledger init"),
        None,
    );
}

// ─── Reads ───────────────────────────────────────────────────────────────────

fn get_vault(pic: &PocketIc, backend: Principal, vault_id: u64) -> Option<CandidVault> {
    let vaults: Vec<CandidVault> = query_as(
        pic,
        backend,
        Principal::anonymous(),
        "get_vaults",
        Encode!(&Some(user())).unwrap(),
    );
    vaults.into_iter().find(|v| v.vault_id == vault_id)
}

fn icusd_balance(pic: &PocketIc, icusd: Principal, who: Principal) -> u64 {
    let bal: candid::Nat = query_as(
        pic,
        icusd,
        Principal::anonymous(),
        "icrc1_balance_of",
        Encode!(&Account {
            owner: who,
            subaccount: None
        })
        .unwrap(),
    );
    bal.0.try_into().unwrap_or(u64::MAX)
}

/// Register XRP collateral and set the XRP price (via XRC). Returns nothing; panics
/// on failure. Probes tEd25519 availability and skips the test body if absent.
fn register_xrp(pic: &PocketIc, backend: Principal) {
    decode_result::<()>(
        update_as(
            pic,
            backend,
            dev(),
            "set_xrp_schnorr_key_name",
            Encode!(&"key_1".to_string()).unwrap(),
        ),
        "set_xrp_schnorr_key_name",
    )
    .expect("set_xrp_schnorr_key_name");
    decode_result::<()>(
        update_as(
            pic,
            backend,
            dev(),
            "register_xrp_collateral",
            Encode!().unwrap(),
        ),
        "register_xrp_collateral",
    )
    .expect("register_xrp_collateral");
}

/// icrc2-approve the backend to pull `amount` icUSD from `owner` (for repay/liquidate).
fn approve_icusd(
    pic: &PocketIc,
    icusd: Principal,
    owner: Principal,
    spender: Principal,
    amount: u64,
) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: candid::Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let reply = update_as(pic, icusd, owner, "icrc2_approve", Encode!(&args).unwrap());
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<candid::Nat, Reserved>)
                .expect("decode approve")
                .expect("approve ok");
        }
        WasmResult::Reject(m) => panic!("icrc2_approve rejected: {m}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Happy path: open -> confirm deposit -> borrow -> repay -> withdraw&close -> settle
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn xrp_native_happy_path_open_deposit_borrow_repay_close_settle() {
    let Env {
        pic,
        backend,
        icusd,
        xrc: _,
    } = boot();
    register_xrp(&pic, backend);

    // ── open_xrp_vault (derives custody address via tEd25519) ────────────────
    // If PocketIC cannot provision key_1, this errors -> gated skip.
    let open_reply = update_as(&pic, backend, user(), "open_xrp_vault", Encode!().unwrap());
    let open: XrpVaultOpenInfo =
        match decode_result::<XrpVaultOpenInfo>(open_reply, "open_xrp_vault") {
            Ok(info) => info,
            Err(_) => {
                eprintln!("[gated] tEd25519 unavailable in this PocketIC build; skipping XRP e2e");
                return;
            }
        };
    let vault_id = open.vault_id;
    assert!(!open.custody_address.is_empty(), "custody address derived");
    assert!(
        open.custody_address.starts_with('r'),
        "XRPL classic address starts with r"
    );
    // No vault yet (deposit unconfirmed).
    assert!(
        get_vault(&pic, backend, vault_id).is_none(),
        "no Vault before deposit confirm"
    );

    // ── confirm_xrp_deposit: 500 XRP funded on custody (native http mock) ─────
    let deposit_drops = 500 * XRP;
    let credited = decode_result::<u64>(
        call_with_rippled(
            &pic,
            backend,
            user(),
            "confirm_xrp_deposit",
            Encode!(&vault_id).unwrap(),
            |m| match m {
                "account_info" => rippled_account_info(deposit_drops, 1, 100),
                "server_state" => rippled_server_state(RESERVE_DROPS),
                other => panic!("unexpected rippled method in confirm: {other}"),
            },
        ),
        "confirm_xrp_deposit",
    )
    .expect("confirm_xrp_deposit ok");
    // Credited = balance - reserve = 500 XRP - 1 XRP.
    assert_eq!(
        credited,
        deposit_drops - RESERVE_DROPS,
        "credited = balance - base reserve"
    );

    let v = get_vault(&pic, backend, vault_id).expect("Vault exists after confirm");
    assert_eq!(
        v.collateral_type,
        xrp_collateral_principal(),
        "collateral is native-XRP"
    );
    assert_eq!(
        v.collateral_amount,
        deposit_drops - RESERVE_DROPS,
        "collateral credited in drops"
    );
    assert_eq!(v.borrowed_icusd_amount, 0, "no debt yet");

    // ── borrow 50 icUSD (XRP price auto-pulled from XRC: $0.50) ──────────────
    // 499 XRP * $0.50 = ~$249 collateral; $50 debt -> CR ~498% (> 150%).
    let borrow_amount = 50 * E8;
    let borrow_result = decode_result::<rumi_protocol_backend::SuccessWithFee>(
        update_as(
            &pic,
            backend,
            user(),
            "borrow_from_vault",
            Encode!(&VaultArg {
                vault_id,
                amount: borrow_amount
            })
            .unwrap(),
        ),
        "borrow_from_vault",
    )
    .expect("borrow ok");
    assert_eq!(
        borrow_result.xrp_claim_id, None,
        "non-liquidation SuccessWithFee results must not expose an XRP claim id"
    );
    let v = get_vault(&pic, backend, vault_id).expect("vault");
    assert_eq!(v.borrowed_icusd_amount, borrow_amount, "debt = borrowed");
    let bal = icusd_balance(&pic, icusd, user());
    assert!(
        bal >= borrow_amount - E8,
        "user received ~50 icUSD (net of borrow fee): {bal}"
    );

    // ── repay the full debt (approve backend to pull icUSD, then repay) ───────
    // Top up the user so they can cover the debt + the one-time borrow fee + ledger
    // fees (the borrow netted its fee out of the minted icUSD). Minted from the
    // backend (the icUSD minting account).
    mint_icusd(&pic, icusd, backend, user(), 5 * E8);
    approve_icusd(&pic, icusd, user(), backend, borrow_amount + 5 * E8);
    decode_result::<u64>(
        update_as(
            &pic,
            backend,
            user(),
            "repay_to_vault",
            Encode!(&VaultArg {
                vault_id,
                amount: borrow_amount
            })
            .unwrap(),
        ),
        "repay_to_vault",
    )
    .expect("repay ok");
    let v = get_vault(&pic, backend, vault_id).expect("vault");
    assert_eq!(v.borrowed_icusd_amount, 0, "debt cleared after repay");

    // ── withdraw & close: creates an XrpClaim for the full collateral ─────────
    decode_result::<Option<u64>>(
        update_as(
            &pic,
            backend,
            user(),
            "withdraw_and_close_vault",
            Encode!(&vault_id).unwrap(),
        ),
        "withdraw_and_close_vault",
    )
    .expect("withdraw_and_close ok");
    assert!(get_vault(&pic, backend, vault_id).is_none(), "vault closed");

    // A claim must now exist for the withdrawn collateral.
    let claims = xrp_claims(&pic, backend);
    assert_eq!(claims.len(), 1, "one XRP claim after close: {claims:?}");
    let claim_id = claims[0];

    // ── settle_xrp_claim is two-phase (anti double-pay) ───────────────────────
    // Call 1 derives the custody Sequence (account_info), signs the Payment, records
    // the in-flight settlement, and submits. The claim is RETAINED (settlement set)
    // so a retry confirms instead of re-paying.
    let dest = "rPdvC6ccq8hCdPKSPJkPmyZ4Mi1oG2FFkT".to_string(); // a valid XRPL classic addr
    let tx_hash = decode_result::<String>(
        call_with_rippled(
            &pic,
            backend,
            user(),
            "settle_xrp_claim",
            Encode!(&claim_id, &dest).unwrap(),
            |m| match m {
                "account_info" => rippled_account_info(deposit_drops, 5, 200),
                "server_state" => rippled_server_state(RESERVE_DROPS),
                "submit" => rippled_submit_ok(),
                "tx" => rippled_tx_validated(
                    "E2E0000000000000000000000000000000000000000000000000000000000001",
                    deposit_drops - RESERVE_DROPS,
                    201,
                ),
                other => panic!("unexpected rippled method in settle (submit phase): {other}"),
            },
        ),
        "settle_xrp_claim (submit)",
    )
    .expect("settle submit ok");
    assert!(!tx_hash.is_empty(), "settle returns the local tx hash");
    assert_eq!(
        xrp_claims(&pic, backend).len(),
        1,
        "claim retained until the Payment validates"
    );

    // Call 2 confirms the recorded settlement: fetch_tx_status -> Validated -> remove.
    let confirm_hash = decode_result::<String>(
        call_with_rippled(
            &pic,
            backend,
            user(),
            "settle_xrp_claim",
            Encode!(&claim_id, &dest).unwrap(),
            |m| match m {
                "tx" => rippled_tx_validated(&tx_hash, deposit_drops - RESERVE_DROPS, 201),
                "account_info" => rippled_account_info(deposit_drops, 6, 250),
                "server_state" => rippled_server_state(RESERVE_DROPS),
                other => panic!("unexpected rippled method in settle (confirm phase): {other}"),
            },
        ),
        "settle_xrp_claim (confirm)",
    )
    .expect("settle confirm ok");
    assert_eq!(
        confirm_hash, tx_hash,
        "confirm returns the same (already-broadcast) hash"
    );
    assert!(
        xrp_claims(&pic, backend).is_empty(),
        "claim removed after validated settlement"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Liquidation path: XRP vault goes underwater -> claim-based external liquidation
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn xrp_native_liquidation_is_claim_based() {
    let Env {
        pic,
        backend,
        icusd,
        xrc,
    } = boot();
    register_xrp(&pic, backend);

    let open_reply = update_as(&pic, backend, user(), "open_xrp_vault", Encode!().unwrap());
    let open: XrpVaultOpenInfo =
        match decode_result::<XrpVaultOpenInfo>(open_reply, "open_xrp_vault") {
            Ok(info) => info,
            Err(_) => {
                eprintln!("[gated] tEd25519 unavailable; skipping XRP liquidation e2e");
                return;
            }
        };
    let vault_id = open.vault_id;

    let deposit_drops = 200 * XRP;
    decode_result::<u64>(
        call_with_rippled(
            &pic,
            backend,
            user(),
            "confirm_xrp_deposit",
            Encode!(&vault_id).unwrap(),
            |m| match m {
                "account_info" => rippled_account_info(deposit_drops, 1, 100),
                "server_state" => rippled_server_state(RESERVE_DROPS),
                other => panic!("unexpected rippled method: {other}"),
            },
        ),
        "confirm_xrp_deposit",
    )
    .expect("confirm ok");

    // Borrow near the limit at $0.50: ~199 XRP * $0.50 = ~$99.5; borrow $60 -> CR ~165%.
    let borrow_amount = 60 * E8;
    decode_result::<rumi_protocol_backend::SuccessWithFee>(
        update_as(
            &pic,
            backend,
            user(),
            "borrow_from_vault",
            Encode!(&VaultArg {
                vault_id,
                amount: borrow_amount
            })
            .unwrap(),
        ),
        "borrow_from_vault",
    )
    .expect("borrow ok");

    // Crash the XRP price to $0.36 (199 XRP -> ~$71.6 vs ~$60 debt -> CR ~119%, below
    // the 133% liquidation threshold). 0.36/0.50 = 0.72 stays inside the 0.70 price-
    // sanity band, so the single on-demand re-fetch during liquidation accepts it
    // immediately (a deeper one-shot crash would be queued as an outlier needing N
    // confirmations). The re-fetch fires because the cached price is now > 60s old.
    crash_xrp_price(&pic, xrc, 36 * E8 / 100); // $0.36

    // Fund the external liquidator with icUSD (minted from the backend minting
    // account) and approve the backend to pull it for the partial repay.
    mint_icusd(&pic, icusd, backend, liquidator(), 40 * E8);
    approve_icusd(&pic, icusd, liquidator(), backend, 40 * E8);

    // Native-XRP is excluded from automated SP/bot liquidation (P5), so liquidation
    // is the external, claim-based path: the liquidator repays part of the debt and
    // the seized XRP becomes an XrpClaim they later settle to an XRPL address.
    let before_claims = xrp_claims_full(&pic, backend);
    let liquidation_result = decode_result::<rumi_protocol_backend::SuccessWithFee>(
        update_as(
            &pic,
            backend,
            liquidator(),
            "liquidate_vault_partial",
            Encode!(&VaultArg {
                vault_id,
                amount: 30 * E8
            })
            .unwrap(),
        ),
        "liquidate_vault_partial",
    )
    .expect("claim-based partial liquidation ok");
    let after = xrp_claims_full(&pic, backend);
    assert!(
        after.len() > before_claims.len(),
        "claim-based liquidation produced an XrpClaim (before={before_claims:?}, after={after:?})"
    );

    let claim_id = liquidation_result
        .xrp_claim_id
        .expect("native-XRP manual liquidation returns the liquidator reward claim id");
    assert!(
        before_claims.iter().all(|(id, _)| *id != claim_id),
        "returned XRP claim id must be newly created during liquidation"
    );
    let (_, returned_claim) = after
        .iter()
        .find(|(id, _)| *id == claim_id)
        .unwrap_or_else(|| panic!("returned XRP claim id {claim_id} absent from claims {after:?}"));
    assert_eq!(
        returned_claim.claimant,
        liquidator(),
        "returned claim id belongs to the liquidator reward claim"
    );
    assert_ne!(
        returned_claim.claimant,
        dev(),
        "returned claim id must not be the protocol-fee claim"
    );
    assert_ne!(
        returned_claim.claimant,
        user(),
        "returned claim id must not be owner-excess collateral"
    );
    assert_eq!(
        returned_claim.custody_owner,
        user(),
        "liquidator reward claim pays from the liquidated vault custody owner"
    );
    assert_eq!(
        returned_claim.custody_nonce, vault_id,
        "liquidator reward claim pays from the liquidated vault custody nonce"
    );
    assert_eq!(
        Some(returned_claim.drops),
        liquidation_result.collateral_amount_received,
        "returned claim id must carry the exact liquidator reward amount"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Frontend↔backend contract: what the vault_frontend's XRP panel gate +
// collateralStore read off a real registered XRP collateral. (No tEd25519 / XRPL
// funds needed — this is the deterministic stand-in for the "does the UI light up"
// browser check.)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn xrp_collateral_contract_matches_frontend_expectations() {
    let Env { pic, backend, .. } = boot();
    register_xrp(&pic, backend);
    let xrp = xrp_collateral_principal();

    // collateralStore.fetchSupportedCollateral iterates get_supported_collateral_types;
    // the panel only renders if an entry with custody NativeXrp is present here.
    let supported: Vec<(Principal, candid::Reserved)> = query_as(
        &pic,
        backend,
        Principal::anonymous(),
        "get_supported_collateral_types",
        Encode!().unwrap(),
    );
    assert!(
        supported.iter().any(|(p, _)| *p == xrp),
        "native-XRP must appear in get_supported_collateral_types: {:?}",
        supported
            .iter()
            .map(|(p, _)| p.to_text())
            .collect::<Vec<_>>()
    );

    // The UI reads get_collateral_config(xrp): custody_kind drives `hasXrpCollateral`
    // and the "XRP"/skip-icrc1_symbol branch; decimals (6) drives the collateral math.
    let cfg: Option<CollateralConfigView> = query_as(
        &pic,
        backend,
        Principal::anonymous(),
        "get_collateral_config",
        Encode!(&xrp).unwrap(),
    );
    let cfg = cfg.expect("XRP collateral config exists after register_xrp_collateral");
    assert_eq!(cfg.decimals, 6, "native-XRP is 6 decimals (drops)");
    assert_eq!(
        cfg.custody_kind,
        Some(CustodyKindView::NativeXrp),
        "native-XRP custody_kind must be NativeXrp so the frontend detects it"
    );
}

// ─── XRP claim + price helpers ───────────────────────────────────────────────

/// IDs of all outstanding XRP claims (via the dev query). Returns [] if the query
/// is absent (older builds) — the asserting tests treat that as a hard failure.
fn xrp_claims(pic: &PocketIc, backend: Principal) -> Vec<u64> {
    xrp_claims_full(pic, backend)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

fn xrp_claims_full(pic: &PocketIc, backend: Principal) -> Vec<(u64, XrpClaimView)> {
    match pic.query_call(backend, dev(), "get_xrp_claims", Encode!().unwrap()) {
        Ok(WasmResult::Reply(b)) => Decode!(&b, Vec<(u64, XrpClaimView)>).unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpClaimView {
    claimant: Principal,
    drops: u64,
    custody_owner: Principal,
    custody_nonce: u64,
    created_at_ns: u64,
}

/// Crash the XRP/USD price: set the new rate on the XRC mock and advance time past
/// the 60s soft freshness threshold so the next price-sensitive op (here the
/// liquidation's `validate_freshness_for_vault`) re-fetches the crashed value. The
/// re-fetch resets the timestamp, so the price is FRESH (within the 10-min hard
/// ceiling) when liquidation reads it.
fn crash_xrp_price(pic: &PocketIc, xrc: Principal, price_e8: u64) {
    let reply = update_as(
        pic,
        xrc,
        Principal::anonymous(),
        "set_exchange_rate",
        Encode!(&"XRP".to_string(), &"USD".to_string(), &price_e8).unwrap(),
    );
    if let WasmResult::Reject(m) = reply {
        panic!("set_exchange_rate rejected: {m}");
    }
    pic.advance_time(std::time::Duration::from_secs(120));
    for _ in 0..3 {
        pic.tick();
    }
}

/// Mint icUSD to `to` by transferring FROM the minting account (`minter`). An
/// icrc1_transfer whose `from` is the minting account is a mint, so no fee applies.
fn mint_icusd(pic: &PocketIc, icusd: Principal, minter: Principal, to: Principal, amount: u64) {
    transfer_icusd(pic, icusd, minter, to, amount);
}

fn transfer_icusd(pic: &PocketIc, icusd: Principal, from: Principal, to: Principal, amount: u64) {
    let args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        fee: None,
        created_at_time: None,
        memo: None,
        amount: candid::Nat::from(amount),
    };
    let reply = update_as(pic, icusd, from, "icrc1_transfer", Encode!(&args).unwrap());
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<candid::Nat, Reserved>)
                .expect("decode transfer")
                .expect("transfer ok");
        }
        WasmResult::Reject(m) => panic!("icrc1_transfer rejected: {m}"),
    }
}
