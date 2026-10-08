# Fiat-backed 3pool points: proposed rule and adversarial review

Date: October 7, 2026 (America/Los_Angeles).
Source anchor: `RumiLabsXYZ/rumi-protocol-v2`, commit `4b4b1680a89b37ab82641ae1e1cb6addf98dcdb5`.
Scope: policy and source feasibility review. No implementation, deployment, or live points adjustment is claimed.

## Policy oracle

1. Preserve all already-recorded points. In particular, historical matched ckUSDC/ckUSDT contributions that earned 5× keep their original points.
2. For each historical `CkStable3PoolUnmatched` accrual row before the cutover, calculate an adjustment of `floor(points_delta / 3)` in the existing integer points units and append it only when positive. The top-up applies to every qualifying unmatched portion, including the unmatched portion of a mixed deposit. Do not apply it to other sources, correction rows, or future 4× accrual. Classify eligible legacy accrual by the immutable cutover epoch/index, not by wall-clock write time; use a fixed activation prefix plus exactly-once inline adjustments for qualifying rows written by the remaining legacy epoch.
3. This is a one-time uplift of recorded historical contributions. It is not a replay of the historical two-snapshot selection at 4×. Do not reconstruct eligibility from current holdings or give historical credit for activity before registration.
4. Start flat 4× for future ckUSDC/ckUSDT **3pool deposit** accrual at a named epoch boundary. Both matched and unmatched fiat-backed value get 4×. Historical 5× earnings are preserved; existing deposits do not retain ongoing 5× treatment after cutover.
5. Vault repayment remains 5×. icUSD deposits, borrowing, Stability Pool, 3USD and AMM multipliers and holding-verification rules retain their existing treatment.
6. Publish the effective epoch and historical adjustment formula. With a fixed pro-rata reward allocation, top-ups increase the denominator and reduce the percentage share of participants receiving no top-up. Preserving point totals does not preserve allocation share.

## Why the incentive rule makes sense

The old rule pays 5× on `2 * min(USDC, USDT)` and 3× on `abs(USDC - USDT)`. A depositor can swap half of a single-asset holding beforehand to earn more points while contributing roughly the same fiat-backed capital. A balanced deposit can also be less useful than a single-asset deposit of the asset the pool currently lacks. Flat 4× removes the composition bonus. It is a reasonable policy choice, not a demonstrated economically optimal rate or a budget-neutral change.

Arbitrage is price-driven and does not guarantee equal token reserves or preservation of the pool's total fiat-backed liquidity. This recommendation does not depend on a bot guaranteeing either outcome.

## Accounting examples and limits

For identical holding periods: historical matched points of 5,000 remain 5,000; historical unmatched points of 3,000 receive a 1,000 top-up and become 4,000. Other-source points of 2,000 remain 2,000. The matched participant's share changes from `5,000 / 10,000 = 50%` to `5,000 / 11,000 = 45.4545...%`.

Time scaling already floors each source independently. For non-saturated arithmetic, `old + floor(old / 3)` can be one smallest points unit below direct `floor(4 * underlying_dollar_days)` for the same retained historical contribution. The rounding rule is deliberately per original row. Aggregating first and rounding once would define a different policy. A coordinator check of 500 representative value/time combinations verified the zero-or-one-unit difference; this is arithmetic evidence, not migration test coverage.

Historical snapshot choice matters more than rounding. Snapshot A with $100 total matched value (the aggregate `2 * min(USDC, USDT)`, not $100 of each token) yields 500 old weighted units; B with $140 unmatched yields 420. The old rule retains B. The top-up yields 560 for that already-selected contribution. A hypothetical 4× replay would choose A at 400. That replay changes historical selection and cannot be reconstructed from the retained ledger because the engine consumes the selected snapshot buffer at close. The uplift interpretation must not be advertised as exact historical flat-4× replay.

## Implementation acceptance criteria

- Keep original audit rows and historical epoch summaries immutable. Append separately identifiable policy adjustments and update principal totals atomically. Present old summaries as original accrual summaries, and corrections separately.
- Fix the exclusive historical ledger cutoff at activation; correct later rows from the one remaining legacy epoch within its existing bounded close chunks. Use a durable once-only migration identifier, cursor, completion marker and bounded batches, or an equivalent durable exactly-once design. Trap/error on missing principal or overflow; never silently skip or saturate a correction. Treat evidence of saturated historical arithmetic as an explicit reconciliation exception, not as proof of correct original 3× accounting. Retries and upgrades must not duplicate top-ups.
- Ensure a single epoch uses one multiplier policy for both snapshots and close. A persisted epoch policy or activation restricted to a genuine epoch boundary can enforce this. Fence the epoch driver while any migration requiring a between-epochs transition is active. Pausing ingestion is not inherently required by an immutable ledger-prefix design; do not add it as a gate without a concrete race.
- Keep historical matched 5×, historical unmatched 3× plus adjustment, and future flat-4× accrual distinguishable in audit/UI/reporting. If new output variants are emitted, regenerate Candid and declarations and account for old client decoding. An additive enum variant is not sufficient proof of old-client compatibility.
- Reconcile ledger sums to principal totals and report per-user before/after points and shares using the same immutable cutoff and population. Live extraction is needed for actual per-user numbers; source alone supplies no production balances.
- Validate rounding, matched/unmatched mixed deposits, other-source preservation, whole-snapshot selection, exactly-once batch resume, populated-state upgrade, authorization, overflow, cutoff exclusion, epoch transition and client/report compatibility before merging implementation.
- Correct the existing report script's matched-points label of 10×; source math is 5×. Future customer-facing labels must agree with the active backend rule at release time.

## Review board

| Card | Owner | Model | Scope | State |
| --- | --- | --- | --- | --- |
| PS4-POLICY-1 | points_policy_review | GPT-5.6 Luna | Independent incentive/fairness/source review | Complete: supports direction, incomplete migration contract |
| PS4-ACCOUNTING-1 | points_accounting_review | GPT-5.6 Luna | Independent historical math and state feasibility | Complete: feasible uplift; exact replay unavailable |
| PS4-POLICY-2 | policy_final_check | GPT-5.6 Luna | Refined policy oracle review | PASS: no concrete policy blocker |
| PS4-ACCOUNTING-2 | accounting_final_check | GPT-5.6 Luna | Refined accounting/transition acceptance criteria | PASS: no concrete accounting blocker |

Sonnet availability check returned `loggedIn: false`, so native Luna was the explicit fallback. Coordinator owns integration and final disposition. No live data, secrets, source implementation or deployment actions form part of this report.

Final disposition: two review rounds, with two fresh independent reviewers in round two. Both final reviewers passed the written policy and accounting logic. Advisory clarifications about aggregate matched value, zero adjustments, immutable epoch classification, saturation exceptions and equivalent migration designs are incorporated above. The coordinator independently checked the examples; the accounting handoff's stray subtotal of 8,000 is not the adjusted overall total, which is 11,000. No implementation, migration tests, live report or release readiness is approved by these policy verdicts.

Proposed durable catalog lesson, not an applied catalog/memory edit: the points ledger retains rounded contributions from the historical whole-snapshot winner. A multiplier correction should define an explicit recorded-contribution uplift oracle rather than claim exact historical replay from discarded inputs.

## Source evidence

- `src/rumi_points/src/accrual.rs:72-95`: whole-snapshot minimum and independent integer time scaling.
- `src/rumi_points/src/accrual.rs:150-166`: current matched 5× and unmatched 3× formula.
- `src/rumi_points/src/types.rs:68-81,158-168`: source categories and historical audit entry shape.
- `src/rumi_points/src/state.rs:619-649`: point totals and pro-rata leaderboard shares.
- `src/rumi_points/src/state.rs:1467-1492`: snapshot consumption, append-only audit writes and principal totals.
- `src/vault_frontend/src/lib/utils/pointsRules.ts` and `pointsBreakdown.ts`: rule metadata and historical source grouping.
- `scripts/airdrop-report.py:35-46`: current report labels.

All source pins above refer to commit `4b4b1680`, rather than assuming the dirty primary checkout matches current main.

## Authorized implementation and release

Rob authorized implementation, merge and deployment on October 7. The implementation schedules the next epoch while preserving the current epoch's multiplier and snapshots. A separate versioned stable cell stores the immutable cutover, original-ledger cutoff and historical cursor. Qualifying rows produced later by the remaining legacy epoch receive their adjustments inside the existing principal close chunks; they never require a final full-ledger scan. Original epoch summaries retain original accrual; adjustments have a separate source and appear in season totals.

Live preflight found the driver already disabled in legacy epoch 18. The current secure-seed upgrade holds that legacy epoch and fences polling under the existing PTS-002 security rule. This release preserves that hold and schedules epoch 19; it does not recover epoch 18 or claim future 4x is already accruing. Historical fixed-prefix adjustments remain independently runnable. See the preflight runbook for exact target and verification steps.
