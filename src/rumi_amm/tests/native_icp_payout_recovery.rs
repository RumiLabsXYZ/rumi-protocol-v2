use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::{
    state::{OutboundPayout, OutboundPayoutStatus},
    types::{AmmError, AmmInitArgs},
};
use sha2::{Digest, Sha224, Sha256};
use std::{process::Command, time::UNIX_EPOCH};

#[path = "../../liquidation_bot/src/native_icp_blocks.rs"]
mod native_icp_blocks;

const ICP_LEDGER: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const FEE: u64 = 10_000;
const OFFICIAL_LEDGER_GZIP_SHA256: &str =
    "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";

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
struct Duration {
    secs: u64,
    nanos: u32,
}
#[derive(CandidType, Deserialize)]
struct Tokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize)]
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
#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInit),
}

fn reply<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(reason) => panic!("canister rejected: {reason}"),
    }
}
fn amm_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}
fn native_ledger_wasm() -> Vec<u8> {
    let path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .unwrap_or_else(|_| "/private/tmp/rumi-nns-ledger-69b755.wasm.gz".into());
    let compressed =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read official ledger {path}: {e}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&compressed)),
        OFFICIAL_LEDGER_GZIP_SHA256,
        "pinned official NNS ledger artifact hash changed"
    );
    let result = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip");
    assert!(
        result.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn account_identifier_hex(owner: Principal) -> String {
    let mut hash = Sha224::new();
    hash.update(b"\x0Aaccount-id");
    hash.update(owner.as_slice());
    hash.update([0u8; 32]);
    let digest = hash.finalize();
    let mut crc = !0u32;
    for byte in digest.iter() {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut bytes = (!crc).to_be_bytes().to_vec();
    bytes.extend_from_slice(&digest);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn now_ns(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

struct Env {
    pic: PocketIc,
    amm: Principal,
    ledger: Principal,
    admin: Principal,
    recipient: Principal,
    index: u64,
    payout: OutboundPayout,
}

fn setup(archive: bool) -> Env {
    // The canonical native ICP ledger principal is classified by PocketIC as
    // an NNS-subnet canister ID, so the fixture needs an NNS subnet as well.
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let admin = Principal::self_authenticating(&[70, 71, 72]);
    let recipient = Principal::self_authenticating(&[73, 74, 75]);
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(
        amm,
        amm_wasm(),
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    );

    let ledger = pic
        .create_canister_with_id(Some(admin), None, Principal::from_text(ICP_LEDGER).unwrap())
        .unwrap();
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    pic.set_time(UNIX_EPOCH + std::time::Duration::from_nanos(1_711_324_800_000_000_000));
    let init = LedgerArg::Init(LedgerInit {
        minting_account: account_identifier_hex(Principal::management_canister()),
        icrc1_minting_account: Some(Account {
            owner: Principal::management_canister(),
            subaccount: None,
        }),
        initial_values: vec![(account_identifier_hex(amm), Tokens { e8s: 2_000_000 })],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(ArchiveOptions {
            num_blocks_to_archive: 1,
            max_transactions_per_response: Some(100),
            trigger_threshold: if archive { 2 } else { 1_000 },
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: admin,
            more_controller_ids: None,
        }),
        send_whitelist: vec![],
        transfer_fee: Some(Tokens { e8s: FEE }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(FeatureFlags { icrc2: true }),
    });
    pic.install_canister(
        ledger,
        native_ledger_wasm(),
        encode_args((init,)).unwrap(),
        Some(admin),
    );

    let memo = vec![0x52, 0x55, 0x4d, 0x49];
    let timestamp = now_ns(&pic);
    let transfer = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: recipient,
            subaccount: None,
        },
        amount: Nat::from(90_000u64),
        fee: Some(Nat::from(FEE)),
        memo: Some(memo.clone().into()),
        created_at_time: Some(timestamp),
    };
    let transfer_result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = reply(
        pic.update_call(ledger, amm, "icrc1_transfer", encode_one(transfer).unwrap())
            .unwrap(),
    );
    let index = transfer_result.expect("seed exact native ICP payout");
    let index: u64 = index.0.try_into().expect("block index fits u64");
    let payout = OutboundPayout {
        id: 1,
        operation_id: "test-native-payout".into(),
        ledger,
        from: amm,
        from_subaccount: None,
        to: recipient,
        to_subaccount: None,
        gross_amount: 100_000,
        net_amount: 90_000,
        fee: u128::from(FEE),
        memo,
        created_at_time: timestamp,
        status: OutboundPayoutStatus::Ambiguous,
    };
    Env {
        pic,
        amm,
        ledger,
        admin,
        recipient,
        index,
        payout,
    }
}

fn verify(env: &Env, index: u64, payout: &OutboundPayout) -> Result<(), AmmError> {
    reply(
        env.pic
            .update_call(
                env.amm,
                env.admin,
                "pocketic_verify_native_payout_block",
                encode_args((env.ledger, index, payout)).unwrap(),
            )
            .unwrap(),
    )
}

fn query_native_blocks(
    pic: &PocketIc,
    ledger: Principal,
    start: u64,
) -> native_icp_blocks::QueryBlocksResponse {
    let bytes = match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "query_blocks",
            encode_one(native_icp_blocks::GetBlocksArgs { start, length: 1 }).unwrap(),
        )
        .expect("query official native ICP ledger blocks")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(reason) => panic!("native query_blocks rejected: {reason}"),
    };
    native_icp_blocks::decode_query_blocks(&bytes).expect("decode official query_blocks schema")
}

#[test]
#[ignore = "requires POCKET_IC_BIN and the pinned official NNS ledger gzip"]
fn native_icp_direct_block_matches_exact_payout_and_rejects_wrong_tuple() {
    let env = setup(false);
    verify(&env, env.index, &env.payout).expect("exact native ledger block proves payout");
    let mut wrong = env.payout.clone();
    wrong.to = env.admin;
    assert!(
        verify(&env, env.index, &wrong).is_err(),
        "wrong recipient must not prove payout"
    );
    let mut wrong_memo = env.payout.clone();
    wrong_memo.memo.push(0xff);
    assert!(
        verify(&env, env.index, &wrong_memo).is_err(),
        "wrong memo must not prove payout"
    );
}

#[test]
#[ignore = "requires POCKET_IC_BIN and the pinned official NNS ledger gzip"]
fn native_icp_archive_evidence_remains_held() {
    let env = setup(true);
    // Generate enough blocks to move the original transfer out of the direct
    // range. `query_blocks` will return an archive descriptor, which the AMM
    // adapter must reject without invoking that callback.
    for nonce in 0..5u64 {
        let transfer = TransferArg {
            from_subaccount: None,
            to: Account {
                owner: env.recipient,
                subaccount: None,
            },
            amount: Nat::from(10_000u64),
            fee: Some(Nat::from(FEE)),
            memo: Some(vec![nonce as u8].into()),
            created_at_time: Some(now_ns(&env.pic) + nonce + 1),
        };
        let result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = reply(
            env.pic
                .update_call(
                    env.ledger,
                    env.amm,
                    "icrc1_transfer",
                    encode_one(transfer).unwrap(),
                )
                .unwrap(),
        );
        result.expect("advance native ledger archive range");
    }
    let response = query_native_blocks(&env.pic, env.ledger, env.index);
    assert!(
        response.blocks.is_empty(),
        "aged target must no longer be in direct blocks"
    );
    assert!(
        response.archived_blocks.iter().any(|range| {
            range.length > 0
                && range
                    .start
                    .checked_add(range.length)
                    .is_some_and(|end| env.index >= range.start && env.index < end)
        }),
        "official native ledger must advertise an archive range covering the target index"
    );
    let result = verify(&env, env.index, &env.payout);
    assert!(
        result.is_err(),
        "archived or ambiguous native block evidence must remain held"
    );
}
