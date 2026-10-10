use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::{
    icrc1::account::Account,
    icrc2::{approve::ApproveArgs, transfer_from::TransferFromArgs},
};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::{
    state::{InboundLeg, InboundLegStatus},
    types::{AmmError, AmmInitArgs},
};
use sha2::{Digest, Sha224, Sha256};
use std::{process::Command, time::UNIX_EPOCH};

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
    let compressed = std::fs::read(&path).unwrap_or_else(|e| panic!("read official ledger {path}: {e}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&compressed)),
        OFFICIAL_LEDGER_GZIP_SHA256,
        "pinned official NNS ledger artifact hash changed"
    );
    let result = Command::new("gzip").args(["-dc", &path]).output().expect("run gzip");
    assert!(result.status.success(), "gzip failed: {}", String::from_utf8_lossy(&result.stderr));
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
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    let mut bytes = (!crc).to_be_bytes().to_vec();
    bytes.extend_from_slice(&digest);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ns(pic: &PocketIc) -> u64 {
    pic.get_time().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

struct Env {
    pic: PocketIc,
    amm: Principal,
    ledger: Principal,
    admin: Principal,
    user: Principal,
    destination_subaccount: [u8; 32],
    index: u64,
    leg: InboundLeg,
}

fn setup(archive: bool) -> Env {
    let pic = PocketIcBuilder::new().with_nns_subnet().with_application_subnet().build();
    let admin = Principal::self_authenticating(&[70, 71, 72]);
    let user = Principal::self_authenticating(&[73, 74, 75]);
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(amm, amm_wasm(), encode_one(AmmInitArgs { admin }).unwrap(), Some(admin));

    let ledger = pic
        .create_canister_with_id(Some(admin), None, Principal::from_text(ICP_LEDGER).unwrap())
        .unwrap();
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    pic.set_time(UNIX_EPOCH + std::time::Duration::from_nanos(1_711_324_800_000_000_000));
    let init = LedgerArg::Init(LedgerInit {
        minting_account: account_identifier_hex(Principal::management_canister()),
        icrc1_minting_account: Some(Account { owner: Principal::management_canister(), subaccount: None }),
        initial_values: vec![(account_identifier_hex(user), Tokens { e8s: 2_000_000 })],
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
    pic.install_canister(ledger, native_ledger_wasm(), encode_args((init,)).unwrap(), Some(admin));

    let destination_subaccount = [0x42; 32];
    let amount = 90_000u64;
    let memo = vec![0x52, 0x55, 0x4d, 0x49];
    let timestamp = now_ns(&pic);
    let approve = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: amm, subaccount: None },
        amount: Nat::from(1_000_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: Some(Nat::from(FEE)),
        memo: None,
        created_at_time: Some(timestamp),
    };
    let approved: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = reply(
        pic.update_call(ledger, user, "icrc2_approve", encode_one(approve).unwrap()).unwrap(),
    );
    approved.expect("approve AMM to pull user ICP");

    let pull = TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: user, subaccount: None },
        to: Account { owner: amm, subaccount: Some(destination_subaccount) },
        amount: Nat::from(amount),
        fee: Some(Nat::from(FEE)),
        memo: Some(memo.clone().into()),
        created_at_time: Some(timestamp + 1),
    };
    let pulled: Result<Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> = reply(
        pic.update_call(ledger, amm, "icrc2_transfer_from", encode_one(pull).unwrap()).unwrap(),
    );
    let index = pulled.expect("AMM transfer_from creates inbound ledger block");
    let index: u64 = index.0.try_into().expect("block index fits u64");
    let leg = InboundLeg {
        ledger,
        from: user,
        to_subaccount: Some(destination_subaccount),
        amount: u128::from(amount),
        fee: Some(u128::from(FEE)),
        memo,
        created_at_time: timestamp + 1,
        status: InboundLegStatus::Ambiguous,
    };
    Env { pic, amm, ledger, admin, user, destination_subaccount, index, leg }
}

fn verify(env: &Env, index: u64, leg: &InboundLeg) -> Result<(), AmmError> {
    reply(env.pic.update_call(
        env.amm,
        env.admin,
        "pocketic_verify_native_inbound_block",
        encode_args((env.ledger, index, leg)).unwrap(),
    ).unwrap())
}

#[test]
#[ignore = "requires POCKET_IC_BIN and the pinned official NNS ledger gzip"]
fn native_icp_transfer_from_block_matches_exact_inbound_tuple() {
    let env = setup(false);
    verify(&env, env.index, &env.leg).expect("exact native ledger block proves inbound pull");

    let mut wrong = env.leg.clone();
    wrong.fee = Some(u128::from(FEE + 1));
    assert!(verify(&env, env.index, &wrong).is_err(), "wrong fee must be rejected");
    let mut wrong = env.leg.clone();
    wrong.memo.push(0xff);
    assert!(verify(&env, env.index, &wrong).is_err(), "wrong memo must be rejected");
    let mut wrong = env.leg.clone();
    wrong.created_at_time += 1;
    assert!(verify(&env, env.index, &wrong).is_err(), "wrong timestamp must be rejected");
    let mut wrong = env.leg.clone();
    wrong.to_subaccount = Some([0x24; 32]);
    assert!(verify(&env, env.index, &wrong).is_err(), "wrong destination subaccount must be rejected");
    assert!(verify(&env, env.index + 1000, &env.leg).is_err(), "wrong block index must be rejected");

    // A second principal is separately approved, then performs the pull. Its
    // valid block must fail when checked against the AMM's expected spender.
    let alternate_spender = Principal::self_authenticating(&[76, 77, 78]);
    let timestamp = now_ns(&env.pic) + 10;
    let approve = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: alternate_spender, subaccount: None },
        amount: Nat::from(100_000u64),
        expected_allowance: None,
        expires_at: None,
        fee: Some(Nat::from(FEE)),
        memo: None,
        created_at_time: Some(timestamp),
    };
    let result: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = reply(
        env.pic.update_call(env.ledger, env.user, "icrc2_approve", encode_one(approve).unwrap()).unwrap(),
    );
    result.expect("approve alternate spender for negative proof case");
    let pull = TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: env.user, subaccount: None },
        to: Account { owner: env.amm, subaccount: Some(env.destination_subaccount) },
        amount: Nat::from(20_000u64),
        fee: Some(Nat::from(FEE)),
        memo: Some(env.leg.memo.clone().into()),
        created_at_time: Some(timestamp + 1),
    };
    let pulled: Result<Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> = reply(
        env.pic.update_call(env.ledger, alternate_spender, "icrc2_transfer_from", encode_one(pull).unwrap()).unwrap(),
    );
    let wrong_spender_index: u64 = pulled.expect("alternate spender pull").0.try_into().unwrap();
    let mut wrong_spender_leg = env.leg.clone();
    wrong_spender_leg.amount = 20_000;
    wrong_spender_leg.created_at_time = timestamp + 1;
    assert!(verify(&env, wrong_spender_index, &wrong_spender_leg).is_err(), "wrong spender must be rejected");
}

#[test]
#[ignore = "requires POCKET_IC_BIN and the pinned official NNS ledger gzip"]
fn archived_native_icp_inbound_evidence_remains_held() {
    let env = setup(true);
    for nonce in 0..5u64 {
        let alternate_spender = Principal::self_authenticating(&[80 + nonce as u8]);
        let timestamp = now_ns(&env.pic) + nonce + 100;
        let approve = ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: alternate_spender, subaccount: None },
            amount: Nat::from(100_000u64),
            expected_allowance: None,
            expires_at: None,
            fee: Some(Nat::from(FEE)),
            memo: None,
            created_at_time: Some(timestamp),
        };
        let result: Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError> = reply(
            env.pic.update_call(env.ledger, env.user, "icrc2_approve", encode_one(approve).unwrap()).unwrap(),
        );
        result.expect("approve archive-advancing spender");
        let pull = TransferFromArgs {
            spender_subaccount: None,
            from: Account { owner: env.user, subaccount: None },
            to: Account { owner: env.amm, subaccount: Some(env.destination_subaccount) },
            amount: Nat::from(10_000u64),
            fee: Some(Nat::from(FEE)),
            memo: Some(vec![nonce as u8].into()),
            created_at_time: Some(timestamp + 1),
        };
        let result: Result<Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> = reply(
            env.pic.update_call(env.ledger, alternate_spender, "icrc2_transfer_from", encode_one(pull).unwrap()).unwrap(),
        );
        result.expect("advance ledger archive range");
    }
    assert!(verify(&env, env.index, &env.leg).is_err(), "archived inbound evidence must remain held");
}
