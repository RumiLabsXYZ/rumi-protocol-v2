# Liquidation Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Record collateral price in liquidation history and show all open vaults on the liquidation page.

**Architecture:** Two independent features on one branch. Feature 1 threads the collateral price from the backend through the stability pool's inter-canister response into the liquidation record. Feature 2 adds a `get_all_vaults` query endpoint and updates the frontend to display all vaults with reduced opacity for non-liquidatable ones.

**Tech Stack:** Rust (IC canisters), Svelte/TypeScript (frontend), Candid (.did)

---

### Task 1: Add `collateral_price_e8s` to backend `StabilityPoolLiquidationResult`

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs:32-40` (struct definition)
- Modify: `src/rumi_protocol_backend/src/main.rs:648-656` (struct construction in `stability_pool_liquidate`)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did:385-393` (Candid type)

**Step 1: Add field to Rust struct**

In `src/rumi_protocol_backend/src/main.rs`, add `collateral_price_e8s` to the struct at line 32:

```rust
#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}
```

**Step 2: Populate the field in `stability_pool_liquidate`**

The price is already available as `_collateral_price_usd` (line 605, currently prefixed with `_` because unused). Remove the underscore and use it. At line 648, update the struct construction:

Change the destructured variable name at line 605 from `_collateral_price_usd` to `collateral_price_usd`, then at line 648:

```rust
    Ok(StabilityPoolLiquidationResult {
        success: true,
        vault_id,
        liquidated_debt: liquidatable_debt.to_u64(),
        collateral_received: collateral_available.to_u64(),
        collateral_type: vault.collateral_type.to_string(),
        block_index: result.block_index,
        fee: result.fee_amount_paid,
        collateral_price_e8s: collateral_price_usd.to_u64(),
    })
```

**Step 3: Update `.did` file**

In `src/rumi_protocol_backend/rumi_protocol_backend.did`, add the field to the Candid type at line 385:

```
type StabilityPoolLiquidationResult = record {
  fee : nat64;
  block_index : nat64;
  vault_id : nat64;
  liquidated_debt : nat64;
  success : bool;
  collateral_type : text;
  collateral_received : nat64;
  collateral_price_e8s : nat64;
};
```

**Step 4: Build backend to verify**

Run: `cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend`
Expected: compiles successfully

**Step 5: Commit**

```
feat: add collateral_price_e8s to StabilityPoolLiquidationResult
```

---

### Task 2: Thread price through stability pool's `process_liquidation_gains`

**Files:**
- Modify: `src/stability_pool/src/state.rs:337-344` (`process_liquidation_gains` signature)
- Modify: `src/stability_pool/src/state.rs:348-354` (`process_liquidation_gains_at` signature)
- Modify: `src/stability_pool/src/state.rs:443` (hardcoded `Some(0)` → use parameter)
- Modify: `src/stability_pool/src/liquidation.rs:260-268` (call site in `execute_single_liquidation`)
- Modify: `src/stability_pool/src/monitor.rs:158-165` (struct definition — add field)

**Step 1: Add `collateral_price_e8s` to monitor's `StabilityPoolLiquidationResult`**

In `src/rumi_stability_pool/src/monitor.rs` at line 158, add the field to match the backend:

```rust
#[derive(candid::CandidType, serde::Deserialize, Clone, Debug)]
pub struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}
```

**Step 2: Update `process_liquidation_gains` signature**

In `src/stability_pool/src/state.rs`, update both functions to accept the price:

```rust
    pub fn process_liquidation_gains(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_gained: u64,
        collateral_price_e8s: u64,
    ) {
        self.process_liquidation_gains_at(vault_id, collateral_type, stables_consumed, collateral_gained, collateral_price_e8s, ic_cdk::api::time());
    }

    pub fn process_liquidation_gains_at(
        &mut self,
        vault_id: u64,
        collateral_type: Principal,
        stables_consumed: &BTreeMap<Principal, u64>,
        collateral_gained: u64,
        collateral_price_e8s: u64,
        timestamp: u64,
    ) {
```

Then at line 443 (inside `process_liquidation_gains_at`), change:

```rust
        // OLD:
        collateral_price_e8s: Some(0), // TODO: pass from backend in future update
        // NEW:
        collateral_price_e8s: Some(collateral_price_e8s),
```

**Step 3: Update call site in `execute_single_liquidation`**

In `src/stability_pool/src/liquidation.rs` at line 260-268, the price isn't available because `execute_single_liquidation` doesn't call `stability_pool_liquidate` — it calls `liquidate_vault_partial` directly which doesn't return the price. Pass `0` here since the direct liquidation path doesn't have the price:

```rust
        mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                total_collateral_gained,
                0, // price not available from direct liquidation path
            );
        });
```

**Step 4: Update monitor call site**

Check `src/rumi_stability_pool/src/monitor.rs` for any calls to `process_liquidation_gains`. The monitor calls `stability_pool_liquidate` on the backend which DOES return the price. Find the call site and pass `result.collateral_price_e8s` through.

**Step 5: Update all test call sites**

In `src/stability_pool/src/state.rs`, update every `process_liquidation_gains_at` call in tests to include a price parameter (use a realistic value like `7_50000000` for $7.50 ICP). There are calls at approximately lines 958, 1028, 1251, 1284, 1453. Add `0` or a test price as the new parameter before the timestamp parameter.

**Step 6: Build stability pool to verify**

Run: `cargo build --target wasm32-unknown-unknown -p stability_pool`
Expected: compiles successfully

**Step 7: Run tests**

Run: `cargo test -p stability_pool`
Expected: all tests pass

**Step 8: Commit**

```
feat: thread collateral price through stability pool liquidation records
```

---

### Task 3: Add `get_all_vaults` backend query endpoint

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (new endpoint, near line 703)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` (add to service)

**Step 1: Add the endpoint**

In `src/rumi_protocol_backend/src/main.rs`, add after `get_liquidatable_vaults` (after line 703):

```rust
#[candid_method(query)]
#[query]
fn get_all_vaults() -> Vec<CandidVault> {
    read_state(|s| {
        s.vault_id_to_vaults
            .values()
            .cloned()
            .map(CandidVault::from)
            .collect::<Vec<CandidVault>>()
    })
}
```

**Step 2: Add to `.did` file**

In `src/rumi_protocol_backend/rumi_protocol_backend.did`, add near the other vault queries (after `get_liquidatable_vaults` at line 464):

```
  get_all_vaults : () -> (vec CandidVault) query;
```

**Step 3: Build to verify**

Run: `cargo build --target wasm32-unknown-unknown -p rumi_protocol_backend`
Expected: compiles successfully

**Step 4: Regenerate frontend declarations**

Run: `dfx generate rumi_protocol_backend`
Expected: updates `src/declarations/rumi_protocol_backend/`

**Step 5: Commit**

```
feat: add get_all_vaults query endpoint
```

---

### Task 4: Add `getAllVaults` to frontend API client and service

**Files:**
- Modify: `src/vault_frontend/src/lib/services/protocol/apiClient.ts` (new static method, near line 2224)
- Modify: `src/vault_frontend/src/lib/services/protocol.ts` (add to service)

**Step 1: Add API client method**

In `src/vault_frontend/src/lib/services/protocol/apiClient.ts`, after `getLiquidatableVaults` (line 2224):

```typescript
    static async getAllVaults(): Promise<CandidVault[]> {
      try {
        const vaults = await ApiClient.getPublicData<CandidVault[]>('get_all_vaults');
        return vaults;
      } catch (err) {
        console.error('Failed to get all vaults:', err);
        return [];
      }
    }
```

**Step 2: Add to protocol service**

In `src/vault_frontend/src/lib/services/protocol.ts`, add to both the class and the export object following the pattern of `getLiquidatableVaults`:

Add to the `ProtocolService` class:
```typescript
  static getAllVaults = ApiClient.getAllVaults;
```

Add to the `protocolService` export object:
```typescript
  getAllVaults: ProtocolService.getAllVaults,
```

**Step 3: Commit**

```
feat: add getAllVaults to frontend API layer
```

---

### Task 5: Update liquidation page to show all vaults

**Files:**
- Modify: `src/vault_frontend/src/routes/liquidations/+page.svelte`

**Step 1: Add state and fetching for all vaults**

In the `<script>` section, add a new state variable near line 14:

```typescript
  let allVaults: CandidVault[] = [];
```

Add a new function to load all vaults (near `loadLiquidatableVaults`):

```typescript
  async function loadAllVaults() {
    try {
      const vaults = await protocolService.getAllVaults();
      allVaults = vaults.map(vault => ({
        ...vault,
        original_icp_margin_amount: vault.icp_margin_amount,
        original_borrowed_icusd_amount: vault.borrowed_icusd_amount,
        icp_margin_amount: Number(vault.icp_margin_amount || 0),
        collateral_amount: Number(vault.collateral_amount || vault.icp_margin_amount || 0),
        borrowed_icusd_amount: Number(vault.borrowed_icusd_amount || 0),
        vault_id: Number(vault.vault_id || 0),
        owner: vault.owner.toString()
      }));
    } catch (error) {
      console.error("Error loading all vaults:", error);
    }
  }
```

Add a reactive derived list for non-liquidatable vaults sorted by CR:

```typescript
  $: nonLiquidatableVaults = allVaults
    .filter(v => !liquidatableVaults.some(lv => lv.vault_id === v.vault_id))
    .sort((a, b) => calculateCollateralRatio(a) - calculateCollateralRatio(b));
```

**Step 2: Update onMount to fetch both**

Update the `onMount` to also call `loadAllVaults` and refresh on the same interval:

```typescript
  onMount(() => {
    refreshIcpPrice(); loadLiquidatableVaults(); loadAllVaults();
    if ($wallet.isConnected) wallet.refreshBalance().catch(() => {});
    const pi = setInterval(refreshIcpPrice, 30000);
    const vi = setInterval(() => { loadLiquidatableVaults(); loadAllVaults(); }, 60000);
    return () => { clearInterval(pi); clearInterval(vi); };
  });
```

**Step 3: Update summary stat**

Update the summary line (line 300) to show total vault count:

```svelte
    <span class="summary-stat">{sortedVaults.length} liquidatable vault{sortedVaults.length !== 1 ? 's' : ''} · {allVaults.length} total</span>
```

**Step 4: Add the non-liquidatable vaults section to the template**

After the closing `{/if}` of the existing vault list (after line 403), add the new section. Keep the existing empty state message only when there are zero vaults total. Add the all-vaults section:

After the existing `{/each}` block for `sortedVaults` and before the closing `</div>` of `liq-list`, add a divider and the non-liquidatable vaults:

```svelte
    {#if nonLiquidatableVaults.length > 0}
      <div class="section-divider"></div>
      <div class="section-header">All Vaults</div>
      {#each nonLiquidatableVaults as vault (vault.vault_id)}
        {@const cr = calculateCollateralRatio(vault)}
        {@const debt = getVaultDebt(vault)}
        {@const ci = getVaultCollateralInfo(vault)}

        <div class="liq-card liq-card-inactive">
          <div class="card-body">
            <div class="card-left">
              <div class="left-header">
                <span class="vault-id">#{vault.vault_id}</span>
                <span class="cr-badge">
                  {formatNumber(cr, 1)}%
                </span>
              </div>
              <div class="left-stats">
                <span class="stat"><span class="stat-label">Debt</span> <span class="stat-value">{formatStableDisplay(debt)} icUSD</span></span>
                <span class="stat-sep">·</span>
                <span class="stat"><span class="stat-label">Collateral</span> <span class="stat-value">{formatNumber(ci.collateralAmount, 4)} {ci.symbol}</span></span>
              </div>
            </div>
          </div>
        </div>
      {/each}
    {/if}
```

**Step 5: Add styles**

Add to the `<style>` section:

```css
  .liq-card-inactive { opacity: 0.7; }
  .section-divider {
    height: 1px;
    background: var(--rumi-border);
    margin: 0.75rem 0;
    opacity: 0.5;
  }
  .section-header {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    margin-bottom: 0.375rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
```

**Step 6: Verify locally**

Run: `npm run dev` (from vault_frontend directory) or use the dev server
Expected: Liquidation page shows liquidatable vaults at top, divider, then all other vaults at reduced opacity

**Step 7: Commit**

```
feat: show all open vaults on liquidation page
```
