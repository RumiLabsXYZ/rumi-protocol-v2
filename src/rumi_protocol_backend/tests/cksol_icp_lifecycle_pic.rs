//! Local PocketIC integration: actual ledger balances, minter cycle refund, and backend upgrade.
//! Build fresh Wasms with `cargo build -p rumi_protocol_backend --release --target
//! wasm32-unknown-unknown` and `cargo build --manifest-path
//! src/rumi_protocol_backend/tests/cksol_icp_mock/Cargo.toml --release --target
//! wasm32-unknown-unknown --locked`. Set CKSOL_PIC_BACKEND_WASM,
//! CKSOL_PIC_MOCK_WASM, and POCKET_IC_BIN (server 7.0.0), then run
//! `cargo test -p rumi_protocol_backend --test cksol_icp_lifecycle_pic -- --ignored`.
//! All canisters run locally, including an upgrade while a minter call is pending.
use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::cksol_icp_paid::{deposit_cap, CkSolIcpPayment, CkSolIcpTerms};
use rumi_protocol_backend::cksol_paid::CkSolPaymentPhase;
use rumi_protocol_backend::state::{CkSolRelayAttempt, CkSolRelayStatus};
use rumi_protocol_backend::{InitArg, ProtocolArg, UpgradeArg};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

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
enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
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
enum LedgerArgument {
    Init(LedgerInitArgs),
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
fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}
fn reply<T: for<'de> Deserialize<'de> + CandidType>(r: WasmResult) -> T {
    match r {
        WasmResult::Reply(b) => decode_one(&b).unwrap(),
        WasmResult::Reject(e) => panic!("canister rejected: {e}"),
    }
}
fn update<T: for<'de> Deserialize<'de> + CandidType>(
    p: &PocketIc,
    id: Principal,
    sender: Principal,
    method: &str,
    arg: Vec<u8>,
) -> T {
    reply(p.update_call(id, sender, method, arg).unwrap())
}
fn query<T: for<'de> Deserialize<'de> + CandidType>(
    p: &PocketIc,
    id: Principal,
    method: &str,
    arg: Vec<u8>,
) -> T {
    reply(
        p.query_call(id, Principal::anonymous(), method, arg)
            .unwrap(),
    )
}
fn wasm(name: &str) -> Vec<u8> {
    std::fs::read(
        std::env::var(name).unwrap_or_else(|_| panic!("set {name} to the freshly built Wasm")),
    )
    .unwrap()
}

#[test]
#[ignore = "requires freshly built canister Wasms and PocketIC 7.0.0"]
fn real_ledger_settlement_and_upgrade_preserve_receipt() {
    let p = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let now = 1_700_000_000u64;
    p.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(now));
    let owner = Principal::self_authenticating(b"cksol-pic-owner");
    let dev = Principal::self_authenticating(b"cksol-pic-developer");
    let pending_owner = Principal::self_authenticating(b"cksol-pic-pending");
    let backend = p.create_canister();
    p.add_cycles(backend, 100_000_000_000_000);
    let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
    let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
    for id in [ledger, cmc, minter] {
        p.create_canister_with_id(None, None, id).unwrap();
        p.add_cycles(id, 5_000_000_000_000);
    }
    let mock = wasm("CKSOL_PIC_MOCK_WASM");
    p.install_canister(cmc, mock.clone(), encode_args(()).unwrap(), None);
    p.install_canister(minter, mock, encode_args(()).unwrap(), None);
    let fee = 10_000u64;
    let initial = 100_000_000u64;
    p.install_canister(
        ledger,
        include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec(),
        encode_one(LedgerArgument::Init(LedgerInitArgs {
            minting_account: account(dev),
            fee_collector_account: None,
            transfer_fee: Nat::from(fee),
            decimals: Some(8),
            max_memo_length: Some(64),
            token_name: "Test ICP".into(),
            token_symbol: "ICP".into(),
            metadata: vec![],
            initial_balances: vec![
                (account(owner), Nat::from(initial)),
                (account(pending_owner), Nat::from(initial)),
            ],
            feature_flags: Some(FeatureFlags { icrc2: true }),
            maximum_number_of_accounts: None,
            accounts_overflow_trim_quantity: None,
            archive_options: ArchiveOptions {
                num_blocks_to_archive: 2000,
                max_transactions_per_response: None,
                trigger_threshold: 1000,
                max_message_size_bytes: None,
                cycles_for_archive_creation: None,
                node_max_memory_size_bytes: None,
                controller_id: dev,
                more_controller_ids: None,
            },
        }))
        .unwrap(),
        None,
    );
    let backend_wasm = wasm("CKSOL_PIC_BACKEND_WASM");
    p.install_canister(
        backend,
        backend_wasm.clone(),
        encode_one(ProtocolArg::Init(InitArg {
            xrc_principal: Principal::management_canister(),
            icusd_ledger_principal: ledger,
            icp_ledger_principal: ledger,
            fee_e8s: fee,
            developer_principal: dev,
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }))
        .unwrap(),
        None,
    );
    let reserve: Result<(), String> = update(
        &p,
        backend,
        dev,
        "set_cksol_icp_reserve",
        encode_one(1_000_000_000_000u64).unwrap(),
    );
    reserve.unwrap();
    let rate = 40_000u64;
    let cap = deposit_cap(rate, fee).unwrap();
    let approval: Result<Nat, candid::types::reserved::Reserved> = update(
        &p,
        ledger,
        owner,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(cap + fee),
            expected_allowance: None,
            expires_at: None,
            fee: Some(Nat::from(fee)),
            memo: None,
            created_at_time: None,
        })
        .unwrap(),
    );
    assert!(approval.is_ok());
    let r: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        owner,
        "start_cksol_deposit_with_icp",
        encode_args((1u64, cap, rate, now, fee)).unwrap(),
    );
    let paid = r.unwrap();
    assert_eq!(paid.phase, CkSolPaymentPhase::Complete);
    assert_eq!(paid.deposit_id, Some(42));
    assert_eq!(paid.net_cycles, 100_000_000_000);
    assert_eq!(paid.service_fee_cycles, 5_000_000_000);
    let balance: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(owner)).unwrap(),
    );
    assert_eq!(
        balance,
        Nat::from(initial - cap - fee - fee + paid.refundable_e8s)
    );
    assert_eq!(paid.charged_e8s, 2_650_000);
    assert_eq!(paid.operating_fee_cycles, 1_000_000_000);
    assert_eq!(paid.actual_refund_fee_e8s, fee);
    assert_eq!(cap, paid.charged_e8s + paid.refundable_e8s + fee);
    let mut digest = Sha256::new();
    digest.update(b"rumi-cksol-icp-escrow-v1");
    digest.update(owner.as_slice());
    digest.update(1u64.to_be_bytes());
    let subaccount: [u8; 32] = digest.finalize().into();
    let escrow: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(Account {
            owner: backend,
            subaccount: Some(subaccount),
        })
        .unwrap(),
    );
    assert_eq!(escrow, Nat::from(paid.charged_e8s));
    assert_eq!(
        balance.clone() + escrow + Nat::from(3 * fee),
        Nat::from(initial)
    );
    p.upgrade_canister(
        backend,
        backend_wasm,
        encode_one(ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: None,
        }))
        .unwrap(),
        None,
    )
    .unwrap();
    let restored: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(owner).unwrap(),
    );
    assert_eq!(restored.unwrap(), paid);
    let replay: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        owner,
        "resume_cksol_icp_payment",
        encode_one(1u64).unwrap(),
    );
    assert_eq!(replay.unwrap(), paid);
    let after: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(owner)).unwrap(),
    );
    assert_eq!(balance, after);

    // A different unfunded, unapproved caller receives a definite ledger rejection;
    // no ICP is collected and the reservation is released, including after upgrade.
    let other = Principal::self_authenticating(b"cksol-pic-unfunded");
    let reserve: Result<(), String> = update(
        &p,
        backend,
        dev,
        "set_cksol_icp_reserve",
        encode_one(1_000_000_000_000u64).unwrap(),
    );
    reserve.unwrap();
    let rejected: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        other,
        "start_cksol_deposit_with_icp",
        encode_args((2u64, cap, rate, now, fee)).unwrap(),
    );
    let rejected = rejected.unwrap();
    assert_eq!(rejected.phase, CkSolPaymentPhase::FundingRejected);
    assert_eq!(rejected.funding_block, None);
    let empty: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(other)).unwrap(),
    );
    assert_eq!(empty, Nat::from(0u8));
    p.upgrade_canister(
        backend,
        wasm("CKSOL_PIC_BACKEND_WASM"),
        encode_one(ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: None,
        }))
        .unwrap(),
        None,
    )
    .unwrap();
    let restored_rejection: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(other).unwrap(),
    );
    assert_eq!(restored_rejection.unwrap(), rejected);
    let terms: CkSolIcpTerms = query(&p, backend, "get_cksol_icp_terms", encode_args(()).unwrap());
    assert_eq!(terms.reserve_available_cycles, 1_000_000_000_000);

    // Pause a real inter-canister mint reply and upgrade while the backend
    // retains its DepositPending journal. Neither upgrade nor resume may
    // charge the user or dispatch a second mint.
    let _: () = update(&p, minter, dev, "set_hold", encode_one(true).unwrap());
    let approval: Result<Nat, candid::types::reserved::Reserved> = update(
        &p,
        ledger,
        pending_owner,
        "icrc2_approve",
        encode_one(ApproveArgs {
            from_subaccount: None,
            spender: account(backend),
            amount: Nat::from(cap + fee),
            expected_allowance: None,
            expires_at: None,
            fee: Some(Nat::from(fee)),
            memo: None,
            created_at_time: None,
        })
        .unwrap(),
    );
    assert!(approval.is_ok());
    let _message = p
        .submit_call(
            backend,
            pending_owner,
            "start_cksol_deposit_with_icp",
            encode_args((3u64, cap, rate, now, fee)).unwrap(),
        )
        .unwrap();
    let mut observed = false;
    for _ in 0..80 {
        p.tick();
        let calls: u64 = query(&p, minter, "calls", encode_args(()).unwrap());
        if calls == 2 {
            observed = true;
            break;
        }
    }
    assert!(observed, "minter call did not reach the held response");
    let pending: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(pending_owner).unwrap(),
    );
    let pending = pending.unwrap();
    assert_eq!(pending.phase, CkSolPaymentPhase::DepositPending);
    assert!(pending.funding_block.is_some());
    let before: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(pending_owner)).unwrap(),
    );
    assert_eq!(before, Nat::from(initial - cap - 2 * fee));
    let cycles_before_upgrade = p.cycle_balance(backend);
    p.upgrade_canister(
        backend,
        wasm("CKSOL_PIC_BACKEND_WASM"),
        encode_one(ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: None,
        }))
        .unwrap(),
        None,
    )
    .unwrap();
    let cycles_after_upgrade = p.cycle_balance(backend);
    let restored: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(pending_owner).unwrap(),
    );
    let restored = restored.unwrap();
    assert_eq!(restored.phase, CkSolPaymentPhase::RefundReady);
    assert_eq!(restored.refundable_e8s, cap - fee);
    assert_eq!(restored.charged_e8s, 0);
    assert_eq!(restored.service_fee_cycles, 0);
    assert_eq!(restored.operating_fee_cycles, 0);
    let resumed: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        pending_owner,
        "resume_cksol_icp_payment",
        encode_one(3u64).unwrap(),
    );
    let recovered = resumed.unwrap();
    assert_eq!(recovered.phase, CkSolPaymentPhase::Complete);
    assert_eq!(recovered.charged_e8s, 0);
    assert_eq!(recovered.service_fee_cycles, 0);
    assert_eq!(recovered.operating_fee_cycles, 0);
    assert_eq!(recovered.refundable_e8s, cap - fee);
    let relay: Option<CkSolRelayAttempt> = query(
        &p,
        backend,
        "get_cksol_deposit",
        encode_one(pending_owner).unwrap(),
    );
    assert!(matches!(
        relay.unwrap().status,
        CkSolRelayStatus::SubmittedOrUnknown
    ));
    let calls: u64 = query(&p, minter, "calls", encode_args(()).unwrap());
    assert_eq!(calls, 2);
    let after: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(pending_owner)).unwrap(),
    );
    assert_eq!(after, Nat::from(initial - 3 * fee));

    // Let the original mocked minter reply arrive after the upgrade. Verify
    // that a delayed callback cannot cause a second debit or mint dispatch.
    for _ in 0..70 {
        p.tick();
    }
    let completed: u64 = query(&p, minter, "completed", encode_args(()).unwrap());
    assert_eq!(completed, 2, "the delayed minter call must finish");
    let later: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(pending_owner).unwrap(),
    );
    assert_eq!(later.unwrap(), recovered);
    let calls: u64 = query(&p, minter, "calls", encode_args(()).unwrap());
    assert_eq!(calls, 2);
    let after_late: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(account(pending_owner)).unwrap(),
    );
    assert_eq!(after_late, after);
    let terms: CkSolIcpTerms = query(&p, backend, "get_cksol_icp_terms", encode_args(()).unwrap());
    assert_eq!(
        terms.reserve_available_cycles, 0,
        "unattributed cycle refund must not replenish the allocation"
    );
    let cycles_after_late = p.cycle_balance(backend);
    println!("pending minter cycle balances: before upgrade={cycles_before_upgrade}, after upgrade={cycles_after_upgrade}, after late reply={cycles_after_late}");
    let mut digest = Sha256::new();
    digest.update(b"rumi-cksol-icp-escrow-v1");
    digest.update(pending_owner.as_slice());
    digest.update(3u64.to_be_bytes());
    let subaccount: [u8; 32] = digest.finalize().into();
    let held: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(Account {
            owner: backend,
            subaccount: Some(subaccount),
        })
        .unwrap(),
    );
    assert_eq!(held, Nat::from(0u8));
    // Later operator ID binding does not cause a second refund.
    let binding: Result<(), String> = update(
        &p,
        backend,
        dev,
        "recover_cksol_deposit_id",
        encode_args((pending_owner, 42u64)).unwrap(),
    );
    binding.unwrap();
    let resumed: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        pending_owner,
        "resume_cksol_icp_payment",
        encode_one(3u64).unwrap(),
    );
    assert_eq!(resumed.unwrap(), recovered);
    let still_held: Nat = query(
        &p,
        ledger,
        "icrc1_balance_of",
        encode_one(Account {
            owner: backend,
            subaccount: Some(subaccount),
        })
        .unwrap(),
    );
    assert_eq!(held, still_held);
}

#[test]
#[ignore = "requires freshly built canister Wasms and PocketIC 7.0.0"]
fn refund_reply_lost_on_upgrade_retries_same_ledger_tuple() {
    let p = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_application_subnet()
        .build();
    let now = 1_700_000_000u64;
    p.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(now));
    let owner = Principal::self_authenticating(b"cksol-pic-refund-owner");
    let dev = Principal::self_authenticating(b"cksol-pic-refund-developer");
    let backend = p.create_canister();
    p.add_cycles(backend, 100_000_000_000_000);
    let ledger = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
    let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
    let mock = wasm("CKSOL_PIC_MOCK_WASM");
    for id in [ledger, cmc, minter] {
        p.create_canister_with_id(None, None, id).unwrap();
        p.add_cycles(id, 5_000_000_000_000);
        p.install_canister(id, mock.clone(), encode_args(()).unwrap(), None);
    }
    let _: () = update(&p, ledger, dev, "set_hold", encode_one(true).unwrap());
    let backend_wasm = wasm("CKSOL_PIC_BACKEND_WASM");
    p.install_canister(
        backend,
        backend_wasm.clone(),
        encode_one(ProtocolArg::Init(InitArg {
            xrc_principal: Principal::management_canister(),
            icusd_ledger_principal: ledger,
            icp_ledger_principal: ledger,
            fee_e8s: 10_000,
            developer_principal: dev,
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }))
        .unwrap(),
        None,
    );
    let reserve: Result<(), String> = update(
        &p,
        backend,
        dev,
        "set_cksol_icp_reserve",
        encode_one(1_000_000_000_000u64).unwrap(),
    );
    reserve.unwrap();
    let cap = deposit_cap(40_000, 10_000).unwrap();
    let _message = p
        .submit_call(
            backend,
            owner,
            "start_cksol_deposit_with_icp",
            encode_args((1u64, cap, 40_000u64, now, 10_000u64)).unwrap(),
        )
        .unwrap();
    let mut seen = false;
    for _ in 0..80 {
        p.tick();
        let count: u64 = query(&p, ledger, "refund_calls", encode_args(()).unwrap());
        if count == 1 {
            seen = true;
            break;
        }
    }
    assert!(seen, "ledger refund was not dispatched");
    let pending: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(owner).unwrap(),
    );
    assert_eq!(pending.unwrap().phase, CkSolPaymentPhase::RefundPending);
    p.upgrade_canister(
        backend,
        backend_wasm,
        encode_one(ProtocolArg::Upgrade(UpgradeArg {
            mode: None,
            description: None,
        }))
        .unwrap(),
        None,
    )
    .unwrap();
    let restored: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(owner).unwrap(),
    );
    assert_eq!(restored.unwrap().phase, CkSolPaymentPhase::RefundPending);
    let result: Result<CkSolIcpPayment, String> = update(
        &p,
        backend,
        owner,
        "resume_cksol_icp_payment",
        encode_one(1u64).unwrap(),
    );
    let payment = result.unwrap();
    assert_eq!(payment.phase, CkSolPaymentPhase::Complete);
    assert_eq!(payment.refund_block, Some(Nat::from(2u8)));
    let count: u64 = query(&p, ledger, "refund_calls", encode_args(()).unwrap());
    assert_eq!(
        count, 2,
        "mock accepts only a byte-identical duplicate tuple"
    );
    for _ in 0..70 {
        p.tick();
    }
    let completed: u64 = query(&p, ledger, "refund_completed", encode_args(()).unwrap());
    assert_eq!(
        completed, 1,
        "the delayed original refund reply must complete"
    );
    let later: Option<CkSolIcpPayment> = query(
        &p,
        backend,
        "get_cksol_icp_payment",
        encode_one(owner).unwrap(),
    );
    assert_eq!(later.unwrap(), payment);
    let count: u64 = query(&p, ledger, "refund_calls", encode_args(()).unwrap());
    assert_eq!(count, 2);
}
