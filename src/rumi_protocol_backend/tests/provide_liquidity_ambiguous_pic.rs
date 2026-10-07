//! Bounded reproduction of the legacy `provide_liquidity` lost-reply gap.
//!
//! The test-only `flaky_ledger` commits ICRC-2 `transfer_from`, then returns
//! `GenericError` once. The backend therefore returns an error without
//! crediting liquidity. A user retry obtains a fresh op nonce, so the ledger
//! sees a distinct dedup tuple and debits the user a second time while the
//! backend credits liquidity once.
//!
//! Mock semantics matter: this is a Candid `GenericError` returned by the
//! ledger after its in-memory balance/allowance/block mutations. It is not a
//! PocketIC transport reject or a production ledger implementation. The mock
//! has zero fee and is configured with the backend as its minting account:
//! user-to-backend `transfer_from` burns, while backend-to-user transfer mints.
//! Both leave the backend ledger balance at zero.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::{icrc1::transfer::TransferError, icrc2::transfer_from::TransferFromError};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{InitArg, ProtocolArg, ProtocolError};
use std::{env, fs, path::PathBuf};

mod mock_xrc_canister;

#[derive(CandidType, Deserialize, Clone, Debug)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidityStatus {
    liquidity_provided: u64,
    total_liquidity_provided: u64,
    liquidity_pool_share: f64,
    available_liquidity_reward: u64,
    total_available_returns: u64,
}

fn artifact(name: &str) -> Vec<u8> {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    fs::read(target.join("wasm32-unknown-unknown/release").join(name))
        .unwrap_or_else(|error| panic!("read {name} from CARGO_TARGET_DIR: {error}"))
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn deploy_flaky_ledger(pic: &PocketIc) -> Principal {
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    pic.install_canister(
        ledger,
        artifact("flaky_ledger.wasm"),
        encode_one(()).unwrap(),
        None,
    );
    ledger
}

fn deploy_mock_xrc(pic: &PocketIc) -> Principal {
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 2_000_000_000_000);
    pic.install_canister(
        xrc,
        fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../xrc_demo/xrc/xrc.wasm"))
            .expect("read mock XRC Wasm"),
        mock_xrc_canister::prepare_mock_xrc(),
        None,
    );
    xrc
}

fn set_minting_account(pic: &PocketIc, ledger: Principal, backend: Principal) {
    let result = pic
        .update_call(
            ledger,
            Principal::anonymous(),
            "set_minting_account",
            encode_one(Some(account(backend))).unwrap(),
        )
        .expect("set minting account failed");
    match result {
        WasmResult::Reply(bytes) => {
            let _: () = decode_one(&bytes).expect("decode set minting account");
        }
        WasmResult::Reject(message) => panic!("set minting account rejected: {message}"),
    }
}

fn mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "mint",
        encode_args((account(owner), Nat::from(amount))).unwrap(),
    )
    .expect("mint call failed");
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .expect("balance query failed");
    match result {
        WasmResult::Reply(bytes) => decode_one::<Nat>(&bytes)
            .expect("decode balance")
            .0
            .try_into()
            .expect("balance fits u128"),
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    }
}

fn approve(pic: &PocketIc, ledger: Principal, user: Principal, backend: Principal, amount: u128) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: account(backend),
        amount: Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(ledger, user, "icrc2_approve", encode_one(args).unwrap())
        .expect("approve call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let approval: Result<Nat, ApproveError> =
                decode_one(&bytes).expect("decode approval result");
            assert!(approval.is_ok(), "approval failed: {approval:?}");
        }
        WasmResult::Reject(message) => panic!("approve rejected: {message}"),
    }
}

fn set_phantom_failures(pic: &PocketIc, ledger: Principal, count: u32) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        encode_one(count).unwrap(),
    )
    .expect("set phantom failures failed");
}

fn liquidity_status(pic: &PocketIc, backend: Principal, owner: Principal) -> LiquidityStatus {
    let result = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_liquidity_status",
            encode_one(owner).unwrap(),
        )
        .expect("liquidity status query failed");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode liquidity status"),
        WasmResult::Reject(message) => panic!("liquidity status rejected: {message}"),
    }
}

#[test]
fn legacy_provide_liquidity_retry_after_committed_ambiguous_pull_debits_twice() {
    const LIQUIDITY: u128 = 1_000_000_000;
    const STARTING_BALANCE: u128 = 2 * LIQUIDITY;

    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let user = Principal::self_authenticating(b"provide-liquidity-ambiguous-user");
    let developer = Principal::self_authenticating(b"provide-liquidity-ambiguous-dev");
    let ledger = deploy_flaky_ledger(&pic);
    let xrc = deploy_mock_xrc(&pic);
    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: 0,
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
    set_minting_account(&pic, ledger, backend);

    mint(&pic, ledger, user, STARTING_BALANCE);
    approve(&pic, ledger, user, backend, STARTING_BALANCE);
    assert_eq!(balance(&pic, ledger, user), STARTING_BALANCE);
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);

    // The mock commits the pull before surfacing GenericError to the backend.
    set_phantom_failures(&pic, ledger, 1);
    let first = pic
        .update_call(
            backend,
            user,
            "provide_liquidity",
            encode_one(LIQUIDITY as u64).unwrap(),
        )
        .expect("first provide_liquidity call failed at transport layer");
    let first_error: Result<u64, ProtocolError> = match first {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode first result"),
        WasmResult::Reject(message) => panic!("first call unexpectedly rejected: {message}"),
    };
    assert!(
        matches!(&first_error, Err(ProtocolError::TransferFromError(TransferFromError::GenericError { message, .. }, _)) if message.contains("phantom failure")),
        "committed-then-GenericError should surface to caller: {first_error:?}"
    );
    eprintln!("first provide_liquidity result: {first_error:?}");
    assert_eq!(balance(&pic, ledger, user), STARTING_BALANCE - LIQUIDITY);
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(
        liquidity_status(&pic, backend, user).liquidity_provided,
        0,
        "the failed call must not book a liquidity credit"
    );

    // Retrying the legacy endpoint allocates a fresh op nonce. The flaky
    // ledger's dedup key therefore differs and this is a second real debit.
    let retry = pic
        .update_call(
            backend,
            user,
            "provide_liquidity",
            encode_one(LIQUIDITY as u64).unwrap(),
        )
        .expect("retry provide_liquidity call failed at transport layer");
    let retry_result: Result<u64, ProtocolError> = match retry {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode retry result"),
        WasmResult::Reject(message) => panic!("retry unexpectedly rejected: {message}"),
    };
    assert!(
        retry_result.is_ok(),
        "retry should return the second pull block"
    );
    assert_eq!(balance(&pic, ledger, user), 0);
    assert_eq!(balance(&pic, ledger, backend), 0);
    let credited = liquidity_status(&pic, backend, user);
    assert_eq!(credited.liquidity_provided, LIQUIDITY as u64);
    assert_eq!(credited.total_liquidity_provided, LIQUIDITY as u64);
}

/// The sibling legacy withdrawal path has the inverse ordering: a committed
/// mint can reach the user before the liquidity debit is recorded. Replaying
/// after the ambiguous result can therefore pay twice while debiting the LP
/// position once.
#[test]
fn legacy_withdraw_liquidity_retry_after_committed_ambiguous_mint_pays_twice() {
    const LIQUIDITY: u128 = 1_000_000_000;

    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let user = Principal::self_authenticating(b"withdraw-liquidity-ambiguous-user");
    let developer = Principal::self_authenticating(b"withdraw-liquidity-ambiguous-dev");
    let ledger = deploy_flaky_ledger(&pic);
    let xrc = deploy_mock_xrc(&pic);
    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: 0,
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
    set_minting_account(&pic, ledger, backend);

    mint(&pic, ledger, user, LIQUIDITY);
    approve(&pic, ledger, user, backend, LIQUIDITY);
    let deposit = pic
        .update_call(
            backend,
            user,
            "provide_liquidity",
            encode_one(LIQUIDITY as u64).unwrap(),
        )
        .expect("deposit call failed at transport layer");
    let deposit_result: Result<u64, ProtocolError> = match deposit {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode deposit result"),
        WasmResult::Reject(message) => panic!("deposit unexpectedly rejected: {message}"),
    };
    assert!(deposit_result.is_ok(), "deposit result: {deposit_result:?}");
    assert_eq!(
        liquidity_status(&pic, backend, user).liquidity_provided,
        LIQUIDITY as u64
    );
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(balance(&pic, ledger, user), 0);

    // The ICRC-1 mint commits to the user's account before the mock returns
    // GenericError. The legacy endpoint leaves the pool position untouched.
    set_phantom_failures(&pic, ledger, 1);
    let first = pic
        .update_call(
            backend,
            user,
            "withdraw_liquidity",
            encode_one(LIQUIDITY as u64).unwrap(),
        )
        .expect("first withdrawal failed at transport layer");
    let first_result: Result<u64, ProtocolError> = match first {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode first withdrawal"),
        WasmResult::Reject(message) => panic!("first withdrawal rejected: {message}"),
    };
    assert!(
        matches!(&first_result, Err(ProtocolError::TransferError(TransferError::GenericError { message, .. })) if message.contains("phantom failure")),
        "committed-then-GenericError mint should surface to caller: {first_result:?}"
    );
    assert_eq!(balance(&pic, ledger, user), LIQUIDITY);
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(
        liquidity_status(&pic, backend, user).liquidity_provided,
        LIQUIDITY as u64
    );

    let retry = pic
        .update_call(
            backend,
            user,
            "withdraw_liquidity",
            encode_one(LIQUIDITY as u64).unwrap(),
        )
        .expect("retry withdrawal failed at transport layer");
    let retry_result: Result<u64, ProtocolError> = match retry {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode retry withdrawal"),
        WasmResult::Reject(message) => panic!("retry withdrawal rejected: {message}"),
    };
    assert!(retry_result.is_ok());
    assert_eq!(balance(&pic, ledger, user), 2 * LIQUIDITY);
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);
}
