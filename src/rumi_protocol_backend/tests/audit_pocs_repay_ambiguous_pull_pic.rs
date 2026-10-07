//! PocketIC regression probe for an ambiguous V2 repayment pull.
//!
//! The flaky ledger control commits `icrc2_transfer_from` and then returns a
//! `GenericError` to simulate a lost reply. This is a post-commit ledger error
//! simulation, not a true IC callback trap. An exact same-ID replay must
//! prove the original burn and retire debt once without a second source debit.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    vault::{CandidVault, VaultArg},
    InitArg, ProtocolArg, ProtocolError, UpgradeArg,
};
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(CandidType, Deserialize, Clone)]
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
        .expect("update call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let bytes = pic
        .update_call(
            ledger,
            Principal::anonymous(),
            "mint",
            encode_args((account(owner), Nat::from(amount))).unwrap(),
        )
        .unwrap();
    match bytes {
        WasmResult::Reply(bytes) => {
            let _: () = decode_one(&bytes).expect("mint result");
        }
        WasmResult::Reject(message) => panic!("mint rejected: {message}"),
    }
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

fn vaults(pic: &PocketIc, backend: Principal, owner: Principal) -> Vec<CandidVault> {
    match pic
        .query_call(
            backend,
            owner,
            "get_vaults",
            encode_one(Some(owner)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode vaults"),
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    }
}

#[test]
fn v2_repay_same_id_proves_ambiguous_burn_without_second_debit() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let developer = Principal::self_authenticating(b"repay-ambiguity-developer");
    let user = Principal::self_authenticating(b"repay-ambiguity-user");

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
    let collateral_ledger = pic.create_canister();
    pic.add_cycles(collateral_ledger, 2_000_000_000_000);
    pic.install_canister(
        collateral_ledger,
        artifact("flaky_ledger.wasm"),
        encode_one(()).unwrap(),
        None,
    );
    let _: () = decode_one(&call_update(
        &pic,
        collateral_ledger,
        Principal::anonymous(),
        "set_fee",
        Nat::from(fee),
    ))
    .unwrap();
    mint(&pic, collateral_ledger, user, 10_000_000_000);

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
        icp_ledger_principal: collateral_ledger,
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
    let _: () = decode_one(&call_update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_minting_account",
        Some(account(backend)),
    ))
    .expect("configure backend minting account");
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    let collateral = 5_000_000_000u64;
    let borrowed = 2_000_000_000u64;
    let approval: Result<Nat, ApproveError> = decode_one(&call_update(
        &pic,
        collateral_ledger,
        user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(collateral + borrowed + (fee as u64 * 3)),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode collateral approval");
    approval.expect("approve collateral ingress");

    // Open through the durable collateral ingress, then borrow icUSD from the
    // separate stable ledger that names the backend as its minting account.
    let opened_bytes = match pic
        .update_call(
            backend,
            user,
            "open_vault_v2",
            candid::encode_args((1u128, collateral, None::<Principal>)).unwrap(),
        )
        .expect("open_vault_v2 update")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("open_vault_v2 rejected: {message}"),
    };
    let opened: Result<rumi_protocol_backend::InboundCollateralStatusView, ProtocolError> =
        decode_one(&opened_bytes).expect("decode open_vault_v2");
    let opened = opened.expect("open vault");
    let vault_id = match opened.result.expect("open result") {
        rumi_protocol_backend::InboundCollateralResultView::Open { vault_id, .. } => vault_id,
        other => panic!("unexpected open result: {other:?}"),
    };

    let borrow: Result<rumi_protocol_backend::SuccessWithFee, ProtocolError> =
        decode_one(&call_update(
            &pic,
            backend,
            user,
            "borrow_from_vault",
            VaultArg {
                vault_id,
                amount: borrowed,
            },
        ))
        .expect("decode borrow result");
    borrow.expect("borrow against newly opened vault");
    let initial_debt = vaults(&pic, backend, user)[0].borrowed_icusd_amount;
    assert!(
        initial_debt >= borrowed,
        "borrow must establish sufficient debt"
    );

    let repay_amount = 1_000_000_000u64;
    let repay_approval: Result<Nat, ApproveError> = decode_one(&call_update(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(repay_amount),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode repayment approval");
    repay_approval.expect("approve repayment pulls");

    let user_before = balance(&pic, ledger, user);

    let legacy_repay: Result<u64, ProtocolError> = decode_one(&call_update(
        &pic,
        backend,
        user,
        "repay_to_vault",
        VaultArg {
            vault_id,
            amount: repay_amount,
        },
    ))
    .expect("decode legacy repayment gate");
    assert!(
        legacy_repay.is_err(),
        "legacy no-ID repayment must be gated"
    );
    let legacy_close: Result<rumi_protocol_backend::vault::RepayAndCloseSuccess, ProtocolError> =
        decode_one(&call_update(
            &pic,
            backend,
            user,
            "repay_and_close_vault",
            VaultArg {
                vault_id,
                amount: repay_amount,
            },
        ))
        .expect("decode legacy close gate");
    assert!(legacy_close.is_err(), "legacy no-ID close must be gated");
    let legacy_partial: Result<u64, ProtocolError> = decode_one(&call_update(
        &pic,
        backend,
        user,
        "partial_repay_to_vault",
        VaultArg {
            vault_id,
            amount: repay_amount,
        },
    ))
    .expect("decode legacy partial repayment gate");
    assert!(
        legacy_partial.is_err(),
        "legacy partial repayment must be gated"
    );
    assert_eq!(
        balance(&pic, ledger, user),
        user_before,
        "legacy gates must not pull icUSD"
    );
    assert_eq!(
        vaults(&pic, backend, user)[0].borrowed_icusd_amount,
        initial_debt
    );

    let _: () = decode_one(&call_update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        1u32,
    ))
    .unwrap();

    let first_bytes = match pic
        .update_call(
            backend,
            user,
            "repay_to_vault_v2",
            encode_args((
                1u128,
                VaultArg {
                    vault_id,
                    amount: repay_amount,
                },
            ))
            .unwrap(),
        )
        .expect("first repayment call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("first repayment rejected: {message}"),
    };
    let first: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&first_bytes).expect("decode ambiguous repayment response");
    assert!(
        first
            .as_ref()
            .is_ok_and(|view| view.phase == rumi_protocol_backend::RepaymentV2Phase::HeldPull),
        "post-commit error should leave an ambiguous held request; got {first:?}"
    );
    assert_eq!(
        balance(&pic, ledger, user),
        user_before - repay_amount as u128,
        "burn to minting account is fee-free"
    );
    assert_eq!(
        vaults(&pic, backend, user)[0].borrowed_icusd_amount,
        initial_debt,
        "debt remains unchanged until the burn is positively proved"
    );

    // The ledger has committed the pull, but the backend has only persisted
    // an ambiguous held request. Upgrade before any reconciliation so the
    // retry below must recover from stable state rather than heap state.
    let upgrade = ProtocolArg::Upgrade(UpgradeArg {
        mode: None,
        description: Some("ambiguous repayment recovery regression".into()),
    });
    pic.upgrade_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_args((upgrade,)).expect("encode backend upgrade arg"),
        None,
    )
    .expect("upgrade backend with ambiguous repayment held");
    assert_eq!(
        balance(&pic, ledger, user),
        user_before - repay_amount as u128,
        "upgrade must not change the already-committed ledger pull"
    );
    assert_eq!(
        vaults(&pic, backend, user)[0].borrowed_icusd_amount,
        initial_debt,
        "upgrade must preserve debt until the committed burn is reconciled"
    );
    let after_upgrade_bytes = match pic
        .query_call(
            backend,
            user,
            "get_my_repayment_v2_request_state",
            encode_args(()).unwrap(),
        )
        .expect("held repayment query after upgrade")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("held repayment query rejected: {message}"),
    };
    let after_upgrade: Result<rumi_protocol_backend::RepaymentV2RequestState, ProtocolError> =
        decode_one(&after_upgrade_bytes).expect("decode held repayment after upgrade");
    assert_eq!(
        after_upgrade
            .unwrap()
            .active_request
            .expect("active request survives upgrade")
            .phase,
        rumi_protocol_backend::RepaymentV2Phase::HeldPull,
        "upgrade must retain the unresolved request in HeldPull"
    );

    let wrong_candidate_bytes = match pic
        .update_call(
            backend,
            user,
            "attach_my_repayment_v2_candidate",
            encode_args((1u128, u64::MAX)).unwrap(),
        )
        .expect("wrong candidate attachment")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("candidate call rejected: {message}"),
    };
    let wrong_candidate: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&wrong_candidate_bytes).expect("decode wrong candidate result");
    assert!(
        wrong_candidate.is_err(),
        "wrong block must not become a receipt candidate"
    );
    let active_bytes = match pic
        .query_call(
            backend,
            user,
            "get_my_repayment_v2_request_state",
            encode_args(()).unwrap(),
        )
        .expect("active repayment query")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("active repayment query rejected: {message}"),
    };
    let active: Result<rumi_protocol_backend::RepaymentV2RequestState, ProtocolError> =
        decode_one(&active_bytes).expect("decode active repayment state");
    assert_eq!(
        active.unwrap().active_request.unwrap().phase,
        rumi_protocol_backend::RepaymentV2Phase::HeldPull,
        "failed candidate attachment leaves the ambiguous row held"
    );

    let different = pic
        .update_call(
            backend,
            user,
            "repay_to_vault_v2",
            encode_args((
                2u128,
                VaultArg {
                    vault_id,
                    amount: repay_amount,
                },
            ))
            .unwrap(),
        )
        .expect("different-ID repayment call");
    let different_bytes = match different {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("different-ID call rejected: {message}"),
    };
    let different: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&different_bytes).expect("decode different-ID result");
    assert!(
        different.is_err(),
        "a different ID must not advance past the held request"
    );

    let retry_bytes = match pic
        .update_call(
            backend,
            user,
            "repay_to_vault_v2",
            encode_args((
                1u128,
                VaultArg {
                    vault_id,
                    amount: repay_amount,
                },
            ))
            .unwrap(),
        )
        .expect("exact same-ID retry")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("exact retry rejected: {message}"),
    };
    let retry: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&retry_bytes).expect("decode exact retry");
    assert_eq!(
        retry.unwrap().phase,
        rumi_protocol_backend::RepaymentV2Phase::Complete
    );

    assert_eq!(
        balance(&pic, ledger, user),
        user_before - repay_amount as u128,
        "exact retry must not create a second burn"
    );
    assert_eq!(
        vaults(&pic, backend, user)[0].borrowed_icusd_amount,
        initial_debt - repay_amount,
        "one verified burn retires debt exactly once"
    );

    // A full repay-and-close records the exact stable burn before beginning
    // the collateral payout. Force the first payout to return BadFee, then
    // replay the same request ID after the withdrawal outbox has safely
    // re-pinned its fee. The replay must resume only the close phase.
    let debt_before_close = vaults(&pic, backend, user)[0].borrowed_icusd_amount;
    let close_repay = debt_before_close;
    mint(&pic, ledger, user, close_repay as u128 + fee);
    let close_approval: Result<Nat, ApproveError> = decode_one(&call_update(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(close_repay),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode close repayment approval");
    close_approval.expect("approve close repayment");
    let stable_before_close = balance(&pic, ledger, user);
    let _: () = decode_one(&call_update(
        &pic,
        collateral_ledger,
        Principal::anonymous(),
        "set_fee",
        Nat::from(fee * 2),
    ))
    .unwrap();

    let close_first_bytes = match pic
        .update_call(
            backend,
            user,
            "repay_and_close_vault_v2",
            encode_args((
                2u128,
                VaultArg {
                    vault_id,
                    amount: close_repay,
                },
            ))
            .unwrap(),
        )
        .expect("first close request")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("close request rejected: {message}"),
    };
    let close_first: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&close_first_bytes).expect("decode close pending status");
    assert_eq!(
        close_first.unwrap().phase,
        rumi_protocol_backend::RepaymentV2Phase::ClosePending,
        "a first typed payout BadFee must leave a receipt-backed close pending"
    );
    assert_eq!(
        balance(&pic, ledger, user),
        stable_before_close - close_repay as u128,
        "stable principal is burned once before the close payout retries"
    );

    let payout_retry: Result<u64, ProtocolError> = decode_one(&call_update(
        &pic,
        backend,
        user,
        "retry_pending_collateral_withdrawal",
        vault_id,
    ))
    .expect("decode standalone collateral retry");
    payout_retry.expect("standalone route settles the pinned close payout");

    let close_status_bytes = match pic
        .query_call(
            backend,
            user,
            "get_my_repayment_v2_status",
            encode_one(2u128).unwrap(),
        )
        .expect("query completed repayment close")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("repayment status rejected: {message}"),
    };
    let close_status: Result<Option<rumi_protocol_backend::RepaymentV2StatusView>, ProtocolError> =
        decode_one(&close_status_bytes).expect("decode completed repayment status");
    let close_status = close_status
        .unwrap()
        .expect("completed close result retained");
    assert_eq!(
        close_status.phase,
        rumi_protocol_backend::RepaymentV2Phase::Complete
    );
    assert!(close_status
        .result
        .unwrap()
        .collateral_return_block_index
        .is_some());

    let close_replay_bytes = match pic
        .update_call(
            backend,
            user,
            "repay_and_close_vault_v2",
            encode_args((
                2u128,
                VaultArg {
                    vault_id,
                    amount: close_repay,
                },
            ))
            .unwrap(),
        )
        .expect("same-ID close result replay")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("same-ID close replay rejected: {message}"),
    };
    let close_replay: Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError> =
        decode_one(&close_replay_bytes).expect("decode close result replay");
    assert_eq!(
        close_replay.unwrap().phase,
        rumi_protocol_backend::RepaymentV2Phase::Complete
    );
    assert!(
        vaults(&pic, backend, user).is_empty(),
        "standalone withdrawal retry removes the vault"
    );
    assert_eq!(
        balance(&pic, ledger, user),
        stable_before_close - close_repay as u128,
        "payout recovery and same-ID close result replay perform no second stable burn"
    );
}
