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

use candid::{encode_one, Nat};
use rumi_3pool::icrc3::{BlockWithId, GetBlocksArgs, Icrc3Value};

use common::{deploy_pool_with_liquidity_and_swaps, ThreePoolHarness};

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

    // Probe the test-only endpoint. If this fails, the WASM was almost
    // certainly built without `--features test_endpoints`. Fail fast with
    // a clear message rather than letting the assertion loop produce
    // confusing "method not found" panics inside reference_get_blocks.
    let probe_result = harness.pic.query_call(
        harness.three_pool,
        candid::Principal::anonymous(),
        "test_get_raw_block",
        candid::encode_one(0u64).unwrap(),
    );
    assert!(
        probe_result.is_ok(),
        "test_get_raw_block endpoint missing from WASM. \
         Rebuild with: cargo build -p rumi_3pool --release \
         --target wasm32-unknown-unknown --features test_endpoints"
    );

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
fn post_upgrade_backfills_empty_hash_cache() {
    let harness = deploy_pool_with_liquidity_and_swaps(30);

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
