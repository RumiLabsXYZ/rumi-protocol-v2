# Cycle Sentinel cadence release: live proof, 2026-09-28

## Published behavior

[PR406](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/406) merged at 2026-09-28T18:46:42Z as `1b525338cb7b391f406f84d226a13155bc781ab6`. The merged head was `9a86d2aaceeaf4963a8c490f26c458062ae53c33`; executable source and frontend build inputs had zero diff against tested source `77f8445000342e13fcb0978f78d006464d8cac94` before publication. Later source commits contained review evidence only.

The deployed page shows the authoritative automatic interval, public Refresh telemetry and clear cycles/ICP receiving cards. Configured signers can invoke Run check now through the new `run_maintenance_now` update. It uses the same maintenance and single-flight path as the automatic timer, preserves the armed deadline, and follows existing self-recovery and target reserves, caps, cooldowns and thresholds. Public refresh reads saved results and cannot initiate funding.

## Deterministic and independent checks

- Native backend suite: 615 passed, 0 failed, exit 0 (session71466).
- Final consent tests after the wording repair: 5 passed, 0 failed, exit 0 (session97685).
- Focused final frontend suite: 24 passed, 0 failed, exit 0 (session22760).
- Final full real-canister suite: 40 passed, 0 failed, exit 0, 1,288.85 seconds (session42930). All four new regressions and the existing funding, recovery, lost-reply, refund and upgrade cases passed.
- Backend and frontend production builds: exit 0 (sessions68306 and57034). Production Candid matched in both directions, preserved the prior service, and matched production endpoints with only the known lifecycle exports exempted. No test controls were exported.
- Review round one found an omitted self-recovery effect in wallet consent. That confirmed finding was repaired in both the warning and wallet message. Two fresh reviewers returned PASS in round two and separately closed the final 40-case result. Their committed reports are [A2](sentinel-cadence-review-a2-2026-09-28.md) and [B2](sentinel-cadence-review-b2-2026-09-28.md).

Whole-frontend typecheck still reports the pre-existing 28 errors across23 files, with no diagnostics in changed Sentinel telemetry/service/helper files. The production frontend build passed. There are no configured GitHub status checks; this record does not claim a remote CI pass.

Final integration fixture SHA-256: `bc15f76e37ce562f7f757fe03539a4f83979f704a058a96e62f0dbb02e2ef374`. Full integration log SHA-256: `19dc6c626d9525a00f1a1a017619293afcdb772108480b0749ebe578b9a82876`. Machine-readable local evidence is `/private/tmp/sentinel-cadence-final-evidence.json`.

## Mainnet publication receipts

Sentinel `joh3a-5aaaa-aaaap-quy6a-cai` was upgraded using the existing authorized identity and empty upgrade arguments. Install session68030 exited0 and returned installed successfully. The authoritative running module hash matches the deterministic production gzip exactly:

`6f94d733548f65e1ec5e46309b2aa52ffd8de35ad68c544d625984927f57143a`

The raw Wasm SHA-256 is `20241bf7f2144226f5601301dfc7c837f6c9ca89da058f6e0571437953507102`. All four existing Sentinel controllers were preserved. Receipts: `/private/tmp/sentinel-cadence-install.log` and `/private/tmp/sentinel-cadence-postinstall-sentinel-status.txt`.

Frontend `tcfua-yaaaa-aaaap-qrd7q-cai` received an assets-only sync under its existing authorized identity: session28306 exited0, with648 assets synced. The running frontend module remains `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`; all five controllers were preserved. The public domain served version `1790619280811`, exactly matching the final build. Receipts: `/private/tmp/sentinel-cadence-assets-sync.log`, `/private/tmp/sentinel-cadence-live-frontend-status.txt` and `/private/tmp/sentinel-cadence-live-served-version.json`.

No other canister, controller, oracle or target policy was changed in this publication.

## Authenticated immediate-check proof

Signer-authenticated mainnet `run_maintenance_now` returned Ok with exit0 (session44084). The paired authoritative overview readings show:

| Field | Before | After |
| --- | --- | --- |
| next_sample_at_secs | 1790624901 | 1790624901 |
| last_sample_at_secs | 1790621086 | 1790621421 |
| sample_interval_secs | 3600 | 3600 |
| stale_after_secs | 7200 | 7200 |

Observations advanced while the armed automatic deadline stayed unchanged. All16 static target-policy projections were identical before and after; governance and unresolved-operation response bytes were identical. Proposal17 remained Open with one approval. Unresolved operations and target top-up histories remained empty. Receiving balances were0 cycles and0 ICP, so there was no target refill or funding transfer; normal runtime execution still consumed cycles. The machine-readable assertion record is `/private/tmp/sentinel-cadence-live-verification.json`.

## Four-hour activation remains pending

Policy proposal17 changes only sample interval3600 to14400 and stale window7200 to28800. It preserves the40T global daily cap,10T protected reserve, existing self-recovery policy, all target settings, the two-of-four signer threshold and timelocks. Its pushed argument SHA-256 is `b2ebe76efacabaa8b2cf3ab9a7580e3e8fab96bd31a32d03eafad29044661441`.

It has one distinct signer approval and needs another configured signer. The existing24-hour waiting period ends **2026-09-29T17:55:57Z, September29 10:55:57a.m. Pacific**. Execution after the waiting period and required approvals is a separate action. The schedule is still hourly; neither publication nor the date alone activates four-hour checks. No governance requirement was bypassed.

## Visible page and remaining wallet proof

The live Chrome page at [Cycle Sentinel telemetry](https://app.rumiprotocol.com/explorer/telemetry) visibly shows Every1hour, Refresh telemetry, correct receiving addresses and Copy buttons,0T/0ICP receiving balances and the10T protected reserve. Native screenshots are saved locally at `/private/tmp/sentinel-cadence-live-controls.png` and `/private/tmp/sentinel-cadence-live-deposit-panel.png`; they are not committed because the browser also displays the user's financial overview.

Cycles-ledger deposits use Sentinel owner `joh3a-5aaaa-aaaap-quy6a-cai`, default subaccount, on ledger `um5iw-rqaaa-aaaaq-qaaba-cai`. ICP deposits use the same owner/default subaccount on ledger `ryjl3-tyaaa-aaaaa-aaaba-cai`, or its legacy account identifier `a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e`. Runtime-cycle transfers directly to a canister do not fund the cycles-ledger pool. Receiving deposits remain necessary for funding.

The wallet's read-only operator-access check opened OISY's sign-in page: OISY was signed out. The user has been asked to sign in with their usual account. Therefore authenticated browser visibility of Run check now remains unproven; source gating, backend mainnet execution and public page publication are verified separately. No browser financial action was submitted. The browser extension also requested an update; native app inspection supplied the public page evidence without changing extension settings.

## Owned resource cleanup

All51 owned PocketIC instances were Deleted, and the owned official version7 server exited0; its process, direct children and port were absent afterward. The owned frontend preview server was stopped and its preview tab closed. Test artifacts and caches were retained. The initial disk-pressure worktree cleanup removed0 worktrees: none met eligibility requirements; dirty, unmerged or otherwise protected worktrees were preserved. The managed release worktree remains available for the pending signer/cadence proof.
