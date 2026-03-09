# Liquidation Improvements Design

## Feature 1: Record Collateral Price in Liquidation History

The stability pool's `PoolLiquidationRecord` has a `collateral_price_e8s` field that is hardcoded to `Some(0)`. The backend already knows the price at liquidation time but doesn't return it.

### Changes

**Backend (`main.rs`):** Add `collateral_price_e8s: u64` to `StabilityPoolLiquidationResult`. Populate it from `get_collateral_price_decimal()` which is already called during `stability_pool_liquidate()`.

**Stability pool (`liquidation.rs`):** Read `collateral_price_e8s` from the backend response. Pass it through to `process_liquidation_gains()` / `process_liquidation_gains_at()` so it gets written into the `PoolLiquidationRecord` instead of `Some(0)`.

**Stability pool (`state.rs`):** Add `collateral_price_e8s` parameter to `process_liquidation_gains()` and `process_liquidation_gains_at()`. Update all call sites and tests.

Backward compatible — the record field is already `Option<u64>`.

## Feature 2: All Vaults on Liquidation Page

### Backend

New query endpoint `get_all_vaults() -> Vec<CandidVault>` that returns every open vault. Add to `.did` file as well.

### Frontend (`/liquidations/+page.svelte`)

- Fetch both `get_liquidatable_vaults()` and `get_all_vaults()` on load and every 60s
- Top section: liquidatable vaults at full opacity with the existing action UI (input + liquidate button)
- Visual divider between sections
- Bottom section: remaining vaults (filtered out liquidatable ones) sorted by CR ascending, rendered at ~70% opacity, same card layout but without input/button
- Section headers to label each group
