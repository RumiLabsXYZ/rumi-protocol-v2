//! Exercises the production `recover_outbound_payout` endpoint against the
//! pinned official NNS ICP ledger. The payout is seeded as an ambiguous
//! liability in stable memory because a deterministic official-ledger fixture
//! cannot simulate a lost inter-canister callback after committing a transfer.
//! Recovery itself, block verification, and finalization all run through the
//! candidate production Wasm; no test-only AMM endpoint is used.

use candid::{decode_one, encode_args, encode_one, CandidType, Encode, Nat, Principal};
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::{
    state::{
        AmmState, OutboundPayout, OutboundPayoutLeg, OutboundPayoutLink, OutboundPayoutPurpose,
        OutboundPayoutStatus,
    },
    types::{AmmAdminAction, AmmAdminEvent, AmmInitArgs, CurveType, Pool},
};
use serde::Deserialize;
use sha2::{Digest, Sha224, Sha256};
use std::{collections::BTreeMap, process::Command, time::UNIX_EPOCH};

const ICP_LEDGER: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const OFFICIAL_LEDGER_GZIP_SHA256: &str =
    "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";
const CANDIDATE_AMM_SHA256: &str =
    "994ad8efadf9f5fa178f2c824c68d0d200f898d414498745f924d517d61ac2b1";
const FEE: u64 = 10_000;
const POOL_ID: &str = "ICP_RECOVERY_TEST";

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
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

fn transfer(
    pic: &PocketIc,
    ledger: Principal,
    amm: Principal,
    from_subaccount: [u8; 32],
    to: Principal,
    amount: u64,
    memo: Vec<u8>,
    created_at_time: u64,
) -> u64 {
    let arg = TransferArg {
        from_subaccount: Some(from_subaccount),
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount: Nat::from(amount),
        fee: Some(Nat::from(FEE)),
        memo: Some(memo.into()),
        created_at_time: Some(created_at_time),
    };
    let result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = reply(
        pic.update_call(ledger, amm, "icrc1_transfer", encode_one(arg).unwrap())
            .unwrap(),
    );
    result
        .expect("official ledger transfer")
        .0
        .try_into()
        .expect("block index fits u64")
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

#[test]
#[ignore = "requires PocketIC, pinned official NNS ledger gzip, and exact production AMM Wasm"]
fn public_recovery_accepts_only_the_exact_native_icp_receipt_once() {
    let wasm_path = std::env::var("RUMI_AMM_PRODUCTION_WASM").expect("set production Wasm path");
    let expected_sha =
        std::env::var("RUMI_AMM_PRODUCTION_WASM_SHA256").expect("set reviewed Wasm SHA-256");
    assert_eq!(
        expected_sha, CANDIDATE_AMM_SHA256,
        "test is pinned to the reviewed candidate"
    );
    let wasm = std::fs::read(&wasm_path).unwrap_or_else(|e| panic!("read {wasm_path}: {e}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&wasm)),
        expected_sha,
        "wrong production AMM artifact"
    );

    let pic = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let wrong_recipient = Principal::self_authenticating(&[84, 85, 86]);
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    let ledger = pic
        .create_canister_with_id(Some(admin), None, Principal::from_text(ICP_LEDGER).unwrap())
        .unwrap();
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    pic.set_time(UNIX_EPOCH + std::time::Duration::from_nanos(1_711_324_800_000_000_000));

    let pool_subaccount = [0x3c; 32];
    let payout_memo = vec![0x52, 0x55, 0x4d, 0x49, 0x50];
    let created_at_time = 1_711_324_800_000_000_000;
    let gross_amount = 100_000u128;
    let net_amount = gross_amount - u128::from(FEE);
    let ledger_init = LedgerArg::Init(LedgerInit {
        minting_account: account_identifier_hex(Principal::management_canister(), None),
        icrc1_minting_account: Some(Account {
            owner: Principal::management_canister(),
            subaccount: None,
        }),
        initial_values: vec![(
            account_identifier_hex(amm, Some(pool_subaccount)),
            Tokens { e8s: 1_000_000 },
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
        official_ledger_wasm(),
        encode_args((ledger_init,)).unwrap(),
        Some(admin),
    );

    // Block 0 has the right ledger and sender but the wrong recipient/amount.
    // Block 1 is the exact committed fee-withdrawal payout tuple.
    let wrong_block = transfer(
        &pic,
        ledger,
        amm,
        pool_subaccount,
        wrong_recipient,
        50_000,
        vec![0x01],
        created_at_time - 1,
    );
    let exact_block = transfer(
        &pic,
        ledger,
        amm,
        pool_subaccount,
        admin,
        net_amount as u64,
        payout_memo.clone(),
        created_at_time,
    );

    let pool = Pool {
        token_a: ledger,
        token_b: Principal::self_authenticating(b"placeholder-token"),
        reserve_a: 500_000,
        reserve_b: 500_000,
        fee_bps: 30,
        protocol_fee_bps: 0,
        curve: CurveType::ConstantProduct,
        lp_shares: BTreeMap::from([(admin, 1_000)]),
        total_lp_shares: 1_000,
        protocol_fees_a: 0,
        protocol_fees_b: 0,
        paused: true,
        subaccount_a: pool_subaccount,
        subaccount_b: [0x4d; 32],
        lp_rewards: BTreeMap::new(),
        acc_reward_per_share: 0,
        pending_no_lp: 0,
        total_rewards_distributed: 0,
        processed_donation_nonces: Default::default(),
        reward_balance_snapshot: 0,
    };
    let payout = OutboundPayout {
        id: 7,
        operation_id: format!("protocol_fee_a:{POOL_ID}"),
        ledger,
        from: amm,
        from_subaccount: Some(pool_subaccount),
        to: admin,
        to_subaccount: None,
        gross_amount,
        net_amount,
        fee: u128::from(FEE),
        memo: payout_memo,
        created_at_time,
        status: OutboundPayoutStatus::Ambiguous,
    };
    let mut state = AmmState {
        admin,
        ..AmmState::default()
    };
    state.pools.insert(POOL_ID.into(), pool);
    state.admin_events.push(AmmAdminEvent {
        id: 0,
        caller: admin,
        action: AmmAdminAction::WithdrawProtocolFees {
            pool_id: POOL_ID.into(),
            amount_a: gross_amount,
            amount_b: 0,
        },
        timestamp: created_at_time,
    });
    state.next_admin_event_id = 1;
    state.outbound_payouts.push(payout);
    state.outbound_payout_links.push(OutboundPayoutLink {
        payout_id: 7,
        purpose: OutboundPayoutPurpose::WithdrawProtocolFees {
            pool_id: POOL_ID.into(),
            admin,
            leg: OutboundPayoutLeg::TokenA,
            gross_amount,
        },
    });
    state.next_outbound_payout_id = 8;
    let encoded_state = Encode!(&state).expect("encode candidate state");
    let mut stable_memory = Vec::with_capacity(8 + encoded_state.len());
    stable_memory.extend_from_slice(&(encoded_state.len() as u64).to_le_bytes());
    stable_memory.extend_from_slice(&encoded_state);

    // The candidate's post_upgrade reads the injected durable ambiguity. The
    // recovery call below is the public production endpoint, not a test hook.
    pic.install_canister(amm, b"\0asm\x01\0\0\0".to_vec(), vec![], Some(admin));
    pic.set_stable_memory(
        amm,
        stable_memory,
        pocket_ic::common::rest::BlobCompression::NoCompression,
    );
    pic.upgrade_canister(
        amm,
        wasm,
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    )
    .expect("candidate restores seeded ambiguous fee payout");

    let recover = |block: u64| -> Result<(), rumi_amm::types::AmmError> {
        reply(
            pic.update_call(
                amm,
                admin,
                "recover_outbound_payout",
                encode_args((7u64, Some(block))).unwrap(),
            )
            .unwrap(),
        )
    };
    let held = || -> Vec<OutboundPayout> {
        let result: Result<Vec<OutboundPayout>, rumi_amm::types::AmmError> = reply(
            pic.query_call(
                amm,
                admin,
                "get_unresolved_outbound_payouts",
                encode_args((0u64, 100u64)).unwrap(),
            )
            .unwrap(),
        );
        result.expect("admin can inspect held native payout")
    };

    assert!(
        recover(wrong_block).is_err(),
        "wrong native block tuple must be rejected"
    );
    assert_eq!(
        held().len(),
        1,
        "wrong tuple leaves the payout liability held"
    );
    assert!(
        recover(exact_block + 1).is_err(),
        "missing block must be rejected"
    );
    assert_eq!(
        held().len(),
        1,
        "missing block leaves the payout liability held"
    );

    let user_balance_before = balance(&pic, ledger, admin, None);
    let source_balance_before = balance(&pic, ledger, amm, Some(pool_subaccount));
    recover(exact_block).expect("exact direct native ICP receipt finalizes payout");
    assert!(
        held().is_empty(),
        "exact receipt retires the held liability"
    );
    assert_eq!(
        balance(&pic, ledger, admin, None),
        user_balance_before,
        "recovery records the existing receipt and never transfers to the recipient again"
    );
    assert_eq!(
        balance(&pic, ledger, amm, Some(pool_subaccount)),
        source_balance_before,
        "recovery makes no second debit from the AMM subaccount"
    );
    assert!(
        recover(exact_block).is_err(),
        "a finalized payout cannot be recovered twice"
    );
}
