//! PocketIC proof that the bundled official ICRC ledger records an ordinary
//! ICRC-2 transfer_from as an exact transfer block and that the backend's
//! persisted-tuple validator accepts that block (distinct from minter burns).

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::{
    icrc::generic_value::ICRC3Value,
    icrc1::{
        account::Account,
        transfer::{TransferArg, TransferError},
    },
    icrc2::{
        approve::{ApproveArgs, ApproveError},
        transfer_from::{TransferFromArgs, TransferFromError},
    },
    icrc3::blocks::{GetBlocksRequest, GetBlocksResult},
};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    icrc3_proof::{decode_block, validate_icrc3_transfer_from_block},
    SpLiquidationStablePullTuple,
};
use std::time::{Duration, SystemTime};

const LEDGER_FEE: u64 = 10_000;
const TRANSFER_AMOUNT: u64 = 123_456_789;

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
    nat: Option<Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
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
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn official_ledger(pic: &PocketIc, mint: Principal, controller: Principal) -> Principal {
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    let args = LedgerInitArgs {
        minting_account: account(mint),
        fee_collector_account: None,
        transfer_fee: Nat::from(LEDGER_FEE),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "Transfer From Proof".into(),
        token_symbol: "TFP".into(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2_000,
            trigger_threshold: 1_000,
            controller_id: controller,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    let wasm = include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec();
    pic.install_canister(
        ledger,
        wasm,
        encode_args((LedgerArg::Init(args),)).unwrap(),
        None,
    );
    ledger
}

fn update<T: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: impl CandidType,
) -> T {
    let reply = pic
        .update_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("{method}: {error}"));
    match reply {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("{method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

#[test]
fn official_ledger_2xfer_proves_exact_persisted_transfer_from_tuple() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let minter = Principal::self_authenticating(b"icrc3-2xfer-minter");
    let spender = Principal::self_authenticating(b"icrc3-2xfer-spender");
    let owner = Principal::self_authenticating(b"icrc3-2xfer-owner");
    let recipient = Principal::self_authenticating(b"icrc3-2xfer-recipient");
    let ledger = official_ledger(&pic, minter, minter);

    let funded: Result<Nat, TransferError> = update(
        &pic,
        ledger,
        minter,
        "icrc1_transfer",
        TransferArg {
            from_subaccount: None,
            to: account(owner),
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(1_000_000_000u64),
        },
    );
    funded.expect("minter funds owner");

    let memo = b"ordinary-icrc2-transfer-from-proof".to_vec();
    let created_at_time_ns = pic
        .get_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("PocketIC clock after Unix epoch")
        .as_nanos()
        .try_into()
        .expect("PocketIC timestamp fits u64");
    let approved: Result<Nat, ApproveError> = update(
        &pic,
        ledger,
        owner,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(spender),
            amount: Nat::from(TRANSFER_AMOUNT + LEDGER_FEE),
            expected_allowance: None,
            expires_at: Some(created_at_time_ns + Duration::from_secs(60).as_nanos() as u64),
            fee: None,
            memo: Some(b"approve-for-2xfer".to_vec().into()),
            created_at_time: Some(created_at_time_ns),
        },
    );
    approved.expect("owner approves spender");

    let tuple = SpLiquidationStablePullTuple {
        op_nonce: 17,
        ledger,
        from: account(owner),
        spender: account(spender),
        to: account(recipient),
        amount_raw: TRANSFER_AMOUNT,
        fee_raw: LEDGER_FEE,
        memo: memo.clone(),
        created_at_time_ns,
    };
    let transferred: Result<Nat, TransferFromError> = update(
        &pic,
        ledger,
        spender,
        "icrc2_transfer_from",
        TransferFromArgs {
            spender_subaccount: None,
            from: tuple.from.clone(),
            to: tuple.to.clone(),
            amount: Nat::from(tuple.amount_raw),
            fee: Some(Nat::from(tuple.fee_raw)),
            memo: Some(tuple.memo.clone().into()),
            created_at_time: Some(tuple.created_at_time_ns),
        },
    );
    let block_index: u64 = transferred
        .expect("ordinary transfer_from succeeds")
        .0
        .try_into()
        .unwrap();

    let response: GetBlocksResult = decode_one(&match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_one(vec![GetBlocksRequest {
                start: Nat::from(block_index),
                length: Nat::from(1u64),
            }])
            .unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("icrc3_get_blocks rejected: {message}"),
    })
    .expect("decode ICRC-3 response");
    let block = response
        .blocks
        .into_iter()
        .find(|candidate| candidate.id == Nat::from(block_index))
        .expect("transfer block returned by official ledger");
    let raw = block.block;
    let decoded = decode_block(&raw).expect("decode official ledger block");

    let top = match &raw {
        ICRC3Value::Map(map) => map,
        other => panic!("unexpected official ledger block shape: {other:?}"),
    };
    assert!(
        !top.contains_key("btype"),
        "the bundled ledger emits the legacy block shape without top-level btype"
    );
    let tx = match top.get("tx") {
        Some(ICRC3Value::Map(map)) => map,
        other => panic!("missing legacy tx map: {other:?}"),
    };
    assert_eq!(
        tx.get("op"),
        Some(&ICRC3Value::Text("xfer".into())),
        "legacy 2xfer is identified by tx.op=xfer"
    );
    assert_eq!(decoded.from, Some(tuple.from.clone()));
    assert_eq!(decoded.to, Some(tuple.to.clone()));
    assert_eq!(decoded.spender, Some(tuple.spender.clone()));
    assert_eq!(decoded.amount, u128::from(tuple.amount_raw));
    assert_eq!(decoded.transaction_fee, Some(u128::from(tuple.fee_raw)));
    assert_eq!(decoded.fee, Some(u128::from(tuple.fee_raw)));
    assert_eq!(decoded.memo.as_deref(), Some(tuple.memo.as_slice()));
    assert_eq!(decoded.created_at_time, Some(tuple.created_at_time_ns));
    eprintln!(
        "official ledger ICRC-3 schema: btype=absent, tx.op=xfer, spender_present={}, fee={}, block_index={}",
        decoded.spender.is_some(), decoded.transaction_fee.unwrap_or_default(), block_index
    );
    validate_icrc3_transfer_from_block(&decoded, &tuple)
        .expect("official ICRC-3 block proves the exact persisted transfer_from tuple");
}
