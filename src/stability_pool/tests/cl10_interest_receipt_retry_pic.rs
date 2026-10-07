//! CL-10 PocketIC regression for source-mint receipt binding.
//!
//! Replaying the exact notification models a backend retry after losing the
//! reply: the SP must acknowledge it without crediting interest twice. Reusing
//! that source block with any changed payload must remain unacknowledged.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::{
    icrc1::account::Account,
    icrc2::approve::{ApproveArgs, ApproveError},
};
use pocket_ic::{PocketIcBuilder, WasmResult};
use stability_pool::types::{
    StabilityPoolError, StabilityPoolInitArgs, StablecoinConfig, UserStabilityPosition,
};

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
enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
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
    Init(LedgerInitArgs),
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn sp_wasm() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/release/stability_pool.wasm"
    ))
    .expect("build the stability_pool release Wasm before this PocketIC test")
}

fn reply<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult, label: &str) -> T {
    match result {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {label} reply: {error}"))
        }
        WasmResult::Reject(message) => panic!("{label} rejected: {message}"),
    }
}

#[test]
fn cl10_interest_retry_is_idempotent_and_conflicting_payload_stays_pending() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(b"cl10-interest-admin");
    let protocol = Principal::self_authenticating(b"cl10-interest-backend");
    let user = Principal::self_authenticating(b"cl10-interest-user");
    let minting = Principal::self_authenticating(b"cl10-interest-minter");

    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    let ledger_args = LedgerArg::Init(LedgerInitArgs {
        minting_account: Account {
            owner: minting,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![(
            Account {
                owner: user,
                subaccount: None,
            },
            candid::Nat::from(1_000_000_000u64),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1_000,
            trigger_threshold: 2_000,
            controller_id: admin,
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
        encode_args((ledger_args,)).unwrap(),
        None,
    );

    let sp = pic.create_canister();
    pic.add_cycles(sp, 2_000_000_000_000);
    pic.install_canister(
        sp,
        sp_wasm(),
        encode_one(StabilityPoolInitArgs {
            protocol_canister_id: protocol,
            authorized_admins: vec![admin],
        })
        .unwrap(),
        None,
    );

    let registered = pic
        .update_call(
            sp,
            admin,
            "register_stablecoin",
            encode_one(StablecoinConfig {
                ledger_id: ledger,
                symbol: "icUSD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: None,
                underlying_pool: None,
            })
            .unwrap(),
        )
        .unwrap();
    reply::<Result<(), StabilityPoolError>>(registered, "register_stablecoin")
        .expect("register icUSD");

    let approved = pic
        .update_call(
            ledger,
            user,
            "icrc2_approve",
            encode_one(ApproveArgs {
                from_subaccount: None,
                spender: Account {
                    owner: sp,
                    subaccount: None,
                },
                amount: candid::Nat::from(100_000_000u64),
                expected_allowance: None,
                expires_at: None,
                fee: None,
                memo: None,
                created_at_time: None,
            })
            .unwrap(),
        )
        .unwrap();
    reply::<Result<candid::Nat, ApproveError>>(approved, "icrc2_approve")
        .expect("approve SP spending");
    let deposited = pic
        .update_call(
            sp,
            user,
            "deposit",
            encode_args((ledger, 100_000_000u64)).unwrap(),
        )
        .unwrap();
    reply::<Result<(), StabilityPoolError>>(deposited, "deposit").expect("deposit succeeds");

    let notification = || encode_args((ledger, 10_000u64, None::<Principal>, 42u64)).unwrap();
    for _ in 0..2 {
        let result = pic
            .update_call(sp, protocol, "receive_interest_revenue_v2", notification())
            .unwrap();
        reply::<Result<(), StabilityPoolError>>(result, "same-payload retry")
            .expect("same source receipt is acknowledged");
    }

    for conflicting in [
        encode_args((ledger, 10_001u64, None::<Principal>, 42u64)).unwrap(),
        encode_args((ledger, 10_000u64, Some(admin), 42u64)).unwrap(),
    ] {
        let result = pic
            .update_call(sp, protocol, "receive_interest_revenue_v2", conflicting)
            .unwrap();
        assert!(matches!(
            reply::<Result<(), StabilityPoolError>>(result, "conflicting retry"),
            Err(StabilityPoolError::SystemBusy)
        ));
    }

    let position: Option<UserStabilityPosition> = reply(
        pic.query_call(
            sp,
            user,
            "get_user_position",
            encode_one(Some(user)).unwrap(),
        )
        .unwrap(),
        "get_user_position",
    );
    let position = position.expect("depositor position exists");
    assert_eq!(position.total_interest_earned_e8s, 10_000);
    assert_eq!(
        position.stablecoin_balances.get(&ledger).copied(),
        Some(100_010_000),
        "a repeated receipt credits once and conflicting retries never credit",
    );
}
