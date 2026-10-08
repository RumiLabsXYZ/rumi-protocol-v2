// src/rumi_3pool/tests/icrc3_hash_cache.rs
//
// Verifies the ICRC-3 hash-chain cache optimization (Task 6) produces output
// bit-identical to a from-scratch reference computation.
//
// The reference impl walks the raw block log from block 0, building the hash
// chain incrementally without touching block_hashes::get. The optimized path
// (icrc3_get_blocks) uses the cached tip hash. If any byte differs, this test
// catches it before threeusd_index detects a chain break in production.

mod common;

use candid::{decode_one, encode_one, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use rumi_3pool::icrc3::{BlockWithId, GetBlocksArgs, Icrc3Value};
use rumi_3pool::types::{Icrc3Transaction, ThreePoolPendingClaim};

use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};
use pocket_ic::WasmResult;

fn reply(result: WasmResult) -> Vec<u8> {
    match result {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("canister rejected: {message}"),
    }
}

fn assert_block_transaction_metadata(
    harness: &ThreePoolHarness,
    block_id: u64,
    memo: &[u8],
    created_at_time: u64,
) {
    let block = harness.icrc3_get_blocks(block_id, 1);
    let Icrc3Value::Map(fields) = &block[0].block else {
        panic!("ICRC-3 block must be a map");
    };
    let Icrc3Value::Map(tx) = fields
        .iter()
        .find(|(key, _)| key == "tx")
        .map(|(_, value)| value)
        .expect("block has tx map")
    else {
        panic!("ICRC-3 tx must be a map");
    };
    assert!(tx.iter().any(|(key, value)| {
        key == "memo" && value == &Icrc3Value::Blob(memo.to_vec())
    }));
    assert!(tx.iter().any(|(key, value)| {
        key == "ts" && value == &Icrc3Value::Nat(Nat::from(created_at_time))
    }));
}

/// Verify that the WASM running in the harness was built with
/// `--features test_endpoints`. The two test-only endpoints
/// (`test_get_raw_block`, `test_clear_hash_cache`) are gated behind that
/// feature; without it, this whole test file is unrunnable.
///
/// Probing here gives a clear, actionable error message rather than
/// letting individual update / query calls panic with PocketIC's
/// generic "method not found" rejection deep inside an assertion loop.
fn assert_test_endpoints_built(harness: &ThreePoolHarness) {
    let probe = harness.pic.query_call(
        harness.three_pool,
        candid::Principal::anonymous(),
        "test_get_raw_block",
        candid::encode_one(0u64).unwrap(),
    );
    assert!(
        probe.is_ok(),
        "test-only endpoints missing from WASM. \
         Rebuild with: cargo build -p rumi_3pool --release \
         --target wasm32-unknown-unknown --features test_endpoints"
    );
}

/// Reference implementation: rebuild the entire hash chain from block 0,
/// returning the ICRC-3 Value form of the requested range. This mirrors the
/// pre-optimization O(N) logic and deliberately does NOT use block_hashes::get.
fn reference_get_blocks(
    harness: &ThreePoolHarness,
    start: u64,
    length: u64,
) -> Vec<BlockWithId> {
    let log_length = harness.icrc3_log_length();
    let end = std::cmp::min(start.saturating_add(length), log_length);
    if start >= end {
        return vec![];
    }

    let mut prev_hash: Option<[u8; 32]> = None;
    let mut out = Vec::new();

    for i in 0..end {
        let block = harness.get_raw_block(i);
        let encoded: Icrc3Value =
            rumi_3pool::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
        let block_hash = rumi_3pool::certification::hash_value(&encoded);

        if i >= start {
            out.push(BlockWithId {
                id: Nat::from(i),
                block: encoded,
            });
        }

        prev_hash = Some(block_hash);
    }

    out
}

#[test]
fn icrc3_get_blocks_matches_reference_for_all_windows() {
    // Deploy pool and run 50 swaps to produce a meaningful-length ICRC-3 log.
    // Each swap emits at least one block, plus add_liquidity emits blocks too.
    let harness = deploy_pool_with_liquidity_and_swaps(50);
    assert_test_endpoints_built(&harness);

    let log_length = harness.icrc3_log_length();
    // 1 Mint (initial AddLiquidity) + 50 Transfers (deploy_pool's loop) = 51.
    // Lower than this means LP token operations stopped generating ICRC-3
    // blocks somewhere -- a regression worth catching here.
    assert!(log_length >= 51, "expected at least 51 blocks, got {log_length}");

    // Adjust window list based on actual log_length.
    let test_windows: Vec<(u64, u64)> = vec![
        (0, 1),
        (0, 10),
        (0, log_length),
        (log_length.saturating_sub(1), 1),
        (log_length.saturating_sub(1), 2),  // straddle: last valid + one past end
        (log_length / 2, 5),
        (log_length, 10),     // off-the-end -> empty
        (log_length + 1, 5),  // past end -> empty
        (5, 0),               // zero length -> empty
    ];

    for (start, length) in test_windows {
        let optimized = harness.icrc3_get_blocks(start, length);
        let reference = reference_get_blocks(&harness, start, length);

        assert_eq!(
            optimized.len(),
            reference.len(),
            "block count mismatch at window (start={start}, length={length}): \
             optimized={}, reference={}",
            optimized.len(),
            reference.len(),
        );

        for (a, b) in optimized.iter().zip(reference.iter()) {
            assert_eq!(
                a.id, b.id,
                "id mismatch at window (start={start}, length={length}): \
                 optimized id={:?}, reference id={:?}",
                a.id, b.id,
            );
            assert_eq!(
                a.block, b.block,
                "block content mismatch at window (start={start}, length={length}) \
                 for block id={:?}",
                a.id,
            );
        }
    }
}

#[test]
fn icrc3_get_blocks_cycle_cost_is_constant_in_log_length() {
    // We measure cycles burned per `icrc3_get_blocks` UPDATE call
    // (replicated execution, i.e. the production polling path). With
    // 200 blocks vs 50 blocks, the per-call cost should be approximately
    // constant -- the hallmark of an O(range) algorithm. Without the
    // cache, cost would be ~4x higher at 200 blocks.

    fn cycles_per_call(harness: &common::ThreePoolHarness, n_calls: u32) -> u128 {
        let log_length = harness.icrc3_log_length();
        let last = log_length.saturating_sub(1);
        let arg = encode_one(vec![GetBlocksArgs {
            start: Nat::from(last),
            length: Nat::from(1u64),
        }]).unwrap();

        let before = harness.pic.cycle_balance(harness.three_pool);
        for _ in 0..n_calls {
            let _ = harness.pic
                .update_call(harness.three_pool, candid::Principal::anonymous(),
                             "icrc3_get_blocks", arg.clone())
                .expect("icrc3_get_blocks update failed");
        }
        let after = harness.pic.cycle_balance(harness.three_pool);
        let burned = before.saturating_sub(after);
        burned / (n_calls as u128)
    }

    // Build two harnesses with different block counts.
    let small = common::deploy_pool_with_liquidity_and_swaps(50);
    let large = common::deploy_pool_with_liquidity_and_swaps(200);

    let small_per_call = cycles_per_call(&small, 50);
    let large_per_call = cycles_per_call(&large, 50);

    eprintln!(
        "icrc3_get_blocks cycles/call: 50 blocks={small_per_call}, 200 blocks={large_per_call}"
    );

    // With the cache, cost is dominated by the per-update message base
    // plus a single block encode + hash. We expect the ratio to stay
    // well under 1.5x even though log_length grew 4x. Without the cache,
    // ratio would approach 4x. (Measured on the optimized branch: ratio
    // is ~1.001x, so the 1.5x bound has ample margin against drift while
    // catching subtler regressions than a looser 2x bound would.)
    assert!(
        large_per_call * 2 < small_per_call * 3,
        "icrc3_get_blocks cycles per call grew super-linearly with log_length: \
         50 blocks: {small_per_call}, 200 blocks: {large_per_call}. \
         The hash-chain cache is not effective."
    );

    // Sanity floor: at minimum the call costs more than a no-op message.
    assert!(small_per_call > 100_000, "suspiciously low: {small_per_call}");
}

#[test]
fn post_upgrade_traps_on_hash_cache_length_mismatch() {
    let harness = deploy_pool_with_liquidity_and_swaps(10);
    assert_test_endpoints_built(&harness);

    // Corrupt the cache by appending a bogus 32-byte hash. block_hashes::len()
    // is now blocks::len() + 1, which both `backfill_hash_chain` and the
    // post_upgrade integrity check should detect.
    let bogus = vec![0xFFu8; 32];
    let _ = harness.pic
        .update_call(harness.three_pool, harness.admin, "test_corrupt_hash_cache_tip",
                     candid::encode_one(bogus).unwrap())
        .expect("test_corrupt_hash_cache_tip failed");

    // Trigger an upgrade. post_upgrade traps; PocketIC reports the trap and
    // rolls back stable memory atomically per IC spec, leaving the canister
    // on the prior wasm.
    let wasm = include_bytes!(
        "../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm"
    ).to_vec();
    let result = harness.pic.upgrade_canister(harness.three_pool, wasm, vec![], None);

    assert!(
        result.is_err(),
        "expected upgrade to trap on hash cache corruption, but it succeeded"
    );

    // Verify the trap message identifies the failure mode. We accept either
    // the backfill_hash_chain trap ("block_hashes ... exceeds blocks ...")
    // or the post_upgrade integrity-check trap ("hash cache length mismatch").
    let err_str = format!("{:?}", result.err().unwrap());
    let mentions_hash = err_str.contains("hash") || err_str.contains("block_hashes");
    assert!(
        mentions_hash,
        "expected trap message to mention the hash cache, got: {err_str}"
    );
}

#[test]
fn post_upgrade_backfills_empty_hash_cache() {
    let harness = deploy_pool_with_liquidity_and_swaps(30);
    assert_test_endpoints_built(&harness);

    let log_length = harness.icrc3_log_length();
    assert!(log_length >= 30, "expected at least 30 blocks, got {log_length}");

    // Snapshot the current view of all blocks via the live optimized endpoint.
    // After post_upgrade backfills the cleared cache, the response for the
    // same query must be byte-identical.
    let pre_upgrade_blocks = harness.icrc3_get_blocks(0, log_length);
    assert_eq!(pre_upgrade_blocks.len(), log_length as usize);

    // Clear the hash cache, simulating pre-Task-3 mainnet state.
    let _ = harness.pic
        .update_call(harness.three_pool, harness.admin, "test_clear_hash_cache",
                     candid::encode_one(()).unwrap())
        .expect("test_clear_hash_cache failed");

    // Upgrade with the same wasm. post_upgrade runs backfill_hash_chain,
    // which should detect hashes_len < blocks_len and refill all entries.
    // Sender is None (PocketIC provisional mode allows the upgrade without
    // a controller check, matching how we installed the canister initially).
    let wasm = include_bytes!(
        "../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm"
    ).to_vec();
    harness.pic
        .upgrade_canister(harness.three_pool, wasm, vec![], None)
        .expect("upgrade failed (post_upgrade likely trapped on integrity check)");

    // After backfill, identical query response.
    let post_upgrade_blocks = harness.icrc3_get_blocks(0, log_length);
    assert_eq!(post_upgrade_blocks.len(), pre_upgrade_blocks.len());
    for (a, b) in pre_upgrade_blocks.iter().zip(post_upgrade_blocks.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.block, b.block, "block content changed across upgrade with backfill");
    }
}

#[test]
fn transfer_and_approve_blocks_preserve_request_memo_and_timestamp() {
    let harness = deploy_pool_with_liquidity_and_swaps(0);
    assert_test_endpoints_built(&harness);
    let now = harness
        .pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let memo = vec![0x52, 0x75, 0x6d, 0x69];

    let transfer = TransferArg {
        from_subaccount: None,
        to: Account { owner: harness.user, subaccount: None },
        fee: None,
        created_at_time: Some(now),
        memo: Some(Memo(serde_bytes::ByteBuf::from(memo.clone()))),
        amount: Nat::from(1u64),
    };
    let transfer_result: Result<Nat, TransferError> = decode_one(&reply(
        harness
            .pic
            .update_call(
                harness.three_pool,
                harness.user,
                "icrc1_transfer",
                encode_one(transfer).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let transfer_id: u64 = transfer_result
        .expect("timestamped memo transfer succeeds")
        .0
        .try_into()
        .unwrap();
    let transfer_block = harness.get_raw_block(transfer_id);
    match transfer_block.tx {
        Icrc3Transaction::Transfer { memo: actual_memo, created_at_time, .. } => {
            assert_eq!(actual_memo, Some(memo.clone()));
            assert_eq!(created_at_time, Some(now));
        }
        other => panic!("expected transfer block, got {other:?}"),
    }
    assert_block_transaction_metadata(&harness, transfer_id, &memo, now);

    let spender = Principal::self_authenticating(&[0x53, 0x50]);
    let approve = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: Nat::from(123u64),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: Some(Memo(serde_bytes::ByteBuf::from(memo.clone()))),
        created_at_time: Some(now),
    };
    let approve_result: Result<Nat, ApproveError> = decode_one(&reply(
        harness
            .pic
            .update_call(
                harness.three_pool,
                harness.user,
                "icrc2_approve",
                encode_one(approve).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let approve_id: u64 = approve_result
        .expect("timestamped memo approval succeeds")
        .0
        .try_into()
        .unwrap();
    let approve_block = harness.get_raw_block(approve_id);
    match approve_block.tx {
        Icrc3Transaction::Approve { memo: actual_memo, created_at_time, .. } => {
            assert_eq!(actual_memo, Some(memo.clone()));
            assert_eq!(created_at_time, Some(now));
        }
        other => panic!("expected approval block, got {other:?}"),
    }
    assert_block_transaction_metadata(&harness, approve_id, &memo, now);

    let transfer_from = TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: harness.user, subaccount: None },
        to: Account { owner: harness.user, subaccount: None },
        amount: Nat::from(1u64),
        fee: None,
        memo: Some(Memo(serde_bytes::ByteBuf::from(memo.clone()))),
        created_at_time: Some(now),
    };
    let transfer_from_result: Result<Nat, TransferFromError> = decode_one(&reply(
        harness
            .pic
            .update_call(
                harness.three_pool,
                spender,
                "icrc2_transfer_from",
                encode_one(transfer_from).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let transfer_from_id: u64 = transfer_from_result
        .expect("timestamped memo transfer_from succeeds")
        .0
        .try_into()
        .unwrap();
    match harness.get_raw_block(transfer_from_id).tx {
        Icrc3Transaction::Transfer { memo: actual_memo, created_at_time, .. } => {
            assert_eq!(actual_memo, Some(memo.clone()));
            assert_eq!(created_at_time, Some(now));
        }
        other => panic!("expected transfer_from block, got {other:?}"),
    }
    assert_block_transaction_metadata(&harness, transfer_from_id, &memo, now);
}

/// Build exact-main with `test_endpoints` and set `RUMI_3POOL_OLD_WASM` to that
/// Wasm path when running this ignored compatibility test. This makes the
/// upgrade direction explicit and checks populated LP, ICRC-3, and pending
/// claim data across the current-main-to-new-source upgrade.
#[test]
#[ignore = "requires an old 3pool Wasm at RUMI_3POOL_OLD_WASM"]
fn populated_old_wasm_upgrade_preserves_icrc3_blocks_and_hash_chain() {
    let old_wasm_path = std::env::var("RUMI_3POOL_OLD_WASM")
        .expect("set RUMI_3POOL_OLD_WASM to the pre-metadata 3pool Wasm");
    let old_wasm = std::fs::read(old_wasm_path).expect("read old 3pool Wasm");
    let harness = common::deploy_pool_with_liquidity_fee_and_swaps_with_wasm(3, 0, old_wasm);

    let claim_id: u64 = decode_one(&reply(
        harness
            .pic
            .update_call(
                harness.three_pool,
                harness.user,
                "test_insert_pending_claim",
                candid::encode_args((0u8, 42u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();

    let memo = vec![4, 3, 2, 1];
    let now = harness
        .pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let args = TransferArg {
        from_subaccount: None,
        to: Account { owner: harness.user, subaccount: None },
        fee: None,
        created_at_time: Some(now),
        memo: Some(Memo(serde_bytes::ByteBuf::from(memo))),
        amount: Nat::from(1u64),
    };
    let transfer: Result<Nat, TransferError> = decode_one(&reply(
        harness
            .pic
            .update_call(harness.three_pool, harness.user, "icrc1_transfer", encode_one(args).unwrap())
            .unwrap(),
    ))
    .unwrap();
    transfer.expect("old version transfer succeeds");

    let length = harness.icrc3_log_length();
    let before = harness.icrc3_get_blocks(0, length);
    let new_wasm = common::three_pool_wasm();
    harness
        .pic
        .upgrade_canister(harness.three_pool, new_wasm, vec![], None)
        .expect("upgrade from old block schema succeeds");
    let claims: Vec<ThreePoolPendingClaim> = decode_one(
        &reply(
            harness
                .pic
                .query_call(
                    harness.three_pool,
                    Principal::anonymous(),
                    "get_pending_claims",
                    candid::encode_args((0u64, 100u64)).unwrap(),
                )
                .unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].id, claim_id);
    assert_eq!(claims[0].claimant, harness.user);
    assert_eq!(claims[0].amount, 42);
    let after = harness.icrc3_get_blocks(0, length);

    assert_eq!(after.len(), before.len());
    for (old, upgraded) in before.iter().zip(after.iter()) {
        assert_eq!(old.id, upgraded.id);
        assert_eq!(old.block, upgraded.block, "old ICRC-3 block changed across upgrade");
    }
}
