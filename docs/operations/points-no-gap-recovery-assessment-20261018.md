# Points outage: no-gap recovery assessment

Status: investigation checkpoint, not a completed recovery or approved credit allocation.

## Updated user requirement

Rob explicitly requires restoring processing and covering the missed period as
if the program had continued. Preserving existing totals and restarting the
driver alone does not meet that requirement. Existing earned rows must remain
unchanged; catch-up must not double-count them.

## Why the existing release is insufficient on its own

The held epoch 18 repair in source `0eefe9fa` preserves its original reward
period and saved A, with one newly scheduled B. Its next epoch opens at actual
restart time. See `src/rumi_points/src/epoch.rs`:
`safe_due_epoch_start`, `resume_legacy_reseed`, and the driver's `Start`
branch. See `src/rumi_points/src/state.rs::run_close_accrual_chunk` and
`src/rumi_points/src/accrual.rs::accrue_principal_for_policy`: accrual uses
`epoch_end_capped - epoch_start`.

Epoch 19's scheduled interval is October 12–19 UTC. Rebasing its opening to
restart omits the preceding part of that interval from ordinary epoch accrual.
The existing release therefore cannot by itself substantiate "no gap". This
finding follows directly from the updated user requirement, not an optional
release gate.

## Retained activity verified with query-only calls

At October 18, 06:36–06:41 UTC, the saved points cursors were unchanged:

| Source | Saved next cursor | Observed count | Assessment |
| --- | ---: | ---: | --- |
| Backend | 512,671 | 523,347 | Forward scan covered the frozen count; scanned through next cursor 523,348 as the source advanced |
| 3pool | 110 | 110 | At observed tail |
| Stability Pool | 1,870 | 1,896 | Oldest retained ID 0; preserved all pending IDs 1,870–1,895 |
| AMM | 5 | 5 | Oldest retained ID 0; at observed tail |

The backend scan preserved 72 unique filtered events: 5 borrows, 33 repayments,
33 alternate withdraw-and-close events, and 1 liquidation. Its 11 forward
responses had monotonic continuation bounds and no missing scan interval.
A preliminary count of 39 omitted the alternate close path and is superseded
by this complete inventory. The wire mirror already recognizes that path.

SP's 26 preserved rows were 1 emergency pause, 9 interest receipts, 1
liquidation notification, 7 collateral opt-ins and 8 opt-outs. Their IDs are
strictly contiguous. All query calls succeeded. These observations establish
retention at the saved cursors and preserve the bounded backlog; they do not
establish replay, applied credits, full historical balances or ongoing tails.

Local evidence is protected under
`.recovery-artifacts/rumi-points-recovery-20261017/no-gap-assessment-20261018`:
`coverage-assessment.json`, `query-receipt.json`,
`retained-backlog-query-receipt.json` and the hashed raw query files.
Raw financial/user records are not included in this document.

## Exact replay and compensation are different proof obligations

The scoring rule selects the smaller **whole** A/B weighted snapshot and scales
it by the epoch period; repayment windows are added separately. See
`accrual.rs::min_by_total` and `accrue_principal_for_policy`.
An event replay alone does not recover every historical snapshot input: wallet
token balances, pool verification and valuations also contribute to capture.
Epoch 19 never opened; its secure entropy and snapshots were not captured.
See `snapshot_seed.rs` and `epoch.rs::close_current_epoch`.

Consequently this evidence does not determine a unique counterfactual award
for the missed interval. Do not silently backdate a newly observed balance,
invent historical prices or random snapshots, or describe a compensating
allocation as an exact replay of unobserved snapshots.

## Required remaining work

1. Resolve the original execution request disposition. Preserve root sole
   ownership; no duplicate install or alternative execution path.
2. Inventory the missed interval per principal and source, including actual
   balance/valuation evidence and missing inputs. Refresh source counts and
   retention before execution.
3. Produce a concrete, reviewable make-whole allocation for any amount not
   exactly reconstructable. It must cover the omitted time, preserve existing
   earned credits and the established multiplier cutover, identify assumptions
   and exceptions, and prevent duplicate application. No allocation has been
   calculated or applied yet.
4. Any implementation must use additive durable reconciliation records, bounded
   batches, an immutable input/allocation manifest and explicit idempotency
   keys. Preserve original 417 rows and audit every added credit.
5. Review source, stable-state/Candid compatibility, accounting and actual
   retained-state behavior before production changes. The existing PR583
   reviews cover its previous resume scope, not a new compensation algorithm.
6. After one reviewed upgrade: verify preserved state, bounded source replay,
   reconciled credits and source tails; resume once; observe B/close and future
   secure epoch advancement. Do not claim success from driver enable alone.

The old candidate's current timing guard remains intact. This assessment grants
no permission to bypass it or the execution hold. Broader recovery design may
need to handle an expired window explicitly; it must not silently weaken the
guard. No code, canister state, driver, hold, source cursor or credit was changed
by this investigation, and no tests were run.
