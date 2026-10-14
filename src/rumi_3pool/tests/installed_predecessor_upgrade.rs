//! Upgrade exact 3pool production Wasm pairs with populated state.
//!
//! The predecessor fixture is reproducibly rebuilt from commit `5f776101`
//! (the source base of PR #541); its SHA-256 matches the installed module
//! (`74a81602f61ee08023bc1a6746f4961aebcdb8c7f98b0019c9eefbb63a3998ae`).
//! The candidate is the production artifact at SHA-256
//! `d1bc983879dc4a385ec87a398168f798802027b428bbef0f1c8382435ed96c6e`.
//! The October 2026 successor test starts with that same installed Wasm and
//! upgrades to the PR #566 production artifact at SHA-256
//! `7e5510608d9b2d0f27cc259afb3f9fe94681229a6b11298c1f703881cb481c6d`.
//! This ignored, opt-in test reads both artifacts from explicit environment
//! paths and verifies their hashes before installing or upgrading.

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{deploy_pool_with_liquidity_fee_and_swaps_with_wasm, ThreePoolHarness};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use pocket_ic::WasmResult;
use rumi_3pool::payouts::{GlobalPayoutInventoryPage, PayoutEntitlement};
use rumi_3pool::types::{PoolStatus, ThreePoolError, ThreePoolPendingClaim};
use sha2::{Digest, Sha256};

const PREDECESSOR_WASM_SHA256: &str =
    "74a81602f61ee08023bc1a6746f4961aebcdb8c7f98b0019c9eefbb63a3998ae";
const CANDIDATE_WASM_SHA256: &str =
    "d1bc983879dc4a385ec87a398168f798802027b428bbef0f1c8382435ed96c6e";
const OCTOBER_CANDIDATE_WASM_SHA256: &str =
    "7e5510608d9b2d0f27cc259afb3f9fe94681229a6b11298c1f703881cb481c6d";
const PR577_CANDIDATE_WASM_SHA256: &str =
    "95b075c7c9f14d43bb8f44599264b37ff7ad8b3f84c49d216f1a46846500a991";
const PRE_UPGRADE_ALLOWANCE: u128 = 1_234_567;
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

fn global_payout_inventory(h: &ThreePoolHarness) -> Vec<PayoutEntitlement> {
    let mut entries = Vec::new();
    let mut after_id = None;
    loop {
        let page: Result<GlobalPayoutInventoryPage, ThreePoolError> = reply(
            h.pic
                .query_call(
                    h.three_pool,
                    h.admin,
                    "get_global_payout_inventory",
                    encode_args((after_id, 20u64)).unwrap(),
                )
                .expect("global payout inventory query"),
        );
        let page = page.expect("admin can read the global payout inventory");
        entries.extend(page.entries);
        match page.next_cursor {
            Some(cursor) => {
                assert!(after_id.is_none_or(|previous| cursor > previous));
                after_id = Some(cursor);
            }
            None => {
                assert_eq!(entries.len() as u64, page.total_count);
                return entries;
            }
        }
    }
}

#[test]
#[ignore = "opt-in exact production Wasm upgrade; requires PocketIC and both pinned artifact paths"]
fn exact_installed_predecessor_upgrade_preserves_populated_pool_and_payouts() {
    verify_upgrade(PREDECESSOR_WASM_SHA256, CANDIDATE_WASM_SHA256);
}

#[test]
#[ignore = "opt-in exact production Wasm upgrade; requires PocketIC and both pinned artifact paths"]
fn october_candidate_upgrade_preserves_populated_pool_and_payouts() {
    verify_upgrade(CANDIDATE_WASM_SHA256, OCTOBER_CANDIDATE_WASM_SHA256);
}

#[test]
#[ignore = "opt-in exact production Wasm upgrade; requires PocketIC and both pinned artifact paths"]
fn pr577_candidate_upgrade_preserves_populated_pool_and_payouts() {
    verify_upgrade(
        OCTOBER_CANDIDATE_WASM_SHA256,
        PR577_CANDIDATE_WASM_SHA256,
    );
}

fn verify_upgrade(predecessor_hash: &str, candidate_hash: &str) {
    let predecessor_wasm =
        read_hash_pinned_wasm("RUMI_3POOL_INSTALLED_PREDECESSOR_WASM", predecessor_hash);
    let candidate_wasm = read_hash_pinned_wasm("RUMI_3POOL_CANDIDATE_WASM", candidate_hash);

    // This deploys the exact predecessor and seeds its pool through the real
    // public add_liquidity endpoint against the shared PocketIC ledgers.
    let h = deploy_pool_with_liquidity_fee_and_swaps_with_wasm(0, 0, predecessor_wasm);
    let spender = Principal::self_authenticating(&[51, 52, 53]);
    let shareholder = Principal::self_authenticating(&[54, 55, 56]);

    // Seed a second LP owner, a valid post-cutover allowance, and accrued
    // admin fees before taking the pre-upgrade snapshot.
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
    approve_lp(&h, spender, PRE_UPGRADE_ALLOWANCE);
    assert_eq!(allowance(&h, h.user, spender), PRE_UPGRADE_ALLOWANCE);

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
    let payouts_before = global_payout_inventory(&h);
    assert!(
        !payouts_before.is_empty(),
        "swap seeds a retained payout identity"
    );
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
        PRE_UPGRADE_ALLOWANCE,
        "valid pre-upgrade LP allowance survives"
    );
    assert_eq!(ledger_balances(&h, h.user), user_tokens_before);
    assert_eq!(ledger_balances(&h, h.three_pool), pool_tokens_before);
    assert_eq!(
        pending_claims(&h),
        claims_before,
        "claims exposed by the installed predecessor survive"
    );
    assert_eq!(
        global_payout_inventory(&h),
        payouts_before,
        "global payout identities survive"
    );

    // A newly approved allowance and the payout inventory must survive a
    // second candidate-to-candidate upgrade as well.
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
    assert_eq!(global_payout_inventory(&h), payouts_before);
    assert_eq!(ledger_balances(&h, h.user), user_tokens_before);
    assert_eq!(ledger_balances(&h, h.three_pool), pool_tokens_before);
}
