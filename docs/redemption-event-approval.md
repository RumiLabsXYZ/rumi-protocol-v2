# Redemption event payout correction — approval review

**Status: proposed only.** The companion [`redemption-event.patch`](redemption-event.patch) is a full unified diff against the current `event.rs`. It has not been applied, compiled, or tested. Apply it only after explicit approval of this implementation. No source edits were made as part of preparing these artifacts.

## Why this change is needed

The water-fill deductions are in the collateral token's raw native units. The current recorder instead derives the queued transfer as `consumed / collateral_price`, which produces an e8-scaled amount regardless of whether the token uses 6, 8, or 18 decimals. It also pays from consumed debt rather than from actual collateral after integer rounding and underwater saturation. That can create an unbacked or incorrectly scaled payout.

The proposed recorder uses the sum of `VaultRedemption.collateral_seized` as the payout, using saturating addition. New events store that exact raw amount in `payout_collateral_raw`, including an empty outcome vector, so replay cannot rerun a new redemption against changed eligibility. `min_net_collateral_raw` is stored alongside it and copied into the pending transfer.

For upgrade compatibility, events missing `payout_collateral_raw` retain the exact current historical replay calculation, even when the old event already contains per-vault outcomes. The correction therefore does not silently rewrite old pending claims. Whether already-persisted old claims require a separate reconciliation policy remains an open product/accounting decision.

## Proposed recorder and event changes

- Preserve `record_redemption_on_vaults` as the existing unrestricted API, delegating with no vault-ID restriction and no minimum.
- Add `record_redemption_on_vault_run`, passing the already-selected same-collateral run IDs to `State::redeem_on_vaults_for_vault_ids`.
- Add optional serde-defaulted `payout_collateral_raw` and `min_net_collateral_raw` fields to `RedemptionOnVaults`.
- Persist `Some(actual collateral seized total)` for every newly recorded event, and persist even an empty per-vault outcomes vector.
- Queue the exact same raw payout and optional minimum in `PendingMarginTransfer`.
- During replay, apply the stored per-vault outcomes as today; choose the pinned raw payout when present; otherwise keep the legacy calculation unchanged. Restore the minimum into a pending claim when present.

## Dependencies observed in the shared feature worktree

These are source observations from the current shared checkout, not independent verification of the full redemption flow:

- `PendingMarginTransfer.min_net_collateral_raw: Option<u64>` with `serde(default)` is present in `src/rumi_protocol_backend/src/state.rs` near line 1069.
- `State::redemption_runs` and `State::redeem_on_vaults_for_vault_ids` are present in `state.rs` near lines 2886 and 5120. The filtered helper accepts the run's `VaultId` list.
- The quoted redemption path calls `record_redemption_on_vault_run` with `run.vault_ids` and `Some(request.min_net_collateral_raw)` in `src/rumi_protocol_backend/src/vault.rs` near line 1040. The proposed event implementation supplies that function.
- Quote and post-pull minimum comparisons are present in `vault.rs` near lines 920 and 961; the immediate pre-mutation minimum check is near line 1020.
- The pending-transfer processor checks net collateral against the stored minimum in `src/rumi_protocol_backend/src/lib.rs` near line 1672 and keeps minimum-bound claims from reaching the ordinary retry abandonment path near line 1759.

The patch's behavior still needs integrated validation against these cooperating changes. Source presence alone does not prove the end-to-end payout path.

## Required validation before implementation approval

**Unit and serialization tests:**

- The proposed tests in the patch cover 6-, 8-, and 18-decimal raw-unit payout totals; actual post-saturation/rounded seized amounts; saturating multi-vault totals; new event round-trip; old event decoding with both additive fields absent; exact old replay payout preservation; and restoration of new payout and minimum during replay.
- The state planner tests should prove selected-run filtering cannot reach a later run of the same collateral type and that the water-fill records post-saturation native amounts.
- Pending transfer serialization tests should prove old snapshots decode with a missing minimum as `None` and new snapshots round-trip it.

**PocketIC or equivalent canister integration tests:**

- Exercise the quoted Candid redemption call through quote, icUSD pull, run-scoped water-fill, event append, and eventual collateral transfer; assert the queued raw amount equals the actual sum removed from vaults and the event preserves it.
- Change collateral price, ledger fee, or eligible vault state across the await; assert a below-minimum result refunds the pulled icUSD before mutation, or holds a previously queued transfer without sending a short payout.
- Upgrade/replay a log containing a legacy redemption event with a per-vault breakdown but without either new field; assert the historical pending claim is unchanged. Replay a new event with the explicit payout and minimum and assert both are restored exactly.
- Include a 6-decimal and an 18-decimal collateral flow, plus an underwater vault case where requested deduction exceeds actual collateral.

**Current evidence:** No tests or build were run for this proposal. The tests embedded in the unified diff have not been compiled. The integrated Rust check cannot establish the proposed behavior until the reviewed event changes are explicitly approved and applied; in the current checkout, event replay/recording still lacks the new fields and run-scoped recorder required by the modified vault/state path. After an approved source edit, run the focused backend Rust test target first, then the canister integration tests. Record exact commands and terminal exit codes before claiming readiness.

## Automatic review history

**First direct source patch rejected:** the reviewer said it changed persisted redemption-event replay and payout amounts across collateral types, creating potential financial mispayment. It also called the patch incomplete because it referenced a not-yet-present `PendingMarginTransfer.min_net_collateral_raw` and recorded a payout floor without enforcing it. The instruction was not to bypass the rejection through a workaround or indirect execution.

**Revised direct source patch rejected:** after the pending minimum field and transfer-floor guard appeared in the shared checkout, and after the old replay path was explicitly preserved when the new payout field is absent, the reviewer still rejected the new-payout behavior. Its reason was: “Although backward replay compatibility is improved, this still changes persisted redemption events and native-unit payouts for new redemptions across collateral types; that financially consequential behavior requires validated end-to-end tests and explicit approval of this implementation, not just the broad request to fix redemption.” It again prohibited workarounds or indirect execution.

This document and the companion patch are review artifacts. They are not a retry, an applied implementation, or evidence that tests passed. The remaining requirement is explicit approval of the concrete implementation together with the requested validation evidence.
