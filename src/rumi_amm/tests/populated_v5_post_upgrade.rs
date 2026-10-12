//! Exercises the release Wasm's real `post_upgrade` path against a frozen,
//! populated V5 snapshot. The predecessor module deliberately has no
//! `pre_upgrade` hook, so it cannot overwrite the injected snapshot.
//!
//! This is an opt-in release test. Set both environment variables to the
//! source-matched production artifact and its reviewed SHA-256 before running.

use candid::{decode_one, encode_args, encode_one, CandidType, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::TransferArg;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::state::{try_decode_state, OutboundPayout};
use rumi_amm::types::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};

const PREDECESSOR_SHA256: &str = "832485e330faa18fd3f7b2d05b3dca670698227e420e255a128b0333c2979fb2";
const CANDIDATE_SHA256: &str = "994ad8efadf9f5fa178f2c824c68d0d200f898d414498745f924d517d61ac2b1";
const ICUSD_LEDGER_PRINCIPAL: &str = "t6bor-paaaa-aaaap-qrd5q-cai";

#[derive(CandidType, Deserialize)]
struct LedgerFeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize)]
struct LedgerArchiveOptions {
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
enum LedgerMetadataValue {
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
    metadata: Vec<(String, LedgerMetadataValue)>,
    initial_balances: Vec<(Account, candid::Nat)>,
    feature_flags: Option<LedgerFeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: LedgerArchiveOptions,
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

/// Frozen pre-journal state shape used by the deployed V5 stable snapshot.
#[derive(CandidType)]
struct LegacyAmmStateV5 {
    admin: Principal,
    pools: BTreeMap<PoolId, Pool>,
    pool_creation_open: bool,
    maintenance_mode: bool,
    pending_claims: Vec<PendingClaim>,
    next_claim_id: u64,
    swap_events: Vec<AmmSwapEvent>,
    next_swap_event_id: u64,
    liquidity_events: Vec<AmmLiquidityEvent>,
    next_liquidity_event_id: u64,
    admin_events: Vec<AmmAdminEvent>,
    next_admin_event_id: u64,
    holder_snapshots: Vec<HolderSnapshot>,
    reward_events: Vec<AmmRewardEvent>,
    next_reward_event_id: u64,
    claim_events: Vec<AmmClaimEvent>,
    next_claim_event_id: u64,
    protocol_backend_principal: Option<Principal>,
    tvl_samples: Vec<TvlSample>,
}

fn reply<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode AMM reply"),
        WasmResult::Reject(reason) => panic!("AMM rejected instead of replying: {reason}"),
    }
}

fn expect_held<T: for<'de> Deserialize<'de> + CandidType>(result: WasmResult, expected: &str) {
    let result: Result<T, AmmError> = reply(result);
    assert!(
        matches!(result, Err(AmmError::InvalidInput { reason }) if reason == expected),
        "production ingress must return its specific held error"
    );
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn deploy_collateral_ledger(
    pic: &PocketIc,
    admin: Principal,
    minting_account: Principal,
    lp: Principal,
    name: &str,
) -> Principal {
    let ledger = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(ledger, 2_000_000_000_000);
    let init = LedgerInitArgs {
        minting_account: Account {
            owner: minting_account,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0_u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: name.to_string(),
        token_symbol: name.to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account {
                owner: lp,
                subaccount: None,
            },
            candid::Nat::from(1_000_000_000_000_u128),
        )],
        feature_flags: Some(LedgerFeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: LedgerArchiveOptions {
            num_blocks_to_archive: 2_000,
            trigger_threshold: 1_000,
            controller_id: admin,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        ledger,
        ledger_wasm(),
        encode_args((LedgerArg::Init(init),)).expect("encode ledger init"),
        Some(admin),
    );
    ledger
}

fn approve_amm(pic: &PocketIc, ledger: Principal, amm: Principal, lp: Principal) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: amm,
            subaccount: None,
        },
        amount: candid::Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(ledger, lp, "icrc2_approve", encode_one(args).unwrap())
        .expect("approve call transport");
    match result {
        WasmResult::Reply(bytes) => {
            let decoded: Result<candid::Nat, icrc_ledger_types::icrc2::approve::ApproveError> =
                decode_one(&bytes).expect("decode approve result");
            decoded.expect("approve AMM");
        }
        WasmResult::Reject(reason) => panic!("approve rejected: {reason}"),
    }
}

fn reward_subaccount(pool_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rumi_amm:rewards:");
    hasher.update(pool_id.as_bytes());
    hasher.finalize().into()
}

fn mint_reward(
    pic: &PocketIc,
    ledger: Principal,
    minting_account: Principal,
    amm: Principal,
    pool_id: &str,
    amount: u128,
) {
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: amm,
            subaccount: Some(reward_subaccount(pool_id)),
        },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(
            ledger,
            minting_account,
            "icrc1_transfer",
            encode_one(args).unwrap(),
        )
        .expect("mint reward transfer call");
    match result {
        WasmResult::Reply(bytes) => {
            let decoded: Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError> =
                decode_one(&bytes).expect("decode reward transfer");
            decoded.expect("mint reward funds");
        }
        WasmResult::Reject(reason) => panic!("reward transfer rejected: {reason}"),
    }
}

fn snapshot() -> (LegacyAmmStateV5, Principal, Principal, String) {
    let admin = Principal::from_text("aaaaa-aa").expect("management principal");
    let lp = Principal::self_authenticating(b"v5-snapshot-lp-a");
    let second_lp = Principal::self_authenticating(b"v5-snapshot-lp-b");
    let token_a = Principal::self_authenticating(b"v5-snapshot-token-a");
    let token_b = Principal::self_authenticating(b"v5-snapshot-token-b");
    let pool_id = "3USD_ICP".to_string();

    let mut lp_shares = BTreeMap::new();
    lp_shares.insert(lp, 60_000);
    lp_shares.insert(second_lp, 40_000);
    let mut lp_rewards = BTreeMap::new();
    lp_rewards.insert(
        lp,
        RewardState {
            reward_debt: 7_500,
            claimable: 125,
        },
    );
    lp_rewards.insert(
        second_lp,
        RewardState {
            reward_debt: 5_000,
            claimable: 75,
        },
    );
    let mut processed_donation_nonces = VecDeque::new();
    processed_donation_nonces.extend([901, 902]);
    let mut pools = BTreeMap::new();
    pools.insert(
        pool_id.clone(),
        Pool {
            token_a,
            token_b,
            reserve_a: 12_345_678,
            reserve_b: 87_654_321,
            fee_bps: 30,
            protocol_fee_bps: 100,
            curve: CurveType::ConstantProduct,
            lp_shares,
            total_lp_shares: 100_000,
            protocol_fees_a: 321,
            protocol_fees_b: 654,
            paused: true,
            subaccount_a: [0x31; 32],
            subaccount_b: [0x32; 32],
            lp_rewards,
            acc_reward_per_share: 125_000_000_000,
            pending_no_lp: 23,
            total_rewards_distributed: 12_500,
            processed_donation_nonces,
            reward_balance_snapshot: 6_789,
        },
    );
    let swap_events = vec![AmmSwapEvent {
        id: 40,
        caller: lp,
        pool_id: pool_id.clone(),
        token_in: token_a,
        amount_in: 1_001,
        token_out: token_b,
        amount_out: 997,
        fee: 3,
        timestamp: 10_000,
    }];
    let liquidity_events = vec![AmmLiquidityEvent {
        id: 41,
        caller: lp,
        pool_id: pool_id.clone(),
        action: AmmLiquidityAction::AddLiquidity,
        token_a,
        amount_a: 12_000,
        token_b,
        amount_b: 24_000,
        lp_shares: 8_000,
        timestamp: 10_001,
    }];
    let admin_events = vec![AmmAdminEvent {
        id: 42,
        caller: admin,
        action: AmmAdminAction::PausePool {
            pool_id: pool_id.clone(),
        },
        timestamp: 10_002,
    }];
    let holder_snapshots = vec![HolderSnapshot {
        token: "icUSD".to_string(),
        timestamp: 10_003,
        holder_count: 2,
        total_supply: 5_000_000,
        top_holders: vec![
            HolderEntry {
                holder: lp,
                balance: 3_000_000,
            },
            HolderEntry {
                holder: second_lp,
                balance: 2_000_000,
            },
        ],
    }];
    let reward_events = vec![AmmRewardEvent {
        id: 43,
        pool_id: pool_id.clone(),
        amount: 12_500,
        total_shares_at_time: 100_000,
        nonce: 902,
        timestamp: 10_004,
    }];
    let claim_events = vec![AmmClaimEvent {
        id: 44,
        pool_id: pool_id.clone(),
        claimant: lp,
        amount: 125,
        timestamp: 10_005,
    }];
    let tvl_samples = vec![TvlSample {
        pool_id: pool_id.clone(),
        timestamp: 10_006,
        reserve_a: 12_345_678,
        reserve_b: 87_654_321,
        price_a_e8s: 100_000_000,
        price_b_e8s: 25_000_000,
        tvl_usd_e8s: 34_259_258,
    }];

    (
        LegacyAmmStateV5 {
            admin,
            pools,
            pool_creation_open: true,
            maintenance_mode: true,
            pending_claims: vec![PendingClaim {
                id: 8,
                pool_id: "legacy_pool".to_string(),
                claimant: admin,
                token: admin,
                subaccount: [3; 32],
                amount: 1_234,
                reason: "legacy unresolved transfer".to_string(),
                created_at: 99,
            }],
            next_claim_id: 9,
            swap_events,
            next_swap_event_id: 41,
            liquidity_events,
            next_liquidity_event_id: 42,
            admin_events,
            next_admin_event_id: 43,
            holder_snapshots,
            reward_events,
            next_reward_event_id: 44,
            claim_events,
            next_claim_event_id: 45,
            protocol_backend_principal: Some(admin),
            tvl_samples,
        },
        admin,
        lp,
        pool_id,
    )
}

#[test]
#[ignore = "requires source-matched RUMI_AMM_PRODUCTION_WASM and reviewed SHA-256"]
fn production_post_upgrade_preserves_populated_v5_snapshot() {
    let wasm_path = std::env::var("RUMI_AMM_PRODUCTION_WASM")
        .expect("set RUMI_AMM_PRODUCTION_WASM to the source-matched upload artifact");
    let expected_sha = std::env::var("RUMI_AMM_PRODUCTION_WASM_SHA256")
        .expect("set RUMI_AMM_PRODUCTION_WASM_SHA256 to the reviewed upload hash");
    let wasm = std::fs::read(&wasm_path).expect("read production AMM Wasm");
    assert_eq!(
        format!("{:x}", Sha256::digest(&wasm)),
        expected_sha,
        "wrong production AMM artifact"
    );

    let (legacy, admin, lp, pool_id) = snapshot();
    let caller = Principal::self_authenticating(b"v5-snapshot-caller");
    let nonexistent_ledger = Principal::self_authenticating(b"v5-snapshot-no-ledger");
    let legacy_bytes = candid::encode_one(&legacy).expect("encode frozen V5 state");
    let mut stable_memory = Vec::with_capacity(8 + legacy_bytes.len());
    stable_memory.extend_from_slice(&(legacy_bytes.len() as u64).to_le_bytes());
    stable_memory.extend_from_slice(&legacy_bytes);

    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    // An empty, valid Wasm module has no pre_upgrade export to rewrite the
    // frozen V5 bytes before the candidate's post_upgrade hook reads them.
    pic.install_canister(amm, b"\0asm\x01\0\0\0".to_vec(), vec![], Some(admin));
    pic.set_stable_memory(
        amm,
        stable_memory,
        pocket_ic::common::rest::BlobCompression::NoCompression,
    );
    pic.upgrade_canister(
        amm,
        wasm,
        encode_one(AmmInitArgs { admin }).expect("encode upgrade init args"),
        Some(admin),
    )
    .expect("production Wasm must upgrade and run post_upgrade on populated V5 state");

    let pools: Vec<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pools", encode_args(()).unwrap())
            .unwrap(),
    );
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].pool_id, pool_id);
    assert_eq!(
        (pools[0].reserve_a, pools[0].reserve_b),
        (12_345_678, 87_654_321)
    );
    assert_eq!(pools[0].total_lp_shares, 100_000);
    assert!(pools[0].paused);
    assert_eq!(
        reply::<u128>(
            pic.query_call(
                amm,
                admin,
                "get_lp_balance",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        60_000
    );
    assert_eq!(
        reply::<candid::Nat>(
            pic.query_call(
                amm,
                admin,
                "get_pending_rewards",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        candid::Nat::from(125_u64)
    );
    assert_eq!(
        reply::<candid::Nat>(
            pic.query_call(
                amm,
                admin,
                "get_pending_rewards",
                encode_args((
                    pool_id.clone(),
                    Principal::self_authenticating(b"v5-snapshot-lp-b"),
                ))
                .unwrap(),
            )
            .unwrap(),
        ),
        candid::Nat::from(75_u64)
    );
    assert!(reply::<bool>(
        pic.query_call(amm, admin, "is_maintenance_mode", encode_args(()).unwrap())
            .unwrap()
    ));

    let swap_request = vec![7_u8; 32];
    let add_request = vec![8_u8; 32];
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap",
            encode_args((pool_id.clone(), nonexistent_ledger, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy swap is disabled; use swap_v2 with a durable request ID",
    );
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap_v2",
            encode_args((
                swap_request.clone(),
                pool_id.clone(),
                nonexistent_ledger,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "swap_v2 is held pending effect-then-error and upgrade lifecycle verification",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity",
            encode_args((pool_id.clone(), 100_u128, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy add_liquidity is disabled; use add_liquidity_v2 with a durable request ID",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity_v2",
            encode_args((
                add_request.clone(),
                pool_id.clone(),
                100_u128,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "add_liquidity_v2 remains held until both inbound legs have durable phase replay",
    );

    let claims: Vec<PendingClaim> = reply(
        pic.query_call(amm, admin, "get_pending_claims", encode_args(()).unwrap())
            .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&claims).unwrap(),
        candid::encode_one(&legacy.pending_claims).unwrap(),
        "legacy claim liability must remain exactly preserved"
    );
    let swaps: Vec<AmmSwapEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_swap_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&swaps).unwrap(),
        candid::encode_one(&legacy.swap_events).unwrap()
    );
    let liquidity: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_liquidity_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&liquidity).unwrap(),
        candid::encode_one(&legacy.liquidity_events).unwrap()
    );
    let admins: Vec<AmmAdminEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_admin_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&admins).unwrap(),
        candid::encode_one(&legacy.admin_events).unwrap()
    );
    let snapshots: Vec<HolderSnapshot> = reply(
        pic.query_call(
            amm,
            admin,
            "get_holder_snapshots",
            encode_args(("icUSD".to_string(), 0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&snapshots).unwrap(),
        candid::encode_one(&legacy.holder_snapshots).unwrap()
    );

    let held_claim: Result<(), AmmError> = reply(
        pic.update_call(amm, admin, "claim_pending", encode_args((8_u64,)).unwrap())
            .unwrap(),
    );
    assert!(matches!(
        held_claim,
        Err(AmmError::InvalidInput { reason })
            if reason == "legacy pending claim has no bound outbound payout identity and is held for reconciliation"
    ));
    for request_id in [swap_request, add_request] {
        let status: Result<rumi_amm::state::InboundOperationStatus, AmmError> = reply(
            pic.query_call(
                amm,
                caller,
                "get_inbound_operation",
                encode_args((request_id,)).unwrap(),
            )
            .unwrap(),
        );
        assert!(matches!(
            status,
            Err(AmmError::InvalidInput { reason }) if reason == "inbound operation not found"
        ));
    }
    let next_sequence: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        next_sequence.unwrap(),
        1,
        "held ingress and V5 upgrade leave the inbound journal empty"
    );
    let payouts: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert!(
        payouts.unwrap().is_empty(),
        "held ingress and V5 upgrade leave the payout journal empty"
    );
}

/// Unlike the injected V5 fixture above, this path runs the exact predecessor
/// Wasm's pre_upgrade hook after building a real pool, LP position, and reward
/// state through its ledger-backed public endpoints.
#[test]
#[ignore = "requires exact installed predecessor and reviewed candidate Wasm artifacts"]
fn installed_predecessor_populated_state_survives_candidate_upgrade() {
    let predecessor_path = std::env::var("RUMI_AMM_PREDECESSOR_WASM")
        .expect("set RUMI_AMM_PREDECESSOR_WASM to the installed predecessor artifact");
    let candidate_path = std::env::var("RUMI_AMM_PRODUCTION_WASM")
        .expect("set RUMI_AMM_PRODUCTION_WASM to the reviewed candidate artifact");
    let predecessor_wasm = std::fs::read(predecessor_path).expect("read predecessor Wasm");
    let candidate_wasm = std::fs::read(candidate_path).expect("read candidate Wasm");
    assert_eq!(
        format!("{:x}", Sha256::digest(&predecessor_wasm)),
        PREDECESSOR_SHA256,
        "predecessor must match the currently installed AMM module"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&candidate_wasm)),
        CANDIDATE_SHA256,
        "candidate must match the reviewed release Wasm"
    );

    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(b"predecessor-upgrade-admin");
    let lp = Principal::self_authenticating(b"predecessor-upgrade-lp");
    let minting_account = Principal::self_authenticating(b"predecessor-upgrade-mint");
    let token_a = deploy_collateral_ledger(&pic, admin, minting_account, lp, "3USD");
    let token_b = deploy_collateral_ledger(&pic, admin, minting_account, lp, "ICP");

    let icusd_id = Principal::from_text(ICUSD_LEDGER_PRINCIPAL).expect("icUSD ledger principal");
    let icusd_ledger = pic
        .create_canister_with_id(Some(admin), None, icusd_id)
        .expect("create test icUSD ledger at AMM's fixed ledger ID");
    pic.add_cycles(icusd_ledger, 2_000_000_000_000);
    let icusd_init = LedgerInitArgs {
        minting_account: Account {
            owner: minting_account,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: candid::Nat::from(0_u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "icUSD".to_string(),
        token_symbol: "icUSD".to_string(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(LedgerFeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: LedgerArchiveOptions {
            num_blocks_to_archive: 2_000,
            trigger_threshold: 1_000,
            controller_id: admin,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        icusd_ledger,
        ledger_wasm(),
        encode_args((LedgerArg::Init(icusd_init),)).unwrap(),
        Some(admin),
    );

    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(
        amm,
        predecessor_wasm,
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    );
    approve_amm(&pic, token_a, amm, lp);
    approve_amm(&pic, token_b, amm, lp);

    let pool_id: String = reply::<Result<String, AmmError>>(
        pic.update_call(
            amm,
            admin,
            "create_pool",
            encode_one(CreatePoolArgs {
                token_a,
                token_b,
                fee_bps: 30,
                curve: CurveType::ConstantProduct,
            })
            .unwrap(),
        )
        .unwrap(),
    )
    .expect("predecessor creates pool through public update endpoint");

    let minted_shares: u128 = reply::<Result<u128, AmmError>>(
        pic.update_call(
            amm,
            lp,
            "add_liquidity",
            encode_args((
                pool_id.clone(),
                1_000_000_000_u128,
                1_000_000_000_u128,
                0_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
    )
    .expect("predecessor adds ledger-backed liquidity");
    assert!(minted_shares > 0);

    reply::<Result<(), AmmError>>(
        pic.update_call(
            amm,
            admin,
            "set_protocol_backend_principal",
            encode_one(admin).unwrap(),
        )
        .unwrap(),
    )
    .expect("configure reward notifier principal");
    let reward_amount = 10_000_000_000_u128;
    mint_reward(
        &pic,
        icusd_ledger,
        minting_account,
        amm,
        &pool_id,
        reward_amount,
    );
    reply::<Result<(), AmmError>>(
        pic.update_call(
            amm,
            admin,
            "notify_reward_received",
            encode_args((pool_id.clone(), reward_amount, 701_u64)).unwrap(),
        )
        .unwrap(),
    )
    .expect("record reward through predecessor endpoint");

    let pools_before: Vec<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pools", encode_args(()).unwrap())
            .unwrap(),
    );
    let shares_before: u128 = reply(
        pic.query_call(
            amm,
            admin,
            "get_lp_balance",
            encode_args((pool_id.clone(), lp)).unwrap(),
        )
        .unwrap(),
    );
    let rewards_before: candid::Nat = reply(
        pic.query_call(
            amm,
            admin,
            "get_pending_rewards",
            encode_args((pool_id.clone(), lp)).unwrap(),
        )
        .unwrap(),
    );
    let liquidity_before: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_liquidity_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    let admin_events_before: Vec<AmmAdminEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_admin_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(pools_before.len(), 1);
    assert!(pools_before[0].reserve_a > 0 && pools_before[0].reserve_b > 0);
    assert!(shares_before > 0 && rewards_before.0 > candid::Nat::from(0_u8).0);
    assert_eq!(liquidity_before.len(), 1);

    // The installed predecessor—not a test fixture—runs pre_upgrade here and
    // writes its in-memory state to stable memory before candidate post_upgrade.
    pic.upgrade_canister(
        amm,
        candidate_wasm,
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    )
    .expect("candidate upgrades the exact installed predecessor");

    let pools_after: Vec<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pools", encode_args(()).unwrap())
            .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&pools_after).unwrap(),
        candid::encode_one(&pools_before).unwrap(),
        "pool reserves and total LP supply survive the real upgrade"
    );
    assert_eq!(
        reply::<u128>(
            pic.query_call(
                amm,
                admin,
                "get_lp_balance",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        shares_before
    );
    assert_eq!(
        reply::<candid::Nat>(
            pic.query_call(
                amm,
                admin,
                "get_pending_rewards",
                encode_args((pool_id.clone(), lp)).unwrap(),
            )
            .unwrap(),
        ),
        rewards_before
    );
    let liquidity_after: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_liquidity_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&liquidity_after).unwrap(),
        candid::encode_one(&liquidity_before).unwrap()
    );
    let admin_events_after: Vec<AmmAdminEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_admin_events",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        candid::encode_one(&admin_events_after).unwrap(),
        candid::encode_one(&admin_events_before).unwrap()
    );

    // Read the predecessor's emitted snapshot back from stable memory and
    // inspect fields which the public Candid interface does not expose.
    let stable = pic.get_stable_memory(amm);
    assert!(
        stable.len() >= 8,
        "predecessor must persist a length prefix"
    );
    let len = u64::from_le_bytes(stable[..8].try_into().unwrap()) as usize;
    assert!(len > 0 && len <= stable.len() - 8);
    let persisted = try_decode_state(&stable[8..8 + len])
        .expect("the actual predecessor snapshot must decode on the host");
    let persisted_pool = persisted.pools.get(&pool_id).expect("persisted pool");
    assert_eq!(persisted_pool.reserve_a, pools_before[0].reserve_a);
    assert_eq!(persisted_pool.reserve_b, pools_before[0].reserve_b);
    assert_eq!(persisted_pool.lp_shares.get(&lp), Some(&shares_before));
    assert!(persisted_pool.acc_reward_per_share > 0);
    assert_eq!(persisted_pool.total_rewards_distributed, reward_amount);
    assert_eq!(persisted.reward_events.len(), 1);
    assert_eq!(persisted.next_reward_event_id, 1);

    let caller = Principal::self_authenticating(b"post-upgrade-held-caller");
    let absent_ledger = Principal::self_authenticating(b"post-upgrade-no-ledger");
    let swap_request = vec![0x71_u8; 32];
    let add_request = vec![0x72_u8; 32];
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap",
            encode_args((pool_id.clone(), absent_ledger, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy swap is disabled; use swap_v2 with a durable request ID",
    );
    expect_held::<SwapResult>(
        pic.update_call(
            amm,
            caller,
            "swap_v2",
            encode_args((
                swap_request.clone(),
                pool_id.clone(),
                absent_ledger,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "swap_v2 is held pending effect-then-error and upgrade lifecycle verification",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity",
            encode_args((pool_id.clone(), 100_u128, 100_u128, 1_u128)).unwrap(),
        )
        .unwrap(),
        "legacy add_liquidity is disabled; use add_liquidity_v2 with a durable request ID",
    );
    expect_held::<u128>(
        pic.update_call(
            amm,
            caller,
            "add_liquidity_v2",
            encode_args((
                add_request.clone(),
                pool_id.clone(),
                100_u128,
                100_u128,
                1_u128,
            ))
            .unwrap(),
        )
        .unwrap(),
        "add_liquidity_v2 remains held until both inbound legs have durable phase replay",
    );
    for request_id in [swap_request, add_request] {
        let status: Result<rumi_amm::state::InboundOperationStatus, AmmError> = reply(
            pic.query_call(
                amm,
                caller,
                "get_inbound_operation",
                encode_args((request_id,)).unwrap(),
            )
            .unwrap(),
        );
        assert!(matches!(
            status,
            Err(AmmError::InvalidInput { reason }) if reason == "inbound operation not found"
        ));
    }
    let next_sequence: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(next_sequence.unwrap(), 1);
    let payouts: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0_u64, 100_u64)).unwrap(),
        )
        .unwrap(),
    );
    assert!(payouts.unwrap().is_empty());
}
