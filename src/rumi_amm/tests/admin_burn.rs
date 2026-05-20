// Integration tests for `admin_burn_subaccount_balance`.
//
// This endpoint is the recovery path for the 2026-05-19 AMM1 pool_id
// mismatch incident: the backend's `donate_icusd_to_amm1` was hardcoded
// with `pool_id = "3USD_ICP"` but the AMM stores the pool under
// `make_pool_id(token_a, token_b)`, so newly minted icUSD ended up at a
// phantom subaccount no pool can see. This endpoint lets an admin
// transfer the stuck balance to the icUSD ledger's minting account
// (which the ICRC-1 ledger treats as a burn).
//
// Tests cover:
//   (a) admin burns a stuck balance
//   (b) non-admin caller is rejected
//   (c) burn of an empty subaccount returns Ok(0)
//   (d) burn of a dust balance (<= ledger fee) is a no-op

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::TransferArg;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use sha2::{Digest, Sha256};

use rumi_amm::types::*;

// icUSD ledger principal that the AMM canister hardcodes via `ICUSD_LEDGER`.
const ICUSD_LEDGER_PRINCIPAL: &str = "t6bor-paaaa-aaaap-qrd5q-cai";

// ─── Candid types for ICRC-1 ledger initialization ───
//
// Duplicated from `pocket_ic_tests.rs` because Rust's integration-test
// model treats each `tests/*.rs` file as its own crate and the harness
// in that file lives behind a `#[cfg(test)]` boundary. Keeping the
// shape local here avoids needing to extract a `tests/common/` module
// just for this single endpoint test.

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
    transfer_fee: candid::Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, candid::Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize)]
enum MetadataValue {
    Nat(candid::Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn amm_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}

/// Mirrors `rumi_amm::reward_subaccount_for` so the test can derive the
/// AMM's reward subaccount client-side. Used here just to seed a known
/// non-empty subaccount in the burn test.
fn reward_subaccount(pool_id: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"rumi_amm:rewards:");
    h.update(pool_id.as_bytes());
    let digest = h.finalize();
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&digest);
    sub
}

const LEDGER_FEE: u128 = 10_000;

struct BurnEnv {
    pic: PocketIc,
    amm_id: Principal,
    icusd_ledger_id: Principal,
    icusd_minting: Principal,
    admin: Principal,
    other: Principal,
}

/// Set up an AMM environment plus a test icUSD ledger pinned at the
/// AMM's hardcoded `ICUSD_LEDGER` principal.
fn setup_burn_env() -> BurnEnv {
    let pic = PocketIcBuilder::new()
        .with_application_subnet()
        .build();

    let icusd_minting = Principal::self_authenticating(&[200, 201, 202]);
    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let other = Principal::self_authenticating(&[42, 42, 42, 42]);

    // Install icUSD ledger at the exact canister ID the AMM hardcodes.
    let icusd_ledger_id_target = Principal::from_text(ICUSD_LEDGER_PRINCIPAL)
        .expect("invalid icUSD ledger principal");
    let icusd_ledger_id = pic
        .create_canister_with_id(Some(admin), None, icusd_ledger_id_target)
        .expect("create icusd ledger at hardcoded id");
    pic.add_cycles(icusd_ledger_id, 2_000_000_000_000);

    let icusd_init = LedgerInitArgs {
        minting_account: Account { owner: icusd_minting, subaccount: None },
        fee_collector_account: None,
        // Non-zero ledger fee mirrors the live icUSD ledger and exercises
        // the fee-handling path in the burn endpoint.
        transfer_fee: candid::Nat::from(LEDGER_FEE as u64),
        decimals: Some(8),
        max_memo_length: Some(32),
        token_name: "icUSD".to_string(),
        token_symbol: "icUSD".to_string(),
        metadata: vec![],
        initial_balances: vec![],
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
    let icusd_arg = LedgerArg::Init(icusd_init);
    let encoded = encode_args((icusd_arg,)).expect("encode icusd init");
    pic.install_canister(icusd_ledger_id, icrc1_ledger_wasm(), encoded, Some(admin));

    // Install the AMM.
    let amm_id = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm_id, 2_000_000_000_000);
    let amm_init = AmmInitArgs { admin };
    let encoded = encode_one(amm_init).expect("encode amm init");
    pic.install_canister(amm_id, amm_wasm(), encoded, Some(admin));

    BurnEnv { pic, amm_id, icusd_ledger_id, icusd_minting, admin, other }
}

/// Mint icUSD into the given subaccount of the AMM via `icrc1_transfer`
/// from the minting account (ICRC-1 treats this as a mint, no fee).
fn mint_icusd_to_amm_subaccount(env: &BurnEnv, subaccount: [u8; 32], amount: u128) {
    let args = TransferArg {
        from_subaccount: None,
        to: Account { owner: env.amm_id, subaccount: Some(subaccount) },
        amount: candid::Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = env
        .pic
        .update_call(
            env.icusd_ledger_id,
            env.icusd_minting,
            "icrc1_transfer",
            encode_one(args).unwrap(),
        )
        .expect("mint icrc1_transfer call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError> =
                decode_one(&bytes).expect("decode failed");
            res.expect("mint returned Err");
        }
        WasmResult::Reject(msg) => panic!("mint rejected: {}", msg),
    }
}

/// Query icUSD balance at the given AMM subaccount.
fn icusd_balance_of(env: &BurnEnv, subaccount: Option<[u8; 32]>) -> u128 {
    let acct = Account { owner: env.amm_id, subaccount };
    let result = env
        .pic
        .query_call(
            env.icusd_ledger_id,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(acct).unwrap(),
        )
        .expect("icrc1_balance_of failed");
    match result {
        WasmResult::Reply(bytes) => {
            let n: candid::Nat = decode_one(&bytes).expect("decode failed");
            n.0.try_into().unwrap()
        }
        WasmResult::Reject(msg) => panic!("icrc1_balance_of rejected: {}", msg),
    }
}

/// Call `admin_burn_subaccount_balance` from `caller` and return the
/// decoded `Result<u128, AmmError>`.
fn call_burn(
    env: &BurnEnv,
    caller: Principal,
    ledger: Principal,
    subaccount: Vec<u8>,
) -> Result<u128, AmmError> {
    let result = env
        .pic
        .update_call(
            env.amm_id,
            caller,
            "admin_burn_subaccount_balance",
            encode_args((ledger, subaccount)).unwrap(),
        )
        .expect("admin_burn_subaccount_balance call failed");
    match result {
        WasmResult::Reply(bytes) => {
            let res: Result<candid::Nat, AmmError> =
                decode_one(&bytes).expect("decode failed");
            res.map(|n| n.0.try_into().unwrap())
        }
        WasmResult::Reject(msg) => panic!("admin_burn_subaccount_balance rejected: {}", msg),
    }
}

// ════════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════════

/// (a) Admin burns a stuck balance.
///
/// Seeds the AMM canister's reward subaccount for an arbitrary pool ID
/// with icUSD, then calls `admin_burn_subaccount_balance` as admin. The
/// endpoint must transfer the full balance to the ledger's minting
/// account (a burn). ICRC-1 ledgers do not charge a fee on burns, so
/// the source subaccount nets to zero and the returned amount equals
/// the original stuck balance.
#[test]
fn admin_burn_stuck_balance_succeeds() {
    let env = setup_burn_env();
    let pool_id = "3USD_ICP"; // the phantom pool_id from the incident
    let sub = reward_subaccount(pool_id);

    let stuck_amount: u128 = 89_00000000; // ~$89, matching incident scale
    mint_icusd_to_amm_subaccount(&env, sub, stuck_amount);
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        stuck_amount,
        "subaccount should be seeded before burn",
    );

    let burned = call_burn(&env, env.admin, env.icusd_ledger_id, sub.to_vec())
        .expect("admin burn should succeed");

    // Burns to the minting account charge no fee per ICRC-1, so the
    // entire balance is burned in a single transfer.
    assert_eq!(
        burned, stuck_amount,
        "burned amount should equal full stuck balance",
    );
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        0,
        "subaccount should be empty after burn",
    );
}

/// (b) Non-admin caller is rejected.
///
/// The burn must be admin-only. Seed a stuck balance, call as a
/// non-admin principal, expect `AmmError::Unauthorized` and no balance
/// change.
#[test]
fn admin_burn_unauthorized_rejected() {
    let env = setup_burn_env();
    let pool_id = "3USD_ICP";
    let sub = reward_subaccount(pool_id);

    let stuck_amount: u128 = 10_00000000;
    mint_icusd_to_amm_subaccount(&env, sub, stuck_amount);

    let result = call_burn(&env, env.other, env.icusd_ledger_id, sub.to_vec());
    assert!(
        matches!(result, Err(AmmError::Unauthorized)),
        "non-admin caller must get Unauthorized, got {:?}",
        result,
    );
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        stuck_amount,
        "balance must be unchanged after rejected call",
    );
}

/// (c) Burn of an empty subaccount returns Ok(0).
///
/// When the subaccount holds no funds (or fewer than the ledger fee),
/// the endpoint is a no-op and returns Ok(0). No transfer attempted.
#[test]
fn admin_burn_empty_subaccount_noop() {
    let env = setup_burn_env();
    let pool_id = "nonexistent_pool";
    let sub = reward_subaccount(pool_id);

    // Sanity: subaccount is empty.
    assert_eq!(icusd_balance_of(&env, Some(sub)), 0);

    let burned = call_burn(&env, env.admin, env.icusd_ledger_id, sub.to_vec())
        .expect("burn of empty subaccount should be Ok(0)");
    assert_eq!(burned, 0, "burn of empty subaccount should return 0");
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        0,
        "subaccount must remain empty",
    );
}

/// (d) Burn of a dust balance (<= ledger fee) is a no-op.
///
/// ICRC-1 ledgers enforce `amount >= min_burn_amount` (typically equal
/// to the transfer fee) on burns, so a balance at or below the fee
/// cannot be burned cleanly. The endpoint short-circuits to `Ok(0)`
/// and leaves the dust in place — re-running once the balance grows
/// past the fee will burn it normally. This test covers the
/// `balance <= fee` branch that the other tests don't exercise.
#[test]
fn admin_burn_dust_balance_below_fee_noop() {
    let env = setup_burn_env();
    let pool_id = "dust_pool";
    let sub = reward_subaccount(pool_id);

    // Seed exactly LEDGER_FEE worth of icUSD: positive balance, but
    // not strictly greater than the fee, so the burn short-circuits.
    let dust: u128 = LEDGER_FEE;
    mint_icusd_to_amm_subaccount(&env, sub, dust);
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        dust,
        "subaccount should hold dust before the no-op call",
    );

    let burned = call_burn(&env, env.admin, env.icusd_ledger_id, sub.to_vec())
        .expect("burn of dust balance should be Ok(0) (no-op)");
    assert_eq!(
        burned, 0,
        "dust at-or-below ledger fee must return 0 burned",
    );
    assert_eq!(
        icusd_balance_of(&env, Some(sub)),
        dust,
        "dust must stay put — endpoint refuses to attempt an un-burnable transfer",
    );
}
