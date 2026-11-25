# Season 2 approved release preflight and historical provenance — November 24, 2026

Owner: Orin. This is an execution/status receipt, not a new release scope or changed points policy.

## Approval and execution

Rob directly answered “Approve the exact Season 2 release” in Ansel's chat, human message `01a210f9-1ef0-7332-8ddd-add7ee8db4ef`. Orin independently read the actual human question/reply. The approved contract remains [packet32fdd7ac](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/32fdd7ac/docs/operations/season2-repair-20261121.md), SHA256 `4abbbbe3da9b51431c9ca4546c35a97d894cf03dedba247b5ddc26c60d785266`.

Approval is recorded **approved / execution blocked**, not awaiting a new product decision. Automatic approval review rejected the first stop command before process creation: it classified the live stop as a production-disruptive mutation and did not accept the human approval retrieved from another chat as trusted direct authorization here. No alternate sender/executor/route was used. S2 was not stopped, funded, backed up, upgraded, archived or activated; frontend assets were not published.

Remaining execution permission is direct approval in Orin's chat for the same exact packet: corrected assets-only banner publication, stop S2, one 1T execution-cycle top-up plus 100M fee from the existing default deploy account, private backup, exact guarded upgrade, epoch0 archive, held catch-up and prospective activation. No missed-period credit, compensation, forfeiture, SP action or other funds are included.

## Fresh preflight

Read-only receipts retained privately on November25 UTC / November24 Pacific:

- S2 `2cvsh-3aaaa-aaaaj-qspga-cai` Running, sole controller/admin unchanged, old gzip module `d301b54c5c2db14fa26baff7435f8152bbb8623866d3bf2fd703ae2b2c039d9c`, 151,718,235,943 execution cycles. Epoch0, A complete/B absent, both timers enabled, cursors570731/111/2709/5. No ordinary ledger rows or positive leaderboard total. Finished15-row zero-credit bootstrap unchanged; backfill absent. No existing management snapshots.
- Default deploy account remains3,400,550,044,313 cycles on ledger `um5iw-rqaaa-aaaaq-qaaba-cai`; fee100,000,000. Approved debit1,000,100,000,000. No additional wallet payment is needed. Destination is actual execution cycles on S2, not a token transfer or top-up to the ledger itself.
- Full scorer candidate gzip hash still `7c2bccf91c018fc0ad5ce9cf7338a9c2c4974c36a5b0114dd5defbc46f07175f`. All761 prepared UI file hashes match manifest `9b45bbc916bf798c5a4b83cd8acceba3cc9b24ba51d2f3d85beae86fec05b0e7`. No repeated build or passing test run was needed.
- Frontend target/controller set/module/environment unchanged from the packet. Its S1 environment binding remains S1. Full paginated asset backup and publication readback remain execution steps, not completed proof.

The pinned epoch1 activation window remains usable at this checkpoint: activation must succeed **before November26 15:05 Pacific /23:05UTC**. It ends at November26 15:45Pacific. Re-read state and balances before executing; do not extend, backdate or silently use another epoch. A denied stop is not a funding outcome. No transfer was submitted.

## S2-HISTORY-01 bounded provenance result

All90 input hashes recorded by the retained baseline match their current bytes (zero missing/mismatched). The original60 raw position responses remain, including15 vault responses. Their filesystem modification times reproduce the retained November1 16:51:48.270836–16:51:57.243692UTC anchor. The existing extraction script explicitly obtains this from file timestamps, not returned canister time. None of the15 vault responses exposes `last_accrual_time`.

This establishes input preservation and anchor provenance. It does **not** establish an atomic common snapshot, the exact query-execution instant or the hidden per-vault accrual clock. The first16h51m of November1 remain unsupported by these position anchors. No clock was fitted to balances.

A bounded filename inventory of the original November1 reconciliation tree found no repayment/withdrawal request, ingress, rejection or failure receipt. This is a scoped evidence result, not proof that no failed call happened or that no other private archive can exist. The previously referenced final residual/comparison files are no longer present at their recorded paths; the last reported42/49 match and7 overall/1 tracked residual are retained as prior evidence, not freshly reproduced. No fresh residual improvement is claimed.

Source at diagnostic pin `4d2d1b23` has repay accrual before caller/minimum/debt/transfer failure branches (`vault.rs:9436–9507`), and partial-withdraw accrual before later price/config/minimum validations (`vault.rs:10690–10746`). That supports a possible unlogged accrual mechanism. It does not prove a particular historical failed call, nor establish byte identity to the November1 deployed module. Four already-bound successful memo checkpoints and eight claim-journal stages are prior completed work; no duplicate synthetic task/replay was launched.

Independent category evidence was also classified: retained 3pool endpoint-VP output explicitly labels itself a proposed model rather than exact historical snapshot awards; baseline SP exports record gaps and aggregate interest rather than a complete per-owner accrual history. These older baseline gap notes must not be promoted to a claim that later retained SP replay failed to fill them. Wallet endpoint balances and event receipts do not supply missing random A/B snapshot holdings. No category totals were imported or relabeled exact.

Recommendation: continue prospective release under its already-approved contract once execution review accepts direct authorization. Historical source-model reconciliation remains separate. The next useful historical input is a provenance-bound hidden-clock/operation receipt or recovered original private comparison, not another fit or repeated passing synthetic test. Missing random historical snapshots and `[T0,T1)` activity remain unresolved; no forfeiture or compensation is inferred.

## Evidence boundary

Private journal records each read's command, timestamps, exit status and output hashes. The production stop has a separate rejection receipt and no command process. Raw participant rows, private seeds, future capture schedule and snapshot bytes remain local. Worktree inventory removed zero; guarded cleanup could not refresh origin because `.git/FETCH_HEAD` was not writable. Locked/dirty/unmerged worktrees were preserved.

## Independent SP stream audit after the initial provenance checkpoint

The next already-allocated category check verified all39 inputs of the later SP cohort reconciliation: zero missing/mismatched files. Independently merging its five full source pages and tail yields **2,548 distinct events, IDs0–2547, zero gaps, 62 identical overlap rows and zero conflicting overlaps**. This supersedes the earlier baseline's export-gap note for that frozen endpoint. It does not prove later coverage through T0 or a current live source tail.

Complete event IDs still do not establish exact historical per-owner SP allocations, eligibility or random snapshot holdings. The retained later model explicitly uses loose allocation bounds and a post-cutoff endpoint proxy; its cutoff is November10 16:00UTC, not the final November12 T0. No replay, credit or fitted timestamp was added. Next history work should target actual hidden-clock/operation receipts and the remaining endpoint/eligibility evidence, keeping event completeness distinct from award completeness.
