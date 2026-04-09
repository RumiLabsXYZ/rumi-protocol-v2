# Liquidation Bot ICPSwap Rework + Full History Implementation Plan (v3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dead KongSwap + 3pool liquidation path with a single ICPSwap ICP->ckUSDC swap that delivers ckUSDC directly into the backend canister's own ckUSDC ledger account, and add a full append-only per-liquidation history stored in the bot's stable memory.

**Architecture:**
- Bot swaps seized ICP -> ckUSDC on ICPSwap pool `mohjv-bqaaa-aaaag-qjyia-cai` using `depositFromAndSwap` + an infinite ICRC-2 approve, mirroring `rumi-arb-bot/src/arb_bot/src/swaps.rs`.
- Bot transfers swap proceeds to the backend via a plain `icrc1_transfer` on the ckUSDC ledger. The backend already holds real ckUSDC balances from the existing `repay_to_vault_with_stable` feature (`vault.rs:888`); the new flow is structurally identical to a user doing a stable-repay, so the backend needs zero new deposit-verification logic.
- `bot_claim_liquidation` and `bot_confirm_liquidation` are unchanged. Confirm already does pure state bookkeeping (no icUSD burn, no ledger transfer) and partial-claim semantics via `compute_partial_liquidation_cap` are preserved.
- The cosmetic `bot_deposit_to_reserves` counter endpoint and the `bot_total_icusd_deposited_e8s` state field are deleted as dead code.
- Bot migrates its state store from the current raw `stable64_write(0, ...)` JSON-blob pattern to `ic-stable-structures` with `MemoryManager`, for an append-only `StableBTreeMap<u64, LiquidationRecordVersioned>` that is upgrade-safe and effectively unbounded. The migration path is handled with an explicit legacy-blob rescue in `post_upgrade` before any `MemoryManager` access.

**Failure Recovery Model:**
- **Swap failure (before money changes form):** Bot still holds ICP. Return ICP to backend via `icrc1_transfer`, call `bot_cancel_liquidation`. Clean recovery, no risk.
- **Transfer failure (swap succeeded, ckUSDC transfer to backend failed):** Bot holds ckUSDC, vault is locked. **No retry.** Immediately mark the claim as "stuck" in bot history. Admin resolves manually via `admin_sweep_ckusdc` (bot) and `admin_resolve_stuck_claim` (backend). This avoids the double-transfer bugs that retries caused previously.
- **Confirm failure (ckUSDC delivered to backend, confirm call failed):** Idempotent (safe to repeat). Retry up to 5 times immediately. If all retries fail, mark as stuck. The backend's confirm is idempotent: it looks up the claim by vault_id, writes down debt, deletes claim. A second call with the same vault_id gets "no active claim" and fails harmlessly.

**Tech Stack:** Rust, ic-cdk 0.12, ic-stable-structures 0.6.5, candid 0.10.6, icrc-ledger-types, pocket-ic 6.0 for integration tests.

---

## Key audit findings that shaped this plan

Performed before writing v2; see chat log 2026-04-08 for citations.

1. **`bot_confirm_liquidation` does NOT burn icUSD today.** It only subtracts `claim.debt_amount` from `vault.borrowed_icusd_amount` and `claim.collateral_amount` from `vault.collateral_amount` in state (`main.rs:1694-1695`). No `icrc1_transfer`, no burn. This code is already correct for the new reserve-backed model and does not need to change.
2. **`bot_deposit_to_reserves` is a cosmetic counter.** It only increments `bot_total_icusd_deposited_e8s` (`main.rs:2080`). No ledger interaction. Dead code, to be deleted.
3. **Claims are PARTIAL by default** via `compute_partial_liquidation_cap` at `state.rs:2275`, called from `main.rs:1610`. Partial claims stay partial in this rework. Only the minimum debt needed to restore vault health is liquidated. Do not rewrite confirm to use vault-zero semantics.
4. **`compute_total_collateral_ratio` at `state.rs:1141` does not count ckUSDC/ckUSDT reserves.** This is a pre-existing bug relevant to the shipped `repay_to_vault_with_stable` feature as well. Per Rob: fix in a **separate follow-up PR**, not this one. This PR inherits the same accounting quirk that stable-repay already has.
5. **Backend ckUSDC ledger account already holds real funds** via stable-repay (`vault.rs:980` calls `transfer_stable_from` -> `management.rs:613` ICRC-2 transfer_from into `protocol_account` subaccount None). No new reserve storage primitive needed.
6. **Claims are physical ICRC-1 transfers.** `bot_claim_liquidation` (`main.rs:1628-1639`) calls `management::transfer_collateral` which does a real ICRC-1 transfer of ICP from the backend canister to the bot canister. The ICP physically moves.
7. **ICPSwap `quote` and `depositFromAndSwap` take DIFFERENT arg types.** `quote` takes `SwapArgs` (3 fields: `amountIn`, `amountOutMinimum`, `zeroForOne`). `depositFromAndSwap` takes `DepositAndSwapArgs` (5 fields: adds `tokenInFee`, `tokenOutFee`). Plan v2 had this wrong; v3 uses correct types for each.
8. **ICPSwap `metadata()` returns `Result_6` (wrapped `PoolMetadata`), not raw `PoolMetadata`.** Must unwrap the result variant. `PoolMetadata` has 9 fields (fee, key, liquidity, maxLiquidityPerTick, nextPositionId, sqrtPriceX96, tick, token0, token1).

---

## Scope

**In scope (this PR)**
- KongSwap + 3pool code removal from `liquidation_bot`
- ICPSwap single-hop ICP->ckUSDC swap with quote + slippage + infinite approve
- Bot -> backend ckUSDC delivery via plain `icrc1_transfer` (no new backend endpoint)
- **Delete** `bot_deposit_to_reserves` endpoint and `bot_total_icusd_deposited_e8s` state field on backend
- Bot migration to `ic-stable-structures` with `MemoryManager`, **including** a safe `post_upgrade` rescue of the legacy JSON blob at offset 0
- `LiquidationRecordVersioned::V1` struct, stable map, next-id cell
- Bot query endpoints: `get_liquidation`, `get_liquidations`, `get_liquidation_count`, `get_stuck_liquidations`
- Bot admin endpoints: `admin_resolve_pool_ordering` (one-time call to cache `zeroForOne`), `admin_approve_pool` (one-time infinite ICRC-2 approve for ICP to ICPSwap pool), `admin_sweep_ckusdc` (emergency: transfer all bot ckUSDC to a target, optionally mark associated record as AdminResolved), `admin_retry_stuck_claim` (re-attempt confirm for a stuck claim)
- Backend admin endpoint: `admin_resolve_stuck_claim` (force-clear a stuck bot claim, unlock vault, restore budget)
- Candid updates for both canisters + declaration regen
- PocketIC integration test with a stub ICPSwap pool canister
- Docs: mainnet migration runbook

**Out of scope (deferred)**
- **CR math counting ckUSDC/ckUSDT reserves** (separate PR, fixes pre-existing stable-repay bug simultaneously)
- Treasury split (liquidation bonus -> treasury canister)
- Multi-collateral bot support
- Analytics canister puller
- Alerting/notification system for stuck claims
- Any change to `bot_claim_liquidation` / `bot_confirm_liquidation` / `bot_cancel_liquidation` / `compute_partial_liquidation_cap`
- Any change to icUSD ledger supply handling

---

## File Structure

### Files created

- `src/liquidation_bot/src/memory.rs` -- `MemoryManager` setup, `MemoryId` allocation, helpers for legacy-blob rescue
- `src/liquidation_bot/src/history.rs` -- `LiquidationRecordVersioned`, `LiquidationRecordV1`, `LiquidationStatus`, stable-map accessors, record-writer helpers
- `src/liquidation_bot/src/icpswap.rs` -- ICPSwap `depositFromAndSwap` + `quote` + `metadata` candid types and call wrappers (ported from `rumi-arb-bot/src/arb_bot/src/swaps.rs`)
- `src/test_icpswap_stub/` -- tiny stub pool canister used only in pocket_ic tests
- `docs/liquidation-bot-icpswap-migration.md` -- mainnet rollout runbook

### Files modified

- `src/liquidation_bot/src/state.rs` -- remove `three_pool_principal`, `kong_swap_principal`, `ckusdt_ledger`, `icusd_ledger` from `BotConfig`; add `icpswap_pool: Principal`, `icpswap_zero_for_one: Option<bool>`, `icp_fee_e8s: Option<u64>`, `ckusdc_fee_e6: Option<u64>`. Keep `ckusdc_ledger`. Replace raw `stable64_write`/`stable64_read` with `StableCell` in dedicated `MemoryId`. Legacy `BotLiquidationEvent` and `BotStats` kept for deserialization compatibility during migration, then deprecated.
- `src/liquidation_bot/src/swap.rs` -- delete entire KongSwap + 3pool implementation; replace with `swap_icp_for_ckusdc` (calls ICPSwap `depositFromAndSwap`), `quote_icp_for_ckusdc` (calls ICPSwap `quote` with 3-field `SwapArgs`), `approve_infinite` (one-time `u128::MAX` ICRC-2 approve), `return_collateral_to_backend` (preserved from current code).
- `src/liquidation_bot/src/process.rs` -- rewrite `process_pending`: single swap hop, transfer ckUSDC directly to backend, confirm with retry (5x for confirm only, zero retries for transfer), write `LiquidationRecord` at every phase transition (including failures). Delete all KongSwap/3pool helpers.
- `src/liquidation_bot/src/lib.rs` -- wire `memory`/`history`/`icpswap` modules, add query/admin endpoints, rewrite `init` / `pre_upgrade` / `post_upgrade` for the stable-memory migration. Delete test endpoints that reference KongSwap/3pool (`test_swap_pipeline`, `test_force_liquidate`, `test_force_partial_liquidate` or rewrite them for new flow).
- `src/liquidation_bot/liquidation_bot.did` -- remove KongSwap/3pool fields from `BotConfig`, add ICPSwap pool + fee cache fields, add new history query + admin endpoints, add `LiquidationRecordV1` type.
- `src/rumi_protocol_backend/src/main.rs` -- **delete** `bot_deposit_to_reserves` endpoint (~2069-2084). **Add** `admin_resolve_stuck_claim` endpoint.
- `src/rumi_protocol_backend/src/state.rs` -- **delete** `bot_total_icusd_deposited_e8s` field; keep `serde(default)` on adjacent fields to stay deserialization-compatible.
- `src/rumi_protocol_backend/rumi_protocol_backend.did` -- remove `bot_deposit_to_reserves`, add `admin_resolve_stuck_claim`.
- `src/rumi_protocol_backend/tests/pocket_ic_tests.rs` -- new end-to-end test.
- `src/rumi_protocol_backend/src/api/bot_stats.rs` (or inline in `main.rs:2088-2097`) -- remove `total_icusd_deposited_e8s` field from `BotStatsResponse`.
- Frontend files referencing deleted symbols (audit + update):
  - `src/vault_frontend/src/routes/docs/liquidation-bot/+page.svelte`
  - `src/vault_frontend/src/routes/explorer/+page.svelte`
  - `src/vault_frontend/src/lib/components/liquidations/LiquidationBotTab.svelte`

### Files deleted

None. (Code is removed in place; `swap.rs` stays as a file.)

---

## Cargo dependency changes

- `src/liquidation_bot/Cargo.toml` -- remove `futures = "0.3"` (no longer needed; was only used for parallel KongSwap quotes)
- `ic-stable-structures = "0.6.5"` already present, no change needed.

---

## Task Breakdown

### Task 0: Branch setup
- [ ] Create branch `feat/liquidation-bot-icpswap-rework` from `main`

**Commit:** `chore: create liquidation bot ICPSwap rework branch`

---

### Task 1: Safe stable-memory foundation with legacy-blob rescue

This is the riskiest change and must come first. The current bot uses raw `stable64_write(0, ...)` with an 8-byte length prefix. `MemoryManager::init` will write its own header at offset 0, destroying the legacy blob. We must rescue the blob BEFORE initializing MemoryManager.

**Create `src/liquidation_bot/src/memory.rs`:**

```rust
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{DefaultMemoryImpl, StableCell, StableBTreeMap};
use std::cell::RefCell;

pub type Mem = VirtualMemory<DefaultMemoryImpl>;

pub const MEM_ID_CONFIG: MemoryId = MemoryId::new(0);
pub const MEM_ID_HISTORY: MemoryId = MemoryId::new(1);
pub const MEM_ID_NEXT_ID: MemoryId = MemoryId::new(2);

thread_local! {
    pub static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> =
        RefCell::new(MemoryManager::init(DefaultMemoryImpl::default()));
}

pub fn get_memory(id: MemoryId) -> Mem {
    MEMORY_MANAGER.with(|mm| mm.borrow().get(id))
}
```

**Rescue flow in `post_upgrade` (in `lib.rs`):**

```rust
#[post_upgrade]
fn post_upgrade() {
    // STEP 1: Rescue legacy JSON blob BEFORE MemoryManager::init runs.
    // The thread_local MEMORY_MANAGER hasn't been accessed yet at this point.
    // Read raw stable memory directly.
    let size = ic_cdk::api::stable::stable64_size();
    let legacy_state: Option<BotState> = if size > 0 {
        let mut len_bytes = [0u8; 8];
        ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
        let len = u64::from_le_bytes(len_bytes) as usize;
        if len > 0 && len < 10_000_000 {
            let mut bytes = vec![0u8; len];
            ic_cdk::api::stable::stable64_read(8, &mut bytes);
            serde_json::from_slice(&bytes).ok()
        } else {
            None
        }
    } else {
        None
    };

    // STEP 2: Now initialize MemoryManager. On first migration, this writes a new
    // header at offset 0 (which is fine -- we already rescued the data above).
    // On subsequent upgrades, MemoryManager::init is a read-or-create operation:
    // it detects the existing header and reads it, so this is safe and idempotent.
    memory::init_memory_manager();

    // STEP 3: Write rescued config into the new StableCell, init heap state.
    if let Some(state) = legacy_state {
        // Migrate legacy events to new history map
        history::migrate_legacy_events(&state.liquidation_events);
        state::init_state(state);
    } else {
        // Already migrated on a prior upgrade -- load config from StableCell
        state::load_config_from_stable();
    }

    // STEP 4: Set a flag so subsequent pre_upgrade uses the new path.
    state::mutate_state(|s| s.migrated_to_stable_structures = true);

    setup_timer();
}
```

**Key safety points:**
- `MEMORY_MANAGER` thread_local is lazy. It only initializes when first accessed. We do raw `stable64_read` BEFORE any access to `MEMORY_MANAGER` or any `get_memory()` call.
- We also add `memory::init_memory_manager()` as an explicit function that forces the thread_local to initialize (simply calls `MEMORY_MANAGER.with(|_| {})`).
- The `len < 10_000_000` guard prevents reading garbage if stable memory is in an unexpected state.

**Update `pre_upgrade`:**

```rust
#[pre_upgrade]
fn pre_upgrade() {
    let migrated = state::read_state(|s| s.migrated_to_stable_structures);
    if migrated {
        // New path: save config to StableCell (history is already in StableBTreeMap)
        state::save_config_to_stable();
    } else {
        // Legacy path: first upgrade hasn't happened yet
        state::save_to_stable_memory();
    }
}
```

- [ ] Create `src/liquidation_bot/src/memory.rs` with MemoryManager, MemoryId constants
- [ ] Add `migrated_to_stable_structures: bool` field (with `#[serde(default)]`) to `BotState`
- [ ] Implement rescue logic in `post_upgrade`
- [ ] Implement dual-path `pre_upgrade`
- [ ] Unit test: serialize a BotState to bytes, write to a mock stable memory buffer, verify rescue reads it back correctly

**Commit:** `feat(bot): add stable-structures MemoryManager with legacy blob rescue`

---

### Task 2: Liquidation history type + stable map

**Create `src/liquidation_bot/src/history.rs`:**

```rust
use candid::{CandidType, Deserialize};
use ic_stable_structures::{StableBTreeMap, StableCell, Storable};
use std::borrow::Cow;
use std::cell::RefCell;

use crate::memory;

#[derive(CandidType, Clone, Debug, Deserialize, serde::Serialize)]
pub enum LiquidationStatus {
    /// Swap succeeded, ckUSDC transferred, confirm succeeded. Done.
    Completed,
    /// Swap failed. ICP returned to backend, claim cancelled. No loss.
    SwapFailed,
    /// Swap succeeded but ckUSDC transfer to backend failed.
    /// Bot is holding ckUSDC. Needs manual admin resolution.
    TransferFailed,
    /// Swap + transfer succeeded but confirm failed after retries.
    /// ckUSDC is in backend but vault still shows old debt. Needs admin resolution.
    ConfirmFailed,
    /// Claim itself failed. Nothing happened.
    ClaimFailed,
    /// Was stuck (TransferFailed or ConfirmFailed), but admin manually resolved it.
    AdminResolved,
}

#[derive(CandidType, Clone, Debug, Deserialize, serde::Serialize)]
pub struct LiquidationRecordV1 {
    pub id: u64,
    pub vault_id: u64,
    pub timestamp: u64,
    pub status: LiquidationStatus,

    // Amounts (all in native units for their token)
    pub collateral_claimed_e8s: u64,   // ICP claimed from backend
    pub debt_to_cover_e8s: u64,        // icUSD debt this claim covers
    pub icp_swapped_e8s: u64,          // ICP sent to ICPSwap
    pub ckusdc_received_e6: u64,       // ckUSDC received from swap
    pub ckusdc_transferred_e6: u64,    // ckUSDC successfully sent to backend
    pub icp_to_treasury_e8s: u64,      // Liquidation bonus sent to treasury

    // Price data
    pub oracle_price_e8s: u64,         // ICP/USD price from backend at claim time
    pub effective_price_e8s: u64,      // Actual swap price achieved
    pub slippage_bps: i32,             // Slippage in basis points

    // Error info
    pub error_message: Option<String>,
    pub confirm_retry_count: u8,       // How many confirm retries were attempted
}

#[derive(CandidType, Clone, Debug, Deserialize, serde::Serialize)]
pub enum LiquidationRecordVersioned {
    V1(LiquidationRecordV1),
}

// Storable impl: serialize as candid bytes
impl Storable for LiquidationRecordVersioned {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(candid::encode_one(self).expect("Failed to encode record"))
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).expect("Failed to decode record")
    }
    const BOUND: ic_stable_structures::storable::Bound =
        ic_stable_structures::storable::Bound::Unbounded;
}

thread_local! {
    static HISTORY: RefCell<StableBTreeMap<u64, LiquidationRecordVersioned, memory::Mem>> =
        RefCell::new(StableBTreeMap::init(memory::get_memory(memory::MEM_ID_HISTORY)));

    static NEXT_ID: RefCell<StableCell<u64, memory::Mem>> =
        RefCell::new(StableCell::init(
            memory::get_memory(memory::MEM_ID_NEXT_ID),
            0u64,
        ).expect("Failed to init NEXT_ID cell"));
}

pub fn next_id() -> u64 {
    NEXT_ID.with(|c| {
        let id = *c.borrow().get();
        c.borrow_mut().set(id + 1).expect("Failed to increment NEXT_ID");
        id
    })
}

pub fn insert_record(record: LiquidationRecordVersioned) {
    let id = match &record {
        LiquidationRecordVersioned::V1(r) => r.id,
    };
    HISTORY.with(|h| h.borrow_mut().insert(id, record));
}

pub fn get_record(id: u64) -> Option<LiquidationRecordVersioned> {
    HISTORY.with(|h| h.borrow().get(&id))
}

pub fn get_records(offset: u64, limit: u64) -> Vec<LiquidationRecordVersioned> {
    HISTORY.with(|h| {
        let map = h.borrow();
        let count = NEXT_ID.with(|c| *c.borrow().get());
        if count == 0 || offset >= count { return vec![]; }
        // Return most recent first
        let start = count.saturating_sub(offset + limit);
        let end = count.saturating_sub(offset);
        (start..end).filter_map(|id| map.get(&id)).collect()
    })
}

pub fn record_count() -> u64 {
    NEXT_ID.with(|c| *c.borrow().get())
}

/// Returns all records with TransferFailed or ConfirmFailed status.
pub fn get_stuck_records() -> Vec<LiquidationRecordVersioned> {
    HISTORY.with(|h| {
        let map = h.borrow();
        let count = NEXT_ID.with(|c| *c.borrow().get());
        (0..count).filter_map(|id| {
            let record = map.get(&id)?;
            match &record {
                LiquidationRecordVersioned::V1(r) => match r.status {
                    LiquidationStatus::TransferFailed | LiquidationStatus::ConfirmFailed => Some(record),
                    _ => None,
                },
            }
        }).collect()
    })
}

/// Update a record's status (used by admin resolution).
pub fn update_record_status(id: u64, new_status: LiquidationStatus) {
    HISTORY.with(|h| {
        let mut map = h.borrow_mut();
        if let Some(mut record) = map.get(&id) {
            match &mut record {
                LiquidationRecordVersioned::V1(ref mut r) => {
                    r.status = new_status;
                }
            }
            map.insert(id, record);
        }
    });
}

/// Migrate legacy BotLiquidationEvent entries into the new stable map.
/// Called once during the first post_upgrade after migration.
pub fn migrate_legacy_events(events: &[crate::state::BotLiquidationEvent]) {
    for event in events {
        let id = next_id();
        let status = if event.success {
            LiquidationStatus::Completed
        } else {
            LiquidationStatus::SwapFailed
        };
        let record = LiquidationRecordV1 {
            id,
            vault_id: event.vault_id,
            timestamp: event.timestamp,
            status,
            collateral_claimed_e8s: event.collateral_received_e8s,
            debt_to_cover_e8s: event.debt_covered_e8s,
            icp_swapped_e8s: 0, // not tracked in legacy
            ckusdc_received_e6: 0, // legacy used icUSD, not ckUSDC
            ckusdc_transferred_e6: 0,
            icp_to_treasury_e8s: event.collateral_to_treasury_e8s,
            oracle_price_e8s: event.effective_price_e8s,
            effective_price_e8s: event.effective_price_e8s,
            slippage_bps: event.slippage_bps,
            error_message: event.error_message.clone(),
            confirm_retry_count: 0,
        };
        insert_record(LiquidationRecordVersioned::V1(record));
    }
}
```

- [ ] Create `src/liquidation_bot/src/history.rs`
- [ ] Implement `Storable` for `LiquidationRecordVersioned`
- [ ] Implement `migrate_legacy_events`
- [ ] Implement query helpers: `next_id`, `insert_record`, `get_record`, `get_records`, `record_count`, `get_stuck_records`, `update_record_status`

**Commit:** `feat(bot): add LiquidationRecordV1 history type with stable map`

---

### Task 3: ICPSwap integration module

**Create `src/liquidation_bot/src/icpswap.rs`:**

Port from `rumi-arb-bot/src/arb_bot/src/swaps.rs`. Key difference from plan v2: `quote` and `depositFromAndSwap` use DIFFERENT arg types.

```rust
use candid::{CandidType, Deserialize, Nat, Principal};

// -- ICPSwap Types --

/// Args for `depositFromAndSwap` (5 fields)
#[derive(CandidType, Clone, Debug)]
pub struct DepositAndSwapArgs {
    #[serde(rename = "amountIn")]
    pub amount_in: String,
    #[serde(rename = "amountOutMinimum")]
    pub amount_out_minimum: String,
    #[serde(rename = "zeroForOne")]
    pub zero_for_one: bool,
    #[serde(rename = "tokenInFee")]
    pub token_in_fee: Nat,
    #[serde(rename = "tokenOutFee")]
    pub token_out_fee: Nat,
}

/// Args for `quote` (3 fields -- different from DepositAndSwapArgs!)
#[derive(CandidType, Clone, Debug)]
pub struct SwapArgs {
    #[serde(rename = "amountIn")]
    pub amount_in: String,
    #[serde(rename = "amountOutMinimum")]
    pub amount_out_minimum: String,
    #[serde(rename = "zeroForOne")]
    pub zero_for_one: bool,
}

/// ICPSwap error variant
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum IcpSwapError {
    CommonError,
    InsufficientFunds,
    InternalError(String),
    UnsupportedToken(String),
}

/// Result from quote and depositFromAndSwap
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum IcpSwapResult {
    #[serde(rename = "ok")]
    Ok(Nat),
    #[serde(rename = "err")]
    Err(IcpSwapError),
}

impl std::fmt::Display for IcpSwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IcpSwapError::CommonError => write!(f, "CommonError"),
            IcpSwapError::InsufficientFunds => write!(f, "InsufficientFunds"),
            IcpSwapError::InternalError(s) => write!(f, "InternalError: {}", s),
            IcpSwapError::UnsupportedToken(s) => write!(f, "UnsupportedToken: {}", s),
        }
    }
}

// -- Pool metadata (used to determine token ordering) --

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Token {
    pub address: String,
    pub standard: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct PoolMetadata {
    pub fee: Nat,
    pub key: String,
    pub liquidity: Nat,
    #[serde(rename = "maxLiquidityPerTick")]
    pub max_liquidity_per_tick: Nat,
    #[serde(rename = "nextPositionId")]
    pub next_position_id: Nat,
    #[serde(rename = "sqrtPriceX96")]
    pub sqrt_price_x96: Nat,
    pub tick: candid::Int,
    pub token0: Token,
    pub token1: Token,
}

/// metadata() returns Result_6 = variant { ok: PoolMetadata; err: Error }
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MetadataResult {
    #[serde(rename = "ok")]
    Ok(PoolMetadata),
    #[serde(rename = "err")]
    Err(IcpSwapError),
}

// -- Call wrappers --

/// Query a swap quote. Uses 3-field SwapArgs (NOT DepositAndSwapArgs).
pub async fn quote(
    pool: Principal,
    amount_in: u64,
    zero_for_one: bool,
) -> Result<u64, String> {
    let args = SwapArgs {
        amount_in: amount_in.to_string(),
        amount_out_minimum: "0".to_string(),
        zero_for_one,
    };

    let result: Result<(IcpSwapResult,), _> =
        ic_cdk::call(pool, "quote", (args,)).await;

    match result {
        Ok((IcpSwapResult::Ok(n),)) => {
            Ok(n.0.to_string().parse::<u64>().unwrap_or(0))
        }
        Ok((IcpSwapResult::Err(e),)) => Err(format!("ICPSwap quote error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap quote call failed ({:?}): {}", code, msg)),
    }
}

/// Execute a swap via depositFromAndSwap. Uses 5-field DepositAndSwapArgs.
/// Requires prior ICRC-2 approval from the bot to the pool for the input token.
pub async fn deposit_and_swap(
    pool: Principal,
    amount_in: u64,
    min_amount_out: u64,
    zero_for_one: bool,
    token_in_fee: u64,
    token_out_fee: u64,
) -> Result<u64, String> {
    let args = DepositAndSwapArgs {
        amount_in: amount_in.to_string(),
        amount_out_minimum: min_amount_out.to_string(),
        zero_for_one,
        token_in_fee: Nat::from(token_in_fee),
        token_out_fee: Nat::from(token_out_fee),
    };

    let result: Result<(IcpSwapResult,), _> =
        ic_cdk::call(pool, "depositFromAndSwap", (args,)).await;

    match result {
        Ok((IcpSwapResult::Ok(n),)) => {
            Ok(n.0.to_string().parse::<u64>().unwrap_or(0))
        }
        Ok((IcpSwapResult::Err(e),)) => Err(format!("ICPSwap swap error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap swap call failed ({:?}): {}", code, msg)),
    }
}

/// Fetch pool metadata to determine token ordering (which token is token0).
pub async fn fetch_metadata(pool: Principal) -> Result<PoolMetadata, String> {
    let result: Result<(MetadataResult,), _> =
        ic_cdk::call(pool, "metadata", ()).await;

    match result {
        Ok((MetadataResult::Ok(m),)) => Ok(m),
        Ok((MetadataResult::Err(e),)) => Err(format!("ICPSwap metadata error: {}", e)),
        Err((code, msg)) => Err(format!("ICPSwap metadata call failed ({:?}): {}", code, msg)),
    }
}
```

- [ ] Create `src/liquidation_bot/src/icpswap.rs` with correct types for both `quote` (3-field) and `depositFromAndSwap` (5-field)
- [ ] Implement `quote`, `deposit_and_swap`, `fetch_metadata` call wrappers
- [ ] Define all ICPSwap candid types with correct `serde(rename)` for camelCase

**Commit:** `feat(bot): add ICPSwap integration module with correct quote/swap types`

---

### Task 4: Swap module rewrite

**Rewrite `src/liquidation_bot/src/swap.rs`:**

Delete all KongSwap and 3pool code. Replace with:

```rust
use candid::{Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::ApproveArgs;

use crate::icpswap;
use crate::state::BotConfig;

pub struct SwapResult {
    pub ckusdc_received_e6: u64,
    pub effective_price_e8s: u64,
}

/// One-time infinite ICRC-2 approve: bot approves the ICPSwap pool to spend ICP.
/// Amount = u128::MAX, no expiry. Only needs to be called once per (token, spender) pair.
pub async fn approve_infinite(
    token_ledger: Principal,
    spender: Principal,
) -> Result<(), String> {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: spender, subaccount: None },
        amount: Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError>,), _> =
        ic_cdk::call(token_ledger, "icrc2_approve", (args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Approve failed: {:?}", e)),
        Err((code, msg)) => Err(format!("Approve call failed: {:?} {}", code, msg)),
    }
}

/// Quote how much ckUSDC we'd get for `icp_amount_e8s` ICP.
pub async fn quote_icp_for_ckusdc(config: &BotConfig, icp_amount_e8s: u64) -> Result<u64, String> {
    let zero_for_one = config.icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one).await
}

/// Swap ICP for ckUSDC on ICPSwap. Returns ckUSDC received (e6 native units).
///
/// Flow: get quote -> apply slippage -> call depositFromAndSwap.
/// Requires infinite approve to already be in place.
pub async fn swap_icp_for_ckusdc(
    config: &BotConfig,
    icp_amount_e8s: u64,
) -> Result<SwapResult, String> {
    let zero_for_one = config.icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    // Get quote
    let quoted_output = icpswap::quote(
        config.icpswap_pool,
        icp_amount_e8s,
        zero_for_one,
    ).await?;

    if quoted_output == 0 {
        return Err("Quote returned zero output".to_string());
    }

    // Apply slippage tolerance
    let min_output = apply_slippage(quoted_output, config.max_slippage_bps);

    log!(crate::INFO, "ICPSwap quote: {} ICP e8s -> {} ckUSDC e6 (min: {})",
        icp_amount_e8s, quoted_output, min_output);

    // ICP fee = 10_000 e8s, ckUSDC fee = 10 e6 (configurable via cached fields)
    let icp_fee = config.icp_fee_e8s.unwrap_or(10_000);
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(10);

    // Execute swap
    let received = icpswap::deposit_and_swap(
        config.icpswap_pool,
        icp_amount_e8s,
        min_output,
        zero_for_one,
        icp_fee,
        ckusdc_fee,
    ).await?;

    // Calculate effective price: (ckusdc_e6 * 100) / icp_e8s * 1e8
    // = ckusdc_e6 * 10_000_000_000 / icp_e8s
    // This gives price in e8 format (matching oracle price format)
    let effective_price_e8s = if icp_amount_e8s > 0 {
        (received as u128 * 10_000_000_000 / icp_amount_e8s as u128) as u64
    } else {
        0
    };

    log!(crate::INFO, "ICPSwap swap complete: {} ckUSDC e6 received, effective price {} e8s",
        received, effective_price_e8s);

    Ok(SwapResult {
        ckusdc_received_e6: received,
        effective_price_e8s,
    })
}

/// Apply slippage tolerance: reduce expected output by max_slippage_bps basis points.
fn apply_slippage(amount: u64, max_slippage_bps: u16) -> u64 {
    let reduction = amount as u128 * max_slippage_bps as u128 / 10_000;
    (amount as u128 - reduction) as u64
}

/// Transfer collateral (ICP) back to the backend canister.
/// Used when swap fails and we need to return the claimed ICP.
pub async fn return_collateral_to_backend(
    config: &BotConfig,
    amount_e8s: u64,
    collateral_ledger: Principal,
) -> Result<(), String> {
    let fee = config.icp_fee_e8s.unwrap_or(10_000);
    let send_amount = amount_e8s.saturating_sub(fee);
    if send_amount == 0 {
        return Err("Collateral amount too small to cover transfer fee".to_string());
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()), // dedup protection
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(collateral_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("Transfer call failed: {:?} {}", code, msg)),
    }
}

/// Transfer ckUSDC from bot to backend canister.
/// This is a direct icrc1_transfer (bot sends from its own account, no approve needed).
/// Returns the actual amount received by the backend (after fee subtraction).
pub async fn transfer_ckusdc_to_backend(
    config: &BotConfig,
    amount_e6: u64,
) -> Result<u64, String> {
    let fee = config.ckusdc_fee_e6.unwrap_or(10);
    let send_amount = amount_e6.saturating_sub(fee);
    if send_amount == 0 {
        return Err("ckUSDC amount too small to cover transfer fee".to_string());
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()), // dedup protection
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(config.ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(send_amount),
        Ok((Err(e),)) => Err(format!("ckUSDC transfer error: {:?}", e)),
        Err((code, msg)) => Err(format!("ckUSDC transfer call failed: {:?} {}", code, msg)),
    }
}

/// Transfer ICP to treasury (liquidation bonus).
pub async fn transfer_icp_to_treasury(
    config: &BotConfig,
    amount_e8s: u64,
) -> Result<(), String> {
    let fee = config.icp_fee_e8s.unwrap_or(10_000);
    let send_amount = amount_e8s.saturating_sub(fee);
    if send_amount == 0 {
        return Ok(()); // dust amount, skip
    }

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.treasury_principal,
            subaccount: None,
        },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()), // dedup protection
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(config.icp_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(_),)) => {
            log!(crate::INFO, "Transferred {} e8s ICP to treasury", send_amount);
            Ok(())
        }
        Ok((Err(e),)) => Err(format!("ICP transfer to treasury failed: {:?}", e)),
        Err((code, msg)) => Err(format!("ICP transfer call failed: {:?} {}", code, msg)),
    }
}
```

- [ ] Delete all KongSwap types and functions
- [ ] Delete all 3pool types and functions
- [ ] Implement `approve_infinite`, `quote_icp_for_ckusdc`, `swap_icp_for_ckusdc`
- [ ] Implement `transfer_ckusdc_to_backend` (new: bot sends ckUSDC directly to backend, returns actual amount after fee)
- [ ] Keep `return_collateral_to_backend` (adapted to use cached fee)
- [ ] Keep `transfer_icp_to_treasury` (adapted)
- [ ] Delete `swap_icp_for_stable` and `swap_stable_for_icusd`

**Commit:** `feat(bot): rewrite swap module for ICPSwap single-hop ICP->ckUSDC`

---

### Task 5: Update BotConfig + state management

**Modify `src/liquidation_bot/src/state.rs`:**

```rust
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotConfig {
    pub backend_principal: Principal,
    pub treasury_principal: Principal,
    pub admin: Principal,
    pub max_slippage_bps: u16,
    pub icp_ledger: Principal,
    pub ckusdc_ledger: Principal,

    // ICPSwap config (replaces kong_swap + three_pool)
    pub icpswap_pool: Principal,

    // Cached after admin_resolve_pool_ordering (determines swap direction)
    #[serde(default)]
    pub icpswap_zero_for_one: Option<bool>,

    // Cached ledger fees (set by admin or auto-detected)
    #[serde(default)]
    pub icp_fee_e8s: Option<u64>,
    #[serde(default)]
    pub ckusdc_fee_e6: Option<u64>,

    // Legacy fields kept for deserialization compatibility (ignored)
    #[serde(default)]
    pub three_pool_principal: Option<Principal>,
    #[serde(default)]
    pub kong_swap_principal: Option<Principal>,
    #[serde(default)]
    pub ckusdt_ledger: Option<Principal>,
    #[serde(default)]
    pub icusd_ledger: Option<Principal>,
}
```

Note: Legacy fields use `Option` with `#[serde(default)]` so that both old-format (required fields) and new-format (absent fields) configs deserialize correctly. After one upgrade cycle, these can be removed.

**Update save/load to use dedicated `VirtualMemory` region:**

Replace raw `stable64_write`/`stable64_read` with a length-prefixed JSON write to `memory::get_memory(MEM_ID_CONFIG)`. Note: `StableCell` requires `Bound::Bounded` which doesn't work for variable-size JSON. Instead, write directly to the `VirtualMemory` with an 8-byte length prefix (same pattern as the legacy code, but scoped to a virtual memory region instead of raw offset 0). The `save_to_stable_memory` and `load_from_stable_memory` functions remain for the legacy path (first upgrade only).

- [ ] Update `BotConfig` struct: remove required KongSwap/3pool fields, add ICPSwap fields, keep legacy as `Option` with `serde(default)`
- [ ] Add `save_config_to_stable` and `load_config_from_stable` using StableCell
- [ ] Keep legacy `save_to_stable_memory` and `load_from_stable_memory` for pre-migration path
- [ ] Update `BotStats`: rename `total_icusd_burned_e8s` to `total_ckusdc_deposited_e6`, keep `serde(alias)` for backward compat

**Commit:** `feat(bot): update BotConfig for ICPSwap, add StableCell config storage`

---

### Task 6: Backend cleanup -- delete dead code, add admin_resolve_stuck_claim

**Modify `src/rumi_protocol_backend/src/main.rs`:**

1. **Delete** `bot_deposit_to_reserves` endpoint (lines ~2069-2084)
2. **Delete** `bot_total_icusd_deposited_e8s` from state.rs
3. **Remove** `total_icusd_deposited_e8s` from `BotStatsResponse` and `get_bot_stats`
4. **Add** `admin_resolve_stuck_claim` endpoint:

```rust
/// Admin-only: force-resolve a stuck bot claim. Used when the bot's ckUSDC transfer
/// or confirm failed and the vault is stuck with bot_processing=true.
///
/// - `apply_debt_reduction = false`: TransferFailed case. ckUSDC never reached the backend,
///   so vault debt stays as-is. Just unlocks vault and restores budget.
/// - `apply_debt_reduction = true`: ConfirmFailed case. ckUSDC DID reach the backend,
///   so also write down the vault's debt and collateral (same as what confirm would do).
///
/// The admin should ALWAYS try `admin_retry_stuck_claim` on the bot first.
/// Use this only as a last resort after manually verifying fund positions.
#[candid_method(update)]
#[update]
fn admin_resolve_stuck_claim(vault_id: u64, apply_debt_reduction: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == Some(caller));
    if !is_dev {
        return Err(ProtocolError::GenericError("Unauthorized: developer only".to_string()));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            if apply_debt_reduction {
                // ConfirmFailed: ckUSDC reached backend, write down debt like confirm would
                vault.borrowed_icusd_amount -= ICUSD::new(claim.debt_amount);
                vault.collateral_amount = vault.collateral_amount.saturating_sub(claim.collateral_amount);
                s.bot_total_debt_covered_e8s += claim.debt_amount;
            }
            vault.bot_processing = false;
        }
        if !apply_debt_reduction {
            // TransferFailed: ckUSDC never arrived, restore budget
            s.bot_budget_remaining_e8s += claim.debt_amount;
        }
        s.bot_claims.remove(&vault_id);
    });

    log!(INFO, "[admin_resolve_stuck_claim] Resolved stuck claim for vault #{}: debt={}, collateral={}, debt_reduced={}",
        vault_id, claim.debt_amount, claim.collateral_amount, apply_debt_reduction);

    Ok(())
}
```

- [ ] Delete `bot_deposit_to_reserves` endpoint
- [ ] Delete `bot_total_icusd_deposited_e8s` from state (with `serde(default)` for compat)
- [ ] Remove `total_icusd_deposited_e8s` from `BotStatsResponse`
- [ ] Add `admin_resolve_stuck_claim` endpoint (developer-only)
- [ ] Update `rumi_protocol_backend.did`

**Commit:** `feat(backend): delete bot_deposit_to_reserves, add admin_resolve_stuck_claim`

---

### Task 7: Rewrite process.rs -- single-hop flow with failure recovery

**Rewrite `src/liquidation_bot/src/process.rs`:**

The new flow:

```
claim -> swap ICP->ckUSDC -> transfer ckUSDC to backend -> confirm (with retry) -> treasury
```

Failure recovery:
- **Swap fails:** return ICP, cancel claim. Record `SwapFailed`.
- **Transfer fails:** Do NOT retry. Do NOT cancel (ICP is already gone, swapped to ckUSDC). Record `TransferFailed`. Log loudly. Admin resolves later.
- **Confirm fails:** Retry up to 5 times immediately. If all fail, record `ConfirmFailed`. Log loudly. Admin resolves later.

```rust
use crate::{history, state, swap};
use crate::history::{LiquidationRecordV1, LiquidationRecordVersioned, LiquidationStatus};
// ... (BackendResult, BackendError, BotLiquidationResult types stay the same)

const CONFIRM_ATTEMPTS: u8 = 5;

pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };

    let config = match state::read_state(|s| s.config.clone()) {
        Some(c) => c,
        None => {
            log!(crate::INFO, "Bot not configured, skipping vault #{}", vault.vault_id);
            return;
        }
    };

    log!(crate::INFO, "Processing vault #{}", vault.vault_id);
    let record_id = history::next_id();
    let timestamp = ic_cdk::api::time();

    // -- Phase 1: CLAIM --
    let liq_result = call_bot_claim_liquidation(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price_e8s),
        Err(e) => {
            log!(crate::INFO, "Claim failed for vault #{}: {}", vault.vault_id, e);
            write_record(record_id, vault.vault_id, timestamp, LiquidationStatus::ClaimFailed,
                0, 0, 0, 0, 0, 0, 0, 0, 0, Some(e), 0);
            return;
        }
    };

    // -- Phase 2: SWAP ICP -> ckUSDC --
    let swap_amount = calculate_swap_amount(collateral_amount, debt_covered, collateral_price);
    let swap_result = swap::swap_icp_for_ckusdc(&config, swap_amount).await;

    let (ckusdc_received, effective_price) = match swap_result {
        Ok(r) => (r.ckusdc_received_e6, r.effective_price_e8s),
        Err(e) => {
            log!(crate::INFO, "Swap failed for vault #{}: {}. Returning ICP.", vault.vault_id, e);
            // Return ICP and cancel -- clean recovery
            let _ = swap::return_collateral_to_backend(&config, collateral_amount, config.icp_ledger).await;
            let _ = call_bot_cancel_liquidation(&config, vault.vault_id).await;
            write_record(record_id, vault.vault_id, timestamp, LiquidationStatus::SwapFailed,
                collateral_amount, debt_covered, swap_amount, 0, 0, 0,
                collateral_price, 0, 0, Some(e), 0);
            return;
        }
    };

    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    // -- Phase 3: TRANSFER ckUSDC to backend --
    // NO RETRY on transfer. If it fails, mark stuck immediately.
    // Returns actual amount received by backend (after fee subtraction).
    let transfer_result = swap::transfer_ckusdc_to_backend(&config, ckusdc_received).await;

    let ckusdc_transferred = match transfer_result {
        Ok(actual_sent) => actual_sent,
        Err(e) => {
            log!(crate::INFO,
                "STUCK: ckUSDC transfer failed for vault #{}. Bot holding {} ckUSDC e6. Error: {}. Needs admin resolution.",
                vault.vault_id, ckusdc_received, e);
            write_record(record_id, vault.vault_id, timestamp, LiquidationStatus::TransferFailed,
                collateral_amount, debt_covered, swap_amount, ckusdc_received, 0, 0,
                collateral_price, effective_price, slippage_bps, Some(e), 0);
            return;
        }
    };

    // -- Phase 4: CONFIRM (with retry -- this is idempotent) --
    let mut confirm_ok = false;
    let mut confirm_retries: u8 = 0;
    let mut last_confirm_err = String::new();

    for attempt in 0..CONFIRM_ATTEMPTS {
        match call_bot_confirm_liquidation(&config, vault.vault_id).await {
            Ok(()) => {
                confirm_ok = true;
                confirm_retries = attempt;
                break;
            }
            Err(e) => {
                last_confirm_err = e;
                confirm_retries = attempt;
                if attempt + 1 < CONFIRM_ATTEMPTS {
                    log!(crate::INFO, "Confirm attempt {}/{} failed for vault #{}: {}. Retrying.",
                        attempt + 1, CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
                }
            }
        }
    }

    if !confirm_ok {
        log!(crate::INFO,
            "STUCK: Confirm failed after {} attempts for vault #{}. ckUSDC is in backend but debt not written down. Error: {}. Needs admin resolution.",
            CONFIRM_ATTEMPTS, vault.vault_id, last_confirm_err);
        write_record(record_id, vault.vault_id, timestamp, LiquidationStatus::ConfirmFailed,
            collateral_amount, debt_covered, swap_amount, ckusdc_received, ckusdc_transferred, 0,
            collateral_price, effective_price, slippage_bps, Some(last_confirm_err), confirm_retries);
        return;
    }

    // -- Phase 5: TREASURY (liquidation bonus) --
    let icp_to_treasury = collateral_amount.saturating_sub(swap_amount);
    if icp_to_treasury > 0 {
        let _ = swap::transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // -- Phase 6: SUCCESS --
    log!(crate::INFO, "Vault #{} liquidated: debt={} e8s, ckUSDC={} e6, treasury={} e8s ICP",
        vault.vault_id, debt_covered, ckusdc_received, icp_to_treasury);

    write_record(record_id, vault.vault_id, timestamp, LiquidationStatus::Completed,
        collateral_amount, debt_covered, swap_amount, ckusdc_received, ckusdc_transferred,
        icp_to_treasury, collateral_price, effective_price, slippage_bps, None, confirm_retries);

    // Update legacy stats (for backward compat with existing explorer UI)
    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
        s.stats.events_count += 1;
    });
}
```

**Note on confirm retries:** On the IC, you cannot `await` a sleep inside a single update call. The retries happen back-to-back with no delay. This is sufficient for transient network errors. If the issue is sustained (e.g., backend canister is stopped), no amount of delay within one call will help. Timer-based delayed retries can be added in a follow-up if needed.

- [ ] Rewrite `process_pending` with the 6-phase flow
- [ ] Implement no-retry for transfer, immediate-retry (5x) for confirm
- [ ] Write `LiquidationRecord` at every phase transition
- [ ] Delete all KongSwap/3pool helpers (`call_bot_deposit_to_reserves`, `swap_stable_for_icusd` references)
- [ ] Keep `call_bot_claim_liquidation`, `call_bot_confirm_liquidation`, `call_bot_cancel_liquidation` (unchanged)
- [ ] Keep `calculate_swap_amount` and `calculate_slippage` helpers
- [ ] Delete public test wrappers: `call_bot_deposit_to_reserves_pub`, update remaining pub wrappers
- [ ] Add `write_record` helper function

**Commit:** `feat(bot): rewrite process.rs for single-hop ICPSwap with failure recovery`

---

### Task 8: Bot query + admin endpoints

**Modify `src/liquidation_bot/src/lib.rs`:**

Add new endpoints, update/delete old test endpoints.

```rust
// -- History query endpoints --

#[query]
fn get_liquidation(id: u64) -> Option<history::LiquidationRecordVersioned> {
    history::get_record(id)
}

#[query]
fn get_liquidations(offset: u64, limit: u64) -> Vec<history::LiquidationRecordVersioned> {
    history::get_records(offset, limit)
}

#[query]
fn get_liquidation_count() -> u64 {
    history::record_count()
}

/// Returns all records with TransferFailed or ConfirmFailed status.
/// Makes it easy for admin to see what needs manual resolution.
#[query]
fn get_stuck_liquidations() -> Vec<history::LiquidationRecordVersioned> {
    history::get_stuck_records()
}

// -- Admin endpoints --

/// One-time: fetch pool metadata to determine if ICP is token0 or token1.
/// Sets `icpswap_zero_for_one` in config.
#[update]
async fn admin_resolve_pool_ordering() {
    require_admin();
    let pool = state::read_state(|s| s.config.as_ref().unwrap().icpswap_pool);
    let icp_ledger = state::read_state(|s| s.config.as_ref().unwrap().icp_ledger);

    let metadata = icpswap::fetch_metadata(pool).await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Failed to fetch metadata: {}", e)));

    let icp_text = icp_ledger.to_text();
    let zero_for_one = metadata.token0.address == icp_text;

    state::mutate_state(|s| {
        if let Some(ref mut config) = s.config {
            config.icpswap_zero_for_one = Some(zero_for_one);
        }
    });

    log!(INFO, "Pool ordering resolved: ICP is token{}, zeroForOne={}",
        if zero_for_one { "0" } else { "1" }, zero_for_one);
}

/// One-time: set up infinite ICRC-2 approve for ICP to the ICPSwap pool.
#[update]
async fn admin_approve_pool() {
    require_admin();
    let (icp_ledger, pool) = state::read_state(|s| {
        let c = s.config.as_ref().unwrap();
        (c.icp_ledger, c.icpswap_pool)
    });

    swap::approve_infinite(icp_ledger, pool).await
        .unwrap_or_else(|e| ic_cdk::trap(&format!("Approve failed: {}", e)));

    log!(INFO, "Infinite approve set: ICP ledger {} -> pool {}", icp_ledger, pool);
}

/// Emergency: transfer all bot ckUSDC to a target principal and mark the
/// associated stuck liquidation record as AdminResolved.
/// Used to recover stuck ckUSDC after a TransferFailed event.
#[update]
async fn admin_sweep_ckusdc(target: Principal, record_id: Option<u64>) {
    require_admin();
    let ckusdc_ledger = state::read_state(|s| s.config.as_ref().unwrap().ckusdc_ledger);

    // Query bot's ckUSDC balance
    let balance_result: Result<(Nat,), _> =
        ic_cdk::call(ckusdc_ledger, "icrc1_balance_of", (Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },)).await;

    let balance = match balance_result {
        Ok((b,)) => {
            let val = b.0.to_string().parse::<u64>().unwrap_or(0);
            if val == 0 {
                ic_cdk::trap("Bot has zero ckUSDC balance");
            }
            val
        }
        Err((code, msg)) => ic_cdk::trap(&format!("Balance query failed: {:?} {}", code, msg)),
    };

    let fee = state::read_state(|s| s.config.as_ref().unwrap().ckusdc_fee_e6.unwrap_or(10));
    let send_amount = balance.saturating_sub(fee);

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account { owner: target, subaccount: None },
        amount: Nat::from(send_amount),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()), // dedup protection
    };

    let result: Result<(Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> =
        ic_cdk::call(ckusdc_ledger, "icrc1_transfer", (transfer_args,)).await;

    match result {
        Ok((Ok(block),)) => {
            log!(INFO, "Swept {} ckUSDC e6 to {}, block {}", send_amount, target, block);
            // If a record_id was provided, update its status to AdminResolved
            if let Some(id) = record_id {
                history::update_record_status(id, history::LiquidationStatus::AdminResolved);
                log!(INFO, "Marked record #{} as AdminResolved", id);
            }
        }
        Ok((Err(e),)) => ic_cdk::trap(&format!("Sweep transfer failed: {:?}", e)),
        Err((code, msg)) => ic_cdk::trap(&format!("Sweep call failed: {:?} {}", code, msg)),
    }
}

/// Retry confirm for a stuck claim (after TransferFailed where ckUSDC was
/// manually swept to the backend, or after a transient ConfirmFailed).
#[update]
async fn admin_retry_stuck_claim(vault_id: u64) {
    require_admin();
    let config = state::read_state(|s| s.config.clone()).expect("Not configured");

    match process::call_bot_confirm_liquidation_pub(&config, vault_id).await {
        Ok(()) => {
            log!(INFO, "admin_retry_stuck_claim: confirmed vault #{}", vault_id);
        }
        Err(e) => {
            ic_cdk::trap(&format!("Confirm still failing for vault #{}: {}", vault_id, e));
        }
    }
}
```

**Delete or rewrite test endpoints:**
- `test_swap_pipeline` -- delete (references KongSwap + 3pool)
- `test_force_liquidate` -- rewrite to use new single-hop flow (or delete if admin_retry + manual testing is sufficient)
- `test_force_partial_liquidate` -- same treatment

- [ ] Add `get_liquidation`, `get_liquidations`, `get_liquidation_count`, `get_stuck_liquidations` query endpoints
- [ ] Add `admin_resolve_pool_ordering`, `admin_approve_pool`, `admin_sweep_ckusdc`, `admin_retry_stuck_claim` admin endpoints
- [ ] Delete or rewrite `test_swap_pipeline`, `test_force_liquidate`, `test_force_partial_liquidate`
- [ ] Wire `mod memory; mod history; mod icpswap;` in lib.rs

**Commit:** `feat(bot): add history queries + admin endpoints for ICPSwap and stuck claims`

---

### Task 9: Candid updates + declaration regeneration

- [ ] Update `src/liquidation_bot/liquidation_bot.did`:
  - Remove `three_pool_principal`, `kong_swap_principal`, `ckusdt_ledger`, `icusd_ledger` from BotConfig
  - Add `icpswap_pool`, `icpswap_zero_for_one`, `icp_fee_e8s`, `ckusdc_fee_e6`
  - Add `LiquidationRecordV1`, `LiquidationStatus`, `LiquidationRecordVersioned` types
  - Add `get_liquidation`, `get_liquidations`, `get_liquidation_count`, `get_stuck_liquidations` query methods
  - Add `admin_resolve_pool_ordering`, `admin_approve_pool`, `admin_sweep_ckusdc`, `admin_retry_stuck_claim` update methods
  - Remove `test_swap_pipeline` and related test endpoints (or update)
- [ ] Update `src/rumi_protocol_backend/rumi_protocol_backend.did`:
  - Remove `bot_deposit_to_reserves`
  - Remove `total_icusd_deposited_e8s` from `BotStatsResponse`
  - Add `admin_resolve_stuck_claim`
- [ ] Regenerate declarations: `dfx generate liquidation_bot && dfx generate rumi_protocol_backend`

**Commit:** `chore: update candid interfaces and regenerate declarations`

---

### Task 10: Frontend audit -- fix references to deleted symbols

The following frontend files reference `total_icusd_deposited_e8s` (being deleted from `BotStatsResponse`):

- `src/vault_frontend/src/lib/components/liquidations/LiquidationBotTab.svelte` (lines 12, 69, 133, 137)
  - Line 137 computes "deficit" from `total_debt_covered_e8s - total_icusd_deposited_e8s` -- remove or replace
- `src/vault_frontend/src/routes/explorer/+page.svelte` (line ~1263)
- `src/vault_frontend/src/routes/docs/liquidation-bot/+page.svelte` (line ~85)

- [ ] Remove/replace references to `total_icusd_deposited_e8s` in `LiquidationBotTab.svelte`
- [ ] Remove "deficit" computation or replace with meaningful metric from new history
- [ ] Fix `explorer/+page.svelte` reference
- [ ] Fix `docs/liquidation-bot/+page.svelte` reference
- [ ] Verify no other frontend files reference deleted symbols: `grep -r "icusd_deposited\|bot_deposit_to_reserves\|kong_swap\|three_pool" src/vault_frontend/`

**Commit:** `fix(frontend): remove references to deleted bot stats fields`

---

### Task 11: PocketIC end-to-end test

Create a stub ICPSwap pool canister that implements `quote`, `depositFromAndSwap`, and `metadata` with hardcoded responses. Test the full flow: claim -> swap -> transfer -> confirm.

- [ ] Create `src/test_icpswap_stub/` canister with hardcoded ICPSwap responses
- [ ] Write integration test in `pocket_ic_tests` or a new `pocket_ic_bot_tests` file:
  - Test happy path: vault -> claim -> swap -> transfer -> confirm -> verify vault debt reduced
  - Test swap failure: stub returns error -> verify ICP returned, claim cancelled
  - Test transfer failure: mock ckUSDC transfer failure -> verify record written with `TransferFailed`
  - Test confirm retry: first confirm call fails, second succeeds -> verify `Completed` with `confirm_retry_count > 0`
- [ ] Verify legacy migration: create bot with old-format state, upgrade, verify history map populated

**Commit:** `test(bot): PocketIC end-to-end test for ICPSwap liquidation flow`

---

### Task 12: Mainnet migration runbook

**Create `docs/liquidation-bot-icpswap-migration.md`:**

Document the exact steps for deploying this upgrade to mainnet:

1. Build the new bot wasm
2. Deploy as upgrade (NOT reinstall): `dfx deploy liquidation_bot --network ic`
3. Verify legacy state migrated: `dfx canister call liquidation_bot get_liquidation_count --network ic`
4. Set new config with ICPSwap pool: `dfx canister call liquidation_bot set_config '(...)' --network ic`
5. Resolve pool ordering: `dfx canister call liquidation_bot admin_resolve_pool_ordering --network ic`
6. Set infinite approve: `dfx canister call liquidation_bot admin_approve_pool --network ic`
7. Deploy backend with `bot_deposit_to_reserves` deleted + `admin_resolve_stuck_claim` added
8. Re-add ICP to `bot_allowed_collateral_types` on backend to activate the bot
9. Monitor first liquidation via `get_liquidations`

- [ ] Write runbook
- [ ] Include rollback instructions (re-deploy old wasm, state is backward-compatible)
- [ ] Document Candid breaking change: `BotConfig` fields changed from required to optional. After upgrade, `set_config` calls must use the new format (ICPSwap fields instead of KongSwap/3pool). Old dfx scripts will not work.

**Commit:** `docs: mainnet migration runbook for liquidation bot ICPSwap rework`

---

### Task 13: Final verification + PR

- [ ] `cargo build --target wasm32-unknown-unknown --release -p liquidation_bot`
- [ ] `cargo build --target wasm32-unknown-unknown --release -p rumi_protocol_backend`
- [ ] `cargo test` (unit tests)
- [ ] `POCKET_IC_BIN=./pocket-ic cargo test --test pocket_ic_tests` (integration)
- [ ] Review all changed files for consistency
- [ ] Create PR against `main`

**Commit:** N/A (PR creation)

---

## Known issues inherited from the shipped protocol (NOT fixed in this PR)

1. **`compute_total_collateral_ratio` does not count ckUSDC/ckUSDT reserves.** This is a pre-existing issue relevant to `repay_to_vault_with_stable` as well, and is being addressed in a separate follow-up PR (PR2). This PR's new liquidation flow will add to protocol ckUSDC reserves in exactly the same way the stable-repay feature already does, so it inherits the same accounting quirk with no additional blast radius.

2. **Confirm retry is immediate (no delay).** On the IC, you cannot `await` a sleep within a single update call. The 5 retries happen back-to-back. If the issue is a brief network hiccup, immediate retries may all fail. Timer-based delayed retries can be added in a follow-up if this proves insufficient in practice.

---

## Task dependencies

```
Task 0 (branch)
  |
Task 1 (memory foundation) --> Task 2 (history types) --> Task 7 (process.rs)
  |                                                             |
Task 5 (BotConfig)          --> Task 3 (icpswap module)  --> Task 4 (swap module) --> Task 7
  |                                                                                      |
Task 6 (backend cleanup)   ------------------------------------------------------------> Task 8 (endpoints)
                                                                                         |
                                                                                    Task 9 (candid + decl)
                                                                                         |
                                                                                    Task 10 (frontend)
                                                                                         |
                                                                                    Task 11 (tests)
                                                                                         |
                                                                                    Task 12 (runbook)
                                                                                         |
                                                                                    Task 13 (PR)
```

Tasks 1, 5, and 6 can be started in parallel.
Tasks 3 and 4 can be done in parallel with Task 2.
Task 7 depends on Tasks 1-5 (needs memory, history, icpswap, swap, and config all ready).
Tasks 8-13 are sequential.
