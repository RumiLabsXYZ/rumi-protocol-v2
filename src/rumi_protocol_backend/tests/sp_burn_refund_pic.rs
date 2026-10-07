//! PocketIC integration coverage for authenticated SP burn compensation.
//!
//! This fixture uses the repository's official `ic-icrc1-ledger.wasm`. It
//! proves the backend accepts a real burn block, returns the exact amount to
//! the registered SP, and keeps that outcome idempotent across retries and a
//! backend upgrade. No vault is installed in this fixture: the reimbursement
//! is authorized by the exact unused burn proof and remains independent of
//! current vault state.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::icrc3_proof::{encode_writedown_memo, SpProofLedger, SpWritedownProof};
use rumi_protocol_backend::sp_burn_refund::SpBurnRefundReceipt;
use rumi_protocol_backend::ProtocolError;
use std::time::Duration;

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
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

#[derive(CandidType, Deserialize, Clone, Debug)]
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
    #[serde(rename = "Upgrade")]
    Upgrade(Option<LedgerUpgradeArgs>),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LedgerUpgradeArgs {
    metadata: Option<Vec<(String, MetadataValue)>>,
    token_symbol: Option<String>,
    token_name: Option<String>,
    transfer_fee: Option<Nat>,
    change_fee_collector: Option<ChangeFeeCollector>,
    max_memo_length: Option<u16>,
    feature_flags: Option<FeatureFlags>,
    accounts_overflow_trim_quantity: Option<u64>,
    change_archive_options: Option<ChangeArchiveOptions>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ChangeFeeCollector {
    #[serde(rename = "Unset")]
    Unset,
    #[serde(rename = "SetTo")]
    SetTo(Account),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChangeArchiveOptions {
    num_blocks_to_archive: Option<u64>,
    max_transactions_per_response: Option<u64>,
    trigger_threshold: Option<u64>,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    controller_id: Option<Principal>,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    fee: Option<Nat>,
    created_at_time: Option<u64>,
    memo: Option<Vec<u8>>,
    amount: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct UpgradeArg {
    mode: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolLiquidationResult {
    success: bool,
    vault_id: u64,
    liquidated_debt: u64,
    collateral_received: u64,
    collateral_type: String,
    block_index: u64,
    fee: u64,
    collateral_price_e8s: u64,
}

struct Fixture {
    pic: PocketIc,
    backend: Principal,
    ledger: Principal,
    sp: Principal,
    attacker: Principal,
    amount: u64,
    vault_id: u64,
    starting_balance: u64,
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn deploy_ledger(
    pic: &PocketIc,
    minting_account: Account,
    initial_balance: u64,
    controller: Principal,
    max_memo_length: u16,
) -> Principal {
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 2_000_000_000_000);
    let init = LedgerInitArgs {
        minting_account,
        fee_collector_account: None,
        transfer_fee: Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(max_memo_length),
        token_name: "icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![(
            account(Principal::self_authenticating(b"refund-sp")),
            Nat::from(initial_balance),
        )],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 64,
            trigger_threshold: 40,
            controller_id: controller,
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
        None,
    );
    ledger
}

fn fixture() -> Fixture {
    fixture_with_memo_limit(64)
}

fn fixture_with_memo_limit(max_memo_length: u16) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let backend = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    let sp = Principal::self_authenticating(b"refund-sp");
    let attacker = Principal::self_authenticating(b"refund-attacker");
    let developer = Principal::self_authenticating(b"refund-developer");
    pic.set_controllers(backend, None, vec![Principal::anonymous(), developer])
        .expect("set backend controllers");

    let ledger = deploy_ledger(
        &pic,
        account(backend),
        1_000_000_000,
        developer,
        max_memo_length,
    );
    let mgmt = Principal::from_text("aaaaa-aa").expect("management canister principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer,
    });
    pic.install_canister(
        backend,
        backend_wasm(),
        encode_args((init,)).expect("encode backend init"),
        None,
    );
    expect_ok(
        pic.update_call(
            backend,
            developer,
            "set_stability_pool_principal",
            encode_args((sp,)).expect("encode registered SP"),
        )
        .expect("register SP call"),
        "register SP",
    );

    Fixture {
        pic,
        backend,
        ledger,
        sp,
        attacker,
        amount: 250_000_000,
        vault_id: 91_337,
        starting_balance: 1_000_000_000,
    }
}

fn fixture_with_flaky_ledger() -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let backend = pic.create_canister();
    let ledger = pic.create_canister();
    pic.add_cycles(backend, 2_000_000_000_000);
    pic.add_cycles(ledger, 2_000_000_000_000);
    let sp = Principal::self_authenticating(b"refund-sp");
    let attacker = Principal::self_authenticating(b"refund-attacker");
    let developer = Principal::self_authenticating(b"refund-developer");
    pic.set_controllers(backend, None, vec![Principal::anonymous(), developer])
        .expect("set backend controllers");

    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));
    let target_dir = if target_dir.is_absolute() {
        target_dir
    } else {
        workspace_root.join(target_dir)
    };
    let flaky_ledger_path = target_dir.join("wasm32-unknown-unknown/release/flaky_ledger.wasm");
    let flaky_ledger = std::fs::read(&flaky_ledger_path).unwrap_or_else(|error| {
        panic!(
            "could not read flaky_ledger Wasm at {}: {error}; build src/flaky_ledger first",
            flaky_ledger_path.display()
        )
    });
    pic.install_canister(
        ledger,
        flaky_ledger,
        encode_one(()).expect("encode flaky ledger init"),
        None,
    );
    expect_unit(
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "set_fee",
            encode_one(Nat::from(10_000u64)).expect("encode ledger fee"),
        )
        .expect("set flaky ledger fee"),
        "set flaky ledger fee",
    );
    expect_unit(
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "set_minting_account",
            encode_one(Some(account(backend))).expect("encode minter"),
        )
        .expect("set flaky ledger minter"),
        "set flaky ledger minter",
    );
    expect_unit(
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "mint",
            encode_args((account(sp), Nat::from(1_000_000_000u64)))
                .expect("encode initial SP balance"),
        )
        .expect("mint initial SP balance"),
        "mint initial SP balance",
    );

    let mgmt = Principal::from_text("aaaaa-aa").expect("management canister principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: ledger,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer,
    });
    pic.install_canister(
        backend,
        backend_wasm(),
        encode_args((init,)).expect("encode backend init"),
        None,
    );
    expect_ok(
        pic.update_call(
            backend,
            developer,
            "set_stability_pool_principal",
            encode_args((sp,)).expect("encode registered SP"),
        )
        .expect("register SP call"),
        "register SP",
    );

    Fixture {
        pic,
        backend,
        ledger,
        sp,
        attacker,
        amount: 250_000_000,
        vault_id: 91_337,
        starting_balance: 1_000_000_000,
    }
}

fn expect_ok(result: WasmResult, label: &str) {
    match result {
        WasmResult::Reply(bytes) => {
            let decoded: Result<(), ProtocolError> =
                decode_one(&bytes).unwrap_or_else(|e| panic!("decode {label}: {e}"));
            decoded.unwrap_or_else(|e| panic!("{label} returned error: {e:?}"));
        }
        WasmResult::Reject(message) => panic!("{label} rejected: {message}"),
    }
}

fn expect_unit(result: WasmResult, label: &str) {
    match result {
        WasmResult::Reply(bytes) => {
            decode_one::<()>(&bytes).unwrap_or_else(|error| panic!("decode {label}: {error}"))
        }
        WasmResult::Reject(message) => panic!("{label} rejected: {message}"),
    }
}

fn burn(f: &Fixture) -> (u64, SpWritedownProof) {
    let result = f
        .pic
        .update_call(
            f.ledger,
            f.sp,
            "icrc1_transfer",
            encode_one(TransferArg {
                from_subaccount: None,
                to: account(f.backend),
                fee: None,
                created_at_time: None,
                memo: Some(encode_writedown_memo(f.vault_id)),
                amount: Nat::from(f.amount),
            })
            .expect("encode burn"),
        )
        .expect("burn call");
    let block: Result<Nat, TransferError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode burn result"),
        WasmResult::Reject(message) => panic!("burn rejected: {message}"),
    };
    let block_index = block
        .expect("official ledger burn")
        .0
        .to_string()
        .parse()
        .expect("block index fits u64");
    (
        block_index,
        SpWritedownProof {
            block_index,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo: f.vault_id,
        },
    )
}

fn refund(
    f: &Fixture,
    caller: Principal,
    vault_id: u64,
    amount: u64,
    proof: SpWritedownProof,
) -> Result<SpBurnRefundReceipt, ProtocolError> {
    let result = f
        .pic
        .update_call(
            f.backend,
            caller,
            "refund_stability_pool_burn",
            encode_args((vault_id, amount, proof)).expect("encode refund request"),
        )
        .expect("refund call");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode refund result"),
        WasmResult::Reject(message) => panic!("refund rejected: {message}"),
    }
}

fn balance(f: &Fixture) -> u64 {
    let result = f
        .pic
        .query_call(
            f.ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(f.sp)).expect("encode SP account"),
        )
        .expect("balance query");
    match result {
        WasmResult::Reply(bytes) => {
            let value: Nat = decode_one(&bytes).expect("decode balance");
            value.0.to_string().parse().expect("balance fits u64")
        }
        WasmResult::Reject(message) => panic!("balance query rejected: {message}"),
    }
}

fn push_history_blocks(f: &Fixture, count: usize) {
    for index in 0..count {
        let result = f
            .pic
            .update_call(
                f.ledger,
                f.sp,
                "icrc1_transfer",
                encode_one(TransferArg {
                    from_subaccount: None,
                    to: account(f.attacker),
                    fee: None,
                    created_at_time: None,
                    memo: None,
                    amount: Nat::from(1u64),
                })
                .expect("encode history filler transfer"),
            )
            .unwrap_or_else(|error| panic!("history filler transfer {index}: {error}"));
        let transfer: Result<Nat, TransferError> = match result {
            WasmResult::Reply(bytes) => decode_one(&bytes)
                .unwrap_or_else(|error| panic!("decode history filler transfer {index}: {error}")),
            WasmResult::Reject(message) => {
                panic!("history filler transfer {index} rejected: {message}")
            }
        };
        transfer.unwrap_or_else(|error| panic!("history filler transfer {index} failed: {error:?}"));
    }
}

fn consumed_proofs(f: &Fixture) -> Vec<(SpProofLedger, u64)> {
    let result = f
        .pic
        .query_call(
            f.backend,
            Principal::anonymous(),
            "get_consumed_writedown_proofs",
            encode_args(()).expect("encode consumed proof query"),
        )
        .expect("consumed proof query");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode consumed proofs"),
        WasmResult::Reject(message) => panic!("consumed proof query rejected: {message}"),
    }
}

#[test]
fn expired_refund_scans_archive_then_retries_only_after_too_old_and_complete_absence() {
    let f = fixture_with_memo_limit(21);
    let (_, proof) = burn(&f);
    let original_created_at_time: u64 = f.pic.get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after Unix epoch")
        .as_nanos()
        .try_into()
        .expect("PocketIC timestamp fits u64 nanoseconds");

    // The official ledger accepts the 21-byte burn memo and rejects the
    // backend's 24-byte refund memo. Proof and minter reads therefore succeed,
    // so the backend durably journals the obligation before this mint fails.
    assert!(refund(&f, f.sp, f.vault_id, f.amount, proof.clone()).is_err());
    assert_eq!(balance(&f), f.starting_balance - f.amount);

    // Upgrade the same official ledger with a compatible memo limit so the
    // expired original identity can be retried under its unchanged tuple.
    f.pic
        .upgrade_canister(
            f.ledger,
            ledger_wasm(),
            encode_args((LedgerArg::Upgrade(Some(LedgerUpgradeArgs {
                metadata: None,
                token_symbol: None,
                token_name: None,
                transfer_fee: None,
                change_fee_collector: None,
                max_memo_length: Some(64),
                feature_flags: None,
                accounts_overflow_trim_quantity: None,
                change_archive_options: None,
            })),))
            .expect("encode ledger upgrade"),
            None,
        )
        .expect("upgrade ledger to accept the persisted refund memo");

    // The official ledger archives its old blocks after this threshold. The
    // resulting history is also longer than the backend's bounded scan page,
    // so the first no-match page is persisted before the backend upgrade.
    push_history_blocks(&f, 600);
    let history: GetBlocksResult = f
        .pic
        .query_call(
            f.ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_args((vec![GetBlocksRequest {
                start: Nat::from(0u64),
                length: Nat::from(64u64),
            }],))
            .expect("encode archive probe"),
        )
        .map(|result| match result {
            WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode archive probe"),
            WasmResult::Reject(message) => panic!("archive probe rejected: {message}"),
        })
        .expect("archive probe call");
    assert!(
        !history.archived_blocks.is_empty(),
        "fixture must exercise an official-ledger archive callback"
    );
    // A 25-hour delay takes the original created_at_time outside ICRC-1's
    // transaction window. The retry must return TooOld, persist partial scan
    // progress, and leave the original obligation and balances unchanged.
    f.pic.advance_time(Duration::from_secs(25 * 60 * 60));
    let expired_retry = refund(&f, f.sp, f.vault_id, f.amount, proof.clone());
    assert!(expired_retry.is_err(), "TooOld retry should persist a partial scan, got: {expired_retry:?}; ledger log length was {}", history.log_length);
    assert_eq!(
        balance(&f),
        f.starting_balance - f.amount - 600 * 10_000 - 600
    );

    f.pic.advance_time(Duration::from_secs(600));
    for _ in 0..10 {
        f.pic.tick();
    }
    let upgrade = ProtocolArg::Upgrade(UpgradeArg { mode: None });
    f.pic
        .upgrade_canister(
            f.backend,
            backend_wasm(),
            encode_args((upgrade,)).expect("encode backend upgrade"),
            None,
        )
        .expect("upgrade backend with partial refund-history scan");

    // Continue the persisted complete-prefix scan across the upgrade. Only
    // the combination of this post-TooOld complete prefix and the pinned
    // official ledger's monotonic expiry check permits a fresh mint tuple.
    for _ in 0..8 {
        assert!(refund(&f, f.sp, f.vault_id, f.amount, proof.clone()).is_err());
    }
    let recovered = refund(&f, f.sp, f.vault_id, f.amount, proof.clone())
        .expect("complete history absence plus TooOld must unblock compensation");
    assert!(recovered.refund_created_at_time > original_created_at_time);
    assert_eq!(recovered.refund_memo.len(), 32);
    assert_eq!(
        balance(&f),
        f.starting_balance - 600 * 10_000 - 600,
        "safe fresh identity restores the burned principal exactly once"
    );

    // Model the old request arriving after the expiry gate. The official
    // ledger must reject it as TooOld even after the new tuple paid.
    let mut old_memo = b"RSPRFND:".to_vec();
    old_memo.extend_from_slice(&proof.block_index.to_be_bytes());
    old_memo.extend_from_slice(&f.vault_id.to_be_bytes());
    let late_old_tuple: Result<Nat, TransferError> = match f
        .pic
        .update_call(
            f.ledger,
            f.backend,
            "icrc1_transfer",
            encode_one(TransferArg {
                from_subaccount: None,
                to: account(f.sp),
                fee: None,
                created_at_time: Some(original_created_at_time),
                memo: Some(old_memo),
                amount: Nat::from(f.amount),
            })
            .expect("encode delayed old refund tuple"),
        )
        .expect("old expired tuple returns a ledger error")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode old tuple reply"),
        WasmResult::Reject(message) => panic!("old tuple rejected as call: {message}"),
    };
    assert!(matches!(late_old_tuple, Err(TransferError::TooOld)));
    assert_eq!(balance(&f), f.starting_balance - 600 * 10_000 - 600);
    assert_eq!(refund(&f, f.sp, f.vault_id, f.amount, proof).unwrap(), recovered);
    assert_eq!(balance(&f), f.starting_balance - 600 * 10_000 - 600);
}

#[test]
fn committed_refund_lost_reply_recovers_original_mint_after_dedup_expiry() {
    let f = fixture_with_flaky_ledger();
    let (burn_index, proof) = burn(&f);

    // The flaky ledger commits the mint and its history/dedup rows, then traps
    // after an await so the backend sees a transport reject and retains its
    // exact pending transfer identity.
    expect_unit(
        f.pic
            .update_call(
                f.ledger,
                Principal::anonymous(),
                "set_trap_after_commit_for_caller",
                encode_one(Some(f.backend)).expect("encode trap caller"),
            )
            .expect("arm lost-reply injection"),
        "arm lost-reply injection",
    );
    let first_error = refund(&f, f.sp, f.vault_id, f.amount, proof.clone())
        .expect_err("injected lost reply should return a protocol error");
    assert!(
        format!("{first_error:?}").contains("Injected trap after committed transfer"),
        "the first refund must fail at the committed ledger reply: {first_error:?}"
    );
    assert_eq!(balance(&f), f.starting_balance);

    // This test ledger exposes explicit controls instead of implementing the
    // production ledger's 24-hour dedup timer. Advance beyond the window,
    // expire its dedup row, and have the next exact-tuple transfer return
    // TooOld. The backend must reconcile the committed block from history.
    f.pic.advance_time(Duration::from_secs(25 * 60 * 60));
    expect_unit(
        f.pic
            .update_call(
                f.ledger,
                Principal::anonymous(),
                "reset_dedup",
                encode_args(()).expect("encode dedup reset"),
            )
            .expect("expire flaky ledger dedup row"),
        "expire flaky ledger dedup row",
    );
    expect_unit(
        f.pic
            .update_call(
                f.ledger,
                Principal::anonymous(),
                "set_too_old_failures",
                encode_one(1u32).expect("encode TooOld injection"),
            )
            .expect("arm expired-tuple response"),
        "arm expired-tuple response",
    );
    let recovered = refund(&f, f.sp, f.vault_id, f.amount, proof.clone())
        .expect("same proof must recover the already committed refund");
    assert_eq!(recovered.vault_id, f.vault_id);
    assert_eq!(recovered.burn_block_index, burn_index);
    assert_eq!(recovered.amount_e8s, f.amount);
    assert_eq!(recovered.ledger, f.ledger);
    assert_eq!(recovered.recipient, f.sp);
    assert_eq!(recovered.refund_block_index, burn_index + 1);
    assert!(recovered.refund_created_at_time > 0);
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "recovery must not mint twice"
    );

    let replay = refund(&f, f.sp, f.vault_id, f.amount, proof)
        .expect("settled same-proof replay must return the original receipt");
    assert_eq!(
        replay, recovered,
        "replay must retain the original tuple and block"
    );
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "replay must not change balance"
    );
}

#[test]
fn real_burn_refund_requires_exact_registered_proof_and_restores_missing_vault_burn() {
    let f = fixture();
    let (block_index, proof) = burn(&f);
    assert_eq!(balance(&f), f.starting_balance - f.amount);

    assert!(refund(&f, f.attacker, f.vault_id, f.amount, proof.clone()).is_err());
    assert!(refund(&f, f.sp, f.vault_id, f.amount + 1, proof.clone()).is_err());
    assert!(refund(
        &f,
        f.sp,
        f.vault_id + 1,
        f.amount,
        SpWritedownProof {
            vault_id_memo: f.vault_id + 1,
            ..proof.clone()
        },
    )
    .is_err());
    assert!(refund(
        &f,
        f.sp,
        f.vault_id,
        f.amount,
        SpWritedownProof {
            block_index: block_index + 50_000,
            ..proof.clone()
        },
    )
    .is_err());
    assert_eq!(
        balance(&f),
        f.starting_balance - f.amount,
        "rejected requests must not mint"
    );

    // The fixture deliberately has no vault with this id. The authenticated
    // committed burn remains sufficient to restore exactly the burned funds.
    let receipt = refund(&f, f.sp, f.vault_id, f.amount, proof.clone())
        .expect("exact unused burn proof should be compensated");
    assert_eq!(receipt.vault_id, f.vault_id);
    assert_eq!(receipt.amount_e8s, f.amount);
    assert_eq!(receipt.ledger, f.ledger);
    assert_eq!(receipt.recipient, f.sp);
    assert_eq!(receipt.burn_block_index, block_index);
    assert!(consumed_proofs(&f).contains(&(SpProofLedger::IcusdBurn, block_index)));
    assert!(receipt.refund_memo.len() <= 64);
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "SP must receive the exact burned amount"
    );

    // Compensation consumes the proof globally. Even though the original
    // vault is absent, the same block cannot later authorize an absorption.
    let absorb_attempt = f
        .pic
        .update_call(
            f.backend,
            f.sp,
            "stability_pool_liquidate_debt_burned",
            encode_args((f.vault_id, f.amount, proof.clone())).expect("encode absorption attempt"),
        )
        .expect("absorption attempt");
    let absorb_attempt: Result<StabilityPoolLiquidationResult, ProtocolError> = match absorb_attempt
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode absorption attempt"),
        WasmResult::Reject(message) => panic!("absorption attempt rejected: {message}"),
    };
    assert!(
        absorb_attempt.is_err(),
        "a compensated proof must not authorize absorption"
    );
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "absorption retry must not change SP balance"
    );

    // The burn index is a real ledger block but is not a valid mint receipt.
    let wrong_reconcile = f
        .pic
        .update_call(
            f.backend,
            f.sp,
            "reconcile_stability_pool_burn_refund",
            encode_args((f.vault_id, f.amount, proof.clone(), block_index))
                .expect("encode bad reconcile"),
        )
        .expect("wrong reconcile call");
    let wrong_reconcile: Result<SpBurnRefundReceipt, ProtocolError> = match wrong_reconcile {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode wrong reconcile"),
        WasmResult::Reject(message) => panic!("wrong reconcile rejected: {message}"),
    };
    assert!(
        wrong_reconcile.is_err(),
        "burn block must not be accepted as the refund mint block"
    );
}

#[test]
fn refund_retry_upgrade_and_verified_reconciliation_reuse_one_mint() {
    let f = fixture();
    let (block_index, proof) = burn(&f);
    let receipt = refund(&f, f.sp, f.vault_id, f.amount, proof.clone()).expect("refund");
    assert_eq!(balance(&f), f.starting_balance);

    let retry = refund(&f, f.sp, f.vault_id, f.amount, proof.clone()).expect("idempotent retry");
    assert_eq!(
        retry, receipt,
        "retry must return the fixed compensation receipt"
    );
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "retry must not mint a second refund"
    );

    // PocketIC applies an install-rate limit to large Wasms. Advancing the
    // simulated clock mirrors the existing backend upgrade fixtures.
    f.pic.advance_time(Duration::from_secs(600));
    for _ in 0..10 {
        f.pic.tick();
    }
    let upgrade = ProtocolArg::Upgrade(UpgradeArg { mode: None });
    f.pic
        .upgrade_canister(
            f.backend,
            backend_wasm(),
            encode_args((upgrade,)).expect("encode upgrade"),
            None,
        )
        .expect("upgrade backend with settled refund");
    let after_upgrade = refund(&f, f.sp, f.vault_id, f.amount, proof.clone())
        .expect("retry settled refund after upgrade");
    assert_eq!(after_upgrade, receipt);
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "upgrade retry must not mint again"
    );

    let reconciled = f
        .pic
        .update_call(
            f.backend,
            f.sp,
            "reconcile_stability_pool_burn_refund",
            encode_args((f.vault_id, f.amount, proof, receipt.refund_block_index))
                .expect("encode reconciliation"),
        )
        .expect("verified reconciliation call");
    let reconciled: Result<SpBurnRefundReceipt, ProtocolError> = match reconciled {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reconciliation"),
        WasmResult::Reject(message) => panic!("reconciliation rejected: {message}"),
    };
    assert_eq!(
        reconciled.expect("actual refund mint block should reconcile"),
        receipt
    );
    assert_eq!(receipt.burn_block_index, block_index);
    assert_eq!(
        balance(&f),
        f.starting_balance,
        "reconciliation must not mint again"
    );
}

#[test]
fn failed_mint_obligation_survives_backend_upgrade_for_same_identity_retry() {
    // The official ledger accepts the 21-byte burn memo but rejects the
    // backend's 24-byte compensation memo. That makes the first mint fail only
    // after the backend has verified and durably journaled the burn proof.
    let f = fixture_with_memo_limit(21);
    let (block_index, proof) = burn(&f);
    let first = refund(&f, f.sp, f.vault_id, f.amount, proof.clone());
    assert!(
        first.is_err(),
        "ledger memo bound should leave the refund pending"
    );
    assert_eq!(balance(&f), f.starting_balance - f.amount);
    assert!(consumed_proofs(&f).contains(&(SpProofLedger::IcusdBurn, block_index)));

    f.pic.advance_time(Duration::from_secs(600));
    for _ in 0..10 {
        f.pic.tick();
    }
    let upgrade = ProtocolArg::Upgrade(UpgradeArg { mode: None });
    f.pic
        .upgrade_canister(
            f.backend,
            backend_wasm(),
            encode_args((upgrade,)).expect("encode backend upgrade"),
            None,
        )
        .expect("upgrade backend with pending refund");

    let retry = refund(&f, f.sp, f.vault_id, f.amount, proof.clone());
    assert!(
        retry.is_err(),
        "retry must retain and reuse the fixed memo tuple"
    );
    assert_eq!(
        balance(&f),
        f.starting_balance - f.amount,
        "failed retry must not mint"
    );

    // A rejected block is checked against the retained obligation. This
    // distinguishes a pending journal entry from a missing refund record.
    let reconcile = f
        .pic
        .update_call(
            f.backend,
            f.sp,
            "reconcile_stability_pool_burn_refund",
            encode_args((f.vault_id, f.amount, proof, block_index))
                .expect("encode invalid reconciliation"),
        )
        .expect("invalid reconciliation call");
    let reconcile: Result<SpBurnRefundReceipt, ProtocolError> = match reconcile {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode invalid reconciliation"),
        WasmResult::Reject(message) => panic!("reconciliation rejected: {message}"),
    };
    let error = reconcile.expect_err("the original burn block is not a refund mint receipt");
    assert!(
        format!("{error:?}").contains("SP refund block does not match the current persisted transfer identity"),
        "must retain a pending journal and verify supplied block: {error:?}"
    );
}
