# Vault 197 repayment frontend artifact and publication scope

Prepared for the current OISY repayment failure. **Source, focused tests and the exact artifact are verified; no publication or customer repayment has occurred.** Dorian remains the sole prospective publisher and resolves current exact publication authority before any write. Merritt remains the single customer-case coordinator.

## Fixed behavior and source

The repayment click now owns one uniquely named signer window and one SDK agent for both ledger approval and the backend repayment call. Cancellation closes the window immediately and cleans up a channel that finishes opening later. Existing caller/session/view, token, amount, approval expiry, backend methods and unknown-outcome guards remain intact. No automatic financial retry was added.

Runtime source is `3294c9561664d2ec48070ac0bab50862913f22dc`, full tree `4d04bbcaafc1a13ac5f3fe4931ee9913acfe45f0`, frontend tree `e25d2ef75c505890ae6828b32fae86d235c55dcf`. Exactly seven frontend runtime/test paths differ from deployed `8e6f6f01`; its deposit and recovery-menu/queue fixes are composed. Scripts, declarations, package manifests and lockfile are unchanged. [Source/test/review proof](vault197-oisy-repayment-20261118.md) records 44 passing focused tests and two fresh final independent source reviews after fixing the independently discovered pending-handshake cleanup race.

## One actual build and frozen bytes

One offline source-matched build under fresh LOCAL `aa43e929-9bff-42a3-974f-ffd475479d66` passed exit 0 in **54.580 seconds**. Peak owned RSS was 2,157,445,120 bytes; maximum observed physical growth was 28,565,504 bytes. Source and dependency pins remained unchanged and owned process group 8437 closed. Cody independently verified all 764 artifact files, all 646 exported source files, dependencies, resources and empty group, then released the consumed allocation. There was no repeated build, package installation or new worktree.

- [Frozen artifact](evidence/vault197-oisy-repayment-artifact-20261118/frontend-artifact.tar.gz): **8,191,223 bytes**, SHA256 `3915e35932d1b47df5a8a8345c61c1416fc4a26fec4ea3642a8a54913a4ac82e`.
- [Complete manifest](evidence/vault197-oisy-repayment-artifact-20261118/artifact-manifest.json): **764 files / 11,322,356 bytes**, SHA256 `c0e20f5654625c32ca442e1be7dc49119d5cf9de0622be85740440e0a0bae6ae`.
- Generated frontend version: `1795064489138`.
- [Actual build receipt](evidence/vault197-oisy-repayment-artifact-20261118/receipt.json), [redacted build log](evidence/vault197-oisy-repayment-artifact-20261118/build.redacted.log), [independent verification](evidence/vault197-oisy-repayment-artifact-20261118/independent-root-verification.redacted.json) and [compiled behavior checks](evidence/vault197-oisy-repayment-artifact-20261118/compiled-checks.json).

The PATH preflight was reconciled before dispatch: the launch explicitly used the same `/usr/local/bin` Node 24.13.1 / npm 11.8.0 recipe. No request, runner, source or dependency pin was substituted. Earlier completed build grants and artifacts remain untouched.

## Fresh baseline and exact additive plan

The stable double-read baseline finished **November 18, 2026 at 9:00:11 PM Pacific**: 1,088 asset keys; frontend Running `04e565b3`; backend Running `4409bfb7`; certified backend Candid SHA256 `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6`; observed Stability Pool hold=true. Management settings, certified controllers and Commit/Prepare permissions are frozen alongside the inventory. The publisher refreshes and compares them again before uploads and commits; any drift stops for reconciliation.

Target is existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai`, network `ic`, identity `rumi_identity`. The [exact plan](evidence/vault197-oisy-repayment-artifact-20261118/dispatch/dispatch-manifest.json), SHA256 `36cc96e745d8710b6ccf586c17a806139bb59157bbfc10c3b2b3374a7c8ee123`, is:

1. Stage **267 new immutable keys / 356 encoding sets** using one batch, six chunk calls and one commit. Existing active entries stay unchanged. Verify all planned stored encodings, new identity HTTP responses, properties and preserved baseline before activation.
2. Activate **7 encoding sets across the same six entry keys**: `/index.html`, its `.br` and `.gz` sidecars, `/_app/version.json`, and its `.br` and `.gz` sidecars. One batch, two chunk calls and one atomic commit. Verify the final stored and HTTP bytes independently.

Total: **12 calls**, 5,114,362 upload bytes, **1,355 final asset keys**. Preserve all 1,088 historical keys, their properties and untouched encodings, including 594 historical keys absent from this build. 488 build keys are unchanged. Exactly `.ic-assets.json` and its two compression sidecars are ignored for dispatch. Both discovery files and Internet Identity/custom-domain assets are unchanged.

The inspector encoded the stage commit at 477,318 bytes and activation commit at 652 bytes; largest chunk argument is 1,400,137 bytes, below the retained 1,800,000-byte bound. Three local argument-flush/deadline boundary cases pass without a real subprocess or canister call.

## Executable and verification pins

[Execution pins](evidence/vault197-oisy-repayment-artifact-20261118/dispatch/execution-pins.json) SHA256 `0bb50083ef81a3bd209aae0a7ca7753f489facb87797cb52a7f8193bc0a2f7b5` bind all payload and code files. Publisher SHA256 `31fa425ebc05376fb432c8857952a07881df2c740100a1e0b02580ee8302cc9b` differs from prior verified `ec7198ce` only in four exact count literals for this plan; identity, phases, allowed methods, persistent journals, unknown-outcome stop, preflights, deadline and argument-fsync safeguards remain unchanged.

Use the preserved **corrected effective Content-Type verifier** through [run-verification.mjs](evidence/vault197-oisy-repayment-artifact-20261118/verification-header-amendment/run-verification.mjs), with execution pins above and verification-pins SHA256 `c41307c5ba631179e3ad15a3146cfcbb3995e9bcda9ddca9ef7fd9419323c289`. Corrected verifier SHA256 `97dc8911fc0ae5b55f70485ba4230bf5ce71ff98fa42a17bd2741866ba9f5b99` changes only the two expected final-key count literals; the existing explicit-header override correction is retained. Do not use an older verifier that compares only base `content_type`.

Dorian must check authority against these concrete bytes and current live baseline before any stage/activation command. Build allocation and packet preparation do not authorize publication. Stop and read back any unknown outcome before another write; never replay a financial action to test this UI.

No asset deletion/unset, property/permission change, backend install, controller/settings change, client signing or repayment retry, reserve/Vault 202/bot/worker/claim action or liquidation resumption is included. All independent financial holds remain. Source/tests/build prove the scoped candidate; deployed bytes, actual customer consent and successful repayment remain unverified. No new hosted CI or canonical main merge is claimed for this source branch.
