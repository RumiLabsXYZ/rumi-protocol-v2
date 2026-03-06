# Multi-Collateral Refactor Plan

## Context

The Rumi Protocol backend is live on ICP mainnet with a single collateral type (ICP). One active user has real vaults and state. The protocol uses event sourcing — all state is rebuilt from an event log on every canister upgrade. This refactor parameterizes the internals so that adding a new collateral token later requires only calling an admin function, not changing code. ICP remains the only collateral after this refactor.

**Critical constraint**: Canister upgrade with existing state. Event replay must remain backward-compatible. External behavior for ICP vaults must not change.

**Branch**: `feature/multi-collateral-refactor` off `main`.

---

## Phase 1: New Types & CollateralConfig

### 1a. Core types (`state.rs`)

**CollateralType** = `Principal` (ledger canister ID uniquely identifies each token)

**CollateralStatus** enum — graduated severity:
```
Active      — full functionality
Paused      — no new borrows/vaults; repay, withdraw, close still work
Frozen      — HARD STOP. Nothing works except admin actions. Emergency brake.
Sunset      — winding down: repay and close only, no new activity
Deprecated  — fully wound down, read-only
```

**PriceSource** enum:
```
Xrc { base_asset: String, quote_asset: String }
// Extensible for future oracle types
```

**CollateralConfig** struct:
```
ledger_canister_id: Principal
decimals: u8                        // Fetched from icrc1_decimals() on add, not manual
liquidation_ratio: Ratio            // e.g., 1.33 — below this, vault is liquidatable
minimum_collateral_ratio: Ratio     // e.g., 1.5 — below this, recovery mode triggers
liquidation_bonus: Ratio            // e.g., 1.15
borrowing_fee: Ratio                // one-time fee at mint, e.g., 0.005
interest_rate_apr: Ratio            // ongoing rate (default 0.0 for now, accrual added later)
debt_ceiling: u64                   // u64::MAX = no cap
min_vault_debt: ICUSD               // dust threshold
ledger_fee: u64                     // transfer fee in native units
price_source: PriceSource
status: CollateralStatus
last_price: Option<Decimal>         // USD per 1 whole token
last_price_timestamp: Option<u64>
redemption_fee_floor: Ratio         // min redemption fee (e.g., 0.5%)
redemption_fee_ceiling: Ratio       // max redemption fee (e.g., 5%)
current_base_rate: Ratio            // dynamic rate that spikes on redemption, decays over time
last_redemption_time: u64
recovery_target_cr: Ratio           // e.g., 1.55 — liquidation restores vault to this CR
```

### 1b. Vault struct changes (`vault.rs`)

```rust
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    #[serde(alias = "icp_margin_amount")]
    pub collateral_amount: u64,         // Raw amount in token's native precision
    pub vault_id: u64,
    #[serde(default = "default_collateral_type")]
    pub collateral_type: Principal,     // Ledger canister ID; defaults to ICP for legacy events
}
```

- `collateral_amount` is raw `u64` — different tokens have different decimal precisions (ICP=8, ckUSDC=6). The `decimals` field in CollateralConfig tells how to interpret it.
- `#[serde(alias = "icp_margin_amount")]` — deserializes old events that used the old field name into the new field. New events going forward write `collateral_amount`. This exact pattern is already used in this codebase for PartialLiquidateVault event fields.
- `#[serde(default = "default_collateral_type")]` returns ICP ledger principal for old events lacking this field.

### 1c. CandidVault update (`vault.rs`)

Add `collateral_type: Principal` and `collateral_amount: u64`. Keep `icp_margin_amount: u64` populated with same value for frontend backward compat.

### 1d. Collateral value helper (`numeric.rs`)

```rust
/// Convert raw collateral amount to USD value using price and decimals
fn collateral_usd_value(amount: u64, price_usd: Decimal, decimals: u8) -> Decimal {
    Decimal::from(amount) / Decimal::from(10u64.pow(decimals as u32)) * price_usd
}
```

---

## Phase 2: State Struct Changes (`state.rs`)

**New fields on State:**
```
collateral_configs: BTreeMap<Principal, CollateralConfig>
collateral_to_vault_ids: BTreeMap<Principal, BTreeSet<u64>>
```

**Keep existing global fields** (`fee`, `liquidation_bonus`, `last_icp_rate`, etc.) — needed for backward-compatible event replay of old admin events. During replay, old events set globals AND sync to ICP's CollateralConfig. New runtime code reads exclusively from CollateralConfig.

**New helper methods:**
- `get_collateral_config(ct) -> Option<&CollateralConfig>`
- `get_borrowing_fee_for(ct) -> Ratio`
- `get_liquidation_bonus_for(ct) -> Ratio`
- `get_liquidation_ratio_for(ct) -> Ratio`
- `get_min_collateral_ratio_for(ct) -> Ratio`
- `get_price_for(ct) -> Option<Decimal>`
- `get_redemption_fee_for(ct, amount) -> Ratio`
- `total_debt_for_collateral(ct) -> ICUSD`
- `total_collateral_value_for(ct) -> Decimal` (normalized by decimals)
- `total_collateral_for(ct) -> u64` (raw sum)

**Deprecate**: `total_icp_margin_amount()` — replace all internal usage with `total_collateral_for(ICP_LEDGER)`. Keep as thin wrapper only if needed by dashboard/metrics.

**TCR computation**: `compute_total_collateral_ratio()` sums across ALL collateral types:
```
TCR = Σ(collateral_usd_value_per_type) / Σ(debt_across_all_vaults)
```

**Migration in `State::from(InitArg)`:**
Auto-create ICP CollateralConfig with current defaults (liquidation_ratio=1.33, minimum_collateral_ratio=1.5, bonus=1.15, fee=0.005, decimals=8, XRC price source, Active status, interest_rate_apr=0.0).

---

## Phase 3: Event System Changes (`event.rs`)

**New event variants:**
```
AddCollateralType { collateral_type, config }
UpdateCollateralStatus { collateral_type, status }
UpdateCollateralConfig { collateral_type, config }
```

**Replay handler updates:**
- `Init` → creates ICP CollateralConfig via `State::from()`
- `OpenVault` → inserts vault_id into `collateral_to_vault_ids[vault.collateral_type]`
- Old fee-setting events (`SetBorrowingFee`, `SetLiquidationBonus`, etc.) → set global fields (as before) AND sync same value to `collateral_configs[ICP]`. This is NOT about other tokens — it's about keeping the old global events consistent with the new per-collateral config for ICP specifically during replay.
- New events → directly insert/update `collateral_configs`

**Backward compat**: No changes to existing event shapes. Vault in OpenVault events gets `collateral_type` via serde default and `collateral_amount` via serde alias.

---

## Phase 4: Parameterize Vault Operations (`vault.rs`)

**API approach**: Modify `open_vault` to accept optional `collateral_type` at Candid boundary for backward compat. Internally, ALL code passes `collateral_type` explicitly — ICP is never special-cased inside the Rust code.

### Per-function changes:

**`open_vault(amount, opt collateral_type)`**:
- Resolve collateral_type (default ICP at API boundary only)
- Look up CollateralConfig; check status is Active
- Check debt ceiling; use config's ledger for transfer_from
- Create Vault with explicit `collateral_type`
- Insert into `collateral_to_vault_ids`

**`borrow_from_vault(arg)`**:
- Look up vault's collateral_type → CollateralConfig
- Read `liquidation_ratio`, `borrowing_fee` from config
- Check status allows borrowing (Active only)
- Check debt ceiling
- Normalize collateral value using `decimals` for CR calculation

**`withdraw_collateral` / `withdraw_partial_collateral`**:
- Look up vault's collateral_type → CollateralConfig
- Use config's ledger and `ledger_fee` for transfer
- Use per-collateral `liquidation_ratio` for partial withdrawal limit

**`repay_to_vault`**: Check status allows repayment (Active, Paused, Sunset — not Frozen/Deprecated).

**`liquidate_vault` / `liquidate_vault_partial`**:
- Look up vault's collateral_type → CollateralConfig
- Use per-collateral `liquidation_bonus`, `liquidation_ratio`, price
- Use config's ledger for collateral transfer to liquidator
- Check status allows liquidation (Active, Paused — not Frozen)

**`redeem_collateral(collateral_type, amount)`** — new generic function:
- Filter vaults by collateral_type, sort by CR ascending
- Use per-collateral redemption fee params and ledger
- `redeem_icp(amount)` becomes thin wrapper calling `redeem_collateral(ICP_LEDGER, amount)`
- Frontend can use either; over time migrate to `redeem_collateral`

### Status enforcement matrix:

| Operation         | Active | Paused | Frozen | Sunset | Deprecated |
|-------------------|--------|--------|--------|--------|------------|
| Open vault        | yes    | no     | no     | no     | no         |
| Borrow            | yes    | no     | no     | no     | no         |
| Repay             | yes    | yes    | no     | yes    | no         |
| Add collateral    | yes    | yes    | no     | no     | no         |
| Withdraw          | yes    | no     | no     | yes    | no         |
| Close             | yes    | yes    | no     | yes    | no         |
| Liquidate         | yes    | yes    | no     | no     | no         |
| Redeem            | yes    | no     | no     | no     | no         |

Note: Close requires zero debt AND zero collateral. Since withdraw is blocked in Paused, close only works for already-empty vaults. This prevents collateral outflows during a pause while still allowing debt repayment (which improves system health).

---

## Phase 5: Transfer & Oracle Parameterization

### `management.rs`
- Add `transfer_collateral(amount, to, ledger)` and `transfer_collateral_from(amount, from, ledger)`
- Existing `transfer_icp()` / `transfer_icp_from()` become thin wrappers passing `icp_ledger_principal`

### `xrc.rs`
- Add `ensure_fresh_price_for(collateral_type)`:
  - Look up PriceSource from CollateralConfig
  - For Xrc: call XRC with configured asset pair
  - Store in CollateralConfig's `last_price` / `last_price_timestamp`
  - Also update `State.last_icp_rate` when collateral is ICP (backward compat for dashboard/metrics)
- Existing `ensure_fresh_price()` calls `ensure_fresh_price_for(ICP_LEDGER)`
- Timer: for now, only ICP timer runs. When a new collateral is added, its timer is registered in `add_collateral_token`.

### `main.rs` — `validate_call()`
- Keep as-is for now (basic caller validation + ICP price). Per-operation price fetch by collateral type supplements it.

---

## Phase 6: Admin Functions (`main.rs`)

**New endpoints (developer-only):**
```
add_collateral_token(AddCollateralArg) -> Result<(), ProtocolError>
  — Queries icrc1_decimals() from ledger to populate decimals field
  — Creates CollateralConfig, records AddCollateralType event
  — Registers price-fetching timer for new collateral

set_collateral_status(Principal, CollateralStatus) -> Result<(), ProtocolError>
  — Freeze/pause/sunset/deprecate. Records UpdateCollateralStatus event.

update_collateral_config(Principal, CollateralConfigUpdate) -> Result<(), ProtocolError>
  — Update any per-collateral param. Records UpdateCollateralConfig event.

get_collateral_config(Principal) -> Option<CollateralConfig>              [query]
get_supported_collateral_types() -> Vec<(Principal, CollateralStatus)>    [query]
```

**Existing admin functions** (`set_borrowing_fee`, etc.):
- Keep working, affect ICP only
- Going forward, use `update_collateral_config` for all collateral types including ICP

---

## Phase 7: Migration & Upgrade Safety

Migration is automatic during event replay:
1. `Init` → `State::from(InitArg)` creates ICP CollateralConfig
2. Old admin events → replay syncs to ICP CollateralConfig
3. OpenVault events → Vault gets `collateral_type=ICP` (serde default), `collateral_amount` from old `icp_margin_amount` (serde alias). Vault ID added to `collateral_to_vault_ids[ICP]`.
4. All other events replay identically

**post_upgrade**: After replay, validate `collateral_configs` contains ICP and all vaults have a valid collateral_type. Log warnings for inconsistencies.

No new migration event needed — Init handler + serde defaults handle everything.

---

## Phase 8: Candid & Frontend

- `open_vault(u64, opt principal)` — optional collateral_type, defaults to ICP
- `redeem_collateral(principal, u64)` — new generic redemption
- `redeem_icp(u64)` — kept as convenience wrapper
- Admin endpoints from Phase 6
- CandidVault: adds `collateral_type`, `collateral_amount`; keeps `icp_margin_amount`
- `.did` file updated

---

## Implementation Sequence

1. Create branch `feature/multi-collateral-refactor`
2. **Types**: CollateralStatus, PriceSource, CollateralConfig in `state.rs`
3. **Vault struct**: `collateral_type` + `collateral_amount` with serde compat in `vault.rs`
4. **State struct**: `collateral_configs`, `collateral_to_vault_ids`, helpers in `state.rs`
5. **Events**: new variants + replay updates in `event.rs`
6. **Management**: generic transfer functions in `management.rs`
7. **Vault operations**: parameterize to read from CollateralConfig in `vault.rs`
8. **Admin endpoints** in `main.rs`
9. **Oracle**: parameterize in `xrc.rs`
10. **Candid**: update `.did` file and endpoint signatures
11. **Tests**: migration, freeze, new-collateral, existing tests pass
12. `cargo build && cargo test`

---

## Files Modified

| File | Changes |
|------|---------|
| `state.rs` | CollateralConfig, CollateralStatus, PriceSource; new State fields; helpers; migration |
| `vault.rs` | Vault struct; all operations parameterized; status enforcement |
| `event.rs` | New event variants; replay updates |
| `main.rs` | Admin endpoints; opt collateral_type on open_vault; redeem_collateral |
| `lib.rs` | ICP_LEDGER_PRINCIPAL constant; AddCollateralArg; status matrix |
| `management.rs` | Generic transfer_collateral / transfer_collateral_from |
| `xrc.rs` | Parameterized price fetching |
| `numeric.rs` | collateral_usd_value helper |
| `dashboard.rs` | Per-collateral stats |
| `rumi_protocol_backend.did` | New Candid types and endpoints |

---

## Verification

1. `cargo build` — compiles cleanly
2. `cargo test` — all existing tests pass
3. Event replay test: old events deserialize correctly, vaults get collateral_type=ICP
4. Migration test: Init → OpenVault → SetBorrowingFee → replay, ICP config has synced values
5. New collateral test: add_collateral_token stores config
6. Freeze test: Frozen collateral blocks everything; Paused blocks borrows but allows repay
7. Local deploy: `dfx deploy` and verify ICP operations work identically
