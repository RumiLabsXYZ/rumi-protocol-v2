# Season 2 history: actual T0 source boundary — November 24, 2026

Owner: Orin, S2-HISTORY-01. This extends the [earlier retained-input audit](season2-preflight-history-20261124.md); it does not change the [executed prospective release](../operations/season2-release-execution-20261124.md), historical credit authority or earned-point rules.

## Completed source reconciliation

The original November12 T0 collection remains available. Its raw SP `get_pool_events` Candid response was independently decoded against the retained source4117ba1b interface and matched the entire212-row saved normalized summary. That summary's SHA256 also matches the original bootstrap binding. Typed decoding confirms response compatibility; it does not independently prove the historical deployed Wasm/source identity.

Merging this original2384–2595 page with the already-verified0–2547 stream yields **2,596 consecutive IDs,0–2595:48 additional unique events,164 matching T0 overlap rows, zero conflicting overlaps and zero gaps**. Timestamps are monotonic. Every retained event is before T0 (November12 23:45UTC); the final event was23:24:37.908533UTC. The response was captured23:45:17.510–18.752UTC. This closes the previously unverified SP event coverage from the November10 endpoint through the actual approved cutoff.

For November1→T0, the retained stream contains221 InterestReceived events, one LiquidationNotification, one LiquidationExecuted and two OptOutCollateral events. It contains **zero Deposit/Withdraw events** in that interval. These are stream observations, not per-owner balances or an award transcript; events before November1 remain part of the preserved prefix.

## Independent next boundary check

The original T0 3USD ledger response was independently decoded as well. The retained baseline and T0 both have log length **2,489**; the T0 response contains last block ID **2,488**, timestamp October8 02:28:58.647813UTC. The T0 query was captured November12 23:45:21.044–22.448UTC. The observed log-length delta is zero. This supports the existing no-observed-ledger-activity model through T0; log length is a count, not the last block ID.

The17-query original T0 evidence set consists of global backend registration/timestamp scans, SP event/count reads, 3pool/AMM counts and the ledger tip. It contains **zero participant-addressed vault, SP-position or wallet-balance reads**. The bootstrap binding explicitly records no participant-addressed queries. An existing preparation helper is not a completed T0 per-owner position replay; its required current-position files are not supplied by this collection.

## What remains unsupported

The checked SP interface defines InterestReceived with token ledger and aggregate amount (`src/stability_pool/stability_pool.did` at4117ba1b, line504); it supplies no per-owner interest allocation or collateral eligibility tag in that event. Complete event IDs therefore do not resolve recipient allocations. Two opt-out events are not a complete initial eligibility-state snapshot. The global source reads are sequential post-cutoff observations, not an atomic participant state snapshot.

No random historical A/B holdings or hidden vault accrual clocks were recovered. No exact historical points, zero-owner defaults, forfeiture or compensation were inferred. Per-owner SP allocation/eligibility provenance, missing exact snapshot evidence and the earlier debt clock/residual remain separate gaps. The baseline's earlier SP export-gap warning is superseded **for IDs0–2595 through this T0**, not for all later time or all reward categories.

No native replay, build, repeated test or live call was needed. Raw pages, participant rows and hashes with local paths remain private. The new unique-event/ledger-boundary result is concrete progress; it is not historical award completion. Next useful accepted work is source-bound allocation/eligibility or original operation-clock evidence, rather than another unchanged event replay or a fitted clock. Prospective scoring is independent and remains active under its existing rules.


## Next accepted evidence item: preference-anchor binding

All15 original late-November1 SP text responses were typed re-encoded against the4117ba1b UserStabilityPosition definition and independently decoded; the entire result matches each retained typed response. This is binding to saved **Candid text**, not original response-byte provenance. Eleven responses contain actual positions with both opted-out and eligible-interest preference fields explicitly present; four explicitly return no position. All11 eligible-interest options are present.

Thus preference evidence is **partial, not wholly absent**. The retained later global snapshot lists13 depositors;11 match these November1 position anchors and2 lack them. These are saved-snapshot counts, not a current live depositor inventory. The anchors are the late-November1 file-time observations, not midnight and not T0. They do not provide the missing event-specific collateral source or complete initial balances/eligibility of all pool members; denominator and rounding allocation remain unresolved. No missing owner's balance or preference was defaulted to zero.

A directly relevant next source capability exists: the pinned backend records private TreasuryInterestMint phases with a StabilityPool collateral route. Inspect whether the already-retained private journal actually contains provenance-bound records before choosing another replay. This is a candidate source, not a recovered historical mapping or an added release gate.
