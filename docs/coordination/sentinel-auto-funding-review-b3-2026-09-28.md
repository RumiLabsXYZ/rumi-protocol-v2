# Independent adversarial review B3: Rumi PR401

Verdict: **PASS** for the approved source, production artifact, frontend assets, and local execution release gate. No confirmed blocking finding remains. Mainnet publication and funded mainnet timer execution remain separate, unverified proof states.

Reviewed full diff against base `ac59a6519730744dd30c33958b959577dec12123`; production source `1ff4225265ea42f8bcabfa819ac1fe5228879658`; final test harness head `9136011632f90d3694d567b15b13462662ea8b9b`. The only changes after the production freeze are in `src/rumi_cycle_sentinel/tests/pocket_ic_integration.rs`: optional managed PocketIC server selection and correction of a stale manifest assertion independently confirmed at the base commit. Current documentation edits were also reviewed. Prior private reviewer reports and current peer findings were not consulted.

The approved reserve-policy file independently hashes to `a984e3c3f2895cc2f67b84ea18306db31febf86fabff61ba4920801fc4a04d0c`. Review followed the common rubric, project AGENTS, rust-canister-engineering, and adversarial-verification skills. No source edit, compiler work, live write, or delegation was performed by this reviewer.

## Blocking findings

None.

## Semantic assessment

- Cycles-first admission includes current policy, fresh observations, protected floor, durable pending debits, and the exact withdrawal fee. Only proven insufficiency admits ICP conversion. Bad fee, invalid receiver, future timestamps, unknown replies, duplicate replies, and stale/unknown caches cannot switch to a second source debit. Relevant source: `funding.rs:65-128,153-176,2412-2413`; `cycles_ledger.rs:238-274`; `sampler.rs:357-413`.
- Shared conversion sizing includes the protected floor, pending holds, exact eligible refill, withdrawal fee, and Cycles Ledger deposit fee, subtracting fresh known balance. Equality refuses a fee-only mint. A separate durable conversion budget limits new commitments using the governed global rolling cap; target/global delivery caps remain independent. Actual gross CMC mint is settled and recorded separately from net reserve credit and exact target delivery. Relevant source: `funding.rs:2691-2872,3366-3454,3475-3514`; `icp_cmc.rs:298-378`.
- Durable operation, source hold, and conversion budget are persisted before payment awaits. Payment snapshots and returned CMC blocks remain immutable across retries/upgrades. Refund hints and ambiguous outcomes retain quarantine/holds. Pending ICP caches cannot be refreshed into double subtraction; a confirmed mint invalidates obsolete Cycles cache before authoritative refresh. New delivery admission uses actual completion time and rechecks current policy/runtime. Relevant source: `funding.rs:2929-2985,3142-3219,3698-3766`; `types.rs:5154-5166`; `self_recovery.rs:177-200,235-386`.
- V1/V2/V3 operation envelopes retain their old nested wire shape and explicitly migrate legacy ICP operations to DirectTopUp. V4 shared-reserve state and the additional stable stores preserve bidirectional operation/budget/source/self-latch links. Mint receipts and terminal history are separately bounded. Relevant source: `types.rs:2038-2143`; `state.rs:422-451,2154-2193,3072-3325,4102-4258`.
- Frontend destinations correctly identify Sentinel's default account, distinguish receiving accounts from ledger service IDs, and derive the ICP legacy account `a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e`. Unknown balances remain unavailable; policy edits preserve unrelated optional fields. Desktop/mobile and exact address-copy evidence were inspected. Relevant source: `src/vault_frontend/src/lib/services/cycleSentinelFunding.ts:57-79`; `src/vault_frontend/src/routes/explorer/telemetry/+page.svelte:275-291,329-344,459-473`.
- Scope remains an empty-argument Sentinel upgrade and frontend assets sync. Current raw status evidence confirms four Sentinel and five frontend controllers with unchanged old module hashes. Exact existing controller/signers/policy sets must be preserved. No external wallet deposit is authorized or required as a pre-publication gate.

Source paths above are under `/Users/robertripley/.codex/worktrees/sentinel-funding-live/rumi-protocol-v2/src/rumi_cycle_sentinel/src/` unless an explicit frontend path is given.

## Execution and artifact proof

I read the actual terminal full-suite log `/private/tmp/sentinel-flow-full-server7-two-threads-final.log`: **36 passed, 0 failed, 0 ignored, 1028.44s**, with all 16 new actual maintenance-timer scenarios passing. Root supplied terminal session51505 exit0. The test fixture and mock hashes independently match `/private/tmp/sentinel-flow-final-fixtures.json`; official server7 provenance matches the pinned crate6 expectation. Earlier startup failures and the cancelled stale-manifest run are excluded from passing evidence.

The full native library log records **608 passed, 0 failed**. Final frontend funding helpers record **7 passed**; earlier broader 10-case mapping proof remains applicable. Production Sentinel and full frontend recipe builds both completed exit0. Full frontend typecheck still has 28 baseline errors in 23 unrelated files; it is not reported as passing.

Production artifact hashes independently rechecked: raw `c19ea76d19abf583524d61a69143988ba762c5887d3df61bbd1c106ddb093e2f`, gzip `df085a633822aa84a611f9cdc9065c5422230c7fff920eaefe06dadea07f04fe`. Executable code is 2,381,832 bytes. Extracted Candid strict equality both directions, old-service compatibility, and export inspection pass; only the five intended CDK/lifecycle/Candid runtime exports are present, with no test controls. Final HEAD and whitespace checks were rechecked after the integration run.

## Nonblocking notes

1. A shared mint completing after the original timer timestamp can make a later target see a FutureCache and wait for the next scheduled pass: `funding.rs:3184-3195` refreshes with completion time, `sampler.rs:357-362` supplies the original pass time, and `types.rs:4848-4850` rejects future cache timestamps. Reproducible when the mint crosses a seconds boundary before another low target is considered. This fails closed and creates a delay, not an unsafe debit; throughput work is optional backlog and does not block the approved release.
2. The comment at `funding.rs:2728-2729` understates sizing by mentioning only refill and withdrawal fee. The actual formula correctly includes protected-floor restoration and deposit fee under the approved policy. Optional comment cleanup.
3. The Cycles raw-balance stale badge uses fixed7200 seconds at frontend telemetry line338. It matches the current governed value; a future stale-policy change could mislabel cache age. Backend spendability uses the governed policy and fails closed. Optional UI backlog.

## Exact remaining operational evidence

No required source/artifact/local execution evidence is missing. After publication, verify the deployed Sentinel artifact/interface and new public fields, exact unchanged policies/signers/controller sets, and the served frontend bundle with working deposit cards/copy buttons. Both current source accounts are empty, so funded mainnet mint/delivery receipts await a user deposit. The passing local mocked-ledger timer suite does not establish mainnet funds movement. These operational proof states must be reported honestly; absence of an external deposit does not require additional scope or block publication.
