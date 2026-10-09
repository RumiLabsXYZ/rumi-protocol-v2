//! Exercises the native ICP ledger block wire schema and archive callback with
//! the pinned official NNS ledger release. Run with:
//! `POCKET_IC_BIN=/path/to/pocket-ic RUMI_TEST_NNS_LEDGER_WASM_GZ=/path/to/rumi-nns-ledger-69b755.wasm.gz cargo test -p liquidation_bot --test native_icp_treasury_archive_pic -- --ignored --exact official_icp_treasury_transfer_decodes_direct_and_archived_blocks --nocapture`

use candid::{CandidType, Encode, IDLArgs, IDLValue, Nat, Principal};
use pocket_ic::{PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha256};
use std::{
    process::Command,
    time::{Duration as StdDuration, UNIX_EPOCH},
};

#[path = "../src/native_icp_blocks.rs"]
mod native_icp_blocks;
use native_icp_blocks::{Block as IcpBlock, BlockSource, GetBlocksArgs, QueryBlocksResponse};

const ICP_LEDGER_ID: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const OFFICIAL_LEDGER_GZIP_SHA256: &str =
    "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";

#[derive(CandidType)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}
#[derive(CandidType)]
struct Tokens {
    e8s: u64,
}
#[derive(CandidType)]
struct Duration {
    secs: u64,
    nanos: u32,
}
#[derive(CandidType)]
struct FeatureFlags {
    icrc2: bool,
}
#[derive(CandidType)]
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
#[derive(CandidType)]
struct LedgerInit {
    minting_account: String,
    icrc1_minting_account: Option<Account>,
    initial_values: Vec<(String, Tokens)>,
    max_message_size_bytes: Option<u64>,
    transaction_window: Option<Duration>,
    archive_options: Option<ArchiveOptions>,
    send_whitelist: Vec<Principal>,
    transfer_fee: Option<Tokens>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    feature_flags: Option<FeatureFlags>,
}
#[derive(CandidType)]
enum LedgerArg {
    Init(LedgerInit),
}
#[derive(CandidType)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}
fn account_identifier(owner: Principal) -> [u8; 32] {
    native_icp_blocks::default_account_identifier(owner)
        .try_into()
        .expect("default AccountIdentifier is 32 bytes")
}

fn account_identifier_hex(owner: Principal) -> String {
    account_identifier(owner)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn transfer_block_index(bytes: &[u8]) -> u64 {
    let decoded = IDLArgs::from_bytes(bytes).expect("decode transfer response");
    let IDLValue::Variant(result) = &decoded.args[0] else {
        panic!("transfer result is not a variant")
    };
    assert_eq!(
        result.0.id.get_id(),
        candid::idl_hash("Ok"),
        "transfer failed: {decoded:?}"
    );
    match &result.0.val {
        IDLValue::Nat(n) => u64::try_from(n.0.clone()).expect("block index fits u64"),
        IDLValue::Nat64(n) => *n,
        value => panic!("unexpected block index type {value:?}"),
    }
}

fn ledger_transfer(
    pic: &pocket_ic::PocketIc,
    ledger: Principal,
    sender: Principal,
    to: Principal,
    amount: u64,
    memo: Vec<u8>,
    created_at_time: u64,
) -> u64 {
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: Nat::from(amount),
        fee: Some(Nat::from(10_000u64)),
        memo: Some(memo),
        created_at_time: Some(created_at_time),
    };
    let bytes = match pic
        .update_call(ledger, sender, "icrc1_transfer", Encode!(&args).unwrap())
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("ICP transfer rejected: {message}"),
    };
    transfer_block_index(&bytes)
}

fn query_blocks(pic: &pocket_ic::PocketIc, ledger: Principal, start: u64) -> QueryBlocksResponse {
    let bytes = match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "query_blocks",
            Encode!(&GetBlocksArgs { start, length: 1 }).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("query_blocks rejected: {message}"),
    };
    native_icp_blocks::decode_query_blocks(&bytes).expect("decode native ICP query_blocks ABI")
}

fn assert_exact_transfer(
    block: &IcpBlock,
    sender: Principal,
    treasury: Principal,
    amount: u64,
    fee: u64,
    memo: &[u8],
    created_at_time: u64,
) {
    native_icp_blocks::verify_treasury_transfer(
        block,
        sender,
        treasury,
        amount,
        fee,
        memo,
        created_at_time,
    )
    .expect("native ICP block matches the production receipt-tuple verifier");
}

#[test]
#[ignore = "requires POCKET_IC_BIN and the pinned official NNS ledger gzip"]
fn official_icp_treasury_transfer_decodes_direct_and_archived_blocks() {
    let ledger_gzip_path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .expect("set RUMI_TEST_NNS_LEDGER_WASM_GZ to the pinned official NNS ledger gzip");
    let ledger_gzip = std::fs::read(&ledger_gzip_path)
        .unwrap_or_else(|error| panic!("read {ledger_gzip_path}: {error}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&ledger_gzip)),
        OFFICIAL_LEDGER_GZIP_SHA256,
        "pinned official NNS ledger gzip hash changed"
    );
    let decompressed = Command::new("gzip")
        .args(["-dc", &ledger_gzip_path])
        .output()
        .expect("run gzip to decompress pinned official NNS ledger");
    assert!(
        decompressed.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&decompressed.stderr)
    );
    let ledger_wasm = decompressed.stdout;

    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let created_at_time = 1_711_324_800_000_000_000u64;
    pic.set_time(UNIX_EPOCH + StdDuration::from_nanos(created_at_time));
    let ledger = Principal::from_text(ICP_LEDGER_ID).unwrap();
    let sender = Principal::from_slice(&[1]);
    let treasury = Principal::from_slice(&[2]);
    let controller = Principal::from_slice(&[9]);
    pic.create_canister_with_id(None, None, ledger)
        .expect("create fixed native ICP ledger ID");
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    let init = LedgerArg::Init(LedgerInit {
        minting_account: account_identifier_hex(Principal::management_canister()),
        icrc1_minting_account: Some(Account {
            owner: Principal::management_canister(),
            subaccount: None,
        }),
        initial_values: vec![(
            account_identifier_hex(sender),
            Tokens {
                e8s: 1_000_000_000_000,
            },
        )],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(ArchiveOptions {
            num_blocks_to_archive: 1,
            max_transactions_per_response: Some(100),
            trigger_threshold: 2,
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: controller,
            more_controller_ids: None,
        }),
        send_whitelist: Vec::new(),
        transfer_fee: Some(Tokens { e8s: 10_000 }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(FeatureFlags { icrc2: true }),
    });
    pic.install_canister(ledger, ledger_wasm, Encode!(&init).unwrap(), None);

    let amount = 15_000u64;
    let fee = 10_000u64;
    let gross = amount + fee;
    let memo = b"RUMI:TB1:exact-ledger-fixture".to_vec();
    assert_eq!(gross, 25_000);
    let block_index = ledger_transfer(
        &pic,
        ledger,
        sender,
        treasury,
        amount,
        memo.clone(),
        created_at_time,
    );

    let direct = query_blocks(&pic, ledger, block_index);
    let direct_source = native_icp_blocks::select_block_source(direct, block_index)
        .expect("production selector accepts exactly one direct receipt block");
    let BlockSource::Direct(direct_block) = direct_source else {
        panic!("fresh transfer should remain in the direct ledger range");
    };
    assert_exact_transfer(
        &direct_block,
        sender,
        treasury,
        amount,
        fee,
        &memo,
        created_at_time,
    );

    // A different, valid ICP transfer at the operator-supplied wrong block
    // index must not be accepted as proof for the saved treasury tuple.
    let wrong_block_index = ledger_transfer(
        &pic,
        ledger,
        sender,
        treasury,
        1,
        b"wrong-candidate-block".to_vec(),
        created_at_time + 10,
    );
    let wrong_source = native_icp_blocks::select_block_source(
        query_blocks(&pic, ledger, wrong_block_index),
        wrong_block_index,
    )
    .expect("select the valid but unrelated candidate block");
    let BlockSource::Direct(wrong_block) = wrong_source else {
        panic!("new candidate transfer should remain in the direct ledger range");
    };
    assert!(native_icp_blocks::verify_treasury_transfer(
        &wrong_block,
        sender,
        treasury,
        amount,
        fee,
        &memo,
        created_at_time,
    )
    .is_err());

    let mut archived_block = None;
    for index in 0..20u64 {
        let _ = ledger_transfer(
            &pic,
            ledger,
            sender,
            treasury,
            1,
            format!("archive-{index}").into_bytes(),
            created_at_time + index + 1,
        );
        for _ in 0..10 {
            pic.tick();
        }
        let response = query_blocks(&pic, ledger, block_index);
        if let Ok(BlockSource::Archive {
            canister_id,
            method,
        }) = native_icp_blocks::select_block_source(response, block_index)
        {
            let archive_reply = pic
                .query_call(
                    canister_id,
                    Principal::anonymous(),
                    &method,
                    Encode!(&GetBlocksArgs {
                        start: block_index,
                        length: 1
                    })
                    .unwrap(),
                )
                .expect("call ledger-advertised archive callback")
                .into_reply()
                .expect("archive callback reply");
            let result = native_icp_blocks::decode_archive_result(&archive_reply)
                .expect("decode native archive callback ABI");
            let mut blocks = result.expect("archive serves exact block").blocks;
            assert_eq!(
                blocks.len(),
                1,
                "callback returns exactly one requested block"
            );
            archived_block = blocks.pop();
            break;
        }
    }
    let archived_block =
        archived_block.expect("low archive threshold moves receipt block to archive");
    assert_exact_transfer(
        &archived_block,
        sender,
        treasury,
        amount,
        fee,
        &memo,
        created_at_time,
    );
}

trait PocketIcReply {
    fn into_reply(self) -> Result<Vec<u8>, String>;
}

impl PocketIcReply for WasmResult {
    fn into_reply(self) -> Result<Vec<u8>, String> {
        match self {
            WasmResult::Reply(bytes) => Ok(bytes),
            WasmResult::Reject(message) => Err(message),
        }
    }
}
