//! Empirical expiry and zero-amount semantics of the checked-in Rumi ICRC-1
//! ledger Wasm. This exercises the ledger directly; it does not add or call a
//! backend recovery probe.

use candid::{decode_one, encode_args, encode_one, CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{TransferArg, TransferError},
};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::{PocketIcBuilder, WasmResult};
use serde::Deserialize;
use std::time::{Duration, SystemTime};

const BASE_TIME_NS: u64 = 1_700_000_000_000_000_000;
const ADVANCE_PAST_WINDOW: Duration = Duration::from_secs(25 * 60 * 60);

#[derive(CandidType, Deserialize)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    max_transactions_per_response: Option<u64>,
    trigger_threshold: u64,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    controller_id: Principal,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize)]
enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
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
enum LedgerArgument {
    Init(LedgerInitArgs),
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn install_ledger(pic: &pocket_ic::PocketIc, minter: Principal) -> Principal {
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);

    let init = LedgerArgument::Init(LedgerInitArgs {
        minting_account: account(minter),
        fee_collector_account: None,
        transfer_fee: Nat::from(0u8),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "Rumi icUSD".to_string(),
        token_symbol: "icUSD".to_string(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2_000,
            max_transactions_per_response: None,
            trigger_threshold: 1_000,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            controller_id: Principal::anonymous(),
            more_controller_ids: None,
        },
    });

    pic.install_canister(
        ledger,
        ledger_wasm(),
        encode_args((init,)).expect("encode official ledger init args"),
        None,
    );
    ledger
}

fn call_transfer(
    pic: &pocket_ic::PocketIc,
    ledger: Principal,
    caller: Principal,
    amount: u64,
    created_at_time: u64,
    memo: &[u8],
) -> Result<Nat, TransferError> {
    let args = TransferArg {
        from_subaccount: None,
        to: account(Principal::anonymous()),
        amount: Nat::from(amount),
        fee: None,
        memo: Some(memo.to_vec().into()),
        created_at_time: Some(created_at_time),
    };
    match pic
        .update_call(
            ledger,
            caller,
            "icrc1_transfer",
            encode_one(args).expect("encode transfer args"),
        )
        .expect("call icrc1_transfer")
    {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).expect("decode typed icrc1_transfer result")
        }
        WasmResult::Reject(message) => panic!("icrc1_transfer rejected: {message}"),
    }
}

fn log_length(pic: &pocket_ic::PocketIc, ledger: Principal) -> u64 {
    let request = GetBlocksRequest {
        start: Nat::from(0u8),
        length: Nat::from(100u8),
    };
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_args((vec![request],)).expect("encode icrc3_get_blocks request"),
        )
        .expect("query icrc3_get_blocks");
    let result: GetBlocksResult = match reply {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode ICRC-3 blocks result"),
        WasmResult::Reject(message) => panic!("icrc3_get_blocks rejected: {message}"),
    };
    u64::try_from(result.log_length.0).expect("log length fits u64")
}

fn balance(pic: &pocket_ic::PocketIc, ledger: Principal, owner: Principal) -> Nat {
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_args((account(owner),)).expect("encode icrc1_balance_of account"),
        )
        .expect("query icrc1_balance_of");
    match reply {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode ledger balance"),
        WasmResult::Reject(message) => panic!("icrc1_balance_of rejected: {message}"),
    }
}

#[test]
fn official_ledger_fresh_zero_and_expired_transfers_have_expected_effects() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_nanos(BASE_TIME_NS));

    let minter = Principal::self_authenticating(b"borrow-mint-ledger-minter");
    let recipient = Principal::anonymous();
    let ledger = install_ledger(&pic, minter);

    let initial_log_length = log_length(&pic, ledger);
    let initial_minter_balance = balance(&pic, ledger, minter);
    let initial_recipient_balance = balance(&pic, ledger, recipient);

    // A fresh zero amount is a distinct ledger operation. Record whether it
    // creates an ICRC-3 entry while leaving both account balances unchanged.
    let fresh_zero = call_transfer(
        &pic,
        ledger,
        minter,
        0,
        BASE_TIME_NS,
        b"borrow-mint-fresh-zero",
    );
    assert!(fresh_zero.is_ok(), "fresh zero transfer result: {fresh_zero:?}");
    assert_eq!(
        log_length(&pic, ledger),
        initial_log_length + 1,
        "a successful fresh zero mint should add one ICRC-3 block"
    );
    assert_eq!(balance(&pic, ledger, minter), initial_minter_balance);
    assert_eq!(balance(&pic, ledger, recipient), initial_recipient_balance);

    // Use two never-before-seen operation identities. This ensures Duplicate
    // cannot hide whether expiry rejects each amount with typed TooOld.
    pic.advance_time(ADVANCE_PAST_WINDOW);
    let expired_time_ns = BASE_TIME_NS;

    let before_positive_log = log_length(&pic, ledger);
    let before_positive_minter_balance = balance(&pic, ledger, minter);
    let before_positive_recipient_balance = balance(&pic, ledger, recipient);
    let expired_positive = call_transfer(
        &pic,
        ledger,
        minter,
        123,
        expired_time_ns,
        b"borrow-mint-expired-positive",
    );
    assert_eq!(expired_positive, Err(TransferError::TooOld));
    assert_eq!(log_length(&pic, ledger), before_positive_log);
    assert_eq!(balance(&pic, ledger, minter), before_positive_minter_balance);
    assert_eq!(balance(&pic, ledger, recipient), before_positive_recipient_balance);

    let before_zero_log = log_length(&pic, ledger);
    let before_zero_minter_balance = balance(&pic, ledger, minter);
    let before_zero_recipient_balance = balance(&pic, ledger, recipient);
    let expired_zero = call_transfer(
        &pic,
        ledger,
        minter,
        0,
        expired_time_ns,
        b"borrow-mint-expired-zero",
    );
    assert_eq!(expired_zero, Err(TransferError::TooOld));
    assert_eq!(log_length(&pic, ledger), before_zero_log);
    assert_eq!(balance(&pic, ledger, minter), before_zero_minter_balance);
    assert_eq!(balance(&pic, ledger, recipient), before_zero_recipient_balance);
}
