# rumi_analytics Phase 4: Event Tailing & BalanceTracker

**Date**: 2026-04-09
**Status**: Design
**Prereqs**: Phase 3 (complete, deployed), Phase 1 (merged)

## Context

Phase 3 deployed three daily current-state collectors (TVL, vaults, stability). Phase 4 adds the event tailing backbone: cursor-based polling of source canisters every 60 seconds, mirroring events into local EVT_* StableLogs, ICRC-3 block replay for holder balance tracking, daily holder snapshots, historical backfill, and a collector health query.

All source canister event interfaces already exist (confirmed in Phase 0 audit). No source canister changes needed.

## Architecture Overview

Every 60 seconds, the pull cycle checks each source canister for new events since the last cursor position, fetches them in bounded batches, and writes them to local EVT_* mirror logs. ICRC-3 block streams from icusd_ledger and 3pool feed a BalanceTracker that maintains running account balances.

Six active cursor sources:

| Cursor (StableCell) | MemoryId | Source Call | Destination |
|---------------------|----------|------------|-------------|
| Backend events | 1 | `backend.get_events()` | EVT_LIQUIDATIONS + EVT_VAULTS (routed by variant) |
| 3pool swaps | 2 | `3pool.get_swap_events()` | EVT_SWAPS |
| 3pool liquidity | 3 | `3pool.get_liquidity_events()` | EVT_LIQUIDITY |
| 3pool blocks | 4 | `3pool.icrc3_get_blocks()` | BalanceTracker (BAL_3USD) |
| AMM swaps | 5 | `amm.get_amm_swap_events()` | EVT_SWAPS |
| icUSD blocks | 7 | `icusd_ledger.icrc3_get_blocks()` | BalanceTracker (BAL_ICUSD) |

MEM_CURSOR_STABILITY_EVENTS (MemoryId 6) stays reserved but unused. Stability pool user events (deposit, withdraw, claim) are already captured by the Phase 3 daily snapshot, and the liquidation perspective comes from backend events.

## Cursor Infrastructure

Each cursor is a `StableCell<u64>` initialized to 0. The cursor represents the next event index to fetch (i.e., the cursor value = number of events already processed).

**Cursor advancement protocol** (custom event sources):
1. Read cursor value from StableCell
2. Call source's count endpoint (e.g., `get_event_count()`)
3. If count <= cursor, nothing to do
4. Fetch `min(count - cursor, BATCH_SIZE)` events starting at cursor
5. Process events (normalize, route, write to EVT_* log)
6. Advance cursor to `cursor + events_processed`

**Cursor advancement protocol** (ICRC-3 block sources):
ICRC-3 has no separate count endpoint. Instead:
1. Read cursor value from StableCell
2. Call `icrc3_get_blocks(vec { record { start = cursor; length = BATCH_SIZE } })`
3. The response includes `log_length` (total blocks). If no blocks returned, nothing to do.
4. Process returned blocks (parse transfers, update BalanceTracker)
5. Advance cursor to `cursor + blocks_processed`
6. If `archived_blocks` entries are present, follow callbacks to fetch archived blocks (counts toward batch limit)

The cursor only advances after a successful write. On error at any step, the cursor stays put and the next 60s tick retries from the same position. This makes the system idempotent and crash-safe.

**BATCH_SIZE**: 500 events per cursor per tick. This keeps each inter-canister call bounded and leaves room for all 6 cursors to run within a single 60s window.

## Event Tailing Sources

### Backend Events (cursor 1)

**Source**: `backend.get_events(record { start; length })` returns `vec Event`. The backend's `Event` type has 62 variants. We filter and route:

**Routed to EVT_LIQUIDATIONS**:
- `liquidate_vault` (full liquidation)
- `partial_liquidate_vault` (partial liquidation)
- `redistribute_vault` (redistribution)

**Routed to EVT_VAULTS**:
- `open_vault`
- `borrow_from_vault`
- `repay_to_vault`
- `collateral_withdrawn`
- `partial_collateral_withdrawn`
- `withdraw_and_close_vault`
- `VaultWithdrawnAndClosed`
- `dust_forgiven`
- `redemption_on_vaults`

**Ignored**: All admin/config events (`set_borrowing_fee`, `price_update`, `set_min_collateral_ratio`, etc.). These are configuration changes, not user activity. The cursor still advances past them.

### 3pool Swaps (cursor 2)

**Source**: `3pool.get_swap_events(start, length)` returns `vec SwapEvent`.

All events go to EVT_SWAPS. We use the V1 endpoint (V2 adds imbalance/rebalancing fields that we don't need for Phase 4 analytics, and V1 events are a strict subset).

### 3pool Liquidity (cursor 3)

**Source**: `3pool.get_liquidity_events(start, length)` returns `vec LiquidityEvent`.

All events go to EVT_LIQUIDITY. Uses V1 endpoint for the same reason as swaps.

### AMM Swaps (cursor 5)

**Source**: `amm.get_amm_swap_events(start, length)` returns `vec AmmSwapEvent`.

All events go to EVT_SWAPS (same log as 3pool swaps, but with a `source` discriminator).

### ICRC-3 Block Tailing (cursors 4 and 7)

**Sources**:
- `icusd_ledger.icrc3_get_blocks(vec { record { start; length } })` (cursor 7)
- `3pool.icrc3_get_blocks(vec { record { start; length } })` (cursor 4)

These feed the BalanceTracker, not an EVT_* log. The tailing function:
1. Fetches blocks from the cursor position
2. Parses each `ICRC3Value` block to extract transfer operations
3. For each transfer/mint/burn, applies balance deltas to the BalanceTracker
4. Advances the cursor

**ICRC-3 block parsing**: ICRC-1 blocks encode transactions as `Map` values. The `tx` field contains the operation. Standard block kinds:
- `"1xfer"` (transfer): extract `from`, `to`, `amt`. Debit sender, credit receiver.
- `"1mint"` (mint): extract `to`, `amt`. Credit receiver.
- `"1burn"` (burn): extract `from`, `amt`. Debit sender.
- `"2approve"` (approve): no balance change, skip.

The parser extracts `Account` (principal + optional subaccount) and amount from the ICRC3Value tree. Malformed blocks are logged and skipped (cursor still advances).

**Archive handling**: `icrc3_get_blocks` may return `archived_blocks` entries pointing to archive canisters. During backfill, we need to follow these. During steady-state tailing, recent blocks are typically in the main canister. The tailing function checks `archived_blocks` and follows callbacks if present.

## EVT_* Storage Types

We store normalized event types (not raw source events) to keep storage lean and provide a consistent interface for Phase 5 rollups.

### EVT_LIQUIDATIONS (MemoryIds 44/45)

```rust
struct AnalyticsLiquidationEvent {
    timestamp_ns: u64,
    source_event_id: u64,       // backend event index
    vault_id: u64,
    collateral_type: Principal,
    collateral_amount: u64,     // collateral seized
    debt_amount: u64,           // debt covered
    liquidation_kind: LiquidationKind, // Full, Partial, Redistribution
}

enum LiquidationKind { Full, Partial, Redistribution }
```

### EVT_VAULTS (MemoryIds 50/51)

```rust
struct AnalyticsVaultEvent {
    timestamp_ns: u64,
    source_event_id: u64,
    vault_id: u64,
    owner: Principal,
    event_kind: VaultEventKind,
    collateral_type: Principal,
    amount: u64,                // collateral or debt amount, context-dependent
}

enum VaultEventKind {
    Opened,
    Borrowed,
    Repaid,
    CollateralWithdrawn,
    PartialCollateralWithdrawn,
    WithdrawAndClose,
    Closed,
    DustForgiven,
    Redeemed,
}
```

### EVT_SWAPS (MemoryIds 46/47)

```rust
struct AnalyticsSwapEvent {
    timestamp_ns: u64,
    source: SwapSource,         // ThreePool or AMM
    source_event_id: u64,
    caller: Principal,
    token_in: Principal,        // for 3pool: derived from token index
    token_out: Principal,
    amount_in: u64,             // truncated from nat for 3pool
    amount_out: u64,
    fee: u64,
}

enum SwapSource { ThreePool, Amm }
```

For 3pool swaps, `token_in` and `token_out` are `nat8` indices (0/1/2). The source wrapper resolves these to Principals using the known 3pool token ordering (icUSD=0, ckUSDT=1, ckUSDC=2). The analytics canister hardcodes this mapping since the 3pool's token list is fixed at deploy time.

### EVT_LIQUIDITY (MemoryIds 48/49)

```rust
struct AnalyticsLiquidityEvent {
    timestamp_ns: u64,
    source_event_id: u64,
    caller: Principal,
    action: LiquidityAction,    // Add, Remove, RemoveOneCoin, Donate
    amounts: Vec<u64>,          // per-token amounts (truncated from nat)
    lp_amount: u64,
    coin_index: Option<u8>,     // for RemoveOneCoin: which token was removed
    fee: Option<u64>,           // fee charged (truncated from nat)
}

enum LiquidityAction { Add, Remove, RemoveOneCoin, Donate }
```

All EVT_* types implement `Storable` with Candid encoding and `Bound::Unbounded` (same pattern as Phase 3's `DailyVaultSnapshotRow`).

## BalanceTracker

Two `StableBTreeMap<Vec<u8>, u64>` collections track current balances:

| Map | MemoryId | Key | Value |
|-----|----------|-----|-------|
| BAL_ICUSD | 56 | Account (Principal + subaccount, Candid-encoded) | balance (u64, e8s) |
| BAL_3USD | 57 | Account (Principal + subaccount, Candid-encoded) | balance (u64, e8s for 3USD) |

And two `StableBTreeMap<Vec<u8>, u64>` for first-seen timestamps:

| Map | MemoryId | Key | Value |
|-----|----------|-----|-------|
| FIRSTSEEN_ICUSD | 58 | Account (same encoding) | timestamp_ns when first seen |
| FIRSTSEEN_3USD | 59 | Account (same encoding) | timestamp_ns when first seen |

**Balance update logic** (per parsed ICRC-3 block):

```
match operation:
  Transfer(from, to, amount):
    bal[from] = bal[from].saturating_sub(amount)
    bal[to] = bal[to].saturating_add(amount)
    if bal[from] == 0: remove key (keeps map clean)
    if firstseen[to] missing: set firstseen[to] = block_timestamp

  Mint(to, amount):
    bal[to] = bal[to].saturating_add(amount)
    if firstseen[to] missing: set firstseen[to] = block_timestamp

  Burn(from, amount):
    bal[from] = bal[from].saturating_sub(amount)
    if bal[from] == 0: remove key
```

**Key encoding**: The key is a Candid-encoded `Account` (principal + optional 32-byte subaccount). This gives deterministic ordering and compact representation. For most users, the subaccount is null (just the principal).

**3pool balance note**: 3USD is an ICRC-2 token with 8 decimals, so balances are in e8s units, same as icUSD.

**Fee handling**: ICRC-1 transfers deduct a fee from the sender. The block's `amt` field is the amount received by `to`. The fee is a separate field. For balance tracking: sender loses `amt + fee`, receiver gains `amt`. We parse both fields from the block.

## Daily Holder Snapshots

A new collector `collectors/holders.rs` runs on the daily timer (alongside TVL, vaults, stability). It reads the BalanceTracker maps and computes snapshot metrics.

### DailyHolderRow (one per token per day)

Stored in DAILY_HOLDERS_ICUSD (MemoryIds 14/15) and DAILY_HOLDERS_3USD (MemoryIds 16/17).

```rust
struct DailyHolderRow {
    timestamp_ns: u64,
    token: Principal,               // icUSD or 3USD ledger principal
    total_holders: u32,             // accounts with balance > 0
    total_supply_tracked_e8s: u64,  // sum of all balances (sanity check)
    median_balance_e8s: u64,
    top_50: Vec<(Principal, u64)>,  // top 50 by balance (principal, balance). Subaccount dropped (deliberate simplification; most users have null subaccount)
    top_10_pct_bps: u32,            // % of supply held by top 10, in bps
    gini_bps: u32,                  // Gini coefficient * 10000
    new_holders_today: u32,         // firstseen within last 24h
    distribution_buckets: Vec<u32>, // holder counts per bucket
}
```

**Distribution buckets** (in e8s, powers of 10):
- Bucket 0: 0 < balance <= 100_0000_0000 (0-100 tokens)
- Bucket 1: 100-1,000 tokens
- Bucket 2: 1,000-10,000 tokens
- Bucket 3: 10,000-100,000 tokens
- Bucket 4: > 100,000 tokens

**Gini coefficient**: Computed by sorting all balances ascending, then:
```
gini = (2 * sum(i * balance[i] for i in 0..n)) / (n * sum(all_balances)) - (n + 1) / n
```
Result clamped to [0, 1] and stored as basis points (0 = perfect equality, 10000 = one holder has everything).

**Computation cost**: Iterating a StableBTreeMap is O(n) in stable memory reads. For moderate holder counts (< 50,000) this is well within IC instruction limits. If holder counts grow very large in the future, we can sample or cache, but that's not needed now.

**Storable impl**: Candid encoding, `Bound::Unbounded` (variable-length due to top_50 and distribution_buckets).

## Historical Backfill

On first deploy (or when adding a new ICRC-3 source), the cursors start at 0 but the ledgers may have thousands of historical blocks. The backfill mechanism replays these to populate the BalanceTracker.

**Design**: Admin calls `start_backfill(token: Principal)` which sets a `backfill_active` flag in state for that token. While the flag is set, the corresponding ICRC-3 tailing function processes `BACKFILL_BATCH_SIZE` (1000) blocks per tick instead of the normal `BATCH_SIZE` (500). Once the cursor catches up to the ledger tip, the flag auto-clears and steady-state tailing resumes at normal batch size.

**Idempotency**: The cursor itself is the progress marker. If the canister upgrades mid-backfill, the flag persists in SlimState, and the next post_upgrade resumes from the cursor position. No separate "backfill offset" needed.

**Admin gating**: `start_backfill` is an update call restricted to the admin principal. It returns immediately; progress is observable via `get_collector_health()`.

**Archive following**: During backfill, early blocks may be in archive canisters. The `icrc3_get_blocks` response includes `archived_blocks` with callbacks. The backfill tailing function follows these callbacks to fetch archived blocks. Each archive call counts toward the per-tick batch limit.

**Expected backfill duration**: With ~10,000 historical blocks and 1000 blocks per 60s tick, full backfill takes roughly 10 minutes. Acceptable for a one-time operation.

## Pull Cycle Structure

The enhanced pull cycle runs sequentially to stay within instruction limits:

```rust
async fn pull_cycle() {
    refresh_supply_cache().await;

    // Event tailing (each is independent, but run sequentially for instruction budget)
    tail_backend_events().await;
    tail_3pool_swaps().await;
    tail_3pool_liquidity().await;
    tail_amm_swaps().await;

    // ICRC-3 block tailing (feeds BalanceTracker)
    tail_icusd_blocks().await;
    tail_3pool_blocks().await;
}
```

Each `tail_*` function is self-contained: reads its own cursor, fetches, processes, advances. If one fails, the others still run (errors logged, error counter incremented, cursor unchanged).

**Concurrency note**: On IC, `futures::join!` interleaves calls but doesn't reduce total instruction cost. Running sequentially is simpler and avoids any ordering issues with shared state (e.g., EVT_SWAPS receives from both 3pool and AMM).

## get_collector_health() Query

New query endpoint exposing the internal state of the tailing system:

```rust
struct CollectorHealth {
    cursors: Vec<CursorStatus>,
    error_counters: ErrorCounters,   // existing from Phase 1
    backfill_active: Vec<Principal>,  // tokens currently backfilling
    last_pull_cycle_ns: u64,         // timestamp of last successful pull cycle
    balance_tracker_stats: Vec<BalanceTrackerStats>,
}

struct CursorStatus {
    name: String,           // e.g., "backend_events", "icusd_blocks"
    cursor_position: u64,   // current cursor value
    source_count: u64,      // last known source event count (0 if unknown)
    last_success_ns: u64,   // timestamp of last successful fetch
    last_error: Option<String>, // most recent error message, if any
}

struct BalanceTrackerStats {
    token: Principal,
    holder_count: u64,      // number of entries in the balance map
    total_tracked_e8s: u64, // sum of all balances
}
```

This is a query (no state mutation), so it's cheap. The `source_count` field is the last value seen from the count endpoint (cached in state during the pull cycle, not fetched on every health query).

## State Extensions

SlimState gains these fields:

```rust
// Cursor last-success timestamps (persisted across upgrades)
cursor_last_success: HashMap<u8, u64>,  // cursor_id -> timestamp_ns
cursor_last_error: HashMap<u8, String>, // cursor_id -> last error message
cursor_source_counts: HashMap<u8, u64>, // cursor_id -> last known source count

// Backfill flags
backfill_active_icusd: bool,
backfill_active_3usd: bool,

// Pull cycle tracking
last_pull_cycle_ns: u64,
```

The cursor StableCells (positions) are separate from SlimState since they're in their own MemoryIds. The metadata above (last success, errors) lives in SlimState for convenience.

## Query Endpoints

Two new query methods, same pagination pattern as existing series:

- `get_holder_series(RangeQuery, token: Principal) -> HolderSeriesResponse`
  - Returns `Vec<DailyHolderRow>` from DAILY_HOLDERS_ICUSD or DAILY_HOLDERS_3USD based on token principal
- `get_collector_health() -> CollectorHealth`
  - No parameters, returns current system state

Response types:
```
HolderSeriesResponse { rows: Vec<DailyHolderRow>, next_from_ts: Option<u64> }
```

## Timer Wiring

The daily_snapshot function gains a fourth collector call:

```rust
async fn daily_snapshot() {
    let (tvl_res, vaults_res, stability_res, holders_res) = futures::join!(
        collectors::tvl::run(),
        collectors::vaults::run(),
        collectors::stability::run(),
        collectors::holders::run(),  // NEW
    );
    // ... log errors independently
}
```

## Candid Interface Updates

New types added to the `.did` file:
- `CollectorHealth`, `CursorStatus`, `BalanceTrackerStats`
- `DailyHolderRow`
- `HolderSeriesResponse`
- `LiquidationKind`, `VaultEventKind`, `SwapSource`, `LiquidityAction`
- `AnalyticsLiquidationEvent`, `AnalyticsVaultEvent`, `AnalyticsSwapEvent`, `AnalyticsLiquidityEvent`

New methods:
- `get_holder_series : (RangeQuery, principal) -> (HolderSeriesResponse) query`
- `get_collector_health : () -> (CollectorHealth) query`
- `start_backfill : (principal) -> () ` (admin-gated update)

## Error Handling

- Each tailing function catches errors independently. A failed 3pool call does not prevent the AMM tail from running.
- Per-source error counters (already in SlimState) are incremented on failure.
- The most recent error message is cached in `cursor_last_error` for visibility in `get_collector_health()`.
- Cursors never advance past an error. The next tick retries from the same position.
- ICRC-3 blocks that fail to parse are logged and skipped (cursor advances past them). This is necessary because malformed/unknown block types should not stall the entire pipeline.

## Testing

PocketIC integration tests extending `pocket_ic_analytics.rs`:

1. **Backend event tailing**: Deploy analytics + backend with fixture vaults. Execute vault operations (open, borrow, repay). Advance time past pull cycle. Verify EVT_VAULTS log contains expected events.

2. **Liquidation event tailing**: Execute a liquidation on the backend. Advance time. Verify EVT_LIQUIDATIONS log contains the event with correct fields.

3. **3pool swap event tailing**: Execute a swap on the 3pool. Advance time. Verify EVT_SWAPS log contains the event with `source = ThreePool`.

4. **ICRC-3 balance tracking**: Deploy analytics + icusd_ledger. Mint icUSD to accounts, transfer between them. Advance time past multiple pull cycles. Verify BalanceTracker balances match expected values.

5. **Daily holder snapshot**: After populating BalanceTracker, advance past daily tick. Query `get_holder_series`. Verify total_holders, top_50, distribution_buckets.

6. **Collector health**: After tailing, call `get_collector_health()`. Verify cursor positions, last_success timestamps, zero errors.

7. **Backfill**: Pre-populate icusd_ledger with historical mints. Deploy analytics. Call `start_backfill`. Advance time through multiple ticks. Verify BalanceTracker matches expected state and backfill flag auto-cleared.

8. **Error resilience**: Deploy analytics with unavailable source canister. Advance time. Verify other cursors still advance, error counter incremented, `get_collector_health()` shows the error.

9. **Upgrade preserves cursors + balances**: Populate cursors and BalanceTracker, upgrade canister, verify everything survives.

## What This Does NOT Include

- Event-derived daily rollups (Phase 5: liquidation/swap/fee summaries computed from EVT_* logs)
- Fast/hourly snapshot tiers (Phase 5)
- Live query layer with TWAPs, VWAPs, etc. (Phase 6)
- HTTP series endpoints (Phase 7)
- Frontend integration (Phase 7)
