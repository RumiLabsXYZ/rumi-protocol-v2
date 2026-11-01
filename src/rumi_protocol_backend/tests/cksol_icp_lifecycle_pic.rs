//! Local PocketIC integration: actual ledger balances, minter cycle refund, and backend upgrade.
//! Build fresh Wasms with `cargo build -p rumi_protocol_backend --release --target
//! wasm32-unknown-unknown` and `cargo build --manifest-path
//! src/rumi_protocol_backend/tests/cksol_icp_mock/Cargo.toml --release --target
//! wasm32-unknown-unknown --locked`. Set CKSOL_PIC_BACKEND_WASM,
//! CKSOL_PIC_MOCK_WASM, and POCKET_IC_BIN (server 7.0.0), then run
//! `cargo test -p rumi_protocol_backend --test cksol_icp_lifecycle_pic -- --ignored`.
//! All canisters run locally. Pending-call upgrade is not exercised here.
use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::cksol_icp_paid::{deposit_cap, CkSolIcpPayment, CkSolIcpTerms};
use rumi_protocol_backend::cksol_paid::CkSolPaymentPhase;
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
            initial_balances: vec![(account(owner), Nat::from(initial))],
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
}
