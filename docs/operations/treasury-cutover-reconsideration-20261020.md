# Treasury cutover: one-attempt reconsideration and verification

## Proposed decision

Authorize **one additional Treasury upgrade attempt through the normal approval review system**, accepting the unresolved risk that an unidentified earlier request could still execute. This authorization is specific to Treasury; previous points authorizations do not cover it. A fresh rejection stops execution. No alternate executor, indirect execution, native UI installation or repeated install wrapper is permitted.

The identified historical Treasury wrapper has a terminal tool rejection. The rejection refers to an allegedly earlier unresolved request for which no internal request identifier, cancellation receipt or terminal outcome has been established. The corrected audit distinguishes these two facts. An old deployed module or successful points recovery does not resolve that uncertainty.

## Exact immutable candidate

- Source: merged PR #527, commit `1c45ded4c8ad85e396626421674a9da4f8148be1`.
- Target: Treasury `tlg74-oiaaa-aaaap-qrd6a-cai` on IC.
- Mode: **upgrade**, argument `()`. Never reinstall or restore a historical financial snapshot.
- Wasm SHA256: `73f5a6b7e06cd91ab808887a6366f1e780054b40b0621928b9b739e5964d32e3`.
- Wasm length: 1,224,702 bytes; executable code: 935,111 bytes.
- The artifact was canonically reproduced previously; its hash, compatibility against freshly read live Candid, and endpoint correspondence were rechecked on October 20. No new build or tests were run for this reconsideration.

## Fresh preparation already observed

Treasury was Running on its old module, unpaused, with 2,123 deposits. All deposit records were captured in three typed pages (1,000 + 1,000 + 123), strictly sorted and unique, with unchanged status before and after capture. The developer identity and backend membership in the Treasury management controller set were verified. Private operational records and snapshots remain local.

These observations must be refreshed if execution is delayed or state changes. Bookkeeping balances are not ledger settlement proof.

## Authorized execution after the explicit decision

1. Revalidate the exact artifact, identity, complete controller set, backend controller membership, sole root execution ownership, current module, source bindings and complete deposit baseline. Preserve unrelated services and all existing holds.
2. Take and download a fresh supported Treasury snapshot before installation, with one bounded stop/create/download/start sequence as needed. Verify all payloads, including stored-module encoding and expanded hash. Keep Treasury query availability restored if an approval remains unresolved. Never restore stale state or delete protected snapshots to create capacity.
3. Submit exactly one normal-reviewed upgrade of the pinned artifact. Preserve the exclusive request marker, command and terminal result. On unknown outcome or rejection, stop; query-only observation does not authorize another submission.
4. After actual success, verify Running on the exact new module, unchanged controllers, pause state and every pre-existing deposit record. New ordinary deposits may append; do not require legitimate financial totals to freeze across unrelated activity.
5. Run bounded `continue_icusd_deposit_block_backfill()` calls as the verified controller. The implementation handles at most 100 historical deposit rows per call, keeps icUSD ingress held until completion, and marks zero/duplicate historical blocks ambiguous rather than authorizing replay. The operator is limited to 31 migration calls plus one completed no-op (32 total update calls maximum) and stops on every error, timeout, decoder failure or unexpected result. It performs no mint, transfer, withdrawal, receipt clearing or funds replay.
6. Require an actual typed successful completion response, followed by one typed successful completed no-op reporting zero processed rows. Confirm every pre-existing deposit record remains unchanged, the exact module remains Running and all unrelated service holds persist. Observe ordinary backend acknowledgments without manually replaying funds or clearing receipts.
7. Two independent actual-evidence reviews must verify the upgrade, retained data and backfill receipts before claiming completion. Preparation, source review and a queued request are not completion.

## Scope boundaries

Points recovery is complete and must not be repeated. No backend/bot upgrade or restart, oracle timer setter, forced mode, cap expansion, Stability Pool activation, reserve redemption enablement, foreign-chain activation, financial canary or ledger replay is included. Root remains the sole Treasury executor. This proposal does not claim that the unidentified earlier request was cancelled.
