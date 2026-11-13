//! PocketIC regression for the durable ICRC-2 collateral-pull journal.
//!
//! The flaky ledger commits `icrc2_transfer_from` and returns a GenericError
//! once, simulating a lost reply. Replaying the same operation must reuse the
//! same transfer tuple, credit the vault once, and reject altered requests
//! without another ledger dispatch.

use candid::{decode_one, encode_args, encode_one, CandidType, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::state::{
    BorrowMintPhase, BorrowMintStatus, VaultCollateralPullPhase, VaultCollateralPullRequest,
    VaultOperationStatus,
};
use rumi_protocol_backend::vault::{
    AddMarginV2Args, CandidVault, OpenVaultAndBorrowV2Args, OpenVaultSuccess, OpenVaultV2Args,
    VaultOperationIdArg,
};
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
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: Account,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

fn wasm_path(env_name: &str, default_relative: &str) -> std::path::PathBuf {
    std::env::var_os(env_name).map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(default_relative),
        std::path::PathBuf::from,
    )
}

fn protocol_wasm() -> Vec<u8> {
    std::fs::read(wasm_path(
        "RUMI_VAULT_PULL_BACKEND_WASM",
        "../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm",
    ))
    .expect("build the source-matched rumi_protocol_backend Wasm before running this test")
}

fn flaky_ledger_wasm() -> Vec<u8> {
    std::fs::read(wasm_path(
        "RUMI_VAULT_PULL_LEDGER_WASM",
        "../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm",
    ))
    .expect("build the flaky_ledger Wasm before running this test")
}

fn reply<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult, method: &str) -> T {
    match result {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method} reply: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
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

fn install_flaky_ledger(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    id
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let bytes = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(Account {
                owner,
                subaccount: None,
            })
            .unwrap(),
        )
        .expect("query icrc1_balance_of");
    let balance: Nat = reply(bytes, "icrc1_balance_of");
    u64::try_from(balance.0).expect("balance fits u64")
}

fn block_count(pic: &PocketIc, ledger: Principal) -> u64 {
    let bytes = pic
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
        reply(bytes, "icrc3_get_blocks");
    u64::try_from(blocks.log_length.0).expect("block count fits u64")
}

fn status(pic: &PocketIc, backend: Principal, owner: Principal) -> VaultOperationStatus {
    let bytes = pic
        .query_call(
            backend,
            owner,
            "get_vault_operation_status",
            encode_args(()).unwrap(),
        )
        .expect("query get_vault_operation_status");
    let result: Result<VaultOperationStatus, ProtocolError> =
        reply(bytes, "get_vault_operation_status");
    result.expect("operation status query succeeds")
}

fn install_fixture() -> (PocketIc, Principal, Principal, Principal, Principal, u64) {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let owner = Principal::self_authenticating(b"vault-pull-owner");
    let developer = Principal::self_authenticating(b"vault-pull-developer");
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
    pic.install_canister(
        backend,
        protocol_wasm(),
        encode_one(ProtocolArg::Init(ProtocolInitArg {
            xrc_principal: xrc,
            icusd_ledger_principal: icusd_ledger,
            icp_ledger_principal: icp_ledger,
            fee_e8s: 0,
            developer_principal: developer,
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }))
        .unwrap(),
        None,
    );
    // The compound path validates the cached collateral price. Let the
    // canister's zero-delay startup XRC fetch consume the seeded ICP/USD rate.
    pic.advance_time(std::time::Duration::from_secs(1));
    for _ in 0..5 {
        pic.tick();
    }

    let amount = 5_000_000_000u64;
    let user_balance = amount + 10_000;
    call(
        &pic,
        icp_ledger,
        Principal::anonymous(),
        "set_fee",
        encode_one(Nat::from(10_000u64)).unwrap(),
    );
    call(
        &pic,
        icp_ledger,
        Principal::anonymous(),
        "mint",
        encode_args((
            Account {
                owner,
                subaccount: None,
            },
            Nat::from(user_balance),
        ))
        .unwrap(),
    );
    call(
        &pic,
        icp_ledger,
        owner,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: backend,
                subaccount: None,
            },
            amount: Nat::from(user_balance),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        })
        .unwrap(),
    );

    (pic, backend, icp_ledger, icusd_ledger, owner, amount)
}

fn assert_one_journaled_pull(compound: bool) {
    let (pic, backend, ledger, _icusd_ledger, owner, amount) = install_fixture();

    let blocks_before_pull = block_count(&pic, ledger);
    let balance_before_pull = balance(&pic, ledger, owner);
    let backend_before_pull = balance(&pic, ledger, backend);
    if !compound {
        let old_open: Result<OpenVaultSuccess, ProtocolError> = reply(
            call(
                &pic,
                backend,
                owner,
                "open_vault",
                encode_args((amount, Option::<Principal>::None)).unwrap(),
            ),
            "open_vault",
        );
        assert!(old_open.is_err(), "legacy open_vault must be disabled");
        assert_eq!(balance(&pic, ledger, owner), balance_before_pull);
        assert_eq!(balance(&pic, ledger, backend), backend_before_pull);
        assert_eq!(block_count(&pic, ledger), blocks_before_pull);
    }
    call(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        encode_one(1u32).unwrap(),
    );

    let method = if compound {
        "open_vault_and_borrow_v2"
    } else {
        "open_vault_v2"
    };
    let operation_id = 1u64;
    let first_args = if compound {
        encode_one(OpenVaultAndBorrowV2Args {
            operation_id,
            collateral_amount: amount,
            borrow_amount: 0,
            collateral_type: None,
        })
        .unwrap()
    } else {
        encode_one(OpenVaultV2Args {
            operation_id,
            collateral_amount: amount,
            collateral_type: None,
        })
        .unwrap()
    };
    let first: Result<OpenVaultSuccess, ProtocolError> = reply(
        call(&pic, backend, owner, method, first_args.clone()),
        method,
    );
    assert!(
        first.is_err(),
        "phantom commit must remain unresolved until replay; got {first:?}"
    );
    assert_eq!(
        balance(&pic, ledger, owner),
        balance_before_pull - amount - 10_000
    );
    assert_eq!(balance(&pic, ledger, backend), backend_before_pull + amount);
    assert_eq!(block_count(&pic, ledger), blocks_before_pull + 1);
    let submitted = status(&pic, backend, owner)
        .active
        .expect("operation stays active");
    assert_eq!(submitted.operation_id, operation_id);
    assert_eq!(submitted.phase, VaultCollateralPullPhase::Submitted);
    assert_eq!(
        submitted.request,
        if compound {
            VaultCollateralPullRequest::OpenVaultAndBorrow {
                collateral_type: ledger,
                amount_e8s: amount,
                borrow_amount_e8s: 0,
            }
        } else {
            VaultCollateralPullRequest::OpenVault {
                collateral_type: ledger,
                amount_e8s: amount,
            }
        }
    );
    let original_request = submitted.request.clone();

    let changed_id_args = if compound {
        encode_one(OpenVaultAndBorrowV2Args {
            operation_id: operation_id + 1,
            collateral_amount: amount,
            borrow_amount: 0,
            collateral_type: None,
        })
        .unwrap()
    } else {
        encode_one(OpenVaultV2Args {
            operation_id: operation_id + 1,
            collateral_amount: amount,
            collateral_type: None,
        })
        .unwrap()
    };
    let changed_id: Result<OpenVaultSuccess, ProtocolError> =
        reply(call(&pic, backend, owner, method, changed_id_args), method);
    assert!(
        changed_id.is_err(),
        "a different operation ID cannot replace an ambiguous pull"
    );

    let changed_amount_args = if compound {
        encode_one(OpenVaultAndBorrowV2Args {
            operation_id,
            collateral_amount: amount + 1,
            borrow_amount: 0,
            collateral_type: None,
        })
        .unwrap()
    } else {
        encode_one(OpenVaultV2Args {
            operation_id,
            collateral_amount: amount + 1,
            collateral_type: None,
        })
        .unwrap()
    };
    let changed_amount: Result<OpenVaultSuccess, ProtocolError> = reply(
        call(&pic, backend, owner, method, changed_amount_args),
        method,
    );
    assert!(
        changed_amount.is_err(),
        "the operation ID is bound to its original arguments"
    );
    let still_submitted = status(&pic, backend, owner)
        .active
        .expect("ambiguous operation remains active");
    assert_eq!(still_submitted.operation_id, operation_id);
    assert_eq!(
        still_submitted.request, original_request,
        "rejected variants preserve the journal tuple"
    );
    assert_eq!(still_submitted.phase, VaultCollateralPullPhase::Submitted);
    assert_eq!(
        balance(&pic, ledger, owner),
        balance_before_pull - amount - 10_000
    );
    assert_eq!(balance(&pic, ledger, backend), backend_before_pull + amount);
    assert_eq!(block_count(&pic, ledger), blocks_before_pull + 1);

    let retry: Result<OpenVaultSuccess, ProtocolError> =
        reply(call(&pic, backend, owner, method, first_args), method);
    let opened = retry.expect("identical retry accepts the ledger's duplicate receipt");
    assert_eq!(
        balance(&pic, ledger, owner),
        balance_before_pull - amount - 10_000
    );
    assert_eq!(balance(&pic, ledger, backend), backend_before_pull + amount);
    assert_eq!(block_count(&pic, ledger), blocks_before_pull + 1);

    let vaults: Vec<CandidVault> = reply(
        pic.query_call(
            backend,
            owner,
            "get_vaults",
            encode_args((Some(owner),)).unwrap(),
        )
        .expect("query get_vaults"),
        "get_vaults",
    );
    let owned: Vec<_> = vaults
        .iter()
        .filter(|vault| vault.vault_id == opened.vault_id)
        .collect();
    assert_eq!(owned.len(), 1, "the pull credits exactly one vault");
    assert_eq!(owned[0].collateral_amount, amount);
    let completed = status(&pic, backend, owner)
        .active
        .expect("result remains until ACK");
    assert_eq!(completed.operation_id, operation_id);
    assert!(matches!(
        completed.phase,
        VaultCollateralPullPhase::Completed { .. }
    ));
}

#[test]
fn open_vault_v2_replays_one_phantom_committed_collateral_pull() {
    assert_one_journaled_pull(false);
}

#[test]
fn compound_open_v2_replays_one_phantom_committed_collateral_pull() {
    assert_one_journaled_pull(true);
}

fn assert_compound_borrow_mint_receipt_recovery_completes_bound_operation(use_scan: bool) {
    let (pic, backend, collateral_ledger, icusd_ledger, owner, amount) = install_fixture();
    call(
        &pic,
        icusd_ledger,
        Principal::anonymous(),
        "set_minter",
        encode_args((Some(backend),)).unwrap(),
    );
    call(
        &pic,
        icusd_ledger,
        Principal::anonymous(),
        "set_phantom_icrc1_failures",
        encode_args((1u32,)).unwrap(),
    );

    let operation_id = 1u64;
    let borrow_amount = 1_000_000_000u64;
    let compound_args = encode_one(OpenVaultAndBorrowV2Args {
        operation_id,
        collateral_amount: amount,
        borrow_amount,
        collateral_type: None,
    })
    .unwrap();
    let opened: Result<OpenVaultSuccess, ProtocolError> = reply(
        call(
            &pic,
            backend,
            owner,
            "open_vault_and_borrow_v2",
            compound_args.clone(),
        ),
        "open_vault_and_borrow_v2",
    );
    assert!(
        opened.is_err(),
        "the borrow mint reply is deliberately lost"
    );

    let active = status(&pic, backend, owner)
        .active
        .expect("compound operation remains active");
    assert_eq!(active.operation_id, operation_id);
    assert_eq!(
        active.request,
        VaultCollateralPullRequest::OpenVaultAndBorrow {
            collateral_type: collateral_ledger,
            amount_e8s: amount,
            borrow_amount_e8s: borrow_amount,
        }
    );
    assert!(matches!(
        active.phase,
        VaultCollateralPullPhase::VaultCredited { .. }
    ));

    let pending_reply = pic
        .query_call(
            backend,
            owner,
            "get_my_pending_borrow_mints",
            encode_args(()).unwrap(),
        )
        .expect("query get_my_pending_borrow_mints");
    let pending: Vec<BorrowMintStatus> = reply(pending_reply, "get_my_pending_borrow_mints");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].vault_id, active.vault_id);
    assert_eq!(pending[0].borrowed_amount_e8s, borrow_amount);
    assert_eq!(pending[0].phase, BorrowMintPhase::SubmittedOrUnknown);
    assert_eq!(balance(&pic, icusd_ledger, owner), borrow_amount);
    assert_eq!(block_count(&pic, icusd_ledger), 1);

    // A direct receipt candidate is accepted only after an exact ledger retry
    // returns TooOld and moves the durable borrow journal into proof recovery.
    // The flaky ledger models this deterministically; no wall-clock advance is
    // required to cross its dedup window.
    call(
        &pic,
        icusd_ledger,
        Principal::anonymous(),
        "set_too_old_after_phantom_mint",
        encode_one(1u32).unwrap(),
    );
    let exact_retry: Result<OpenVaultSuccess, ProtocolError> = reply(
        call(
            &pic,
            backend,
            owner,
            "open_vault_and_borrow_v2",
            compound_args,
        ),
        "open_vault_and_borrow_v2",
    );
    assert!(
        exact_retry.is_err(),
        "TooOld keeps the compound journal held for positive receipt recovery"
    );
    let pending_reply = pic
        .query_call(
            backend,
            owner,
            "get_my_pending_borrow_mints",
            encode_args(()).unwrap(),
        )
        .expect("query get_my_pending_borrow_mints after TooOld");
    let pending: Vec<BorrowMintStatus> = reply(pending_reply, "get_my_pending_borrow_mints");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].phase, BorrowMintPhase::ReceiptRecoveryRequired);
    assert_eq!(balance(&pic, icusd_ledger, owner), borrow_amount);
    assert_eq!(block_count(&pic, icusd_ledger), 1);

    if use_scan {
        let advanced: Result<(), ProtocolError> = reply(
            call(
                &pic,
                backend,
                owner,
                "advance_pending_borrow_mint_recovery",
                encode_args((active.vault_id,)).unwrap(),
            ),
            "advance_pending_borrow_mint_recovery",
        );
        advanced.expect("the receipt scan must commit debt to the matching compound operation");
    } else {
        let candidate_block = block_count(&pic, icusd_ledger) - 1;
        let reconciled: Result<SuccessWithFee, ProtocolError> = reply(
            call(
                &pic,
                backend,
                owner,
                "reconcile_pending_borrow_mint_from_block",
                encode_args((active.vault_id, candidate_block)).unwrap(),
            ),
            "reconcile_pending_borrow_mint_from_block",
        );
        let _reconciled = reconciled.expect(
            "positive receipt reconciliation must pass the compound operation ID into debt commit",
        );
    }

    let vaults: Vec<CandidVault> = reply(
        pic.query_call(
            backend,
            owner,
            "get_vaults",
            encode_args((Some(owner),)).unwrap(),
        )
        .expect("query get_vaults"),
        "get_vaults",
    );
    let vault = vaults
        .iter()
        .find(|vault| vault.vault_id == active.vault_id)
        .expect("compound vault exists");
    assert_eq!(vault.collateral_amount, amount);
    assert_eq!(vault.borrowed_icusd_amount, borrow_amount);
    assert!(pic
        .query_call(
            backend,
            owner,
            "get_my_pending_borrow_mints",
            encode_args(()).unwrap(),
        )
        .map(
            |bytes| reply::<Vec<BorrowMintStatus>>(bytes, "get_my_pending_borrow_mints").is_empty()
        )
        .expect("query pending borrow status"));
    let completed = status(&pic, backend, owner)
        .active
        .expect("compound result remains until ACK");
    assert!(matches!(
        completed.phase,
        VaultCollateralPullPhase::Completed { .. }
    ));
}

#[test]
fn compound_borrow_mint_receipt_reconciliation_completes_bound_operation() {
    assert_compound_borrow_mint_receipt_recovery_completes_bound_operation(false);
}

#[test]
fn compound_borrow_mint_receipt_scan_completes_bound_operation() {
    assert_compound_borrow_mint_receipt_recovery_completes_bound_operation(true);
}


#[derive(CandidType, Deserialize)]
struct Sec19PayoutView {
    operation_id: u128,
    amount_gross: rumi_protocol_backend::numeric::ICP,
    ledger: Option<Principal>,
    held_for_manual_retry: bool,
    reconciliation_required: bool,
}
#[derive(CandidType, Deserialize)]
struct Sec19PayoutPage { items: Vec<Sec19PayoutView> }

fn sec19_payouts(pic: &PocketIc, backend: Principal, owner: Principal) -> Sec19PayoutPage {
    reply(pic.query_call(backend, owner, "get_pending_payouts",
        encode_args((None::<u128>, Some(50u16))).unwrap()).unwrap(), "get_pending_payouts")
}

fn sec19_open_and_fund() -> (PocketIc, Principal, Principal, Principal, u64, u64) {
    let (pic, backend, ledger, _, owner, original) = install_fixture();
    let opened: Result<OpenVaultSuccess, ProtocolError> = reply(call(&pic, backend, owner,
        "open_vault_v2", encode_one(OpenVaultV2Args { operation_id: 1, collateral_amount: original,
            collateral_type: None }).unwrap()), "open_vault_v2");
    let vault = opened.expect("open original vault").vault_id;
    let ack: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "ack_vault_operation",
        encode_one(VaultOperationIdArg { operation_id: 1 }).unwrap()), "ack_vault_operation");
    assert_eq!(ack.unwrap(), 1);
    let topup = 1_301_909_000u64;
    call(&pic, ledger, Principal::anonymous(), "mint", encode_args((Account { owner, subaccount: None },
        Nat::from(topup + 30_000))).unwrap());
    call(&pic, ledger, owner, "icrc2_approve", encode_one(ApproveArgs {
        from_subaccount: None, spender: Account { owner: backend, subaccount: None },
        amount: Nat::from(topup + 10_000), expected_allowance: None, expires_at: None,
        fee: None, memo: None, created_at_time: None }).unwrap());
    (pic, backend, ledger, owner, vault, topup)
}

fn sec19_topup_args(vault_id: u64, amount: u64) -> Vec<u8> {
    encode_one(AddMarginV2Args { operation_id: 2, vault_id, amount }).unwrap()
}

fn sec19_vaults(pic: &PocketIc, backend: Principal, owner: Principal) -> Vec<CandidVault> {
    reply(pic.query_call(backend, owner, "get_vaults", encode_args((Some(owner),)).unwrap()).unwrap(), "get_vaults")
}

#[test]
fn sec19_topup_lost_reply_exact_retry_credits_once() {
    let (pic, backend, ledger, owner, vault_id, topup) = sec19_open_and_fund();
    let original = sec19_vaults(&pic, backend, owner).into_iter().find(|v| v.vault_id == vault_id).unwrap().collateral_amount;
    let owner_before = balance(&pic, ledger, owner);
    let backend_before = balance(&pic, ledger, backend);
    let blocks = block_count(&pic, ledger);
    call(&pic, ledger, Principal::anonymous(), "set_phantom_transfer_from_failures", encode_one(1u32).unwrap());
    let args = sec19_topup_args(vault_id, topup);
    let first: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args.clone()), "add_margin_to_vault_v2");
    assert!(first.is_err());
    assert_eq!(status(&pic, backend, owner).active.unwrap().phase, VaultCollateralPullPhase::Submitted);
    assert_eq!(sec19_vaults(&pic, backend, owner).into_iter().find(|v| v.vault_id == vault_id).unwrap().collateral_amount, original);
    assert_eq!(balance(&pic, ledger, owner), owner_before - topup - 10_000);
    assert_eq!(balance(&pic, ledger, backend), backend_before + topup);
    assert_eq!(block_count(&pic, ledger), blocks + 1);
    let retry: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args.clone()), "add_margin_to_vault_v2");
    let block = retry.expect("exact retry recovers Duplicate receipt");
    let repeated: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args), "add_margin_to_vault_v2");
    assert_eq!(repeated.unwrap(), block);
    assert_eq!(block_count(&pic, ledger), blocks + 1);
    assert_eq!(sec19_vaults(&pic, backend, owner).into_iter().find(|v| v.vault_id == vault_id).unwrap().collateral_amount, original + topup);
    assert!(sec19_payouts(&pic, backend, owner).items.is_empty());
}

#[test]
fn sec19_lost_pull_then_target_closed_creates_one_held_recovery() {
    let (pic, backend, ledger, owner, vault_id, topup) = sec19_open_and_fund();
    let args = sec19_topup_args(vault_id, topup);
    let blocks = block_count(&pic, ledger);
    call(&pic, ledger, Principal::anonymous(), "set_phantom_transfer_from_failures", encode_one(1u32).unwrap());
    let first: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args.clone()), "add_margin_to_vault_v2");
    assert!(first.is_err());
    assert_eq!(block_count(&pic, ledger), blocks + 1);
    let other = Principal::self_authenticating(b"sec19-other-owner");
    let wrong: Result<(), ProtocolError> = reply(call(&pic, backend, other, "reconcile_vault_collateral_pull_from_block",
        encode_args((VaultOperationIdArg { operation_id: 2 }, blocks)).unwrap()), "reconcile_vault_collateral_pull_from_block");
    assert!(wrong.is_err());
    let closed: Result<Option<u64>, ProtocolError> = reply(call(&pic, backend, owner, "withdraw_and_close_vault",
        encode_one(vault_id).unwrap()), "withdraw_and_close_vault");
    closed.expect("the separate zero-debt vault can be closed while the pull reply is unknown");
    assert!(sec19_vaults(&pic, backend, owner).iter().all(|v| v.vault_id != vault_id));
    let blocks_after_close = block_count(&pic, ledger);
    let owner_after_close = balance(&pic, ledger, owner);
    let backend_after_close = balance(&pic, ledger, backend);
    let reconciled: Result<(), ProtocolError> = reply(call(&pic, backend, owner, "reconcile_vault_collateral_pull_from_block",
        encode_args((VaultOperationIdArg { operation_id: 2 }, blocks)).unwrap()), "reconcile_vault_collateral_pull_from_block");
    reconciled.expect("the direct exact block proves the original pinned pull");
    let retry: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args.clone()), "add_margin_to_vault_v2");
    assert!(retry.is_err(), "held liability is not a vault credit");
    let active = status(&pic, backend, owner).active.unwrap();
    let recovery = active.recovery.expect("owner sees held recovery detail");
    assert_eq!((recovery.vault_id, recovery.block_index, recovery.amount, recovery.ledger), (vault_id, blocks, topup, ledger));
    let page = sec19_payouts(&pic, backend, owner);
    assert_eq!(page.items.len(), 1);
    let row = &page.items[0];
    assert_eq!(row.operation_id, recovery.payout_operation_id);
    assert_eq!(row.amount_gross.to_u64(), topup);
    assert_eq!(row.ledger, Some(ledger));
    assert!(row.held_for_manual_retry && row.reconciliation_required);
    assert!(sec19_payouts(&pic, backend, other).items.is_empty());
    let again: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args), "add_margin_to_vault_v2");
    assert!(again.is_err());
    assert_eq!(block_count(&pic, ledger), blocks_after_close, "reconcile and exact retry never dispatch a second pull");
    assert_eq!(balance(&pic, ledger, owner), owner_after_close);
    assert_eq!(balance(&pic, ledger, backend), backend_after_close);
    assert_eq!(sec19_payouts(&pic, backend, owner).items.len(), 1);
    let ack: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "ack_vault_operation",
        encode_one(VaultOperationIdArg { operation_id: 2 }).unwrap()), "ack_vault_operation");
    assert_eq!(ack.unwrap(), 2);
    assert_eq!(sec19_payouts(&pic, backend, owner).items.len(), 1, "ACK preserves the held liability");
}

#[test]
fn sec19_typed_no_effect_ack_retires_operation_without_a_pull() {
    let (pic, backend, ledger, owner, vault_id, topup) = sec19_open_and_fund();
    let args = sec19_topup_args(vault_id, topup);
    let blocks = block_count(&pic, ledger);
    let balance_before = balance(&pic, ledger, owner);
    let vault_before = sec19_vaults(&pic, backend, owner).into_iter()
        .find(|vault| vault.vault_id == vault_id).unwrap().collateral_amount;
    call(&pic, ledger, Principal::anonymous(), "set_bad_fee_failures", encode_one(1u32).unwrap());
    let failed: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args.clone()), "add_margin_to_vault_v2");
    assert!(matches!(failed, Err(ProtocolError::TransferFromError(_, _))));
    assert!(matches!(status(&pic, backend, owner).active.unwrap().phase, VaultCollateralPullPhase::SafeNoEffect { .. }));
    assert_eq!(block_count(&pic, ledger), blocks);
    assert_eq!(balance(&pic, ledger, owner), balance_before);
    let ack: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "ack_vault_operation",
        encode_one(VaultOperationIdArg { operation_id: 2 }).unwrap()), "ack_vault_operation");
    assert_eq!(ack.unwrap(), 2);
    assert_eq!(status(&pic, backend, owner).acknowledged_through, 2);
    let stale: Result<u64, ProtocolError> = reply(call(&pic, backend, owner, "add_margin_to_vault_v2", args), "add_margin_to_vault_v2");
    assert!(stale.is_err());
    assert_eq!(block_count(&pic, ledger), blocks);
    assert_eq!(sec19_vaults(&pic, backend, owner).into_iter()
        .find(|vault| vault.vault_id == vault_id).unwrap().collateral_amount, vault_before);
}
