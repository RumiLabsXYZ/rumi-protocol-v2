//! Upgrade the exact installed PR #483 3pool production Wasm with populated
//! state into the exact candidate production Wasm under review.
//!
//! The predecessor fixture is reproducibly rebuilt from PR #483 merge commit
//! `7b2d0dbf690fa0a64007e99ba016a89e31f61491`; its SHA-256 matches the
//! installed module (`9797bf2454923ef41d6ef6a003435e0430aaa2b2b413734c6d9e6e921889dfc0`).
//! The candidate is the production artifact at SHA-256
//! `ae8491da84212c1e49b2d1b7f21598e8e82532cb3395d6066dc5e2880bd50034`.
//! This ignored, opt-in test reads both artifacts from explicit environment
//! paths and verifies their hashes before installing or upgrading.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{deploy_pool_with_liquidity_fee_and_swaps_with_wasm, ThreePoolHarness};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use pocket_ic::WasmResult;
use rumi_3pool::types::{PoolStatus, ThreePoolError, ThreePoolPendingClaim};
use sha2::{Digest, Sha256};

const PREDECESSOR_WASM_SHA256: &str =
    "9797bf2454923ef41d6ef6a003435e0430aaa2b2b413734c6d9e6e921889dfc0";
const CANDIDATE_WASM_SHA256: &str =
    "ae8491da84212c1e49b2d1b7f21598e8e82532cb3395d6066dc5e2880bd50034";
const LEGACY_ALLOWANCE: u128 = 1_234_567;
const LP_TRANSFER: u128 = 100_000;

fn reply<T: for<'de> candid::Deserialize<'de> + candid::CandidType>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode reply"),
        WasmResult::Reject(message) => panic!("call rejected: {message}"),
    }
}

fn read_hash_pinned_wasm(variable: &str, expected_hash: &str) -> Vec<u8> {
    let path = std::env::var(variable)
        .unwrap_or_else(|_| panic!("set {variable} to the exact production Wasm artifact"));
    let wasm = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let actual_hash = format!("{:x}", Sha256::digest(&wasm));
    assert_eq!(
        actual_hash, expected_hash,
        "{variable}={path} is not the pinned production artifact"
    );
    wasm
}

fn allowance(h: &ThreePoolHarness, owner: Principal, spender: Principal) -> u128 {
    let result: Allowance = reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "icrc2_allowance",
                encode_one(AllowanceArgs {
                    account: Account {
                        owner,
                        subaccount: None,
                    },
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                })
                .unwrap(),
            )
            .expect("icrc2_allowance query"),
    );
    result.allowance.0.try_into().expect("allowance fits u128")
}

fn approve_lp(h: &ThreePoolHarness, spender: Principal, amount: u128) {
    let result: Result<Nat, ApproveError> = reply(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "icrc2_approve",
                encode_one(ApproveArgs {
                    from_subaccount: None,
                    spender: Account {
                        owner: spender,
                        subaccount: None,
                    },
                    amount: Nat::from(amount),
                    expected_allowance: None,
                    expires_at: None,
                    fee: None,
                    memo: None,
                    created_at_time: None,
                })
                .unwrap(),
            )
            .expect("icrc2_approve update"),
    );
    result.expect("approve LP allowance");
}

fn lp_balance(h: &ThreePoolHarness, owner: Principal) -> u128 {
    let balance: Nat = reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "icrc1_balance_of",
                encode_one(Account {
                    owner,
                    subaccount: None,
                })
                .unwrap(),
            )
            .expect("LP balance query"),
    );
    balance.0.try_into().expect("LP balance fits u128")
}

fn total_supply(h: &ThreePoolHarness) -> u128 {
    let supply: Nat = reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "icrc1_total_supply",
                encode_args(()).unwrap(),
            )
            .expect("LP total supply query"),
    );
    supply.0.try_into().expect("LP supply fits u128")
}

fn pool_status(h: &ThreePoolHarness) -> PoolStatus {
    reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_pool_status",
                encode_args(()).unwrap(),
            )
            .expect("pool status query"),
    )
}

fn admin_fees(h: &ThreePoolHarness) -> Vec<u128> {
    reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_admin_fees",
                encode_args(()).unwrap(),
            )
            .expect("admin fee query"),
    )
}

fn ledger_balances(h: &ThreePoolHarness, owner: Principal) -> [u128; 3] {
    std::array::from_fn(|index| {
        let balance: Nat = reply(
            h.pic
                .query_call(
                    h.ledgers[index],
                    Principal::anonymous(),
                    "icrc1_balance_of",
                    encode_one(Account {
                        owner,
                        subaccount: None,
                    })
                    .unwrap(),
                )
                .expect("underlying ledger balance query"),
        );
        balance.0.try_into().expect("ledger balance fits u128")
    })
}

fn pending_claims(
    h: &ThreePoolHarness,
) -> Vec<(u64, Principal, u8, Principal, String, u128, String)> {
    let claim_count: u64 = reply(
        h.pic
            .query_call(
                h.three_pool,
                Principal::anonymous(),
                "get_pending_claim_count",
                encode_args(()).unwrap(),
            )
            .expect("get_pending_claim_count query"),
    );
    let mut claims = Vec::new();
    let mut offset = 0u64;
    while offset < claim_count {
        let limit = (claim_count - offset).min(1000);
        let page: Vec<ThreePoolPendingClaim> = reply(
            h.pic
                .query_call(
                    h.three_pool,
                    Principal::anonymous(),
                    "get_pending_claims",
                    encode_args((offset, limit)).unwrap(),
                )
                .expect("get_pending_claims query"),
        );
        assert!(!page.is_empty(), "claim pagination stopped before count");
        let next_offset = offset
            .checked_add(page.len() as u64)
            .expect("pending claim offset overflow");
        assert!(
            next_offset > offset && next_offset <= claim_count,
            "claim page exceeded the reported total"
        );
        claims.extend(page);
        offset = next_offset;
    }
    assert_eq!(claims.len() as u64, claim_count);
    claims
        .into_iter()
        .map(|claim| {
            (
                claim.id,
                claim.claimant,
                claim.token_index,
                claim.ledger,
                claim.symbol,
                claim.amount,
                claim.reason,
            )
        })
        .collect()
}

#[test]
#[ignore = "opt-in exact production Wasm upgrade; requires PocketIC and both pinned artifact paths"]
fn exact_installed_predecessor_upgrade_preserves_populated_pool_and_cuts_over_allowances_once() {
    let predecessor_wasm = read_hash_pinned_wasm(
        "RUMI_3POOL_INSTALLED_PREDECESSOR_WASM",
        PREDECESSOR_WASM_SHA256,
    );
    let candidate_wasm = read_hash_pinned_wasm("RUMI_3POOL_CANDIDATE_WASM", CANDIDATE_WASM_SHA256);

    // This deploys the exact predecessor and seeds its pool through the real
    // public add_liquidity endpoint against the shared PocketIC ledgers.
    let h = deploy_pool_with_liquidity_fee_and_swaps_with_wasm(0, 0, predecessor_wasm);
    let spender = Principal::self_authenticating(&[51, 52, 53]);
    let shareholder = Principal::self_authenticating(&[54, 55, 56]);

    // Seed a second LP owner, an allowance that must be invalidated, and
    // accrued admin fees before taking the pre-upgrade snapshot.
    let transfer = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: shareholder,
            subaccount: None,
        },
        fee: None,
        created_at_time: None,
        memo: None,
        amount: Nat::from(LP_TRANSFER),
    };
    let transfer_result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> = reply(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "icrc1_transfer",
                encode_one(transfer).unwrap(),
            )
            .expect("LP transfer update"),
    );
    transfer_result.expect("seed second LP shareholder");
    approve_lp(&h, spender, LEGACY_ALLOWANCE);
    assert_eq!(allowance(&h, h.user, spender), LEGACY_ALLOWANCE);

    let swap: Result<u128, ThreePoolError> = reply(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, 100_000_000_000u128, 0u128)).unwrap(),
            )
            .expect("pre-upgrade swap"),
    );
    swap.expect("accrue pre-upgrade swap fees");

    let user_lp_before = lp_balance(&h, h.user);
    let shareholder_lp_before = lp_balance(&h, shareholder);
    let supply_before = total_supply(&h);
    let reserves_before = pool_status(&h).balances;
    let admin_fees_before = admin_fees(&h);
    assert!(admin_fees_before.iter().any(|fee| *fee > 0));
    assert_eq!(user_lp_before + shareholder_lp_before, supply_before);
    let user_tokens_before = ledger_balances(&h, h.user);
    let pool_tokens_before = ledger_balances(&h, h.three_pool);
    let claims_before = pending_claims(&h);
    eprintln!(
        "installed predecessor pending claim count: {}",
        claims_before.len()
    );

    h.pic
        .upgrade_canister(
            h.three_pool,
            candidate_wasm.clone(),
            encode_args(()).unwrap(),
            None,
        )
        .expect("exact predecessor-to-candidate upgrade failed");

    assert_eq!(
        lp_balance(&h, h.user),
        user_lp_before,
        "user LP shares survive"
    );
    assert_eq!(
        lp_balance(&h, shareholder),
        shareholder_lp_before,
        "second owner's LP shares survive"
    );
    assert_eq!(total_supply(&h), supply_before, "LP supply survives");
    assert_eq!(
        pool_status(&h).balances,
        reserves_before,
        "pool reserves survive"
    );
    assert_eq!(
        admin_fees(&h),
        admin_fees_before,
        "admin fee liabilities survive"
    );
    assert_eq!(
        allowance(&h, h.user, spender),
        0,
        "ambiguous legacy LP allowance is invalidated once"
    );
    assert_eq!(ledger_balances(&h, h.user), user_tokens_before);
    assert_eq!(ledger_balances(&h, h.three_pool), pool_tokens_before);
    assert_eq!(
        pending_claims(&h),
        claims_before,
        "claims exposed by the installed predecessor survive"
    );

    // A newly approved allowance is post-cutover data. A second upgrade must
    // retain it, proving the legacy clear is one-time rather than every-upgrade.
    const POST_CUTOVER_ALLOWANCE: u128 = 765_432;
    approve_lp(&h, spender, POST_CUTOVER_ALLOWANCE);
    assert_eq!(allowance(&h, h.user, spender), POST_CUTOVER_ALLOWANCE);
    h.pic
        .upgrade_canister(h.three_pool, candidate_wasm, encode_args(()).unwrap(), None)
        .expect("second candidate upgrade failed");
    assert_eq!(
        allowance(&h, h.user, spender),
        POST_CUTOVER_ALLOWANCE,
        "post-cutover LP allowance survives subsequent upgrades"
    );
    assert_eq!(lp_balance(&h, h.user), user_lp_before);
    assert_eq!(lp_balance(&h, shareholder), shareholder_lp_before);
    assert_eq!(total_supply(&h), supply_before);
    assert_eq!(pool_status(&h).balances, reserves_before);
    assert_eq!(admin_fees(&h), admin_fees_before);
    assert_eq!(ledger_balances(&h, h.user), user_tokens_before);
    assert_eq!(ledger_balances(&h, h.three_pool), pool_tokens_before);
}
