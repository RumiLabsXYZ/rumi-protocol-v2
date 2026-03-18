# Liquidation Bot Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a privileged bot canister that liquidates unhealthy ICP-collateral vaults on credit, swaps collateral for icUSD via KongSwap + 3pool, and deposits proceeds to protocol reserves.

**Architecture:** New `liquidation_bot` canister receives fire-and-forget notifications from the backend, processes one vault at a time (no retry), swaps ICP→ckStable→icUSD, deposits to reserves. Backend tracks credit/obligation. Frontend adds a "Liquidation Bot" tab to `/liquidations`.

**Tech Stack:** Rust (ic-cdk), Candid, SvelteKit frontend, KongSwap inter-canister calls, 3pool inter-canister calls.

**Spec:** `docs/superpowers/specs/2026-03-14-liquidation-bot-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `src/liquidation_bot/Cargo.toml` | Crate config, deps matching stability_pool pattern |
| `src/liquidation_bot/src/lib.rs` | Canister entry: init, upgrade, timer, endpoints |
| `src/liquidation_bot/src/state.rs` | BotState, stable memory, config |
| `src/liquidation_bot/src/swap.rs` | DexSwap trait + KongSwap implementation |
| `src/liquidation_bot/src/process.rs` | Core liquidation processing logic |
| `src/liquidation_bot/liquidation_bot.did` | Candid interface |
| `src/vault_frontend/src/lib/components/liquidations/LiquidationBotTab.svelte` | Frontend tab |

### Modified Files
| File | Changes |
|------|---------|
| `Cargo.toml` (root) | Add `src/liquidation_bot` to workspace members |
| `dfx.json` | Add `liquidation_bot` canister entry |
| `src/rumi_protocol_backend/src/state.rs` | Bot config fields, budget tracking |
| `src/rumi_protocol_backend/src/main.rs` | New endpoints: bot_liquidate, bot_deposit_to_reserves, set_liquidation_bot_config, get_bot_stats, reset_bot_budget |
| `src/rumi_protocol_backend/src/lib.rs` | Enrich notification payload, also notify bot canister |
| `src/rumi_protocol_backend/rumi_protocol_backend.did` | New types and methods |
| `src/vault_frontend/src/routes/liquidations/+page.svelte` | Add bot tab (first tab) |
| `src/vault_frontend/src/routes/+layout.svelte` | Header indicator for unhandled liquidatable vaults |

---

## Chunk 1: Bot Canister Scaffold + Backend State

### Task 1: Create bot canister crate

**Files:**
- Create: `src/liquidation_bot/Cargo.toml`
- Create: `src/liquidation_bot/src/lib.rs`
- Create: `src/liquidation_bot/src/state.rs`
- Modify: `Cargo.toml` (root workspace)
- Modify: `dfx.json`

- [ ] **Step 1: Create `src/liquidation_bot/Cargo.toml`**

```toml
[package]
name = "liquidation_bot"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
crate-type = ["cdylib", "rlib"]

[dependencies]
candid = "0.10.6"
ic-stable-structures = "0.6.5"
ic-cdk = "0.12.0"
ic-cdk-timers = "0.10.0"
serde = "1.0.210"
serde_json = "1.0"
ic-cdk-macros = "0.8.3"
ic-canister-log = { git = "https://github.com/Rumi-Protocol/ic", rev = "fc278709" }
icrc-ledger-types = { git = "https://github.com/Rumi-Protocol/ic", rev = "fc278709" }
```

- [ ] **Step 2: Create `src/liquidation_bot/src/state.rs`**

Defines `BotState`, `BotConfig`, `BotLiquidationEvent`, stable memory save/load.

```rust
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;
use std::cell::RefCell;

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotConfig {
    pub backend_principal: Principal,
    pub three_pool_principal: Principal,
    pub kong_swap_principal: Principal,
    pub treasury_principal: Principal,
    pub admin: Principal,
    pub max_slippage_bps: u16,
    pub icp_ledger: Principal,
    pub ckusdc_ledger: Principal,
    pub ckusdt_ledger: Principal,
    pub icusd_ledger: Principal,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BotLiquidationEvent {
    pub timestamp: u64,
    pub vault_id: u64,
    pub debt_covered_e8s: u64,
    pub collateral_received_e8s: u64,
    pub icusd_burned_e8s: u64,
    pub collateral_to_treasury_e8s: u64,
    pub swap_route: String,       // e.g. "ICP→ckUSDC→icUSD"
    pub effective_price_e8s: u64, // ICP price from DEX (e8s per ICP)
    pub slippage_bps: i32,        // actual vs oracle
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, Default)]
pub struct BotStats {
    pub total_debt_covered_e8s: u64,
    pub total_icusd_burned_e8s: u64,
    pub total_collateral_received_e8s: u64,
    pub total_collateral_to_treasury_e8s: u64,
    pub events_count: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct BotState {
    pub config: Option<BotConfig>,
    pub stats: BotStats,
    pub liquidation_events: Vec<BotLiquidationEvent>,
    pub pending_vaults: Vec<LiquidatableVaultInfo>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,
    pub collateral_amount: u64,
    pub recommended_liquidation_amount: u64,
    pub collateral_price_e8s: u64,
}

thread_local! {
    static STATE: RefCell<Option<BotState>> = RefCell::default();
}

pub fn mutate_state<F, R>(f: F) -> R
where F: FnOnce(&mut BotState) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("State not initialized")))
}

pub fn read_state<F, R>(f: F) -> R
where F: FnOnce(&BotState) -> R {
    STATE.with(|s| f(s.borrow().as_ref().expect("State not initialized")))
}

pub fn init_state(state: BotState) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
}

pub fn save_to_stable_memory() {
    STATE.with(|s| {
        let state = s.borrow();
        let state = state.as_ref().expect("State not initialized");
        let bytes = serde_json::to_vec(state).expect("Failed to serialize state");
        let len = bytes.len() as u64;
        ic_cdk::api::stable::stable64_grow((len + 65535) / 65536).expect("Failed to grow stable memory");
        ic_cdk::api::stable::stable64_write(0, &len.to_le_bytes());
        ic_cdk::api::stable::stable64_write(8, &bytes);
    });
}

pub fn load_from_stable_memory() {
    let mut len_bytes = [0u8; 8];
    ic_cdk::api::stable::stable64_read(0, &mut len_bytes);
    let len = u64::from_le_bytes(len_bytes) as usize;
    if len == 0 {
        init_state(BotState::default());
        return;
    }
    let mut bytes = vec![0u8; len];
    ic_cdk::api::stable::stable64_read(8, &mut bytes);
    let state: BotState = serde_json::from_slice(&bytes).expect("Failed to deserialize state");
    init_state(state);
}
```

- [ ] **Step 3: Create `src/liquidation_bot/src/lib.rs`**

Canister entry point with init, upgrade, timer, and stub endpoints.

```rust
use candid::{CandidType, Deserialize, Principal};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use ic_canister_log::{declare_log_buffer, log};

mod state;
mod process;
mod swap;

use state::{BotConfig, BotLiquidationEvent, BotState, LiquidatableVaultInfo};

declare_log_buffer!(name = INFO, capacity = 1000);

#[derive(CandidType, Deserialize)]
pub struct BotInitArgs {
    pub config: BotConfig,
}

#[init]
fn init(args: BotInitArgs) {
    state::init_state(BotState {
        config: Some(args.config),
        ..Default::default()
    });
    setup_timer();
}

#[pre_upgrade]
fn pre_upgrade() {
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    setup_timer();
}

fn setup_timer() {
    ic_cdk_timers::set_timer_interval(
        std::time::Duration::from_secs(30),
        || ic_cdk::spawn(process::process_pending()),
    );
}

#[update]
fn notify_liquidatable_vaults(vaults: Vec<LiquidatableVaultInfo>) {
    let caller = ic_cdk::api::caller();
    let backend = state::read_state(|s| {
        s.config.as_ref().map(|c| c.backend_principal)
    });
    if Some(caller) != backend {
        log!(INFO, "Rejected notification from unauthorized caller: {}", caller);
        return;
    }
    let count = vaults.len();
    state::mutate_state(|s| {
        s.pending_vaults = vaults;
    });
    log!(INFO, "Received {} liquidatable vaults from backend", count);
}

#[query]
fn get_bot_stats() -> state::BotStats {
    state::read_state(|s| s.stats.clone())
}

#[query]
fn get_liquidation_events(offset: u64, limit: u64) -> Vec<BotLiquidationEvent> {
    state::read_state(|s| {
        let len = s.liquidation_events.len();
        let start = (len as u64).saturating_sub(offset + limit) as usize;
        let end = (len as u64).saturating_sub(offset) as usize;
        s.liquidation_events[start..end].to_vec()
    })
}

#[update]
fn set_config(config: BotConfig) {
    let caller = ic_cdk::api::caller();
    let is_admin = state::read_state(|s| {
        s.config.as_ref().map(|c| c.admin == caller).unwrap_or(false)
    });
    if !is_admin {
        ic_cdk::trap("Unauthorized: only admin can set config");
    }
    state::mutate_state(|s| s.config = Some(config));
}
```

- [ ] **Step 4: Add workspace member and dfx entry**

In root `Cargo.toml`, add `"src/liquidation_bot"` to the `members` array.

In `dfx.json`, add:
```json
"liquidation_bot": {
  "candid": "src/liquidation_bot/liquidation_bot.did",
  "package": "liquidation_bot",
  "type": "rust",
  "metadata": [{ "name": "candid:service" }]
}
```

- [ ] **Step 5: Create stub `src/liquidation_bot/src/process.rs`**

```rust
use crate::state;
use ic_canister_log::log;

pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };
    log!(crate::INFO, "Processing vault #{} (stub — DEX swap not yet implemented)", vault.vault_id);
    // TODO: Task 3 will implement the full flow
}
```

- [ ] **Step 6: Create stub `src/liquidation_bot/src/swap.rs`**

```rust
use candid::Principal;

#[derive(Debug)]
pub struct DexQuote {
    pub output_amount: u64,
    pub route: String,
    pub target_token: Principal,
}

#[derive(Debug)]
pub enum SwapError {
    SlippageExceeded { expected: u64, actual: u64 },
    DexCallFailed(String),
    InsufficientLiquidity,
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::SlippageExceeded { expected, actual } =>
                write!(f, "Slippage exceeded: expected {} got {}", expected, actual),
            SwapError::DexCallFailed(msg) => write!(f, "DEX call failed: {}", msg),
            SwapError::InsufficientLiquidity => write!(f, "Insufficient liquidity"),
        }
    }
}

// KongSwap integration will be implemented in Task 3
```

- [ ] **Step 7: Create `src/liquidation_bot/liquidation_bot.did`**

```candid
type BotConfig = record {
  backend_principal : principal;
  three_pool_principal : principal;
  kong_swap_principal : principal;
  treasury_principal : principal;
  admin : principal;
  max_slippage_bps : nat16;
  icp_ledger : principal;
  ckusdc_ledger : principal;
  ckusdt_ledger : principal;
  icusd_ledger : principal;
};

type BotInitArgs = record {
  config : BotConfig;
};

type BotLiquidationEvent = record {
  timestamp : nat64;
  vault_id : nat64;
  debt_covered_e8s : nat64;
  collateral_received_e8s : nat64;
  icusd_burned_e8s : nat64;
  collateral_to_treasury_e8s : nat64;
  swap_route : text;
  effective_price_e8s : nat64;
  slippage_bps : int32;
  success : bool;
  error_message : opt text;
};

type BotStats = record {
  total_debt_covered_e8s : nat64;
  total_icusd_burned_e8s : nat64;
  total_collateral_received_e8s : nat64;
  total_collateral_to_treasury_e8s : nat64;
  events_count : nat64;
};

type LiquidatableVaultInfo = record {
  vault_id : nat64;
  collateral_type : principal;
  debt_amount : nat64;
  collateral_amount : nat64;
  recommended_liquidation_amount : nat64;
  collateral_price_e8s : nat64;
};

service : (BotInitArgs) -> {
  notify_liquidatable_vaults : (vec LiquidatableVaultInfo) -> ();
  get_bot_stats : () -> (BotStats) query;
  get_liquidation_events : (nat64, nat64) -> (vec BotLiquidationEvent) query;
  set_config : (BotConfig) -> ();
};
```

- [ ] **Step 8: Verify it compiles**

Run: `cargo check -p liquidation_bot`
Expected: Compiles with warnings only (unused code in stubs)

- [ ] **Step 9: Commit**

```bash
git add src/liquidation_bot/ Cargo.toml dfx.json
git commit -m "feat: scaffold liquidation bot canister with state, stubs, and candid"
```

---

### Task 2: Backend changes — bot endpoints and budget tracking

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs`
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/src/lib.rs`
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

- [ ] **Step 1: Write tests for liquidation amount formula and budget logic**

Add to `src/rumi_protocol_backend/src/state.rs` test module:

```rust
#[test]
fn test_bot_liquidation_amount_formula() {
    // L = (T*D - C) / (T - B) where T=target CR, D=debt, C=collateral value, B=bonus
    // Vault: 1000 icUSD debt, $1400 collateral (CR=1.40), target=1.50, bonus=1.15
    let t = 1.50_f64;
    let d = 1000.0;
    let c = 1400.0;
    let b = 1.15;
    let l = (t * d - c) / (t - b);
    // L = (1500 - 1400) / (1.50 - 1.15) = 100 / 0.35 = 285.71
    assert!((l - 285.71).abs() < 0.01, "L should be ~285.71, got {}", l);

    // After liquidation: debt = 1000 - 285.71 = 714.29
    // Collateral seized = 285.71 * 1.15 = 328.57
    // Remaining collateral value = 1400 - 328.57 = 1071.43
    // New CR = 1071.43 / 714.29 = 1.50 ✓
    let new_debt = d - l;
    let seized = l * b;
    let new_collateral = c - seized;
    let new_cr = new_collateral / new_debt;
    assert!((new_cr - 1.50).abs() < 0.01, "New CR should be 1.50, got {}", new_cr);
}

#[test]
fn test_bot_budget_decrement() {
    let mut state = test_state();
    state.bot_budget_total_e8s = 1_000_000_000_000; // $10,000
    state.bot_budget_remaining_e8s = 1_000_000_000_000;

    // Simulate bot liquidating 285.71 icUSD
    let liquidation_amount = 28_571_000_000u64; // 285.71 icUSD in e8s
    assert!(state.bot_budget_remaining_e8s >= liquidation_amount);
    state.bot_budget_remaining_e8s -= liquidation_amount;
    state.bot_total_debt_covered_e8s += liquidation_amount;

    assert_eq!(state.bot_budget_remaining_e8s, 1_000_000_000_000 - 28_571_000_000);
    assert_eq!(state.bot_total_debt_covered_e8s, 28_571_000_000);
}

#[test]
fn test_bot_budget_exhausted_blocks_liquidation() {
    let mut state = test_state();
    state.bot_budget_remaining_e8s = 10_000_000; // 0.1 icUSD remaining

    let liquidation_amount = 28_571_000_000u64; // 285.71 icUSD
    assert!(state.bot_budget_remaining_e8s < liquidation_amount,
        "Budget should be insufficient");
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib -p rumi_protocol_backend -- test_bot_`
Expected: 3 tests pass (these test pure math, no state initialization issues)

- [ ] **Step 3: Add bot state fields to `state.rs`**

Add to the `State` struct (around line 491):

```rust
// Liquidation bot
pub liquidation_bot_principal: Option<Principal>,
pub bot_budget_total_e8s: u64,
pub bot_budget_remaining_e8s: u64,
pub bot_budget_start_timestamp: u64,
pub bot_total_debt_covered_e8s: u64,
pub bot_total_icusd_deposited_e8s: u64,
```

Initialize all to 0 / None in the `From<InitArg>` impl.

- [ ] **Step 4: Add bot endpoints to `main.rs`**

New endpoints:
- `set_liquidation_bot_config(principal: Principal, monthly_budget_e8s: u64)` — admin only
- `bot_liquidate(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError>` — bot only, validates budget, calculates L, executes partial liquidation, returns collateral
- `bot_deposit_to_reserves(amount_e8s: u64)` — bot only, decrements obligation
- `get_bot_stats() -> BotStatsResponse` — public query
- `reset_bot_budget()` — admin only

The `bot_liquidate` endpoint:
1. Check caller == `liquidation_bot_principal`
2. Get vault, price, compute `L = (T*D - C) / (T - B)`
3. Check `bot_budget_remaining_e8s >= L`
4. Execute partial liquidation (reuse existing `liquidate_vault_partial` internal logic)
5. Decrement budget, increment `bot_total_debt_covered_e8s`
6. Create `PendingMarginTransfer` for the bot canister to receive ICP
7. Return `BotLiquidationResult { collateral_amount, debt_covered, collateral_price }`

- [ ] **Step 5: Enrich notification payload in `lib.rs`**

Modify `LiquidatableVaultInfo` to include:
```rust
pub recommended_liquidation_amount: u64,
pub collateral_price_e8s: u64,
```

In `check_vaults()`, compute `L` for each vault and include it. Also add a second fire-and-forget call to the bot canister (same pattern as stability pool notification).

- [ ] **Step 6: Update `.did` file**

Add new types (`BotLiquidationResult`, `BotStatsResponse`) and endpoints to `rumi_protocol_backend.did`.

- [ ] **Step 7: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend`
Expected: Compiles (warnings OK)

- [ ] **Step 8: Run all existing tests**

Run: `cargo test --lib`
Expected: All 119+ tests pass

- [ ] **Step 9: Commit**

```bash
git add src/rumi_protocol_backend/
git commit -m "feat: add bot liquidation endpoints, budget tracking, and enriched notifications"
```

---

## Chunk 2: DEX Integration + Processing Logic

### Task 3: KongSwap integration

**Files:**
- Modify: `src/liquidation_bot/src/swap.rs`

- [ ] **Step 1: Implement KongSwap swap**

Add KongSwap candid types and the `swap_icp_for_stable` function. Key flow:
1. Call `swap_amounts(pay_token: "ICP", pay_amount, receive_token: "ckUSDC")` to get quote
2. Call `swap_amounts(pay_token: "ICP", pay_amount, receive_token: "ckUSDT")` to get quote
3. Pick better rate
4. Call `swap(SwapArgs)` with `max_slippage` from config
5. Return `SwapResult { output_amount, route, target_token }`

The KongSwap canister uses text identifiers for tokens (symbol or canister ID). We'll use canister ID format for precision.

Also implement `swap_stable_for_icusd` which calls our 3pool:
1. Approve 3pool to spend ckStable (ICRC-2 approve on the stablecoin ledger)
2. Call 3pool `swap(token_index, 0, amount, min_output)` where token_index is 1 (ckUSDT) or 2 (ckUSDC)
3. Return icUSD amount received

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p liquidation_bot`

- [ ] **Step 3: Commit**

```bash
git add src/liquidation_bot/src/swap.rs
git commit -m "feat: implement KongSwap + 3pool swap integration with best-rate selection"
```

---

### Task 4: Full processing logic

**Files:**
- Modify: `src/liquidation_bot/src/process.rs`

- [ ] **Step 1: Implement `process_pending()`**

Full flow per vault:
```rust
pub async fn process_pending() {
    let vault = state::mutate_state(|s| s.pending_vaults.pop());
    let Some(vault) = vault else { return };

    let config = state::read_state(|s| s.config.clone())
        .expect("Bot not configured");

    // 1. Call bot_liquidate on backend
    let liq_result = call_bot_liquidate(&config, vault.vault_id).await;
    let (collateral_amount, debt_covered, collateral_price) = match liq_result {
        Ok(r) => (r.collateral_amount, r.debt_covered, r.collateral_price),
        Err(e) => {
            log_failed_event(&vault, &format!("bot_liquidate failed: {}", e));
            return; // Don't retry — will fall through to stability pool
        }
    };

    // 2. Swap ICP → ckStable (best of ckUSDC/ckUSDT on KongSwap)
    let swap_amount = calculate_swap_amount(collateral_amount, debt_covered, collateral_price);
    let stable_result = swap::swap_icp_for_stable(&config, swap_amount).await;
    let (stable_amount, stable_token, route) = match stable_result {
        Ok(r) => (r.output_amount, r.target_token, r.route),
        Err(e) => {
            log_failed_event(&vault, &format!("DEX swap failed: {}", e));
            return; // ICP stays in bot — admin can recover manually
        }
    };

    // 3. Swap ckStable → icUSD (via 3pool)
    let icusd_result = swap::swap_stable_for_icusd(&config, stable_amount, stable_token).await;
    let icusd_amount = match icusd_result {
        Ok(amount) => amount,
        Err(e) => {
            log_failed_event(&vault, &format!("3pool swap failed: {}", e));
            return;
        }
    };

    // 4. Deposit icUSD to backend reserves
    if let Err(e) = call_bot_deposit_to_reserves(&config, icusd_amount).await {
        log_failed_event(&vault, &format!("deposit_to_reserves failed: {}", e));
        return;
    }

    // 5. Send remaining ICP to treasury
    let icp_to_treasury = collateral_amount - swap_amount;
    if icp_to_treasury > 0 {
        let _ = transfer_icp_to_treasury(&config, icp_to_treasury).await;
    }

    // 6. Log success event
    let effective_price = if swap_amount > 0 {
        (stable_amount as u128 * 100_000_000 / swap_amount as u128) as u64
    } else { 0 };
    let slippage_bps = calculate_slippage(effective_price, collateral_price);

    state::mutate_state(|s| {
        s.stats.total_debt_covered_e8s += debt_covered;
        s.stats.total_icusd_burned_e8s += icusd_amount;
        s.stats.total_collateral_received_e8s += collateral_amount;
        s.stats.total_collateral_to_treasury_e8s += icp_to_treasury;
        s.stats.events_count += 1;
        s.liquidation_events.push(BotLiquidationEvent {
            timestamp: ic_cdk::api::time(),
            vault_id: vault.vault_id,
            debt_covered_e8s: debt_covered,
            collateral_received_e8s: collateral_amount,
            icusd_burned_e8s: icusd_amount,
            collateral_to_treasury_e8s: icp_to_treasury,
            swap_route: route,
            effective_price_e8s: effective_price,
            slippage_bps,
            success: true,
            error_message: None,
        });
    });
}
```

- [ ] **Step 2: Implement helper functions**

`call_bot_liquidate`, `call_bot_deposit_to_reserves`, `transfer_icp_to_treasury`, `calculate_swap_amount`, `calculate_slippage`, `log_failed_event`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p liquidation_bot`

- [ ] **Step 4: Commit**

```bash
git add src/liquidation_bot/src/process.rs
git commit -m "feat: implement full liquidation processing flow with failure escalation"
```

---

## Chunk 3: Frontend Tab + Header Indicator

### Task 5: Liquidation Bot frontend tab

**Files:**
- Create: `src/vault_frontend/src/lib/components/liquidations/LiquidationBotTab.svelte`
- Modify: `src/vault_frontend/src/routes/liquidations/+page.svelte`

- [ ] **Step 1: Create `LiquidationBotTab.svelte`**

Component shows:
- **Status card**: Budget remaining / total, days left in fiscal month
- **All-time stats**: Debt covered, icUSD burned, deficit, ICP to treasury
- **Event log**: Paginated table of `BotLiquidationEvent`s, newest first

Queries `get_bot_stats()` on the backend and `get_bot_stats()` / `get_liquidation_events()` on the bot canister.

- [ ] **Step 2: Add "Liquidation Bot" as first tab**

In `+page.svelte`:
- Add `'bot'` to the Tab type: `type Tab = 'bot' | 'pool' | 'manual'`
- Default to `'bot'` tab
- Add the tab button before Stability Pool
- Import and render `LiquidationBotTab`

- [ ] **Step 3: Verify frontend builds**

Run: `npm run build --prefix src/vault_frontend`

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/
git commit -m "feat: add Liquidation Bot tab to /liquidations page"
```

---

### Task 6: Header indicator for unhandled liquidatable vaults

**Files:**
- Modify: `src/vault_frontend/src/routes/+layout.svelte`

- [ ] **Step 1: Add liquidation alert indicator**

In the app header/nav, add a small indicator (pulsing dot or icon) that appears when there are liquidatable vaults that neither the bot nor the stability pool has handled. This links to the manual liquidations tab (`/liquidations?tab=manual`).

The indicator queries `get_all_vaults()` (already called by existing pages) and checks if any vault has CR below the liquidation threshold.

- [ ] **Step 2: Verify frontend builds**

Run: `npm run build --prefix src/vault_frontend`

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/+layout.svelte
git commit -m "feat: add header indicator for unhandled liquidatable vaults"
```

---

## Chunk 4: Integration + Final Verification

### Task 7: Wire everything together and verify

- [ ] **Step 1: Run full test suite**

Run: `cargo test --lib`
Expected: All tests pass (119+ existing + new bot tests)

- [ ] **Step 2: Build all canisters**

Run: `cargo build --target wasm32-unknown-unknown --release -p liquidation_bot -p rumi_protocol_backend`

- [ ] **Step 3: Build frontend**

Run: `npm run build --prefix src/vault_frontend`

- [ ] **Step 4: Final commit**

```bash
git commit -m "chore: verify all canisters build and tests pass"
```

- [ ] **Step 5: Push branch**

```bash
git push -u origin feat/liquidation-bot
```
