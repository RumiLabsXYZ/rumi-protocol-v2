//! Green PocketIC integration coverage for the Liquidity V2 receipt journal.
//!
//! `flaky_ledger` deliberately commits the transfer and then returns a
//! `GenericError`. This is a ledger-level post-commit error simulation, not a
//! PocketIC transport reject or a true callback trap. The test configures the
//! backend as minting account: Provide burns user funds (backend balance stays
//! zero), while Withdraw mints directly to the user. Provide first holds while
//! ICRC-3 is unavailable, then exact history proof settles it after upgrade;
//! Withdraw demonstrates immediate recovery when history is available.
//!
//! ClaimReturns is not covered here: this fixture does not have a public,
//! deterministic way to accrue a user's native-ICP liquidity rewards.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    InitArg, LiquidityV2Kind, LiquidityV2Phase, LiquidityV2RequestState, LiquidityV2StatusView,
    ProtocolArg, ProtocolError, UpgradeArg,
};
use std::{env, fs, path::PathBuf};

const LIQUIDITY: u64 = 1_000_000_000;
const STARTING_BALANCE: u128 = 2 * LIQUIDITY as u128;

#[derive(CandidType, Deserialize, Clone, Debug)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
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

#[derive(CandidType, Deserialize)]
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

fn update<T: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: T,
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} update failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn update_pair<A: CandidType, B: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: (A, B),
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_args(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} update failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let _: () = decode_one(&update_pair(
        pic,
        ledger,
        Principal::anonymous(),
        "mint",
        (account(owner), Nat::from(amount)),
    ))
    .expect("decode mock mint");
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let bytes = match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .expect("balance query")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    };
    decode_one::<Nat>(&bytes)
        .expect("decode balance")
        .0
        .try_into()
        .expect("balance fits u128")
}

fn liquidity_status(pic: &PocketIc, backend: Principal, owner: Principal) -> LiquidityStatus {
    let bytes = match pic
        .query_call(
            backend,
            owner,
            "get_liquidity_status",
            encode_one(owner).unwrap(),
        )
        .expect("liquidity status query")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("liquidity status rejected: {message}"),
    };
    decode_one(&bytes).expect("decode liquidity status")
}

fn request_state(pic: &PocketIc, backend: Principal, owner: Principal) -> LiquidityV2RequestState {
    let bytes = match pic
        .query_call(
            backend,
            owner,
            "get_my_liquidity_v2_request_state",
            encode_args(()).unwrap(),
        )
        .expect("liquidity V2 request-state query")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("request-state query rejected: {message}"),
    };
    decode_one(&bytes).expect("decode liquidity V2 request state")
}

fn assert_held(view: &LiquidityV2StatusView, request_id: u128, kind: LiquidityV2Kind) {
    assert_eq!(view.request_id, request_id);
    assert_eq!(view.kind, kind);
    assert_eq!(view.phase, LiquidityV2Phase::Held);
    assert!(view.had_ambiguous_attempt);
}

#[test]
fn liquidity_v2_recovers_exact_burn_and_mint_once_across_upgrade() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let user = Principal::self_authenticating(b"liquidity-v2-exact-receipts-user");
    let developer = Principal::self_authenticating(b"liquidity-v2-exact-receipts-dev");

    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    pic.install_canister(
        ledger,
        artifact("flaky_ledger.wasm"),
        encode_one(()).unwrap(),
        None,
    );
    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(),
        mock_xrc_canister::prepare_mock_xrc(),
        None,
    );

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
    let _: () = decode_one(&update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_minting_account",
        Some(account(backend)),
    ))
    .expect("configure backend minting account");

    mint(&pic, ledger, user, STARTING_BALANCE);
    let approval: Result<Nat, ApproveError> = decode_one(&update(
        &pic,
        ledger,
        user,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(STARTING_BALANCE),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode ICRC-2 approval");
    assert!(approval.is_ok(), "approval failed: {approval:?}");
    assert_eq!(balance(&pic, ledger, user), STARTING_BALANCE);
    assert_eq!(balance(&pic, ledger, backend), 0);

    // The ICRC-2 burn commits and the mock then returns GenericError. It is
    // ambiguous to the caller, so no LP credit may be recorded yet.
    let _: () = decode_one(&update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_icrc3_unavailable_after_phantom_failure",
        true,
    ))
    .expect("arm post-commit receipt-history outage");
    let _: () = decode_one(&update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        1u32,
    ))
    .expect("arm post-commit ICRC-2 fault");
    let first_provide: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "provide_liquidity_v2",
        (1u128, LIQUIDITY),
    ))
    .expect("decode ambiguous Provide result");
    assert!(
        first_provide.is_err(),
        "expected ambiguous Provide, got {first_provide:?}"
    );
    let _: () = decode_one(&update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_icrc3_unavailable_after_phantom_failure",
        false,
    ))
    .expect("restore ICRC-3 history after held Provide");
    assert_eq!(
        balance(&pic, ledger, user),
        STARTING_BALANCE - LIQUIDITY as u128
    );
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);
    let held_provide = request_state(&pic, backend, user);
    assert_eq!(held_provide.next_request_id, 2);
    assert_held(
        held_provide
            .active_request
            .as_ref()
            .expect("held Provide row"),
        1,
        LiquidityV2Kind::Provide,
    );

    // An invalid candidate must not settle the request. A different ID is
    // fenced while the original receipt remains unresolved.
    let wrong_candidate: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "attach_my_liquidity_v2_candidate",
        (1u128, u64::MAX),
    ))
    .expect("decode wrong candidate response");
    assert!(
        wrong_candidate.is_err(),
        "wrong candidate unexpectedly proved receipt"
    );
    let different_id: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "provide_liquidity_v2",
        (2u128, LIQUIDITY),
    ))
    .expect("decode different-ID fence response");
    assert!(
        different_id.is_err(),
        "different request ID bypassed held-operation fence"
    );
    assert_eq!(
        balance(&pic, ledger, user),
        STARTING_BALANCE - LIQUIDITY as u128
    );
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);

    // Persist the Held request through an actual backend upgrade before exact
    // same-ID recovery scans ICRC-3 history and proves the original burn.
    pic.upgrade_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_args((ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: Some("liquidity V2 held-receipt regression".into()),
        }),))
        .unwrap(),
        None,
    )
    .expect("upgrade backend while Provide receipt is held");
    let upgraded_state = request_state(&pic, backend, user);
    assert_held(
        upgraded_state
            .active_request
            .as_ref()
            .expect("Held Provide survives upgrade"),
        1,
        LiquidityV2Kind::Provide,
    );
    assert_eq!(
        balance(&pic, ledger, user),
        STARTING_BALANCE - LIQUIDITY as u128
    );
    let recovered_provide: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "provide_liquidity_v2",
        (1u128, LIQUIDITY),
    ))
    .expect("decode same-ID Provide recovery");
    let provide_block = recovered_provide.expect("exact burn should recover");
    assert_eq!(
        balance(&pic, ledger, user),
        STARTING_BALANCE - LIQUIDITY as u128
    );
    assert_eq!(balance(&pic, ledger, backend), 0);
    let credited = liquidity_status(&pic, backend, user);
    assert_eq!(credited.liquidity_provided, LIQUIDITY);
    assert_eq!(credited.total_liquidity_provided, LIQUIDITY);
    let completed_provide = request_state(&pic, backend, user);
    assert_eq!(completed_provide.next_request_id, 2);
    assert!(completed_provide.active_request.is_none());
    let provide_result = completed_provide
        .latest_result
        .expect("terminal Provide result");
    assert_eq!(provide_result.phase, LiquidityV2Phase::Complete);
    assert_eq!(provide_result.result_block_index, Some(provide_block));
    let provide_replay: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "provide_liquidity_v2",
        (1u128, LIQUIDITY),
    ))
    .expect("decode terminal Provide replay");
    assert_eq!(
        provide_replay.expect("terminal replay succeeds"),
        provide_block
    );
    assert_eq!(
        balance(&pic, ledger, user),
        STARTING_BALANCE - LIQUIDITY as u128
    );
    assert_eq!(
        liquidity_status(&pic, backend, user).liquidity_provided,
        LIQUIDITY
    );

    // With the backend configured as minter, an ambiguous ICRC-1 transfer
    // commits exactly one mint to the owner. Immediate ICRC-3 history proof
    // recovers it in the same call; terminal replay neither mints nor debits.
    let _: () = decode_one(&update(
        &pic,
        ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        1u32,
    ))
    .expect("arm post-commit mint fault");
    let first_withdraw: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "withdraw_liquidity_v2",
        (2u128, LIQUIDITY),
    ))
    .expect("decode ambiguous Withdraw result");
    let withdraw_block =
        first_withdraw.expect("available exact mint receipt should recover immediately");
    assert_eq!(balance(&pic, ledger, user), STARTING_BALANCE);
    assert_eq!(balance(&pic, ledger, backend), 0);
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);
    let completed_withdraw = request_state(&pic, backend, user);
    assert_eq!(completed_withdraw.next_request_id, 3);
    assert!(completed_withdraw.active_request.is_none());
    let withdraw_result = completed_withdraw
        .latest_result
        .expect("terminal Withdraw result");
    assert_eq!(withdraw_result.phase, LiquidityV2Phase::Complete);
    assert_eq!(withdraw_result.kind, LiquidityV2Kind::Withdraw);
    assert!(withdraw_result.had_ambiguous_attempt);
    assert_eq!(withdraw_result.result_block_index, Some(withdraw_block));
    let withdraw_replay: Result<u64, ProtocolError> = decode_one(&update_pair(
        &pic,
        backend,
        user,
        "withdraw_liquidity_v2",
        (2u128, LIQUIDITY),
    ))
    .expect("decode terminal Withdraw replay");
    assert_eq!(
        withdraw_replay.expect("terminal replay succeeds"),
        withdraw_block
    );
    assert_eq!(balance(&pic, ledger, user), STARTING_BALANCE);
    assert_eq!(liquidity_status(&pic, backend, user).liquidity_provided, 0);
}

mod mock_xrc_canister;
