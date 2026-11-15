//! Install and drive the REAL `ic-icrc1-ledger` wasm (`src/ledger/ic-icrc1-ledger.wasm`)
//! in PocketIC, with ICRC-2 enabled and a deliberately tiny archive threshold so
//! scenario tests can exercise archived-block reads.
//!
//! Call and result types come from `icrc-ledger-types` (already a dependency of
//! the bridge crate); only the ledger's init-argument shape is mirrored here
//! (copied from `src/ledger/ledger.did`).

use candid::{decode_one, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::{PocketIc, WasmResult};
use serde_bytes::ByteBuf;
use std::path::PathBuf;

/// Fee used by the harness ledger (0.0001 icUSD).
pub const TRANSFER_FEE_E8S: u64 = 10_000;

// ----- init argument mirror (ledger.did `LedgerArg` / `InitArgs`) -----

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(ByteBuf),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct FeatureFlags {
    pub icrc2: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ArchiveOptions {
    pub num_blocks_to_archive: u64,
    pub max_transactions_per_response: Option<u64>,
    pub trigger_threshold: u64,
    pub max_message_size_bytes: Option<u64>,
    pub cycles_for_archive_creation: Option<u64>,
    pub node_max_memory_size_bytes: Option<u64>,
    pub controller_id: Principal,
    pub more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    pub minting_account: Account,
    pub fee_collector_account: Option<Account>,
    pub transfer_fee: Nat,
    pub decimals: Option<u8>,
    pub max_memo_length: Option<u16>,
    pub token_symbol: String,
    pub token_name: String,
    pub metadata: Vec<(String, MetadataValue)>,
    pub initial_balances: Vec<(Account, Nat)>,
    pub feature_flags: Option<FeatureFlags>,
    pub maximum_number_of_accounts: Option<u64>,
    pub accounts_overflow_trim_quantity: Option<u64>,
    pub archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum LedgerArg {
    Init(InitArgs),
    // `Upgrade : opt UpgradeArgs` is not needed by the harness.
}

/// Knobs for [`deploy_icrc_ledger`].
#[derive(Clone, Debug)]
pub struct LedgerConfig {
    pub minting_account: Account,
    pub initial_balances: Vec<(Account, u128)>,
    /// Becomes the archive canisters' controller (the test principal).
    pub controller: Principal,
    /// Blocks held in the ledger before it spawns an archive. Default 50.
    pub archive_trigger_threshold: u64,
    /// Blocks moved to the archive per archiving round. Default 20.
    pub num_blocks_to_archive: u64,
    pub token_symbol: String,
    pub max_memo_length: Option<u16>,
}

impl LedgerConfig {
    pub fn new(
        minting_account: Account,
        controller: Principal,
        initial_balances: Vec<(Account, u128)>,
    ) -> LedgerConfig {
        LedgerConfig {
            minting_account,
            initial_balances,
            controller,
            archive_trigger_threshold: 50,
            num_blocks_to_archive: 20,
            token_symbol: "icUSD".to_string(),
            max_memo_length: None,
        }
    }
}

pub fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn ledger_wasm() -> Vec<u8> {
    let path = std::env::var("ICRC_LEDGER_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ledger/ic-icrc1-ledger.wasm")
        });
    std::fs::read(&path).unwrap_or_else(|e| panic!("read ledger wasm {}: {e}", path.display()))
}

/// Install the ICRC ledger (ICRC-2 on, 8 decimals, fee 10_000 e8s) with a small
/// archive threshold and enough cycles to spawn archive canisters.
pub fn deploy_icrc_ledger(pic: &PocketIc, cfg: &LedgerConfig) -> Principal {
    let id = pic.create_canister();
    // Archive creation attaches `cycles_for_archive_creation` out of the ledger's balance.
    pic.add_cycles(id, 500_000_000_000_000);
    let init = InitArgs {
        minting_account: cfg.minting_account.clone(),
        fee_collector_account: None,
        transfer_fee: Nat::from(TRANSFER_FEE_E8S),
        decimals: Some(8),
        max_memo_length: cfg.max_memo_length,
        token_symbol: cfg.token_symbol.clone(),
        token_name: "Rumi icUSD (harness)".to_string(),
        metadata: vec![],
        initial_balances: cfg
            .initial_balances
            .iter()
            .map(|(a, v)| (a.clone(), Nat::from(*v)))
            .collect(),
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: cfg.num_blocks_to_archive,
            max_transactions_per_response: None,
            trigger_threshold: cfg.archive_trigger_threshold,
            max_message_size_bytes: None,
            cycles_for_archive_creation: Some(100_000_000_000_000),
            node_max_memory_size_bytes: None,
            controller_id: cfg.controller,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        id,
        ledger_wasm(),
        candid::encode_args((LedgerArg::Init(init),)).expect("encode init"),
        None,
    );
    id
}

// ----- call helpers -----

fn update<T: CandidType, R: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    method: &str,
    arg: T,
) -> R {
    match pic
        .update_call(ledger, sender, method, encode_one(arg).expect("encode"))
        .unwrap_or_else(|e| panic!("{method} call failed: {e}"))
    {
        WasmResult::Reply(b) => {
            decode_one(&b).unwrap_or_else(|e| panic!("decode {method} reply: {e}"))
        }
        WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
    }
}

fn query_raw<T: CandidType>(pic: &PocketIc, ledger: Principal, method: &str, arg: T) -> Vec<u8> {
    match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            method,
            encode_one(arg).expect("encode"),
        )
        .unwrap_or_else(|e| panic!("{method} query failed: {e}"))
    {
        WasmResult::Reply(b) => b,
        WasmResult::Reject(m) => panic!("{method} rejected: {m}"),
    }
}

pub fn icrc1_balance_of(pic: &PocketIc, ledger: Principal, who: &Account) -> Nat {
    decode_one(&query_raw(pic, ledger, "icrc1_balance_of", who.clone())).expect("decode balance")
}

pub fn icrc1_total_supply(pic: &PocketIc, ledger: Principal) -> Nat {
    decode_one(&query_raw(pic, ledger, "icrc1_total_supply", ())).expect("decode total supply")
}

pub fn icrc1_fee(pic: &PocketIc, ledger: Principal) -> Nat {
    decode_one(&query_raw(pic, ledger, "icrc1_fee", ())).expect("decode fee")
}

/// `icrc1_transfer` as `sender`. Returns the block index or the ledger error.
pub fn icrc1_transfer(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    arg: TransferArg,
) -> Result<Nat, TransferError> {
    update(pic, ledger, sender, "icrc1_transfer", arg)
}

/// Convenience: transfer `amount` from `sender`'s default account to `to`
/// with the harness fee supplied explicitly.
pub fn transfer(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    to: Account,
    amount: u128,
) -> Result<Nat, TransferError> {
    icrc1_transfer(
        pic,
        ledger,
        sender,
        TransferArg {
            from_subaccount: None,
            to,
            amount: Nat::from(amount),
            fee: Some(Nat::from(TRANSFER_FEE_E8S)),
            memo: None,
            created_at_time: None,
        },
    )
}

/// `icrc2_approve` as `sender` (the token owner).
pub fn icrc2_approve(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    arg: ApproveArgs,
) -> Result<Nat, ApproveError> {
    update(pic, ledger, sender, "icrc2_approve", arg)
}

/// Raw Candid bytes of `icrc3_get_blocks([{start, length}])`.
pub fn icrc3_get_blocks_raw(pic: &PocketIc, ledger: Principal, start: u64, length: u64) -> Vec<u8> {
    query_raw(
        pic,
        ledger,
        "icrc3_get_blocks",
        vec![GetBlocksRequest {
            start: Nat::from(start),
            length: Nat::from(length),
        }],
    )
}

pub fn icrc3_get_blocks(
    pic: &PocketIc,
    ledger: Principal,
    start: u64,
    length: u64,
) -> GetBlocksResult {
    decode_one(&icrc3_get_blocks_raw(pic, ledger, start, length)).expect("decode GetBlocksResult")
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct GetArchivesArgs {
    from: Option<Principal>,
}

/// Raw Candid bytes of `icrc3_get_archives({ from = null })`: one
/// `{ canister_id; start; end }` record per archive canister.
pub fn icrc3_get_archives_raw(pic: &PocketIc, ledger: Principal) -> Vec<u8> {
    query_raw(
        pic,
        ledger,
        "icrc3_get_archives",
        GetArchivesArgs { from: None },
    )
}

/// Render raw Candid reply bytes as Candid text (for recording exact shapes).
pub fn describe_candid(bytes: &[u8]) -> String {
    candid::IDLArgs::from_bytes(bytes)
        .map(|a| a.to_string())
        .unwrap_or_else(|e| format!("<undecodable candid: {e}>"))
}
