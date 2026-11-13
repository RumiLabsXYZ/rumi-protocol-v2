# SEC19 remaining source validation — fresh allocation required

Continue the unchanged backend source **e2a0122a9a35393e2c0509540551594a403695ab** on `codex/add-margin-recovery-20261113`. This is source validation under the existing collateral-deposit repair assignment. It does not authorize production release, a flag bypass or a vault transaction.

## Retained first-run evidence

Allocation `174c2752-147f-4475-99b0-f86aafcc1816` was consumed once. Its [receipt](evidence/add-margin-validation-20261113/first-run/completed-checks.json), [native stages](evidence/add-margin-validation-20261113/first-run/steps.json), [session](evidence/add-margin-validation-20261113/first-run/validation-session.json) and [independent empty-group census](evidence/add-margin-validation-20261113/first-run/independent-group-closure.json) are preserved. Six new SEC19 unit tests, one old-journal compatibility test and two existing transfer-outcome tests passed. All nine ran against the exact pinned backend inputs.

The run stopped during unfinished canonical Candid generation after 1,286.65 seconds because the filesystem free-space-loss guard exceeded 8 GiB. That metric was 8,644,390,912 bytes; it measures host filesystem free-space loss, not attributable per-job allocation. Minimum free space remained 36,212,932,608 bytes, above the 20 GiB floor. Peak owned RSS was 2,506,981,376 bytes. The driver was terminated with native exit -15, the outer runner exited 1, and owned process group 73091 was independently empty. No Candid/declarations were changed and no Rust source error was reported. This is a stopped resource-bounded run, not a passing full validation.

Never reuse that allocation or dispatch marker. Preserve its logs, test outputs, cache, source and all SEC17/SEC18 release artifacts. No automatic retry or cleanup is proposed.

## Exact remaining batch

The [continuation runner](evidence/add-margin-continuation-20261113/run-once.py), [steps](evidence/add-margin-continuation-20261113/steps.py), [manifest](evidence/add-margin-continuation-20261113/manifest.json) and [PocketIC tool pin](evidence/add-margin-continuation-20261113/tool-pins.json) are readable source artifacts. Runtime namespace is `/private/tmp/rumi-sec19-continuation-20261113`. It has no dispatch marker or allocation yet.

Before work, verify unchanged HEAD and all backend input bytes, the original nine test results and exact retained evidence hashes. Reuse the already populated cache without touching unchanged source mtimes or rerunning the completed tests. The remaining sequential steps are unchanged from the original plan:

1. Canonical `RUMI_REGEN_DID=1` bin compatibility test and canonical declaration regeneration.
2. Actual old-service subtype check and the ordinary declared/generated compatibility test.
3. Source-matched release backend/flaky-ledger Wasm build, existing metadata/shrink/O1 pipeline, measured executable-code guard and artifact hashes.
4. Only the three `sec19_` cases in the existing PocketIC harness: committed lost reply/exact retry/one credit; committed lost reply then zero-debt target closure/positive block proof/one held liability surviving ACK; and typed no-effect rejection/ACK without a pull.
5. Check unchanged non-generated backend inputs and all owned process closure. Every test stage must execute a positive count.

Use installed, offline tools and the same source-matched cache; one Cargo job and one Binaryen core. Total 30 minutes includes 60 seconds owned closure. Maximum aggregate owned RSS and filesystem free-space loss are each 8 GiB, with a 20 GiB free-space floor checked afresh. PocketIC server 7.0.0 matches crate 6.0.0; no download, new harness, broad suite, dependency install, paid fallback or binary export. Preserve the existing 10,223,616-byte project code guard; this is not a network-limit assertion.

Cody must independently close the old allocation and issue one fresh exclusive allocation with UUID, path, SHA256 and start deadline. Verify the new packet and grant, then dispatch this continuation once. Failure, uncertainty or a resource boundary stops without automatic retry. Frontend changes remain a separate unstaged source lane and are excluded from this backend validation. Only canonical Candid/declaration writes are expected in backend inputs. No production, reserve, snapshot, controller, signer, approval, transfer or financial action is included.
