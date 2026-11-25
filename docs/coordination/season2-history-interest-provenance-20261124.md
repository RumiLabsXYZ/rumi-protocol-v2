# Season 2 history: collateral-tagged interest provenance — November 24, 2026

Owner: Orin, continuing S2-HISTORY-01 after the [actual T0 boundary result](season2-history-t0-boundary-20261124.md). This is a source-evidence result, not an award calculation or a change to the [executed prospective release](../operations/season2-release-execution-20261124.md).

## Concrete result

**All221 retained November1→T0 SP interest events now have uniquely matched, collateral-tagged mint candidates whose exact mint tuples are independently bound to public ledger receipts.** No historical points were calculated, imported or compensated.

The public SP InterestReceived event lacks collateral and per-owner allocation fields. The already-retained backend private payout journal supplies a different source: explicit StabilityPool routes in TreasuryInterestMint records. Three existing backend backups were inspected read-only. Journal prefixes remain byte-identical across5,902→6,070→6,567 entries. No new snapshot, production call or security campaign was started to obtain these records.

The post-T0 backup contains221 SP-directed operations created between November1 and T0, across six explicit collateral routes. Every selected operation has exactly one ordered Started→Updated→Accepted→Completed lifecycle; the tuple and route stay identical in the Updated record. Each has a distinct accepted mint block. Older records without an explicit route were kept separate, rather than treating a modern decoder default as recorded historical evidence.

All221 operations uniquely match the retained public SP stream by token ledger and amount, with consistent exact-integer chronology and backend caller. There are zero missing or ambiguous candidates and no unpaired interval interest events in the inspected set. This is an observed/source-supported join: the public SP event itself contains no mint nonce, so the event-ID association remains an inference rather than a direct nonce receipt.

## Independent public-ledger binding

Six bounded anonymous icUSD ledger queries returned exactly221 distinct accepted mint blocks. All221 match the private operation's **mint operation, amount, memo, created-at time, SP recipient and default subaccount**. Every memo equals the recorded128-bit nonce, and observed ledger mint times precede their matched SP events. Comparisons preserve full integer precision. There were **zero tuple mismatches, updates or transfers**. The journal and raw participant records stayed local; only public ledger block indices were sent in read requests.

The first retained snapshot supported178 interval candidates; the next extended that to192; the already-retained post-T0 snapshot covered the final29. The final221 result supersedes those partial counts. It is mint provenance, not per-owner credit proof.

## Remaining allocation evidence

The earlier anchor binding verified15 saved SP text/typed responses:11 preference-bearing positions and4 explicit empty positions, all at the late-November1 anchor rather than midnight. The later global snapshot has13 depositors;2 lack matching original position anchors. Both recorded interval OptOutCollateral events belong to the saved cohort; none is attributed to those two outside owners. Absence of recorded changes does not by itself prove their initial preference state or balances.

Exact per-owner eligible denominators, initial outside-owner state and integer rounding allocations remain unresolved. So do missing exact random historical A/B snapshots and the earlier nonzero debt clock/residual. Do not equate the221 verified mint tuples with221 owner allocations or use absent records as zero. Prospective scoring remains independent; all financial/SP, historical-credit, compensation and policy holds are unchanged.

## Next existing-queue checkpoint

Expected result was to test whether public-event collateral attribution was unavailable. Actual retained journals plus ledger receipts provide the entire interval's mint provenance. **Continue and narrow** the historical model work to initial owner balances/eligibility and allocation rules; do not repeat unchanged event extraction, synthetic tests or a debt replay with no new clock input. The new private route/receipt data are ready inputs for that work, while exact-award claims remain unsupported. No user policy decision is requested by this source-only result.

Proposed durable owner lesson: distinguish an explicitly recorded historical route from a field supplied by a later serde default. In pinned source4117ba1b, `src/rumi_protocol_backend/src/state.rs:2812–2815` documents the default; the historical data contain both recorded-route and legacy missing-route shapes. This is a catalog proposal, not a catalog/memory edit or a new release gate.


Input-readiness follow-through: both outside-anchor depositors have **positive icUSD balances at the retained November10 endpoint**. Their missing initial state therefore cannot be bypassed by treating them as zero or irrelevant to interest eligibility. This is an endpoint observation, not a proven opening balance. The next accepted allocation step must preserve those two owners and the earlier midnight-to-anchor gap.
