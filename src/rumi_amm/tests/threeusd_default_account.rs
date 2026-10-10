use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha256};

use rumi_amm::types::{AmmError, AmmInitArgs, CreatePoolArgs, CurveType, PoolInfo};

const THREEPOOL_LEDGER: &str = "fohh4-yyaaa-aaaap-qtkpa-cai";
const INITIAL_BALANCE: u128 = 1_000_000_000_000_000;
const DEPOSIT: u128 = 10_000_000_000;

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
enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

fn amm_wasm() -> Vec<u8> {
    include_bytes!("../../../.icp/cache/artifacts/rumi_amm").to_vec()
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn install_ledger(
    pic: &PocketIc,
    ledger_id: Principal,
    admin: Principal,
    minting_account: Principal,
    initial_owner: Principal,
    initial_balance: u128,
    symbol: &str,
) {
    pic.add_cycles(ledger_id, 2_000_000_000_000);
    let args = LedgerInitArgs {
        minting_account: Account {
            owner: minting_account,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: Nat::from(0u8),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: symbol.to_string(),
        token_symbol: symbol.to_string(),
        metadata: vec![],
        initial_balances: vec![(
            Account {
                owner: initial_owner,
                subaccount: None,
            },
            Nat::from(initial_balance),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
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
        ledger_id,
        ledger_wasm(),
        encode_args((LedgerArg::Init(args),)).unwrap(),
        Some(admin),
    );
}

fn balance(pic: &PocketIc, ledger: Principal, account: Account) -> u128 {
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account).unwrap(),
        )
        .expect("balance query failed");
    match result {
        WasmResult::Reply(bytes) => decode_one::<Nat>(&bytes).unwrap().0.try_into().unwrap(),
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    }
}

fn decode_amm_ok<T: CandidType + for<'de> Deserialize<'de>>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => {
            let result: Result<T, AmmError> = decode_one(&bytes).expect("AMM result decode failed");
            result.expect("AMM returned an error")
        }
        WasmResult::Reject(message) => panic!("AMM call rejected: {message}"),
    }
}

fn pool_subaccount(pool_id: &str, token_label: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pool_id.as_bytes());
    hasher.update(b"_");
    hasher.update(token_label.as_bytes());
    hasher.finalize().into()
}

#[test]
fn threeusd_liquidity_uses_default_account_and_second_pool_is_rejected() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let minting_account = Principal::self_authenticating(&[100, 101, 102]);
    let threeusd = Principal::from_text(THREEPOOL_LEDGER).unwrap();
    let threeusd = pic
        .create_canister_with_id(Some(admin), None, threeusd)
        .expect("create test THREEPOOL ledger at its production principal");
    install_ledger(
        &pic,
        threeusd,
        admin,
        minting_account,
        user,
        INITIAL_BALANCE,
        "3USD",
    );

    let icp = pic.create_canister_with_settings(Some(admin), None);
    install_ledger(
        &pic,
        icp,
        admin,
        minting_account,
        user,
        INITIAL_BALANCE,
        "ICP",
    );

    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(
        amm,
        amm_wasm(),
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    );

    let create = |token_a, token_b| CreatePoolArgs {
        token_a,
        token_b,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let pool_id: String = decode_amm_ok(
        pic.update_call(
            amm,
            admin,
            "create_pool",
            encode_one(create(threeusd, icp)).unwrap(),
        )
        .unwrap(),
    );

    // The existing pool stays available, but a second 3USD pool would share
    // the one ledger account and make its per-pool reserve accounting unsafe.
    let second = pic
        .update_call(
            amm,
            admin,
            "create_pool",
            encode_one(create(threeusd, Principal::self_authenticating(&[9, 8, 7]))).unwrap(),
        )
        .unwrap();
    match second {
        WasmResult::Reply(bytes) => {
            let result: Result<String, AmmError> = decode_one(&bytes).unwrap();
            assert!(matches!(result, Err(AmmError::InvalidInput { .. })));
        }
        WasmResult::Reject(message) => panic!("second pool rejected at transport: {message}"),
    }

    for ledger in [threeusd, icp] {
        let approved = pic
            .update_call(
                ledger,
                user,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: None,
                    spender: Account {
                        owner: amm,
                        subaccount: None,
                    },
                    amount: Nat::from(u128::MAX),
                    expected_allowance: None,
                    expires_at: None,
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(approved, WasmResult::Reply(_)));
    }

    let added: Nat = decode_amm_ok(
        pic.update_call(
            amm,
            user,
            "add_liquidity",
            encode_args((pool_id.clone(), DEPOSIT, DEPOSIT, 0u128)).unwrap(),
        )
        .unwrap(),
    );
    let shares: u128 = added.0.try_into().unwrap();
    assert!(shares > 1_000);

    let pool_info: Option<PoolInfo> = decode_one(&match pic
        .query_call(
            amm,
            Principal::anonymous(),
            "get_pool",
            encode_one(pool_id.clone()).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("get_pool rejected: {message}"),
    })
    .unwrap();
    let pool_info = pool_info.expect("first 3USD pool remains present");
    let threeusd_label = if threeusd.to_text() <= icp.to_text() {
        "token_a"
    } else {
        "token_b"
    };
    let pool_sub = pool_subaccount(&pool_id, threeusd_label);
    assert_eq!(
        balance(
            &pic,
            threeusd,
            Account {
                owner: amm,
                subaccount: None,
            },
        ),
        DEPOSIT,
        "3USD deposit must reach the canister's default account",
    );
    assert_eq!(
        balance(
            &pic,
            threeusd,
            Account {
                owner: amm,
                subaccount: Some(pool_sub),
            },
        ),
        0,
        "3USD must not be sent to the legacy per-pool subaccount",
    );

    let user_shares = shares - 1_000; // AMM locks MINIMUM_LIQUIDITY.
    let (withdrawn_a, withdrawn_b) = decode_amm_ok::<(Nat, Nat)>(
        pic.update_call(
            amm,
            user,
            "remove_liquidity",
            encode_args((pool_id, user_shares, 0u128, 0u128)).unwrap(),
        )
        .unwrap(),
    );
    let withdrawn_threeusd: u128 = if pool_info.token_a == threeusd {
        withdrawn_a.0.try_into().unwrap()
    } else {
        withdrawn_b.0.try_into().unwrap()
    };
    assert!(withdrawn_threeusd > 0);
    assert_eq!(
        balance(
            &pic,
            threeusd,
            Account {
                owner: amm,
                subaccount: None,
            },
        ),
        DEPOSIT - withdrawn_threeusd,
        "3USD withdrawal must debit the same default account",
    );
    assert_eq!(
        balance(
            &pic,
            threeusd,
            Account {
                owner: user,
                subaccount: None,
            },
        ),
        INITIAL_BALANCE - DEPOSIT + withdrawn_threeusd,
    );
}
