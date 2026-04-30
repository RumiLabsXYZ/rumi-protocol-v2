// Shared PocketIC harness for 3pool integration tests.
//
// Wraps PocketIC + the three ICRC-1 ledgers + the 3pool canister behind a
// `ThreePoolHarness` struct so individual tests stay focused on assertions
// rather than setup boilerplate.

#![allow(dead_code)]

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIcBuilder, WasmResult};
use rumi_3pool::icrc3::{BlockWithId, GetBlocksArgs, GetBlocksResult};
use rumi_3pool::types::*;

// ─── Candid types for ICRC-1 ledger initialization ───

#[derive(CandidType, Deserialize)]
pub struct FeatureFlags {
    pub icrc2: bool,
}

#[derive(CandidType, Deserialize)]
pub struct ArchiveOptions {
    pub num_blocks_to_archive: u64,
    pub trigger_threshold: u64,
    pub controller_id: Principal,
    pub max_transactions_per_response: Option<u64>,
    pub max_message_size_bytes: Option<u64>,
    pub cycles_for_archive_creation: Option<u64>,
    pub node_max_memory_size_bytes: Option<u64>,
    pub more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize)]
pub enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize)]
pub struct LedgerInitArgs {
    pub minting_account: Account,
    pub fee_collector_account: Option<Account>,
    pub transfer_fee: candid::Nat,
    pub decimals: Option<u8>,
    pub max_memo_length: Option<u16>,
    pub token_name: String,
    pub token_symbol: String,
    pub metadata: Vec<(String, MetadataValue)>,
    pub initial_balances: Vec<(Account, candid::Nat)>,
    pub feature_flags: Option<FeatureFlags>,
    pub maximum_number_of_accounts: Option<u64>,
    pub accounts_overflow_trim_quantity: Option<u64>,
    pub archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize)]
pub enum LedgerArg {
    Init(LedgerInitArgs),
}

// ─── WASM loaders ───

pub fn icrc1_ledger_wasm() -> Vec<u8> {
    // Path: from src/rumi_3pool/tests/common/ back to src/rumi_3pool/ledger/
    include_bytes!("../../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

pub fn three_pool_wasm() -> Vec<u8> {
    // Both integration_test.rs and icrc3_hash_cache.rs include the same
    // WASM file. Build with `--features test_endpoints` if you need the
    // test_get_raw_block endpoint exposed (icrc3_hash_cache.rs needs it;
    // integration_test.rs doesn't but the endpoint being present is harmless).
    include_bytes!("../../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec()
}

// ─── Harness ───

pub struct ThreePoolHarness {
    pub pic: pocket_ic::PocketIc,
    pub admin: Principal,
    pub user: Principal,
    pub three_pool: Principal,
    pub ledgers: [Principal; 3],
}

impl ThreePoolHarness {
    pub fn icrc3_get_blocks(&self, start: u64, length: u64) -> Vec<BlockWithId> {
        let arg = vec![GetBlocksArgs {
            start: Nat::from(start),
            length: Nat::from(length),
        }];
        let bytes = self
            .pic
            .query_call(
                self.three_pool,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(arg).unwrap(),
            )
            .expect("icrc3_get_blocks query failed");
        let WasmResult::Reply(reply) = bytes else {
            panic!("icrc3_get_blocks rejected")
        };
        let result: GetBlocksResult = decode_one(&reply).unwrap();
        result.blocks
    }

    pub fn icrc3_log_length(&self) -> u64 {
        let arg = vec![GetBlocksArgs {
            start: Nat::from(0u64),
            length: Nat::from(0u64),
        }];
        let bytes = self
            .pic
            .query_call(
                self.three_pool,
                Principal::anonymous(),
                "icrc3_get_blocks",
                encode_one(arg).unwrap(),
            )
            .expect("icrc3_get_blocks query failed");
        let WasmResult::Reply(reply) = bytes else {
            panic!("icrc3_get_blocks rejected")
        };
        let result: GetBlocksResult = decode_one(&reply).unwrap();
        result.log_length.0.try_into().unwrap()
    }

    /// Read a single raw block via the test-only test_get_raw_block endpoint.
    /// Used by the reference impl in `icrc3_hash_cache.rs` to walk the chain
    /// from scratch without going through icrc3_get_blocks.
    pub fn get_raw_block(&self, id: u64) -> Icrc3Block {
        let bytes = self
            .pic
            .query_call(
                self.three_pool,
                Principal::anonymous(),
                "test_get_raw_block",
                encode_one(id).unwrap(),
            )
            .expect("test_get_raw_block query failed");
        let WasmResult::Reply(reply) = bytes else {
            panic!("test_get_raw_block rejected")
        };
        decode_one::<Option<Icrc3Block>>(&reply)
            .unwrap()
            .unwrap_or_else(|| panic!("block {id} missing"))
    }
}

struct LedgerSpec {
    name: &'static str,
    symbol: &'static str,
    decimals: u8,
    initial_balance: u128,
}

/// Deploy the pool with bootstrap liquidity. Then perform `n_swaps`
/// alternating swaps to generate additional ICRC-3 blocks.
///
/// Returns a harness with at least `n_swaps` + a handful of LP-token mint
/// blocks in the ICRC-3 log.
pub fn deploy_pool_with_liquidity_and_swaps(n_swaps: u64) -> ThreePoolHarness {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let minting_account = Principal::self_authenticating(&[100, 100, 100]);
    let user = Principal::self_authenticating(&[1, 2, 3, 4]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);

    let ledger_specs = [
        LedgerSpec {
            name: "icUSD",
            symbol: "icUSD",
            decimals: 8,
            initial_balance: 1_000_000_000_000_000, // 10M with 8 decimals
        },
        LedgerSpec {
            name: "ckUSDT",
            symbol: "ckUSDT",
            decimals: 6,
            initial_balance: 10_000_000_000_000, // 10M with 6 decimals
        },
        LedgerSpec {
            name: "ckUSDC",
            symbol: "ckUSDC",
            decimals: 6,
            initial_balance: 10_000_000_000_000, // 10M with 6 decimals
        },
    ];

    let mut ledger_ids = Vec::new();
    for spec in &ledger_specs {
        let ledger_id = pic.create_canister();
        pic.add_cycles(ledger_id, 2_000_000_000_000);
        let init_args = LedgerInitArgs {
            minting_account: Account {
                owner: minting_account,
                subaccount: None,
            },
            fee_collector_account: None,
            transfer_fee: candid::Nat::from(0u64),
            decimals: Some(spec.decimals),
            max_memo_length: Some(32),
            token_name: spec.name.to_string(),
            token_symbol: spec.symbol.to_string(),
            metadata: vec![],
            initial_balances: vec![(
                Account {
                    owner: user,
                    subaccount: None,
                },
                candid::Nat::from(spec.initial_balance),
            )],
            feature_flags: Some(FeatureFlags { icrc2: true }),
            maximum_number_of_accounts: None,
            accounts_overflow_trim_quantity: None,
            archive_options: ArchiveOptions {
                num_blocks_to_archive: 2000,
                trigger_threshold: 1000,
                controller_id: admin,
                max_transactions_per_response: None,
                max_message_size_bytes: None,
                cycles_for_archive_creation: None,
                node_max_memory_size_bytes: None,
                more_controller_ids: None,
            },
        };
        let encoded = encode_args((LedgerArg::Init(init_args),)).unwrap();
        pic.install_canister(ledger_id, icrc1_ledger_wasm(), encoded, None);
        ledger_ids.push(ledger_id);
    }

    let pool_init_args = ThreePoolInitArgs {
        tokens: [
            TokenConfig {
                ledger_id: ledger_ids[0],
                symbol: "icUSD".to_string(),
                decimals: 8,
                precision_mul: 10_000_000_000,
            },
            TokenConfig {
                ledger_id: ledger_ids[1],
                symbol: "ckUSDT".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
            TokenConfig {
                ledger_id: ledger_ids[2],
                symbol: "ckUSDC".to_string(),
                decimals: 6,
                precision_mul: 1_000_000_000_000,
            },
        ],
        initial_a: 100,
        swap_fee_bps: 4,
        admin_fee_bps: 5000,
        admin,
    };

    let pool_id = pic.create_canister();
    pic.add_cycles(pool_id, 2_000_000_000_000);
    pic.install_canister(
        pool_id,
        three_pool_wasm(),
        encode_one(pool_init_args).unwrap(),
        None,
    );

    // Approve pool on all 3 ledgers.
    for ledger_id in &ledger_ids {
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: pool_id,
                subaccount: None,
            },
            amount: candid::Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        pic.update_call(
            *ledger_id,
            user,
            "icrc2_approve",
            encode_one(approve_args).unwrap(),
        )
        .expect("icrc2_approve failed");
    }

    // Seed pool with 1M / 1M / 1M.
    let add_liq_amounts: Vec<u128> = vec![
        100_000_000_000_000, // 1M icUSD (8 dec)
        1_000_000_000_000,   // 1M ckUSDT (6 dec)
        1_000_000_000_000,   // 1M ckUSDC (6 dec)
    ];
    let res = pic
        .update_call(
            pool_id,
            user,
            "add_liquidity",
            encode_args((add_liq_amounts, 0u128)).unwrap(),
        )
        .expect("add_liquidity failed");
    if let WasmResult::Reply(bytes) = res {
        let r: Result<candid::Nat, ThreePoolError> = decode_one(&bytes).unwrap();
        r.expect("add_liquidity err");
    }

    let harness = ThreePoolHarness {
        pic,
        admin,
        user,
        three_pool: pool_id,
        ledgers: [ledger_ids[0], ledger_ids[1], ledger_ids[2]],
    };

    // Generate n_swaps additional ICRC-3 blocks by transferring 1 unit of LP
    // token to a dummy recipient for each iteration. LP transfers are logged as
    // ICRC-3 Transfer blocks. Swaps are NOT logged in the ICRC-3 chain (only
    // LP token mint/burn/transfer/approve operations are).
    let dummy_recipient = Principal::self_authenticating(&[99, 88, 77]);
    for _ in 0..n_swaps {
        let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
            from_subaccount: None,
            to: Account {
                owner: dummy_recipient,
                subaccount: None,
            },
            fee: None,
            created_at_time: None,
            memo: None,
            amount: candid::Nat::from(1u64),
        };
        let res = harness
            .pic
            .update_call(
                harness.three_pool,
                harness.user,
                "icrc1_transfer",
                encode_one(transfer_args).unwrap(),
            )
            .expect("icrc1_transfer failed");
        if let WasmResult::Reply(bytes) = res {
            let r: Result<
                candid::Nat,
                icrc_ledger_types::icrc1::transfer::TransferError,
            > = decode_one(&bytes).unwrap();
            r.expect("icrc1_transfer returned err");
        }
    }

    harness
}
