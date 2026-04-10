# Analytics Phase 4: Event Tailing & BalanceTracker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cursor-based event tailing from 6 sources, ICRC-3 block replay for holder tracking, daily holder snapshots, historical backfill, and a collector health query to the rumi_analytics canister.

**Architecture:** The 60s pull cycle polls source canisters for new events since the last cursor position, writing normalized events to EVT_* StableLogs and applying ICRC-3 block deltas to BalanceTracker StableBTreeMaps. A daily holder collector computes snapshot metrics (Gini, top-50, distribution buckets) from the BalanceTracker. Admin-gated backfill processes historical blocks in chunks via the pull cycle.

**Tech Stack:** Rust, ic-cdk 0.12.0, ic-stable-structures 0.6.7, candid 0.10.6, futures 0.3

**Spec:** `docs/superpowers/specs/2026-04-09-analytics-phase4-event-tailing-design.md`

---

## File Structure

### New files
- `src/rumi_analytics/src/storage/events.rs` -- EVT_* row types, Storable impls, StableLog instances, accessor modules
- `src/rumi_analytics/src/storage/cursors.rs` -- StableCell<u64> cursor instances, read/write helpers
- `src/rumi_analytics/src/storage/balance_tracker.rs` -- StableBTreeMap instances for BAL_* and FIRSTSEEN_*, apply_delta/get/iterate helpers
- `src/rumi_analytics/src/storage/holders.rs` -- DailyHolderRow type, Storable impl, StableLog instances, accessor modules
- `src/rumi_analytics/src/sources/amm.rs` -- Source wrapper for AMM swap event queries
- `src/rumi_analytics/src/tailing/mod.rs` -- Module declaration for tailing functions
- `src/rumi_analytics/src/tailing/backend_events.rs` -- Backend event tailing + routing to EVT_LIQUIDATIONS/EVT_VAULTS
- `src/rumi_analytics/src/tailing/three_pool_swaps.rs` -- 3pool swap event tailing
- `src/rumi_analytics/src/tailing/three_pool_liquidity.rs` -- 3pool liquidity event tailing
- `src/rumi_analytics/src/tailing/amm_swaps.rs` -- AMM swap event tailing
- `src/rumi_analytics/src/tailing/icrc3.rs` -- ICRC-3 block tailing + BalanceTracker update (shared by icusd + 3pool)
- `src/rumi_analytics/src/collectors/holders.rs` -- Daily holder snapshot collector (Gini, top-50, distribution)

### Modified files
- `src/rumi_analytics/src/storage.rs` -- Split into `storage/mod.rs` re-exporting submodules; existing types stay
- `src/rumi_analytics/src/sources/mod.rs` -- Add `pub mod amm;`
- `src/rumi_analytics/src/sources/backend.rs` -- Add `get_events()` and `get_event_count()` wrappers
- `src/rumi_analytics/src/sources/icusd_ledger.rs` -- Add `icrc3_get_blocks()` wrapper
- `src/rumi_analytics/src/sources/three_pool.rs` -- Add `get_swap_events()`, `get_swap_event_count()`, `get_liquidity_events()`, `get_liquidity_event_count()`, `icrc3_get_blocks()` wrappers
- `src/rumi_analytics/src/state.rs` -- Add Phase 4 fields to SlimState (cursor metadata, backfill flags, last_pull_cycle_ns)
- `src/rumi_analytics/src/timers.rs` -- Expand pull_cycle with tail_* calls; add holders to daily_snapshot
- `src/rumi_analytics/src/collectors/mod.rs` -- Add `pub mod holders;`
- `src/rumi_analytics/src/types.rs` -- Add HolderSeriesResponse, CollectorHealth, CursorStatus, BalanceTrackerStats
- `src/rumi_analytics/src/queries/historical.rs` -- Add get_holder_series
- `src/rumi_analytics/src/lib.rs` -- Add get_holder_series, get_collector_health, start_backfill entry points
- `src/rumi_analytics/rumi_analytics.did` -- Add all new types and methods
- `src/rumi_analytics/tests/pocket_ic_analytics.rs` -- Phase 4 integration tests

---

## Task Dependency Graph

Tasks 1-4 are foundational (storage + sources) and can be done in order. Task 5 extends SlimState with Phase 4 fields (depends on 1). Tasks 6-8 are source wrappers and tailing functions (depend on 1-5). Task 9 is the holder collector (depends on 1-4). Tasks 10-11 are wiring (depend on 5-9). Task 12 is the Candid interface. Task 13 is integration tests. Task 14 is deploy.

---

### Task 1: Split storage.rs into submodules + add cursor infrastructure

The existing `storage.rs` is 437 lines and about to get much bigger. Split it into `storage/mod.rs` (re-exports + existing types) and new submodule files.

**Files:**
- Rename: `src/rumi_analytics/src/storage.rs` -> `src/rumi_analytics/src/storage/mod.rs`
- Create: `src/rumi_analytics/src/storage/cursors.rs`

**Context:** Existing storage.rs contains all MemoryId constants, SlimState, DailyTvlRow, DailyVaultSnapshotRow, DailyStabilityRow, their Storable impls, thread_local! declarations for MEMORY_MANAGER/SLIM_CELL/DAILY_*_LOG, and accessor modules (daily_tvl, daily_vaults, daily_stability). All of this stays in mod.rs. The MEMORY_MANAGER must be accessible from submodules.

- [ ] **Step 1: Convert storage.rs to storage/mod.rs**

Create directory `src/rumi_analytics/src/storage/` and move `storage.rs` to `storage/mod.rs`. Add submodule declarations at the bottom. Make `MEMORY_MANAGER` accessible to submodules by adding a helper function.

Add to the end of `storage/mod.rs` (after existing code):

```rust
pub mod cursors;
pub mod events;
pub mod balance_tracker;
pub mod holders;

/// Get a virtual memory from the shared MemoryManager. Used by submodules.
pub(crate) fn get_memory(id: MemoryId) -> Memory {
    MEMORY_MANAGER.with(|m| m.borrow().get(id))
}
```

- [ ] **Step 2: Create cursors.rs**

```rust
//! Cursor StableCells for event tailing. Each cursor tracks the next event
//! index to fetch from a source canister.

use ic_stable_structures::StableCell;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_CURSOR_BACKEND_EVENTS, MEM_CURSOR_3POOL_SWAPS,
    MEM_CURSOR_3POOL_LIQUIDITY, MEM_CURSOR_3POOL_BLOCKS,
    MEM_CURSOR_AMM_SWAPS, MEM_CURSOR_ICUSD_BLOCKS,
};

thread_local! {
    static CURSOR_BACKEND_EVENTS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_BACKEND_EVENTS), 0u64)
            .expect("init cursor backend_events")
    );
    static CURSOR_3POOL_SWAPS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_SWAPS), 0u64)
            .expect("init cursor 3pool_swaps")
    );
    static CURSOR_3POOL_LIQUIDITY: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_LIQUIDITY), 0u64)
            .expect("init cursor 3pool_liquidity")
    );
    static CURSOR_3POOL_BLOCKS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_BLOCKS), 0u64)
            .expect("init cursor 3pool_blocks")
    );
    static CURSOR_AMM_SWAPS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_AMM_SWAPS), 0u64)
            .expect("init cursor amm_swaps")
    );
    static CURSOR_ICUSD_BLOCKS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_ICUSD_BLOCKS), 0u64)
            .expect("init cursor icusd_blocks")
    );
}

/// Cursor identifiers matching MemoryIds. Used as keys in SlimState metadata maps.
pub const CURSOR_ID_BACKEND_EVENTS: u8 = 1;
pub const CURSOR_ID_3POOL_SWAPS: u8 = 2;
pub const CURSOR_ID_3POOL_LIQUIDITY: u8 = 3;
pub const CURSOR_ID_3POOL_BLOCKS: u8 = 4;
pub const CURSOR_ID_AMM_SWAPS: u8 = 5;
pub const CURSOR_ID_ICUSD_BLOCKS: u8 = 7;

macro_rules! cursor_accessors {
    ($mod_name:ident, $cell:ident) => {
        pub mod $mod_name {
            use super::*;
            pub fn get() -> u64 {
                $cell.with(|c| *c.borrow().get())
            }
            pub fn set(val: u64) {
                $cell.with(|c| c.borrow_mut().set(val).expect(concat!("set cursor ", stringify!($mod_name))));
            }
        }
    };
}

cursor_accessors!(backend_events, CURSOR_BACKEND_EVENTS);
cursor_accessors!(three_pool_swaps, CURSOR_3POOL_SWAPS);
cursor_accessors!(three_pool_liquidity, CURSOR_3POOL_LIQUIDITY);
cursor_accessors!(three_pool_blocks, CURSOR_3POOL_BLOCKS);
cursor_accessors!(amm_swaps, CURSOR_AMM_SWAPS);
cursor_accessors!(icusd_blocks, CURSOR_ICUSD_BLOCKS);
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`
Expected: No errors. (Warnings about unused submodules are fine since events.rs, balance_tracker.rs, holders.rs don't exist yet.)

- [ ] **Step 4: Create empty placeholder submodules so it compiles clean**

Create empty files for the modules declared in mod.rs that don't exist yet:

`src/rumi_analytics/src/storage/events.rs`:
```rust
//! EVT_* event log types and StableLog instances. Populated in Task 2.
```

`src/rumi_analytics/src/storage/balance_tracker.rs`:
```rust
//! BalanceTracker StableBTreeMap instances. Populated in Task 3.
```

`src/rumi_analytics/src/storage/holders.rs`:
```rust
//! DailyHolderRow type and StableLog instances. Populated in Task 4.
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`
Expected: No errors.

- [ ] **Step 6: Run existing tests to confirm no regressions**

Run: `cargo test -p rumi_analytics --lib 2>&1 | tail -5`
Expected: All existing unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_analytics/src/storage/ src/rumi_analytics/src/storage.rs
git commit -m "refactor(analytics): split storage.rs into submodules + add cursor infrastructure"
```

Note: `git add` will pick up the deletion of `storage.rs` and creation of `storage/` automatically.

---

### Task 2: EVT_* event log types and StableLogs

**Files:**
- Create (populate): `src/rumi_analytics/src/storage/events.rs`

**Context:** The spec defines 4 normalized event types: AnalyticsLiquidationEvent, AnalyticsVaultEvent, AnalyticsSwapEvent, AnalyticsLiquidityEvent. Each needs a Storable impl (Candid encoding, Bound::Unbounded), a StableLog in thread_local!, and an accessor module with push/len/get/range. Follow the exact pattern from `storage/mod.rs`'s daily_tvl module.

- [ ] **Step 1: Write unit tests for event type serialization**

Add to the bottom of `events.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    #[test]
    fn liquidation_event_roundtrip() {
        let evt = AnalyticsLiquidationEvent {
            timestamp_ns: 1_000_000,
            source_event_id: 42,
            vault_id: 7,
            collateral_type: Principal::anonymous(),
            collateral_amount: 500_000_000,
            debt_amount: 100_000_000,
            liquidation_kind: LiquidationKind::Full,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsLiquidationEvent::from_bytes(bytes);
        assert_eq!(decoded.vault_id, 7);
        assert_eq!(decoded.collateral_amount, 500_000_000);
    }

    #[test]
    fn swap_event_roundtrip() {
        let evt = AnalyticsSwapEvent {
            timestamp_ns: 2_000_000,
            source: SwapSource::ThreePool,
            source_event_id: 10,
            caller: Principal::anonymous(),
            token_in: Principal::anonymous(),
            token_out: Principal::anonymous(),
            amount_in: 1_000_000,
            amount_out: 999_000,
            fee: 1_000,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsSwapEvent::from_bytes(bytes);
        assert_eq!(decoded.amount_in, 1_000_000);
        assert!(matches!(decoded.source, SwapSource::ThreePool));
    }

    #[test]
    fn vault_event_roundtrip() {
        let evt = AnalyticsVaultEvent {
            timestamp_ns: 3_000_000,
            source_event_id: 5,
            vault_id: 1,
            owner: Principal::anonymous(),
            event_kind: VaultEventKind::Opened,
            collateral_type: Principal::anonymous(),
            amount: 10_000_000_000,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsVaultEvent::from_bytes(bytes);
        assert_eq!(decoded.vault_id, 1);
        assert!(matches!(decoded.event_kind, VaultEventKind::Opened));
    }

    #[test]
    fn liquidity_event_roundtrip() {
        let evt = AnalyticsLiquidityEvent {
            timestamp_ns: 4_000_000,
            source_event_id: 20,
            caller: Principal::anonymous(),
            action: LiquidityAction::Add,
            amounts: vec![100, 200, 300],
            lp_amount: 500,
            coin_index: None,
            fee: Some(5),
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsLiquidityEvent::from_bytes(bytes);
        assert_eq!(decoded.amounts, vec![100, 200, 300]);
        assert_eq!(decoded.fee, Some(5));
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p rumi_analytics --lib events 2>&1 | tail -5`
Expected: FAIL (types don't exist yet)

- [ ] **Step 3: Implement event types, Storable impls, StableLogs, and accessor modules**

Write the full `events.rs`:

```rust
//! EVT_* normalized event log types and StableLog instances.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_EVT_LIQUIDATIONS_IDX, MEM_EVT_LIQUIDATIONS_DATA,
    MEM_EVT_SWAPS_IDX, MEM_EVT_SWAPS_DATA,
    MEM_EVT_LIQUIDITY_IDX, MEM_EVT_LIQUIDITY_DATA,
    MEM_EVT_VAULTS_IDX, MEM_EVT_VAULTS_DATA,
};

// --- Enum types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LiquidationKind {
    Full,
    Partial,
    Redistribution,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum VaultEventKind {
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

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SwapSource {
    ThreePool,
    Amm,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LiquidityAction {
    Add,
    Remove,
    RemoveOneCoin,
    Donate,
}

// --- Event row types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsLiquidationEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub collateral_amount: u64,
    pub debt_amount: u64,
    pub liquidation_kind: LiquidationKind,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsVaultEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub vault_id: u64,
    pub owner: Principal,
    pub event_kind: VaultEventKind,
    pub collateral_type: Principal,
    pub amount: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsSwapEvent {
    pub timestamp_ns: u64,
    pub source: SwapSource,
    pub source_event_id: u64,
    pub caller: Principal,
    pub token_in: Principal,
    pub token_out: Principal,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsLiquidityEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: Vec<u64>,
    pub lp_amount: u64,
    pub coin_index: Option<u8>,
    pub fee: Option<u64>,
}

// --- Storable impls ---

macro_rules! storable_candid {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<[u8]> {
                Cow::Owned(Encode!(self).expect(concat!(stringify!($t), " encode")))
            }
            fn from_bytes(bytes: Cow<[u8]>) -> Self {
                Decode!(bytes.as_ref(), Self).expect(concat!(stringify!($t), " decode"))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

storable_candid!(AnalyticsLiquidationEvent);
storable_candid!(AnalyticsVaultEvent);
storable_candid!(AnalyticsSwapEvent);
storable_candid!(AnalyticsLiquidityEvent);

// --- StableLog instances ---

thread_local! {
    static EVT_LIQUIDATIONS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsLiquidationEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_LIQUIDATIONS_IDX),
                get_memory(MEM_EVT_LIQUIDATIONS_DATA),
            ).expect("init EVT_LIQUIDATIONS log")
        });

    static EVT_SWAPS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsSwapEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_SWAPS_IDX),
                get_memory(MEM_EVT_SWAPS_DATA),
            ).expect("init EVT_SWAPS log")
        });

    static EVT_LIQUIDITY_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsLiquidityEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_LIQUIDITY_IDX),
                get_memory(MEM_EVT_LIQUIDITY_DATA),
            ).expect("init EVT_LIQUIDITY log")
        });

    static EVT_VAULTS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsVaultEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_VAULTS_IDX),
                get_memory(MEM_EVT_VAULTS_DATA),
            ).expect("init EVT_VAULTS log")
        });
}

// --- Accessor modules ---

macro_rules! evt_accessors {
    ($mod_name:ident, $log:ident, $row_type:ty) => {
        pub mod $mod_name {
            use super::*;

            pub fn push(row: $row_type) {
                $log.with(|log| {
                    log.borrow_mut().append(&row).expect(concat!("append ", stringify!($mod_name)));
                });
            }

            pub fn len() -> u64 {
                $log.with(|log| log.borrow().len())
            }

            pub fn get(index: u64) -> Option<$row_type> {
                $log.with(|log| log.borrow().get(index))
            }

            pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<$row_type> {
                let mut out = Vec::new();
                $log.with(|log| {
                    let log = log.borrow();
                    let n = log.len();
                    for i in 0..n {
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= to_ts {
                                break;
                            }
                            if row.timestamp_ns >= from_ts {
                                out.push(row);
                                if out.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                });
                out
            }
        }
    };
}

evt_accessors!(evt_liquidations, EVT_LIQUIDATIONS_LOG, AnalyticsLiquidationEvent);
evt_accessors!(evt_swaps, EVT_SWAPS_LOG, AnalyticsSwapEvent);
evt_accessors!(evt_liquidity, EVT_LIQUIDITY_LOG, AnalyticsLiquidityEvent);
evt_accessors!(evt_vaults, EVT_VAULTS_LOG, AnalyticsVaultEvent);

// Tests at the bottom (from Step 1)
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rumi_analytics --lib events 2>&1 | tail -5`
Expected: All 4 roundtrip tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/storage/events.rs
git commit -m "feat(analytics): add EVT_* event log types and StableLogs"
```

---

### Task 3: BalanceTracker StableBTreeMap instances

**Files:**
- Create (populate): `src/rumi_analytics/src/storage/balance_tracker.rs`

**Context:** Two balance maps (BAL_ICUSD MemoryId 56, BAL_3USD MemoryId 57) and two first-seen maps (FIRSTSEEN_ICUSD MemoryId 58, FIRSTSEEN_3USD MemoryId 59). Key is a Candid-encoded `Account` (Principal + optional subaccount). Value is u64. The balance tracker needs apply_transfer, apply_mint, apply_burn operations, plus iteration for the holder snapshot.

Note: `ic-stable-structures` `StableBTreeMap` requires fixed-size keys when using `Bound::Bounded`. For variable-length Account keys, we use `Vec<u8>` as the key type with `Bound::Unbounded` which is NOT supported by StableBTreeMap. Instead, we encode the Account to a fixed-size byte array. An Account is at most 29 bytes (principal) + 32 bytes (subaccount) = 61 bytes. We'll use a 64-byte fixed buffer with length prefix.

- [ ] **Step 1: Write unit tests**

Add to the bottom of `balance_tracker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    #[test]
    fn account_key_roundtrip() {
        let acct = Account {
            owner: Principal::anonymous(),
            subaccount: None,
        };
        let key = AccountKey::from_account(&acct);
        let decoded = key.to_account();
        assert_eq!(decoded.owner, acct.owner);
        assert_eq!(decoded.subaccount, acct.subaccount);
    }

    #[test]
    fn account_key_with_subaccount_roundtrip() {
        let mut sub = [0u8; 32];
        sub[0] = 1;
        sub[31] = 0xFF;
        let acct = Account {
            owner: Principal::anonymous(),
            subaccount: Some(sub),
        };
        let key = AccountKey::from_account(&acct);
        let decoded = key.to_account();
        assert_eq!(decoded.owner, acct.owner);
        assert_eq!(decoded.subaccount, Some(sub));
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p rumi_analytics --lib balance_tracker 2>&1 | tail -5`
Expected: FAIL

- [ ] **Step 3: Implement balance_tracker.rs**

```rust
//! BalanceTracker: StableBTreeMap-backed running balance and first-seen
//! tracking for icUSD and 3USD holders.

use candid::{Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use ic_stable_structures::StableBTreeMap;
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{MEM_BAL_ICUSD, MEM_BAL_3USD, MEM_FIRSTSEEN_ICUSD, MEM_FIRSTSEEN_3USD};

/// ICRC-1 Account: principal + optional 32-byte subaccount.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<[u8; 32]>,
}

/// Fixed-size key for StableBTreeMap. Layout: 1 byte principal length,
/// up to 29 bytes principal, 1 byte subaccount flag (0 or 1),
/// 32 bytes subaccount (zeroed if absent). Total: 63 bytes.
const ACCOUNT_KEY_LEN: usize = 63;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AccountKey(pub [u8; ACCOUNT_KEY_LEN]);

impl AccountKey {
    pub fn from_account(acct: &Account) -> Self {
        let mut buf = [0u8; ACCOUNT_KEY_LEN];
        let principal_bytes = acct.owner.as_slice();
        buf[0] = principal_bytes.len() as u8;
        buf[1..1 + principal_bytes.len()].copy_from_slice(principal_bytes);
        match &acct.subaccount {
            Some(sub) => {
                buf[30] = 1;
                buf[31..63].copy_from_slice(sub);
            }
            None => {
                buf[30] = 0;
                // bytes 31..63 stay zeroed
            }
        }
        Self(buf)
    }

    pub fn to_account(&self) -> Account {
        let plen = self.0[0] as usize;
        let owner = Principal::from_slice(&self.0[1..1 + plen]);
        let subaccount = if self.0[30] == 1 {
            let mut sub = [0u8; 32];
            sub.copy_from_slice(&self.0[31..63]);
            Some(sub)
        } else {
            None
        };
        Account { owner, subaccount }
    }

    /// Extract just the owner Principal without constructing full Account.
    pub fn owner(&self) -> Principal {
        let plen = self.0[0] as usize;
        Principal::from_slice(&self.0[1..1 + plen])
    }
}

impl Storable for AccountKey {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Borrowed(&self.0)
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        let mut buf = [0u8; ACCOUNT_KEY_LEN];
        buf.copy_from_slice(&bytes[..ACCOUNT_KEY_LEN]);
        Self(buf)
    }
    const BOUND: Bound = Bound::Bounded {
        max_size: ACCOUNT_KEY_LEN as u32,
        is_fixed_size: true,
    };
}

#[derive(Clone, Debug)]
pub struct BalVal(pub u64);

impl Storable for BalVal {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(self.0.to_le_bytes().to_vec())
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes[..8]);
        Self(u64::from_le_bytes(arr))
    }
    const BOUND: Bound = Bound::Bounded {
        max_size: 8,
        is_fixed_size: true,
    };
}

thread_local! {
    static BAL_ICUSD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_BAL_ICUSD))
    );
    static BAL_3USD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_BAL_3USD))
    );
    static FIRSTSEEN_ICUSD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_FIRSTSEEN_ICUSD))
    );
    static FIRSTSEEN_3USD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_FIRSTSEEN_3USD))
    );
}

/// Which token's maps to operate on.
#[derive(Clone, Copy, Debug)]
pub enum Token { IcUsd, ThreeUsd }

fn with_bal<F, R>(token: Token, f: F) -> R
where F: FnOnce(&mut StableBTreeMap<AccountKey, BalVal, Memory>) -> R {
    match token {
        Token::IcUsd => BAL_ICUSD_MAP.with(|m| f(&mut m.borrow_mut())),
        Token::ThreeUsd => BAL_3USD_MAP.with(|m| f(&mut m.borrow_mut())),
    }
}

fn with_firstseen<F, R>(token: Token, f: F) -> R
where F: FnOnce(&mut StableBTreeMap<AccountKey, BalVal, Memory>) -> R {
    match token {
        Token::IcUsd => FIRSTSEEN_ICUSD_MAP.with(|m| f(&mut m.borrow_mut())),
        Token::ThreeUsd => FIRSTSEEN_3USD_MAP.with(|m| f(&mut m.borrow_mut())),
    }
}

/// Record first-seen timestamp if not already set.
fn maybe_set_firstseen(token: Token, key: &AccountKey, timestamp_ns: u64) {
    with_firstseen(token, |map| {
        if map.get(key).is_none() {
            map.insert(key.clone(), BalVal(timestamp_ns));
        }
    });
}

/// Credit an account. Inserts if not present.
pub fn credit(token: Token, acct: &Account, amount: u64, timestamp_ns: u64) {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| {
        let current = map.get(&key).map(|v| v.0).unwrap_or(0);
        map.insert(key.clone(), BalVal(current.saturating_add(amount)));
    });
    maybe_set_firstseen(token, &key, timestamp_ns);
}

/// Debit an account. Removes entry if balance reaches 0.
pub fn debit(token: Token, acct: &Account, amount: u64) {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| {
        let current = map.get(&key).map(|v| v.0).unwrap_or(0);
        let new_bal = current.saturating_sub(amount);
        if new_bal == 0 {
            map.remove(&key);
        } else {
            map.insert(key, BalVal(new_bal));
        }
    });
}

/// Apply a transfer: debit sender (amount + fee), credit receiver (amount).
pub fn apply_transfer(
    token: Token,
    from: &Account,
    to: &Account,
    amount: u64,
    fee: u64,
    timestamp_ns: u64,
) {
    debit(token, from, amount.saturating_add(fee));
    credit(token, to, amount, timestamp_ns);
}

/// Apply a mint: credit receiver.
pub fn apply_mint(token: Token, to: &Account, amount: u64, timestamp_ns: u64) {
    credit(token, to, amount, timestamp_ns);
}

/// Apply a burn: debit sender.
pub fn apply_burn(token: Token, from: &Account, amount: u64) {
    debit(token, from, amount);
}

/// Get balance for a specific account.
pub fn get_balance(token: Token, acct: &Account) -> u64 {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| map.get(&key).map(|v| v.0).unwrap_or(0))
}

/// Get total holder count for a token.
pub fn holder_count(token: Token) -> u64 {
    with_bal(token, |map| map.len())
}

/// Iterate all (account, balance) pairs. Collects into a Vec.
/// For holder snapshot computation (Gini, top-N, buckets).
pub fn all_balances(token: Token) -> Vec<(Account, u64)> {
    with_bal(token, |map| {
        map.iter().map(|(k, v)| (k.to_account(), v.0)).collect()
    })
}

/// Get first-seen timestamp for an account, if tracked.
pub fn get_firstseen(token: Token, acct: &Account) -> Option<u64> {
    let key = AccountKey::from_account(acct);
    with_firstseen(token, |map| map.get(&key).map(|v| v.0))
}

/// Count accounts whose first-seen timestamp falls in [from_ns, to_ns).
pub fn count_new_holders(token: Token, from_ns: u64, to_ns: u64) -> u32 {
    with_firstseen(token, |map| {
        map.iter()
            .filter(|(_, v)| v.0 >= from_ns && v.0 < to_ns)
            .count() as u32
    })
}

/// Get sum of all balances (for sanity check).
pub fn total_supply_tracked(token: Token) -> u64 {
    with_bal(token, |map| {
        map.iter().map(|(_, v)| v.0).fold(0u64, |a, b| a.saturating_add(b))
    })
}

// ... tests from Step 1 go here
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rumi_analytics --lib balance_tracker 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/storage/balance_tracker.rs
git commit -m "feat(analytics): add BalanceTracker StableBTreeMap instances"
```

---

### Task 4: DailyHolderRow type + StableLogs + holder snapshot collector

**Files:**
- Create (populate): `src/rumi_analytics/src/storage/holders.rs`
- Create: `src/rumi_analytics/src/collectors/holders.rs`
- Modify: `src/rumi_analytics/src/collectors/mod.rs`

**Context:** DailyHolderRow stores daily holder metrics per token. The holder collector reads from the BalanceTracker to compute Gini coefficient, top 50 holders, distribution buckets, median balance, and new-holder count. Two StableLogs: DAILY_HOLDERS_ICUSD (MemoryIds 14/15) and DAILY_HOLDERS_3USD (MemoryIds 16/17).

- [ ] **Step 1: Write unit tests for Gini + snapshot computation**

In `src/rumi_analytics/src/collectors/holders.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gini_perfect_equality() {
        // All same balance => Gini = 0
        let balances = vec![100, 100, 100, 100];
        assert_eq!(compute_gini_bps(&balances), 0);
    }

    #[test]
    fn gini_perfect_inequality() {
        // One holder has everything
        let balances = vec![0, 0, 0, 1000];
        // Gini should be close to 10000 (bps) for extreme inequality
        let g = compute_gini_bps(&balances);
        assert!(g > 7000, "expected high Gini, got {}", g);
    }

    #[test]
    fn gini_moderate_inequality() {
        let balances = vec![10, 20, 30, 40, 50];
        let g = compute_gini_bps(&balances);
        assert!(g > 0 && g < 5000, "expected moderate Gini, got {}", g);
    }

    #[test]
    fn gini_empty() {
        assert_eq!(compute_gini_bps(&[]), 0);
    }

    #[test]
    fn gini_single() {
        assert_eq!(compute_gini_bps(&[100]), 0);
    }

    #[test]
    fn distribution_buckets_correct() {
        // Buckets: 0-100, 100-1k, 1k-10k, 10k-100k, >100k (in tokens, but input is e8s)
        let balances_e8s: Vec<u64> = vec![
            50_0000_0000,       // 50 tokens -> bucket 0
            500_0000_0000,      // 500 tokens -> bucket 1
            5_000_0000_0000,    // 5,000 tokens -> bucket 2
            50_000_0000_0000,   // 50,000 tokens -> bucket 3
            500_000_0000_0000,  // 500,000 tokens -> bucket 4
            10_0000_0000,       // 10 tokens -> bucket 0
        ];
        let buckets = compute_distribution_buckets(&balances_e8s);
        assert_eq!(buckets, vec![2, 1, 1, 1, 1]);
    }

    #[test]
    fn top_n_extracts_correct_top() {
        let holders = vec![
            (candid::Principal::anonymous(), 100u64),
            (candid::Principal::anonymous(), 500),
            (candid::Principal::anonymous(), 200),
        ];
        let top = top_n_holders(&holders, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1, 500);
        assert_eq!(top[1].1, 200);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p rumi_analytics --lib holders 2>&1 | tail -5`
Expected: FAIL

- [ ] **Step 3: Implement holders storage type**

In `src/rumi_analytics/src/storage/holders.rs`:

```rust
//! DailyHolderRow type and StableLog instances for icUSD and 3USD holder snapshots.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_DAILY_HOLDERS_ICUSD_IDX, MEM_DAILY_HOLDERS_ICUSD_DATA,
    MEM_DAILY_HOLDERS_3USD_IDX, MEM_DAILY_HOLDERS_3USD_DATA,
};

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct DailyHolderRow {
    pub timestamp_ns: u64,
    pub token: Principal,
    pub total_holders: u32,
    pub total_supply_tracked_e8s: u64,
    pub median_balance_e8s: u64,
    pub top_50: Vec<(Principal, u64)>,
    pub top_10_pct_bps: u32,
    pub gini_bps: u32,
    pub new_holders_today: u32,
    pub distribution_buckets: Vec<u32>,
}

impl Storable for DailyHolderRow {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(Encode!(self).expect("DailyHolderRow encode"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).expect("DailyHolderRow decode")
    }
    const BOUND: Bound = Bound::Unbounded;
}

thread_local! {
    static DAILY_HOLDERS_ICUSD_LOG: RefCell<ic_stable_structures::StableLog<DailyHolderRow, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_HOLDERS_ICUSD_IDX),
                get_memory(MEM_DAILY_HOLDERS_ICUSD_DATA),
            ).expect("init DAILY_HOLDERS_ICUSD log")
        });

    static DAILY_HOLDERS_3USD_LOG: RefCell<ic_stable_structures::StableLog<DailyHolderRow, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_DAILY_HOLDERS_3USD_IDX),
                get_memory(MEM_DAILY_HOLDERS_3USD_DATA),
            ).expect("init DAILY_HOLDERS_3USD log")
        });
}

macro_rules! holder_accessors {
    ($mod_name:ident, $log:ident) => {
        pub mod $mod_name {
            use super::*;

            pub fn push(row: DailyHolderRow) {
                $log.with(|log| {
                    log.borrow_mut().append(&row).expect(concat!("append ", stringify!($mod_name)));
                });
            }

            pub fn len() -> u64 {
                $log.with(|log| log.borrow().len())
            }

            pub fn get(index: u64) -> Option<DailyHolderRow> {
                $log.with(|log| log.borrow().get(index))
            }

            pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<DailyHolderRow> {
                let mut out = Vec::new();
                $log.with(|log| {
                    let log = log.borrow();
                    let n = log.len();
                    for i in 0..n {
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= to_ts { break; }
                            if row.timestamp_ns >= from_ts {
                                out.push(row);
                                if out.len() >= limit { break; }
                            }
                        }
                    }
                });
                out
            }
        }
    };
}

holder_accessors!(daily_holders_icusd, DAILY_HOLDERS_ICUSD_LOG);
holder_accessors!(daily_holders_3usd, DAILY_HOLDERS_3USD_LOG);
```

- [ ] **Step 4: Implement the holder collector**

In `src/rumi_analytics/src/collectors/holders.rs`:

```rust
//! Daily holder snapshot collector. Reads BalanceTracker state and computes
//! Gini coefficient, top-50, distribution buckets, median balance.

use candid::Principal;
use crate::{state, storage};
use storage::balance_tracker::{self, Token};
use storage::holders::DailyHolderRow;

const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// Distribution bucket thresholds in e8s (powers of 10 in token terms).
const BUCKET_THRESHOLDS: [u64; 4] = [
    100_0000_0000,       // 100 tokens
    1_000_0000_0000,     // 1,000 tokens
    10_000_0000_0000,    // 10,000 tokens
    100_000_0000_0000,   // 100,000 tokens
];

pub fn compute_gini_bps(sorted_balances: &[u64]) -> u32 {
    let n = sorted_balances.len();
    if n <= 1 {
        return 0;
    }
    let total: u128 = sorted_balances.iter().map(|&b| b as u128).sum();
    if total == 0 {
        return 0;
    }
    // Gini = (2 * sum(i * balance[i])) / (n * total) - (n + 1) / n
    // Using 1-indexed: sum((2*i - n - 1) * balance[i]) / (n * total)
    let n128 = n as u128;
    let mut numerator: i128 = 0;
    for (i, &bal) in sorted_balances.iter().enumerate() {
        let rank = (i as i128 + 1) * 2 - n128 as i128 - 1;
        numerator += rank * bal as i128;
    }
    let gini = numerator as f64 / (n128 as f64 * total as f64);
    let gini_clamped = gini.clamp(0.0, 1.0);
    (gini_clamped * 10_000.0) as u32
}

pub fn compute_distribution_buckets(balances_e8s: &[u64]) -> Vec<u32> {
    let mut buckets = vec![0u32; BUCKET_THRESHOLDS.len() + 1];
    for &bal in balances_e8s {
        let idx = BUCKET_THRESHOLDS.iter().position(|&t| bal <= t).unwrap_or(BUCKET_THRESHOLDS.len());
        buckets[idx] += 1;
    }
    buckets
}

pub fn top_n_holders(holders: &[(Principal, u64)], n: usize) -> Vec<(Principal, u64)> {
    let mut sorted: Vec<_> = holders.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted.truncate(n);
    sorted
}

fn compute_median(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n == 0 { return 0; }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        let lo = sorted[n / 2 - 1] as u128;
        let hi = sorted[n / 2] as u128;
        ((lo + hi) / 2) as u64
    }
}

fn snapshot_token(token: Token, token_principal: Principal, now_ns: u64) -> DailyHolderRow {
    let all = balance_tracker::all_balances(token);

    let total_holders = all.len() as u32;
    let total_supply_tracked = balance_tracker::total_supply_tracked(token);

    // Collect balances for stats, and (principal, balance) for top-N.
    let mut balances: Vec<u64> = all.iter().map(|(_, b)| *b).collect();
    let principal_balances: Vec<(Principal, u64)> = all.iter().map(|(a, b)| (a.owner.clone(), *b)).collect();

    balances.sort_unstable();
    let median = compute_median(&balances);
    let gini = compute_gini_bps(&balances);
    let buckets = compute_distribution_buckets(&balances);
    let top_50 = top_n_holders(&principal_balances, 50);

    // Top-10 concentration: sum of top 10 balances / total * 10000
    let top_10_sum: u64 = {
        let mut sorted_desc = balances.clone();
        sorted_desc.sort_unstable_by(|a, b| b.cmp(a));
        sorted_desc.iter().take(10).sum()
    };
    let top_10_pct = if total_supply_tracked > 0 {
        ((top_10_sum as f64 / total_supply_tracked as f64) * 10_000.0) as u32
    } else {
        0
    };

    // New holders: first-seen within last 24h.
    let day_start = now_ns.saturating_sub(NANOS_PER_DAY);
    let new_holders = balance_tracker::count_new_holders(token, day_start, now_ns);

    DailyHolderRow {
        timestamp_ns: now_ns,
        token: token_principal,
        total_holders,
        total_supply_tracked_e8s: total_supply_tracked,
        median_balance_e8s: median,
        top_50,
        top_10_pct_bps: top_10_pct,
        gini_bps: gini,
        new_holders_today: new_holders,
        distribution_buckets: buckets,
    }
}

pub async fn run() -> Result<(), String> {
    let (icusd_ledger, three_pool) = state::read_state(|s| {
        (s.sources.icusd_ledger, s.sources.three_pool)
    });

    let now = ic_cdk::api::time();

    // Only produce snapshots if the BalanceTracker has data (cursors have run).
    if balance_tracker::holder_count(Token::IcUsd) > 0 {
        let row = snapshot_token(Token::IcUsd, icusd_ledger, now);
        storage::holders::daily_holders_icusd::push(row);
    }

    if balance_tracker::holder_count(Token::ThreeUsd) > 0 {
        let row = snapshot_token(Token::ThreeUsd, three_pool, now);
        storage::holders::daily_holders_3usd::push(row);
    }

    Ok(())
}

// ... tests from Step 1 go here
```

- [ ] **Step 5: Add `pub mod holders;` to collectors/mod.rs**

- [ ] **Step 6: Run tests**

Run: `cargo test -p rumi_analytics --lib holders 2>&1 | tail -10`
Expected: All holder tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_analytics/src/storage/holders.rs src/rumi_analytics/src/collectors/holders.rs src/rumi_analytics/src/collectors/mod.rs
git commit -m "feat(analytics): add DailyHolderRow + holder snapshot collector with Gini/top-50/buckets"
```

---

### Task 5: Extend SlimState for Phase 4 metadata

**Files:**
- Modify: `src/rumi_analytics/src/storage/mod.rs` (SlimState struct)

**Context:** Add cursor metadata (last_success, last_error, source_counts), backfill flags, and pull cycle tracking to SlimState. All new fields are `Option` or `Default` for backward compat with existing on-chain state (Candid record subtyping handles the upgrade).

- [ ] **Step 1: Add Phase 4 fields to SlimState**

In `src/rumi_analytics/src/storage/mod.rs`, add to the `SlimState` struct after `error_counters`:

```rust
    // Phase 4: Cursor metadata (persisted across upgrades)
    pub cursor_last_success: Option<std::collections::HashMap<u8, u64>>,
    pub cursor_last_error: Option<std::collections::HashMap<u8, String>>,
    pub cursor_source_counts: Option<std::collections::HashMap<u8, u64>>,
    // Phase 4: Backfill flags
    pub backfill_active_icusd: Option<bool>,
    pub backfill_active_3usd: Option<bool>,
    // Phase 4: Pull cycle tracking
    pub last_pull_cycle_ns: Option<u64>,
```

Update `Default` impl to include these with `None` values.

- [ ] **Step 2: Verify it compiles and existing tests pass**

Run: `cargo test -p rumi_analytics --lib 2>&1 | tail -5`
Expected: All existing tests pass. The `Option` wrapping ensures Candid backward compat.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_analytics/src/storage/mod.rs
git commit -m "feat(analytics): extend SlimState with Phase 4 cursor metadata and backfill flags"
```

---

### Task 6: Source wrappers for event tailing

**Files:**
- Modify: `src/rumi_analytics/src/sources/backend.rs` -- Add get_events, get_event_count
- Modify: `src/rumi_analytics/src/sources/three_pool.rs` -- Add swap/liquidity event fetchers + icrc3_get_blocks
- Modify: `src/rumi_analytics/src/sources/icusd_ledger.rs` -- Add icrc3_get_blocks
- Create: `src/rumi_analytics/src/sources/amm.rs` -- AMM swap event fetchers
- Modify: `src/rumi_analytics/src/sources/mod.rs` -- Add `pub mod amm;`

**Context:** Each source wrapper follows the same pattern as existing ones in `backend.rs`: define a subset struct matching the Candid types, do an `ic_cdk::call`, map errors. For the backend Event type, we define a minimal subset enum covering only the variants we route. For ICRC-3, we need to decode the generic `ICRC3Value` tree.

The backend's `Event` is a variant with 62 cases. We only deserialize the ones we care about. In Candid, unknown variant fields are ignored during deserialization if the Rust enum has a catch-all. We use `#[serde(other)]` for this.

- [ ] **Step 1: Add backend event wrappers to sources/backend.rs**

Add to `sources/backend.rs`:

```rust
// --- Event tailing (Phase 4) ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
}

/// Minimal subset of the backend Event variant. Only the variants we route
/// to EVT_LIQUIDATIONS and EVT_VAULTS are deserialized; all others fall
/// through to `Unknown`.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BackendEvent {
    #[serde(rename = "open_vault")]
    OpenVault {
        block_index: u64,
        vault: BackendVaultRecord,
        timestamp: Option<u64>,
    },
    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        block_index: u64,
        vault_id: u64,
        fee_amount: u64,
        borrowed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "repay_to_vault")]
    RepayToVault {
        block_index: u64,
        vault_id: u64,
        repayed_amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "collateral_withdrawn")]
    CollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_collateral_withdrawn")]
    PartialCollateralWithdrawn {
        block_index: u64,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "withdraw_and_close_vault")]
    WithdrawAndCloseVault {
        block_index: Option<u64>,
        vault_id: u64,
        amount: u64,
        caller: Option<Principal>,
        timestamp: Option<u64>,
    },
    VaultWithdrawnAndClosed {
        vault_id: u64,
        timestamp: u64,
        caller: Principal,
        amount: u64,
    },
    #[serde(rename = "dust_forgiven")]
    DustForgiven {
        vault_id: u64,
        amount: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        icusd_amount: u64,
        icusd_block_index: u64,
        owner: Principal,
        fee_amount: u64,
        collateral_type: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "liquidate_vault")]
    LiquidateVault {
        vault_id: u64,
        liquidator: Option<Principal>,
        timestamp: Option<u64>,
    },
    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        protocol_fee_collateral: Option<u64>,
        liquidator_payment: u64,
        vault_id: u64,
        liquidator: Option<Principal>,
        icp_to_liquidator: u64,
        timestamp: Option<u64>,
    },
    #[serde(rename = "redistribute_vault")]
    RedistributeVault {
        vault_id: u64,
        timestamp: Option<u64>,
    },
    // Catch-all for all other event variants we don't care about.
    #[serde(other)]
    Unknown,
}

/// Vault record embedded in open_vault events.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BackendVaultRecord {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_type: Principal,
    pub collateral_amount: u64,
    pub borrowed_icusd_amount: u64,
}

pub async fn get_events(
    backend: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<BackendEvent>, String> {
    let arg = GetEventsArg { start, length };
    let (events,): (Vec<BackendEvent>,) = ic_cdk::call(backend, "get_events", (arg,))
        .await
        .map_err(|(code, msg)| format!("get_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_event_count(backend: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(backend, "get_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_event_count: {:?} {}", code, msg))?;
    Ok(count)
}
```

**Important note for the implementer:** Candid deserialization does NOT use serde's `#[serde(other)]`. The `candid::Deserialize` derive has its own mechanism. Test the `Unknown` catch-all variant immediately after writing this code. If Candid rejects unknown variants at deserialization time, switch to this alternative: define all 62 backend event variants explicitly (copy from the .did file), or deserialize as `Vec<candid::IDLValue>` and pattern-match variant names. The priority is: get it compiling and correctly handling unknown variants before moving on to the routing logic. If you need to switch to IDLValue, define a helper `fn try_extract_backend_event(val: &candid::IDLValue) -> Option<BackendEvent>` that matches on variant name strings.

- [ ] **Step 2: Add 3pool event + ICRC-3 wrappers to sources/three_pool.rs**

Add to `sources/three_pool.rs`:

```rust
// --- Event tailing (Phase 4) ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolSwapEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub token_in: u8,
    pub token_out: u8,
    pub amount_in: candid::Nat,
    pub amount_out: candid::Nat,
    pub fee: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ThreePoolLiquidityAction {
    AddLiquidity,
    RemoveLiquidity,
    RemoveOneCoin,
    Donate,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ThreePoolLiquidityEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: ThreePoolLiquidityAction,
    pub amounts: Vec<candid::Nat>,
    pub lp_amount: candid::Nat,
    pub coin_index: Option<u8>,
    pub fee: Option<candid::Nat>,
}

pub async fn get_swap_events(
    three_pool: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<ThreePoolSwapEvent>, String> {
    let (events,): (Vec<ThreePoolSwapEvent>,) =
        ic_cdk::call(three_pool, "get_swap_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_swap_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_swap_event_count(three_pool: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(three_pool, "get_swap_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_swap_event_count: {:?} {}", code, msg))?;
    Ok(count)
}

pub async fn get_liquidity_events(
    three_pool: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<ThreePoolLiquidityEvent>, String> {
    let (events,): (Vec<ThreePoolLiquidityEvent>,) =
        ic_cdk::call(three_pool, "get_liquidity_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_liquidity_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_liquidity_event_count(three_pool: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(three_pool, "get_liquidity_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_liquidity_event_count: {:?} {}", code, msg))?;
    Ok(count)
}
```

- [ ] **Step 3: Add ICRC-3 block fetching to sources/icusd_ledger.rs**

Add to `sources/icusd_ledger.rs`:

```rust
// --- ICRC-3 block tailing (Phase 4) ---

use candid::Nat;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBlocksArg {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BlockWithId {
    pub id: Nat,
    pub block: icrc_ledger_types::icrc3::blocks::ICRC3Value,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBlocksResult {
    pub log_length: Nat,
    pub blocks: Vec<BlockWithId>,
    // archived_blocks omitted for now; added when archive following is implemented
}

pub async fn icrc3_get_blocks(
    ledger: Principal,
    start: u64,
    length: u64,
) -> Result<GetBlocksResult, String> {
    let args = vec![GetBlocksArg {
        start: Nat::from(start),
        length: Nat::from(length),
    }];
    let (result,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args,))
        .await
        .map_err(|(code, msg)| format!("icrc3_get_blocks: {:?} {}", code, msg))?;
    Ok(result)
}
```

**Note for implementer:** Check if `icrc_ledger_types::icrc3::blocks::ICRC3Value` is available in the `icrc-ledger-types` crate at the pinned revision (fc278709). If not, define a local ICRC3Value enum:

```rust
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ICRC3Value {
    Blob(Vec<u8>),
    Text(String),
    Nat(Nat),
    Int(candid::Int),
    Array(Vec<ICRC3Value>),
    Map(Vec<(String, ICRC3Value)>),
}
```

- [ ] **Step 4: Create sources/amm.rs**

```rust
//! Source wrapper for rumi_amm event queries.

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct AmmSwapEvent {
    pub id: u64,
    pub caller: Principal,
    pub pool_id: String,
    pub token_in: Principal,
    pub amount_in: Nat,
    pub token_out: Principal,
    pub amount_out: Nat,
    pub fee: Nat,
    pub timestamp: u64,
}

pub async fn get_amm_swap_events(
    amm: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<AmmSwapEvent>, String> {
    let (events,): (Vec<AmmSwapEvent>,) =
        ic_cdk::call(amm, "get_amm_swap_events", (start, length))
            .await
            .map_err(|(code, msg)| format!("get_amm_swap_events: {:?} {}", code, msg))?;
    Ok(events)
}

pub async fn get_amm_swap_event_count(amm: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(amm, "get_amm_swap_event_count", ())
        .await
        .map_err(|(code, msg)| format!("get_amm_swap_event_count: {:?} {}", code, msg))?;
    Ok(count)
}
```

- [ ] **Step 5: Add `pub mod amm;` to sources/mod.rs**

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`
Expected: No errors.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_analytics/src/sources/
git commit -m "feat(analytics): add source wrappers for event tailing (backend events, 3pool, AMM, ICRC-3)"
```

---

### Task 7: Tailing module - custom event sources

**Files:**
- Create: `src/rumi_analytics/src/tailing/mod.rs`
- Create: `src/rumi_analytics/src/tailing/backend_events.rs`
- Create: `src/rumi_analytics/src/tailing/three_pool_swaps.rs`
- Create: `src/rumi_analytics/src/tailing/three_pool_liquidity.rs`
- Create: `src/rumi_analytics/src/tailing/amm_swaps.rs`
- Modify: `src/rumi_analytics/src/lib.rs` -- Add `mod tailing;`

**Context:** Each tailing function follows the cursor protocol: read cursor, check count, fetch batch, process, advance cursor. Backend events are routed to EVT_LIQUIDATIONS or EVT_VAULTS. 3pool swaps and AMM swaps go to EVT_SWAPS. 3pool liquidity goes to EVT_LIQUIDITY.

The BATCH_SIZE is 500. Each function updates SlimState cursor metadata on success/failure.

- [ ] **Step 1: Create tailing/mod.rs**

```rust
//! Event tailing functions. Each module implements a single cursor's
//! fetch-process-advance cycle.

pub mod backend_events;
pub mod three_pool_swaps;
pub mod three_pool_liquidity;
pub mod amm_swaps;
pub mod icrc3;

pub const BATCH_SIZE: u64 = 500;
pub const BACKFILL_BATCH_SIZE: u64 = 1000;
```

- [ ] **Step 2: Implement backend_events.rs**

```rust
//! Backend event tailing. Routes events to EVT_LIQUIDATIONS and EVT_VAULTS.

use candid::Principal;
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::BATCH_SIZE;

pub async fn run() {
    let backend = state::read_state(|s| s.sources.backend);
    let cursor = cursors::backend_events::get();

    let count = match sources::backend::get_event_count(backend).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_backend] get_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.backend += 1;
                update_cursor_error(s, cursors::CURSOR_ID_BACKEND_EVENTS, e.clone());
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_BACKEND_EVENTS, count);
    });

    if count <= cursor {
        return;
    }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::backend::get_events(backend, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_backend] get_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.backend += 1;
                update_cursor_error(s, cursors::CURSOR_ID_BACKEND_EVENTS, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for (i, event) in events.iter().enumerate() {
        let event_id = cursor + i as u64;
        route_backend_event(event_id, event);
        processed += 1;
    }

    if processed > 0 {
        cursors::backend_events::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_BACKEND_EVENTS, ic_cdk::api::time());
        });
    }
}

fn route_backend_event(event_id: u64, event: &sources::backend::BackendEvent) {
    use sources::backend::BackendEvent::*;

    match event {
        // --- EVT_LIQUIDATIONS ---
        LiquidateVault { vault_id, timestamp, .. } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(), // not available in this variant
                collateral_amount: 0,
                debt_amount: 0,
                liquidation_kind: LiquidationKind::Full,
            });
        }
        PartialLiquidateVault { vault_id, liquidator_payment, icp_to_liquidator, timestamp, .. } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(),
                collateral_amount: *icp_to_liquidator,
                debt_amount: *liquidator_payment,
                liquidation_kind: LiquidationKind::Partial,
            });
        }
        RedistributeVault { vault_id, timestamp } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(),
                collateral_amount: 0,
                debt_amount: 0,
                liquidation_kind: LiquidationKind::Redistribution,
            });
        }

        // --- EVT_VAULTS ---
        OpenVault { vault, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: vault.vault_id,
                owner: vault.owner,
                event_kind: VaultEventKind::Opened,
                collateral_type: vault.collateral_type,
                amount: vault.collateral_amount,
            });
        }
        BorrowFromVault { vault_id, borrowed_amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::Borrowed,
                collateral_type: Principal::anonymous(),
                amount: *borrowed_amount,
            });
        }
        RepayToVault { vault_id, repayed_amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::Repaid,
                collateral_type: Principal::anonymous(),
                amount: *repayed_amount,
            });
        }
        CollateralWithdrawn { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::CollateralWithdrawn,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        PartialCollateralWithdrawn { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::PartialCollateralWithdrawn,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        WithdrawAndCloseVault { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::WithdrawAndClose,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        VaultWithdrawnAndClosed { vault_id, timestamp, caller, amount } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: *timestamp,
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: *caller,
                event_kind: VaultEventKind::Closed,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        DustForgiven { vault_id, amount, timestamp } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: Principal::anonymous(),
                event_kind: VaultEventKind::DustForgiven,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        RedemptionOnVaults { icusd_amount, owner, collateral_type, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: 0, // redemptions affect multiple vaults
                owner: *owner,
                event_kind: VaultEventKind::Redeemed,
                collateral_type: collateral_type.unwrap_or(Principal::anonymous()),
                amount: *icusd_amount,
            });
        }

        // All other variants (admin, config, etc.) - skip
        Unknown => {}
    }
}

// --- Cursor metadata helpers ---
// These update the Option<HashMap> fields in SlimState.

fn update_cursor_success(s: &mut storage::SlimState, cursor_id: u8, timestamp_ns: u64) {
    let map = s.cursor_last_success.get_or_insert_with(Default::default);
    map.insert(cursor_id, timestamp_ns);
    // Clear error on success
    if let Some(err_map) = &mut s.cursor_last_error {
        err_map.remove(&cursor_id);
    }
}

fn update_cursor_error(s: &mut storage::SlimState, cursor_id: u8, error: String) {
    let map = s.cursor_last_error.get_or_insert_with(Default::default);
    map.insert(cursor_id, error);
}

fn update_cursor_source_count(s: &mut storage::SlimState, cursor_id: u8, count: u64) {
    let map = s.cursor_source_counts.get_or_insert_with(Default::default);
    map.insert(cursor_id, count);
}
```

- [ ] **Step 3: Implement three_pool_swaps.rs, three_pool_liquidity.rs, amm_swaps.rs**

These follow the same cursor pattern. Each reads its cursor, calls its source, normalizes events to the analytics type, pushes to the appropriate EVT_* log.

**three_pool_swaps.rs:** Converts 3pool token indices (0/1/2) to Principals. Hardcode the 3pool token ordering: index 0 = icUSD (t6bor-paaaa-aaaap-qrd5q-cai), index 1 = ckUSDT, index 2 = ckUSDC. Store a `THREE_POOL_TOKENS: [Principal; 3]` constant. Convert `Nat` amounts to `u64` via `nat_to_u64` helper (truncate, log warning on overflow).

**amm_swaps.rs:** AMM events already have Principal-typed token_in/token_out, so no index resolution needed. Convert `Nat` amounts to `u64`.

**three_pool_liquidity.rs:** Convert `LiquidityAction` from 3pool's variant to our `LiquidityAction`. Convert `Nat` amounts.

Each file is ~80-100 lines following the same pattern as backend_events.rs. The implementer should reference backend_events.rs and adapt.

- [ ] **Step 4: Add `mod tailing;` to lib.rs**

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`
Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_analytics/src/tailing/ src/rumi_analytics/src/lib.rs
git commit -m "feat(analytics): add event tailing functions for backend, 3pool, and AMM sources"
```

---

### Task 8: ICRC-3 block tailing + BalanceTracker integration

**Files:**
- Create: `src/rumi_analytics/src/tailing/icrc3.rs`

**Context:** This is the most complex tailing function. It fetches ICRC-3 blocks, parses the generic `ICRC3Value` tree to extract transfer/mint/burn operations, and applies balance deltas to the BalanceTracker. Shared by both icusd_ledger and 3pool block tailing.

The ICRC-3 block structure is a `Map` containing a `tx` field which is itself a `Map` with fields like `op` (operation type), `from`, `to`, `amt`, `fee`. The block-level `ts` field is the timestamp.

- [ ] **Step 1: Write unit tests for ICRC-3 block parsing**

Test the parser with hand-constructed ICRC3Value trees for transfer, mint, burn, and approve operations.

- [ ] **Step 2: Implement icrc3.rs**

```rust
//! ICRC-3 block tailing. Fetches blocks from icusd_ledger or 3pool,
//! parses transfer/mint/burn operations, applies to BalanceTracker.

use candid::{Nat, Principal};
use crate::{sources, state, storage};
use storage::balance_tracker::{self, Account, Token};
use storage::cursors;
use super::{BATCH_SIZE, BACKFILL_BATCH_SIZE};

/// Parse ICRC-3 blocks and apply balance deltas for icUSD.
pub async fn tail_icusd_blocks() {
    let ledger = state::read_state(|s| s.sources.icusd_ledger);
    let is_backfill = state::read_state(|s| s.backfill_active_icusd.unwrap_or(false));
    let batch = if is_backfill { BACKFILL_BATCH_SIZE } else { BATCH_SIZE };

    tail_blocks(
        ledger,
        Token::IcUsd,
        cursors::CURSOR_ID_ICUSD_BLOCKS,
        || cursors::icusd_blocks::get(),
        |v| cursors::icusd_blocks::set(v),
        batch,
        is_backfill,
    ).await;
}

/// Parse ICRC-3 blocks and apply balance deltas for 3USD.
pub async fn tail_3pool_blocks() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let is_backfill = state::read_state(|s| s.backfill_active_3usd.unwrap_or(false));
    let batch = if is_backfill { BACKFILL_BATCH_SIZE } else { BATCH_SIZE };

    tail_blocks(
        three_pool,
        Token::ThreeUsd,
        cursors::CURSOR_ID_3POOL_BLOCKS,
        || cursors::three_pool_blocks::get(),
        |v| cursors::three_pool_blocks::set(v),
        batch,
        is_backfill,
    ).await;
}

async fn tail_blocks<G, S>(
    canister: Principal,
    token: Token,
    cursor_id: u8,
    get_cursor: G,
    set_cursor: S,
    batch_size: u64,
    is_backfill: bool,
) where
    G: Fn() -> u64,
    S: Fn(u64),
{
    let cursor = get_cursor();

    let result = match sources::icusd_ledger::icrc3_get_blocks(canister, cursor, batch_size).await {
        Ok(r) => r,
        Err(e) => {
            ic_cdk::println!("[tail_icrc3] icrc3_get_blocks failed for {:?}: {}", token, e);
            state::mutate_state(|s| {
                match token {
                    Token::IcUsd => s.error_counters.icusd_ledger += 1,
                    Token::ThreeUsd => s.error_counters.three_pool += 1,
                }
            });
            return;
        }
    };

    let log_length = nat_to_u64(&result.log_length);

    if result.blocks.is_empty() {
        // Check if backfill is done
        if is_backfill && cursor >= log_length {
            clear_backfill_flag(token);
        }
        return;
    }

    let mut processed = 0u64;
    for block_with_id in &result.blocks {
        if let Err(e) = process_block(token, &block_with_id.block) {
            ic_cdk::println!("[tail_icrc3] skipping malformed block: {}", e);
        }
        processed += 1;
    }

    if processed > 0 {
        set_cursor(cursor + processed);
        state::mutate_state(|s| {
            let map = s.cursor_last_success.get_or_insert_with(Default::default);
            map.insert(cursor_id, ic_cdk::api::time());
            let cmap = s.cursor_source_counts.get_or_insert_with(Default::default);
            cmap.insert(cursor_id, log_length);
        });
    }

    // Auto-clear backfill when caught up
    if is_backfill && (cursor + processed) >= log_length {
        clear_backfill_flag(token);
    }
}

fn clear_backfill_flag(token: Token) {
    state::mutate_state(|s| {
        match token {
            Token::IcUsd => s.backfill_active_icusd = Some(false),
            Token::ThreeUsd => s.backfill_active_3usd = Some(false),
        }
    });
    ic_cdk::println!("[tail_icrc3] backfill complete for {:?}", token);
}

/// Parse a single ICRC-3 block and apply balance changes.
fn process_block(token: Token, block: &sources::icusd_ledger::ICRC3Value) -> Result<(), String> {
    // ICRC-3 blocks are Map values. Extract the `tx` field.
    // Block structure: Map { "ts": Nat, "tx": Map { ... }, "btype": Text, ... }
    // The implementer should extract fields from the ICRC3Value tree.
    // This is a sketch; actual extraction depends on the ICRC3Value variant structure.

    // Extract timestamp from block-level "ts" field
    let timestamp_ns = extract_nat_field(block, "ts").unwrap_or(0);

    // Extract the "tx" map
    let tx = extract_map_field(block, "tx")
        .ok_or_else(|| "missing tx field".to_string())?;

    // Determine operation type from "btype" or from tx structure
    let btype = extract_text_field(block, "btype").unwrap_or_default();

    match btype.as_str() {
        "1xfer" => {
            let from = extract_account_field(&tx, "from")
                .ok_or_else(|| "1xfer missing from".to_string())?;
            let to = extract_account_field(&tx, "to")
                .ok_or_else(|| "1xfer missing to".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1xfer missing amt".to_string())?;
            let fee = extract_nat_field(&tx, "fee").unwrap_or(0);
            balance_tracker::apply_transfer(token, &from, &to, amt, fee, timestamp_ns);
        }
        "1mint" => {
            let to = extract_account_field(&tx, "to")
                .ok_or_else(|| "1mint missing to".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1mint missing amt".to_string())?;
            balance_tracker::apply_mint(token, &to, amt, timestamp_ns);
        }
        "1burn" => {
            let from = extract_account_field(&tx, "from")
                .ok_or_else(|| "1burn missing from".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1burn missing amt".to_string())?;
            balance_tracker::apply_burn(token, &from, amt);
        }
        "2approve" => {
            // No balance change
        }
        other => {
            // Unknown block type, skip silently
            ic_cdk::println!("[tail_icrc3] unknown btype: {}", other);
        }
    }

    Ok(())
}

// --- ICRC3Value extraction helpers ---
// These extract typed fields from the ICRC3Value::Map variant.
// The exact implementation depends on the ICRC3Value enum definition.
// The implementer must adapt these to the actual ICRC3Value type used
// (either from icrc-ledger-types or locally defined).

use sources::icusd_ledger::ICRC3Value;

fn extract_map_field(value: &ICRC3Value, field: &str) -> Option<ICRC3Value> {
    match value {
        ICRC3Value::Map(entries) => entries
            .iter()
            .find(|(k, _)| k == field)
            .map(|(_, v)| v.clone()),
        _ => None,
    }
}

fn extract_nat_field(value: &ICRC3Value, field: &str) -> Option<u64> {
    match extract_map_field(value, field)? {
        ICRC3Value::Nat(n) => Some(nat_to_u64(&n)),
        _ => None,
    }
}

fn extract_text_field(value: &ICRC3Value, field: &str) -> Option<String> {
    match extract_map_field(value, field)? {
        ICRC3Value::Text(s) => Some(s),
        _ => None,
    }
}

fn extract_account_field(value: &ICRC3Value, field: &str) -> Option<Account> {
    // Account in ICRC-3 blocks is encoded as a Map with "owner" (Blob) and optional "subaccount" (Blob),
    // or as an Array [owner_blob] / [owner_blob, subaccount_blob].
    let acct_val = extract_map_field(value, field)?;
    match &acct_val {
        ICRC3Value::Map(entries) => {
            let owner_blob = entries.iter().find(|(k, _)| k == "owner")
                .and_then(|(_, v)| match v { ICRC3Value::Blob(b) => Some(b.clone()), _ => None })?;
            let owner = Principal::from_slice(&owner_blob);
            let subaccount = entries.iter().find(|(k, _)| k == "subaccount")
                .and_then(|(_, v)| match v { ICRC3Value::Blob(b) => {
                    let mut sa = [0u8; 32];
                    if b.len() == 32 { sa.copy_from_slice(b); Some(sa) } else { None }
                }, _ => None });
            Some(Account { owner, subaccount })
        }
        ICRC3Value::Array(arr) => {
            let owner_blob = match arr.first()? { ICRC3Value::Blob(b) => b, _ => return None };
            let owner = Principal::from_slice(owner_blob);
            let subaccount = arr.get(1).and_then(|v| match v { ICRC3Value::Blob(b) => {
                let mut sa = [0u8; 32]; if b.len() == 32 { sa.copy_from_slice(b); Some(sa) } else { None }
            }, _ => None });
            Some(Account { owner, subaccount })
        }
        _ => None,
    }
}

fn nat_to_u64(nat: &candid::Nat) -> u64 {
    // candid::Nat wraps num_bigint::BigUint. Convert to u64, clamping to MAX.
    use num_traits::ToPrimitive;
    nat.0.to_u64().unwrap_or(u64::MAX)
}
```

**Important note for the implementer:** The ICRC3Value extraction helpers above assume a locally-defined ICRC3Value enum with variants `Map(Vec<(String, ICRC3Value)>)`, `Nat(candid::Nat)`, `Text(String)`, `Blob(Vec<u8>)`, `Array(Vec<ICRC3Value>)`. If using `icrc-ledger-types`, check the actual enum variant names and adapt accordingly. The `num_traits` crate must be in Cargo.toml for `ToPrimitive` in `nat_to_u64`.

- [ ] **Step 3: Run unit tests for block parsing**

Run: `cargo test -p rumi_analytics --lib icrc3 2>&1 | tail -10`

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/tailing/icrc3.rs
git commit -m "feat(analytics): add ICRC-3 block tailing with BalanceTracker integration"
```

---

### Task 9: Wire tailing into pull cycle + add holders to daily snapshot

**Files:**
- Modify: `src/rumi_analytics/src/timers.rs`

**Context:** Expand `pull_cycle()` to call all tailing functions sequentially. Add `collectors::holders::run()` to `daily_snapshot()`.

- [ ] **Step 1: Update timers.rs**

Replace `pull_cycle`:

```rust
async fn pull_cycle() {
    refresh_supply_cache().await;

    // Event tailing (Phase 4)
    tailing::backend_events::run().await;
    tailing::three_pool_swaps::run().await;
    tailing::three_pool_liquidity::run().await;
    tailing::amm_swaps::run().await;

    // ICRC-3 block tailing (Phase 4)
    tailing::icrc3::tail_icusd_blocks().await;
    tailing::icrc3::tail_3pool_blocks().await;

    // Update pull cycle timestamp
    state::mutate_state(|s| {
        s.last_pull_cycle_ns = Some(ic_cdk::api::time());
    });
}
```

Add the `tailing` import at the top of timers.rs:
```rust
use crate::{collectors, sources, state, tailing};
```

Update `daily_snapshot` to include holders:

```rust
async fn daily_snapshot() {
    let (tvl_res, vaults_res, stability_res, holders_res) = futures::join!(
        collectors::tvl::run(),
        collectors::vaults::run(),
        collectors::stability::run(),
        collectors::holders::run(),
    );

    if let Err(e) = tvl_res {
        ic_cdk::println!("rumi_analytics: daily TVL snapshot failed: {}", e);
    }
    if let Err(e) = vaults_res {
        ic_cdk::println!("rumi_analytics: daily vault snapshot failed: {}", e);
    }
    if let Err(e) = stability_res {
        ic_cdk::println!("rumi_analytics: daily stability snapshot failed: {}", e);
    }
    if let Err(e) = holders_res {
        ic_cdk::println!("rumi_analytics: daily holder snapshot failed: {}", e);
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`

- [ ] **Step 3: Commit**

```bash
git add src/rumi_analytics/src/timers.rs
git commit -m "feat(analytics): wire event tailing into pull cycle + holders into daily snapshot"
```

---

### Task 10: Query endpoints + response types + get_collector_health + start_backfill

**Files:**
- Modify: `src/rumi_analytics/src/types.rs`
- Modify: `src/rumi_analytics/src/queries/historical.rs`
- Modify: `src/rumi_analytics/src/lib.rs`

**Context:** Add `HolderSeriesResponse`, `CollectorHealth`, `CursorStatus`, `BalanceTrackerStats` to types.rs. Add `get_holder_series` to queries. Add `get_collector_health` and `start_backfill` entry points to lib.rs.

- [ ] **Step 1: Add types to types.rs**

```rust
use crate::storage::holders::DailyHolderRow;

#[derive(CandidType, Clone, Debug)]
pub struct HolderSeriesResponse {
    pub rows: Vec<DailyHolderRow>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct CollectorHealth {
    pub cursors: Vec<CursorStatus>,
    pub error_counters: crate::storage::ErrorCounters,
    pub backfill_active: Vec<Principal>,
    pub last_pull_cycle_ns: u64,
    pub balance_tracker_stats: Vec<BalanceTrackerStats>,
}

#[derive(CandidType, Clone, Debug)]
pub struct CursorStatus {
    pub name: String,
    pub cursor_position: u64,
    pub source_count: u64,
    pub last_success_ns: u64,
    pub last_error: Option<String>,
}

#[derive(CandidType, Clone, Debug)]
pub struct BalanceTrackerStats {
    pub token: Principal,
    pub holder_count: u64,
    pub total_tracked_e8s: u64,
}
```

- [ ] **Step 2: Add get_holder_series to queries/historical.rs**

```rust
pub fn get_holder_series(query: RangeQuery, token: Principal) -> HolderSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    // Determine which log to read based on token principal.
    let icusd_ledger = crate::state::read_state(|s| s.sources.icusd_ledger);
    let rows = if token == icusd_ledger {
        storage::holders::daily_holders_icusd::range(from, to, limit)
    } else {
        storage::holders::daily_holders_3usd::range(from, to, limit)
    };

    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    HolderSeriesResponse { rows, next_from_ts }
}
```

- [ ] **Step 3: Add entry points to lib.rs**

Add `get_holder_series`, `get_collector_health`, and `start_backfill` as canister methods.

`get_collector_health` reads cursor positions from StableCells and metadata from SlimState.

`start_backfill` is an update call gated to admin that sets `backfill_active_icusd` or `backfill_active_3usd` in SlimState.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rumi_analytics 2>&1 | grep -E "^error"`

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/types.rs src/rumi_analytics/src/queries/historical.rs src/rumi_analytics/src/lib.rs
git commit -m "feat(analytics): add holder series query, collector health, and backfill endpoints"
```

---

### Task 11: Update Candid interface

**Files:**
- Modify: `src/rumi_analytics/rumi_analytics.did`

**Context:** Add all new types and three new methods. The canister uses `ic_cdk::export_candid!()` to auto-generate the .did, so we may be able to just rebuild and copy the output. If not, manually add the types.

- [ ] **Step 1: Build the canister to regenerate .did**

Run: `cargo build -p rumi_analytics --target wasm32-unknown-unknown --release 2>&1 | tail -5`

If `export_candid!()` generates the .did automatically, copy it from the build output.

- [ ] **Step 2: Verify the .did contains the new types and methods**

Check that the .did file includes: `CollectorHealth`, `CursorStatus`, `BalanceTrackerStats`, `DailyHolderRow`, `HolderSeriesResponse`, `LiquidationKind`, `VaultEventKind`, `SwapSource`, `LiquidityAction`, and the 4 Analytics*Event types. Check for the 3 new methods: `get_holder_series`, `get_collector_health`, `start_backfill`.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_analytics/rumi_analytics.did
git commit -m "feat(analytics): update candid interface with Phase 4 types and endpoints"
```

---

### Task 12: PocketIC integration tests

**Files:**
- Modify: `src/rumi_analytics/tests/pocket_ic_analytics.rs`

**Context:** Add tests for event tailing, ICRC-3 balance tracking, holder snapshots, collector health, backfill, error resilience, and upgrade preservation. These tests deploy the analytics canister alongside real backend/stability_pool/3pool/icusd_ledger wasm binaries in PocketIC, execute operations, advance time, and verify the analytics canister captured the expected data.

The existing tests use `POCKET_IC_BIN` env var for the PocketIC binary path.

- [ ] **Step 1: Add test-side type definitions**

Add Candid types for `CollectorHealth`, `CursorStatus`, `BalanceTrackerStats`, `DailyHolderRow`, `HolderSeriesResponse` to the test file for query decoding.

- [ ] **Step 2: Add test helper functions**

Add `query_holder_series`, `query_collector_health`, `call_start_backfill` helpers.

- [ ] **Step 3: Implement integration tests**

Tests to add (from the spec):

1. **collector_health_reports_cursor_positions**: Deploy analytics, advance past a pull cycle, query collector_health, verify cursor positions and last_success timestamps are non-zero.

2. **icrc3_balance_tracking_mint_and_transfer**: Deploy analytics + icusd_ledger. Mint icUSD to account A. Advance past pull cycles. Verify BalanceTracker via collector_health stats (holder_count, total_tracked). Transfer from A to B. Advance. Verify both accounts tracked.

3. **daily_holder_snapshot_computed**: After balance tracking is populated, advance past daily tick. Query get_holder_series. Verify total_holders, top_50 length, distribution_buckets.

4. **backfill_processes_historical_blocks**: Pre-populate icusd_ledger with mints (done during setup). Deploy analytics. Call start_backfill. Advance through multiple 60s ticks. Verify BalanceTracker matches expected state. Verify backfill auto-cleared via collector_health.

5. **error_resilience_partial_tailing**: Deploy analytics with an unreachable AMM canister. Advance past pull cycles. Verify other cursors still advanced (backend events etc.). Verify AMM error counter incremented.

6. **upgrade_preserves_cursors_and_balances**: Populate cursors and BalanceTracker, upgrade canister, verify cursor positions and holder counts survive.

- [ ] **Step 4: Run integration tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_analytics --test pocket_ic_analytics 2>&1 | tail -10`
Expected: All tests pass (old and new).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/tests/pocket_ic_analytics.rs
git commit -m "test(analytics): add Phase 4 integration tests for event tailing, balance tracking, and holder snapshots"
```

---

### Task 13: Final review and test run

**Files:** None (review only)

- [ ] **Step 1: Run all unit tests**

Run: `cargo test -p rumi_analytics --lib 2>&1 | tail -10`
Expected: All pass.

- [ ] **Step 2: Run all integration tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_analytics 2>&1 | tail -10`
Expected: All pass (unit + integration).

- [ ] **Step 3: Build for mainnet**

Run: `dfx build rumi_analytics --network ic 2>&1 | tail -5`
Expected: Build succeeds.

- [ ] **Step 4: Review all changed files**

Review `git diff main..HEAD --stat` and spot-check key files for correctness.

---

### Task 14: Deploy to mainnet

**Files:** None

- [ ] **Step 1: Deploy**

```bash
dfx canister install rumi_analytics --network ic --mode upgrade --argument '(record { admin = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"; backend = principal "tfesu-vyaaa-aaaap-qrd7a-cai"; icusd_ledger = principal "t6bor-paaaa-aaaap-qrd5q-cai"; three_pool = principal "fohh4-yyaaa-aaaap-qtkpa-cai"; stability_pool = principal "tmhzi-dqaaa-aaaap-qrd6q-cai"; amm = principal "ijlzs-2yaaa-aaaap-quaaq-cai" })'
```

- [ ] **Step 2: Verify new endpoints**

```bash
dfx canister call rumi_analytics get_collector_health --network ic
dfx canister call rumi_analytics ping --network ic
```

- [ ] **Step 3: Start backfill for icUSD and 3USD**

```bash
dfx canister call rumi_analytics start_backfill '(principal "t6bor-paaaa-aaaap-qrd5q-cai")' --network ic
dfx canister call rumi_analytics start_backfill '(principal "fohh4-yyaaa-aaaap-qtkpa-cai")' --network ic
```

- [ ] **Step 4: Monitor backfill progress**

Wait a few minutes, then check:
```bash
dfx canister call rumi_analytics get_collector_health --network ic
```

Verify cursor positions are advancing and backfill flags clear when caught up.
