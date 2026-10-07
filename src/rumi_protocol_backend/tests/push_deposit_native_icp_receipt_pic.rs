//! Positive receipt proof for push-deposit V2 against the pinned official NNS
//! ICP ledger. The backend sweeps its deterministic caller subaccount, then
//! the fixture independently checks the exact `query_blocks` transfer using
//! the ledger's `account_identifier` method. Completion survives upgrade and
//! replay cannot create a second transfer or vault credit.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{TransferArg, TransferError},
    },
    icrc3::archive::QueryArchiveFn,
};
use pocket_ic::{ErrorCode, PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    vault::CandidVault, InitArg, ProtocolArg, ProtocolError, PushDepositSweepPhase,
    PushDepositSweepResultView, PushDepositSweepStatusView, UpgradeArg,
};
use sha2::{Digest, Sha224, Sha256};
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime},
};

const LEDGER_FEE: u64 = 10_000;
const DEPOSIT: u64 = 100_000_000;

#[derive(CandidType, Deserialize)]
struct NativeIcpInitArgs {
    minting_account: String,
    icrc1_minting_account: Option<Account>,
    initial_values: Vec<(String, IcpTokens)>,
    max_message_size_bytes: Option<u64>,
    transaction_window: Option<LedgerDuration>,
    archive_options: Option<NativeArchiveOptions>,
    send_whitelist: Vec<Principal>,
    transfer_fee: Option<IcpTokens>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    feature_flags: Option<FeatureFlags>,
}

#[derive(CandidType, Deserialize)]
enum NativeLedgerArg {
    #[serde(rename = "Init")]
    Init(NativeIcpInitArgs),
}

#[derive(CandidType, Deserialize)]
struct IcpTokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize)]
struct LedgerDuration {
    secs: u64,
    nanos: u32,
}
#[derive(CandidType, Deserialize)]
struct NativeArchiveOptions {
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
struct FeatureFlags {
    icrc2: bool,
}
#[derive(CandidType, Deserialize)]
struct MockXrc {
    rates: Vec<(String, u64)>,
}

#[derive(CandidType, Deserialize)]
struct NativeTimestamp {
    timestamp_nanos: u64,
}
#[derive(CandidType, Deserialize)]
struct NativeTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeOperation>,
    created_at_time: NativeTimestamp,
}
#[derive(CandidType, Deserialize)]
enum NativeOperation {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: IcpTokens,
    },
    Mint {
        to: Vec<u8>,
        amount: IcpTokens,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: IcpTokens,
        fee: IcpTokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: IcpTokens,
        expected_allowance: Option<IcpTokens>,
        fee: IcpTokens,
        expires_at: Option<NativeTimestamp>,
    },
}
#[derive(CandidType, Deserialize)]
struct NativeBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeTransaction,
    timestamp: NativeTimestamp,
}
#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksArgs {
    start: u64,
    length: u64,
}
#[derive(CandidType, Deserialize)]
struct NativeQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<NativeBlock>,
    first_block_index: u64,
    archived_blocks: Vec<NativeArchiveRange>,
}
#[derive(CandidType, Deserialize)]
struct NativeBlockRange {
    blocks: Vec<NativeBlock>,
}
#[derive(CandidType, Deserialize)]
enum NativeArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}
type NativeArchiveResult = Result<NativeBlockRange, NativeArchiveError>;
#[derive(CandidType, Deserialize)]
struct NativeArchiveRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<NativeQueryBlocksArgs, NativeArchiveResult>,
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn native_account_identifier(owner: Principal) -> String {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0u8; 32]);
    let hash = hasher.finalize();
    let mut crc = !0u32;
    for byte in &hash {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut identifier = Vec::with_capacity(32);
    identifier.extend_from_slice(&(!crc).to_be_bytes());
    identifier.extend_from_slice(&hash);
    identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn native_ledger_wasm() -> Vec<u8> {
    const EXPECTED_GZIP_SHA256: &str =
        "51f4be010f23064137defacd627ffbec024c5133210c68ca3b80ab8f257101d6";
    let path = env::var("RUMI_TEST_NNS_LEDGER_WASM_GZ")
        .unwrap_or_else(|_| "/private/tmp/rumi-nns-ledger-69b755.wasm.gz".into());
    let gzip = fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    assert_eq!(
        format!("{:x}", Sha256::digest(&gzip)),
        EXPECTED_GZIP_SHA256,
        "pinned official NNS ledger gzip hash"
    );
    let output = Command::new("gzip")
        .args(["-dc", &path])
        .output()
        .expect("run gzip to unpack pinned ledger");
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn deploy_native_ledger(
    pic: &PocketIc,
    owner: Principal,
    initial_balance: u64,
    controller: Principal,
) -> Principal {
    let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    pic.create_canister_with_id(None, None, ledger)
        .expect("create canonical NNS ledger principal");
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    let init = NativeLedgerArg::Init(NativeIcpInitArgs {
        minting_account: native_account_identifier(Principal::management_canister()),
        icrc1_minting_account: Some(account(Principal::management_canister())),
        initial_values: vec![(
            native_account_identifier(owner),
            IcpTokens {
                e8s: initial_balance,
            },
        )],
        max_message_size_bytes: Some(1_048_576),
        transaction_window: None,
        archive_options: Some(NativeArchiveOptions {
            num_blocks_to_archive: 1_000,
            max_transactions_per_response: Some(100),
            trigger_threshold: 1_000,
            max_message_size_bytes: Some(1_048_576),
            cycles_for_archive_creation: Some(1_000_000_000_000),
            node_max_memory_size_bytes: Some(1_073_741_824),
            controller_id: controller,
            more_controller_ids: None,
        }),
        send_whitelist: Vec::new(),
        transfer_fee: Some(IcpTokens { e8s: LEDGER_FEE }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(FeatureFlags { icrc2: true }),
    });
    pic.install_canister(
        ledger,
        native_ledger_wasm(),
        encode_args((init,)).unwrap(),
        None,
    );
    ledger
}

fn artifact(name: &str) -> Vec<u8> {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    fs::read(target.join("wasm32-unknown-unknown/release").join(name))
        .unwrap_or_else(|error| panic!("read {name} from CARGO_TARGET_DIR: {error}"))
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
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn update_args<T: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: impl candid::utils::ArgumentEncoder,
) -> T {
    let reply = pic
        .update_call(canister, caller, method, encode_args(args).unwrap())
        .unwrap_or_else(|error| panic!("{method}: {error}"));
    match reply {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn query<T: CandidType + for<'de> Deserialize<'de>>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: impl CandidType,
) -> T {
    let reply = pic
        .query_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("{method}: {error}"));
    match reply {
        WasmResult::Reply(bytes) => {
            decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn native_account_id(pic: &PocketIc, ledger: Principal, account: Account) -> Vec<u8> {
    query(
        pic,
        ledger,
        Principal::anonymous(),
        "account_identifier",
        account,
    )
}

fn vaults(pic: &PocketIc, backend: Principal, owner: Principal) -> Vec<CandidVault> {
    query(pic, backend, owner, "get_vaults", Some(owner))
}

#[test]
fn push_deposit_sweep_is_verified_by_official_native_ledger_and_credits_once_across_upgrade() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let user = Principal::self_authenticating(b"native-push-receipt-user");
    let developer = Principal::self_authenticating(b"native-push-receipt-developer");
    let ledger = deploy_native_ledger(&pic, user, 1_000_000_000, developer);
    let icrc3_probe = pic.query_call(
        ledger,
        Principal::anonymous(),
        "icrc3_get_blocks",
        Vec::new(),
    );
    assert!(
        matches!(icrc3_probe, Err(error) if error.code == ErrorCode::CanisterMethodNotFound),
        "pinned native ICP ledger must lack icrc3_get_blocks so push-sweep receipt acceptance exercises query_blocks/account_identifier rather than the ICRC-3 fallback"
    );

    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(),
        encode_one(MockXrc {
            rates: vec![("ICP/USD".into(), 1_000_000_000)],
        })
        .unwrap(),
        None,
    );

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    pic.set_controllers(backend, None, vec![Principal::anonymous(), developer])
        .unwrap();
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: LEDGER_FEE,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_one(init).unwrap(),
        None,
    );
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    let deposit_account: Account = query(
        &pic,
        backend,
        user,
        "get_deposit_account",
        None::<Principal>,
    );
    assert_eq!(
        deposit_account.owner, backend,
        "deposit account is owned by the backend canister"
    );
    assert!(
        deposit_account.subaccount.is_some(),
        "caller deposit is isolated in a backend-owned subaccount"
    );
    let funded: Result<Nat, TransferError> = update(
        &pic,
        ledger,
        user,
        "icrc1_transfer",
        TransferArg {
            from_subaccount: None,
            to: deposit_account.clone(),
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(DEPOSIT),
        },
    );
    funded.expect("fund the caller's deterministic backend subaccount");
    assert_eq!(
        query::<Nat>(
            &pic,
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            deposit_account.clone()
        )
        .0,
        DEPOSIT.into()
    );

    let opened: Result<rumi_protocol_backend::vault::OpenVaultSuccess, ProtocolError> = update_args(
        &pic,
        backend,
        user,
        "open_vault_with_deposit_v2",
        (0u64, None::<Principal>, 1u128),
    );
    let opened = opened.expect("official native ICP sweep and exact receipt verification succeed");
    assert_eq!(opened.vault_id, 1);

    let status: Option<PushDepositSweepStatusView> =
        query(&pic, backend, user, "get_my_push_deposit_sweep", ledger);
    let status = status.expect("completed sweep status is retained");
    assert_eq!(status.phase, PushDepositSweepPhase::Complete);
    assert_eq!(status.amount_raw, DEPOSIT - LEDGER_FEE);
    let block_index = status
        .candidate_block_index
        .expect("verified native ledger block index");
    assert_eq!(
        status.result,
        Some(PushDepositSweepResultView::Open {
            vault_id: 1,
            block_index
        })
    );

    // Independently inspect the exact native ledger block and prove it names
    // the backend-owned caller subaccount and the backend's receiving account.
    let source = native_account_id(&pic, ledger, deposit_account);
    let destination = native_account_id(&pic, ledger, account(backend));
    let history: NativeQueryBlocksResponse = query(
        &pic,
        ledger,
        Principal::anonymous(),
        "query_blocks",
        NativeQueryBlocksArgs {
            start: block_index,
            length: 1,
        },
    );
    let offset =
        usize::try_from(block_index - history.first_block_index).expect("block offset fits usize");
    let block = history
        .blocks
        .get(offset)
        .expect("sweep is in direct native ledger history");
    match block
        .transaction
        .operation
        .as_ref()
        .expect("native ledger operation")
    {
        NativeOperation::Transfer {
            from,
            to,
            spender,
            amount,
            fee,
        } => {
            assert_eq!(from, &source, "source is backend-owned caller subaccount");
            assert_eq!(to, &destination, "destination is backend account");
            assert!(spender.is_none(), "sweep is a direct ICRC-1 transfer");
            assert_eq!(amount.e8s, DEPOSIT - LEDGER_FEE);
            assert_eq!(fee.e8s, LEDGER_FEE);
        }
        _ => panic!("push sweep block is not a transfer"),
    }
    assert_eq!(
        block.transaction.icrc1_memo.as_deref(),
        Some(status.memo.as_slice())
    );
    assert_eq!(
        block.transaction.created_at_time.timestamp_nanos,
        status.created_at_time_ns
    );
    assert_eq!(
        query::<Nat>(
            &pic,
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            account(backend)
        )
        .0,
        (DEPOSIT - LEDGER_FEE).into()
    );
    assert_eq!(
        vaults(&pic, backend, user)
            .iter()
            .map(|v| v.collateral_amount)
            .sum::<u64>(),
        DEPOSIT - LEDGER_FEE
    );

    let upgrade = ProtocolArg::Upgrade(UpgradeArg {
        mode: None,
        description: Some("native push-deposit receipt persistence".into()),
    });
    pic.upgrade_canister(
        backend,
        artifact("rumi_protocol_backend.wasm"),
        encode_args((upgrade,)).unwrap(),
        None,
    )
    .expect("upgrade backend after native-ledger receipt proof");
    let replay: Result<rumi_protocol_backend::vault::OpenVaultSuccess, ProtocolError> = update_args(
        &pic,
        backend,
        user,
        "open_vault_with_deposit_v2",
        (0u64, None::<Principal>, 1u128),
    );
    assert!(matches!(replay, Ok(result) if result.vault_id == 1));
    let after_replay: NativeQueryBlocksResponse = query(
        &pic,
        ledger,
        Principal::anonymous(),
        "query_blocks",
        NativeQueryBlocksArgs {
            start: 0,
            length: 100,
        },
    );
    assert_eq!(
        after_replay.chain_length, history.chain_length,
        "terminal replay must not create another ledger transfer"
    );
    let vaults_after_upgrade = vaults(&pic, backend, user);
    assert_eq!(vaults_after_upgrade.len(), 1);
    assert_eq!(
        vaults_after_upgrade[0].collateral_amount,
        DEPOSIT - LEDGER_FEE
    );
    let status_after_upgrade: Option<PushDepositSweepStatusView> =
        query(&pic, backend, user, "get_my_push_deposit_sweep", ledger);
    assert_eq!(
        status_after_upgrade.unwrap().candidate_block_index,
        Some(block_index)
    );
}
