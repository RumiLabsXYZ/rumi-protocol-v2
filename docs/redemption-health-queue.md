# Redemption health queue: audit and design

## What the queue should tell a redeemer

The queue is a live snapshot of which vault collateral is next under the protocol's redemption health rule. It answers two separate questions:

1. Which collateral is currently first, and how much debt-backed payout capacity is in that contiguous run of vaults?
2. What would the selected redemption return now, in the exact token and native token units?

The list is a ranked snapshot, not a promise that one large request will traverse every row. One transaction redeems one collateral token from one contiguous run. A request larger than the first run's eligible debt capacity is rejected before icUSD is pulled. After a successful redemption, the protocol recomputes vault health and the queue can change. A different collateral can therefore move to the top before the prior collateral's remaining debt is exhausted.

This distinction preserves a truthful per-transaction payout while showing the wider on-deck order. It also avoids implying that all displayed rows form a guaranteed multi-token waterfall.

## Why health must be normalized

A displayed collateral ratio is meaningful only against that collateral's own thresholds. For example, 150% CR on ICP can be close to its 133% liquidation boundary, while 145% on ckXAUT can be much farther from its 112% boundary. Sorting the raw ratios would put ckXAUT first even though the ICP vault is more exposed relative to its own risk limits.

The vault card's CR text fades from white to pink between a per-collateral fade-start of `1.351 × borrowThresholdCR` and that collateral's liquidation CR. The redemption queue uses the same underlying interval as a numeric headroom:

```text
headroom = (currentCR - liquidationCR)
           / (1.351 × borrowThresholdCR - liquidationCR)
```

Lower headroom means a more pink/red CR text and less distance from the liquidation boundary. Sort ascending, without clamping. At the liquidation boundary the score is `0`; below it the score is negative; at the white-to-pink fade start it is `1`; above it the score is greater than `1`. This keeps genuinely unhealthy positions at the front and retains useful ordering outside the visible color transition.

Using the example thresholds:

| Collateral | CR | Liquidation CR | Borrow threshold | Headroom | Priority |
| --- | ---: | ---: | ---: | ---: | --- |
| ICP | 150% | 133% | 150% | `(1.50 - 1.33) / (1.351 × 1.50 - 1.33) ≈ 0.244` | Earlier |
| ckXAUT | 145% | 112% | 118% | `(1.45 - 1.12) / (1.351 × 1.18 - 1.12) ≈ 0.696` | Later |

This is the shade-order idea expressed as a number. It aligns with the CR text interpolation in [`VaultCard.svelte`](../src/vault_frontend/src/lib/components/vault/VaultCard.svelte#L205) while making scores comparable across collateral types.

## Queue construction and row meaning

The backend takes one consistent state snapshot and:

1. Selects vaults eligible for redemption: positive debt, a supported payout rail, redemption-enabled collateral configuration, a positive usable price, not claimed by the liquidation bot, and not held by a conflicting per-vault operation lock. A missing or stale price, or invalid thresholds that make the denominator nonpositive, must make the quote unavailable rather than silently assign a rank.
2. Computes each eligible vault's CR from that collateral's configured price and decimals, then computes normalized headroom using the formula above. The update path refreshes/checks oracle freshness before accepting the quote; a read-only query must expose stale/unavailable state instead of implying it refreshed the oracle.
3. Sorts all eligible vaults globally by headroom ascending. Deterministic tie-breaks should use configured redemption tier and then stable vault/collateral identity; redemption tier is only a tie-break after health.
4. Groups adjacent entries with the same collateral principal. The same collateral may appear more than once if another collateral's vaults fall between its vaults in the global health order.
5. Treats the first run as the only executable run for the current transaction. Its debt-backed capacity is the sum of eligible debt in that run, not the sum of every token unit held in all vaults of that collateral type.

Each queue row should show its position, collateral symbol, current normalized health/headroom (and optional CR for human context), run debt capacity, and the equivalent maximum collateral payout in that token's native units at the current quote inputs. Rows after the first are informational snapshots. They do not make those other collateral types spendable in the current call.

Within the first run, the existing water-fill fairness rule remains: it raises the least healthy eligible vaults toward the next CR band and splits debt within a band. It must be restricted to the vault IDs in the first contiguous run. Otherwise a quote for a small run could silently seize collateral from later runs of the same token.

The list must be recalculated after each transaction. In particular, it must not tell users that an initial `70 ICP` bucket must be fully exhausted before any other collateral can rank first: redeeming from that run improves its vaults' health, so the global order can change after a partial request.

## Quote and submit contract

The browser must not calculate the payout token from an ICP assumption or independently reconstruct the fee/RMR math. The backend returns the quote from the same state and rules used by the update path, including selected collateral principal, symbol/decimals, price, fee, RMR, effective debt capacity, expected native-unit payout, and a minimum payout bound.

Submission carries the quoted collateral principal and minimum acceptable payout. Before pulling icUSD, the backend recomputes the current queue and quote, verifies that the requested principal still matches the executable first run, checks that the amount fits that run, and verifies the current payout meets the minimum. A stale quote, changed first run, insufficient capacity, stale price, or amount below the backend minimum fails before the icUSD transfer. After the asynchronous icUSD pull, the backend must recheck mutable fee/RMR/capacity inputs before committing the vault mutation; any changed result below the caller's minimum must be compensated without leaving an unbacked burn.

The API addition should be additive: introduce typed quote/queue results and a new quote-bound redemption entry point, while keeping existing Candid methods and their wire shapes available for older clients. Regenerate declarations from the canonical backend Candid source. Do not hand-edit generated declarations.

The page should use the quote-bound vault route even when reserve redemptions are enabled. This page's promise is that the selected queue row is the collateral paid; the existing reserve-first route can return ckUSDT/ckUSDC and then a vault collateral, so it does not satisfy that promise. The legacy reserve method remains available to its existing callers.

The displayed token quantity and minimum must use the collateral's configured decimals. For example, converting icUSD value into raw token units must use the same decimal-aware conversion used by the backend water-fill, then return those raw units to the browser. Do not assume 8 decimals for every collateral.

## Report queued payout honestly

The current update result reports `collateral_amount_received` from the calculated margin, while also placing a `PendingMarginTransfer` into state for asynchronous processing. The amount is therefore a queued payout at that point, not proof the ledger transfer has completed. The UI should say the redemption was accepted and the quoted collateral payout is queued, then show delivery only from authoritative transfer status. If a wallet wrapper returns an ambiguous transport error, do not describe that as a confirmed payout or as proof that only the icUSD burn succeeded; reconcile the update result and transfer status.

A persisted minimum-payout floor on the pending-transfer record could strengthen later fee/settlement changes, but it is optional follow-up work. It is not a prerequisite for the bounded quote/submit check if the pending amount is fixed and already satisfies the accepted minimum.

**Review status:** a proposed `event.rs` payout/persistence source edit was rejected by automatic approval review because it changed high-impact accounting while its pending-minimum field and enforcement dependencies were incomplete. `event.rs` remains unchanged. The implementation now has a serde-default `min_net_collateral_raw` field in `PendingMarginTransfer` (`state.rs`) and backend quote/submit and pending-work enforcement edits are in progress, but those pieces have not yet been reviewed or verified together. New payout-unit persistence and replay compatibility are still a distinct unresolved part of the full contract. Do not claim the pending minimum or post-queue floor is end-to-end implemented until its write/read enforcement, failure/refund behavior, legacy replay path, and regression evidence are checked as one accounting transition. Do not route the rejected edit through another tool or path.

## Current implementation evidence

At baseline `ac59a6519730744dd30c33958b959577dec12123`:

- [`Vault::health_score`](../src/rumi_protocol_backend/src/vault.rs#L149) currently returns `CR / liquidation_ratio`. This compares each vault with its own liquidation threshold, but does not implement the frontend's CR text-shade interval.
- [`State::get_collateral_types_by_redemption_priority`](../src/rumi_protocol_backend/src/state.rs#L2857) currently aggregates each collateral to its worst vault health and sorts redemption tier first, health second. It also counts debt-bearing vaults without the same bot/operation-lock exclusions used by capacity and execution.
- [`State::redeem_on_vaults`](../src/rumi_protocol_backend/src/state.rs#L4990) water-fills eligible vaults of one collateral type by raw CR and skips bot-claimed or locked vaults. It has no run-ID restriction today.
- [`redeem_collateral`](../src/rumi_protocol_backend/src/vault.rs#L557) validates the caller's requested collateral exists, then selects the first priority collateral at lines 600–611. The argument does not select the returned token. It checks capacity for that one collateral at lines 637–659, then queues a pending transfer at lines 769–776.
- [`redeem_reserves`](../src/rumi_protocol_backend/src/vault.rs#L437) also chooses only the first priority collateral for vault spillover. The reserve path may pay a stablecoin as well as that collateral.
- [`/redeem`](../src/vault_frontend/src/routes/redeem/+page.svelte#L173) estimates reserve plus ICP value locally; its no-reserve submit path calls `redeemIcp` at lines 306–320 and displays an ICP estimate. The reserve path labels vault spillover as ICP at lines 276–292. The page has no backend-authoritative queue or token-bound minimum.
- Existing docs say tiers are traversed and lowest-CR vaults are targeted ([`docs/redemptions/+page.svelte`](../src/vault_frontend/src/routes/docs/redemptions/+page.svelte#L214)); wording should be reconciled with the new health-ranked one-token-per-call behavior.

## Verification plan and scope boundary

The regression suite should cover: cross-collateral ordering by the exact shade-headroom equation; ICP 150% before ckXAUT 145% for the example thresholds; same-collateral adjacent grouping and a collateral reappearing after an intervening asset; deterministic ties; exclusion of inactive, unpriced, zero-debt, bot-claimed, and locked vaults; run-local debt-backed capacity; decimal-aware payout units; stale queue/principal rejection before icUSD transfer; min-payout rejection before burn; post-await fee/RMR/capacity change compensation; and queued-versus-delivered response semantics. Add a Candid/API-boundary test for the new public methods and frontend tests that reject stale/error quotes and show the exact collateral.

Run focused Rust unit tests first for the pure ranking, grouping, capacity, and conversion logic; then the narrowest PocketIC/interface test that exercises quote and submit. For the frontend use the focused Vitest tests, `npm run check`, and production build when the implementation stabilizes. Run adversarial review after deterministic checks because this touches redemption accounting and user-facing financial claims.

The exact cross-collateral global water-fill and a single multi-token payout are deferred. They are larger behavior changes than the user-visible queue and quote contract requires. The rows after the first communicate current order and debt-backed capacity; only the first run can be submitted at a time, and the order is recomputed after each redemption.

Verification results are pending implementation and review. This document records the design contract; it is not evidence of a merged change, deployment, or live payout behavior.
