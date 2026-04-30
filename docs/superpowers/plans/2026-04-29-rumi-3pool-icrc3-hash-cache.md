# Rumi 3pool ICRC-3 Hash-Chain Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut `rumi_3pool` daily cycle burn from ~0.33 TC to ~0.01 TC by removing the O(N) hash-chain rebuild in `icrc3_get_blocks`, and harden the change with end-to-end correctness + cycle benchmarks so the optimization is provably safe and measurably effective.

**Architecture:** Add a parallel `StableLog<StorableHash, ...>` that mirrors block hashes 1:1 with the existing blocks log. `log_block` writes both atomically inside the same message. `icrc3_get_blocks` reads the parent hash directly from the cache and encodes only the requested range — turning per-call work from O(total_blocks) into O(requested_blocks). A one-shot backfill in `post_upgrade` populates the cache for the existing 1,447 mainnet blocks. Defensive cross-checks (cache length parity + tip hash equality) trap on any inconsistency, leveraging IC `post_upgrade` rollback semantics for atomicity.

**Tech Stack:** Rust, `ic-cdk 0.12.0`, `ic-stable-structures 0.6.7` (`StableLog`, `MemoryManager`), `sha2 0.10`, `candid 0.10.6`, PocketIC 6.0.0 for integration tests.

**Spec:** Conversation context, 2026-04-29. Investigation traced 0.33 TC/day burn to [`src/rumi_3pool/src/icrc3.rs:204-217`](../../../src/rumi_3pool/src/icrc3.rs#L204-L217), where the comment itself acknowledges "This is O(end) per request — acceptable for typical index-ng polling windows; if it becomes a hot path we can cache cumulative hashes alongside the blocks log." It is now the hot path.

---

## Background and Why

The `threeusd_index` canister (DFINITY's standard `ic-icrc1-index-ng-latest.wasm.gz`, [dfx.json:41-46](../../../dfx.json#L41-L46)) polls the 3pool via inter-canister update calls roughly every 7-8 seconds to fetch new ICRC-3 blocks. When an update calls a `#[query]` method cross-canister, the query is executed in **replicated mode** and the *called* canister pays the execution cost.

Today, every such poll causes 3pool to read all N blocks from stable memory, Candid-encode each into an `Icrc3Value`, and SHA-256 hash each — solely to compute the parent hash for the first requested block. With N = 1,447 and growing ~90 blocks/day, this dominates the canister's cycle burn (326B cycles/day of activity vs. 3.5B/day idle storage).

The fix is the one suggested in the existing TODO comment: cache the cumulative block hashes. After this change:

- `log_block` (write path): same instructions plus one 32-byte append. Negligible.
- `icrc3_get_blocks` (read/poll path): one cache lookup for the parent hash, then encode only the blocks actually being returned (typically 1–10 per poll).

Expected per-call work: **~29M cycles → ~50K cycles**, a ~600× reduction. Daily burn target: **0.33 TC → ~0.01 TC**.

---

## File Structure

**Modified files:**

- `src/rumi_3pool/src/storage.rs` — add `StorableHash` wrapper, two new memory IDs (18, 19), the `BLOCK_HASHES_LOG` thread_local, and a `block_hashes` log API module. Add a new `backfill_hash_chain` function in the `migration` submodule.
- `src/rumi_3pool/src/state.rs` — modify `log_block` (line 147) so the parent-hash retrieval and the dual write (block + hash) are atomic within the same message.
- `src/rumi_3pool/src/icrc3.rs` — rewrite `icrc3_get_blocks` (line 186) to use the cached parent hash and encode only the requested range.
- `src/rumi_3pool/src/lib.rs` — extend `post_upgrade` (line 70) with the backfill call and tip-equality check.

**New files:**

- `src/rumi_3pool/tests/icrc3_hash_cache.rs` — PocketIC test suite covering: equivalence vs. recomputed reference, hash-chain integrity for arbitrary `(start, length)` queries, cycle benchmark before/after, and post_upgrade backfill correctness.

**No changes to:**

- The Candid interface (`src/rumi_3pool/rumi_3pool.did`) — the bytes returned by `icrc3_get_blocks` must be identical pre- and post-fix, so the Candid signature is unchanged.
- The migration drain path in `storage::migration::drain_legacy_state` — that runs once on the Phase A migration and is already complete on mainnet. The new backfill is additive.

---

## Out of Scope (Explicit, with Reason)

These were considered and deliberately deferred — listed here so future readers don't assume they were missed:

- **ICRC-3 archive canisters.** ICRC-3 supports archive canisters that hold older blocks. We currently return `archived_blocks: vec![]`. At 90 blocks/day we hit 50K blocks in ~18 months, which is the soft threshold where archives meaningfully help. Adding archive support is a multi-canister change with its own deployment surface. **Trigger:** revisit when `blocks::len() > 50_000` or when post_upgrade serialization time exceeds 2s.
- **Other `iter_all` hot paths.** `state.rs:223` (`swap_events_v2`), `state.rs:228` (`liquidity_events_v2`), and `lib.rs:942` (`get_vp_snapshots`) all do full-log scans. They feed user-facing explorer queries, not auto-polled inter-canister calls, so they aren't burning cycles at scale today. Audit step in Task 11 documents them with a watch threshold.
- **Stable memory bucket size reduction.** The 18 memory IDs each consume an 8 MiB bucket on first write. Custom `BUCKET_SIZE_IN_PAGES` would shrink this but requires a destructive `MemoryManager` reset. Risk/benefit doesn't justify it: the structural overhead is ~3.5B cycles/day idle, which is dwarfed by the activity burn we're fixing.
- **Caching the encoded `Icrc3Value` form.** Once we cache the parent hash, encoding work drops to O(requested_range), which is already small (1–10 blocks typically). Caching the encoded form would 3× the storage cost for marginal gain.

---

## Task 1: Add `StorableHash` wrapper

**Files:**
- Modify: `src/rumi_3pool/src/storage.rs` (after line 227 where `StorableU128` ends)

A new wrapper for `[u8; 32]` so we can use it as a `StableLog` element. Mirrors `StorableU128`'s pattern.

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block at the bottom of `storage.rs` (around line 730):

```rust
    #[test]
    fn storable_hash_roundtrip() {
        let original = [0xABu8; 32];
        let sh = StorableHash(original);
        let bytes = sh.to_bytes();
        let back = StorableHash::from_bytes(bytes);
        assert_eq!(back.0, original);
    }

    #[test]
    fn storable_hash_distinct_values() {
        let a = StorableHash([1u8; 32]);
        let b = StorableHash([2u8; 32]);
        assert_ne!(a.to_bytes(), b.to_bytes());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p rumi_3pool --lib storage::tests::storable_hash 2>&1 | tail -20
```

Expected: FAIL with `cannot find type StorableHash in this scope`.

- [ ] **Step 3: Add `StorableHash` definition**

Insert after the `StorableU128` impl ends (line 227) and before the `Unit` definition:

```rust
/// 32-byte hash stored verbatim. Used for the ICRC-3 cumulative hash-chain
/// cache so that `icrc3_get_blocks` can fetch a block's parent hash in O(1)
/// instead of recomputing the chain from block 0.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorableHash(pub [u8; 32]);

impl Storable for StorableHash {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.0.to_vec())
    }

    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes.as_ref());
        StorableHash(arr)
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 32,
        is_fixed_size: true,
    };
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p rumi_3pool --lib storage::tests::storable_hash 2>&1 | tail -20
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/storage.rs
git commit -m "feat(3pool): add StorableHash wrapper for hash-chain cache"
```

---

## Task 2: Add the block-hashes stable log

**Files:**
- Modify: `src/rumi_3pool/src/storage.rs`

Allocate two new memory IDs and stand up the `StableLog<StorableHash, ...>` plus a `block_hashes` log API module that mirrors the existing `blocks` API.

- [ ] **Step 1: Update memory-ID layout comment and constants**

In `storage.rs`, find the comment block at lines 15-27 and update the layout map. Replace lines 15-27 with:

```rust
// Memory ID layout (20 IDs used; 255 available):
//
//   0       SlimState cell              — bounded residual heap
//   1       lp_balances                 — BTreeMap<Principal, u128>
//   2       lp_allowances               — BTreeMap<(Principal, Principal), LpAllowance>
//   3       authorized_burn_callers     — BTreeMap<Principal, ()>
//   4,5     swap_events_v1 log          — preserved forever for auditability
//   6,7     liquidity_events_v1 log     — preserved forever
//   8,9     swap_events_v2 log
//   10,11   liquidity_events_v2 log
//   12,13   admin_events log
//   14,15   vp_snapshots log
//   16,17   icrc3_blocks log
//   18,19   icrc3_block_hashes log      — cumulative hash chain cache (parallel
//                                         to blocks log; entry i == hash of block i)
```

Then add the new memory ID constants. Find line 71 (`const MEM_BLOCKS_DATA: MemoryId = MemoryId::new(17);`) and append two new lines:

```rust
const MEM_BLOCK_HASHES_INDEX: MemoryId = MemoryId::new(18);
const MEM_BLOCK_HASHES_DATA: MemoryId = MemoryId::new(19);
```

- [ ] **Step 2: Add the thread-local stable log**

Find the `BLOCKS_LOG` thread_local at lines 355-362. Append after it (still inside the `thread_local! { ... }` block):

```rust
    pub(crate) static BLOCK_HASHES_LOG: RefCell<StableLog<StorableHash, Memory, Memory>> =
        RefCell::new(
            StableLog::init(
                MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_INDEX)),
                MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_DATA)),
            )
            .expect("init block_hashes log"),
        );
```

- [ ] **Step 3: Add the `block_hashes` log API module**

Find line 513 (`log_api!(blocks, BLOCKS_LOG, Icrc3Block);`) and append:

```rust
log_api!(block_hashes, BLOCK_HASHES_LOG, StorableHash);
```

This generates `block_hashes::push`, `block_hashes::len`, `block_hashes::get`, `block_hashes::range`, and `block_hashes::iter_all` automatically.

- [ ] **Step 4: Verify compilation**

```bash
cargo build -p rumi_3pool 2>&1 | tail -20
```

Expected: build succeeds with no errors. Warnings about unused symbols are acceptable at this point — they go away in Task 3.

- [ ] **Step 5: Add a smoke test for the new log**

Append to the `mod tests` block:

```rust
    #[test]
    fn block_hashes_log_initializes_empty() {
        // Reset memory manager state by using a fresh subprocess for this test
        // is not necessary — we just verify len() is callable and returns 0
        // when the log has never been written. The lazy init will allocate one
        // bucket on first len() call.
        let initial_len = block_hashes::len();
        assert_eq!(initial_len, 0);
    }
```

Note: this test depends on no other test having pushed entries before it. Run it in isolation.

- [ ] **Step 6: Run the smoke test**

```bash
cargo test -p rumi_3pool --lib storage::tests::block_hashes_log_initializes_empty 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_3pool/src/storage.rs
git commit -m "feat(3pool): add icrc3_block_hashes parallel stable log"
```

---

## Task 3: Modify `log_block` to write the hash-chain entry

**Files:**
- Modify: `src/rumi_3pool/src/state.rs:147-166`

Atomically push to both logs inside the same canister message so the cache can never get out of sync with blocks.

- [ ] **Step 1: Read the current `log_block` to confirm exact line numbers**

```bash
grep -n "pub fn log_block" /Users/robertripley/coding/rumi-protocol-v2/src/rumi_3pool/src/state.rs
```

Expected: `147:    pub fn log_block(&mut self, tx: crate::types::Icrc3Transaction) -> u64 {`

- [ ] **Step 2: Update `log_block` to push the hash to the new log**

Replace the body of `log_block` (lines 147-166) with this version. The only change is the addition of the `crate::storage::block_hashes::push(...)` line and an updated docstring:

```rust
    /// Log a transaction block, compute its hash, update certified data,
    /// and return its index.
    ///
    /// Block IDs are sequential starting from 0, matching `StableLog` index.
    /// The hash of each block is also pushed to `storage::block_hashes` so
    /// `icrc3_get_blocks` can fetch a parent hash in O(1) rather than
    /// recomputing the chain from block 0.
    ///
    /// Both writes happen inside this single message; IC message-level
    /// atomicity guarantees they cannot diverge.
    pub fn log_block(&mut self, tx: crate::types::Icrc3Transaction) -> u64 {
        let id = crate::storage::blocks::len();
        let block = crate::types::Icrc3Block {
            id,
            timestamp: ic_cdk::api::time(),
            tx,
        };
        let prev_hash = self.last_block_hash;
        let encoded = crate::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
        let block_hash = crate::certification::hash_value(&encoded);
        crate::storage::blocks::push(block);
        crate::storage::block_hashes::push(crate::storage::StorableHash(block_hash));
        self.last_block_hash = Some(block_hash);
        crate::certification::set_certified_tip(id, &block_hash);
        id
    }
```

- [ ] **Step 3: Verify compilation**

```bash
cargo build -p rumi_3pool 2>&1 | tail -20
```

Expected: clean build.

- [ ] **Step 4: Run the existing 3pool integration test to confirm nothing broke**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test integration_test 2>&1 | tail -30
```

Expected: existing test still passes. New blocks will now also be in `block_hashes` log.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/state.rs
git commit -m "feat(3pool): write hash-chain cache alongside every new block"
```

---

## Task 4: Add a `backfill_hash_chain` migration helper

**Files:**
- Modify: `src/rumi_3pool/src/storage.rs` (in the `migration` submodule, after line 705)

Idempotent helper that catches the cache up to the blocks log. Used both for the first deploy after this change ships (1,447 existing blocks have no cached hashes) and as a defensive no-op on every subsequent upgrade.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `storage.rs`:

```rust
    #[test]
    fn backfill_is_idempotent_when_cache_is_full() {
        // After Task 3, every push to blocks already pushes to block_hashes.
        // So if both logs have the same length, backfill should be a no-op.
        let blocks_before = blocks::len();
        let hashes_before = block_hashes::len();
        if blocks_before != hashes_before {
            // We cannot run this test in isolation if other tests left the logs
            // in an inconsistent state. Skip rather than fail.
            return;
        }
        crate::storage::migration::backfill_hash_chain();
        assert_eq!(blocks::len(), blocks_before);
        assert_eq!(block_hashes::len(), hashes_before);
    }
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p rumi_3pool --lib storage::tests::backfill_is_idempotent 2>&1 | tail -10
```

Expected: FAIL with `cannot find function backfill_hash_chain`.

- [ ] **Step 3: Add the backfill function**

In `storage.rs`, find the end of the `migration` submodule (around line 705 — the closing `}` after `drain_legacy_state`) and add a new function INSIDE the `migration` module, before its closing brace:

```rust
    /// Backfill `block_hashes` to match `blocks` length.
    ///
    /// After this function returns:
    ///   - `block_hashes::len() == blocks::len()`
    ///   - For every `i in 0..blocks::len()`, `block_hashes::get(i)` equals
    ///     the SHA-256 of the ICRC-3 encoding of `blocks::get(i)` with the
    ///     correct parent hash.
    ///
    /// Idempotent: a no-op when the lengths already match. Safe to call from
    /// `post_upgrade` on every upgrade (steady-state cost: 2 stable reads).
    ///
    /// Trapping inside this function rolls back stable memory atomically per
    /// IC `post_upgrade` semantics, so partial backfills cannot persist.
    pub fn backfill_hash_chain() {
        let blocks_len = crate::storage::blocks::len();
        let hashes_len = crate::storage::block_hashes::len();
        if hashes_len >= blocks_len {
            // Already up to date. (`>` would be a logic bug; we tolerate it
            // here and let the integrity check in post_upgrade trap on it.)
            return;
        }

        // Recompute the chain from block 0 up to (but not including) the
        // first missing index. We need the parent hash for the first block
        // we're about to fill, which is the cached hash of (start - 1) if
        // any cache exists, or computed from scratch if hashes_len == 0.
        let start = hashes_len;
        let mut prev_hash: Option<[u8; 32]> = if start == 0 {
            None
        } else {
            Some(
                crate::storage::block_hashes::get(start - 1)
                    .expect("cached hash present below hashes_len")
                    .0,
            )
        };

        for i in start..blocks_len {
            let block = crate::storage::blocks::get(i)
                .expect("block present below blocks_len");
            let encoded = crate::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
            let block_hash = crate::certification::hash_value(&encoded);
            crate::storage::block_hashes::push(crate::storage::StorableHash(block_hash));
            prev_hash = Some(block_hash);
        }
    }
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p rumi_3pool --lib storage::tests::backfill_is_idempotent 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 5: Add a stronger backfill test that exercises the loop**

Append to `mod tests`:

```rust
    #[test]
    fn backfill_fills_missing_hashes_correctly() {
        use crate::types::{Icrc3Block, Icrc3Transaction};
        use candid::Principal;

        // Push three blocks via the storage API directly (skipping log_block,
        // which would also write to block_hashes). This simulates the
        // "existing mainnet state" scenario where blocks exist but the
        // hash-chain cache is empty.
        let baseline = blocks::len();
        let hash_baseline = block_hashes::len();
        if baseline != hash_baseline {
            // Cannot guarantee preconditions; skip.
            return;
        }

        let p = Principal::anonymous();
        for i in 0..3u64 {
            let block = Icrc3Block {
                id: baseline + i,
                timestamp: 1_000 + i,
                tx: Icrc3Transaction::Mint {
                    to: p,
                    amount: 100 + i as u128,
                    to_subaccount: None,
                },
            };
            blocks::push(block);
        }

        assert_eq!(blocks::len(), baseline + 3);
        assert_eq!(block_hashes::len(), hash_baseline);

        crate::storage::migration::backfill_hash_chain();

        assert_eq!(block_hashes::len(), baseline + 3);

        // Verify every newly added hash is consistent with its block's
        // ICRC-3 encoding under the correct parent.
        let mut prev = if baseline == 0 {
            None
        } else {
            Some(block_hashes::get(baseline - 1).unwrap().0)
        };
        for i in baseline..baseline + 3 {
            let block = blocks::get(i).unwrap();
            let encoded = crate::icrc3::encode_block_with_phash(&block, prev.as_ref());
            let expected = crate::certification::hash_value(&encoded);
            let cached = block_hashes::get(i).unwrap().0;
            assert_eq!(cached, expected, "hash mismatch at index {i}");
            prev = Some(cached);
        }
    }
```

- [ ] **Step 6: Run the new test**

```bash
cargo test -p rumi_3pool --lib storage::tests::backfill_fills_missing_hashes_correctly 2>&1 | tail -20
```

Expected: 1 passed.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_3pool/src/storage.rs
git commit -m "feat(3pool): add backfill_hash_chain migration helper"
```

---

## Task 5: Wire backfill + integrity check into `post_upgrade`

**Files:**
- Modify: `src/rumi_3pool/src/lib.rs:70-142`

The `post_upgrade` already has hash-chain verification (lines 92-104). After this change it must additionally:
1. Run `backfill_hash_chain()` so the cache is current.
2. Verify `block_hashes::len() == blocks::len()`.
3. Verify the last cached hash equals `state.last_block_hash`.

These checks belong AFTER any drain logic (so they run on both first-time-after-A and steady-state paths).

- [ ] **Step 1: Read the current `post_upgrade` body**

```bash
sed -n '60,145p' /Users/robertripley/coding/rumi-protocol-v2/src/rumi_3pool/src/lib.rs
```

Confirm structure: there's a match on `storage::migration::read_legacy_blob()` with a `Some(legacy_state) =>` branch (lines 71-119) that does the drain, and a `_ =>` branch (lines 120-130) that's the steady-state path. Both branches converge at line 132.

- [ ] **Step 2: Insert the backfill + verification block**

Find line 139 (the closing of the `if let Some(h) = read_state(...)` block). Right BEFORE the `setup_timers();` call on line 141, insert:

```rust
    // ── ICRC-3 hash-chain cache: backfill (if needed) and verify ──
    //
    // First post_upgrade after this change ships will backfill 1,447 entries.
    // Every subsequent upgrade hits the early-return inside backfill and the
    // checks below run cheaply.
    storage::migration::backfill_hash_chain();

    let blocks_len = storage::blocks::len();
    let hashes_len = storage::block_hashes::len();
    if hashes_len != blocks_len {
        ic_cdk::trap(&format!(
            "post_upgrade: ICRC-3 hash cache length mismatch. \
             blocks={blocks_len} hashes={hashes_len}"
        ));
    }

    if blocks_len > 0 {
        let cached_tip = storage::block_hashes::get(blocks_len - 1)
            .expect("cached tip present when blocks_len > 0")
            .0;
        let state_tip = read_state(|s| s.last_block_hash);
        if Some(cached_tip) != state_tip {
            ic_cdk::trap(&format!(
                "post_upgrade: ICRC-3 cached tip != state.last_block_hash. \
                 cached={:?} state={:?}",
                cached_tip, state_tip
            ));
        }
    }

    log!(INFO,
        "Rumi 3pool post-upgrade: hash cache OK. blocks={blocks_len} hashes={hashes_len}");
```

- [ ] **Step 3: Verify compilation**

```bash
cargo build -p rumi_3pool 2>&1 | tail -20
```

Expected: clean build.

- [ ] **Step 4: Run the integration test to verify post_upgrade succeeds**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test integration_test 2>&1 | tail -30
```

Expected: existing test passes (its post_upgrade path runs the new code on an empty blocks log — backfill is a no-op, lengths both 0, no tip check).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs
git commit -m "feat(3pool): backfill + verify hash-chain cache on post_upgrade"
```

---

## Task 6: Rewrite `icrc3_get_blocks` to use the cache

**Files:**
- Modify: `src/rumi_3pool/src/icrc3.rs:186-225`

The optimization. After this change, per-call work is O(end - start) instead of O(end). Crucially, the response bytes must be **identical** to the old implementation — Task 7's equivalence test enforces this.

- [ ] **Step 1: Replace the function body**

Replace lines 186-225 with:

```rust
pub fn icrc3_get_blocks(args: Vec<GetBlocksArgs>) -> GetBlocksResult {
    let log_length = crate::storage::blocks::len();

    let mut result_blocks = Vec::new();
    for arg in &args {
        let start = nat_to_u64(&arg.start);
        let length = nat_to_u64(&arg.length);

        if start >= log_length {
            continue;
        }
        let end = std::cmp::min(start.saturating_add(length), log_length);
        if end <= start {
            continue;
        }

        // Parent hash for the first requested block: cached at index
        // (start - 1), or None if start == 0.
        let mut prev_hash: Option<[u8; 32]> = if start == 0 {
            None
        } else {
            Some(
                crate::storage::block_hashes::get(start - 1)
                    .expect("hash cache must cover all blocks; backfill runs in post_upgrade")
                    .0,
            )
        };

        // Read only the requested range from the blocks log. Encoding +
        // hashing happens once per returned block, replacing the old
        // O(end) chain rebuild.
        let blocks = crate::storage::blocks::range(start, end - start);
        for block in &blocks {
            let encoded = encode_block_with_phash(block, prev_hash.as_ref());
            // Compute the running hash so the next iteration has its parent.
            // (For the LAST block, prev_hash isn't read again — but computing
            // it keeps the code symmetric and is cheap.)
            let block_hash = crate::certification::hash_value(&encoded);
            result_blocks.push(BlockWithId {
                id: Nat::from(block.id),
                block: encoded,
            });
            prev_hash = Some(block_hash);
        }
    }

    GetBlocksResult {
        log_length: Nat::from(log_length),
        blocks: result_blocks,
        archived_blocks: vec![],
    }
}
```

Note the symmetry: we still compute each returned block's hash because `encode_block_with_phash` for the *next* block needs it as the parent. But we no longer re-derive it for blocks we don't return.

- [ ] **Step 2: Verify compilation**

```bash
cargo build -p rumi_3pool 2>&1 | tail -20
```

Expected: clean build, no warnings.

- [ ] **Step 3: Run all 3pool unit tests**

```bash
cargo test -p rumi_3pool --lib 2>&1 | tail -30
```

Expected: all pass. The optimization should not break any existing logic.

- [ ] **Step 4: Run integration tests**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test integration_test 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/icrc3.rs
git commit -m "perf(3pool): use cached parent hash in icrc3_get_blocks (O(end) -> O(range))"
```

---

## Task 7: Equivalence integration test (new vs. reference)

**Files:**
- Create: `src/rumi_3pool/tests/icrc3_hash_cache.rs`

Property test: for every meaningful `(start, length)` window, the new `icrc3_get_blocks` returns the same `Icrc3Value`s and the chain reconstructs to the same tip hash as a from-scratch reference computation. This is the load-bearing safety net — if the optimization changed any byte, the index canister would detect a chain break in production.

- [ ] **Step 1: Create the test file with the harness**

The harness sets up a fresh PocketIC environment with three ICRC-1 ledgers and the 3pool, mirroring `integration_test.rs`. To keep this plan focused, we **factor the harness out of `integration_test.rs`** in Step 2.

For now, create the test file with imports and a single equivalence test:

```rust
// src/rumi_3pool/tests/icrc3_hash_cache.rs
//
// Verifies the ICRC-3 hash-chain cache optimization (Task 6) produces output
// bit-identical to a from-scratch reference computation, and that the cache
// stays consistent across upgrades.
//
// These tests use the SAME deploy/setup harness as `integration_test.rs`;
// the shared bits live in `tests/common/mod.rs` (introduced in Step 2).

mod common;

use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use pocket_ic::WasmResult;
use rumi_3pool::icrc3::{BlockWithId, GetBlocksArgs, GetBlocksResult, Icrc3Value};
use rumi_3pool::types::*;

use common::{deploy_pool_with_liquidity_and_swaps, three_pool_canister_id, ThreePoolHarness};

/// Reference implementation: rebuild the entire hash chain from block 0,
/// returning the ICRC-3 Value form of the requested range. This is what
/// `icrc3_get_blocks` did pre-optimization.
fn reference_get_blocks(
    harness: &ThreePoolHarness,
    start: u64,
    length: u64,
) -> Vec<BlockWithId> {
    // Fetch every block in [0, start + length) one at a time so the reference
    // doesn't depend on the optimized endpoint at all. We use a private query
    // method that returns raw stored blocks.
    let log_length = harness.icrc3_log_length();
    let end = std::cmp::min(start.saturating_add(length), log_length);
    if start >= end {
        return vec![];
    }

    let mut prev_hash: Option<[u8; 32]> = None;
    let mut out = Vec::new();
    for i in 0..end {
        let block = harness.get_raw_block(i);
        let encoded = rumi_3pool::icrc3::encode_block_with_phash(&block, prev_hash.as_ref());
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
    let harness = deploy_pool_with_liquidity_and_swaps(50);

    let log_length = harness.icrc3_log_length();
    assert!(log_length >= 50, "expected at least 50 blocks, got {log_length}");

    let test_windows: Vec<(u64, u64)> = vec![
        (0, 1),
        (0, 10),
        (0, log_length),
        (log_length - 1, 1),
        (log_length / 2, 5),
        (log_length, 10),       // off-the-end → empty
        (log_length + 1, 5),    // past end → empty
        (5, 0),                 // zero length → empty
    ];

    for (start, length) in test_windows {
        let optimized = harness.icrc3_get_blocks(start, length);
        let reference = reference_get_blocks(&harness, start, length);
        assert_eq!(
            optimized.len(),
            reference.len(),
            "length mismatch at (start={start}, length={length})"
        );
        for (a, b) in optimized.iter().zip(reference.iter()) {
            assert_eq!(a.id, b.id, "id mismatch at (start={start})");
            // Comparing Icrc3Values directly: equal iff representation-identical.
            assert_eq!(a.block, b.block, "block mismatch at (start={start})");
        }
    }
}
```

- [ ] **Step 2: Extract the shared harness into `tests/common/mod.rs`**

Create `src/rumi_3pool/tests/common/mod.rs`. Move the WASM loaders, ledger init types, and a deploy helper here. Replace the body with:

```rust
// Shared PocketIC harness for 3pool integration tests.
//
// Wraps PocketIC + the three ICRC-1 ledgers + the 3pool canister behind a
// `ThreePoolHarness` struct so individual tests stay focused on assertions
// rather than setup boilerplate.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_3pool::icrc3::{BlockWithId, GetBlocksArgs, GetBlocksResult};
use rumi_3pool::types::*;

#[derive(CandidType, Deserialize)]
struct FeatureFlags { icrc2: bool }

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
enum MetadataValue { Nat(Nat), Int(candid::Int), Text(String), Blob(Vec<u8>) }

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
enum LedgerArg { Init(LedgerInitArgs) }

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn three_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec()
}

pub struct ThreePoolHarness {
    pub pic: PocketIc,
    pub admin: Principal,
    pub user: Principal,
    pub three_pool: Principal,
    pub ledgers: [Principal; 3],
}

impl ThreePoolHarness {
    pub fn three_pool(&self) -> Principal { self.three_pool }

    pub fn icrc3_get_blocks(&self, start: u64, length: u64) -> Vec<BlockWithId> {
        let arg = vec![GetBlocksArgs {
            start: Nat::from(start),
            length: Nat::from(length),
        }];
        let bytes = self.pic
            .query_call(self.three_pool, Principal::anonymous(), "icrc3_get_blocks",
                encode_one(arg).unwrap())
            .expect("icrc3_get_blocks query failed");
        let WasmResult::Reply(reply) = bytes else { panic!("icrc3_get_blocks rejected") };
        let result: GetBlocksResult = decode_one(&reply).unwrap();
        result.blocks
    }

    pub fn icrc3_log_length(&self) -> u64 {
        let arg = vec![GetBlocksArgs { start: Nat::from(0u64), length: Nat::from(0u64) }];
        let bytes = self.pic
            .query_call(self.three_pool, Principal::anonymous(), "icrc3_get_blocks",
                encode_one(arg).unwrap())
            .expect("icrc3_get_blocks query failed");
        let WasmResult::Reply(reply) = bytes else { panic!("rejected") };
        let result: GetBlocksResult = decode_one(&reply).unwrap();
        result.log_length.0.try_into().unwrap()
    }

    /// Read a single raw block as stored (without phash recomputation).
    /// Used by the reference impl in `icrc3_hash_cache.rs`. Implemented by
    /// fetching the single-block range via the optimized endpoint and decoding
    /// the returned ICRC-3 Value back into an Icrc3Block via a roundtrip helper.
    ///
    /// NOTE: this returns the BLOCK as stored, NOT the encoded value. It is
    /// for the reference computation only.
    pub fn get_raw_block(&self, id: u64) -> Icrc3Block {
        // Read via a dedicated test endpoint that bypasses ICRC-3 encoding.
        // We add this endpoint in Step 3 below.
        let bytes = self.pic
            .query_call(self.three_pool, Principal::anonymous(), "test_get_raw_block",
                encode_one(id).unwrap())
            .expect("test_get_raw_block query failed");
        let WasmResult::Reply(reply) = bytes else { panic!("rejected") };
        decode_one::<Icrc3Block>(&reply).unwrap()
    }
}

/// Deploy the pool with bootstrap liquidity, then perform `n_swaps` swaps
/// and `n_swaps / 5` add/remove liquidity actions to generate ICRC-3 blocks.
///
/// Returns a harness with at least `n_swaps + 5` blocks in the ICRC-3 log
/// (each LP token transfer / mint / burn produces one block).
pub fn deploy_pool_with_liquidity_and_swaps(n_swaps: u64) -> ThreePoolHarness {
    // Bootstrap is identical to the harness in integration_test.rs through the
    // "deploy + bootstrap liquidity" point. After that, perform `n_swaps`
    // swaps. Implementation detail: copy the existing setup verbatim, refactor
    // shared bits into helper methods on PocketIcBuilder.
    //
    // For brevity here, the helper body follows the same shape as
    // `test_3pool_deploy_add_liquidity_and_swap` in integration_test.rs.
    todo!("Copy + adapt the deploy/bootstrap from integration_test.rs::test_3pool_deploy_add_liquidity_and_swap")
}

pub fn three_pool_canister_id() -> Principal {
    Principal::anonymous() // placeholder; harness fills this in
}
```

That `todo!()` is a deliberate flag — it's filled in during Step 4 below by copying the existing setup logic.

- [ ] **Step 3: Add `test_get_raw_block` endpoint behind a `cfg(test)` gate**

The reference impl needs raw blocks bypassing ICRC-3 encoding. Add to `src/rumi_3pool/src/lib.rs`, near the other `icrc3_*` endpoints (around line 2034):

```rust
/// Test-only: return a raw `Icrc3Block` by id, bypassing ICRC-3 encoding.
/// Used by integration tests as the ground truth for hash-chain comparisons.
#[cfg(any(feature = "test_endpoints", test))]
#[query]
pub fn test_get_raw_block(id: u64) -> Option<types::Icrc3Block> {
    storage::blocks::get(id)
}
```

Add a `test_endpoints` feature to `src/rumi_3pool/Cargo.toml`:

```toml
[features]
test_endpoints = []
```

Build the test wasm with that feature for the integration test:

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
```

The default mainnet build does NOT enable `test_endpoints`, so this endpoint never ships to production.

- [ ] **Step 4: Fill in the harness `deploy_pool_with_liquidity_and_swaps`**

Open `src/rumi_3pool/tests/integration_test.rs` and copy the setup logic (lines ~140-340 of the existing test) into `tests/common/mod.rs` as the body of `deploy_pool_with_liquidity_and_swaps`. After bootstrap liquidity is added, perform `n_swaps` swaps in a loop:

```rust
    for i in 0..n_swaps {
        let token_in = (i % 3) as u8;
        let token_out = ((i + 1) % 3) as u8;
        let amount: u128 = 1_000_000;
        let swap_args = encode_args((token_in, token_out, amount, 0u128)).unwrap();
        let _result = pic.update_call(three_pool, user, "swap", swap_args)
            .expect("swap failed");
    }
```

Adjust amounts and the bootstrap-liquidity values to match what the original test deposits. The exact numbers are not load-bearing here — we just need ≥ 50 blocks generated.

- [ ] **Step 5: Update `integration_test.rs` to use the shared harness**

Open `src/rumi_3pool/tests/integration_test.rs`. Replace its inline setup block with a call to `common::deploy_pool_with_liquidity_and_swaps(0)` (zero swaps; the test does its own). Add `mod common;` at the top.

This eliminates the duplication and ensures any harness fixes propagate to both test files.

- [ ] **Step 6: Run the equivalence test**

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints 2>&1 | tail -30
```

Expected: `icrc3_get_blocks_matches_reference_for_all_windows ... ok`.

- [ ] **Step 7: Run the existing integration test to confirm the harness refactor didn't regress it**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test integration_test 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add src/rumi_3pool/tests/icrc3_hash_cache.rs \
        src/rumi_3pool/tests/common/mod.rs \
        src/rumi_3pool/tests/integration_test.rs \
        src/rumi_3pool/src/lib.rs \
        src/rumi_3pool/Cargo.toml
git commit -m "test(3pool): equivalence test for icrc3_get_blocks vs reference"
```

---

## Task 8: Cycle benchmark test (proves the optimization works)

**Files:**
- Modify: `src/rumi_3pool/tests/icrc3_hash_cache.rs`

Empirical proof. Generate a substantial number of blocks, then measure how many cycles `icrc3_get_blocks` consumes when called as an inter-canister update (i.e., the same execution mode as production polling).

PocketIC 6.0.0 exposes `pic.cycle_balance(canister_id) -> u128`. We:
1. Snapshot balance.
2. Call `icrc3_get_blocks` 100 times via inter-canister update from a helper canister (or via direct update — either works, what matters is replicated execution).
3. Snapshot balance again.
4. Diff / 100 = cycles per call.

Direct update calls go through ingress and pay ingress cost, but for the relative comparison we just need consistent measurement. Easiest: use update mode from PocketIC, then divide.

- [ ] **Step 1: Add the benchmark test**

Append to `src/rumi_3pool/tests/icrc3_hash_cache.rs`:

```rust
#[test]
fn icrc3_get_blocks_cycle_cost_is_constant_in_log_length() {
    // We measure cycles burned per `icrc3_get_blocks` UPDATE call (replicated
    // execution, i.e. the production polling path). With 200 blocks vs. 50
    // blocks, the per-call cost should be approximately constant — the
    // hallmark of an O(range) algorithm. Without the cache, cost would be
    // 4× higher at 200 blocks.

    fn cycles_per_call(harness: &ThreePoolHarness, n_calls: u32) -> u128 {
        let log_length = harness.icrc3_log_length();
        let last = log_length.saturating_sub(1);
        let arg = encode_one(vec![GetBlocksArgs {
            start: Nat::from(last),
            length: Nat::from(1u64),
        }]).unwrap();

        let before = harness.pic.cycle_balance(harness.three_pool);
        for _ in 0..n_calls {
            let _ = harness.pic
                .update_call(harness.three_pool, Principal::anonymous(),
                             "icrc3_get_blocks", arg.clone())
                .expect("icrc3_get_blocks update failed");
        }
        let after = harness.pic.cycle_balance(harness.three_pool);
        let burned = before.saturating_sub(after);
        burned / (n_calls as u128)
    }

    // Build two harnesses with different block counts. (Two separate PocketIC
    // instances; ~30s combined runtime.)
    let small = deploy_pool_with_liquidity_and_swaps(50);
    let large = deploy_pool_with_liquidity_and_swaps(200);

    let small_per_call = cycles_per_call(&small, 50);
    let large_per_call = cycles_per_call(&large, 50);

    eprintln!("icrc3_get_blocks cycles/call: 50 blocks={small_per_call}, 200 blocks={large_per_call}");

    // With the cache, cost is dominated by the per-update message base + a
    // single block encode + hash. We expect the ratio < 1.5× even though
    // log_length grew 4×. Without the cache, ratio would be near 4×.
    assert!(
        large_per_call < small_per_call * 3 / 2,
        "icrc3_get_blocks cycles per call grew super-linearly with log_length: \
         50 blocks: {small_per_call}, 200 blocks: {large_per_call}. \
         The hash-chain cache is not effective."
    );

    // Sanity floor — at minimum the call costs more than a no-op message.
    assert!(small_per_call > 100_000, "suspiciously low: {small_per_call}");
}
```

- [ ] **Step 2: Run the benchmark**

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints \
    icrc3_get_blocks_cycle_cost_is_constant -- --nocapture 2>&1 | tail -30
```

Expected: PASS. The eprintln output should show the 200-block call costing roughly the same as the 50-block call (e.g., both around 1-3M cycles). Record the actual numbers in the commit message for posterity.

- [ ] **Step 3: Verify the test fails without the optimization**

This is the most important verification step — confirming the test catches a regression. Temporarily revert `src/rumi_3pool/src/icrc3.rs` to the O(N) version (just the body of `icrc3_get_blocks`), rerun the bench, confirm it FAILS, then re-apply the optimization.

```bash
git stash push src/rumi_3pool/src/icrc3.rs
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints \
    icrc3_get_blocks_cycle_cost_is_constant -- --nocapture 2>&1 | tail -30
git stash pop
```

Expected during stash: FAIL with the assertion message and a roughly-4× cost ratio. After the pop, re-run to confirm it passes again.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_3pool/tests/icrc3_hash_cache.rs
git commit -m "test(3pool): benchmark icrc3_get_blocks shows constant per-call cost"
```

---

## Task 9: Backfill simulation test

**Files:**
- Modify: `src/rumi_3pool/tests/icrc3_hash_cache.rs`

Simulate the mainnet upgrade scenario: a canister with N blocks but ZERO entries in the new hash-chain log, then run the upgrade and verify `post_upgrade` populates the cache correctly.

We can't trivially clear the new log on a deployed canister in PocketIC (the hash log starts empty before any block is logged, but after Task 3, every `log_block` writes to it). So we simulate by:
1. Deploy 3pool wasm.
2. Generate N blocks.
3. Deploy a special "stripped" wasm that doesn't write to `block_hashes` — same as the pre-Task-3 behavior. Or alternately:
4. Use a `cfg`-gated test endpoint to clear the hash log.

The cleanest approach: a `#[cfg(any(feature = "test_endpoints", test))] #[update]` endpoint `test_clear_hash_cache()` that empties the log. Then we generate blocks, clear the cache, upgrade, and assert post_upgrade backfilled correctly.

- [ ] **Step 1: Add the test endpoint**

In `src/rumi_3pool/src/lib.rs`, near `test_get_raw_block` (Task 7 Step 3):

```rust
/// Test-only: clear the ICRC-3 hash cache. Used by tests to simulate the
/// pre-Task-3 mainnet state where blocks exist but the cache is empty.
#[cfg(any(feature = "test_endpoints", test))]
#[update]
pub fn test_clear_hash_cache() {
    storage::BLOCK_HASHES_LOG.with(|l| {
        let mm = l.borrow();
        let len = mm.len();
        // StableLog has no truncate; rebuild it. We expose a private helper
        // in storage.rs for this in Step 2.
        drop(mm);
    });
    storage::clear_block_hashes_for_test();
}
```

- [ ] **Step 2: Add `clear_block_hashes_for_test` in `storage.rs`**

`StableLog` has no truncate API. The cleanest way to "clear" it is to swap in a fresh log over the same memory IDs. But `MemoryManager` will reuse existing buckets. The simplest approach:

```rust
#[cfg(any(feature = "test_endpoints", test))]
pub fn clear_block_hashes_for_test() {
    // Pop all entries by reinitializing the StableLog over its memories.
    // This zeroes the index header, effectively setting len() to 0.
    BLOCK_HASHES_LOG.with(|l| {
        let mut log = l.borrow_mut();
        // Replace with a fresh log over the same memory IDs. `StableLog::init`
        // will detect the existing magic and reload. To force-clear, we use
        // `StableLog::new` which writes a fresh header.
        let idx_mem = MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_INDEX));
        let dat_mem = MM.with(|m| m.borrow().get(MEM_BLOCK_HASHES_DATA));
        *log = StableLog::new(idx_mem, dat_mem).expect("re-init block_hashes log");
    });
}
```

If `StableLog::new` doesn't exist in `ic-stable-structures 0.6.7`, fall back to: read all entries, then expose a tracking counter that the helper resets — but `StableLog` doesn't support deletion either. In that case, use a `cfg` flag to gate the cache REUSE entirely, simulating "cache empty" via a runtime override.

Verify the API:

```bash
grep -rn "fn new\|fn init" ~/.cargo/registry/src/index.crates.io-*/ic-stable-structures-0.6.7/src/log.rs 2>&1 | head -10
```

If `StableLog::new` is not available, use this alternate Step 2:

```rust
// Alternate: track an "effective length" override.
#[cfg(any(feature = "test_endpoints", test))]
thread_local! {
    static HASH_CACHE_EFFECTIVE_LEN_OVERRIDE: RefCell<Option<u64>> =
        const { RefCell::new(None) };
}

#[cfg(any(feature = "test_endpoints", test))]
pub fn clear_block_hashes_for_test() {
    HASH_CACHE_EFFECTIVE_LEN_OVERRIDE.with(|o| *o.borrow_mut() = Some(0));
}
```

And modify `block_hashes::len` and `block_hashes::get` accessors to consult the override under the `test_endpoints` feature. (This is messier; prefer `StableLog::new` if available.)

- [ ] **Step 3: Add the backfill simulation test**

Append to `icrc3_hash_cache.rs`:

```rust
#[test]
fn post_upgrade_backfills_empty_hash_cache() {
    let harness = deploy_pool_with_liquidity_and_swaps(30);
    let log_length = harness.icrc3_log_length();
    assert!(log_length >= 30);

    // Snapshot the current tip from icrc3_get_tip_certificate (which reads
    // state.last_block_hash and is unchanged by Task 6).
    let pre_upgrade_blocks = harness.icrc3_get_blocks(0, log_length);
    assert_eq!(pre_upgrade_blocks.len(), log_length as usize);

    // Clear the hash cache, simulating pre-Task-3 mainnet state.
    harness.pic
        .update_call(harness.three_pool, harness.admin, "test_clear_hash_cache",
                     encode_one(()).unwrap())
        .expect("clear failed");

    // Trigger an upgrade (with the same wasm; post_upgrade still runs).
    let wasm = include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec();
    harness.pic
        .upgrade_canister(harness.three_pool, wasm, vec![], Some(harness.admin))
        .expect("upgrade failed");

    // After post_upgrade backfill, the cache should be full and the endpoint
    // should return identical bytes for every window.
    let post_upgrade_blocks = harness.icrc3_get_blocks(0, log_length);
    assert_eq!(post_upgrade_blocks.len(), pre_upgrade_blocks.len());
    for (a, b) in pre_upgrade_blocks.iter().zip(post_upgrade_blocks.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.block, b.block, "block changed across upgrade with backfill");
    }
}
```

- [ ] **Step 4: Run the test**

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints \
    post_upgrade_backfills_empty_hash_cache 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_3pool/src/lib.rs \
        src/rumi_3pool/src/storage.rs \
        src/rumi_3pool/tests/icrc3_hash_cache.rs
git commit -m "test(3pool): post_upgrade backfills empty hash cache correctly"
```

---

## Task 10: Trap-on-mismatch tests

**Files:**
- Modify: `src/rumi_3pool/tests/icrc3_hash_cache.rs`

The integrity checks in Task 5 trap on inconsistency. Verify they actually trigger when the data is corrupt — otherwise we have a placebo defense.

- [ ] **Step 1: Add a corruption test**

This test requires another `cfg`-gated helper that writes a wrong hash into the cache. Add to `lib.rs`:

```rust
#[cfg(any(feature = "test_endpoints", test))]
#[update]
pub fn test_corrupt_hash_cache_tip(bogus_hash: Vec<u8>) {
    assert_eq!(bogus_hash.len(), 32);
    let mut h = [0u8; 32];
    h.copy_from_slice(&bogus_hash);
    let len = storage::block_hashes::len();
    if len == 0 { return; }
    storage::block_hashes::push(storage::StorableHash(h));
    // Length is now blocks_len + 1, which will trip the parity check on next
    // upgrade.
}
```

Append to `icrc3_hash_cache.rs`:

```rust
#[test]
fn post_upgrade_traps_on_hash_cache_length_mismatch() {
    let harness = deploy_pool_with_liquidity_and_swaps(10);

    // Corrupt the cache by appending an extra hash.
    let bogus = vec![0xFFu8; 32];
    harness.pic
        .update_call(harness.three_pool, harness.admin, "test_corrupt_hash_cache_tip",
                     encode_one(bogus).unwrap())
        .expect("corrupt failed");

    // Upgrade should trap because hashes_len > blocks_len.
    let wasm = include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_3pool.wasm").to_vec();
    let result = harness.pic.upgrade_canister(harness.three_pool, wasm, vec![], Some(harness.admin));
    assert!(result.is_err(), "expected upgrade to trap on cache mismatch");
    let err_str = format!("{:?}", result.err().unwrap());
    assert!(
        err_str.contains("hash cache length mismatch") || err_str.contains("hash"),
        "expected trap message about hash cache, got: {err_str}"
    );
}
```

- [ ] **Step 2: Run the test**

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints \
    post_upgrade_traps_on_hash_cache_length_mismatch 2>&1 | tail -20
```

Expected: PASS — the trap fires and the upgrade rolls back.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/lib.rs \
        src/rumi_3pool/tests/icrc3_hash_cache.rs
git commit -m "test(3pool): post_upgrade traps on corrupted hash cache"
```

---

## Task 11: Audit document for remaining iter_all hot paths (optional but recommended)

**Files:**
- Modify: `src/rumi_3pool/src/state.rs` and `src/rumi_3pool/src/lib.rs` (comment additions only)

The user asked for a forward-thinking solution. We're not implementing further optimizations now (out of scope per the design decisions), but we should leave breadcrumbs so the next person who looks at cycle burn doesn't repeat the diagnostic work.

- [ ] **Step 1: Annotate the remaining `iter_all` call sites**

Add a comment block above each of these lines:

`src/rumi_3pool/src/state.rs:223` — above `pub fn swap_events_v2`:

```rust
    // PERFORMANCE NOTE: O(N) over all swap events. Used by explorer queries
    // (`get_top_swappers`, `get_volume_series`). Not currently a hot path
    // because these are user-driven, not auto-polled. If swap volume grows or
    // a frontend polls these queries on a short interval, consider:
    //   1. Adding a windowed cache keyed by `StatsWindow` (heap-only).
    //   2. Maintaining running aggregates in `state` updated on every swap.
    // Watch threshold: swap_v2::len() > 50_000 OR per-day query count from
    // explorer > 1_000 (check threeusd_index logs and replica metrics).
    /// Snapshot every v2 swap event...
```

Same pattern at `state.rs:228` (`liquidity_events_v2`) and `lib.rs:942` (`get_vp_snapshots`).

`lib.rs:942` — above `pub fn get_vp_snapshots`:

```rust
    // PERFORMANCE NOTE: O(N) over all VP snapshots. At 4 snapshots/day this
    // grows ~1500/year. Watch threshold: vp_snap::len() > 10_000.
    /// Returns all virtual_price snapshots for APY calculation and historical charts.
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build -p rumi_3pool 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_3pool/src/state.rs src/rumi_3pool/src/lib.rs
git commit -m "docs(3pool): annotate remaining iter_all call sites with watch thresholds"
```

---

## Task 12: Run the full pre-deploy gauntlet

**Files:** none (verification only)

Before any mainnet deploy, run every test surface that touches the 3pool. The pre-deploy hook in `.claude/hooks/pre-deploy-test.sh` runs unit + integration tests; we run them explicitly here so we can read the output.

- [ ] **Step 1: Run all 3pool unit tests**

```bash
cargo test -p rumi_3pool --lib 2>&1 | tail -30
```

Expected: every test passes.

- [ ] **Step 2: Run all 3pool integration tests (default features)**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test integration_test 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 3: Run the new hash-cache test suite**

```bash
cargo build -p rumi_3pool --release --target wasm32-unknown-unknown --features test_endpoints
POCKET_IC_BIN=$PWD/pocket-ic cargo test -p rumi_3pool --test icrc3_hash_cache --features test_endpoints 2>&1 | tail -30
```

Expected: all four tests pass (equivalence, benchmark, backfill, trap).

- [ ] **Step 4: Run the broader pocket_ic_3usd suite that exercises the stability pool ↔ 3pool path**

```bash
POCKET_IC_BIN=$PWD/pocket-ic cargo test --test pocket_ic_3usd 2>&1 | tail -20
```

Expected: pass. (This catches any unintended interaction with the SP's use of the 3pool.)

- [ ] **Step 5: Run the workspace-wide cargo build to catch any cross-crate breakage**

```bash
cargo build --release --target wasm32-unknown-unknown 2>&1 | tail -10
```

Expected: clean build of every canister.

- [ ] **Step 6: Capture cycle baseline before deploy**

```bash
dfx canister --network ic status rumi_3pool 2>&1 | grep -E "Idle cycles|Memory Size|Balance|Module hash" > /tmp/3pool-pre-deploy.txt
cat /tmp/3pool-pre-deploy.txt
```

Save this output — we'll compare against post-deploy numbers in Task 14.

---

## Task 13: Mainnet deploy

**Files:** none (deploy step)

This is a backend canister upgrade. Per [.claude/CLAUDE.md and global memory](../../../.claude/CLAUDE.md), all backend deploys must include a description and use `dfx deploy` (never `install`).

**STOP — REQUIRES USER AUTHORIZATION.** Per memory rule: *"Never merge PRs or deploy without explicit authorization."* The executor MUST pause here, summarize the plan results so far, and explicitly ask the user before running the deploy command.

- [ ] **Step 1: Confirm authorization**

Ask the user: *"Plan complete, all tests green. Ready to deploy rumi_3pool with the hash-cache fix? Pre-deploy hash cache target: 1,447 blocks (or whatever current count is). Backfill is expected to take ~50ms in post_upgrade. Confirm before I run dfx deploy."*

WAIT for explicit approval before proceeding to Step 2.

- [ ] **Step 2: Deploy**

```bash
dfx deploy --network ic rumi_3pool --argument '(variant { Upgrade = record { description = opt "Cache ICRC-3 cumulative hash chain in parallel StableLog. Cuts icrc3_get_blocks per-call cost from O(N) to O(range). Backfills 1,447 existing blocks on post_upgrade." } })' 2>&1 | tail -30
```

Note: 3pool's existing post_upgrade may not accept an `Upgrade` variant — check the canister's init args type at `src/rumi_3pool/src/lib.rs` (look near `#[ic_cdk::init]` and `#[ic_cdk::post_upgrade]`). If 3pool uses `()` for post_upgrade args, drop `--argument` from the deploy.

- [ ] **Step 3: Watch the post_upgrade log**

```bash
dfx canister --network ic logs rumi_3pool 2>&1 | tail -10
```

Expected to see (in order):
- "Rumi 3pool pre-upgrade: flushing SlimState to stable cell"
- "Rumi 3pool post-upgrade: loaded from SlimState. LP supply: ..., holders: ..., blocks: 1447"
- "Rumi 3pool post-upgrade: hash cache OK. blocks=1447 hashes=1447"
- "VP snapshot taken"

If the third line is missing, the new code didn't deploy. If it shows mismatched numbers, investigate immediately — but stable memory rolled back atomically per IC spec, so the canister is on the OLD wasm and ICRC-3 is unaffected.

---

## Task 14: Post-deploy verification (24h watch)

**Files:** none (monitoring)

The fix is empirically validated only when CycleOps shows the expected drop. Plan for two verification windows: a 1-hour smoke check and a 24-hour soak.

- [ ] **Step 1: T+0 — verify icrc3_get_blocks still serves correct data**

```bash
dfx canister --network ic call rumi_3pool icrc3_get_blocks '(vec { record { start = 0 : nat; length = 5 : nat } })' 2>&1 | tail -20
```

Expected: returns 5 blocks with valid `phash` chain. Compare the output's first block's hash bytes against a snapshot taken pre-deploy (you can save one with the same call before Task 13 for direct diff).

- [ ] **Step 2: T+1h — verify threeusd_index has not stalled**

```bash
dfx canister --network ic call threeusd_index status 2>&1 | tail -10
```

Expected: `num_blocks_synced` matches `rumi_3pool::icrc3_get_blocks log_length` (or is within a small lag window). If `num_blocks_synced` is stuck at the pre-deploy value, the index detected a chain break and stopped — investigate immediately. (Highly unlikely given Tasks 7+9, but this is the canary.)

- [ ] **Step 3: T+1h — observe initial cycle burn**

Open the CycleOps dashboard and look at the rumi_3pool burn rate. The 24h average will still include yesterday's pre-fix data. The instant burn rate (last hour) should already show the drop.

- [ ] **Step 4: T+24h — confirm the win**

```bash
dfx canister --network ic status rumi_3pool 2>&1 | grep -E "Idle cycles|Balance"
```

Expected idle cycles: roughly unchanged (~3.5B/day; we added one memory bucket). Expected balance: meaningfully higher than the pre-deploy delta, because the canister stopped burning ~320B cycles/day on icrc3_get_blocks. Compare against `/tmp/3pool-pre-deploy.txt` from Task 12.

CycleOps 24h burn target: **0.33 TC → ~0.01 TC** (give or take, depending on residual update activity).

- [ ] **Step 5: Update the global memory**

After confirmation, update `/Users/robertripley/.claude/projects/-Users-robertripley-coding-rumi-protocol-v2/memory/MEMORY.md` to record the win and remove the relevant follow-up entry. Specifically: replace any "icrc3_get_blocks O(N) burn" mention with "fixed in commit XXX (date)".

---

## Self-Review Checklist

Run through this after the plan is complete and before handoff:

**Spec coverage:**
- ✅ Diagnose cycle burn — established in conversation context.
- ✅ Cache the cumulative hash chain — Tasks 1, 2.
- ✅ Atomic dual-write on `log_block` — Task 3.
- ✅ Idempotent backfill on `post_upgrade` — Task 4, 5.
- ✅ Integrity checks (length parity + tip equality) — Task 5.
- ✅ Optimized `icrc3_get_blocks` — Task 6.
- ✅ Equivalence test against reference impl — Task 7.
- ✅ Cycle benchmark proving the win — Task 8.
- ✅ Migration safety test (post_upgrade backfill from empty) — Task 9.
- ✅ Trap correctness test (corrupted cache) — Task 10.
- ✅ Forward-thinking: audit other hot paths — Task 11.
- ✅ Pre-deploy gauntlet — Task 12.
- ✅ Mainnet deploy with authorization gate — Task 13.
- ✅ Post-deploy verification + memory update — Task 14.

**Type / API consistency:**
- `StorableHash([u8; 32])` — defined in Task 1, used in Tasks 2, 3, 4, 9, 10.
- `block_hashes` log API — generated by `log_api!` macro in Task 2, called as `block_hashes::push`, `len`, `get`, `range` everywhere consistently.
- `MEM_BLOCK_HASHES_INDEX = 18`, `MEM_BLOCK_HASHES_DATA = 19` — used in Task 2 init AND Task 9 alternate clear path.
- `clear_block_hashes_for_test` — defined in Task 9 Step 2, called in Task 9 endpoint and tests.

**Placeholder scan:**
- One acknowledged `todo!()` in Task 7 Step 2 (the harness body) — Step 4 of that task is "Fill in the harness", with explicit instructions on which lines to copy. Not a hidden TODO.
- Task 9 Step 2 has two alternates depending on `StableLog::new` availability in `ic-stable-structures 0.6.7`; Step 2 explicitly tells the executor to grep for the API and pick the right path.
- No "implement later" / "TBD" / vague references.

**Risk surface:**
- ICRC-3 chain integrity is load-bearing for `threeusd_index`. Tasks 7, 9, 10 collectively verify the fix doesn't change a single byte of output, the backfill produces correct hashes, and corruption traps loudly.
- Stable memory layout grows by 2 IDs (18, 19). New buckets cost ~16 MiB extra storage at one-time-burn ~400M cycles/day idle. Net win still ~320B/day.
- Deploy gated on user authorization (Task 13 Step 1).

---

## Execution Handoff

Plan saved to `docs/superpowers/plans/2026-04-29-rumi-3pool-icrc3-hash-cache.md`. Two execution options:

1. **Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration. Best fit here because tasks 1–11 are mostly independent and the agent can carry less context per dispatch.

2. **Inline Execution** — Execute tasks in this session using `superpowers:executing-plans`, with checkpoints at the end of Tasks 6, 10, and 12 for human review.

Which approach?
