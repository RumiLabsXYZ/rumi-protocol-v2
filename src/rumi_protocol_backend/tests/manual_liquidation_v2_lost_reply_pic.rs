//! PocketIC regression for receipt recovery and exact compensation in manual
//! liquidation V2. The flaky ledger commits ICRC-2 transfer_from and returns a
//! GenericError to simulate a lost reply; this is not a callback-trap test.

#![cfg(feature = "test-manual-liquidation-v2-admission")]

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::{
    vault::{CandidVault, VaultArg},
    InboundCollateralResultView, InboundCollateralStatusView, InitArg, ManualLiquidationV2Phase,
    ManualLiquidationV2RefundKind, ManualLiquidationV2StatusView, ProtocolArg, ProtocolError,
    RepaymentV2Phase, RepaymentV2StatusView, SuccessWithFee,
};
use sha2::{Digest, Sha224, Sha256};
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime},
};

const COLLATERAL: u64 = 5_000_000_000;
const BORROWED: u64 = 10_000_000_000;
const LIQUIDATION_CAP: u64 = 2_000_000_000;
const REPAYMENT: u64 = 100_000_000;

#[derive(CandidType, Deserialize, Clone, Debug)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: Account,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize)]
struct MockXrc {
    rates: std::collections::HashMap<String, u64>,
}

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

fn native_icp_account_identifier(owner: Principal) -> String {
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

fn native_icp_ledger_wasm() -> Vec<u8> {
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
        .expect("run gzip to decompress pinned official NNS ledger");
    assert!(
        output.status.success(),
        "gzip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn deploy_native_icp_ledger(
    pic: &PocketIc,
    initial_owner: Principal,
    initial_balance_e8s: u64,
    controller: Principal,
) -> Principal {
    let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    pic.create_canister_with_id(None, None, ledger)
        .expect("create canonical NNS ledger principal");
    pic.add_cycles(ledger, 5_000_000_000_000_000);
    let minting_account = Principal::management_canister();
    let init = NativeLedgerArg::Init(NativeIcpInitArgs {
        minting_account: native_icp_account_identifier(minting_account),
        icrc1_minting_account: Some(account(minting_account)),
        initial_values: vec![(
            native_icp_account_identifier(initial_owner),
            IcpTokens {
                e8s: initial_balance_e8s,
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
        transfer_fee: Some(IcpTokens { e8s: 10_000 }),
        token_symbol: Some("ICP".into()),
        token_name: Some("Internet Computer".into()),
        feature_flags: Some(FeatureFlags { icrc2: true }),
    });
    pic.install_canister(
        ledger,
        native_icp_ledger_wasm(),
        encode_args((init,)).unwrap(),
        None,
    );
    ledger
}

fn artifact(name: &str) -> Vec<u8> {
    if name == "rumi_protocol_backend.wasm" {
        if let Some(path) = env::var_os("RUMI_MANUAL_V2_BACKEND_WASM") {
            return fs::read(path).expect("read source-matched manual V2 backend Wasm");
        }
    }
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    fs::read(target.join("wasm32-unknown-unknown/release").join(name))
        .unwrap_or_else(|error| panic!("read {name} from CARGO_TARGET_DIR: {error}"))
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn update<A: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: A,
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} update failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn update_pair<A: CandidType, B: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: (A, B),
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_args(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} update failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn update_triple<A: CandidType, B: CandidType, C: CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: (A, B, C),
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, encode_args(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} update failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn query<A: CandidType, T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: A,
) -> T {
    let bytes = match pic
        .query_call(canister, caller, method, encode_one(args).unwrap())
        .unwrap_or_else(|error| panic!("{method} query failed: {error}"))
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    };
    decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"))
}

fn expect_ok<T: for<'de> Deserialize<'de> + CandidType>(bytes: Vec<u8>, method: &str) -> T {
    let result: Result<T, ProtocolError> =
        decode_one(&bytes).unwrap_or_else(|error| panic!("decode {method}: {error}"));
    result.unwrap_or_else(|error| panic!("{method} returned {error:?}"))
}

fn balance(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let amount: Nat = query::<_, Nat>(
        pic,
        ledger,
        Principal::anonymous(),
        "icrc1_balance_of",
        account(owner),
    );
    amount.0.try_into().expect("ledger balance fits u128")
}

fn ledger_log_length(pic: &PocketIc, ledger: Principal) -> u64 {
    let result: GetBlocksResult = query(
        pic,
        ledger,
        Principal::anonymous(),
        "icrc3_get_blocks",
        vec![GetBlocksRequest {
            start: Nat::from(0u8),
            length: Nat::from(1u8),
        }],
    );
    result
        .log_length
        .0
        .try_into()
        .expect("ledger log length fits u64")
}

fn vault(pic: &PocketIc, backend: Principal, owner: Principal) -> CandidVault {
    let rows: Vec<CandidVault> = query(
        pic,
        backend,
        Principal::anonymous(),
        "get_vaults",
        Some(owner),
    );
    rows.into_iter()
        .find(|row| row.vault_id == 1)
        .expect("fixture vault exists")
}

#[test]
fn manual_liquidation_v2_recovers_lost_pull_and_refunds_exactly_after_vault_change() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let borrower = Principal::self_authenticating(b"manual-liq-v2-pic-borrower");
    let liquidator = Principal::self_authenticating(b"manual-liq-v2-pic-liquidator");
    let developer = Principal::self_authenticating(b"manual-liq-v2-pic-developer");
    let treasury = Principal::self_authenticating(b"manual-liq-v2-pic-treasury");

    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    pic.set_controllers(backend, None, vec![Principal::anonymous(), developer])
        .unwrap();

    let icp = deploy_native_icp_ledger(&pic, borrower, 1_000_000_000_000, developer);
    let icusd = pic.create_canister();
    pic.add_cycles(icusd, 1_000_000_000_000);
    pic.install_canister(
        icusd,
        artifact("flaky_ledger.wasm"),
        encode_one(()).unwrap(),
        None,
    );

    let xrc = pic.create_canister();
    pic.add_cycles(xrc, 1_000_000_000_000);
    pic.install_canister(
        xrc,
        include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec(),
        encode_one(MockXrc {
            rates: [("ICP/USD".to_string(), 1_000_000_000)].into(),
        })
        .unwrap(),
        None,
    );
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let init = ProtocolArg::Init(InitArg {
        xrc_principal: xrc,
        icusd_ledger_principal: icusd,
        icp_ledger_principal: icp,
        fee_e8s: 10_000,
        developer_principal: developer,
        treasury_principal: Some(treasury),
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
    let _: () = decode_one(&update(
        &pic,
        icusd,
        Principal::anonymous(),
        "set_minting_account",
        Some(account(backend)),
    ))
    .expect("set icUSD backend minter");
    let _: () = decode_one(&update(
        &pic,
        icusd,
        Principal::anonymous(),
        "set_fee",
        Nat::from(10_000u64),
    ))
    .expect("set icUSD ledger fee");

    let _: () = expect_ok(
        update(&pic, backend, developer, "set_borrowing_fee", 0.0f64),
        "set_borrowing_fee",
    );
    let _: () = expect_ok(
        update_pair(&pic, backend, developer, "set_interest_rate", (icp, 0.0f64)),
        "set_interest_rate",
    );
    let _: () = expect_ok(
        update(&pic, backend, developer, "set_treasury_principal", treasury),
        "set_treasury_principal",
    );

    let collateral_approval: Result<Nat, ApproveError> = decode_one(&update(
        &pic,
        icp,
        borrower,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(COLLATERAL + 10_000),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode collateral approval");
    collateral_approval.expect("approve collateral transfer");

    let opened: InboundCollateralStatusView = expect_ok(
        update_triple(
            &pic,
            backend,
            borrower,
            "open_vault_v2",
            (1u128, COLLATERAL, None::<Principal>),
        ),
        "open_vault_v2",
    );
    assert_eq!(opened.request_id, 1);
    assert_eq!(
        opened.phase,
        rumi_protocol_backend::InboundCollateralPhase::Complete
    );
    assert!(matches!(
        opened.result,
        Some(InboundCollateralResultView::Open { vault_id: 1, .. })
    ));
    let _borrowed: SuccessWithFee = expect_ok(
        update(
            &pic,
            backend,
            borrower,
            "borrow_from_vault",
            VaultArg {
                vault_id: 1,
                amount: BORROWED,
            },
        ),
        "borrow_from_vault",
    );

    // Make the vault liquidatable, then let the manual endpoint's freshness
    // gate fetch that rate before pinning its quote.
    let rate_update = pic
        .update_call(
            xrc,
            developer,
            "set_exchange_rate",
            encode_args(("ICP".to_string(), "USD".to_string(), 200_000_000u64)).unwrap(),
        )
        .expect("set_exchange_rate call");
    match rate_update {
        WasmResult::Reply(bytes) => {
            let _: () = decode_one(&bytes).expect("decode rate update");
        }
        WasmResult::Reject(message) => panic!("set_exchange_rate rejected: {message}"),
    }
    // An 80% price fall is intentionally outside the oracle sanity band.
    // Exercise the real three-distinct-source-bar confirmation gate before
    // expecting the liquidation to become eligible.
    for request_id in [91u128, 92u128] {
        pic.advance_time(Duration::from_secs(301));
        let rejected: Result<ManualLiquidationV2StatusView, ProtocolError> =
            decode_one(&update_pair(
                &pic,
                backend,
                liquidator,
                "liquidate_vault_partial_v2",
                (
                    request_id,
                    VaultArg {
                        vault_id: 1,
                        amount: LIQUIDATION_CAP,
                    },
                ),
            ))
            .expect("decode pending outlier liquidation rejection");
        assert!(
            rejected.is_err(),
            "unconfirmed oracle outlier must not liquidate"
        );
    }
    pic.advance_time(Duration::from_secs(301));

    let _: () = decode_one(&update_pair(
        &pic,
        icusd,
        Principal::anonymous(),
        "mint",
        (account(liquidator), Nat::from(1_000_000_000_000u64)),
    ))
    .expect("mint liquidation funds");
    let liquidator_approval: Result<Nat, ApproveError> = decode_one(&update(
        &pic,
        icusd,
        liquidator,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(LIQUIDATION_CAP),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode liquidator approval");
    liquidator_approval.expect("approve exact liquidation cap");
    let liquidator_before = balance(&pic, icusd, liquidator);
    let blocks_before_pull = ledger_log_length(&pic, icusd);

    let _: () = decode_one(&update(
        &pic,
        icusd,
        Principal::anonymous(),
        "set_phantom_failures",
        1u32,
    ))
    .expect("arm committed-pull/lost-reply injection");
    let first: ManualLiquidationV2StatusView = expect_ok(
        update_pair(
            &pic,
            backend,
            liquidator,
            "liquidate_vault_partial_v2",
            (
                1u128,
                VaultArg {
                    vault_id: 1,
                    amount: LIQUIDATION_CAP,
                },
            ),
        ),
        "first manual liquidation request",
    );
    assert_eq!(first.phase, ManualLiquidationV2Phase::HeldPull);
    assert!(first.had_ambiguous_attempt);
    assert_eq!(first.candidate_block_index, None);
    assert_eq!(
        balance(&pic, icusd, liquidator),
        liquidator_before - u128::from(first.pull_amount_raw)
    );
    assert_eq!(ledger_log_length(&pic, icusd), blocks_before_pull + 1);
    let pull_block = blocks_before_pull;

    // Exact same-ID recovery stays held until the committed ledger receipt is
    // attached. The existing journal must not dispatch a second debit.
    let replay: ManualLiquidationV2StatusView = expect_ok(
        update_pair(
            &pic,
            backend,
            liquidator,
            "liquidate_vault_partial_v2",
            (
                1u128,
                VaultArg {
                    vault_id: 1,
                    amount: LIQUIDATION_CAP,
                },
            ),
        ),
        "same-request replay before receipt attachment",
    );
    assert_eq!(replay.phase, ManualLiquidationV2Phase::HeldPull);
    assert_eq!(
        balance(&pic, icusd, liquidator),
        liquidator_before - u128::from(first.pull_amount_raw)
    );
    assert_eq!(ledger_log_length(&pic, icusd), blocks_before_pull + 1);

    // Change the live debt while the receipt is unresolved. The borrower can
    // repay independently; receipt-backed commit must then compensate the
    // liquidator instead of applying the stale pinned liquidation plan.
    let repay_approval: Result<Nat, ApproveError> = decode_one(&update(
        &pic,
        icusd,
        borrower,
        "icrc2_approve",
        ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(REPAYMENT),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        },
    ))
    .expect("decode repayment approval");
    repay_approval.expect("approve borrower repayment");
    let repaid: RepaymentV2StatusView = expect_ok(
        update_pair(
            &pic,
            backend,
            borrower,
            "repay_to_vault_v2",
            (
                1u128,
                VaultArg {
                    vault_id: 1,
                    amount: REPAYMENT,
                },
            ),
        ),
        "repay_to_vault_v2 while manual pull is held",
    );
    assert_eq!(repaid.phase, RepaymentV2Phase::Complete);
    assert_eq!(repaid.request_id, 1);
    assert!(repaid.result.is_some());
    let debt_after_repayment = vault(&pic, backend, borrower).borrowed_icusd_amount;
    assert!(debt_after_repayment < BORROWED);
    let blocks_before_refund = ledger_log_length(&pic, icusd);

    let recovered: ManualLiquidationV2StatusView = expect_ok(
        update_pair(
            &pic,
            backend,
            liquidator,
            "attach_my_manual_liquidation_v2_candidate",
            (1u128, pull_block),
        ),
        "attach exact committed pull receipt",
    );
    assert_eq!(recovered.phase, ManualLiquidationV2Phase::Refunded);
    assert!(!recovered.commit_started);
    let refund = recovered
        .refund
        .expect("exact refund obligation is retained");
    assert_eq!(refund.amount_e8s, first.pull_amount_raw);
    assert_eq!(
        refund.kind,
        ManualLiquidationV2RefundKind::IcusdMint {
            burn_block_index: pull_block
        }
    );
    assert_eq!(balance(&pic, icusd, liquidator), liquidator_before);
    assert_eq!(
        vault(&pic, backend, borrower).borrowed_icusd_amount,
        debt_after_repayment
    );
    assert_eq!(ledger_log_length(&pic, icusd), blocks_before_refund + 1);

    // Terminal replay returns the same request result without another burn or
    // refund mint.
    let terminal_replay: ManualLiquidationV2StatusView = expect_ok(
        update_pair(
            &pic,
            backend,
            liquidator,
            "liquidate_vault_partial_v2",
            (
                1u128,
                VaultArg {
                    vault_id: 1,
                    amount: LIQUIDATION_CAP,
                },
            ),
        ),
        "terminal same-request replay",
    );
    assert_eq!(terminal_replay.phase, ManualLiquidationV2Phase::Refunded);
    assert_eq!(balance(&pic, icusd, liquidator), liquidator_before);
    assert_eq!(ledger_log_length(&pic, icusd), blocks_before_refund + 1);
}
