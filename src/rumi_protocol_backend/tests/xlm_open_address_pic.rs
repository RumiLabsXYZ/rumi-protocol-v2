//! Isolated PocketIC smoke for the test_endpoints-gated native-XLM address open.
//!
//! Requires the backend Wasm at the conventional release target path to have
//! been built with `--features test_endpoints`. PocketIC's II subnet must
//! provision `test_key_1` for the threshold-Ed25519 public-key call.

use candid::{
    decode_one, encode_args, encode_one, CandidType, Deserialize, Encode, Nat, Principal,
};
use icrc_ledger_types::icrc1::account::Account;
use pocket_ic::{PocketIcBuilder, WasmResult};

#[derive(CandidType, Deserialize)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize)]
enum ProtocolArg {
    Init(ProtocolInitArg),
}

#[derive(CandidType, Deserialize)]
struct XlmVaultOpenInfo {
    vault_id: u64,
    custody_address: String,
    network_passphrase: String,
    expires_at_ns: u64,
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
enum LedgerArg {
    #[serde(rename = "Init")]
    Init(LedgerInitArgs),
    #[serde(rename = "Upgrade")]
    Upgrade(Option<()>),
}

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

fn developer() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

#[test]
fn test_endpoint_opens_only_a_testnet_xlm_address() {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();
    let backend = pic.create_canister();
    pic.add_cycles(backend, 100_000_000_000_000);
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 100_000_000_000_000);

    let ledger_init = LedgerInitArgs {
        minting_account: Account {
            owner: backend,
            subaccount: None,
        },
        fee_collector_account: None,
        transfer_fee: Nat::from(10_000u64),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "Test icUSD".into(),
        token_symbol: "icUSD".into(),
        metadata: vec![],
        initial_balances: vec![],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 1_000,
            trigger_threshold: 2_000,
            controller_id: backend,
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
        encode_one(LedgerArg::Init(ledger_init)).expect("encode ledger init"),
        None,
    );

    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: ledger,
        icp_ledger_principal: ledger,
        fee_e8s: 10_000,
        developer_principal: developer(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend,
        backend_wasm(),
        encode_args((init,)).expect("encode backend init"),
        None,
    );
    for _ in 0..5 {
        pic.tick();
    }

    let reply = pic
        .update_call(
            backend,
            developer(),
            "open_xlm_vault",
            encode_args(()).unwrap(),
        )
        .expect("open_xlm_vault call");
    let opened: Result<XlmVaultOpenInfo, candid::Reserved> = match reply {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode XLM open result"),
        WasmResult::Reject(message) => panic!("open_xlm_vault rejected: {message}"),
    };
    let opened = opened.expect("threshold XLM address derivation should succeed");

    assert!(
        rumi_protocol_backend::chains::stellar::is_valid_classic_address(&opened.custody_address),
        "returned custody address must be a checksummed classic G-address"
    );
    assert_eq!(
        opened.network_passphrase,
        "Test SDF Network ; September 2015"
    );
    let now_ns = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time after epoch")
        .as_nanos() as u64;
    assert!(opened.expires_at_ns > now_ns);
    assert_eq!(opened.vault_id, 1);

    let vault_count: u64 = match pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_vault_count",
            Encode!().unwrap(),
        )
        .expect("query vault count")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode vault count"),
        WasmResult::Reject(message) => panic!("get_vault_count rejected: {message}"),
    };
    assert_eq!(vault_count, 0, "address open must not create a Vault");

    let balance: Nat = match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            Encode!(&Account {
                owner: developer(),
                subaccount: None
            })
            .unwrap(),
        )
        .expect("query developer icUSD balance")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode icUSD balance"),
        WasmResult::Reject(message) => panic!("icrc1_balance_of rejected: {message}"),
    };
    assert_eq!(balance, Nat::from(0u8), "address open must not mint icUSD");
}
