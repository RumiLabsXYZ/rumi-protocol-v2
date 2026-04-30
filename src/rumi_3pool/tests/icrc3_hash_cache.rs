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

use candid::Nat;
use rumi_3pool::icrc3::{BlockWithId, Icrc3Value};

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

    let log_length = harness.icrc3_log_length();
    assert!(
        log_length >= 5,
        "expected at least 5 blocks, got {log_length}"
    );

    // Adjust window list based on actual log_length.
    let test_windows: Vec<(u64, u64)> = vec![
        (0, 1),
        (0, 10),
        (0, log_length),
        (log_length.saturating_sub(1), 1),
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
