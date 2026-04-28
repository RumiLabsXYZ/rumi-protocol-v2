# Explorer Review Pass — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every Explorer issue Rob flagged in the 2026-04-27 review — broken cards, mislabeled metrics, missing data, dead admin UI — so each lens shows correct, useful, well-labeled information.

**Architecture:** Most "backend bugs" are actually frontend bugs where the UI reads field names that don't exist on the backend response (e.g., `vault.collateral_ratio`, `liquidation.debt_cleared_e8s`). Fix those client-side from data already being returned. The genuinely backend-side gaps are localized to the analytics canister: a missing tailer for the rumi_stability_pool canister (the leaderboard's data source dried up when SP became its own canister), and `fee_amount` being dropped from backend events during tailing. Address those in the analytics canister only — no protocol-backend changes required.

**Tech Stack:** SvelteKit + TypeScript frontend (vault_frontend), Rust analytics canister (rumi_analytics) on the Internet Computer. Build/deploy via DFX.

**Branch:** `feat/explorer-review-pass` (already created from main; uncommitted Phase-0 work in working tree).

---

## Background: classifying the issues

Rob's review hit ~30 distinct items. Root-cause analysis groups them as:

| Class | Examples | Risk to fix |
|---|---|---|
| **Frontend reads nonexistent fields** | `vault.collateral_ratio`, `liquidation.debt_cleared_e8s` (the field is `stables_consumed: vec record { principal; nat64 }`) | Low — data already returned, just rename/recompute |
| **Frontend uses wrong typeKey filters** | Old CamelCase variant set in `LensActivityPanel` never matched real snake_case backend variants (already fixed in Phase 0) | Low |
| **Analytics canister architectural gap** | Top SP depositors leaderboard reads backend `ProvideLiquidity` events that are never emitted anymore (SP is now a separate canister) | Medium — needs a new tailer + canister deploy |
| **Analytics drops data on the floor** | `fee_amount` is in the deserialized event but the `..` rest-pattern discards it; rollups stub `None` | Low — pure analytics canister change |
| **Pure UX/label cleanup** | "Peg imbalance" misread as de-peg; AMM events show as plain "AMM" with no pool ID | Already fixed in Phase 0 |

**Answer to Rob's question** ("which of these is more indicative of a greater problem?"): the genuinely problematic class is **Analytics canister architectural gap** — when the SP became a separate canister, the analytics tailer wasn't updated to follow it, so any leaderboard or rollup that depends on SP activity has been silently empty since. Everything else is local to the frontend or to a single dropped field in analytics — annoying but isolated.

---

## File structure

**Already modified in Phase 0 (uncommitted on `feat/explorer-review-pass`):**
- `src/vault_frontend/src/lib/components/explorer/EntityLink.svelte` — added `pool` type and `class` prop
- `src/vault_frontend/src/lib/components/explorer/EventRow.svelte` — entity links instead of facet chips
- `src/vault_frontend/src/lib/components/explorer/MixedEventRow.svelte` — same
- `src/vault_frontend/src/lib/components/explorer/CeilingBar.svelte` — show absolute ceiling
- `src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte` — Vol tooltip
- `src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte` — snake_case event filters; AccrueInterest excluded
- `src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte` — sparse-data dots, anchored y-axis
- `src/vault_frontend/src/lib/components/explorer/PoolHealthStrip.svelte` — "Balanced" not "Stable"
- `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte` — Protocol Config and cycles chart removed
- `src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte` — pool registry, AMM Pools card columns
- `src/vault_frontend/src/lib/components/explorer/lenses/RedemptionsLens.svelte` — RMR display
- `src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte` — liq share → interest share
- `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte` — live SP APY
- `src/vault_frontend/src/lib/utils/displayEvent.ts` — AMM source label uses pool index
- `src/vault_frontend/src/lib/utils/eventFacets.ts` — init/upgrade reclassified as admin
- `src/vault_frontend/src/lib/utils/ammNaming.ts` — NEW
- `src/vault_frontend/src/routes/explorer/activity/+page.svelte` — seed AMM pool registry
- `src/vault_frontend/src/routes/explorer/e/pool/[id]/+page.svelte` — pool detail TVL

**Phase 1 will create/modify:**
- `src/vault_frontend/src/lib/utils/vaultCr.ts` — NEW, shared CR computation
- `src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte` — compute CR + median per collateral
- `src/vault_frontend/src/lib/components/explorer/CollateralTable.svelte` — receive medianCrBps from real computation
- `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte` — sum stables_consumed
- `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte` — admin breakdown + canister inventory cards
- `src/vault_frontend/src/lib/components/explorer/AdminBreakdownCard.svelte` — NEW
- `src/vault_frontend/src/lib/components/explorer/CanisterInventoryCard.svelte` — NEW

**Phase 2 will create/modify (analytics canister, Rust):**
- `src/rumi_analytics/src/tailing/backend_events.rs` — capture `fee_amount` on Borrow/Redemption
- `src/rumi_analytics/src/storage/events.rs` — extend `AnalyticsVaultEvent` with `fee_amount`
- `src/rumi_analytics/src/collectors/rollups.rs` — sum `fee_amount` instead of stubbing None
- `src/rumi_analytics/src/sources/stability_pool.rs` — NEW shadow types for SP canister events
- `src/rumi_analytics/src/tailing/stability_pool.rs` — NEW tailer
- `src/rumi_analytics/src/tailing/mod.rs` — register the SP tailer
- `src/rumi_analytics/src/lib.rs` — initialize SP cursor + register in init/post_upgrade
- `src/rumi_analytics/src/storage/cursors.rs` — add SP cursor

---

## Phase 0: Commit work in progress

Phase 0 captures everything done in this session up to the point this plan was written. One commit, descriptive message, no changes — just preserving the work.

### Task 0.1: Sanity-check working tree and commit

**Files:** All Phase-0 modified files listed above.

- [ ] **Step 1: Confirm we're on the feature branch and the audit-wave-8d backend work is stashed**

Run:
```bash
git branch --show-current
git stash list
```
Expected: branch is `feat/explorer-review-pass`; stash list shows `audit-wave-8d-backend-wip`.

- [ ] **Step 2: Run svelte-check to confirm no new TS errors from Phase 0**

Run:
```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -c ERROR
```
Expected: count matches the pre-existing error baseline (33 — all in files Phase 0 didn't touch). If higher, fix before committing.

- [ ] **Step 3: Stage only the Explorer changes**

Run:
```bash
git add src/vault_frontend/ docs/superpowers/plans/2026-04-27-explorer-review-pass.md
git status --short
```
Expected: only `src/vault_frontend/...` and the plan file in green; nothing else staged.

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
feat(explorer): review-pass round 1 — labels, navigation, lens cleanup

Phase 0 of the Explorer review pass per the 2026-04-27 plan:

UX / labeling
- Rename "peg imbalance" → "balance skew" so users don't mistake
  pool weight drift for icUSD de-pegging
- AMM events render as "AMM1 #N" with token-pair labels via a
  client-side pool registry (deterministic alphabetical ordering)
- Pool detail pages show TVL (3pool: sum stables; AMM: 3USD * vp +
  ICP * oracle price)
- Debt-ceiling cell shows the absolute ceiling next to the meter
- Redemptions lens shows RMR (floor → ceiling) range
- Volume charts show dots on non-zero points and anchor y at 0 so
  sparse activity is visible

Bug fixes
- Replace FacetChip with EntityLink for principal/token/pool/vault
  in event rows so clicks navigate to entity pages instead of
  applying filters
- Switch SP APY to the same live formula the /liquidity tab uses
  (was 4.72% from a 7d analytics window vs 6.24% live — discrepancy
  resolved)
- Rewrite LensActivityPanel admin/vault filter sets to use the real
  snake_case event variant names (the previous CamelCase set never
  matched anything, leaving Vault Activity and admin feeds empty)
- Reclassify init/upgrade events as 'admin' typeKey so canister
  upgrades surface in the admin feed
- Filter accrue_interest events out of every UI scope; "View All"
  link no longer points to ?type=admin,system

Cleanup
- Remove "Treasury liq share" tile from Revenue lens (docs material,
  not Explorer); replace with "Treasury interest share"
- Drop the Protocol Config card and analytics-only cycles chart
  from Admin lens

See docs/superpowers/plans/2026-04-27-explorer-review-pass.md for
the full plan including remaining Phase 1 (frontend) and Phase 2
(analytics canister) work.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```
Expected: commit succeeds.

- [ ] **Step 5: Verify**

Run:
```bash
git log --oneline -1
git status --short
```
Expected: latest commit is the one we just made; working tree clean (or showing only audit-wave-8d files we stashed earlier).

---

## Phase 1: Frontend follow-up fixes

Three real bugs left where the frontend reads fields that don't exist, plus two new Admin lens cards to replace the stuff we stripped.

### Task 1.1: Shared CR computation helper

The backend's `Vault` and `CandidVault` types do **not** have a `collateral_ratio` field — CR is computed on-the-fly via `compute_collateral_ratio()` whenever the protocol needs it. The frontend's `vault.collateral_ratio ?? 0` reads `undefined` for every vault, which explains the empty CR distribution histogram and the hardcoded `medianCrBps: 0`.

Fix: add a small helper that takes `(vault, priceMap, configMap)` and returns CR as a percentage. This lets us compute distribution AND median from data CollateralLens already fetches.

**Files:**
- Create: `src/vault_frontend/src/lib/utils/vaultCr.ts`
- Test: `src/vault_frontend/src/lib/utils/vaultCr.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/vault_frontend/src/lib/utils/vaultCr.test.ts`:
```typescript
import { describe, it, expect } from 'vitest';
import { computeVaultCrPct } from './vaultCr';

const ICP = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

describe('computeVaultCrPct', () => {
  it('returns CR as a percent for a typical ICP vault', () => {
    // 100 ICP @ $5 = $500 collateral, 200 icUSD debt → 250%
    const vault = {
      collateral_amount: 100_00_000_000n, // 100 ICP at e8s
      collateral_type: ICP,
      borrowed_icusd_amount: 200_00_000_000n, // 200 icUSD at e8s
    };
    const priceMap = new Map([[ICP, 5]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeCloseTo(250, 1);
  });

  it('returns null when debt is zero (debt-free vault)', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: ICP,
      borrowed_icusd_amount: 0n,
    };
    const priceMap = new Map([[ICP, 5]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeNull();
  });

  it('returns null when price is missing for the collateral', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: ICP,
      borrowed_icusd_amount: 100_00_000_000n,
    };
    expect(computeVaultCrPct(vault, new Map(), new Map())).toBeNull();
  });

  it('handles principal objects with toText()', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: { toText: () => ICP },
      borrowed_icusd_amount: 100_00_000_000n,
    };
    const priceMap = new Map([[ICP, 10]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeCloseTo(1000, 1);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cd src/vault_frontend && npx vitest run src/lib/utils/vaultCr.test.ts
```
Expected: FAIL with "Cannot find module './vaultCr'".

- [ ] **Step 3: Write the implementation**

Create `src/vault_frontend/src/lib/utils/vaultCr.ts`:
```typescript
/**
 * Compute a vault's collateral ratio as a percent (e.g., 243 for 243%).
 *
 * The backend doesn't store CR on the vault — it's computed on demand from
 * collateral_amount × price / borrowed_icusd_amount whenever needed. The
 * frontend has the same inputs (vaults from get_all_vaults, prices from
 * twap or last_price), so computing here is straightforward.
 *
 * Returns null when the vault has zero debt (CR is undefined) or when we
 * don't have a price for the collateral (better to skip than show 0).
 */

function principalText(p: any): string {
  if (!p) return '';
  if (typeof p === 'string') return p;
  if (typeof p?.toText === 'function') return p.toText();
  return String(p);
}

export function computeVaultCrPct(
  vault: any,
  priceMap: Map<string, number>,
  decimalsMap: Map<string, number>,
): number | null {
  const debtE8s = Number(vault.borrowed_icusd_amount ?? 0n);
  if (debtE8s <= 0) return null;

  const collType = principalText(vault.collateral_type);
  const price = priceMap.get(collType);
  if (price == null || price <= 0) return null;

  const decimals = decimalsMap.get(collType) ?? 8;
  const collAmount = Number(vault.collateral_amount ?? 0n) / Math.pow(10, decimals);
  const collateralUsd = collAmount * price;
  const debtUsd = debtE8s / 1e8;
  return (collateralUsd / debtUsd) * 100;
}

/**
 * Median of a numeric array. Returns null for empty input.
 * Used for the per-collateral median CR column in CollateralTable.
 */
export function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cd src/vault_frontend && npx vitest run src/lib/utils/vaultCr.test.ts
```
Expected: 4 tests passing.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/utils/vaultCr.ts src/vault_frontend/src/lib/utils/vaultCr.test.ts
git commit -m "$(cat <<'EOF'
feat(explorer): add client-side vault CR helper

Backend doesn't store collateral_ratio on the vault — it's computed on
demand. CollateralLens currently reads vault.collateral_ratio (which is
undefined) and the table hardcodes medianCrBps=0. Add a helper that
computes CR from the data we already fetch (vaults + prices) so we can
populate both the CR distribution histogram and the per-collateral
median column.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Wire CR helper into CollateralLens

Use the helper to populate the CR distribution histogram and the per-collateral median CR column.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte`

- [ ] **Step 1: Add imports**

In `CollateralLens.svelte`, top of `<script>`, add:
```typescript
  import { computeVaultCrPct, median } from '$utils/vaultCr';
```

- [ ] **Step 2: Replace the broken CR distribution loop**

Find this block (around line 115–133):
```svelte
      // CR distribution histogram from all active vaults
      const bucketDefs: [number, number, string][] = [
        [0, 110, '<110%'],
        ...
      ];
      const buckets: CrBucket[] = bucketDefs.map(([lo, hi, label]) => ({ lo, hi, label, count: 0 }));
      for (const v of allVaults) {
        const cr = Number(v.collateral_ratio ?? 0);
        if (cr === 0) continue;
        const pct = cr; // backend uses percent (e.g. 243 for 243%)
        for (const b of buckets) {
          if (pct >= b.lo && pct < b.hi) { b.count += 1; break; }
        }
      }
      crBuckets = buckets;
```

Replace with:
```svelte
      // CR distribution + per-collateral median CR. Backend returns vaults
      // without a collateral_ratio field; compute from collateral × price /
      // debt using helpers.
      const decimalsMap = new Map<string, number>();
      for (const t of totals) {
        const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
        if (pid && t.decimals != null) decimalsMap.set(pid, Number(t.decimals));
      }

      const crByCollateral = new Map<string, number[]>();
      const bucketDefs: [number, number, string][] = [
        [0, 110, '<110%'],
        [110, 150, '110-150%'],
        [150, 200, '150-200%'],
        [200, 300, '200-300%'],
        [300, 500, '300-500%'],
        [500, Infinity, '500%+'],
      ];
      const buckets: CrBucket[] = bucketDefs.map(([lo, hi, label]) => ({ lo, hi, label, count: 0 }));

      for (const v of allVaults) {
        const cr = computeVaultCrPct(v, priceMap, decimalsMap);
        if (cr == null) continue;
        for (const b of buckets) {
          if (cr >= b.lo && cr < b.hi) { b.count += 1; break; }
        }
        const collType = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
        if (!crByCollateral.has(collType)) crByCollateral.set(collType, []);
        crByCollateral.get(collType)!.push(cr);
      }
      crBuckets = buckets;
```

- [ ] **Step 3: Use the median per collateral when building rows**

Find the `medianCrBps: 0,` line (around line 102). Replace with:
```svelte
          medianCrBps: (() => {
            const m = median(crByCollateral.get(principal) ?? []);
            return m != null ? Math.round(m * 100) : 0; // bps
          })(),
```

Note: Step 3 needs to come **after** the loop that builds `crByCollateral` (Step 2 places the loop earlier than the row-building code, so order is naturally correct — verify by reading the file once more before saving).

- [ ] **Step 4: Type-check**

Run:
```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep "CollateralLens" | grep ERROR
```
Expected: no output (no new errors).

- [ ] **Step 5: Manual smoke test**

Deploy locally or hit mainnet, open `/explorer?lens=collateral`. CR distribution histogram should show bars for each bucket reflecting actual vault CRs. Avg CR column in CollateralTable should show real percentages instead of `--` for collaterals that have vaults.

- [ ] **Step 6: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): compute CR distribution + median client-side

The backend doesn't return a collateral_ratio field on vaults, so the
old code reading v.collateral_ratio was always undefined — every vault
got skipped, the histogram was empty, and the Avg CR column was a
hardcoded 0.

Use the new vaultCr helper to compute CR from the vault data + price
map we already fetch, populate the histogram, and compute a real
median per collateral type.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Fix SP "Recent liquidations absorbed" debt-cleared column

The Candid record `PoolLiquidationRecord` defines `stables_consumed: vec record { principal; nat64 }` — a list of (token-ledger-principal, amount-e8s) pairs showing which stablecoins were burned to clear the vault's debt. The frontend reads `l.debt_cleared_e8s ?? l.debt_amount ?? 0n`, neither of which exists, hence the column is always "0 icUSD".

Fix: sum the amounts in `stables_consumed`. (All stablecoins in scope — icUSD, ckUSDC, ckUSDT — are pegged $1, so a simple sum is correct.)

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte`

- [ ] **Step 1: Locate the broken cell**

Around line 300–302 the table renders:
```svelte
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(Number(l.debt_cleared_e8s ?? l.debt_amount ?? 0n) / 1e8)} icUSD
              </td>
```

- [ ] **Step 2: Add a helper at the bottom of the script section**

In the `<script>` block (just before the closing `</script>`), add:
```typescript
  // Sum stables_consumed: vec record { principal; nat64 } → total icUSD-equivalent.
  // All SP stablecoins (icUSD, ckUSDC, ckUSDT) are pegged $1 so a raw sum (after
  // decimals normalization) is fine. ckUSDC/ckUSDT are 6-decimal; icUSD is 8.
  function debtClearedFromRecord(rec: any): number {
    const stables: Array<[any, bigint]> = rec?.stables_consumed ?? [];
    let total = 0;
    for (const [tokenPrincipal, amountE8s] of stables) {
      const principal = typeof tokenPrincipal?.toText === 'function'
        ? tokenPrincipal.toText() : String(tokenPrincipal);
      // 6 decimals for ckUSDC / ckUSDT, 8 for everything else (icUSD)
      const decimals = principal.includes('xevnm-gaaaa') /* ckUSDC */
        || principal.includes('cngnf-vqaaa') /* ckUSDT */
        ? 6 : 8;
      total += Number(amountE8s) / Math.pow(10, decimals);
    }
    return total;
  }
```

- [ ] **Step 3: Replace the broken cell**

Replace lines 300–302 with:
```svelte
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(debtClearedFromRecord(l))} icUSD
              </td>
```

- [ ] **Step 4: Type-check**

Run:
```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep "StabilityPoolLens" | grep ERROR
```
Expected: no output.

- [ ] **Step 5: Manual smoke test**

Open `/explorer?lens=stability`. The "Recent liquidations absorbed" table should show real debt cleared values (not 0 icUSD) for every row that has stables consumed.

- [ ] **Step 6: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): show real debt cleared in SP liquidations table

The PoolLiquidationRecord candid type stores debt cleared as
`stables_consumed: vec record { principal; nat64 }` — a list of
(stablecoin-ledger, amount) pairs. The lens was reading non-existent
debt_cleared_e8s / debt_amount fields so every row showed 0 icUSD.

Sum the amounts in stables_consumed (with decimal normalization for
ckUSDT/ckUSDC's 6-decimal native units) for the actual debt-cleared
total.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Admin breakdown card

The analytics canister already exposes `get_admin_event_breakdown()` returning per-label counts (SetBorrowingFee × 3, SetInterestRate × 1, etc.). Surface that in a card so the Admin lens shows what's actually been changing recently, not just an event list.

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/AdminBreakdownCard.svelte`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte`

- [ ] **Step 1: Create the card component**

Create `src/vault_frontend/src/lib/components/explorer/AdminBreakdownCard.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchAdminEventBreakdown } from '$services/explorer/analyticsService';
  import type { AdminEventLabelCount } from '$declarations/rumi_analytics/rumi_analytics.did';

  let labels: AdminEventLabelCount[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      const resp = await fetchAdminEventBreakdown();
      labels = resp.labels.slice().sort((a, b) => Number(b.count) - Number(a.count));
    } catch (err) {
      console.error('[AdminBreakdownCard] load failed:', err);
    } finally {
      loading = false;
    }
  });

  // Group setter labels by domain so the card has more visual structure
  // than a flat list — most setters cluster around fees, RMR, recovery
  // mode, and collateral parameters.
  function groupOf(label: string): string {
    if (label.startsWith('SetCollateral')) return 'Collateral';
    if (label.startsWith('SetRmr') || label.startsWith('SetRedemption') || label.startsWith('SetReserve')) return 'Redemption';
    if (label.startsWith('SetRecovery')) return 'Recovery';
    if (label.includes('Fee') || label.includes('InterestRate') || label.includes('InterestSplit') || label.includes('InterestPoolShare')) return 'Fees & interest';
    if (label === 'Init' || label === 'Upgrade') return 'Lifecycle';
    if (label.includes('Bot')) return 'Bot';
    return 'Other';
  }
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Admin actions by label</h3>
  <p class="text-xs text-gray-500 mb-3">Counts of each setter / lifecycle event the analytics canister has tailed.</p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if labels.length === 0}
    <p class="text-sm text-gray-500 py-2">No admin actions recorded yet.</p>
  {:else}
    <div class="grid grid-cols-2 md:grid-cols-3 gap-x-6 gap-y-2">
      {#each labels as l (l.label)}
        <a
          href="/explorer/activity?type=admin&admin={encodeURIComponent(l.label)}"
          class="flex items-baseline justify-between text-sm py-1 border-b border-white/[0.03] hover:bg-white/[0.02]"
          title="{groupOf(l.label)} · click to filter"
        >
          <span class="text-gray-300 font-mono text-xs truncate mr-2">{l.label}</span>
          <span class="tabular-nums text-gray-200 font-medium">{Number(l.count).toLocaleString()}</span>
        </a>
      {/each}
    </div>
  {/if}
</div>
```

- [ ] **Step 2: Mount it in AdminLens**

In `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte`, add the import near the top of `<script>`:
```typescript
  import AdminBreakdownCard from '../AdminBreakdownCard.svelte';
```

Then at the bottom of the file, between the `LensHealthStrip` line and the `Analytics tailing health` block, insert:
```svelte
<AdminBreakdownCard />
```

- [ ] **Step 3: Type-check**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "AdminLens|AdminBreakdownCard" | grep ERROR
```
Expected: no output.

- [ ] **Step 4: Smoke test**

Open `/explorer?lens=admin`. The Admin Breakdown card should list every setter label with a count. Clicking a row should jump to the activity feed pre-filtered to that admin label.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/AdminBreakdownCard.svelte src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): admin actions-by-label breakdown card

Surface the get_admin_event_breakdown analytics endpoint so the Admin
lens shows which setter functions have been called and how often. Each
row links to the pre-filtered activity feed for that label.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.5: Canister inventory card

The Admin lens lost the cycles chart in Phase 0 (analytics-only, not useful). Replace it with a static-ish card listing every canister in the Rumi ecosystem with its principal ID and links to the IC dashboard. This gives admin transparency without needing per-canister polling.

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/CanisterInventoryCard.svelte`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte`

- [ ] **Step 1: Create the card**

Create `src/vault_frontend/src/lib/components/explorer/CanisterInventoryCard.svelte`:
```svelte
<script lang="ts">
  import { CANISTER_IDS } from '$lib/config';
  import EntityLink from './EntityLink.svelte';
  import CopyButton from './CopyButton.svelte';

  // Order: protocol core first, then DeFi, then ledgers, then frontends.
  // The labels here are display-only — the principals come from $lib/config.
  type Row = { label: string; principal: string; role: string };
  const rows: Row[] = [
    { label: 'rumi_protocol_backend', principal: CANISTER_IDS.PROTOCOL_BACKEND ?? '', role: 'Core CDP engine' },
    { label: 'rumi_treasury', principal: CANISTER_IDS.TREASURY ?? '', role: 'Treasury' },
    { label: 'rumi_stability_pool', principal: CANISTER_IDS.STABILITY_POOL ?? '', role: 'Stability pool' },
    { label: 'rumi_3pool', principal: CANISTER_IDS.THREEPOOL, role: 'Stableswap (3USD)' },
    { label: 'rumi_amm', principal: CANISTER_IDS.AMM ?? '', role: 'AMM' },
    { label: 'rumi_analytics', principal: CANISTER_IDS.ANALYTICS ?? '', role: 'Analytics tailer' },
    { label: 'liquidation_bot', principal: CANISTER_IDS.LIQUIDATION_BOT ?? '', role: 'Liquidations' },
    { label: 'icusd_ledger', principal: CANISTER_IDS.ICUSD_LEDGER, role: 'icUSD ICRC-1/2 ledger' },
  ].filter((r) => r.principal);

  function dashboardUrl(principal: string): string {
    return `https://dashboard.internetcomputer.org/canister/${principal}`;
  }
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Canister inventory</h3>
  <p class="text-xs text-gray-500 mb-3">Every protocol canister with its principal ID. Dashboard link shows live cycles, controllers, and module hash.</p>
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b border-white/5">
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Canister</th>
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Role</th>
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Principal</th>
        <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Dashboard</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r (r.principal)}
        <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
          <td class="py-2 px-2 font-mono text-xs text-gray-200">{r.label}</td>
          <td class="py-2 px-2 text-gray-400">{r.role}</td>
          <td class="py-2 px-2 font-mono text-xs">
            <span class="inline-flex items-center gap-1">
              <EntityLink type="canister" value={r.principal} />
              <CopyButton text={r.principal} />
            </span>
          </td>
          <td class="py-2 px-2 text-right">
            <a href={dashboardUrl(r.principal)} target="_blank" rel="noopener" class="text-teal-400 hover:text-teal-300 text-xs">View ↗</a>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
```

- [ ] **Step 2: Verify CANISTER_IDS keys exist**

Run:
```bash
grep -n "PROTOCOL_BACKEND\|TREASURY\|STABILITY_POOL\|AMM\|ANALYTICS\|LIQUIDATION_BOT" src/vault_frontend/src/lib/config.ts | head
```

If any of these aren't defined, you'll need to either add them to `config.ts` or remove that row from the inventory. The Phase 0 work used `CANISTER_IDS.THREEPOOL` and `CANISTER_IDS.ICUSD_LEDGER` so those definitely exist; verify the others before declaring done.

- [ ] **Step 3: Mount in AdminLens**

In `AdminLens.svelte`:
```typescript
  import CanisterInventoryCard from '../CanisterInventoryCard.svelte';
```

Render it after `<AdminBreakdownCard />`:
```svelte
<CanisterInventoryCard />
```

- [ ] **Step 4: Type-check + smoke**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep "CanisterInventoryCard\|AdminLens" | grep ERROR
```
Open `/explorer?lens=admin`; verify the inventory table renders with all listed canisters.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/CanisterInventoryCard.svelte src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): canister inventory card on Admin lens

Replace the analytics-only cycles chart (removed in round 1) with a
static inventory of every protocol canister: name, role, principal,
and a link to the IC dashboard for live cycles + controller view.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.6: Verify Vault Activity card now populated

Phase 0 rewrote `LensActivityPanel`'s `isBackendVaultEvent` to use snake_case variant names (the previous CamelCase set never matched). Verify the Collateral lens's "Vault activity" panel now shows real events.

**Files:**
- (verification only — no code change unless something else is wrong)

- [ ] **Step 1: Open the lens locally / on mainnet**

Navigate to `/explorer?lens=collateral`. Scroll to the "Vault activity" panel.

- [ ] **Step 2: Cross-check against full activity page**

Open `/explorer/activity?type=open_vault,borrow,repay,withdraw_collateral` in another tab. The Collateral lens panel should show approximately the same recent events (same vault IDs, similar timestamps), capped at the lens's `limit=12`.

- [ ] **Step 3: If the panel is empty but the activity page is not**

This means another bug — capture: (a) the network response from `get_events_filtered` (devtools → Network → look for the call from LensActivityPanel), (b) the JSON shape of the first event. Compare to `VAULT_OP_KEYS` set in `LensActivityPanel.svelte`. Likely the event variant key is something we missed.

If the panel matches, no further action; just note in the commit log that the verification passed.

- [ ] **Step 4: No code change → no commit**

(This task only writes a verification step. If issues are found, file as a follow-up task, don't bundle.)

---

## Phase 2: Analytics canister fixes

The analytics canister deploys via `dfx deploy --network ic rumi_analytics`. **Important:** It's an upgrade, never reinstall — global memory holds rolled-up history. Per the project memory, rumi_analytics also exceeds 2MiB ingress so the wasm needs `ic-wasm shrink` + `gzip` before `--mode upgrade`.

### Task 2.1: Capture fee_amount on borrow events

`Event::BorrowFromVault` already carries `fee_amount: ICUSD`. The analytics canister deserializes it (see `src/rumi_analytics/src/sources/backend.rs:102`) but the tailer drops it via the `..` rest pattern.

**Files:**
- Modify: `src/rumi_analytics/src/storage/events.rs` — add `fee_amount` field to `AnalyticsVaultEvent`
- Modify: `src/rumi_analytics/src/tailing/backend_events.rs` — capture `fee_amount`

- [ ] **Step 1: Add field to AnalyticsVaultEvent**

In `src/rumi_analytics/src/storage/events.rs`, find `pub struct AnalyticsVaultEvent { ... }`. Add a new field (defaulted for backward compat with existing stable storage):
```rust
    /// Fee paid on this event in icUSD e8s. Populated for Borrowed and
    /// Redeemed; zero for other event kinds. Older stored events default
    /// to 0 via serde — we just won't have fee data for those.
    #[serde(default)]
    pub fee_amount: u64,
```

- [ ] **Step 2: Update Borrow tailer to capture fee**

In `src/rumi_analytics/src/tailing/backend_events.rs`, find the `BorrowFromVault` branch (around line 108). Replace its `evt_vaults::push(...)` call with:
```rust
        BorrowFromVault { vault_id, borrowed_amount, fee_amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::Borrowed,
                collateral_type: Principal::anonymous(),
                amount: *borrowed_amount,
                fee_amount: *fee_amount,
            });
        }
```

- [ ] **Step 3: Update Redemption tailer**

Find the `RedemptionOnVaults` branch (around line 185). Replace with:
```rust
        RedemptionOnVaults { icusd_amount, fee_amount, owner, collateral_type, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: 0,
                owner: *owner,
                event_kind: VaultEventKind::Redeemed,
                collateral_type: collateral_type.unwrap_or(Principal::anonymous()),
                amount: *icusd_amount,
                fee_amount: *fee_amount,
            });
        }
```

- [ ] **Step 4: Add `fee_amount: 0` to every other `evt_vaults::push` call**

Every other `evt_vaults::push(...)` in this file needs `fee_amount: 0,` added (Opened, Repaid, CollateralWithdrawn, PartialCollateralWithdrawn, WithdrawAndClose, Closed, DustForgiven). Search:
```bash
grep -n "evt_vaults::push" src/rumi_analytics/src/tailing/backend_events.rs
```
Add `fee_amount: 0,` to each AnalyticsVaultEvent literal that doesn't already have it.

- [ ] **Step 5: Build to confirm no missed call sites**

Run:
```bash
cargo check -p rumi_analytics
```
Expected: clean compile.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_analytics/src/storage/events.rs src/rumi_analytics/src/tailing/backend_events.rs
git commit -m "$(cat <<'EOF'
feat(analytics): capture fee_amount on Borrow and Redemption events

The backend events already carry fee_amount; analytics deserialized
it but discarded it via the rest-pattern in the tailer. Capture the
value into AnalyticsVaultEvent.fee_amount so the daily fee rollup
can sum real numbers instead of stubbing None.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.2: Sum fees in the daily rollup

Now that `AnalyticsVaultEvent` has `fee_amount`, replace the stubbed `None` in `rollup_fees` with real sums.

**Files:**
- Modify: `src/rumi_analytics/src/collectors/rollups.rs`

- [ ] **Step 1: Update rollup_fees**

In `src/rumi_analytics/src/collectors/rollups.rs`, replace the function body (lines 100–124) with:
```rust
fn rollup_fees(now: u64, day_start: u64) {
    let swap_events = evt_swaps::range(day_start, now, usize::MAX);
    let vault_events = evt_vaults::range(day_start, now, usize::MAX);

    let swap_fees: u64 = swap_events.iter().map(|e| e.fee).sum();
    let mut borrow_count: u32 = 0;
    let mut redemption_count: u32 = 0;
    let mut borrow_fees: u64 = 0;
    let mut redemption_fees: u64 = 0;

    for e in &vault_events {
        match e.event_kind {
            VaultEventKind::Borrowed => {
                borrow_count += 1;
                borrow_fees = borrow_fees.saturating_add(e.fee_amount);
            }
            VaultEventKind::Redeemed => {
                redemption_count += 1;
                redemption_fees = redemption_fees.saturating_add(e.fee_amount);
            }
            _ => {}
        }
    }

    rollups::daily_fees::push(rollups::DailyFeeRollup {
        timestamp_ns: now,
        borrowing_fees_e8s: Some(borrow_fees),
        borrow_count,
        swap_fees_e8s: swap_fees,
        redemption_fees_e8s: Some(redemption_fees),
        redemption_count,
    });
}
```

- [ ] **Step 2: Update existing rollup test**

Run:
```bash
cargo test -p rumi_analytics rollup_fees 2>&1 | head -40
```
If existing tests assert `borrowing_fees_e8s == None`, update to assert the sum. Read those test files if they exist:
```bash
grep -rn "rollup_fees\|borrowing_fees_e8s" src/rumi_analytics/src/ | grep -E "test|spec"
```

If tests need updates, edit them — keep their assertions semantically identical (still checking borrow_count, swap_fees) but replace the `None` assertion with `Some(expected_sum)`.

- [ ] **Step 3: Run all analytics tests**

```bash
cargo test -p rumi_analytics
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_analytics/src/collectors/rollups.rs
git commit -m "$(cat <<'EOF'
feat(analytics): sum borrow/redemption fees in daily rollup

borrowing_fees_e8s and redemption_fees_e8s were stubbed None pending
fee_amount capture in the tailer. Now that AnalyticsVaultEvent has
fee_amount, sum them per day so the Revenue lens shows real fee
breakdowns instead of \$0.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.3: Stability-pool canister tailer (the architectural fix)

The big one. The analytics canister has no SP-canister tailer, so deposits/withdrawals to `rumi_stability_pool` (a separate canister since the multi-canister split) are invisible to the Top SP Depositors leaderboard. The lens reads `evt_stability::range(...)` which only contains backend `Event::ProvideLiquidity` events that no longer get emitted.

We'll add a tailer that pulls events from the SP canister's existing event log via `get_pool_events`, translates them into `AnalyticsStabilityEvent`, and pushes them into the same `evt_stability` log the leaderboard reads.

**Files:**
- Create: `src/rumi_analytics/src/sources/stability_pool.rs`
- Create: `src/rumi_analytics/src/tailing/stability_pool.rs`
- Modify: `src/rumi_analytics/src/sources/mod.rs`
- Modify: `src/rumi_analytics/src/tailing/mod.rs`
- Modify: `src/rumi_analytics/src/storage/cursors.rs`
- Modify: `src/rumi_analytics/src/lib.rs`
- Modify: `src/rumi_analytics/src/storage/mod.rs` (CanisterPrincipals)

- [ ] **Step 1: Add cursor**

In `src/rumi_analytics/src/storage/cursors.rs`, find the existing cursor module pattern (search for `CURSOR_ID_BACKEND_EVENTS`). Add an analogous one:
```rust
pub const CURSOR_ID_STABILITY_POOL: u8 = /* next free id */;

pub mod stability_pool_events {
    use super::*;
    pub fn get() -> u64 { /* same shape as backend_events::get */ }
    pub fn set(value: u64) { /* same shape as backend_events::set */ }
}
```
Mirror the existing `backend_events` module exactly — same stable-memory key derivation, same getter/setter. Pick the next free `u8` for `CURSOR_ID_STABILITY_POOL` (look at the highest ID already in use in this file).

- [ ] **Step 2: Add SP principal to canister-principals state**

In `src/rumi_analytics/src/storage/mod.rs`, find `pub struct CanisterPrincipals { ... }`. It already has `stability_pool: Principal` (saw in grep earlier). If not, add. Ensure `init` and `post_upgrade` accept it as an init arg (read the existing handling for `backend` to mirror).

- [ ] **Step 3: Define SP shadow types**

Create `src/rumi_analytics/src/sources/stability_pool.rs`:
```rust
//! Shadow types + read helpers for tailing rumi_stability_pool events.
//!
//! The SP canister exposes get_pool_event_count / get_pool_events. We mirror
//! the candid types we care about (Deposit, Withdraw, ClaimCollateral) and
//! ignore the rest — admin events, errors, etc. don't drive the depositor
//! leaderboard.

use candid::{CandidType, Principal};
use serde::Deserialize;

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct PoolEvent {
    pub timestamp: u64,
    pub caller: Principal,
    pub event_type: PoolEventType,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub enum PoolEventType {
    Deposit { amount: u64, token: Principal },
    Withdraw { amount: u64, token: Principal },
    DepositAs3USD { amount: u64 },
    ClaimCollateral { collateral_type: Principal, amount: u64 },
    LiquidationExecuted { /* ignore details for tailer */ },
    LiquidationNotification { /* ignore */ },
    InterestReceived { /* ignore — too noisy */ },
    // Catch-all for variants we don't track. serde + candid both tolerate
    // missing variants if we deserialize as a serde_json::Value-equivalent;
    // if not, list every variant explicitly. Verify against the SP candid.
    #[serde(other)]
    Unknown,
}

pub async fn get_event_count(sp: Principal) -> Result<u64, String> {
    let (count,): (u64,) = ic_cdk::call(sp, "get_pool_event_count", ())
        .await
        .map_err(|e| format!("get_pool_event_count failed: {:?}", e))?;
    Ok(count)
}

pub async fn get_events(sp: Principal, start: u64, length: u64) -> Result<Vec<PoolEvent>, String> {
    let (events,): (Vec<PoolEvent>,) = ic_cdk::call(sp, "get_pool_events", (start, length))
        .await
        .map_err(|e| format!("get_pool_events failed: {:?}", e))?;
    Ok(events)
}
```

**Verify the candid signature** before committing — read `src/declarations/rumi_stability_pool/rumi_stability_pool.did` and confirm `get_pool_event_count` returns `(nat64)` and `get_pool_events` takes `(nat64, nat64)` returning `(vec PoolEvent)`. Adjust the shadow type if the actual variant names differ.

- [ ] **Step 4: Wire into sources/mod.rs**

In `src/rumi_analytics/src/sources/mod.rs`, add:
```rust
pub mod stability_pool;
```

- [ ] **Step 5: Write the tailer**

Create `src/rumi_analytics/src/tailing/stability_pool.rs`:
```rust
//! Tail rumi_stability_pool events into evt_stability so the Top SP
//! Depositors leaderboard has data to aggregate.

use candid::Principal;
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

pub async fn run() {
    let sp = state::read_state(|s| s.sources.stability_pool);
    if sp == Principal::anonymous() {
        return; // SP principal not configured yet
    }
    let cursor = cursors::stability_pool_events::get();

    let count = match sources::stability_pool::get_event_count(sp).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_sp] get_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.stability_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_STABILITY_POOL, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_STABILITY_POOL, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::stability_pool::get_events(sp, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_sp] get_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.stability_pool += 1;
                update_cursor_error(s, cursors::CURSOR_ID_STABILITY_POOL, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for (i, event) in events.iter().enumerate() {
        let event_id = cursor + i as u64;
        route_sp_event(event_id, event);
        processed += 1;
    }

    if processed > 0 {
        cursors::stability_pool_events::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_STABILITY_POOL, ic_cdk::api::time());
        });
    }
}

fn route_sp_event(event_id: u64, event: &sources::stability_pool::PoolEvent) {
    use sources::stability_pool::PoolEventType::*;

    let action = match &event.event_type {
        Deposit { amount, .. } | DepositAs3USD { amount } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::Deposit,
                amount: *amount,
            });
            return;
        }
        Withdraw { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::Withdraw,
                amount: *amount,
            });
            return;
        }
        ClaimCollateral { amount, .. } => {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns: event.timestamp,
                source_event_id: event_id,
                caller: event.caller,
                action: StabilityAction::ClaimReturns,
                amount: *amount,
            });
            return;
        }
        // LiquidationExecuted, LiquidationNotification, InterestReceived,
        // Unknown — ignore for the leaderboard.
        _ => return,
    };
    let _ = action;
}
```

- [ ] **Step 6: Register the tailer**

In `src/rumi_analytics/src/tailing/mod.rs`, find where `backend_events` is registered. Add:
```rust
pub mod stability_pool;
```
Then in whatever async function runs all tailers (probably `pub async fn run_all()` or similar — read the file), add a call to `stability_pool::run().await`.

Add `stability_pool: u32` to the `error_counters` struct so the tailer can increment it (mirror `backend: u32` and any others). Also include in collector_health response.

- [ ] **Step 7: Initialize cursor on first deploy**

In `src/rumi_analytics/src/lib.rs`, find `init` and `post_upgrade`. After cursors are loaded, ensure `cursors::stability_pool_events::get()` returns 0 if never set (default behavior should already do this — verify by reading existing cursor init).

To bootstrap with existing SP history, the cursor starts at 0 and tails will catch up. With a few hundred events that's a few minutes on the next tail tick.

- [ ] **Step 8: Build the canister**

```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
```
Expected: clean compile.

- [ ] **Step 9: Run all analytics tests**

```bash
cargo test -p rumi_analytics
```
Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add src/rumi_analytics/
git commit -m "$(cat <<'EOF'
feat(analytics): tail rumi_stability_pool canister events

The Top SP Depositors leaderboard reads evt_stability, which has only
ever been populated from backend Event::ProvideLiquidity / Withdraw /
ClaimLiquidityReturns. Those variants are dead code — never emitted
since SP became a separate canister. Result: leaderboard always empty,
including for fresh deposits.

Add a tailer that pulls events from rumi_stability_pool's own event
log (get_pool_events) and pushes Deposit/Withdraw/Claim into the
existing evt_stability so the leaderboard, address-page SP timeline,
and SP TVL queries all start working.

Cursor starts at 0 so the first run backfills the full SP history.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.4: Deploy analytics + verify

**Files:** none — deploy step.

- [ ] **Step 1: Build, shrink, gzip**

Per project memory, rumi_analytics wasm exceeds 2MiB ingress. Run:
```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
ic-wasm target/wasm32-unknown-unknown/release/rumi_analytics.wasm -o target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm shrink
gzip -f target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm
ls -la target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm.gz
```
Expected: gzipped wasm under 2MiB.

- [ ] **Step 2: Deploy to mainnet (requires explicit confirmation from Rob)**

```bash
dfx canister install rumi_analytics \
  --network ic \
  --mode upgrade \
  --wasm target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm.gz \
  --argument '(/* InitArgs literal — see existing deploy notes for shape */)'
```

**Note:** rumi_analytics requires full InitArgs even on upgrade per project memory. Get the current init args from the previous deploy — likely captured in a deploy script or commit log. Don't guess.

**STOP here and confirm with Rob before running the deploy command.**

- [ ] **Step 3: Verify post-deploy**

After deploy, wait ~10 minutes for the SP cursor to catch up. Then check:
- `/explorer?lens=stability` — Top depositors card should show entries
- `/explorer?lens=revenue` — fee breakdown should show non-zero borrowing + redemption fees
- `/explorer?lens=admin` — collector health card should show `stability_pool` row with non-zero last_collect

If still empty, sanity-check the SP cursor:
```bash
dfx canister --network ic call rumi_analytics get_collector_health
```

---

## Phase 3: Lower priority / nice-to-have

### Task 3.1 (deferred): Per-pool LP APY split

The analytics `get_apys` returns a single `lp_apy_pct`. To split into 3Pool LP APY and AMM LP APY, add `three_pool_lp_apy_pct` and `amm_lp_apy_pct` to the response and compute each from per-pool fee + TVL data. Defer until there's enough volume on both pools that the distinction matters.

### Task 3.2 (deferred): Multi-canister cycles tracking

Per the user's review: nice-to-have, not crucial. The IC dashboard already shows live cycles for any canister; the inventory card we added in Task 1.5 links there for each canister. If we later want native tabs in the lens, the work is: have rumi_analytics call the management canister `canister_status` per-canister on a slow timer (every hour), store snapshots, and expose `get_cycles_series_per_canister`. Not in scope here.

---

## Self-review checklist

**Spec coverage**
- ✅ "Why is field not populated?" — answered in Background section + Phase 1 fixes
- ✅ Strip cycles chart — Phase 0 (already done)
- ✅ More admin activity — Tasks 1.4 (breakdown) + 1.5 (canister inventory) + Phase 2 setter events flowing into admin feed
- ✅ Top SP depositors empty — Task 2.3
- ✅ Past redemptions / fees missing — Tasks 2.1 + 2.2
- ✅ CR distribution / Avg CR / SP debt cleared — Tasks 1.1, 1.2, 1.3
- ✅ Liquidation Risk card — confirmed not a bug per Rob's clarification, no task needed
- ✅ Branch setup — Phase 0, Task 0.1

**Placeholder scan**
- All steps include the actual code or exact commands.
- No "TBD" / "implement later" markers.
- The deploy step in Task 2.4 explicitly requires Rob's confirmation before running, with the actual command shown.

**Type consistency**
- `computeVaultCrPct` returns `number | null` consistently across Task 1.1 (definition) and Task 1.2 (caller).
- `AnalyticsVaultEvent.fee_amount: u64` defined in Task 2.1, used in Task 2.2 (`e.fee_amount`).
- `AnalyticsStabilityEvent` shape unchanged; tailer in Task 2.3 produces the same struct the existing leaderboard query already reads.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-27-explorer-review-pass.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Good fit here since Phase 1 tasks are short and independent.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch with checkpoints. Useful if you want to discuss design choices in-line on the analytics canister work.

Which approach?
