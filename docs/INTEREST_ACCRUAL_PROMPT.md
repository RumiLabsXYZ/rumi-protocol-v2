# Prompt: Implement Global Debt Multiplier & Interest Accrual

## Context

Rumi Protocol is a CDP (collateralized debt position) protocol on ICP. Users deposit collateral (ICP, ckBTC, ckETH, soon ckXAUT) and borrow icUSD against it. The protocol currently has NO interest accrual — `interest_rate_apr` exists as a field on `CollateralConfig` but nothing reads it. It's hardcoded to 0.0 when adding collateral via `add_collateral_token`.

We need to implement this NOW before more money enters the protocol, because retrofitting accrual onto existing vaults with outstanding debt is messy.

## Problem 1: No Interest Accrual

The `Vault` struct stores a flat `borrowed_icusd_amount` that never grows. If we set `interest_rate_apr` to 0.02, nothing happens. We need debt to compound over time.

## Problem 2: `check_vaults()` Doesn't Scale

`check_vaults()` in `src/rumi_protocol_backend/src/lib.rs:208` runs every 300 seconds (on each XRC price fetch). It iterates ALL vaults, clones them, and computes collateral ratios. This works fine with a few hundred vaults but will burn excessive cycles at scale. Adding per-vault interest accrual inside this loop would make it worse.

## Solution: Global Debt Multiplier (MakerDAO-style)

Instead of tracking per-vault timestamps and compounding individually, use a per-collateral-type cumulative debt multiplier:

### How it works:
1. Each `CollateralConfig` gets a new field: `cumulative_debt_multiplier: Decimal` (starts at 1.0)
2. Each `Vault` gets a new field: `debt_multiplier_snapshot: Decimal` (set to the current cumulative multiplier at time of borrow/repay/any interaction)
3. A vault's **effective debt** = `stored_debt * (current_cumulative_multiplier / vault_snapshot)`
4. The cumulative multiplier is updated once per collateral type on the 300s timer: `multiplier *= (1 + rate * elapsed_seconds / SECONDS_PER_YEAR)`
5. When a user interacts with their vault (borrow, repay, etc.), normalize: set `stored_debt = effective_debt`, `snapshot = current_multiplier`

### Why this is better:
- Updating the multiplier is O(1) per collateral type, not O(n) per vault
- `check_vaults()` just does one extra multiplication per vault (cheap) with no writes
- Interest accrual is always precise — no "last accrual" drift

## What Needs to Change

### `AddCollateralArg` (src/rumi_protocol_backend/src/lib.rs)
Add these three fields that are currently hardcoded in `add_collateral_token`:
- `interest_rate_apr: f64` (currently hardcoded to 0.0)
- `redemption_fee_floor: f64` (currently hardcoded to 0.005)
- `redemption_fee_ceiling: f64` (currently hardcoded to 0.05)

### `CollateralConfig` (src/rumi_protocol_backend/src/state.rs)
Add:
- `cumulative_debt_multiplier: Decimal` (init to 1.0)
- `last_multiplier_update: u64` (timestamp nanos)

### `Vault` struct (src/rumi_protocol_backend/src/vault.rs)
Add:
- `debt_multiplier_snapshot: Decimal` (set to collateral's current multiplier on any interaction)

### `add_collateral_token` (src/rumi_protocol_backend/src/main.rs ~line 1506)
- Use `arg.interest_rate_apr` instead of hardcoded 0.0
- Use `arg.redemption_fee_floor` instead of hardcoded 0.005
- Use `arg.redemption_fee_ceiling` instead of hardcoded 0.05
- Init `cumulative_debt_multiplier` to 1.0
- Init `last_multiplier_update` to current time

### Timer / price fetch (src/rumi_protocol_backend/src/xrc.rs)
On each 300s tick, for each collateral type with interest_rate_apr > 0:
- Calculate elapsed time since `last_multiplier_update`
- Update `cumulative_debt_multiplier *= (1 + rate * elapsed / YEAR_SECONDS)`
- Update `last_multiplier_update`

### Every place that reads `vault.borrowed_icusd_amount`
Must use effective debt instead: `stored_debt * (current_multiplier / vault_snapshot)`. Key locations:
- `compute_collateral_ratio()` — CR calculation
- `check_vaults()` — liquidation eligibility
- `repay_to_vault` / `partial_repay_to_vault` — repayment logic
- `borrow_from_vault` — additional borrowing
- `close_vault` — full repayment
- `partial_liquidate_vault` / liquidation functions
- `redeem` — redemption logic
- Any query endpoints that return vault debt (e.g. `get_vault`)

### Every place that writes `vault.borrowed_icusd_amount`
Must normalize (set stored_debt = effective_debt, snapshot = current_multiplier):
- After borrow
- After repay
- After liquidation
- After redemption

### Migration for existing vaults
In `post_upgrade`:
- Set `cumulative_debt_multiplier = 1.0` for all existing collateral configs
- Set `last_multiplier_update = ic_cdk::api::time()` for all existing collateral configs
- Set `debt_multiplier_snapshot = 1.0` for all existing vaults
- This means existing vaults start accruing from the upgrade moment, no retroactive interest

## Key Files
- `src/rumi_protocol_backend/src/main.rs` — `add_collateral_token`, `post_upgrade`
- `src/rumi_protocol_backend/src/state.rs` — `CollateralConfig` struct, state management
- `src/rumi_protocol_backend/src/vault.rs` — `Vault` struct, all vault operations
- `src/rumi_protocol_backend/src/lib.rs` — `check_vaults()`, `AddCollateralArg`
- `src/rumi_protocol_backend/src/xrc.rs` — timer tick, price fetch
- `src/rumi_protocol_backend/src/numeric.rs` — math helpers
- `src/rumi_protocol_backend/src/event.rs` — event recording

## Critical Rules
- **NEVER reinstall the backend canister** — upgrades only. Reinstalling wipes all on-chain state.
- **NEVER squash merge** — always use regular merge commits.
- Stale operation threshold in state.rs should be 10 minutes (not 3).
- The `#[ic_cdk::heartbeat]` macro was removed intentionally — do NOT re-add it. Timers only.
- Test on local replica before mainnet. User has hundreds of ICP in live vaults.

## Branch
Work on branch: `feat/interest-accrual-debt-multiplier` (create from `main`, NOT from `feat/add-collateral-assets`)
