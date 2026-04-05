# Redemption Event Enrichment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enrich the `RedemptionOnVaults` event with collateral type and per-vault impact data so that (1) event replay uses the correct collateral type instead of hardcoding ICP, and (2) the explorer shows per-vault debt reduction and collateral seized instead of the total redemption amount.

**Architecture:** Add `collateral_type: Option<CollateralType>` and `vault_impacts: Option<Vec<VaultRedemptionImpact>>` to the `RedemptionOnVaults` event. Make `redeem_on_vaults()` and `distribute_redemption_across_band()` return per-vault deltas. Update the replay handler to apply stored deltas directly (with fallback to water-filling for old events). Update the `is_vault_related` filter and both frontend formatters.

**Tech Stack:** Rust (IC canister backend), Svelte/TypeScript (frontend), Candid serialization

---

## Task 1: Add `VaultRedemptionImpact` struct and enrich the Event enum

### Step 1.1: Define the `VaultRedemptionImpact` struct

**File:** `src/rumi_protocol_backend/src/state.rs`

Add after the `PendingMarginTransfer` struct (around line 520):

```rust
/// Per-vault breakdown of a redemption: how much debt was reduced and collateral seized.
#[derive(candid::CandidType, Copy, Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct VaultRedemptionImpact {
    pub vault_id: u64,
    /// icUSD debt reduced from this vault (e8s)
    pub debt_reduced: u64,
    /// Collateral seized from this vault (smallest unit, e.g. e8s for ICP)
    pub collateral_seized: u64,
}
```

### Step 1.2: Add new fields to `RedemptionOnVaults` event

**File:** `src/rumi_protocol_backend/src/event.rs`

Change the `RedemptionOnVaults` variant (lines 71-80) from:

```rust
    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        owner: Principal,
        current_icp_rate: UsdIcp,
        icusd_amount: ICUSD,
        fee_amount: ICUSD,
        icusd_block_index: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },
```

To:

```rust
    #[serde(rename = "redemption_on_vaults")]
    RedemptionOnVaults {
        owner: Principal,
        current_icp_rate: UsdIcp,
        icusd_amount: ICUSD,
        fee_amount: ICUSD,
        icusd_block_index: u64,
        /// Which collateral type was redeemed. None for old events (pre-tiering).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collateral_type: Option<CollateralType>,
        /// Per-vault breakdown: debt reduced and collateral seized. None for old events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vault_impacts: Option<Vec<VaultRedemptionImpact>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
    },
```

Add `VaultRedemptionImpact` to the import at the top of `event.rs`:

```rust
use crate::state::{CollateralConfig, CollateralStatus, CollateralType, PendingMarginTransfer, RateCurveV2, State, VaultRedemptionImpact};
```

### Step 1.3: Verify it compiles

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo check -p rumi_protocol_backend 2>&1
```

Expected: Compile errors in places that construct or destructure `RedemptionOnVaults` (the replay handler, `record_redemption_on_vaults`, event filters). These will be fixed in subsequent tasks.

### Step 1.4: Fix all compile errors from the new fields

There are several places that construct or pattern-match on `RedemptionOnVaults`. Add the new fields everywhere:

**File: `src/rumi_protocol_backend/src/event.rs`** — `record_redemption_on_vaults` function (around line 1182)

The event recording currently looks like:

```rust
    record_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: ct_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
        timestamp: Some(now()),
    });
```

Change to (we'll populate `vault_impacts` properly in Task 2, use `None` for now to get it compiling):

```rust
    record_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: ct_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
        collateral_type: Some(redeem_ct),
        vault_impacts: None, // populated in Task 2
        timestamp: Some(now()),
    });
```

**File: `src/rumi_protocol_backend/src/event.rs`** — replay handler (around line 654)

The destructure pattern currently has `..`. The `..` catch-all means the new fields are silently ignored. Explicitly destructure them for replay:

```rust
            Event::RedemptionOnVaults {
                owner,
                current_icp_rate,
                icusd_amount,
                fee_amount,
                icusd_block_index,
                collateral_type,
                vault_impacts,
                ..
            } => {
```

Keep the existing replay body for now — we'll update it in Task 3.

### Step 1.5: Verify it compiles cleanly

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo check -p rumi_protocol_backend 2>&1
```

Expected: Clean compile (possibly with unused variable warnings for `collateral_type` and `vault_impacts`).

### Step 1.6: Run tests

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib 2>&1
```

Expected: 55 passed, 1 pre-existing failure (IC sandbox).

### Step 1.7: Commit

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/event.rs
git commit -m "feat(backend): add collateral_type and vault_impacts to RedemptionOnVaults event

Add VaultRedemptionImpact struct and two new optional fields to the
RedemptionOnVaults event for correct replay and per-vault explorer display.
Both fields use serde(default) for backward compat with old events.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 2: Make water-filling return per-vault deltas

### Step 2.1: Change `distribute_redemption_across_band` to return deltas

**File:** `src/rumi_protocol_backend/src/state.rs`

Change the signature and body of `distribute_redemption_across_band` (around line 2441):

From:
```rust
    fn distribute_redemption_across_band(
        &mut self,
        vault_ids: &[VaultId],
        debts: &[u128],
        total_debt: u128,
        redemption_e8s: u128,
        price: Decimal,
        decimals: u8,
    ) {
```

To:
```rust
    fn distribute_redemption_across_band(
        &mut self,
        vault_ids: &[VaultId],
        debts: &[u128],
        total_debt: u128,
        redemption_e8s: u128,
        price: Decimal,
        decimals: u8,
    ) -> Vec<VaultRedemptionImpact> {
```

Change the body — add a `results` vec, collect deltas, and return:

```rust
    fn distribute_redemption_across_band(
        &mut self,
        vault_ids: &[VaultId],
        debts: &[u128],
        total_debt: u128,
        redemption_e8s: u128,
        price: Decimal,
        decimals: u8,
    ) -> Vec<VaultRedemptionImpact> {
        let mut results = Vec::new();
        if total_debt == 0 || redemption_e8s == 0 {
            return results;
        }

        let mut distributed: u128 = 0;
        for (i, vault_id) in vault_ids.iter().enumerate() {
            let vault_debt = debts[i];
            // Proportional share: redemption_e8s * vault_debt / total_debt
            let share = if i == vault_ids.len() - 1 {
                // Last vault gets the remainder to avoid rounding dust
                redemption_e8s - distributed
            } else {
                redemption_e8s * vault_debt / total_debt
            };

            if share == 0 {
                continue;
            }

            // Cap at vault's actual debt
            let vault = self.vault_id_to_vaults.get(vault_id).unwrap();
            let max_share = vault.borrowed_icusd_amount.to_u64() as u128;
            let actual_share = share.min(max_share);

            let icusd_to_deduct = ICUSD::from(actual_share as u64);
            let collateral_to_deduct = crate::numeric::icusd_to_collateral_amount(
                icusd_to_deduct,
                price,
                decimals,
            );
            self.deduct_amount_from_vault(collateral_to_deduct, icusd_to_deduct, *vault_id);
            results.push(VaultRedemptionImpact {
                vault_id: *vault_id,
                debt_reduced: actual_share as u64,
                collateral_seized: collateral_to_deduct,
            });
            distributed += actual_share;
        }
        results
    }
```

### Step 2.2: Change `redeem_on_vaults` to collect and return all deltas

**File:** `src/rumi_protocol_backend/src/state.rs`

Change the signature of `redeem_on_vaults` (around line 2308):

From:
```rust
    pub fn redeem_on_vaults(
        &mut self,
        icusd_amount: ICUSD,
        collateral_price: UsdIcp,
        collateral_type: &CollateralType,
    ) {
```

To:
```rust
    pub fn redeem_on_vaults(
        &mut self,
        icusd_amount: ICUSD,
        collateral_price: UsdIcp,
        collateral_type: &CollateralType,
    ) -> Vec<VaultRedemptionImpact> {
```

Then collect results from each `distribute_redemption_across_band` call. The key changes inside the function:

1. Add `let mut all_impacts: Vec<VaultRedemptionImpact> = Vec::new();` after `let mut remaining = ...`

2. Change each `self.distribute_redemption_across_band(...)` call to `all_impacts.extend(self.distribute_redemption_across_band(...))`.

3. At the early returns (lines 2314-2316 for zero amount, line 2354-2356 for empty vaults), return `Vec::new()`.

4. After the while loop ends, return `all_impacts`.

Here's the full updated function body (key sections only — the water-filling logic stays the same, just collecting results):

At the top:
```rust
        if icusd_amount == 0 {
            return Vec::new();
        }
```

Empty vaults:
```rust
        if vault_entries.is_empty() {
            return Vec::new();
        }
```

After `let mut remaining = ...`:
```rust
        let mut all_impacts: Vec<VaultRedemptionImpact> = Vec::new();
```

Each `distribute_redemption_across_band` call becomes:
```rust
                all_impacts.extend(self.distribute_redemption_across_band(
                    &band_vault_ids, &band_debts, total_band_debt,
                    remaining, price, decimals,
                ));
                break;
```

(Apply this pattern to all 4 calls to `distribute_redemption_across_band` in the function.)

After the while loop (before the closing `}`):
```rust
        all_impacts
```

### Step 2.3: Verify it compiles

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo check -p rumi_protocol_backend 2>&1
```

Expected: Clean compile. The callers of `redeem_on_vaults` (in `record_redemption_on_vaults` and the replay handler) currently ignore the return value — Rust allows this.

### Step 2.4: Run tests

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib 2>&1
```

Expected: 55 passed, 1 pre-existing failure.

### Step 2.5: Commit

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat(backend): return per-vault deltas from water-filling

redeem_on_vaults() and distribute_redemption_across_band() now return
Vec<VaultRedemptionImpact> with per-vault debt_reduced and
collateral_seized. Callers can use this to populate event data.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 3: Wire up event recording and replay

### Step 3.1: Update `record_redemption_on_vaults` to capture and store impacts

**File:** `src/rumi_protocol_backend/src/event.rs`

The function currently records the event, then calls `state.redeem_on_vaults(...)`. We need to call water-filling FIRST to get the impacts, then record the event with the impacts.

Change `record_redemption_on_vaults` (around line 1162) to:

```rust
pub fn record_redemption_on_vaults(
    state: &mut State,
    owner: Principal,
    icusd_amount: ICUSD,
    fee_amount: ICUSD,
    collateral_price: UsdIcp,
    icusd_block_index: u64,
) {
    // Fee is already deducted from icusd_amount before calling redeem_on_vaults,
    // so vault owners effectively keep the fee (less collateral seized for their debt).
    // The fee portion of icUSD stays in the protocol canister (burned).

    // Pick the best collateral type based on redemption tier priority.
    // Tier 1 (most exposed) is redeemed first; within a tier, the collateral
    // type whose worst vault has the lowest health score goes first.
    let priority_types = state.get_collateral_types_by_redemption_priority();
    let redeem_ct = priority_types.first()
        .copied()
        .unwrap_or_else(|| state.icp_collateral_type()); // fallback to ICP

    // Use the selected collateral type's price for both water-filling and
    // pending transfer amount calculation. The caller's collateral_price
    // parameter may be for a different collateral type.
    let ct_price = state.get_collateral_config(&redeem_ct)
        .and_then(|c| c.last_price)
        .and_then(rust_decimal::Decimal::from_f64_retain)
        .map(UsdIcp::from)
        .unwrap_or(collateral_price); // fallback to parameter if no config price

    // Run water-filling FIRST to collect per-vault impacts
    let impacts = state.redeem_on_vaults(icusd_amount, ct_price, &redeem_ct);

    // Record event AFTER water-filling so we can include per-vault data.
    // This makes replay exact: the replay handler applies stored deltas
    // directly instead of re-running the water-filling algorithm.
    record_event(&Event::RedemptionOnVaults {
        owner,
        current_icp_rate: ct_price,
        icusd_amount,
        fee_amount,
        icusd_block_index,
        collateral_type: Some(redeem_ct),
        vault_impacts: Some(impacts),
        timestamp: Some(now()),
    });

    let margin: ICP = icusd_amount / ct_price;
    state
        .pending_redemption_transfer
        .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: redeem_ct });
}
```

### Step 3.2: Update the replay handler to use stored deltas

**File:** `src/rumi_protocol_backend/src/event.rs`

Change the `RedemptionOnVaults` replay handler (around line 654) from:

```rust
            Event::RedemptionOnVaults {
                owner,
                current_icp_rate,
                icusd_amount,
                fee_amount,
                icusd_block_index,
                ..
            } => {
                state.provide_liquidity(fee_amount, state.developer_principal);
                let redeem_ct = state.icp_collateral_type();
                state.redeem_on_vaults(icusd_amount, current_icp_rate, &redeem_ct);
                let margin: ICP = icusd_amount / current_icp_rate;
                state
                    .pending_redemption_transfer
                    .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: crate::vault::default_collateral_type() });
            }
```

To:

```rust
            Event::RedemptionOnVaults {
                owner,
                current_icp_rate,
                icusd_amount,
                fee_amount,
                icusd_block_index,
                collateral_type,
                vault_impacts,
                ..
            } => {
                state.provide_liquidity(fee_amount, state.developer_principal);

                // Determine which collateral type was redeemed
                let redeem_ct = collateral_type.unwrap_or_else(|| state.icp_collateral_type());

                if let Some(impacts) = vault_impacts {
                    // New events: apply stored per-vault deltas directly.
                    // This is exact — immune to algorithm changes.
                    for impact in impacts {
                        let debt = ICUSD::from(impact.debt_reduced);
                        state.deduct_amount_from_vault(impact.collateral_seized, debt, impact.vault_id);
                    }
                } else {
                    // Old events (pre-enrichment): re-run water-filling.
                    // These always used ICP, so redeem_ct = ICP from the fallback above.
                    let _ = state.redeem_on_vaults(icusd_amount, current_icp_rate, &redeem_ct);
                }

                let margin: ICP = icusd_amount / current_icp_rate;
                state
                    .pending_redemption_transfer
                    .insert(icusd_block_index, PendingMarginTransfer { owner, margin, collateral_type: redeem_ct });
            }
```

Note: `deduct_amount_from_vault` is currently private (`fn`, not `pub fn`). We need to make it `pub(crate)`:

**File:** `src/rumi_protocol_backend/src/state.rs` (around line 2485)

Change:
```rust
    fn deduct_amount_from_vault(
```
To:
```rust
    pub(crate) fn deduct_amount_from_vault(
```

### Step 3.3: Verify it compiles

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo check -p rumi_protocol_backend 2>&1
```

### Step 3.4: Run tests

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib 2>&1
```

Expected: 55 passed, 1 pre-existing failure.

### Step 3.5: Commit

```bash
git add src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/state.rs
git commit -m "feat(backend): populate vault_impacts in events and use for replay

record_redemption_on_vaults now runs water-filling first, collects
per-vault deltas, then records them in the event. The replay handler
applies stored deltas directly for new events, falling back to
water-filling for old events without vault_impacts.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 4: Update event filters

### Step 4.1: Fix `is_vault_related` to use `vault_impacts`

**File:** `src/rumi_protocol_backend/src/event.rs`

Change line 480 from:

```rust
            Event::RedemptionOnVaults { .. } => true,
```

To:

```rust
            Event::RedemptionOnVaults { vault_impacts, .. } => {
                match vault_impacts {
                    Some(impacts) => impacts.iter().any(|i| &i.vault_id == filter_vault_id),
                    None => true, // old events: show on all vaults (backward compat)
                }
            }
```

### Step 4.2: Verify it compiles and tests pass

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo check -p rumi_protocol_backend 2>&1
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib 2>&1
```

### Step 4.3: Commit

```bash
git add src/rumi_protocol_backend/src/event.rs
git commit -m "fix(backend): filter RedemptionOnVaults events by vault_impacts

Only show redemption events on vault detail pages if the vault was
actually affected. Old events without vault_impacts still show on all
vaults for backward compatibility.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 5: Update frontend explorer formatters

### Step 5.1: Update `explorerFormatters.ts` for per-vault display

**File:** `src/vault_frontend/src/lib/utils/explorerFormatters.ts`

Change the `redemption_on_vaults` case (around line 1072) from:

```typescript
    case 'redemption_on_vaults': {
      const amt = fmtE8s(d.icusd_amount);
      const fee = fmtE8s(d.fee_amount);
      pushIfPresent(fields, addressField('Redeemer', d.owner));
      fields.push(amountField('icUSD Redeemed', d.icusd_amount));
      fields.push(amountField('Fee', d.fee_amount));
      if (d.current_icp_rate !== undefined) {
        fields.push(textField('ICP Rate', `$${Number(d.current_icp_rate).toFixed(4)}`));
      }
      if (d.icusd_block_index !== undefined) fields.push(blockIndexField('icUSD Block Index', d.icusd_block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Redeemed ${amt} icUSD (fee: ${fee} icUSD)`,
        typeName, category, badgeColor, fields,
      };
    }
```

To:

```typescript
    case 'redemption_on_vaults': {
      const amt = fmtE8s(d.icusd_amount);
      const fee = fmtE8s(d.fee_amount);
      pushIfPresent(fields, addressField('Redeemer', d.owner));
      fields.push(amountField('Total icUSD Redeemed', d.icusd_amount));
      fields.push(amountField('Fee', d.fee_amount));
      if (d.collateral_type) {
        const ctPrincipal = d.collateral_type?.toText?.() ?? String(d.collateral_type);
        const ctSym = getTokenSymbol(ctPrincipal);
        const ctDec = getTokenDecimals(ctPrincipal);
        fields.push(textField('Collateral Type', ctSym));
        if (d.current_icp_rate !== undefined) {
          fields.push(textField(`${ctSym} Rate`, `$${Number(d.current_icp_rate).toFixed(4)}`));
        }
        // Show per-vault impacts if available
        if (d.vault_impacts && Array.isArray(d.vault_impacts)) {
          for (const impact of d.vault_impacts) {
            const vid = Number(impact.vault_id);
            const debt = fmtE8s(impact.debt_reduced);
            const coll = fmtE8s(impact.collateral_seized, ctDec);
            fields.push(textField(`Vault #${vid}`, `−${debt} icUSD debt, −${coll} ${ctSym} collateral`));
          }
        }
      } else {
        // Old events without collateral_type
        if (d.current_icp_rate !== undefined) {
          fields.push(textField('ICP Rate', `$${Number(d.current_icp_rate).toFixed(4)}`));
        }
      }
      if (d.icusd_block_index !== undefined) fields.push(blockIndexField('icUSD Block Index', d.icusd_block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Redeemed ${amt} icUSD (fee: ${fee} icUSD)`,
        typeName, category, badgeColor, fields,
      };
    }
```

### Step 5.2: Update `VaultHistory.svelte` for per-vault display

**File:** `src/vault_frontend/src/lib/components/vault/VaultHistory.svelte`

Change the `redemption_on_vaults` handler (around line 116) from:

```typescript
    else if ('redemption_on_vaults' in event) {
      type = 'Redeemed';
      icon = '🔁';
      color = 'text-cyan-400';
      const amt = Number(event.redemption_on_vaults.icusd_amount) / E8S;
      const fee = Number(event.redemption_on_vaults.fee_amount) / E8S;
      details = `${formatStableTx(amt)} icUSD redeemed (fee: ${formatStableTx(fee)})`;
    }
```

To:

```typescript
    else if ('redemption_on_vaults' in event) {
      type = 'Redeemed';
      icon = '🔁';
      color = 'text-cyan-400';
      const e = event.redemption_on_vaults;
      const totalAmt = Number(e.icusd_amount) / E8S;
      const fee = Number(e.fee_amount) / E8S;
      // Try to find this vault's specific impact
      if (e.vault_impacts && Array.isArray(e.vault_impacts)) {
        const myImpact = e.vault_impacts.find((i: any) => Number(i.vault_id) === vaultId);
        if (myImpact) {
          const debtReduced = Number(myImpact.debt_reduced) / E8S;
          const collSeized = Number(myImpact.collateral_seized) / E8S;
          details = `Debt reduced by ${formatStableTx(debtReduced)} icUSD, ${formatNumber(collSeized)} collateral seized`;
        } else {
          details = `${formatStableTx(totalAmt)} icUSD redeemed across vaults (fee: ${formatStableTx(fee)})`;
        }
      } else {
        details = `${formatStableTx(totalAmt)} icUSD redeemed (fee: ${formatStableTx(fee)})`;
      }
    }
```

### Step 5.3: Commit

```bash
git add src/vault_frontend/src/lib/utils/explorerFormatters.ts src/vault_frontend/src/lib/components/vault/VaultHistory.svelte
git commit -m "feat(frontend): show per-vault redemption impact in explorer

Explorer now displays debt reduced and collateral seized for each vault
affected by a redemption. Vault detail pages show only the impact on
that specific vault. Falls back to total display for old events.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 6: Add test for replay with vault_impacts

### Step 6.1: Write a test that verifies replay uses stored deltas

**File:** `src/rumi_protocol_backend/src/state.rs`

Add after the existing `test_tiered_redemption_ordering` test:

```rust
    #[test]
    fn test_redemption_vault_impacts_replay() {
        // Verify that deduct_amount_from_vault correctly applies per-vault deltas
        // (simulating what the replay handler does with vault_impacts)
        let mut state = test_state_with_icp_config();
        let icp_ct = state.icp_collateral_type();

        // Open two vaults with known amounts
        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 1,
            collateral_amount: 500_000_000, // 5 ICP
            borrowed_icusd_amount: ICUSD::new(300_000_000), // 3 icUSD
            collateral_type: icp_ct,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        state.open_vault(crate::vault::Vault {
            owner: Principal::anonymous(),
            vault_id: 2,
            collateral_amount: 800_000_000, // 8 ICP
            borrowed_icusd_amount: ICUSD::new(500_000_000), // 5 icUSD
            collateral_type: icp_ct,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        // Apply deltas as the replay handler would
        let impacts = vec![
            VaultRedemptionImpact { vault_id: 1, debt_reduced: 100_000_000, collateral_seized: 50_000_000 },
            VaultRedemptionImpact { vault_id: 2, debt_reduced: 150_000_000, collateral_seized: 75_000_000 },
        ];
        for impact in &impacts {
            state.deduct_amount_from_vault(
                impact.collateral_seized,
                ICUSD::from(impact.debt_reduced),
                impact.vault_id,
            );
        }

        // Verify vault 1: 3 - 1 = 2 icUSD debt, 5 - 0.5 = 4.5 ICP
        let v1 = state.vault_id_to_vaults.get(&1).unwrap();
        assert_eq!(v1.borrowed_icusd_amount, ICUSD::new(200_000_000));
        assert_eq!(v1.collateral_amount, 450_000_000);

        // Verify vault 2: 5 - 1.5 = 3.5 icUSD debt, 8 - 0.75 = 7.25 ICP
        let v2 = state.vault_id_to_vaults.get(&2).unwrap();
        assert_eq!(v2.borrowed_icusd_amount, ICUSD::new(350_000_000));
        assert_eq!(v2.collateral_amount, 725_000_000);
    }
```

Note: This test uses `test_state_with_icp_config()` — check that this helper exists in the test module. If not, use whatever test state constructor is already in the codebase.

### Step 6.2: Run the test

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib test_redemption_vault_impacts_replay 2>&1
```

Expected: Pass.

### Step 6.3: Commit

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "test(backend): add replay test for vault_impacts deltas

Verifies that the replay handler correctly applies per-vault deltas
from VaultRedemptionImpact, matching what deduct_amount_from_vault does.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

## Task 7: WASM build + final verification

### Step 7.1: Build WASM

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend --release 2>&1
```

Expected: Clean build.

### Step 7.2: Run all tests

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p rumi_protocol_backend --lib 2>&1
```

Expected: 56 passed (55 + 1 new), 1 pre-existing failure.

### Step 7.3: Push and update PR

```bash
git push
```

---

## Summary of files changed

| File | Changes |
|------|---------|
| `src/rumi_protocol_backend/src/state.rs` | Add `VaultRedemptionImpact` struct; `distribute_redemption_across_band` returns `Vec<VaultRedemptionImpact>`; `redeem_on_vaults` returns `Vec<VaultRedemptionImpact>`; `deduct_amount_from_vault` visibility to `pub(crate)`; new test |
| `src/rumi_protocol_backend/src/event.rs` | Add `collateral_type` and `vault_impacts` fields to `RedemptionOnVaults`; update recording to populate new fields; update replay to use stored deltas; update `is_vault_related` filter |
| `src/vault_frontend/src/lib/utils/explorerFormatters.ts` | Show collateral type and per-vault impacts in redemption events |
| `src/vault_frontend/src/lib/components/vault/VaultHistory.svelte` | Show per-vault debt/collateral impact on vault detail page |
