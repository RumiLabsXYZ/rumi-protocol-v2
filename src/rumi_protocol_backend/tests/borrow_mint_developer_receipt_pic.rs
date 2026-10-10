//! Canister-boundary regression for developer-assisted positive receipt recovery.
//!
//! The flaky ledger first commits one `1mint` block and simulates a lost reply.
//! Its opt-in retry control then returns typed `TooOld` for that exact duplicate.
//! This models the backend transition into receipt recovery; it does not claim
//! the fixture reproduces a real ledger's timestamp-expiry implementation.

use candid::{decode_one, encode_args, encode_one, CandidType, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::state::{BorrowMintPhase, BorrowMintStatus};
use rumi_protocol_backend::vault::{CandidVault, OpenVaultSuccess, VaultArg};
use rumi_protocol_backend::{ProtocolError, SuccessWithFee};
use serde::Deserialize;
use std::collections::HashMap;

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
    Upgrade(Option<()>),
}

#[derive(CandidType, Deserialize)]
struct MockXrc {
    rates: HashMap<String, u64>,
}

#[derive(CandidType, Deserialize)]
struct LedgerAccount {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: LedgerAccount,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

fn backend_wasm() -> Vec<u8> {
    let path = std::env::var_os("RUMI_BORROW_RECEIPT_BACKEND_WASM").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        },
        std::path::PathBuf::from,
    );
    std::fs::read(path).expect("build the test_endpoints backend Wasm before running this test")
}

fn flaky_ledger_wasm() -> Vec<u8> {
    let path = std::env::var_os("RUMI_BORROW_RECEIPT_LEDGER_WASM").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm")
        },
        std::path::PathBuf::from,
    );
    std::fs::read(path).expect("build the flaky_ledger Wasm before running this test")
}

fn call(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> WasmResult {
    pic.update_call(canister, caller, method, args)
        .unwrap_or_else(|error| panic!("{method} call failed: {error}"))
}

fn expect_reply(reply: WasmResult, method: &str) {
    if let WasmResult::Reject(message) = reply {
        panic!("{method} rejected: {message}");
    }
}

fn install_flaky_ledger(pic: &PocketIc) -> Principal {
    let canister = pic.create_canister();
    pic.add_cycles(canister, 2_000_000_000_000);
    pic.install_canister(canister, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    canister
}

fn result<T: CandidType + for<'de> Deserialize<'de>>(reply: WasmResult, method: &str) -> T {
    match reply {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method} reply: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn ledger_blocks(pic: &PocketIc, ledger: Principal) -> u64 {
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_args((vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
                start: Nat::from(0u8),
                length: Nat::from(100u8),
            }],))
            .unwrap(),
        )
        .expect("query icrc3_get_blocks");
    let blocks: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
        result(reply, "icrc3_get_blocks");
    u64::try_from(blocks.log_length.0).expect("ledger log length fits u64")
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_args((LedgerAccount {
                owner,
                subaccount: None,
            },))
            .unwrap(),
        )
        .expect("query icrc1_balance_of");
    let balance: Nat = result(reply, "icrc1_balance_of");
    u64::try_from(balance.0).expect("balance fits u64")
}

fn pending_mints(pic: &PocketIc, backend: Principal, owner: Principal) -> Vec<BorrowMintStatus> {
    let reply = pic
        .query_call(
            backend,
            owner,
            "get_my_pending_borrow_mints",
            encode_args(()).unwrap(),
        )
        .expect("query get_my_pending_borrow_mints");
    result(reply, "get_my_pending_borrow_mints")
}

fn vault_debt(pic: &PocketIc, backend: Principal, owner: Principal, vault_id: u64) -> u64 {
    let reply = pic
        .query_call(
            backend,
            owner,
            "get_vaults",
            encode_args((Some(owner),)).unwrap(),
        )
        .expect("query get_vaults");
    let vaults: Vec<CandidVault> = result(reply, "get_vaults");
    let view = vaults
        .into_iter()
        .find(|vault| vault.vault_id == vault_id)
        .expect("owner's vault exists");
    view.borrowed_icusd_amount
}

#[test]
#[ignore = "requires source-matched test_endpoints Wasms and PocketIC server 7.0.0"]
fn developer_reconciles_exact_committed_mint_for_owner_once() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let owner = Principal::self_authenticating(b"borrow-receipt-owner");
    let stranger = Principal::self_authenticating(b"borrow-receipt-stranger");
    let developer = Principal::self_authenticating(b"borrow-receipt-developer");

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let icp_ledger = install_flaky_ledger(&pic);
    let icusd_ledger = install_flaky_ledger(&pic);
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    let mut rates = HashMap::new();
    rates.insert("ICP/USD".to_string(), 1_000_000_000);
    pic.install_canister(
        xrc,
        include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(),
        encode_one(MockXrc { rates }).unwrap(),
        None,
    );

    let protocol_init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd_ledger,
        icp_ledger_principal: icp_ledger,
        fee_e8s: 0,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend,
        backend_wasm(),
        encode_one(protocol_init).unwrap(),
        None,
    );

    expect_reply(
        call(
            &pic,
            icp_ledger,
            owner,
            "mint",
            encode_args((
                LedgerAccount {
                    owner,
                    subaccount: None,
                },
                Nat::from(5_000_000_000u64),
            ))
            .unwrap(),
        ),
        "fixture ICP mint",
    );
    let collateral_amount = 5_000_000_000u64;
    expect_reply(
        call(
            &pic,
            icp_ledger,
            owner,
            "icrc2_approve",
            encode_args((ApproveArgs {
                from_subaccount: None,
                spender: LedgerAccount {
                    owner: backend,
                    subaccount: None,
                },
                amount: Nat::from(collateral_amount),
                expected_allowance: None,
                expires_at: None,
                fee: None,
                memo: None,
                created_at_time: None,
            },))
            .unwrap(),
        ),
        "fixture ICP approve",
    );

    let open_reply = call(
        &pic,
        backend,
        owner,
        "open_vault",
        encode_args((collateral_amount, Option::<Principal>::None)).unwrap(),
    );
    let opened: Result<OpenVaultSuccess, ProtocolError> = result(open_reply, "open_vault");
    let vault_id = opened.expect("open owner vault").vault_id;
    let price_reply = call(
        &pic,
        backend,
        developer,
        "dev_set_collateral_price",
        encode_args((icp_ledger, 10.0f64)).unwrap(),
    );
    let price_set: Result<String, ProtocolError> = result(price_reply, "dev_set_collateral_price");
    price_set.expect("set test ICP price");

    expect_reply(
        call(
            &pic,
            icusd_ledger,
            owner,
            "set_minter",
            encode_args((Some(backend),)).unwrap(),
        ),
        "set_minter",
    );
    expect_reply(
        call(
            &pic,
            icusd_ledger,
            owner,
            "set_phantom_icrc1_failures",
            encode_args((1u32,)).unwrap(),
        ),
        "set_phantom_icrc1_failures",
    );

    let borrow_amount = 1_000_000_000u64;
    let first_reply = call(
        &pic,
        backend,
        owner,
        "borrow_from_vault",
        encode_args((VaultArg {
            vault_id,
            amount: borrow_amount,
        },))
        .unwrap(),
    );
    let first: Result<SuccessWithFee, ProtocolError> = result(first_reply, "borrow_from_vault");
    assert!(
        first.is_err(),
        "first mint reply is deliberately simulated as lost"
    );
    assert_eq!(vault_debt(&pic, backend, owner, vault_id), 0);
    let balance_after_phantom = balance(&pic, icusd_ledger, owner);
    let blocks_after_phantom = ledger_blocks(&pic, icusd_ledger);
    assert_eq!(
        blocks_after_phantom, 1,
        "the phantom mint committed exactly one block"
    );

    expect_reply(
        call(
            &pic,
            icusd_ledger,
            owner,
            "set_too_old_after_phantom_mint",
            encode_args((1u32,)).unwrap(),
        ),
        "set_too_old_after_phantom_mint",
    );
    let retry_reply = call(
        &pic,
        backend,
        owner,
        "borrow_from_vault",
        encode_args((VaultArg {
            vault_id,
            amount: borrow_amount,
        },))
        .unwrap(),
    );
    let retry: Result<SuccessWithFee, ProtocolError> =
        result(retry_reply, "borrow_from_vault retry");
    assert!(
        retry.is_err(),
        "fixture returns a simulated typed TooOld on the exact duplicate"
    );
    let held = pending_mints(&pic, backend, owner);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].phase, BorrowMintPhase::ReceiptRecoveryRequired);
    assert_eq!(
        ledger_blocks(&pic, icusd_ledger),
        blocks_after_phantom,
        "TooOld retry must not append a mint"
    );

    let stranger_reply = call(
        &pic,
        backend,
        stranger,
        "reconcile_pending_borrow_mint_from_block",
        encode_args((vault_id, 0u64)).unwrap(),
    );
    let stranger_result: Result<SuccessWithFee, ProtocolError> =
        result(stranger_reply, "stranger reconciliation");
    assert!(
        stranger_result.is_err(),
        "stranger cannot reconcile the owner's mint"
    );
    assert_eq!(vault_debt(&pic, backend, owner, vault_id), 0);

    let wrong_block_reply = call(
        &pic,
        backend,
        developer,
        "reconcile_pending_borrow_mint_from_block",
        encode_args((vault_id, 1u64)).unwrap(),
    );
    let wrong_block: Result<SuccessWithFee, ProtocolError> =
        result(wrong_block_reply, "wrong-block reconciliation");
    assert!(
        wrong_block.is_err(),
        "developer cannot reconcile from a nonmatching block"
    );
    assert_eq!(pending_mints(&pic, backend, owner).len(), 1);
    assert_eq!(vault_debt(&pic, backend, owner, vault_id), 0);

    let exact_reply = call(
        &pic,
        backend,
        developer,
        "reconcile_pending_borrow_mint_from_block",
        encode_args((vault_id, 0u64)).unwrap(),
    );
    let exact: Result<SuccessWithFee, ProtocolError> =
        result(exact_reply, "exact developer reconciliation");
    exact.expect("developer reconciles exact positive receipt");
    assert_eq!(
        vault_debt(&pic, backend, owner, vault_id),
        borrow_amount,
        "debt belongs to journal owner once"
    );
    assert!(pending_mints(&pic, backend, owner).is_empty());
    assert_eq!(
        balance(&pic, icusd_ledger, owner),
        balance_after_phantom,
        "reconciliation must not mint again"
    );
    assert_eq!(
        ledger_blocks(&pic, icusd_ledger),
        blocks_after_phantom,
        "reconciliation must not append another ledger block"
    );

    let repeat_reply = call(
        &pic,
        backend,
        owner,
        "reconcile_pending_borrow_mint_from_block",
        encode_args((vault_id, 0u64)).unwrap(),
    );
    let repeat: Result<SuccessWithFee, ProtocolError> =
        result(repeat_reply, "repeat reconciliation");
    assert!(
        repeat.is_err(),
        "settled receipt cannot be applied a second time"
    );
    assert_eq!(vault_debt(&pic, backend, owner, vault_id), borrow_amount);
    assert_eq!(balance(&pic, icusd_ledger, owner), balance_after_phantom);
    assert_eq!(ledger_blocks(&pic, icusd_ledger), blocks_after_phantom);
}
