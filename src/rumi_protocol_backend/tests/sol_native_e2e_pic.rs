//! Native-SOL collateral: end-to-end PocketIC integration test (Phase 6).
//!
//! This is the capstone proof that a native-SOL CDP works end-to-end through
//! the ICP-native Vault model + the `chains::sol` rail. It exercises the two
//! pieces of SOL-specific machinery nothing else covers:
//!
//!   * deposit verification — `confirm_sol_deposit` calls the SOL RPC canister
//!     (an ordinary inter-canister call, unlike XRP's direct HTTPS outcalls —
//!     design doc §9: "No transform queries are needed: SOL RPC goes through
//!     the SOL RPC canister as inter-canister calls"). We install a mock
//!     canister (`src/sol_rpc_mock`) at the EXACT hardcoded production SOL RPC
//!     principal (`chains::sol::rpc::SOL_RPC_PRINCIPAL`,
//!     `tghme-zyaaa-aaaar-qarca-cai`) via PocketIC's `create_canister_with_id`,
//!     so the backend's calls reach it unmodified. See this file's closing
//!     doc comment block for why that trick is necessary (chains::sol::rpc has
//!     no operator override, unlike the dormant chains::solana::sol_rpc).
//!   * claim settlement — `settle_sol_claim` threshold-Ed25519 signs a
//!     durable-nonce SOL transfer (`key_1`, provisioned by PocketIC's II
//!     subnet) and calls `sendTransaction` / `getTransaction` via the mock.
//!
//! Happy path: bootstrap nonce -> register SOL -> open -> confirm deposit ->
//! borrow -> repay -> withdraw&close -> settle the resulting SOL claim
//! (two-phase: submit, then confirm — itself the `AlreadyPaid` idempotency
//! row).
//!
//! Liquidation path: open -> confirm deposit -> borrow -> SOL price crashes ->
//! an external liquidator (claim-based) absorbs the vault, producing a
//! `SolClaim` for the seized SOL (native-SOL is excluded from automated
//! SP/bot liquidation, same as XRP, so liquidation is manual/claim-based only).
//!
//! Idempotency: the two highest-value tests in this phase, exercising the
//! §5.3 decision table through the REAL canister (not the pure
//! `sol_settlement_decision` helper):
//!   - `Confirmed` -> `AlreadyPaid`: a second `settle_sol_claim` call submits
//!     NO second transfer (`sendTransaction` call count stays at 1) and
//!     removes the claim.
//!   - `NotFound` + nonce UNCHANGED -> `SafeToResign`: a second
//!     `settle_sol_claim` call on a claim whose prior transaction is reported
//!     `NotFound` (while the shared durable nonce is unchanged) is PERMITTED
//!     to re-sign and re-submit (`sendTransaction` call count advances to 2).
//!
//! ## tEd25519-in-PocketIC: full vs gated
//!
//! `open_sol_vault` / `sol_bootstrap_nonce_account` / `settle_sol_claim` call
//! the management-canister threshold Schnorr (Ed25519) API with key `key_1`.
//! We boot `.with_ii_subnet().with_application_subnet()`. If this build cannot
//! provision `key_1`, the bootstrap probe errors and the FULL block is skipped
//! (the same auto-degrade split `xrp_native_e2e_pic.rs` / the Solana M2 tests
//! use). The dev-gate / precondition tests do not need signing and always run.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal, Reserved};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
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
struct SolVaultOpenInfo {
    vault_id: u64,
    custody_address: String,
    rent_exempt_lamports: u64,
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
    NativeSol,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct CollateralConfigView {
    decimals: u8,
    custody_kind: Option<CustodyKindView>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SolClaimView {
    claimant: Principal,
    lamports: u64,
    custody_owner: Principal,
    custody_nonce: u64,
    created_at_ns: u64,
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
fn sol_rpc_mock_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/sol_rpc_mock.wasm").to_vec()
}

// ─── Constants ───────────────────────────────────────────────────────────────

const E8: u64 = 100_000_000; // 1 icUSD (8 decimals)
const SOL: u64 = 1_000_000_000; // 1 SOL in lamports (9 decimals)
const RENT_EXEMPT_LAMPORTS: u64 = 890_880; // real mainnet 0-byte-account rent-exempt minimum

/// The EXACT principal `chains::sol::rpc::SOL_RPC_PRINCIPAL` hardcodes. Unlike
/// the dormant `chains::solana::sol_rpc` (which has a developer-settable
/// `sol_rpc_principal_override` / `set_sol_rpc_principal`), the native-SOL-
/// collateral rail's RPC wrapper has NO override — see this file's closing doc
/// comment. `create_canister_with_id` lets PocketIC mint a canister at this
/// exact mainnet id inside the test instance, so the mock installed there is
/// what the backend's hardcoded `sol_rpc_principal()` actually reaches.
const SOL_RPC_PRINCIPAL: &str = "tghme-zyaaa-aaaar-qarca-cai";

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}
fn user() -> Principal {
    Principal::from_slice(&[9, 9, 9, 9, 9, 9, 9, 9, 9])
}
fn liquidator() -> Principal {
    Principal::from_slice(&[7, 7, 7, 7, 7, 7, 7, 7, 7])
}
/// Non-anonymous, non-developer principal for dev-gate rejection tests.
fn non_dev() -> Principal {
    Principal::from_slice(&[3, 3, 3, 3, 3])
}

/// The synthetic native-SOL collateral key: `Principal::from_slice(b"rumi-sol-native")`.
fn sol_collateral_principal() -> Principal {
    Principal::from_slice(b"rumi-sol-native")
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

/// Decode a `Result<T, ProtocolError>` reply, treating the error arm as
/// `Reserved` (the backend's rich ProtocolError does not subtype-decode into a
/// minimal local enum). Panics on a Reject. Returns the inner Result.
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

// ─── Boot: II subnet (tEd25519) + application subnet (backend, ledgers, XRC, mock) ──

struct Env {
    pic: PocketIc,
    backend: Principal,
    icusd: Principal,
    xrc: Principal,
    sol_rpc_mock: Principal,
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

    // The mock MUST live at the exact hardcoded SOL RPC principal (see module
    // doc comment): the backend's chains::sol::rpc wrapper has no override.
    let sol_rpc_id = Principal::from_text(SOL_RPC_PRINCIPAL).expect("valid SOL RPC principal");
    let sol_rpc_mock = pic
        .create_canister_with_id(None, None, sol_rpc_id)
        .unwrap_or_else(|e| panic!("create_canister_with_id({SOL_RPC_PRINCIPAL}) failed: {e}"));
    pic.add_cycles(sol_rpc_mock, 100_000_000_000_000);
    pic.install_canister(sol_rpc_mock, sol_rpc_mock_wasm(), Encode!().unwrap(), None);

    // icUSD ledger: backend is the minter (borrow mints, repay burns). icrc2 on.
    install_ledger(&pic, icusd, backend, "icUSD", "icUSD");
    // ICP ledger: required by init; not exercised by SOL-only flows.
    install_ledger(&pic, icp, backend, "Internet Computer", "ICP");

    // XRC mock: configurable rates. Borrow auto-pulls the SOL price on demand.
    let mut rates = HashMap::new();
    rates.insert("ICP/USD".to_string(), 10 * E8); // $10
    rates.insert("SOL/USD".to_string(), 150 * E8); // $150
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
        sol_rpc_mock,
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

fn sol_claims(pic: &PocketIc, backend: Principal) -> Vec<(u64, SolClaimView)> {
    match pic.query_call(backend, dev(), "get_sol_claims", Encode!().unwrap()) {
        Ok(WasmResult::Reply(b)) => Decode!(&b, Vec<(u64, SolClaimView)>).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn send_transaction_count(pic: &PocketIc, mock: Principal) -> u64 {
    query_as(
        pic,
        mock,
        Principal::anonymous(),
        "get_send_transaction_count",
        Encode!().unwrap(),
    )
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

/// Mint icUSD to `to` by transferring FROM the minting account (`minter`).
fn mint_icusd(pic: &PocketIc, icusd: Principal, minter: Principal, to: Principal, amount: u64) {
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
    let reply = update_as(pic, icusd, minter, "icrc1_transfer", Encode!(&args).unwrap());
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<candid::Nat, Reserved>)
                .expect("decode transfer")
                .expect("transfer ok");
        }
        WasmResult::Reject(m) => panic!("icrc1_transfer rejected: {m}"),
    }
}

/// Crash the SOL/USD price and advance time past the 60s soft freshness
/// threshold so the next price-sensitive op re-fetches the crashed value.
fn crash_sol_price(pic: &PocketIc, xrc: Principal, price_e8: u64) {
    // xrc mock lives on the same canister as XRC in this harness's `boot()`
    // call site (passed explicitly by the caller, mirroring xrp_native_e2e_pic).
    let reply = update_as(
        pic,
        xrc,
        Principal::anonymous(),
        "set_exchange_rate",
        Encode!(&"SOL".to_string(), &"USD".to_string(), &price_e8).unwrap(),
    );
    if let WasmResult::Reject(m) = reply {
        panic!("set_exchange_rate rejected: {m}");
    }
    pic.advance_time(std::time::Duration::from_secs(120));
    for _ in 0..3 {
        pic.tick();
    }
}

// ─── Setup helpers ────────────────────────────────────────────────────────────

/// Set the production Schnorr key (flips RPC to "mainnet" per `sol_cluster()`,
/// which the mock does not distinguish) and bootstrap the durable-nonce
/// account. Returns `true` if tEd25519 is available (bootstrap succeeded),
/// `false` if this PocketIC build could not provision `key_1` (caller should
/// then skip the signing-dependent portion of its test).
fn set_prod_key_and_bootstrap_nonce(pic: &PocketIc, backend: Principal) -> bool {
    decode_result::<()>(
        update_as(
            pic,
            backend,
            dev(),
            "set_sol_schnorr_key_name",
            Encode!(&"key_1".to_string()).unwrap(),
        ),
        "set_sol_schnorr_key_name",
    )
    .expect("set_sol_schnorr_key_name");

    match update_as(
        pic,
        backend,
        dev(),
        "sol_bootstrap_nonce_account",
        Encode!(&Option::<String>::None).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, Reserved>) {
            Ok(Ok(addr)) => {
                eprintln!("[sol e2e] tEd25519 AVAILABLE; nonce account = {addr}; running FULL block");
                true
            }
            Ok(Err(_)) => {
                eprintln!("[sol e2e] tEd25519 UNAVAILABLE (bootstrap returned Err); running GATED subset");
                false
            }
            Err(e) => {
                eprintln!("[sol e2e] bootstrap decode error ({e}); running GATED subset");
                false
            }
        },
        WasmResult::Reject(m) => {
            eprintln!("[sol e2e] tEd25519 UNAVAILABLE (bootstrap rejected: {m}); running GATED subset");
            false
        }
    }
}

/// Register SOL collateral (developer-gated); panics on failure. Assumes the
/// production key + nonce bootstrap already succeeded.
fn register_sol(pic: &PocketIc, backend: Principal) {
    decode_result::<()>(
        update_as(
            pic,
            backend,
            dev(),
            "register_sol_collateral",
            Encode!().unwrap(),
        ),
        "register_sol_collateral",
    )
    .expect("register_sol_collateral");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Happy path: bootstrap -> register -> open -> confirm deposit -> borrow ->
// repay -> withdraw&close -> settle (two-phase: submit, then AlreadyPaid confirm)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn sol_native_happy_path_open_deposit_borrow_repay_close_settle() {
    let Env {
        pic,
        backend,
        icusd,
        xrc: _,
        sol_rpc_mock,
    } = boot();

    if !set_prod_key_and_bootstrap_nonce(&pic, backend) {
        return; // gated: tEd25519 unavailable in this PocketIC build
    }
    register_sol(&pic, backend);

    // ── open_sol_vault (derives custody address via tEd25519) ────────────────
    let open: SolVaultOpenInfo = decode_result::<SolVaultOpenInfo>(
        update_as(&pic, backend, user(), "open_sol_vault", Encode!().unwrap()),
        "open_sol_vault",
    )
    .expect("open_sol_vault ok (tEd25519 already proven available)");
    let vault_id = open.vault_id;
    assert!(!open.custody_address.is_empty(), "custody address derived");
    assert_eq!(
        open.rent_exempt_lamports, RENT_EXEMPT_LAMPORTS,
        "rent-exempt minimum comes from the mock's getMinimumBalanceForRentExemption"
    );
    assert!(
        get_vault(&pic, backend, vault_id).is_none(),
        "no Vault before deposit confirm"
    );

    // ── mock-fund the custody address, then confirm_sol_deposit ──────────────
    let deposit_lamports = 5 * SOL;
    update_as(
        &pic,
        sol_rpc_mock,
        Principal::anonymous(),
        "set_balance",
        Encode!(&open.custody_address, &deposit_lamports).unwrap(),
    );

    let credited = decode_result::<u64>(
        update_as(
            &pic,
            backend,
            user(),
            "confirm_sol_deposit",
            Encode!(&vault_id).unwrap(),
        ),
        "confirm_sol_deposit",
    )
    .expect("confirm_sol_deposit ok");
    assert_eq!(
        credited,
        deposit_lamports - RENT_EXEMPT_LAMPORTS,
        "credited = balance - rent-exempt minimum"
    );

    let v = get_vault(&pic, backend, vault_id).expect("Vault exists after confirm");
    assert_eq!(
        v.collateral_type,
        sol_collateral_principal(),
        "collateral is native-SOL"
    );
    assert_eq!(
        v.collateral_amount,
        deposit_lamports - RENT_EXEMPT_LAMPORTS,
        "collateral credited in lamports"
    );
    assert_eq!(v.borrowed_icusd_amount, 0, "no debt yet");

    // ── borrow 100 icUSD (SOL price auto-pulled from XRC: $150) ──────────────
    // (5 SOL - rent) * $150 ≈ $613.6 collateral; $100 debt -> CR ~613% (> 135%).
    let borrow_amount = 100 * E8;
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
        "non-liquidation SuccessWithFee results must not expose a native claim id"
    );
    let v = get_vault(&pic, backend, vault_id).expect("vault");
    assert_eq!(v.borrowed_icusd_amount, borrow_amount, "debt = borrowed");
    let bal = icusd_balance(&pic, icusd, user());
    assert!(
        bal >= borrow_amount - E8,
        "user received ~100 icUSD (net of borrow fee): {bal}"
    );

    // ── repay the full debt ───────────────────────────────────────────────────
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

    // ── withdraw & close: creates a SolClaim; a native-SOL vault STAYS OPEN
    // (design doc §4.1: the rent-exempt reserve remains locked at custody) ────
    let block_index = decode_result::<Option<u64>>(
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
    assert!(block_index.is_some(), "a claim id is returned");
    let v = get_vault(&pic, backend, vault_id)
        .expect("native-SOL vault stays open after withdraw&close (rent-exempt reserve locked)");
    assert_eq!(v.collateral_amount, 0, "collateral fully withdrawn");
    assert_eq!(v.borrowed_icusd_amount, 0, "still no debt");

    let claims = sol_claims(&pic, backend);
    assert_eq!(claims.len(), 1, "one SOL claim after close: {claims:?}");
    let claim_id = claims[0].0;
    assert_eq!(claims[0].1.claimant, user());
    assert_eq!(
        claims[0].1.lamports,
        deposit_lamports - RENT_EXEMPT_LAMPORTS,
        "claim carries the exact withdrawn collateral"
    );

    // ── settle_sol_claim: two-phase (submit, then AlreadyPaid confirm) ───────
    // A valid on-curve destination distinct from custody/settlement/nonce.
    let dest = ed25519_dalek_address(&[0xAAu8; 32]);

    assert_eq!(
        send_transaction_count(&pic, sol_rpc_mock),
        0,
        "no settlement submitted yet"
    );
    let sig1 = decode_result::<String>(
        update_as(
            &pic,
            backend,
            user(),
            "settle_sol_claim",
            Encode!(&claim_id, &dest).unwrap(),
        ),
        "settle_sol_claim (submit)",
    )
    .expect("settle submit ok");
    assert!(!sig1.is_empty(), "settle returns the local tx signature");
    assert_eq!(
        send_transaction_count(&pic, sol_rpc_mock),
        1,
        "exactly one transfer submitted"
    );
    assert_eq!(
        sol_claims(&pic, backend).len(),
        1,
        "claim retained until the transfer confirms"
    );

    // Mock now reports the submitted tx as Confirmed (default `tx_confirmed=true`,
    // unchanged) -> second call is `AlreadyPaid`: NO second transfer, claim removed.
    let sig2 = decode_result::<String>(
        update_as(
            &pic,
            backend,
            user(),
            "settle_sol_claim",
            Encode!(&claim_id, &dest).unwrap(),
        ),
        "settle_sol_claim (confirm)",
    )
    .expect("settle confirm ok");
    assert_eq!(sig2, sig1, "confirm returns the same (already-broadcast) signature");
    assert_eq!(
        send_transaction_count(&pic, sol_rpc_mock),
        1,
        "AlreadyPaid must NOT submit a second transfer"
    );
    assert!(
        sol_claims(&pic, backend).is_empty(),
        "claim removed after a Confirmed settlement (AlreadyPaid)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Liquidation path: SOL vault goes underwater -> claim-based external liquidation
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn sol_native_liquidation_is_claim_based() {
    let Env {
        pic,
        backend,
        icusd,
        xrc,
        sol_rpc_mock,
    } = boot();

    if !set_prod_key_and_bootstrap_nonce(&pic, backend) {
        return; // gated
    }
    register_sol(&pic, backend);

    let open: SolVaultOpenInfo = decode_result::<SolVaultOpenInfo>(
        update_as(&pic, backend, user(), "open_sol_vault", Encode!().unwrap()),
        "open_sol_vault",
    )
    .expect("open_sol_vault ok");
    let vault_id = open.vault_id;

    let deposit_lamports = 2 * SOL;
    update_as(
        &pic,
        sol_rpc_mock,
        Principal::anonymous(),
        "set_balance",
        Encode!(&open.custody_address, &deposit_lamports).unwrap(),
    );
    decode_result::<u64>(
        update_as(
            &pic,
            backend,
            user(),
            "confirm_sol_deposit",
            Encode!(&vault_id).unwrap(),
        ),
        "confirm_sol_deposit",
    )
    .expect("confirm ok");

    // Borrow near the limit at $150: ~1.999 SOL * $150 ≈ $299.9; borrow $220 -> CR ~136%.
    let borrow_amount = 220 * E8;
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

    // Crash SOL/USD from $150 to $108: 1.999 SOL -> ~$215.9 vs ~$220 debt -> CR
    // ~98%, well below the 120% liquidation threshold. 108/150 = 0.72 stays
    // inside the 0.70 price-sanity band (`PRICE_SANITY_BAND_RATIO`), so the
    // single on-demand re-fetch during liquidation accepts it immediately (a
    // deeper one-shot crash would be queued as an outlier needing N
    // confirmations, as $100 — ratio 0.667 — was observed to be).
    crash_sol_price(&pic, xrc, 108 * E8);

    mint_icusd(&pic, icusd, backend, liquidator(), 200 * E8);
    approve_icusd(&pic, icusd, liquidator(), backend, 200 * E8);

    // Native-SOL is excluded from automated SP/bot liquidation, so this is the
    // external, claim-based path: the liquidator repays part of the debt and the
    // seized SOL becomes a SolClaim they later settle to a Solana address.
    let before_claims = sol_claims(&pic, backend);
    let liquidation_result = decode_result::<rumi_protocol_backend::SuccessWithFee>(
        update_as(
            &pic,
            backend,
            liquidator(),
            "liquidate_vault_partial",
            Encode!(&VaultArg {
                vault_id,
                amount: 150 * E8
            })
            .unwrap(),
        ),
        "liquidate_vault_partial",
    )
    .expect("claim-based partial liquidation ok");
    let after = sol_claims(&pic, backend);
    assert!(
        after.len() > before_claims.len(),
        "claim-based liquidation produced a SolClaim (before={before_claims:?}, after={after:?})"
    );

    // See this file's closing doc comment: SuccessWithFee has no separate
    // `sol_claim_id` field; `xrp_claim_id` is the shared native-claim-id slot
    // populated for BOTH XRP and SOL by `queue_collateral_payout`.
    let claim_id = liquidation_result
        .xrp_claim_id
        .expect("native-SOL manual liquidation returns the liquidator reward claim id");
    assert!(
        before_claims.iter().all(|(id, _)| *id != claim_id),
        "returned claim id must be newly created during liquidation"
    );
    let (_, returned_claim) = after
        .iter()
        .find(|(id, _)| *id == claim_id)
        .unwrap_or_else(|| panic!("returned claim id {claim_id} absent from claims {after:?}"));
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
        Some(returned_claim.lamports),
        liquidation_result.collateral_amount_received,
        "returned claim id must carry the exact liquidator reward amount"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Candid contract shape: what the vault_frontend's SOL panel gate +
// collateralStore read off a real registered SOL collateral (mirrors
// xrp_collateral_contract_matches_frontend_expectations). No tEd25519 needed
// beyond the shared bootstrap, since only the REGISTERED CONFIG is inspected.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn sol_collateral_contract_matches_frontend_expectations() {
    let Env { pic, backend, .. } = boot();
    if !set_prod_key_and_bootstrap_nonce(&pic, backend) {
        return; // gated
    }
    register_sol(&pic, backend);
    let sol = sol_collateral_principal();

    // collateralStore.fetchSupportedCollateral iterates get_supported_collateral_types;
    // the panel only renders if an entry with custody NativeSol is present here.
    let supported: Vec<(Principal, candid::Reserved)> = query_as(
        &pic,
        backend,
        Principal::anonymous(),
        "get_supported_collateral_types",
        Encode!().unwrap(),
    );
    assert!(
        supported.iter().any(|(p, _)| *p == sol),
        "native-SOL must appear in get_supported_collateral_types: {:?}",
        supported
            .iter()
            .map(|(p, _)| p.to_text())
            .collect::<Vec<_>>()
    );

    // The UI reads get_collateral_config(sol): custody_kind drives the SOL-panel
    // gate and the "SOL"/skip-icrc1_symbol branch; decimals (9) drives collateral math.
    let cfg: Option<CollateralConfigView> = query_as(
        &pic,
        backend,
        Principal::anonymous(),
        "get_collateral_config",
        Encode!(&sol).unwrap(),
    );
    let cfg = cfg.expect("SOL collateral config exists after register_sol_collateral");
    assert_eq!(cfg.decimals, 9, "native-SOL is 9 decimals (lamports)");
    assert_eq!(
        cfg.custody_kind,
        Some(CustodyKindView::NativeSol),
        "native-SOL custody_kind must be NativeSol so the frontend detects it"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Idempotency (design doc §5.3): the two highest-value cases, through the REAL
// canister rather than the pure sol_settlement_decision helper.
// ═══════════════════════════════════════════════════════════════════════════════

/// `Confirmed` -> `AlreadyPaid`: covered as the closing steps of the happy path
/// above (kept there so it also proves the whole flow works end to end). This
/// test isolates the SECOND, equally load-bearing case that a mid-flow assert
/// buried inside the happy path would not stand out on its own: `NotFound`
/// with the nonce UNCHANGED must permit a re-sign (design doc §5.3, `SafeToResign`).
#[test]
fn sol_settle_claim_notfound_unchanged_nonce_permits_resign() {
    let Env {
        pic,
        backend,
        icusd,
        xrc: _,
        sol_rpc_mock,
    } = boot();

    if !set_prod_key_and_bootstrap_nonce(&pic, backend) {
        return; // gated
    }
    register_sol(&pic, backend);

    let open: SolVaultOpenInfo = decode_result::<SolVaultOpenInfo>(
        update_as(&pic, backend, user(), "open_sol_vault", Encode!().unwrap()),
        "open_sol_vault",
    )
    .expect("open_sol_vault ok");
    let vault_id = open.vault_id;
    let deposit_lamports = 3 * SOL;
    update_as(
        &pic,
        sol_rpc_mock,
        Principal::anonymous(),
        "set_balance",
        Encode!(&open.custody_address, &deposit_lamports).unwrap(),
    );
    decode_result::<u64>(
        update_as(
            &pic,
            backend,
            user(),
            "confirm_sol_deposit",
            Encode!(&vault_id).unwrap(),
        ),
        "confirm_sol_deposit",
    )
    .expect("confirm ok");

    // No borrow needed: withdraw & close a debt-free vault straight to a claim.
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
    let claim_id = sol_claims(&pic, backend)[0].0;

    let dest = ed25519_dalek_address(&[0xBBu8; 32]);

    // Submit #1.
    let sig1 = decode_result::<String>(
        update_as(
            &pic,
            backend,
            user(),
            "settle_sol_claim",
            Encode!(&claim_id, &dest).unwrap(),
        ),
        "settle_sol_claim (submit 1)",
    )
    .expect("settle submit 1 ok");
    assert!(!sig1.is_empty(), "settle returns the local tx signature");
    assert_eq!(send_transaction_count(&pic, sol_rpc_mock), 1);

    // The mock's durable-nonce blockhash is untouched (still its default,
    // recognizable value) — i.e. the "live nonce" the second settle call reads
    // is IDENTICAL to what was recorded with sig1's settlement. Flip the mock to
    // report the prior signature NotFound: `NotFound` + unchanged nonce is
    // conclusive proof the exact signed bytes never landed (a durable-nonce tx
    // that executes always advances the nonce first) -> SafeToResign.
    update_as(
        &pic,
        sol_rpc_mock,
        Principal::anonymous(),
        "set_tx_confirmed",
        Encode!(&false).unwrap(),
    );

    let sig2 = decode_result::<String>(
        update_as(
            &pic,
            backend,
            user(),
            "settle_sol_claim",
            Encode!(&claim_id, &dest).unwrap(),
        ),
        "settle_sol_claim (resign)",
    )
    .expect("re-sign must be PERMITTED (SafeToResign): NotFound + nonce unchanged");
    assert!(!sig2.is_empty());
    assert_eq!(
        send_transaction_count(&pic, sol_rpc_mock),
        2,
        "a legitimate re-sign DOES submit a second transfer (unlike AlreadyPaid)"
    );
    assert_eq!(
        sol_claims(&pic, backend).len(),
        1,
        "claim retained again after the re-sign (not yet confirmed)"
    );

    // Sanity: icusd_balance query still resolves (canister healthy end to end).
    let _ = icusd_balance(&pic, icusd, user());
}

// ═══════════════════════════════════════════════════════════════════════════════
// register_sol_collateral refuses when the nonce account is not bootstrapped
// (design doc §3/§5.1 ordering invariant). No tEd25519 needed: this is a pure
// precondition check ahead of any derivation/signing.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn register_sol_collateral_refuses_without_bootstrapped_nonce() {
    let Env { pic, backend, .. } = boot();

    // Dev-gate: a non-developer caller is rejected before any precondition check.
    let non_dev_result = decode_result::<()>(
        update_as(
            &pic,
            backend,
            non_dev(),
            "register_sol_collateral",
            Encode!().unwrap(),
        ),
        "register_sol_collateral (non-dev)",
    );
    assert!(non_dev_result.is_err(), "non-developer must be rejected");

    // Production key configured, but sol_bootstrap_nonce_account was NEVER
    // called: s.sol_nonce_account stays None.
    decode_result::<()>(
        update_as(
            &pic,
            backend,
            dev(),
            "set_sol_schnorr_key_name",
            Encode!(&"key_1".to_string()).unwrap(),
        ),
        "set_sol_schnorr_key_name",
    )
    .expect("set_sol_schnorr_key_name");

    let result = decode_result::<()>(
        update_as(
            &pic,
            backend,
            dev(),
            "register_sol_collateral",
            Encode!().unwrap(),
        ),
        "register_sol_collateral (no nonce)",
    );
    assert!(
        result.is_err(),
        "register_sol_collateral must refuse without a bootstrapped durable-nonce account"
    );

    // Not registered as a side effect of the refused call.
    let supported: Vec<(Principal, candid::Reserved)> = query_as(
        &pic,
        backend,
        Principal::anonymous(),
        "get_supported_collateral_types",
        Encode!().unwrap(),
    );
    assert!(
        supported.iter().all(|(p, _)| *p != sol_collateral_principal()),
        "SOL must NOT appear in supported collateral types after a refused registration"
    );
}

// ─── Small local helpers ──────────────────────────────────────────────────────

/// A well-formed base58 Solana destination address, derived from a real
/// Ed25519 keypair seed (guaranteed on-curve, matching the design doc §7
/// requirement that `settle_sol_claim` rejects off-curve PDAs). Uses
/// `ed25519-dalek` (already a dev-dependency of the backend crate for the same
/// purpose in `chains/sol/address.rs`'s own tests).
fn ed25519_dalek_address(seed: &[u8; 32]) -> String {
    use ed25519_dalek::SigningKey;
    let sk = SigningKey::from_bytes(seed);
    let pk = sk.verifying_key();
    bs58::encode(pk.to_bytes()).into_string()
}
