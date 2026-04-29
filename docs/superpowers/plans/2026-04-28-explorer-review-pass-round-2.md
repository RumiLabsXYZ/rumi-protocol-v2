# Explorer Review Pass — Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. For each task whose root cause is unknown (D2, E2/F2, E3), use superpowers:systematic-debugging before writing code.

**Goal:** Resolve every leftover and new issue from Rob's round-2 Explorer review — broken bars/labels, dead cards, contradictory fee numbers, missing depositors, sparse Admin lens — by combining frontend lens cleanups with four targeted analytics canister additions, on top of the round-1 work already on `feat/explorer-review-pass`.

**Architecture:** The vast majority of items are frontend (label, layout, chart axis, fetch path). The genuine backend additions are localized to `rumi_analytics`: a new `get_fee_breakdown_window` query that aggregates fees from the raw `evt_vaults` / `evt_swaps` event logs (so the round-1 fee data flows immediately without touching the append-only daily-fees rollup history); a new `get_sp_depositor_principals` query that returns distinct principals from `evt_stability` (the C-family rework then asks the SP canister for authoritative balances per principal); and a new admin-gated `reset_error_counters` endpoint that lets us zero out the 12 leftover stability_pool counter increments from round-1's first deploy. No protocol-backend changes; no SP canister changes; no rollup migrations; no history deletion.

**Tech Stack:** SvelteKit + TypeScript (vault_frontend), Rust analytics canister (rumi_analytics) on the Internet Computer. Build/deploy via DFX; rumi_analytics wasm needs `ic-wasm shrink` + `gzip` before install per project memory.

**Branch:** `feat/explorer-review-pass` (16 commits, do not push until round 2 lands on top).

---

## Background

Round 1 shipped to mainnet. Both `rumi_analytics` (`dtlu2-uqaaa-aaaap-qugcq-cai`) and `vault_frontend` (`tcfua-yaaaa-aaaap-qrd7q-cai`) are live with the round-1 fixes. The brief at `docs/superpowers/plans/2026-04-27-explorer-review-pass-round-2-brief.md` enumerates all leftover items by lens. The four DECISION items the user weighed in on:

- **E4 (historical fee backfill):** The `daily_fees` log is `StableLog` (append-only, no random writes — confirmed in `src/rumi_analytics/src/storage/rollups.rs:108`). Backfilling historical rows would require either a storage migration or wiping the log. We agreed: never delete history. Instead, add a server-side `get_fee_breakdown_window(window_ns)` query that aggregates from `evt_vaults` + `evt_swaps` directly. Daily rollups continue accumulating untouched and remain canonical for daily series; the new query gives correct totals for any window immediately.
- **E5 (Treasury holdings card):** Frontend-direct via `icrc1_balance_of` per ledger using the treasury principal (`tlg74-oiaaa-aaaap-qrd6a-cai`) as account owner. Free, fast, current-by-construction. New `fetchTreasuryHoldings()` in `explorerService.ts`.
- **G1 (sparse Admin top card):** Keep + flesh out. Add Last admin action timestamp + Admin actions 24h count alongside Mode and Collector errors. Mode is too important to merge or drop. Don't add cycles back (round 1 dropped the cycles chart for a reason).
- **G3 (12 leftover stability_pool errors):** Add admin-gated `reset_error_counters(opt vec text)` endpoint to rumi_analytics. The admin-check pattern already exists at `src/rumi_analytics/src/lib.rs:262-265`. Pass a list of source names to selectively zero out counters; pass null to reset all.

For the C-family rework, the SP canister exposes `get_user_position : (opt principal) -> (opt UserStabilityPosition)` (verified in `src/declarations/rumi_stability_pool/rumi_stability_pool.did:245`). `UserStabilityPosition` carries everything we need: `stablecoin_balances`, `collateral_gains`, `opted_out_collateral`, `total_usd_value_e8s`. There's no `list_all_depositors` query, so the analytics canister will return distinct principals from event history; the frontend then asks SP per-principal for authoritative balances.

---

## File structure

**Frontend new files:**
- `src/vault_frontend/src/lib/components/explorer/SpCurrentDepositorsCard.svelte` — C1 snapshot
- `src/vault_frontend/src/lib/components/explorer/SpCoverageCard.svelte` — C4 per-collateral opt-in coverage
- `src/vault_frontend/src/lib/components/explorer/TreasuryHoldingsCard.svelte` — E5

**Frontend modified files:**
- `src/vault_frontend/src/lib/utils/displayEvent.ts` — A1 chip label
- `src/vault_frontend/src/lib/utils/explorerHelpers.ts` — F5 KNOWN_TOKENS label
- `src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte` — F3 single-dot fallback, F4 y-axis mode prop
- `src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte` — D2 redemption fetch widening
- `src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte` — B1, B2
- `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte` — replace top depositors and collateral-in-pool with new cards (C1, C4)
- `src/vault_frontend/src/lib/components/explorer/lenses/RedemptionsLens.svelte` — D1 single current RMR tile
- `src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte` — E1+E4 unified breakdown call, E5 mount, E3 verify post-fix
- `src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte` — F1 tooltip, F5 label fix, F6 row removal, E2/F2 (consumes fix from analytics)
- `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte` — G1 strip, G2 collector errors tooltip
- `src/vault_frontend/src/lib/services/explorer/analyticsService.ts` — fetchFeeBreakdownWindow, fetchSpDepositorPrincipals, resetErrorCounters
- `src/vault_frontend/src/lib/services/explorer/explorerService.ts` — fetchTreasuryHoldings, fetchCurrentSpDepositors

**Analytics canister modified files:**
- `src/rumi_analytics/src/queries/live.rs` — get_fee_breakdown_window, get_sp_depositor_principals
- `src/rumi_analytics/src/lib.rs` — register new queries + reset_error_counters update
- `src/rumi_analytics/src/types.rs` — FeeBreakdownQuery, FeeBreakdownResponse, ResetErrorCountersArgs
- `src/rumi_analytics/rumi_analytics.did` — declare new endpoints
- (Investigation only) `src/rumi_analytics/src/collectors/tvl.rs` and / or `queries/live.rs` for E2/F2 root cause

---

## Phase 1: Frontend cleanups (no backend dep)

### Task 1.1: A1 — AMM event chip label

The activity feed shows `+pool:fohh4-...cai_ryjl3-...cai` for AMM events. Replace with `🌊 3USD/ICP` so it matches the 3pool style.

**Files:**
- Modify: `src/vault_frontend/src/lib/utils/displayEvent.ts`

- [ ] **Step 1: Read the file to find the AMM source override added in round 1**

```bash
grep -n "AMM\|ammNaming\|pool:" src/vault_frontend/src/lib/utils/displayEvent.ts | head -20
```

Identify the function or constant that produces the AMM-event chip text.

- [ ] **Step 2: Replace the truncated-principal chip with a static token-pair label**

Use `getPoolPair()` from `$utils/ammNaming` (already imported / available) to get the friendly label. Replace the chip-text producer:

```typescript
// Old (round 1):
//   chip = `+pool:${truncatePrincipal(poolPrincipalA)}_${truncatePrincipal(poolPrincipalB)}`;
// New:
import { getPoolPair } from './ammNaming';
const pair = getPoolPair(poolPrincipal); // returns e.g. "3USD/ICP"
chip = `🌊 ${pair}`;
```

If `getPoolPair` doesn't yet expose this signature, extend it (it already has the registry seeded in `routes/explorer/activity/+page.svelte`). Look for the AMM index → token-pair mapping and surface a function that takes the pool principal directly.

- [ ] **Step 3: Type-check**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "displayEvent|ammNaming" | grep ERROR
```

Expected: no output.

- [ ] **Step 4: Smoke test**

Open `/explorer/activity` on local or mainnet. AMM rows should show `🌊 3USD/ICP` (or whatever the pair is) in the chip.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/utils/displayEvent.ts src/vault_frontend/src/lib/utils/ammNaming.ts
git commit -m "$(cat <<'EOF'
fix(explorer): readable AMM chip label (token pair, not truncated principals)

Activity feed showed "+pool:fohh4-...cai_ryjl3-...cai" for AMM events.
Use the existing pool registry to render "🌊 3USD/ICP" instead — wave
emoji matches the 3pool style, token pair is the actual content people
care about.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: F5 — fix "3USD LP/ICP" label to "3USD/ICP"

The AMM Pools card pair label says `3USD LP/ICP`. The `LP` suffix is redundant because 3USD itself IS the LP token.

**Files:**
- Modify: `src/vault_frontend/src/lib/utils/explorerHelpers.ts`

- [ ] **Step 1: Find the symbol entry in KNOWN_TOKENS**

```bash
grep -n "3USD LP\|3USD\|THREEPOOL\|fohh4-yyaaa" src/vault_frontend/src/lib/utils/explorerHelpers.ts | head
```

- [ ] **Step 2: Change the symbol from `3USD LP` to `3USD`**

In `KNOWN_TOKENS`, change the entry for the THREEPOOL principal (`fohh4-yyaaa-aaaap-qtkpa-cai`):

```typescript
[CANISTER_IDS.THREEPOOL, { symbol: '3USD', decimals: 8 }],
```

(Verify the pre-existing key shape — could be `{ symbol, decimals, name }` or similar — match it exactly.)

- [ ] **Step 3: Verify other consumers of getTokenSymbol still produce sensible labels**

```bash
grep -rn "getTokenSymbol\|3USD LP" src/vault_frontend/src/lib/ | head -20
```

If anything specifically expects `3USD LP` (e.g. a row label intentionally distinguishing the LP from the underlying), keep that local override. Otherwise let the registry change propagate.

- [ ] **Step 4: Smoke test**

Open `/explorer?lens=dexs`. The AMM Pools row should now read `3USD/ICP`, not `3USD LP/ICP`. Cross-check `/explorer/activity` and `/explorer/e/pool/<principal>` — wherever the symbol surfaces.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/utils/explorerHelpers.ts
git commit -m "$(cat <<'EOF'
fix(explorer): drop redundant "LP" suffix from 3USD token label

3USD is the 3pool LP token, so labelling it "3USD LP" produces awkward
pair labels like "3USD LP/ICP". Strip the suffix in KNOWN_TOKENS.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: F4 — MiniAreaChart y-axis mode prop

Round 1 anchored the y-axis at zero so sparse volume series stay readable. That's wrong for the 3pool virtual price chart, where values cluster at 1.063–1.064 and a chart from zero looks like a flat line. Add a `yAxisMode` prop with two modes.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte`

- [ ] **Step 1: Read the existing y-axis logic**

```bash
grep -n "min\|max\|y0\|y1\|yMin\|yMax\|range\|domain" src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte | head -30
```

Identify where round 1 forced y-anchoring at 0 (likely a `Math.min(0, ...)` or a hardcoded `yMin = 0`).

- [ ] **Step 2: Add prop + branching**

Add to the component's props:

```typescript
export let yAxisMode: 'zero-anchored' | 'data-fit' = 'zero-anchored';
```

Replace the y-min/max derivation:

```typescript
$: dataMin = Math.min(...values);
$: dataMax = Math.max(...values);
$: yMin = yAxisMode === 'zero-anchored' ? Math.min(0, dataMin) : dataMin * 0.999;
$: yMax = yAxisMode === 'zero-anchored' ? Math.max(0, dataMax) : dataMax * 1.001;
```

(The exact path expression depends on how `yMin`/`yMax` flow into the SVG. Match the existing variable names; don't introduce new ones.)

- [ ] **Step 3: Pass `yAxisMode="data-fit"` from the virtual price chart caller**

```bash
grep -rn "MiniAreaChart" src/vault_frontend/src/lib/components/explorer/ | grep -i "virtual\|vp\|three_pool"
```

For the virtual-price chart instance (look in `DexsLens.svelte` or wherever virtual price is rendered), pass `yAxisMode="data-fit"`.

- [ ] **Step 4: Smoke test**

`/explorer?lens=dexs`. Virtual price chart should now show a slope (zooming the y-axis to a tight range around 1.063–1.064). Volume charts should still anchor at 0.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): add data-fit y-axis mode to MiniAreaChart

Round 1's zero-anchored axis is right for volume but wrong for the
virtual price chart, where values barely move from 1.06. The slope was
invisible. Add a yAxisMode prop with 'zero-anchored' (default, for
non-negative volumes) and 'data-fit' (for slowly-changing quantities)
and use 'data-fit' for the virtual price chart.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: F3 — single-dot fallback in MiniAreaChart

When the entire 7-day series has only one non-zero point, the chart shows a single isolated dot that looks broken. Add a more obvious indicator for that case.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte`

- [ ] **Step 1: Detect the single-non-zero case**

In the script section, derive:

```typescript
$: nonZeroCount = values.filter((v) => v > 0).length;
$: isSingleEvent = nonZeroCount === 1;
```

- [ ] **Step 2: Render a vertical marker line at the single non-zero index**

In the SVG markup, when `isSingleEvent`, draw a vertical line + larger dot + a small annotation showing the value:

```svelte
{#if isSingleEvent}
  {@const i = values.findIndex((v) => v > 0)}
  {@const x = (i / (values.length - 1)) * width}
  {@const v = values[i]}
  <line x1={x} y1={0} x2={x} y2={height} stroke="rgb(45 212 191 / 0.4)" stroke-dasharray="2 2" />
  <circle cx={x} cy={height - ((v - yMin) / (yMax - yMin)) * height} r="3.5" fill="rgb(45 212 191)" />
{:else}
  <!-- existing path/dots rendering -->
{/if}
```

(Adjust to match the existing variable names — the actual `width`, `height`, `yMin`, `yMax` names already in scope.)

- [ ] **Step 3: Smoke test**

`/explorer?lens=dexs`. The 3pool swap volume chart with a single $70 point should now render a dashed vertical guide at that timestamp plus a clearer dot, instead of a lonely dot at 70 floating mid-chart.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): single-event chart fallback shows a vertical marker

When a 7d hourly series has just one non-zero point, the bare dot looks
broken. Render a dashed vertical guide + a larger dot at the event index
so the data point is unmistakable.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.5: F1 — Arb score tooltip

The DEXs lens shows an "Arb score" with no explanation. Add a tooltip translating the formula to plain English.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte`

- [ ] **Step 1: Find the formula in the analytics canister**

```bash
grep -rn "arb_score\|arbitrage" src/rumi_analytics/src/queries/live.rs | head -10
```

Read the math; translate to one sentence — typically something like "price deviation between pools weighted by pool depth" or "max profit available to a one-trade arbitrageur."

- [ ] **Step 2: Add the tooltip to the Arb score cell**

In `DexsLens.svelte`, find the Arb score cell:

```bash
grep -n "Arb score\|arb_score" src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte
```

Add a `title` attribute or an info-bubble icon:

```svelte
<span class="inline-flex items-center gap-1">
  Arb score
  <span
    class="text-gray-500 cursor-help text-xs"
    title="Estimated arbitrage opportunity between this pool and others. Higher = more price drift relative to depth. 0 means pools are aligned."
  >ⓘ</span>
</span>
```

(Replace the placeholder description with the real formula translation found in Step 1.)

- [ ] **Step 3: Smoke test**

`/explorer?lens=dexs`. Hovering "Arb score" should show the tooltip.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte
git commit -m "$(cat <<'EOF'
docs(explorer): explain Arb score with a tooltip

The DEXs lens showed a numeric Arb score with no context. Add an info
bubble that translates the analytics formula to plain English.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.6: F6 — remove redundant DEXs bottom row

Three small cards at the bottom of the DEXs lens duplicate the top strip. The third (Stability Pool APY) doesn't belong on the DEXs lens at all.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte`

- [ ] **Step 1: Find the offending row**

```bash
grep -n "PoolHealthStrip\|3pool balance\|3pool LP APY\|Stability pool APY" src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte
```

- [ ] **Step 2: Delete the `<PoolHealthStrip />` line and any unused import**

If `PoolHealthStrip` is only used here, remove the import too. Don't delete the component file — it's still used in the StabilityPool / Overview lenses.

- [ ] **Step 3: Smoke test**

`/explorer?lens=dexs`. The bottom row of three cards is gone; the top metrics (3pool balance, 3pool LP APY) still appear in the lens header.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/DexsLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): drop duplicate metric strip from DEXs lens

The bottom-row cards duplicated the top-strip metrics, and one (SP APY)
isn't even relevant to DEXs. Remove the row.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.7: D1 — single current RMR tile

Replace the `RMR floor → ceiling` (96% → 100%) range tile with a tile showing the active RMR plus an info tooltip.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/RedemptionsLens.svelte`

- [ ] **Step 1: Find the active-RMR derivation**

The active RMR depends on global CR vs `rmr_floor_cr` / `rmr_ceiling_cr`. Look for an existing query exposing it:

```bash
grep -rn "get_current_rmr\|active_rmr\|currentRmr" src/rumi_protocol_backend/src/ src/declarations/rumi_protocol_backend/ | head -10
```

If the backend already exposes a current-RMR query, call it. If not, compute it client-side from `protocolStatus.totalCollateralRatio` (or whatever it's named in the .did) and the four RMR config fields:

```typescript
function activeRmrPct(totalCr: number, floorCr: number, ceilingCr: number, floor: number, ceiling: number): number {
  if (totalCr >= ceilingCr) return ceiling;
  if (totalCr <= floorCr) return floor;
  const t = (totalCr - floorCr) / (ceilingCr - floorCr);
  return floor + t * (ceiling - floor);
}
```

(Confirm the linear interpolation matches the backend's `compute_rmr_for_global_cr` semantics — read `src/rumi_protocol_backend/src/state.rs` and grep for `rmr_floor_cr`.)

- [ ] **Step 2: Replace the range tile**

Find the floor → ceiling tile in `RedemptionsLens.svelte`:

```bash
grep -n "RMR\|rmr_floor\|rmr_ceiling" src/vault_frontend/src/lib/components/explorer/lenses/RedemptionsLens.svelte
```

Replace with a single-value tile:

```svelte
<div class="explorer-tile">
  <div class="text-xs text-gray-500 mb-1 inline-flex items-center gap-1">
    Current RMR
    <span class="cursor-help" title="Redemption Multiplier Ratio: the % of icUSD a redeemer receives in collateral. Slides linearly from {floor}% (when global CR is at or below {floorCr}%) to {ceiling}% (when global CR is at or above {ceilingCr}%). Higher = more collateral returned per icUSD redeemed.">ⓘ</span>
  </div>
  <div class="text-2xl font-semibold text-gray-100 tabular-nums">{activeRmr.toFixed(2)}%</div>
  <div class="text-xs text-gray-500 mt-1">at global CR {totalCrPct.toFixed(1)}%</div>
</div>
```

- [ ] **Step 3: Smoke test**

`/explorer?lens=redemptions`. Tile shows a single "Current RMR" with the active value, contextual sub-text, and a hover tooltip.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/RedemptionsLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): single current RMR tile with curve tooltip

The "RMR floor → ceiling" range tile read as meaningless to non-CDP
users. Replace with the active RMR value (interpolated from current
global CR), sub-text showing what CR drove it, and an info bubble that
explains the floor/ceiling mechanism.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.8: B1 — fix CR distribution histogram bar rendering

Round 1 fixed the empty-data bug (CR is computed client-side via the helper). Bars are still tiny slivers. Investigate height calc + parent container.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte`

- [ ] **Step 1: Use systematic-debugging — read the histogram render block**

```bash
grep -n "crBuckets\|h-40\|maxBucket\|height:" src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte | head -20
```

Read the affected block. Hypothesise: either the `pct = (b.count / maxBucket) * 100` produces tiny percentages (because one bucket dominates), the parent `.flex .items-end .h-40` has lost its height through tailwind class collision, or each bar has a min-height issue. Confirm with browser devtools after deploy or local: inspect the bar div's computed height and the parent's computed height.

- [ ] **Step 2: Apply the fix**

Most likely: the parent loses height because the lens layout puts the histogram in a flex child that doesn't grant explicit height. Two complementary fixes:

(a) Add a min-height to the bar so non-zero values render visibly:

```svelte
<div
  class="flex-1 bg-teal-400/40 hover:bg-teal-400/60 rounded-t transition-colors"
  style="height: {Math.max(pct, 2)}%"  
  title={`${b.label}: ${b.count} vault${b.count === 1 ? '' : 's'}`}
></div>
```

(b) Ensure the histogram container has explicit height:

```svelte
<div class="flex items-end gap-2 h-40 min-h-[10rem]">
  ...
</div>
```

- [ ] **Step 3: Smoke test**

`/explorer?lens=collateral`. Histogram bars should be clearly visible. Hovering shows the count tooltip.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): readable CR histogram bars

Round 1 fixed the empty-data bug by computing CR client-side, but bars
were still tiny slivers — both the dominant-bucket scaling and the
parent flex height contributed. Force a 2% minimum bar height for any
non-zero bucket and pin the container height explicitly.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 1.9: B2 — Liquidation Risk warning-zone vaults

The Liquidation Risk card is empty because `fetchLiquidatableVaults()` returns vaults at-or-below liquidation threshold. Rob wants the warning zone instead: vaults below min CR for borrowing but above the liquidation threshold.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte`

- [ ] **Step 1: Read existing data fetches in the lens**

```bash
grep -n "fetchLiquidatableVaults\|fetchAllVaults\|fetchCollateralConfigs\|liquidationRisk\|borrow_threshold" src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte | head -20
```

Confirm `allVaults`, `priceMap`, and `fetchCollateralConfigs` outputs are already in scope (they were after round 1's CR helper wire-up).

- [ ] **Step 2: Replace `fetchLiquidatableVaults` consumer with client-side filter**

After the round-1 CR computation loop, add a separate filter for warning-zone vaults:

```typescript
import { computeVaultCrPct } from '$utils/vaultCr';

// configs: from fetchCollateralConfigs() — each has borrow_threshold_ratio (min CR for borrow) and liquidation_ratio
const warningZone = [];
for (const v of allVaults) {
  const cr = computeVaultCrPct(v, priceMap, decimalsMap);
  if (cr == null) continue;
  const collType = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
  const cfg = configMap.get(collType);
  if (!cfg) continue;
  const minCrPct = Number(cfg.borrow_threshold_ratio) / 100; // ratio is e.g. 13000 bps for 130%
  const liqCrPct = Number(cfg.liquidation_ratio) / 100;
  if (cr < minCrPct && cr >= liqCrPct) {
    warningZone.push({ ...v, _cr: cr });
  }
}
```

(Verify the units of `borrow_threshold_ratio` and `liquidation_ratio` in the protocol-backend candid — could be bps, percent × 100, or already a ratio. Don't guess; read `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did`.)

- [ ] **Step 3: Pass `warningZone` to `LiquidationRiskTable` instead of `liquidatableVaults`**

```svelte
<LiquidationRiskTable vaults={warningZone} />
```

- [ ] **Step 4: Update `LiquidationRiskTable` if its prop name or shape needs changing**

```bash
grep -n "export let\|interface" src/vault_frontend/src/lib/components/explorer/LiquidationRiskTable.svelte | head
```

Make sure the table renders cleanly with the new shape. If it expected a specific field that doesn't exist (e.g. `debt_amount`), wire to the equivalent field on `Vault` or compute it.

- [ ] **Step 5: Smoke test**

`/explorer?lens=collateral`. The Liquidation Risk card should now show vaults in the warning zone (below min CR but above liquidation), if any exist. Confirm by cross-checking against any vault the UI currently flags with a warning icon.

- [ ] **Step 6: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/CollateralLens.svelte src/vault_frontend/src/lib/components/explorer/LiquidationRiskTable.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): Liquidation Risk shows warning-zone vaults

fetchLiquidatableVaults returns vaults at-or-below the liquidation
threshold — almost always empty in normal operation. Rob's intent for
this card is the warning zone: vaults below min CR for borrowing but
above the liquidation threshold (the same set that triggers the
warning icon on each vault).

Compute client-side from allVaults + priceMap + collateralConfigs,
filter to that band, and pass to LiquidationRiskTable.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: Investigate + fix data bugs (use systematic-debugging)

### Task 2.1: D2 — recent redemption activity panel empty

The lens-level redemption panel is empty even though `/explorer/activity` shows redemptions. Possible causes per the brief: filter mismatch, fetch limit too small, wrong source.

**Files:**
- Investigate: `src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte`
- Possibly modify the same file

- [ ] **Step 1: Use superpowers:systematic-debugging — verify the symptom**

Open `/explorer?lens=redemptions` on mainnet and `/explorer/activity?type=redemption_on_vaults` in another tab. Confirm the lens panel is empty while the activity feed shows redemption rows.

- [ ] **Step 2: Read the panel's filter logic**

```bash
grep -n "isBackendRedemption\|scope\s*===\s*['\"]redemptions\|redemption_on_vaults" src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte | head
```

Read the filter logic for `scope="redemptions"`. Three failure modes to check:
1. Filter expects a key that doesn't match the event variant (should be `redemption_on_vaults` snake_case after round 1)
2. Default `limit=12` is too small — most-recent 12 events don't include any redemptions
3. The fetch endpoint differs from what `/explorer/activity` uses

- [ ] **Step 3: Form a hypothesis and verify with the network response**

Hit the lens locally, open devtools → Network → find the `get_events_filtered` (or whatever) call. Inspect the response payload. If the filter matches but no redemption events are in the response, the limit is the issue. If redemption events are present but the panel filter rejects them, the filter is the issue.

- [ ] **Step 4: Apply the fix**

Most likely fix is widening the fetch for the `redemptions` scope specifically. In `LensActivityPanel.svelte`, where `scope === 'redemptions'`, either bump the fetch limit (e.g. 50) or call a redemption-specific fetch:

```typescript
const fetchLimit = scope === 'redemptions' ? 50 : 12;
const events = await fetchEventsFiltered({ types: TYPE_KEYS_FOR[scope], limit: fetchLimit });
```

Or if the issue is type mismatch, fix the filter set:

```typescript
const TYPE_KEYS_REDEMPTIONS = ['redemption_on_vaults'];
```

(Verify against the actual variant name in `src/rumi_analytics/src/storage/events.rs` — should be `Redeemed` in `VaultEventKind` and surfaced as `redeemed` or `redemption_on_vaults` in the activity-feed payload depending on serialization.)

- [ ] **Step 5: Smoke test**

`/explorer?lens=redemptions`. The Recent redemption activity panel should now show the most recent redemption events, matching what the activity feed page shows.

- [ ] **Step 6: Commit (write the commit body around the actual root cause found in Step 3)**

Template — fill in the diagnosis sentence:

```bash
git add src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): populate redemption activity panel on lens

[One sentence: what was actually broken — type-key mismatch / fetch
limit too narrow / wrong source endpoint — based on Step 3 finding.]

[One sentence: how the fix in Step 4 corrects it.]

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 2.2: E2 / F2 — 3Pool LP APY null root cause

Both lenses show a null LP APY. Round 1 introduced the live-APY computation; if it's null, fall back to analytics; if analytics is null, the chain breaks. Investigate `compute_lp_apy` and the TVL collector.

**Files:**
- Investigate: `src/rumi_analytics/src/queries/live.rs`, `src/rumi_analytics/src/collectors/tvl.rs`
- Modify: whichever of those is producing the None

- [ ] **Step 1: systematic-debugging — verify the symptom on-canister**

```bash
dfx canister --network ic call rumi_analytics get_apys
```

Inspect the response. Confirm `lp_apy_pct = null` (or `opt {}`).

- [ ] **Step 2: Read `compute_lp_apy`**

```bash
grep -n "compute_lp_apy\|lp_apy_pct" src/rumi_analytics/src/queries/live.rs | head
```

Read the function. Identify what input is None — likely the TVL snapshot reserve fields.

- [ ] **Step 3: Read the TVL collector**

```bash
grep -n "three_pool\|reserves\|TvlSnapshot\|push_tvl" src/rumi_analytics/src/collectors/tvl.rs
```

Three possible failure modes:
1. The collector isn't running (timer not registered)
2. The collector runs but errors out (`error_counters.three_pool > 0`)
3. The collector runs but writes None reserves

Check the live state:

```bash
dfx canister --network ic call rumi_analytics get_collector_health
```

Look at `last_collect_ns`, `errors.three_pool`, `source_counts.three_pool`.

- [ ] **Step 4: Form a hypothesis and apply a targeted fix**

Likely root causes (verify before fixing):
- Reserve fields renamed in the 3pool candid; the collector's shadow type is stale → fix shadow type
- Reserve fields wrapped in `opt` upstream → collector unwraps wrong → adjust
- The fee/volume fields used in APY math have wrong units → fix units

Make the targeted change. Don't speculatively rewrite — fix only the field that's None.

- [ ] **Step 5: Build + test**

```bash
cargo test -p rumi_analytics
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
```

- [ ] **Step 6: Commit (write the commit body around the actual root cause found in Step 3-4)**

Template:

```bash
git add src/rumi_analytics/
git commit -m "$(cat <<'EOF'
fix(analytics): 3pool LP APY no longer null

[One sentence: which input was None — empty reserves / wrong field
name / collector erroring — and where the gap was.]

[One sentence: the targeted fix in Step 4.]

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

(Defer deploy until Phase 7 — bundle with the other analytics changes.)

### Task 2.3: E3 — SP APY 4.72% root cause

The lens still shows the analytics 7d fallback (4.72%) instead of the live 6.24%. Round 1's `liveSpApy` derivation depends on `protocolStatus.interestSplit` and `protocolStatus.perCollateralInterest`; one of those is presumably empty.

**Files:**
- Investigate: `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte`
- Possibly modify: same file or `protocolStatus` consumer

- [ ] **Step 1: systematic-debugging — instrument the derivation**

In `StabilityPoolLens.svelte`, temporarily log the inputs:

```typescript
const liveSpApy = $derived.by(() => {
  if (!protocolStatus) return null;
  const split = protocolStatus.interestSplit ?? [];
  const perC = protocolStatus.perCollateralInterest;
  console.log('[SP APY debug]', { split, perC, totalDebt: protocolStatus.totalDebt });
  // ...existing logic
});
```

Open `/explorer?lens=stability` and read the console.

- [ ] **Step 2: Identify which input is empty**

If `interestSplit` is `[]` or `perCollateralInterest` is `undefined`, the live formula returns null and we fall back. Check the candid mapping:

```bash
grep -n "interest_split\|per_collateral_interest" src/declarations/rumi_protocol_backend/rumi_protocol_backend.did
grep -n "interestSplit\|perCollateralInterest" src/vault_frontend/src/lib/services/explorer/explorerService.ts
```

The candid uses snake_case; the frontend may be reading the wrong field name. Or the backend is returning an empty vec.

- [ ] **Step 3: Apply the fix**

If it's a frontend mapping bug, fix the field access. If the backend is returning empty, that's a deeper issue — log the protocol status raw response and verify.

- [ ] **Step 4: Smoke test**

`/explorer?lens=stability`. SP APY should now show the live value (~6.24% per Rob's expectation), matching the /liquidity tab.

- [ ] **Step 5: Remove debug logging + commit (write commit body around the actual finding from Step 2)**

Template:

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): SP APY uses live formula instead of 7d fallback

[One sentence: which input — interestSplit / perCollateralInterest /
totalDebt — was empty or wrong-cased, and why that caused the live
derivation to return null.]

[One sentence: the targeted fix.]

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: Analytics canister additions

### Task 3.1: E4 — `get_fee_breakdown_window` query

Add a server-side aggregation that reads `evt_vaults` + `evt_swaps` directly. Frontend uses this for both the 90d total and the 24h total — same methodology, no contradiction (E1 piggybacks on this).

**Files:**
- Modify: `src/rumi_analytics/src/types.rs`
- Modify: `src/rumi_analytics/src/queries/live.rs`
- Modify: `src/rumi_analytics/src/lib.rs`
- Modify: `src/rumi_analytics/rumi_analytics.did`

- [ ] **Step 1: Add the query types**

In `src/rumi_analytics/src/types.rs`, append:

```rust
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct FeeBreakdownQuery {
    pub window_ns: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct FeeBreakdownResponse {
    pub borrow_fees_icusd_e8s: u64,
    pub redemption_fees_icusd_e8s: u64,
    pub swap_fees_icusd_e8s: u64,
    pub borrow_count: u32,
    pub redemption_count: u32,
    pub swap_count: u32,
    pub start_ns: u64,
    pub end_ns: u64,
}
```

- [ ] **Step 2: Implement the query**

In `src/rumi_analytics/src/queries/live.rs`, add:

```rust
use crate::storage::events::{evt_vaults, evt_swaps, VaultEventKind};
use crate::types::{FeeBreakdownQuery, FeeBreakdownResponse};

pub fn get_fee_breakdown_window(query: FeeBreakdownQuery) -> FeeBreakdownResponse {
    let now = ic_cdk::api::time();
    let start = match query.window_ns {
        Some(window) => now.saturating_sub(window),
        None => 0,
    };

    let vault_events = evt_vaults::range(start, now, usize::MAX);
    let swap_events = evt_swaps::range(start, now, usize::MAX);

    let mut borrow_fees: u64 = 0;
    let mut redemption_fees: u64 = 0;
    let mut borrow_count: u32 = 0;
    let mut redemption_count: u32 = 0;
    for e in &vault_events {
        match e.event_kind {
            VaultEventKind::Borrowed => {
                borrow_fees = borrow_fees.saturating_add(e.fee_amount);
                borrow_count += 1;
            }
            VaultEventKind::Redeemed => {
                redemption_fees = redemption_fees.saturating_add(e.fee_amount);
                redemption_count += 1;
            }
            _ => {}
        }
    }

    let swap_fees: u64 = swap_events.iter().map(|e| e.fee).sum();
    let swap_count = swap_events.len() as u32;

    FeeBreakdownResponse {
        borrow_fees_icusd_e8s: borrow_fees,
        redemption_fees_icusd_e8s: redemption_fees,
        swap_fees_icusd_e8s: swap_fees,
        borrow_count,
        redemption_count,
        swap_count,
        start_ns: start,
        end_ns: now,
    }
}
```

- [ ] **Step 3: Register the query in lib.rs**

In `src/rumi_analytics/src/lib.rs`, near the other `#[query]` declarations:

```rust
#[query]
fn get_fee_breakdown_window(query: types::FeeBreakdownQuery) -> types::FeeBreakdownResponse {
    queries::live::get_fee_breakdown_window(query)
}
```

- [ ] **Step 4: Add to the .did file**

In `src/rumi_analytics/rumi_analytics.did`, declare:

```candid
type FeeBreakdownQuery = record {
  window_ns : opt nat64;
};

type FeeBreakdownResponse = record {
  borrow_fees_icusd_e8s : nat64;
  redemption_fees_icusd_e8s : nat64;
  swap_fees_icusd_e8s : nat64;
  borrow_count : nat32;
  redemption_count : nat32;
  swap_count : nat32;
  start_ns : nat64;
  end_ns : nat64;
};

// In the service block:
get_fee_breakdown_window : (FeeBreakdownQuery) -> (FeeBreakdownResponse) query;
```

- [ ] **Step 5: Build to confirm**

```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
```

Expected: clean compile.

- [ ] **Step 6: Add a unit test**

In `src/rumi_analytics/src/queries/live.rs` or the appropriate test module:

```rust
#[test]
fn fee_breakdown_window_zero_for_empty_logs() {
    // Note: this test asserts the math works on empty inputs.
    // Production data tests live in pocket_ic.
    let q = FeeBreakdownQuery { window_ns: Some(86_400_000_000_000) };
    let r = get_fee_breakdown_window(q);
    assert_eq!(r.borrow_fees_icusd_e8s, 0);
    assert_eq!(r.swap_count, 0);
}
```

```bash
cargo test -p rumi_analytics fee_breakdown_window
```

- [ ] **Step 7: Commit**

```bash
git add src/rumi_analytics/src/types.rs src/rumi_analytics/src/queries/live.rs src/rumi_analytics/src/lib.rs src/rumi_analytics/rumi_analytics.did
git commit -m "$(cat <<'EOF'
feat(analytics): get_fee_breakdown_window query

Aggregates fees from evt_vaults and evt_swaps for any time window.
Used by the Revenue lens for both the 90d total and the 24h total
so the two metrics share methodology — fixes the contradiction where
24h fees > 90d fees due to one being live-estimated and the other
read from null-stubbed historical rollups.

Reads raw event logs rather than rollups, so it works correctly even
for windows that overlap pre-deploy days when borrowing_fees_e8s was
None. The daily_fees rollup remains canonical for daily series; this
query supplements with on-demand window aggregation.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 3.2: C2 — `get_sp_depositor_principals` query

Returns distinct principals from `evt_stability` so the frontend can ask SP per-principal for authoritative balances.

**Files:**
- Modify: `src/rumi_analytics/src/queries/live.rs`
- Modify: `src/rumi_analytics/src/lib.rs`
- Modify: `src/rumi_analytics/rumi_analytics.did`

- [ ] **Step 1: Implement the query**

In `src/rumi_analytics/src/queries/live.rs`:

```rust
use std::collections::HashSet;
use candid::Principal;
use crate::storage::events::evt_stability;

pub fn get_sp_depositor_principals() -> Vec<Principal> {
    let events = evt_stability::range(0, u64::MAX, usize::MAX);
    let mut set: HashSet<Principal> = HashSet::new();
    for e in events {
        set.insert(e.caller);
    }
    set.into_iter().collect()
}
```

- [ ] **Step 2: Register in lib.rs**

```rust
#[query]
fn get_sp_depositor_principals() -> Vec<Principal> {
    queries::live::get_sp_depositor_principals()
}
```

- [ ] **Step 3: Add to .did**

```candid
get_sp_depositor_principals : () -> (vec principal) query;
```

- [ ] **Step 4: Build + test**

```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
```

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/queries/live.rs src/rumi_analytics/src/lib.rs src/rumi_analytics/rumi_analytics.did
git commit -m "$(cat <<'EOF'
feat(analytics): get_sp_depositor_principals query

Returns the distinct set of principals that have ever appeared in an
evt_stability event. The frontend uses this list to fan out per-user
SP queries (get_user_position) and reconstruct the current depositor
snapshot — replaces the round-1 leaderboard query that filtered out
anyone whose deposit total in the recent window was 0, even if their
balance was still > 0.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 3.3: G3 — `reset_error_counters` admin endpoint

Admin-gated update endpoint to zero specific or all source error counters.

**Files:**
- Modify: `src/rumi_analytics/src/types.rs`
- Modify: `src/rumi_analytics/src/lib.rs`
- Modify: `src/rumi_analytics/rumi_analytics.did`

- [ ] **Step 1: Add args type**

In `types.rs`:

```rust
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ResetErrorCountersArgs {
    /// null = reset all sources; else only the listed source names.
    /// Valid names: "backend", "stability_pool", "three_pool", "icusd_ledger".
    pub sources: Option<Vec<String>>,
}
```

- [ ] **Step 2: Add the endpoint in lib.rs**

```rust
#[update]
fn reset_error_counters(args: types::ResetErrorCountersArgs) -> Result<(), String> {
    let admin = state::read_state(|s| s.admin);
    let caller = ic_cdk::caller();
    if caller != admin {
        return Err(format!("unauthorized: caller {} is not admin", caller));
    }
    state::mutate_state(|s| {
        let reset_all = args.sources.is_none();
        let sources = args.sources.unwrap_or_default();
        let touch = |name: &str| reset_all || sources.iter().any(|s| s == name);
        if touch("backend") { s.error_counters.backend = 0; }
        if touch("stability_pool") { s.error_counters.stability_pool = 0; }
        if touch("three_pool") { s.error_counters.three_pool = 0; }
        if touch("icusd_ledger") { s.error_counters.icusd_ledger = 0; }
    });
    Ok(())
}
```

(Verify the exact field names in `ErrorCounters` first:

```bash
grep -n "pub struct ErrorCounters\|pub backend\|pub stability_pool\|pub three_pool\|pub icusd_ledger" src/rumi_analytics/src/storage/mod.rs
```

If a field name differs, adjust.)

- [ ] **Step 3: Add to .did**

```candid
type ResetErrorCountersArgs = record {
  sources : opt vec text;
};

reset_error_counters : (ResetErrorCountersArgs) -> (variant { Ok; Err : text });
```

- [ ] **Step 4: Build + test**

```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
cargo test -p rumi_analytics
```

- [ ] **Step 5: Commit**

```bash
git add src/rumi_analytics/src/types.rs src/rumi_analytics/src/lib.rs src/rumi_analytics/rumi_analytics.did
git commit -m "$(cat <<'EOF'
feat(analytics): admin-gated reset_error_counters endpoint

Operational primitive for zeroing error counters after a known fix
ships. Pass null to reset all, or a list like vec {"stability_pool"}
to selectively reset specific sources. Caller must equal the admin
principal stored in state.

Used in the round-2 deploy to clear the 12 leftover stability_pool
counter increments from round 1's first deploy (which failed at
runtime because the SP candid was missing 11 PoolEventType variants —
fix shipped in round-1 commit b6e59a5).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4: Stability Pool lens rework (depends on Phase 3 deploy)

### Task 4.1: Service: `fetchCurrentSpDepositors`

Combines the new analytics principal-list query with per-principal SP `get_user_position` calls in parallel.

**Files:**
- Modify: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`
- Modify: `src/vault_frontend/src/lib/services/explorer/explorerService.ts`

- [ ] **Step 1: Add `fetchSpDepositorPrincipals` to analyticsService**

```typescript
export async function fetchSpDepositorPrincipals(): Promise<Principal[]> {
  const actor = getAnalyticsActor();
  return await actor.get_sp_depositor_principals();
}
```

- [ ] **Step 2: Add `fetchCurrentSpDepositors` to explorerService**

```typescript
import { fetchSpDepositorPrincipals } from './analyticsService';
import { getStabilityPoolActor } from '$lib/canisters/stabilityPool';
import type { UserStabilityPosition } from '$declarations/rumi_stability_pool/rumi_stability_pool.did';

export type CurrentSpDepositor = {
  principal: Principal;
  position: UserStabilityPosition;
};

export async function fetchCurrentSpDepositors(): Promise<CurrentSpDepositor[]> {
  const principals = await fetchSpDepositorPrincipals();
  const sp = getStabilityPoolActor();
  const positions = await Promise.all(
    principals.map((p) => sp.get_user_position([p]).catch(() => [])),
  );
  return positions
    .map((maybe, i) => {
      const pos = Array.isArray(maybe) ? maybe[0] : maybe;
      return pos ? { principal: principals[i], position: pos as UserStabilityPosition } : null;
    })
    .filter((row): row is CurrentSpDepositor => row !== null && Number(row.position.total_usd_value_e8s) > 0)
    .sort((a, b) => Number(b.position.total_usd_value_e8s - a.position.total_usd_value_e8s));
}
```

(Verify the SP actor accessor — likely already exists; if not, add a small wrapper that builds an `Actor.createActor` against the SP canister.)

- [ ] **Step 3: Type-check**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "explorerService|analyticsService" | grep ERROR
```

Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/services/explorer/analyticsService.ts src/vault_frontend/src/lib/services/explorer/explorerService.ts
git commit -m "$(cat <<'EOF'
feat(explorer): fetchCurrentSpDepositors service

Combines the new analytics get_sp_depositor_principals (distinct
principals from evt_stability) with parallel SP get_user_position
calls. Returns active depositors only (total_usd_value_e8s > 0)
sorted by USD value descending. The SP canister is the source of
truth for current balances; analytics just supplies the principal
set to fan out over.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.2: C1 — Current Depositors snapshot card

Replace the time-windowed Top Depositors leaderboard with a snapshot table that shows every active depositor with per-token columns (icUSD, 3USD, ckUSDT, ckUSDC). No 3USD-into-icUSD normalization.

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/SpCurrentDepositorsCard.svelte`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte`

- [ ] **Step 1: Create the card component**

`SpCurrentDepositorsCard.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchCurrentSpDepositors, type CurrentSpDepositor } from '$services/explorer/explorerService';
  import { getTokenSymbol, getTokenDecimals } from '$utils/explorerHelpers';
  import EntityLink from './EntityLink.svelte';

  let depositors: CurrentSpDepositor[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // Distinct token principals across all depositors → column headers
  const tokens = $derived.by(() => {
    const set = new Set<string>();
    for (const d of depositors) {
      for (const [token] of d.position.stablecoin_balances) {
        set.add(token.toText());
      }
    }
    return Array.from(set);
  });

  function balanceOf(d: CurrentSpDepositor, token: string): bigint {
    for (const [t, amt] of d.position.stablecoin_balances) {
      if (t.toText() === token) return amt;
    }
    return 0n;
  }

  function fmt(amt: bigint, token: string): string {
    const decimals = getTokenDecimals(token) ?? 8;
    if (amt === 0n) return '';
    const v = Number(amt) / Math.pow(10, decimals);
    return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  onMount(async () => {
    try {
      depositors = await fetchCurrentSpDepositors();
    } catch (err: any) {
      console.error('[SpCurrentDepositorsCard] load failed:', err);
      error = err?.message ?? String(err);
    } finally {
      loading = false;
    }
  });
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Current depositors</h3>
  <p class="text-xs text-gray-500 mb-3">
    Every principal currently holding a non-zero SP balance. Per-token columns; sort by total USD value.
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if error}
    <p class="text-sm text-red-400 py-2">Failed to load: {error}</p>
  {:else if depositors.length === 0}
    <p class="text-sm text-gray-500 py-2">No active depositors.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-white/5">
          <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Depositor</th>
          {#each tokens as t (t)}
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">{getTokenSymbol(t)}</th>
          {/each}
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Total USD</th>
        </tr>
      </thead>
      <tbody>
        {#each depositors as d (d.principal.toText())}
          <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
            <td class="py-2 px-2 font-mono text-xs">
              <EntityLink type="principal" value={d.principal.toText()} />
            </td>
            {#each tokens as t (t)}
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">{fmt(balanceOf(d, t), t)}</td>
            {/each}
            <td class="py-2 px-2 text-right tabular-nums text-gray-100 font-medium">
              ${(Number(d.position.total_usd_value_e8s) / 1e8).toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>
```

- [ ] **Step 2: Mount in StabilityPoolLens**

```bash
grep -n "TopDepositorsCard\|fetchTopSpDepositors\|leaderboard\|top depositors" src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte | head
```

Replace the Top Depositors render block with `<SpCurrentDepositorsCard />`. Remove now-unused imports + state if they were only for the old leaderboard.

- [ ] **Step 3: Type-check**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "SpCurrentDepositorsCard|StabilityPoolLens" | grep ERROR
```

Expected: no output.

- [ ] **Step 4: Smoke test (post-deploy)**

After the analytics deploy in Phase 7, open `/explorer?lens=stability`. The new "Current depositors" card should show every active depositor with per-token columns. Specifically: Rob's principal should appear with a non-zero icUSD + 3USD balance (verifies C3).

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/SpCurrentDepositorsCard.svelte src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): SP current depositors snapshot card

Drop the time-windowed Top Depositors leaderboard (which filtered out
anyone with no recent deposit activity even if their balance was
non-zero) and replace with a snapshot of every principal currently
holding an SP position. Per-token columns (icUSD, 3USD, ckUSDT,
ckUSDC, ...) sourced from the SP canister's get_user_position — no
3USD→icUSD normalization. Sorted by total USD value descending.

Fixes the 4-vs-1 depositor mismatch (C2) and Rob-shows-zero (C3) at
the source: SP balances are authoritative, analytics events just
supply the principal set.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 4.3: C4 — Per-collateral opt-in coverage card

For each supported collateral, show what % of depositors are opted in and the total stable backing they provide.

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/SpCoverageCard.svelte`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte`

- [ ] **Step 1: Create the card component**

`SpCoverageCard.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchCurrentSpDepositors, type CurrentSpDepositor } from '$services/explorer/explorerService';
  import { fetchCollateralConfigs } from '$services/explorer/explorerService';
  import { getTokenSymbol } from '$utils/explorerHelpers';

  let depositors: CurrentSpDepositor[] = $state([]);
  let collaterals: string[] = $state([]); // principal text
  let loading = $state(true);

  function isOptedIn(d: CurrentSpDepositor, collType: string): boolean {
    return !d.position.opted_out_collateral.some((p) => p.toText() === collType);
  }

  function totalStableUsd(d: CurrentSpDepositor): number {
    // total_usd_value_e8s already sums stables; treat 1e8 as $1
    return Number(d.position.total_usd_value_e8s) / 1e8;
  }

  const rows = $derived.by(() => {
    return collaterals.map((c) => {
      const optedIn = depositors.filter((d) => isOptedIn(d, c));
      const pct = depositors.length > 0 ? (optedIn.length / depositors.length) * 100 : 0;
      const usd = optedIn.reduce((s, d) => s + totalStableUsd(d), 0);
      return { collateral: c, pct, usd, count: optedIn.length, total: depositors.length };
    });
  });

  onMount(async () => {
    try {
      const [d, configs] = await Promise.all([
        fetchCurrentSpDepositors(),
        fetchCollateralConfigs(),
      ]);
      depositors = d;
      collaterals = configs.map((c) => c.collateral_type?.toText?.() ?? String(c.collateral_type));
    } catch (err) {
      console.error('[SpCoverageCard] load failed:', err);
    } finally {
      loading = false;
    }
  });
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Per-collateral opt-in coverage</h3>
  <p class="text-xs text-gray-500 mb-3">
    For each supported collateral, the share of SP depositors who haven't opted out and the total stable backing they provide.
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if rows.length === 0}
    <p class="text-sm text-gray-500 py-2">No collaterals configured.</p>
  {:else}
    <div class="space-y-2">
      {#each rows as r (r.collateral)}
        <div class="flex items-baseline justify-between text-sm">
          <span class="text-gray-200 font-medium w-16">{getTokenSymbol(r.collateral)}</span>
          <span class="tabular-nums text-gray-400">{r.count}/{r.total} opted in ({r.pct.toFixed(0)}%)</span>
          <span class="tabular-nums text-gray-100 font-medium">
            ${r.usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</div>
```

- [ ] **Step 2: Mount in StabilityPoolLens**

Find the "Collateral in pool" card render in `StabilityPoolLens.svelte` and replace it with `<SpCoverageCard />`. Remove old fetch state if unused.

- [ ] **Step 3: Type-check + smoke**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "SpCoverageCard|StabilityPoolLens" | grep ERROR
```

`/explorer?lens=stability` after deploy: card shows e.g. "ICP 100% $1,000 / BOB 50% $500".

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/SpCoverageCard.svelte src/vault_frontend/src/lib/components/explorer/lenses/StabilityPoolLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): per-collateral opt-in coverage card

Replaces the unstructured "Collateral in pool" card. For each supported
collateral, shows the % of SP depositors who are still opted in (would
absorb a liquidation in that collateral) and the total stable backing
they supply. Computed client-side from current SP positions (which
include opted_out_collateral) — no analytics changes needed.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5: Revenue lens rework

### Task 5.1: E1 + E4 — wire fee breakdown query for both 24h and 90d

Same methodology, no contradiction. Replace both the live-estimated 24h fees and the rollup-summed 90d fees with two calls to `get_fee_breakdown_window`.

**Files:**
- Modify: `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte`

- [ ] **Step 1: Add the service wrapper**

```typescript
export type FeeBreakdown = {
  borrowIcusd: number;
  redemptionIcusd: number;
  swapIcusd: number;
  borrowCount: number;
  redemptionCount: number;
  swapCount: number;
  startNs: bigint;
  endNs: bigint;
};

const NS_PER_DAY = 86_400n * 1_000_000_000n;

export async function fetchFeeBreakdownWindow(windowDays: number | null): Promise<FeeBreakdown> {
  const actor = getAnalyticsActor();
  const window_ns: [] | [bigint] = windowDays == null ? [] : [BigInt(windowDays) * NS_PER_DAY];
  const r = await actor.get_fee_breakdown_window({ window_ns });
  return {
    borrowIcusd: Number(r.borrow_fees_icusd_e8s) / 1e8,
    redemptionIcusd: Number(r.redemption_fees_icusd_e8s) / 1e8,
    swapIcusd: Number(r.swap_fees_icusd_e8s) / 1e8,
    borrowCount: Number(r.borrow_count),
    redemptionCount: Number(r.redemption_count),
    swapCount: Number(r.swap_count),
    startNs: r.start_ns,
    endNs: r.end_ns,
  };
}
```

- [ ] **Step 2: Replace fee derivations in RevenueLens**

```bash
grep -n "fees24h\|totalFees\|totalBorrow\|totalRedemption\|totalSwap\|estimatedDailyBorrow" src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte | head
```

Replace the `feeRows`-based 90d derivation and the `estimatedDailyBorrow`-based 24h derivation with two calls:

```typescript
let fees24h: FeeBreakdown | null = $state(null);
let fees90d: FeeBreakdown | null = $state(null);

onMount(async () => {
  // ...existing fetches
  const [r24, r90] = await Promise.all([
    fetchFeeBreakdownWindow(1),
    fetchFeeBreakdownWindow(90),
  ]);
  fees24h = r24;
  fees90d = r90;
});

const totalBorrow = $derived(fees90d?.borrowIcusd ?? 0);
const totalRedemption = $derived(fees90d?.redemptionIcusd ?? 0);
const totalSwap = $derived(fees90d?.swapIcusd ?? 0);
const totalFees = $derived(totalBorrow + totalRedemption + totalSwap);
const fees24hTotal = $derived((fees24h?.borrowIcusd ?? 0) + (fees24h?.redemptionIcusd ?? 0) + (fees24h?.swapIcusd ?? 0));
```

Remove the now-dead `feeRows.reduce(...)` aggregations and the `estimatedDailyBorrow` derivation. Keep the daily-rollup chart as-is — that still uses the rollup data for the time-series view, which is the rollup's actual purpose.

- [ ] **Step 3: Type-check + smoke**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep "RevenueLens" | grep ERROR
```

Open `/explorer?lens=revenue` after the analytics deploy. The 24h fees and 90d fees should be consistent (90d ≥ 24h). Borrow + redemption fees should show real numbers, not $0.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/lib/services/explorer/analyticsService.ts src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte
git commit -m "$(cat <<'EOF'
fix(explorer): unified fee breakdown for both 24h and 90d cards

Replace the live-estimated 24h fees + rollup-summed 90d fees (which
contradicted each other because one was forward-looking and the other
read null-stubbed historical rollups) with two calls to the new
get_fee_breakdown_window query. Both totals now share methodology and
read directly from evt_vaults + evt_swaps.

90d total now reflects real borrow + redemption fees from the entire
window, not just post-round-1 days.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task 5.2: E5 — Treasury holdings card

Frontend-direct ledger queries via `icrc1_balance_of` using the treasury principal as account owner.

**Files:**
- Create: `src/vault_frontend/src/lib/components/explorer/TreasuryHoldingsCard.svelte`
- Modify: `src/vault_frontend/src/lib/services/explorer/explorerService.ts`
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte`

- [ ] **Step 1: Add `fetchTreasuryHoldings` to explorerService**

```typescript
import { Principal } from '@dfinity/principal';
import { CANISTER_IDS } from '$lib/config';

const TREASURY_PRINCIPAL = Principal.fromText('tlg74-oiaaa-aaaap-qrd6a-cai');

export type TreasuryHolding = {
  symbol: string;
  ledger: string;
  balanceE8s: bigint;
  decimals: number;
  usd: number;
};

const TRACKED_LEDGERS: Array<{ symbol: string; principal: string; decimals: number; usdEachE8s: number }> = [
  { symbol: 'icUSD', principal: CANISTER_IDS.ICUSD_LEDGER, decimals: 8, usdEachE8s: 1 },
  { symbol: 'ckUSDT', principal: 'cngnf-vqaaa-aaaar-qag4q-cai', decimals: 6, usdEachE8s: 1 },
  { symbol: 'ckUSDC', principal: 'xevnm-gaaaa-aaaar-qafnq-cai', decimals: 6, usdEachE8s: 1 },
  { symbol: 'ICP', principal: 'ryjl3-tyaaa-aaaaa-aaaba-cai', decimals: 8, usdEachE8s: -1 /* live price */ },
];

export async function fetchTreasuryHoldings(icpPriceUsd: number): Promise<TreasuryHolding[]> {
  const balances = await Promise.all(
    TRACKED_LEDGERS.map(async (l) => {
      try {
        const actor = getIcrc1Actor(l.principal); // small wrapper
        const balance = await actor.icrc1_balance_of({ owner: TREASURY_PRINCIPAL, subaccount: [] });
        const v = Number(balance) / Math.pow(10, l.decimals);
        const usd = l.usdEachE8s === -1 ? v * icpPriceUsd : v * l.usdEachE8s;
        return { symbol: l.symbol, ledger: l.principal, balanceE8s: balance, decimals: l.decimals, usd };
      } catch (err) {
        console.error(`[fetchTreasuryHoldings] ${l.symbol} failed:`, err);
        return { symbol: l.symbol, ledger: l.principal, balanceE8s: 0n, decimals: l.decimals, usd: 0 };
      }
    }),
  );
  return balances.filter((b) => b.balanceE8s > 0n);
}
```

(If `getIcrc1Actor(principal)` doesn't yet exist, write a small inline wrapper using `Actor.createActor` with the standard ICRC-1 interface idl. There may already be one — grep first.)

- [ ] **Step 2: Create the card**

`TreasuryHoldingsCard.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchTreasuryHoldings, type TreasuryHolding, fetchProtocolStatus } from '$services/explorer/explorerService';

  let holdings: TreasuryHolding[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      const status = await fetchProtocolStatus();
      const icpPrice = Number(status?.last_icp_rate ?? status?.icpPriceE8s ?? 0n) / 1e8;
      holdings = await fetchTreasuryHoldings(icpPrice);
    } catch (err) {
      console.error('[TreasuryHoldingsCard] load failed:', err);
    } finally {
      loading = false;
    }
  });

  const totalUsd = $derived(holdings.reduce((s, h) => s + h.usd, 0));
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Treasury holdings</h3>
  <p class="text-xs text-gray-500 mb-3">
    Live token balances of the rumi_treasury canister.
    <code class="text-gray-600">tlg74-oiaaa-aaaap-qrd6a-cai</code>
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if holdings.length === 0}
    <p class="text-sm text-gray-500 py-2">No tracked balances.</p>
  {:else}
    <table class="w-full text-sm">
      <tbody>
        {#each holdings as h (h.ledger)}
          <tr class="border-b border-white/[0.03]">
            <td class="py-1.5 px-2 text-gray-200">{h.symbol}</td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-300">
              {(Number(h.balanceE8s) / Math.pow(10, h.decimals)).toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-100 font-medium">
              ${h.usd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
          </tr>
        {/each}
        <tr class="border-t border-white/10">
          <td class="py-1.5 px-2 text-xs text-gray-500 uppercase">Total</td>
          <td></td>
          <td class="py-1.5 px-2 text-right tabular-nums text-teal-300 font-semibold">
            ${totalUsd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          </td>
        </tr>
      </tbody>
    </table>
  {/if}
</div>
```

- [ ] **Step 3: Mount in RevenueLens**

Find the existing Treasury card (the "Pending interest = 0 / Flush threshold = 0" tile or block) and replace it with `<TreasuryHoldingsCard />`.

- [ ] **Step 4: Type-check + smoke**

```bash
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -E "TreasuryHoldingsCard|RevenueLens|explorerService" | grep ERROR
```

`/explorer?lens=revenue` after deploy: card shows real balances per token in the treasury, plus total USD.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/components/explorer/TreasuryHoldingsCard.svelte src/vault_frontend/src/lib/services/explorer/explorerService.ts src/vault_frontend/src/lib/components/explorer/lenses/RevenueLens.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): treasury holdings card replaces pending-interest tile

The old card showed Pending interest = 0 / Flush threshold = 0 — a
permanently uninformative metric since the flush threshold is 0.
Replace with actual treasury holdings: query icrc1_balance_of on each
tracked ledger using the rumi_treasury principal as the account owner.
Free, fast, current-by-construction; no analytics changes needed.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: Admin lens enhancements

### Task 6.1: G1 — flesh out the Admin health strip

Add Last admin action timestamp + Admin actions 24h count alongside existing Mode + Collector errors.

**Files:**
- Modify: `src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte`

- [ ] **Step 1: Fetch the latest admin event + 24h count**

The activity feed already supports filtering by admin type. Add to `onMount`:

```typescript
import { fetchEventsFiltered } from '$services/explorer/analyticsService';

let lastAdmin: any = $state(null);
let adminCount24h = $state(0);

onMount(async () => {
  const [chR, stR, latestAdminR, count24hR] = await Promise.allSettled([
    fetchCollectorHealth(),
    fetchProtocolStatus(),
    fetchEventsFiltered({ types: ['admin'], limit: 1 }),
    fetchEventsFiltered({ types: ['admin'], sinceNs: BigInt(Date.now() - 86_400_000) * 1_000_000n }),
  ]);
  // ...existing assignments
  if (latestAdminR.status === 'fulfilled') lastAdmin = latestAdminR.value?.[0] ?? null;
  if (count24hR.status === 'fulfilled') adminCount24h = count24hR.value?.length ?? 0;
});
```

(Verify `fetchEventsFiltered` accepts the `sinceNs` / `limit` params — it likely already does given the round-1 work; if the param name differs, match it.)

- [ ] **Step 2: Add metrics**

Extend `healthMetrics`:

```typescript
const lastAdminRel = $derived.by(() => {
  if (!lastAdmin?.timestamp_ns) return '--';
  const ms = Number(lastAdmin.timestamp_ns) / 1_000_000;
  const ago = Date.now() - ms;
  if (ago < 60_000) return 'just now';
  if (ago < 3_600_000) return `${Math.floor(ago / 60_000)}m ago`;
  if (ago < 86_400_000) return `${Math.floor(ago / 3_600_000)}h ago`;
  return `${Math.floor(ago / 86_400_000)}d ago`;
});

const healthMetrics = $derived.by(() => {
  const metrics: any[] = [
    { label: 'Mode', value: mode },
  ];
  if (collectorHealth) {
    const errs = Object.values(collectorHealth?.errors ?? {}).reduce((s: number, v: any) => s + Number(v ?? 0), 0);
    metrics.push({
      label: 'Collector errors',
      value: errs.toLocaleString(),
      sub: 'analytics tailing',
      tone: errs > 0 ? 'caution' as const : 'good' as const,
      tooltip: 'Failed inter-canister calls from the analytics tailers (e.g. unexpected response shape, decode failure). Per-source breakdown below.',
    });
  }
  metrics.push({ label: 'Last admin action', value: lastAdminRel });
  metrics.push({ label: 'Admin actions 24h', value: adminCount24h.toLocaleString() });
  return metrics;
});
```

- [ ] **Step 3: Render `tooltip` in `LensHealthStrip` if not already supported**

```bash
grep -n "tooltip\|title\|export let metrics" src/vault_frontend/src/lib/components/explorer/LensHealthStrip.svelte | head
```

If tooltips aren't yet rendered, add a small `title={m.tooltip}` to the metric label render. (Keep G2 minimal — just hover text on the label.)

- [ ] **Step 4: Smoke test + commit**

`/explorer?lens=admin`: strip shows four metrics now, with tooltip on Collector errors.

```bash
git add src/vault_frontend/src/lib/components/explorer/lenses/AdminLens.svelte src/vault_frontend/src/lib/components/explorer/LensHealthStrip.svelte
git commit -m "$(cat <<'EOF'
feat(explorer): flesh out Admin health strip

Add Last admin action (relative time) and Admin actions 24h alongside
the existing Mode and Collector errors. Mode stays first since it's
the most important at-a-glance health indicator (Normal vs Recovery
vs Frozen). Tooltip on Collector errors explains what the count means
(failed inter-canister calls from analytics tailers).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7: Analytics deploy + verify

### Task 7.1: Build, shrink, gzip

**Files:** none — deploy step.

- [ ] **Step 1: Build**

```bash
cargo build -p rumi_analytics --target wasm32-unknown-unknown --release
```

- [ ] **Step 2: Shrink**

```bash
ic-wasm target/wasm32-unknown-unknown/release/rumi_analytics.wasm -o target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm shrink
```

- [ ] **Step 3: Gzip**

```bash
gzip -fk target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm
ls -la target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm.gz
```

Expected: file exists, under 2MiB.

### Task 7.2: Deploy to mainnet (requires Rob's confirmation)

**Files:** none — deploy step.

- [ ] **Step 1: Get the InitArgs candid literal from a previous deploy**

Look at the previous deploy command (likely captured in shell history, the round-1 commit message, or a deploy script):

```bash
git log --all --oneline --grep="analytics" --grep="deploy" | head
grep -rn "InitArgs\|admin = principal" .claude/ 2>/dev/null | head
```

If you can't find a captured literal, ask Rob — don't guess.

- [ ] **Step 2: Confirm with Rob before deploying**

Show the planned command + InitArgs to Rob in the chat. Wait for explicit confirmation.

- [ ] **Step 3: Deploy**

```bash
dfx canister install rumi_analytics \
  --network ic \
  --mode upgrade \
  --wasm target/wasm32-unknown-unknown/release/rumi_analytics.shrunk.wasm.gz \
  --argument '<InitArgs literal from Step 1>'
```

- [ ] **Step 4: Wait + verify the new endpoints respond**

```bash
sleep 15
dfx canister --network ic call rumi_analytics get_fee_breakdown_window '(record { window_ns = opt 7_776_000_000_000_000 })'  # 90 days
dfx canister --network ic call rumi_analytics get_sp_depositor_principals
dfx canister --network ic call rumi_analytics get_collector_health
```

Expected: all three return without error. The fee breakdown should show real borrow + redemption + swap fees. The principal list should include at least 4 entries (matching the SP `total_depositors`).

### Task 7.3: Reset stability_pool error counter via the new endpoint

**Files:** none — operational step.

- [ ] **Step 1: Identify the admin principal**

```bash
dfx identity get-principal --identity rumi_identity
```

(Or whichever identity owns the analytics canister — should match the principal stored in `state.admin`.)

- [ ] **Step 2: Reset the stability_pool counter**

```bash
dfx canister --network ic call rumi_analytics reset_error_counters '(record { sources = opt vec { "stability_pool" } })' --identity rumi_identity
```

Expected: `(variant { Ok })`.

- [ ] **Step 3: Verify**

```bash
dfx canister --network ic call rumi_analytics get_collector_health
```

`error_counters.stability_pool` should now read 0.

- [ ] **Step 4: Smoke test the Admin lens**

`/explorer?lens=admin`. Collector errors should now show 0 (or just the actual ongoing count if any have occurred since).

### Task 7.4: Smoke test all lenses

**Files:** none — verification step.

- [ ] **Step 1: Walk every lens**

`/explorer` → Overview, Collateral, Stability, Redemptions, Revenue, DEXs, Admin. For each, check:

- No console errors
- All cards render data (not loading spinners stuck)
- Cross-reference the round-2 brief items: A1 chip label, B1 histogram, B2 warning zone, C1 snapshot, C4 coverage, D1 RMR tile, D2 redemption panel, E1 fees consistent, E2/F2 LP APY non-null, E3 SP APY ≈ 6.24%, E4 fee breakdown non-zero, E5 treasury card, F1 tooltip, F3 single-event chart, F4 virtual price slope, F5 3USD pair, F6 row removed, G1 strip metrics, G3 zero errors

- [ ] **Step 2: Take updated screenshots if anything looks off**

Replace the explorer-lens-* PNGs at the repo root with updated captures so the next reviewer has fresh references.

---

## Phase 8: Push branch + open PR

### Task 8.1: Final review

**Files:** none.

- [ ] **Step 1: Walk the diff**

```bash
git log --oneline main..HEAD
git diff main..HEAD --stat
```

- [ ] **Step 2: Run the full test suite**

```bash
cargo test -p rumi_analytics
cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | grep -c ERROR
```

Expected: tests pass; svelte-check error count matches the round-1 baseline (33 pre-existing errors in unrelated files).

### Task 8.2: Push + open PR

**Files:** none.

- [ ] **Step 1: Push**

```bash
git push -u origin feat/explorer-review-pass
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "feat(explorer): review pass (rounds 1+2)" --body "$(cat <<'EOF'
## Summary

Two rounds of Explorer review, landing together on `feat/explorer-review-pass`. Round 1 fixed labels, navigation, and the analytics-canister architectural gap (SP tailer + fee_amount capture + daily fee rollup sums). Round 2 fixes the remaining bugs Rob flagged on review and reworks several lenses per his vision.

### Round 2 highlights
- **Stability lens rework:** Top Depositors → Current Depositors snapshot (per-token columns), new Per-collateral opt-in coverage card; SP balances now read authoritatively from the SP canister via a new `get_sp_depositor_principals` analytics query
- **Revenue lens:** new `get_fee_breakdown_window` analytics query reads fees directly from `evt_vaults` + `evt_swaps` so the 24h and 90d cards share methodology and historical pre-deploy fees show up immediately
- **Treasury holdings card** (live `icrc1_balance_of` per ledger) replaces the uninformative pending-interest tile
- **Admin lens:** strip flesh-out (Last admin action, 24h count, tooltip on Collector errors); new `reset_error_counters` admin endpoint clears the 12 leftover stability_pool errors from round-1's first deploy
- **Collateral lens:** CR histogram bars actually render now; Liquidation Risk shows warning-zone vaults (below min CR but above liquidation threshold)
- **Redemptions lens:** single Current RMR tile with curve tooltip; recent activity panel populated
- **DEXs lens:** virtual-price chart uses data-fit y-axis to show actual slope; redundant bottom-row strip removed; "3USD LP/ICP" → "3USD/ICP"; Arb score tooltip
- **Activity feed:** AMM events render as "🌊 3USD/ICP" instead of truncated principals

## Test plan
- [ ] svelte-check error count matches round-1 baseline (33 pre-existing)
- [ ] cargo test -p rumi_analytics green
- [ ] Each lens loads without console errors on mainnet
- [ ] SP depositor count on the lens matches `get_pool_status.total_depositors`
- [ ] Rob's principal appears with non-zero icUSD + 3USD balance in the new SP snapshot
- [ ] 90d fees ≥ 24h fees (no contradiction)
- [ ] Treasury card sums match `icrc1_balance_of` calls run independently

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-review checklist

**Spec coverage** — every item from the round-2 brief mapped to a task:

| Brief item | Task |
|---|---|
| A1 AMM chip | 1.1 |
| B1 CR histogram | 1.8 |
| B2 Liquidation Risk | 1.9 |
| C1 Current Depositors | 4.2 (deps 3.2, 4.1) |
| C2 4-vs-1 mismatch | 3.2 + 4.2 |
| C3 Rob's balance 0 | 4.2 (same root as C2) |
| C4 Per-collateral coverage | 4.3 |
| D1 Current RMR tile | 1.7 |
| D2 Redemption panel | 2.1 |
| E1 Fee contradiction | 5.1 (subsumed by E4 unification) |
| E2 3pool LP APY null | 2.2 |
| E3 SP APY 4.72% | 2.3 |
| E4 Fee breakdown | 3.1 + 5.1 |
| E5 Treasury card | 5.2 |
| F1 Arb score tooltip | 1.5 |
| F2 LP APY null | 2.2 (same as E2) |
| F3 Single-dot fallback | 1.4 |
| F4 Virtual price y-axis | 1.3 |
| F5 3USD pair label | 1.2 |
| F6 Bottom row removal | 1.6 |
| G1 Admin strip | 6.1 |
| G2 Collector errors tooltip | 6.1 (combined) |
| G3 Reset error counters | 3.3 + 7.3 |

**Placeholder scan**

- Investigation tasks (2.1, 2.2, 2.3) include placeholders like `<root cause from Step N>` in commit messages — these are deliberate; the executing subagent fills them in once root cause is identified.
- All implementation steps have actual code or exact commands.
- Deploy step (7.2) explicitly requires Rob's confirmation.

**Type consistency**

- `FeeBreakdown` (frontend), `FeeBreakdownResponse` (canister), `FeeBreakdownQuery` (canister) are defined once and used consistently.
- `CurrentSpDepositor` shape is defined in 4.1 and consumed in 4.2 and 4.3.
- `ResetErrorCountersArgs.sources` is `Option<Vec<String>>` in Rust / `opt vec text` in candid / `string[] | undefined` in TS.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-28-explorer-review-pass-round-2.md`. Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks. Phases 1, 2, 4, 5, 6 are mostly independent; Phase 3 (analytics) feeds into Phases 4-6. Phase 7 deploy is the integration point.

**2. Inline Execution** — Execute tasks in this session using executing-plans. Useful if Rob wants to discuss findings on the data-bug investigations (D2, E2/F2, E3) inline.

Which approach?
