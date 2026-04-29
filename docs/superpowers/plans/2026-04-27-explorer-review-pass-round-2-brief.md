# Explorer Review Pass — Round 2 Brief

> Hand this prompt to a fresh session. It picks up from `feat/explorer-review-pass` after the round-1 work shipped. The new session should investigate root causes (use the **systematic-debugging** skill for the data bugs), make design decisions explicitly with Rob (the items marked **DECISION**), and execute as a planned set of tasks.

---

## State of play

- Branch: **`feat/explorer-review-pass`** (16 commits ahead of `main`, **not yet pushed**)
- Two mainnet deploys done by the round-1 session:
  - **rumi_analytics** (`dtlu2-uqaaa-aaaap-qugcq-cai`) — SP tailer live, ingesting from `evt_stability` cursor 126; fee_amount captured in tailer + summed in daily rollup; all 16 SP `PoolEventType` variants matched.
  - **vault_frontend** (`tcfua-yaaaa-aaaap-qrd7q-cai`) — all round-1 frontend changes live.
- The original plan: `docs/superpowers/plans/2026-04-27-explorer-review-pass.md`. All Phase 0/1/2 tasks completed.

**Important wrinkle:** The analytics canister's daily fee rollup writes one row per day. Round-1 added `Some(borrow_fees) / Some(redemption_fees)` to the rollup, but the historical rows pre-deploy still have `borrowing_fees_e8s = null` / `redemption_fees_e8s = null`. The next daily tick (after the deploy) will produce the first rollup with real fee sums. Until then, the Revenue lens shows the historical-only data, which is what Rob is reporting as "nothing changed." Confirm the timer state before assuming a bug — `dfx canister --network ic call rumi_analytics get_collector_health` and look at `last_pull_cycle_ns`. The daily fee rollup runs separately from `pull_cycle` though — track down its trigger schedule.

---

## Round-1 leftovers + new observations

Grouped by lens. Most items are bugs (root-cause-first). A few are design decisions Rob explicitly wants to weigh in on.

### A. Activity feed (cross-lens)

- **A1.** AMM event badge currently shows truncated token identifiers (e.g. `+pool:fohh4-...cai_ryjl3-...cai`). **Rename to `🌊 3USD/ICP`** — wave emoji to match the 3pool style, token-pair label, no AMM1 prefix in the chip itself. The "AMM1" label can stay as the source-prefix on the row's leading cell. Source label is in `src/vault_frontend/src/lib/utils/displayEvent.ts` (`DEX_SOURCE_LABEL` + the AMM-source override added in round 1) and the chip render is in `MixedEventRow.svelte` / `EventRow.svelte`.

### B. Collateral lens

- **B1. CR distribution histogram bars are tiny slivers.** Round-1 fixed the empty-data bug (vault.collateral_ratio doesn't exist; helper computes CR client-side). But the rendering is still broken — bars are unreadably small. Investigate: probably the bar height calc in `CollateralLens.svelte` (the `pct = (b.count / maxBucket) * 100` line and the `style="height: {pct}%"` div). Could also be a CSS height issue — the parent `<div class="flex items-end gap-2 h-40">` may be losing its height. Use systematic-debugging.

- **B2. Liquidation Risk card is empty.** Rob clarified what he wants here: vaults that are **below minimum CR but above the liquidation threshold** — the warning zone where no more borrowing is possible (this is what triggers the warning icon on each vault). Currently the lens calls `fetchLiquidatableVaults()` which returns vaults at-or-below the liquidation threshold, so it's almost always empty.

  Implementation: take all vaults, compute CR client-side via the round-1 helper, filter to those where `cr < min_cr_for_borrowing && cr >= liquidation_ratio`. The thresholds come from `fetchCollateralConfigs` (each has `borrow_threshold_ratio` and `liquidation_ratio`). Display in the existing `LiquidationRiskTable` component.

### C. Stability Pool lens

This section needs a **rework**, not just bug fixes. Rob's vision:

- **C1. Top depositors → "Current depositors" snapshot.** Drop the time-window logic. Show every principal currently holding any SP balance, ordered by total USD value descending. **Don't normalize 3USD into icUSD** — show per-token columns: icUSD, 3USD, ckUSDT, ckUSDC, etc. Each column shows that user's balance in that token (or empty if zero).

- **C2. The pool says 4 depositors but only 1 shows in the leaderboard.** This is a real data bug — the analytics endpoint is missing depositors. Trace through:
  1. `dfx canister --network ic call rumi_analytics get_top_sp_depositors '(record { window_ns = null; limit = opt 50 })'` — does it return all 4?
  2. If not, look at `compute_top_sp_depositors` in `src/rumi_analytics/src/queries/live.rs:749` — the `window_stats.into_iter().filter(|(_, (dep, _))| *dep > 0)` line drops anyone whose deposit total in the window is 0, which means depositors who haven't deposited recently are filtered out even if they still have a balance.
  3. With C1's redesign (snapshot, not flow), this filter logic gets thrown out anyway — read directly from `evt_stability` and reduce per-principal: `+amount on Deposit, -amount on Withdraw`. Net positive = active depositor.

  Also note: the SP canister itself probably exposes a balances query (`get_pool_status` returns `total_depositors` and there's likely a per-user query too — check `src/declarations/rumi_stability_pool/rumi_stability_pool.did`). It might be cleaner to query the SP canister directly for current balances rather than reconstructing from events.

- **C3. Rob's principal shows balance=0 despite having ~$500 in 3USD + a couple icUSD.** Same root cause as C2 likely. Reproducer: his principal is in `~/.config/dfx/identity/rumi_identity/` — `dfx identity get-principal` returns it. Cross-check: `dfx canister --network ic call rumi_stability_pool get_user_position '(principal "<his>")'` (or whatever the SP canister's per-user query is named — find it in the .did) should show real balances.

- **C4. Replace "Collateral in pool" card with "Per-collateral opt-in coverage."**

  Rob's spec: for each supported collateral type, show:
  - Collateral symbol (ICP, BOB, EXE, ckBTC, ckETH, ckXAUT, nICP)
  - **% of depositors who haven't opted out** of that collateral (i.e., would absorb a liquidation in it)
  - **Total $ available** for liquidating that collateral (sum of stable balances across opted-in depositors)

  Example: `ICP 100% $1,000` (all 4 depositors are in for ICP, $1k total backing) / `BOB 50% $500` (half opted out).

  Data source: SP events include `OptOutCollateral { collateral_type }` and `OptInCollateral { collateral_type }` (round-1 already added these to the analytics shadow type but doesn't aggregate them — see `src/rumi_analytics/src/sources/stability_pool.rs`). The "total $ available" cross-references each opted-in depositor's stable balance.

  This is a reasonably big new analytics endpoint. May need a fresh query in `src/rumi_analytics/src/queries/live.rs` (e.g., `get_sp_coverage_per_collateral`) plus frontend consumption.

### D. Redemptions lens

- **D1. Replace RMR range tile with a single "current RMR" tile.** Round-1 showed "RMR floor → ceiling" (96% → 100%) which Rob says is meaningless to readers. Show the **active RMR** value (which depends on current global CR vs `rmr_floor_cr` / `rmr_ceiling_cr`). Add an info-bubble tooltip explaining: what RMR is, how it moves, and ideally a small inline curve preview or a link to the docs curve.

  Find the active-RMR computation in the backend: `src/rumi_protocol_backend/src/state.rs` — search for `rmr_floor_cr`, `rmr_ceiling_cr`, the linear interpolation logic. There may already be a `get_current_rmr()` query exposed in the candid. If not, the frontend can compute it from the protocol status (`total_collateral_ratio`) plus the four RMR config fields.

- **D2. "Recent redemption activity" panel at the bottom is empty even though "View All" shows redemptions.** This is a `LensActivityPanel` bug — the lens-level filter is too strict OR the lens is sourcing from a different feed than View All.

  Look at `LensActivityPanel.svelte` with `scope="redemptions"`. The `isBackendRedemption` filter (rewritten in round 1 to use snake_case) checks for `redemption_on_vaults`. But maybe the limit is too small (default 12) and there are no redemptions in the most recent N events. Try widening the fetch window — for redemptions specifically, "recent" should mean "the latest redemption event regardless of age," not "in the last N events overall." Either bump the fetch size for this scope or change the source to `fetchEventsByPrincipal` / a redemption-specific endpoint.

### E. Revenue lens

**Rob notes none of his original observations were addressed visually. Most of these are real bugs — go through each.**

- **E1. "Fees (90d) = $1" but "24h fees = $5" — contradiction.** The 90d total can't be less than the 24h total. Bug in either:
  - The 90d sum (probably summing only `swap_fees_e8s` from rollups, ignoring borrow + redemption — but those are also $0/null in the historical rollups, so it should at least equal 24h)
  - The 24h estimate (`fees24h` derived in `RevenueLens.svelte` includes `estimatedDailyBorrow` from live protocol state — that's a forward-looking estimate, not actual realized fees)

  The contradiction is real. Track down the math in `RevenueLens.svelte`:
  ```typescript
  const totalBorrow = $derived(feeRows.reduce(...));
  const totalRedemption = $derived(...);
  const totalSwap = $derived(...);
  const totalFees = $derived(totalBorrow + totalRedemption + totalSwap);
  const estimatedDailyBorrow = $derived(/* from protocol status */);
  const fees24h = $derived(swapFees24h + estimatedDailyBorrow);
  ```
  `totalFees` (90d) is realized fees from rollups; `fees24h` is partly estimated. They use different units / methodologies. Either make them consistent or label them clearly so the contradiction goes away. Possibly: replace `fees24h` with the most recent rollup row's actual fees instead of an estimate.

- **E2. 3Pool LP APY = null.** The analytics `compute_lp_apy` returns null when the TVL snapshot rows have `None` reserves. Find why TVL snapshots are empty — look at `src/rumi_analytics/src/collectors/tvl.rs` and `compute_lp_apy` in `queries/live.rs`. Possibly the 3pool snapshot collector isn't running, or the reserve fields aren't being populated correctly.

- **E3. SP APY says 4.72% — Rob believes wrong.** Round-1 added a live SP APY computation matching the /liquidity tab. If the lens still shows 4.72%, the live formula returned null and we fell back to the 7d analytics number. Check `liveSpApy` in `StabilityPoolLens.svelte` — it uses `protocolStatus.interestSplit` and `protocolStatus.perCollateralInterest`. If either is empty/missing, falls back. Investigate why those would be empty post-deploy.

- **E4. Fee breakdown: $0 borrow, $0 redemption, $1 swap — root cause + plan.** Background: round-1's analytics changes (capture fee_amount + sum it) only affect rollups written AFTER the deploy. Historical daily rollups have `borrowing_fees_e8s = null`, `redemption_fees_e8s = null`. Three options:
  1. **Wait** for the daily rollup to fire and write a real-fee row. The Revenue card will show `$X` for "today" but the 90d total will still mostly be null.
  2. **Backfill historical rollups** by re-scanning `evt_vaults` for the past 90 days. Add a one-shot backfill endpoint in rumi_analytics.
  3. **Frontend computes 90d totals on the fly** by reading `evt_vaults` directly (analytics already exposes vault events) and summing `fee_amount` per day. Bypass the rollup entirely for this metric.

  **DECISION:** which approach. I'd lean toward option 2 (backfill) because rollups are the canonical source. Option 3 makes the frontend compute a lot of data on every page load.

- **E5. Treasury card rework.** Currently shows "Pending interest = 0 / Flush threshold = 0." Rob's right — flush threshold of 0 means pending always immediately flushes, so this is permanently uninformative.

  Replace with a **Treasury holdings** card: actual token balances held by the rumi_treasury canister (`tlg74-oiaaa-aaaap-qrd6a-cai`). Query `icrc1_balance_of` on each ledger (icUSD, ckUSDT, ckUSDC, ICP, etc.) using the treasury canister as the account owner. Show one row per token with symbol + balance + USD-equivalent.

  This is "look up balances on the ledgers using the treasury principal as account" — straightforward but new code. Probably belongs in `src/vault_frontend/src/lib/services/explorer/explorerService.ts` as `fetchTreasuryHoldings()`.

### F. DEXs lens

- **F1. Arb score** needs a tooltip explaining what it is. (Look at `src/rumi_analytics/src/queries/live.rs` for the formula. Probably some price-deviation × pool-imbalance metric. Translate the formula into plain English for the tooltip.)

- **F2. 3pool LP APY null** — same root cause as E2. Fix once, both lenses show it.

- **F3. 3pool swap volume chart shows just a single dot at "$70."** Round-1 added dots for non-zero points to make sparse data visible. With only one non-zero point in the entire 7-day hourly series, the result is a single dot — technically correct but looks broken. Better: when only 1 non-zero point exists, fall back to a centered "indicator" or extend the dot rendering to be more obvious (e.g., a vertical line marker + the value). Or change the source data: pull a longer window so there's more chance of multiple points.

- **F4. 3pool virtual price chart y-axis scale.** Currently looks like a flat line because the y-range covers the full positive number space. The virtual price moves slowly (e.g. 1.063 → 1.064) and a chart from 0 means the slope is invisible. **Tighten the y-axis** to a small padding around the actual data range (e.g. `[min × 0.999, max × 1.001]`). The `MiniAreaChart` round-1 changed for the volume case anchored y at 0 for "all non-negative" — that's the wrong behavior here. Add a prop like `yAxisMode: 'zero-anchored' | 'data-fit'` or detect the case (small variance relative to absolute value) and use data-fit automatically.

- **F5. AMM Pools card pair label.** Says `3USD LP/ICP` — should be just `3USD/ICP`. The "LP" suffix comes from `getTokenSymbol()` for the THREEPOOL principal — check `KNOWN_TOKENS` in `src/vault_frontend/src/lib/utils/explorerHelpers.ts`. If the registered symbol is "3USD LP" change it to "3USD" (since 3USD IS the LP token, the LP suffix is redundant). Or, in the AMM Pools card specifically, strip a trailing " LP" from the rendered pair.

- **F6. Remove redundant bottom-row cards.** Below the AMM Pools card there are three small cards (3pool balance / 3pool LP APY / Stability pool APY) duplicating the top-strip metrics. The third one isn't even relevant to the DEXs lens. Strip the entire row out — rendered by `<PoolHealthStrip />` in `DexsLens.svelte`. Just delete that line.

### G. Admin lens

- **G1. Top "health strip" card is sparse.** Currently shows `Mode` and `Collector errors`. Either:
  1. Justify it with more useful stats (canister cycles for ALL canisters, latest setter timestamp, etc.) and keep it
  2. Merge it into the Admin Breakdown card
  3. Drop it entirely

  **DECISION:** which. The 12-errors fix below interacts with this — if we keep the card, the collector errors number is more meaningful once it's accurate.

- **G2. Explain "Collector errors."** Add a tooltip / sub-text on the metric. It's the count of failed inter-canister calls from the analytics canister's tailers (e.g., when the SP canister returns an unexpected response shape and decode fails). Sub-source breakdown is already in the "Analytics tailing health" card below — link the two visually if they stay separate.

- **G3. The 12 leftover `error_counters.stability_pool` errors.** Background: round-1's first deploy of the SP tailer failed at runtime (the SP candid is stale and missing 11 PoolEventType variants — this was the deploy bug) and incremented the counter 12 times before the fix landed. Now the counter sits at 12 forever — accurate that they happened, but misleading because they're from a fixed bug.

  **DECISION:** four options:
  1. **Add a `reset_error_counters` admin endpoint** (gated by the admin principal) on rumi_analytics. One-shot use for cases like this. Slight scope creep.
  2. **Reset in `post_upgrade`** — make the next analytics upgrade zero out specific source counters. Avoids new endpoint but requires a migration commit.
  3. **Frontend "since cursor X" display** — store a baseline counter at deploy time, only show errors-since-baseline. Doesn't actually fix the data.
  4. **Document and accept** — the counter is technically accurate. Add a small "(historical)" annotation explaining post-deploy errors are leftover from a known fix.

  My lean: option 1 (admin endpoint) — useful primitive to have for operational reasons beyond this incident.

---

## Suggested execution order

1. **Triage + brainstorm with Rob.** Before coding, walk through the DECISION items and the design changes (C1/C4/E5/G1/G3) to lock in the approach.
2. **Use writing-plans to convert the agreed decisions into a Round-2 plan.**
3. **Use subagent-driven-development for execution** — each lens is roughly independent.
4. **Investigate the data bugs with systematic-debugging** before fixing them. Don't guess. Especially:
   - C2 (depositor count mismatch)
   - C3 (Rob's balance = 0)
   - E1 (fee total contradiction)
   - E2 / F2 (LP APY null root cause — probably a TVL collector issue)
   - E3 (live SP APY null fallback)

5. **Push the branch + open a PR** when round 2 is complete (round 1 hasn't been pushed yet — the round-2 work goes on the same branch).

---

## Quick reference

| Item | File |
|---|---|
| Round-1 plan | `docs/superpowers/plans/2026-04-27-explorer-review-pass.md` |
| AMM source label override | `src/vault_frontend/src/lib/utils/displayEvent.ts` |
| Pool registry helper | `src/vault_frontend/src/lib/utils/ammNaming.ts` |
| Vault CR helper | `src/vault_frontend/src/lib/utils/vaultCr.ts` |
| Lens activity filter (snake_case rewrite) | `src/vault_frontend/src/lib/components/explorer/LensActivityPanel.svelte` |
| Mini chart with sparse-data dots | `src/vault_frontend/src/lib/components/explorer/MiniAreaChart.svelte` |
| Analytics SP shadow types (16 variants) | `src/rumi_analytics/src/sources/stability_pool.rs` |
| Analytics SP tailer (id-based cursor) | `src/rumi_analytics/src/tailing/stability_pool.rs` |
| Analytics fee rollup (sums after round 1) | `src/rumi_analytics/src/collectors/rollups.rs` |
| SP candid (stale; flagged separately) | `src/declarations/rumi_stability_pool/rumi_stability_pool.did` |
| SP Rust types (16 variants) | `src/stability_pool/src/types.rs` |
| Treasury principal | `tlg74-oiaaa-aaaap-qrd6a-cai` |
| rumi_analytics principal | `dtlu2-uqaaa-aaaap-qugcq-cai` |

Branch: `feat/explorer-review-pass` (16 commits). Do not rebase or squash — Rob prefers regular merge commits per project memory.
