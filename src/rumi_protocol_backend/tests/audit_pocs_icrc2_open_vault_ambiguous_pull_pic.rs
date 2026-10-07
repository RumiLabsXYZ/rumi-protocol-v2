//! End-to-end regression probe for ordinary ICRC-2 collateral ingress.
//!
//! The flaky ledger can return a typed `BadFee` before any debit, or commit an
//! ICRC-2 transfer and then return `GenericError` (its reply-loss simulation).
//! This test exercises both through the real backend `open_vault` endpoint,
//! then retries as a user and checks ledger balances against vault accounting.
//! It does not simulate a true IC callback trap: see the test's assertions and
//! handoff notes for that distinction.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    vault::CandidVault, InboundCollateralStatusView, InitArg, ProtocolArg, ProtocolError,
    UpgradeArg,
};
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType)]
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

#[derive(CandidType)]
struct PushTransferArgs {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize)]
struct MockXrc {
    rates: Vec<(String, u64)>,
}

fn artifact(name: &str) -> Vec<u8> {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    fs::read(target.join("wasm32-unknown-unknown/release").join(name))
        .unwrap_or_else(|e| panic!("read {} from CARGO_TARGET_DIR: {e}", name))
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn call_update<T: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: T,
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("update call {method}: {error:?}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn call_update_args<T: candid::utils::ArgumentEncoder>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: T,
) -> Vec<u8> {
    match pic.update_call(canister, caller, method, encode_args(args).unwrap())
        .unwrap_or_else(|error| panic!("update call {method}: {error:?}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let bytes = match pic
        .update_call(
            ledger,
            Principal::anonymous(),
            "mint",
            encode_args((account(owner), Nat::from(amount))).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("mint rejected: {message}"),
    };
    let _: () = decode_one(&bytes).expect("mint result");
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let bytes = match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    };
    decode_one::<Nat>(&bytes).unwrap().0.try_into().unwrap()
}

fn balance_account(pic: &PocketIc, ledger: Principal, account: Account) -> u128 {
    let bytes = match pic.query_call(ledger, Principal::anonymous(), "icrc1_balance_of", encode_one(account).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    };
    decode_one::<Nat>(&bytes).unwrap().0.try_into().unwrap()
}

fn open_vault(
    pic: &PocketIc,
    backend: Principal,
    caller: Principal,
    request_id: u128,
    amount: u64,
) -> Result<InboundCollateralStatusView, ProtocolError> {
    let bytes = match pic
        .update_call(
            backend,
            caller,
            "open_vault_v2",
            encode_args((request_id, amount, None::<Principal>)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => {
            panic!("backend open_vault rejected at call boundary: {message}")
        }
    };
    decode_one(&bytes).expect("decode open_vault_v2 result")
}

#[test]
fn open_vault_reconciles_committed_reply_loss_without_second_debit() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let developer = Principal::self_authenticating(b"ingress-test-developer");
    let user = Principal::self_authenticating(b"ingress-test-user");

    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    pic.install_canister(
        ledger,
        artifact("flaky_ledger.wasm"),
        encode_one(()).unwrap(),
        None,
    );
    let fee = 10_000u128;
    let _: () = decode_one(&call_update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_fee",
        Nat::from(fee),
    ))
    .unwrap();
    mint(&pic, ledger, user, 1_000_000);

    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(),
        encode_one(MockXrc {
            rates: vec![("ICP/USD".into(), 1_000_000_000)],
        })
        .unwrap(),
        None,
    );
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: 10_000,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_one(init).unwrap(),
        None,
    );
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    let amount = 100_000u64;
    let approval: Result<Nat, ApproveError> = decode_one(&call_update(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(amount * 3 + fee as u64),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("approve result");
    approval.expect("ledger approval");

    let user_before = balance(&pic, ledger, user);
    let backend_before = balance(&pic, ledger, backend);

    // A typed BadFee is a proven no-effect response: no debit or credit lands.
    let _: () = decode_one(&call_update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_bad_fee_failures",
        1u32,
    ))
    .unwrap();
    let request_id = 1u128;
    let typed = open_vault(&pic, backend, user, request_id, amount)
        .expect("typed no-effect should return the durable held status");
    assert!(matches!(
        typed.phase,
        rumi_protocol_backend::InboundCollateralPhase::Held
    ));
    assert_eq!(
        balance(&pic, ledger, user),
        user_before,
        "typed BadFee must not debit the user"
    );
    assert_eq!(
        balance(&pic, ledger, backend),
        backend_before,
        "typed BadFee must not credit the backend"
    );

    // Existing flaky-ledger control commits transfer_from, then returns an
    // error to its caller. This is an ambiguity simulation, not a true trap.
    let _: () = decode_one(&call_update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        1u32,
    ))
    .unwrap();
    let ambiguous = open_vault(&pic, backend, user, request_id, amount)
        .expect("ambiguous pull should return its held status");
    assert!(matches!(
        ambiguous.phase,
        rumi_protocol_backend::InboundCollateralPhase::Held
    ));
    assert_eq!(
        balance(&pic, ledger, user),
        user_before - amount as u128 - fee
    );
    assert_eq!(
        balance(&pic, ledger, backend),
        backend_before + amount as u128
    );
    let pending: Option<rumi_protocol_backend::InboundCollateralStatusView> =
        decode_one(&match pic
            .query_call(
                backend,
                user,
                "get_my_pending_collateral_ingress",
                encode_one(ledger).unwrap(),
            )
            .unwrap()
        {
            WasmResult::Reply(bytes) => bytes,
            WasmResult::Reject(message) => panic!("ingress status rejected: {message}"),
        })
        .expect("pending exact ingress status");
    let pending = pending.expect("ambiguous transfer must retain its durable tuple");
    assert_eq!(pending.amount_raw, amount);
    assert_eq!(pending.ledger, ledger);
    assert!(pending.had_ambiguous_attempt);
    let no_vaults: Vec<CandidVault> = decode_one(&match pic
        .query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap())
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    })
    .unwrap();
    assert!(
        no_vaults.is_empty(),
        "failed ingress must not have created a vault"
    );

    let wrong_candidate_bytes = match pic
        .update_call(
            backend,
            user,
            "attach_my_collateral_ingress_receipt",
            encode_args((ledger, request_id, u64::MAX)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("candidate attachment rejected: {message}"),
    };
    let wrong_candidate: Result<(), ProtocolError> =
        decode_one(&wrong_candidate_bytes).expect("decode wrong-candidate attachment");
    assert!(
        wrong_candidate.is_err(),
        "wrong block must fail exact tuple proof"
    );
    let still_pending: Option<rumi_protocol_backend::InboundCollateralStatusView> =
        decode_one(&match pic
            .query_call(
                backend,
                user,
                "get_my_pending_collateral_ingress",
                encode_one(ledger).unwrap(),
            )
            .unwrap()
        {
            WasmResult::Reply(bytes) => bytes,
            WasmResult::Reject(message) => panic!("ingress status rejected: {message}"),
        })
        .expect("decode unchanged pending row");
    assert_eq!(still_pending.unwrap().candidate_block_index, None);

    // The user retry resumes the same tuple. The ledger returns Duplicate with
    // the original block, exact ICRC-3 proof succeeds, and business credit is
    // committed once.
    let retried = open_vault(&pic, backend, user, request_id, amount)
        .expect("retry should reconcile the vault");
    let (vault_id, block_index) = match retried.result.as_ref().expect("open result") {
        rumi_protocol_backend::InboundCollateralResultView::Open {
            vault_id,
            block_index,
        } => (*vault_id, *block_index),
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(vault_id, 1);
    assert_eq!(
        balance(&pic, ledger, user),
        user_before - amount as u128 - fee
    );
    assert_eq!(
        balance(&pic, ledger, backend),
        backend_before + amount as u128
    );
    let vaults: Vec<CandidVault> = decode_one(&match pic
        .query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap())
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    })
    .unwrap();
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0].collateral_amount, amount);
    assert_eq!(
        balance(&pic, ledger, backend),
        backend_before + amount as u128,
        "the committed pull is credited once"
    );

    // Simulate a lost successful backend response by discarding the exact
    // response and replaying the same stable request ID. It must return the
    // retained vault/block result without another ledger debit.
    let user_after_first = balance(&pic, ledger, user);
    let replay =
        open_vault(&pic, backend, user, request_id, amount).expect("completed request replay");
    match replay.result.expect("saved open result") {
        rumi_protocol_backend::InboundCollateralResultView::Open {
            vault_id: replay_id,
            block_index: replay_block,
        } => {
            assert_eq!(replay_id, vault_id);
            assert_eq!(replay_block, block_index);
        }
        other => panic!("unexpected replay result: {other:?}"),
    }
    assert_eq!(
        balance(&pic, ledger, user),
        user_after_first,
        "same ID replay must not pull again"
    );

    // The same ID with changed payload is rejected. A new sequence ID may
    // intentionally repeat the same economic intent and creates one new vault.
    assert!(matches!(
        open_vault(&pic, backend, user, request_id, amount + 1),
        Err(ProtocolError::GenericError(_))
    ));
    let second = open_vault(&pic, backend, user, request_id + 1, amount)
        .expect("new ID allows a fresh equal open");
    assert!(matches!(
        second.result,
        Some(rumi_protocol_backend::InboundCollateralResultView::Open { vault_id: 2, .. })
    ));
    assert_eq!(
        balance(&pic, ledger, user),
        user_after_first - amount as u128 - fee
    );
    assert!(
        matches!(
            open_vault(&pic, backend, user, request_id, amount),
            Err(ProtocolError::GenericError(_))
        ),
        "compacted older ID must be rejected safely"
    );
}

#[test]
fn push_deposit_sweep_recovers_committed_reply_loss_and_terminal_replay_once() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let developer = Principal::self_authenticating(b"push-sweep-developer");
    let user = Principal::self_authenticating(b"push-sweep-user");
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    pic.install_canister(ledger, artifact("flaky_ledger.wasm"), encode_one(()).unwrap(), None);
    let fee = 10_000u64;
    let _: () = decode_one(&call_update(&pic, ledger, Principal::anonymous(), "set_fee", Nat::from(fee))).unwrap();

    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(xrc, include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(), encode_one(MockXrc {
        rates: vec![("ICP/USD".into(), 1_000_000_000)],
    }).unwrap(), None);
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: fee,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(backend, artifact("rumi_protocol_backend.wasm"), encode_one(init).unwrap(), None);
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 { pic.tick(); }

    let gross_deposit = 1_000_000u64;
    mint(&pic, ledger, user, (2 * (gross_deposit + fee)) as u128);
    let deposit_account: Account = decode_one(&match pic.query_call(
        backend, user, "get_deposit_account", encode_one(None::<Principal>).unwrap(),
    ).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("deposit account query rejected: {message}"),
    }).unwrap();
    let deposit: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = decode_one(&call_update(
        &pic, ledger, user, "icrc1_transfer", PushTransferArgs {
            from_subaccount: None,
            to: deposit_account.clone(),
            amount: Nat::from(gross_deposit),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    )).unwrap();
    deposit.expect("fund caller deposit account");
    assert_eq!(balance_account(&pic, ledger, deposit_account.clone()), gross_deposit as u128);

    // The ledger commits the subaccount sweep and returns a typed GenericError.
    let _: () = decode_one(&call_update(&pic, ledger, Principal::anonymous(), "set_phantom_failures", 1u32)).unwrap();
    let first: Result<rumi_protocol_backend::vault::OpenVaultSuccess, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "open_vault_with_deposit_v2", (0u64, None::<Principal>, 1u128),
    )).unwrap();
    assert!(matches!(first, Err(ProtocolError::TemporarilyUnavailable(_))));
    assert_eq!(balance_account(&pic, ledger, deposit_account.clone()), 0);
    let vaults: Vec<CandidVault> = decode_one(&match pic.query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }).unwrap();
    assert!(vaults.is_empty(), "ambiguous transfer must not credit before exact receipt proof");

    // The exact tuple and held phase survive an upgrade before reconciliation.
    let upgrade = ProtocolArg::Upgrade(UpgradeArg {
        mode: None,
        description: Some("push-deposit ambiguous sweep recovery regression".into()),
    });
    pic.upgrade_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_args((upgrade,)).expect("encode backend upgrade arg"),
        None,
    ).expect("upgrade backend with ambiguous push sweep held");

    let recovered: Result<rumi_protocol_backend::state::PushDepositSweepResult, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "recover_my_push_deposit_sweep", (ledger, 1u128),
    )).unwrap();
    assert!(matches!(recovered, Ok(rumi_protocol_backend::state::PushDepositSweepResult::Open { vault_id: 1, .. })));
    let vaults: Vec<CandidVault> = decode_one(&match pic.query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }).unwrap();
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0].collateral_amount, gross_deposit - fee);

    // Replaying the terminal request ID returns the original vault and cannot
    // sweep or credit another deposit.
    let replay: Result<rumi_protocol_backend::vault::OpenVaultSuccess, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "open_vault_with_deposit_v2", (0u64, None::<Principal>, 1u128),
    )).unwrap();
    assert!(matches!(replay, Ok(result) if result.vault_id == 1));
    let vaults: Vec<CandidVault> = decode_one(&match pic.query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }).unwrap();
    assert_eq!(vaults.len(), 1, "terminal replay must preserve one credit");

    let _: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = decode_one(&call_update(
        &pic, ledger, user, "icrc1_transfer", PushTransferArgs {
            from_subaccount: None,
            to: deposit_account.clone(),
            amount: Nat::from(gross_deposit),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    )).unwrap();
    let _: () = decode_one(&call_update(&pic, ledger, Principal::anonymous(), "set_phantom_failures", 1u32)).unwrap();
    let add: Result<u64, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "add_margin_with_deposit_v2", (1u64, 2u128),
    )).unwrap();
    assert!(matches!(add, Err(ProtocolError::TemporarilyUnavailable(_))));
    let competing_add: Result<u64, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "add_margin_with_deposit_v2", (1u64, 3u128),
    )).unwrap();
    assert!(matches!(competing_add, Err(ProtocolError::TemporarilyUnavailable(_))));
    // Same-ID V2 retry must pass the durable pending-margin fence and resume
    // the exact journaled tuple; a different ID remains fenced.
    let retry_add: Result<u64, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "add_margin_with_deposit_v2", (1u64, 2u128),
    )).unwrap();
    assert!(retry_add.is_ok(), "same-ID AddMargin retry should resume its pending sweep: {retry_add:?}");
    let vaults: Vec<CandidVault> = decode_one(&match pic.query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }).unwrap();
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0].collateral_amount, 2 * (gross_deposit - fee));
    let replay_margin: Result<u64, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "add_margin_with_deposit_v2", (1u64, 2u128),
    )).unwrap();
    assert!(replay_margin.is_ok());
    let vaults: Vec<CandidVault> = decode_one(&match pic.query_call(backend, user, "get_vaults", encode_one(Some(user)).unwrap()).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }).unwrap();
    assert_eq!(vaults[0].collateral_amount, 2 * (gross_deposit - fee), "margin request replay must not credit twice");

    // A stale configured fee is supplied explicitly in the pinned tuple. The
    // ledger must reject with typed BadFee before debit; it cannot commit at a
    // different fee and strand an unverifiable receipt.
    mint(&pic, ledger, user, (gross_deposit + fee) as u128);
    let _: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = decode_one(&call_update(
        &pic, ledger, user, "icrc1_transfer", PushTransferArgs {
            from_subaccount: None,
            to: deposit_account.clone(),
            amount: Nat::from(gross_deposit),
            fee: None,
            memo: None,
            created_at_time: None,
        },
    )).unwrap();
    let _: () = decode_one(&call_update(&pic, ledger, Principal::anonymous(), "set_fee", Nat::from(fee + 1))).unwrap();
    let stale_fee: Result<rumi_protocol_backend::vault::OpenVaultSuccess, ProtocolError> = decode_one(&call_update_args(
        &pic, backend, user, "open_vault_with_deposit_v2", (0u64, None::<Principal>, 3u128),
    )).unwrap();
    assert!(matches!(
        stale_fee,
        Err(ProtocolError::TransferError(
            icrc_ledger_types::icrc1::transfer::TransferError::BadFee { .. }
        ))
    ), "fee drift should be a typed no-effect BadFee: {stale_fee:?}");
    assert_eq!(balance_account(&pic, ledger, deposit_account), gross_deposit as u128, "BadFee must not debit the user's deposit");
}
