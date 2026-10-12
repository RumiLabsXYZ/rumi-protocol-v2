//! Exercises a real native ICP payout through the AMM's transfer callback.
//! The PocketIC-only callback hook traps after the official ledger applies the
//! transfer but before the AMM records its block index. The test then upgrades
//! that durable state to the exact production candidate and recovers by public
//! endpoint using the ledger's exact native block.

use candid::{decode_one, encode_args, encode_one, CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use pocket_ic::{ErrorCode, PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::{
    state::{OutboundPayout, OutboundPayoutStatus},
    types::{AmmError, AmmInitArgs, CreatePoolArgs, CurveType, PoolInfo},
};
use serde::Deserialize;
use sha2::{Digest, Sha224, Sha256};
use std::{process::Command, time::UNIX_EPOCH};

const ICP_LEDGER: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const OFFICIAL_LEDGER_GZIP_SHA256: &str =
    "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";
const CANDIDATE_AMM_SHA256: &str =
    "1f3fddd44fb7832585c9a45d0b46ef607805381531c71f71a8d517a7eddabf6a";
const POCKETIC_TEST_AMM_SHA256: &str =
    "d21a0123e8e27e3831ded245679f0cf61c67a0cae81673a624e33639a5bc656b";
const FEE: u64 = 10_000;

#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksArgs {
    start: u64,
    length: u64,
}
#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksResponse {
    chain_length: u64,
}

#[derive(CandidType, Deserialize)]
struct FeatureFlags {
    icrc2: bool,
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

fn official_ledger_wasm() -> Vec<u8> {
    let path = std::env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .unwrap_or_else(|_| "/private/tmp/rumi-nns-ledger-69b755.wasm.gz".into());
    let compressed = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&compressed)),
        OFFICIAL_LEDGER_GZIP_SHA256
    );
    let output = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip");
    assert!(output.status.success(), "gzip failed");
    output.stdout
}

fn account_identifier_hex(owner: Principal, subaccount: Option<[u8; 32]>) -> String {
    let mut hash = Sha224::new();
    hash.update(b"\x0Aaccount-id");
    hash.update(owner.as_slice());
    hash.update(subaccount.unwrap_or([0; 32]));
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

fn balance(
    pic: &PocketIc,
    ledger: Principal,
    owner: Principal,
    subaccount: Option<[u8; 32]>,
) -> Nat {
    reply(
        pic.query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(Account { owner, subaccount }).unwrap(),
        )
        .unwrap(),
    )
}

fn native_ledger_chain_length(pic: &PocketIc, ledger: Principal) -> u64 {
    let response: NativeQueryBlocksResponse = reply(
        pic.query_call(
            ledger,
            Principal::anonymous(),
            "query_blocks",
            encode_one(NativeQueryBlocksArgs {
                start: 0,
                length: 1,
            })
            .unwrap(),
        )
        .unwrap(),
    );
    response.chain_length
}

fn wasm_from_env(path_key: &str, sha_key: &str, expected_sha: Option<&str>) -> Vec<u8> {
    let path = std::env::var(path_key).unwrap_or_else(|_| panic!("set {path_key}"));
    let expected = std::env::var(sha_key).unwrap_or_else(|_| panic!("set {sha_key}"));
    if let Some(pin) = expected_sha {
        assert_eq!(expected, pin, "wrong reviewed Wasm pin");
    }
    let wasm = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert_eq!(format!("{:x}", Sha256::digest(&wasm)), expected);
    wasm
}

#[test]
#[ignore = "requires PocketIC, pinned official NNS ledger gzip, feature test Wasm, and exact production AMM Wasm"]
fn callback_trap_recovers_exact_native_icp_receipt_once_after_upgrade() {
    let test_wasm = wasm_from_env(
        "RUMI_AMM_POCKETIC_TEST_WASM",
        "RUMI_AMM_POCKETIC_TEST_WASM_SHA256",
        Some(POCKETIC_TEST_AMM_SHA256),
    );
    let production_wasm = wasm_from_env(
        "RUMI_AMM_PRODUCTION_WASM",
        "RUMI_AMM_PRODUCTION_WASM_SHA256",
        Some(CANDIDATE_AMM_SHA256),
    );
    let official_wasm = official_ledger_wasm();
    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let non_admin = Principal::self_authenticating(&[84, 85, 86]);
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    let ledger = pic
        .create_canister_with_id(Some(admin), None, Principal::from_text(ICP_LEDGER).unwrap())
        .unwrap();
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    pic.set_time(UNIX_EPOCH + std::time::Duration::from_nanos(1_711_324_800_000_000_000));
    let created_at_time = 1_711_324_800_000_000_000;
    let ledger_init = LedgerArg::Init(LedgerInit {
        minting_account: account_identifier_hex(Principal::management_canister(), None),
        icrc1_minting_account: Some(Account {
            owner: Principal::management_canister(),
            subaccount: None,
        }),
        initial_values: vec![(
            account_identifier_hex(admin, None),
            Tokens { e8s: 2_000_000 },
        )],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(ArchiveOptions {
            num_blocks_to_archive: 1,
            max_transactions_per_response: Some(100),
            trigger_threshold: 1_000,
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
        official_wasm,
        encode_args((ledger_init,)).unwrap(),
        Some(admin),
    );

    // The pool and payout liability are created through AMM code. Only the
    // reserve and fee amounts are seeded by existing PocketIC fixture controls.
    pic.install_canister(
        amm,
        test_wasm,
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    );
    let placeholder = Principal::self_authenticating(b"placeholder-token");
    let created: Result<String, AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "create_pool",
            encode_one(CreatePoolArgs {
                token_a: ledger,
                token_b: placeholder,
                fee_bps: 30,
                curve: CurveType::ConstantProduct,
            })
            .unwrap(),
        )
        .unwrap(),
    );
    let pool_id = created.expect("create fixture pool");
    let seed: Result<([u8; 32], [u8; 32]), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_pool",
            encode_args((pool_id.clone(), 500_000u128, 500_000u128)).unwrap(),
        )
        .unwrap(),
    );
    let (sub_a, sub_b) = seed.expect("seed fixture reserves");
    let pool: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool_id.clone()).unwrap())
            .unwrap(),
    );
    let pool = pool.expect("pool exists");
    let native_subaccount = if pool.token_a == ledger { sub_a } else { sub_b };
    let (fees_a, fees_b) = if pool.token_a == ledger {
        (100_000u128, 0u128)
    } else {
        (0u128, 100_000u128)
    };
    let seeded: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_protocol_fees",
            encode_args((pool_id.clone(), fees_a, fees_b)).unwrap(),
        )
        .unwrap(),
    );
    seeded.expect("seed native protocol fee liability");

    // Block 0 funds the pool; the AMM's native ICP payout is block 1.
    let funding = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: amm,
            subaccount: Some(native_subaccount),
        },
        amount: Nat::from(1_000_000u64),
        fee: Some(Nat::from(FEE)),
        memo: None,
        created_at_time: Some(created_at_time),
    };
    let funding_block: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = reply(
        pic.update_call(
            ledger,
            admin,
            "icrc1_transfer",
            encode_one(funding).unwrap(),
        )
        .unwrap(),
    );
    let funding_block_index: u64 = funding_block
        .expect("fund pool subaccount")
        .0
        .try_into()
        .expect("funding block index fits u64");

    let unauthorized: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            non_admin,
            "pocketic_trap_next_native_payout_callback",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(unauthorized, Err(AmmError::Unauthorized)));
    let armed: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_trap_next_native_payout_callback",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    armed.expect("admin arms one-shot callback trap");

    let payout_block_index = native_ledger_chain_length(&pic, ledger);
    assert_eq!(payout_block_index, funding_block_index + 1);
    let user_before = balance(&pic, ledger, admin, None);
    let source_before = balance(&pic, ledger, amm, Some(native_subaccount));
    let withdraw = pic.update_call(
        amm,
        admin,
        "withdraw_protocol_fees",
        encode_one(pool_id.clone()).unwrap(),
    );
    match withdraw {
        Ok(WasmResult::Reject(reason))
            if reason.contains(
                "PocketIC-only trap after successful native ICP ledger transfer callback",
            ) => {}
        Ok(WasmResult::Reply(_)) => panic!("callback trap unexpectedly replied successfully"),
        Ok(WasmResult::Reject(reason)) => panic!("unexpected reject: {reason}"),
        Err(error)
            if error.code == ErrorCode::CanisterCalledTrap
                && error.description.contains(
                    "PocketIC-only trap after successful native ICP ledger transfer callback",
                ) => {}
        Err(error) => panic!("unexpected PocketIC update error: {error:?}"),
    }
    assert_eq!(
        native_ledger_chain_length(&pic, ledger),
        payout_block_index + 1,
        "the official ledger committed exactly one block before the callback trapped"
    );
    let user_after_transfer = balance(&pic, ledger, admin, None);
    let source_after_transfer = balance(&pic, ledger, amm, Some(native_subaccount));
    assert_eq!(
        user_after_transfer.0.clone() - user_before.0.clone(),
        Nat::from(90_000u64).0
    );
    assert_eq!(
        source_before.0.clone() - source_after_transfer.0.clone(),
        Nat::from(100_000u64).0
    );

    let held: Vec<OutboundPayout> = reply(
        pic.query_call(
            amm,
            admin,
            "pocketic_get_outbound_payouts",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(held.len(), 1, "the callback trap retains one payout intent");
    assert_eq!(held[0].status, OutboundPayoutStatus::Dispatched);
    assert_eq!(held[0].ledger, ledger);
    assert_eq!(held[0].from_subaccount, Some(native_subaccount));
    assert_eq!(held[0].gross_amount, 100_000);
    assert_eq!(held[0].net_amount, 90_000);

    pic.upgrade_canister(
        amm,
        production_wasm,
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    )
    .expect("upgrade test module state to exact production candidate");

    let unresolved: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0u64, 10u64)).unwrap(),
        )
        .unwrap(),
    );
    let unresolved = unresolved.expect("public admin query reads the upgraded liability");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].id, held[0].id);
    assert_eq!(unresolved[0].status, OutboundPayoutStatus::Dispatched);

    let recover = |block: u64| -> Result<(), AmmError> {
        reply(
            pic.update_call(
                amm,
                admin,
                "recover_outbound_payout",
                encode_args((held[0].id, Some(block))).unwrap(),
            )
            .unwrap(),
        )
    };
    assert!(
        recover(funding_block_index).is_err(),
        "the pool-funding block has the wrong tuple"
    );
    assert!(
        recover(payout_block_index + 1).is_err(),
        "a missing block is rejected"
    );
    recover(payout_block_index).expect("exact direct native ICP receipt finalizes payout");
    assert_eq!(
        balance(&pic, ledger, admin, None),
        user_after_transfer,
        "recovery records the existing receipt without a second recipient credit"
    );
    assert_eq!(
        balance(&pic, ledger, amm, Some(native_subaccount)),
        source_after_transfer,
        "recovery makes no second debit from the AMM subaccount"
    );
    assert!(
        recover(payout_block_index).is_err(),
        "a finalized payout cannot recover twice"
    );
}
