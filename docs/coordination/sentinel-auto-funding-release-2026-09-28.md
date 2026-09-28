# Cycle Sentinel automatic funding release evidence

Date: 2026-09-28
Status: LIVE. Source, artifact, tests and two isolated final reviews PASS; mainnet upgrade, asset publication and live readback completed.
PR: https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/401
Base: ac59a6519730744dd30c33958b959577dec12123
Production source: 1ff4225265ea42f8bcabfa819ac1fe5228879658
Current tested head: 9136011632f90d3694d567b15b13462662ea8b9b

## Authorization and resulting behavior

Rob requested implementation with subagents, activation and publication, and explicitly approved the pushed reserve policy in commit 51f4a20e. Its SHA-256 remains a984e3c3f2895cc2f67b84ea18306db31febf86fabff61ba4920801fc4a04d0c. No external-wallet deposit amount was supplied or transferred.

Deposited Cycles Ledger funds pay first after the protected floor, holds and fees. A fresh proven deficit permits an immutable ICP payment and CMC MINT into the Sentinel/default Cycles Ledger account. A separate durable conversion budget bounds new commitments by the governed global cap; exact target withdrawals keep their own caps and cooldowns. Unknown outcomes retain holds across upgrades. Legacy direct top-up operations retain frozen semantics.

The telemetry page adds prominent Deposit cycles and Deposit ICP cards above the registry, receiving-account copy buttons, raw/spendable/protected balances, timestamps, conversion status, scheduled checks and readable target rules. Runtime fuel is separate. The receiving principal/default account is joh3a-5aaaa-aaaap-quy6a-cai. The legacy ICP receiving account is a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e, verified through the SDK and independent SHA224/CRC32 derivation.

## Deterministic proof

- Backend: 608 passed, 0 failed, terminal exit 0, 205.10s, session47715; /private/tmp/rumi-sentinel-round2-final-lib.log. Focused legacy V2 wire and direct-rail headroom regressions also passed, session72109; /private/tmp/rumi-sentinel-round2b-focused.log.
- Final test fixtures built successfully, session59508; /private/tmp/sentinel-flow-fixture-build-round2-final.log. Official PocketIC server7.0.0 matches pinned crate6.0.0; version, official source and hashes are in /private/tmp/sentinel-pocketic7-provenance.json. Test-only commit61fb4fad adds a documented optional managed-server URL; commit91360116 corrects a stale baseline manifest assertion to the existing four signers/threshold2. Neither changes production source or dependencies.
- Full36 real integration cases, including16 new timer scenarios, passed:36passed,0failed,0ignored, terminal exit0,1028.44s, session51505, bounded2 independent test threads; /private/tmp/sentinel-flow-full-server7-two-threads-final.log. Fixture hashes are in /private/tmp/sentinel-flow-final-fixtures.json. The corrected manifest case and separate real-timer protected-floor case also passed. Failed or cancelled earlier attempts are not passing evidence.
- Funding-page tests: 10/10 passed; /private/tmp/sentinel-frontend-test-final.log. Final helper7/7 passed after the freshness display alignment, session82027; /private/tmp/sentinel-frontend-test-final4.log.
- Full production frontend asset recipe passed, session85457; /private/tmp/sentinel-frontend-recipe-debug.log. Domain/icon files are present in dist; no package or lockfile changes were made.
- Whole frontend typecheck retains28 baseline errors in23 unrelated files; telemetry and the funding helper have no errors. /private/tmp/sentinel-frontend-check.log.
- Built desktop/mobile layout and both receiving-account copy buttons were verified; /private/tmp/sentinel-pr401-preview-final.png and /private/tmp/sentinel-pr401-preview-mobile-final.png. Preview decode errors against the old live backend are expected until the backend-first upgrade.
- Scoped format and whitespace checks passed. Canonical Candid and generated JS/TS bindings agree; old-service compatibility passed.
- Production build session32670 exited0; /private/tmp/sentinel-production-build-round2.log. Raw Wasm SHA256 c19ea76d19abf583524d61a69143988ba762c5887d3df61bbd1c106ddb093e2f, 2,923,342bytes; executable code section2,381,832bytes. Install gzip SHA256 df085a633822aa84a611f9cdc9065c5422230c7fff920eaefe06dadea07f04fe, 731,870bytes, exact raw roundtrip. Strict extracted Candid equality both directions, backwards service compatibility and endpoint checker exited0; only five explicit CDK/lifecycle exports are hidden, with no test controls. /private/tmp/sentinel-production-artifact-final.json. Production is unchanged after the two test-only commits.

## Independent review

Two isolated first-round reviews identified shared floor/partial-deficit validation, low-runtime source refresh, pending-ICP cache double accounting and stale-clock admission issues. Two isolated second-round reviews identified a nested V2 wire migration mismatch, post-await clock handling and recovery delivery after external runtime restoration. All accepted findings were repaired and covered by focused regressions and real-timer cases. The frozen V2 nested wire explicitly migrates to DirectTopUp. New admission and settlement use actual post-await clocks; immutable paid retries remain unchanged. A mint completed after external runtime restoration retains reserve and opens no synthetic runtime withdrawal/history.

Both fresh isolated third-round reviewers returned PASS after independently inspecting the complete36-case result, final artifact identity, source, approved policy and test-only harness changes. No confirmed blocking finding remains. Readable final reports are docs/coordination/sentinel-auto-funding-review-a3-2026-09-28.md and docs/coordination/sentinel-auto-funding-review-b3-2026-09-28.md. A later-target FutureCache can defer additional targets to the next scheduled pass after a mint; this fails closed and is optional throughput backlog, not a release gate. Fixed raw-cache age display under future policy changes and comment cleanup are also deferred optional notes.

## Mainnet preflight and release scope

Read-only snapshot: /private/tmp/sentinel-live-final-preflight/snapshot.json. Fresh raw status proof supersedes a controller transcription typo in that snapshot: /private/tmp/sentinel-immediate-before-status.txt, /private/tmp/sentinel-frontend-immediate-before-status.txt and /private/tmp/sentinel-release-before-authoritative.json. Governance, public targets and unresolved operations were refreshed successfully into /private/tmp/sentinel-immediate-before-proposals.json, targets.json and unresolved.json. Sentinel joh3a-5aaaa-aaaap-quy6a-cai is Running, old module18e39e8ad9eae91be9f52a2e5ad349877f9fc9855d581d2a4c13b1d9b9049626; robvector is signer/controller. Frontend tcfua-yaaaa-aaaap-qrd7q-cai is Running, module04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62; rumi_identity is asset controller. Preserve the exact four Sentinel and five frontend controllers, all signers and policies.

All16 registered targets already have enabled and auto_topup true; threshold3T, refill2T, cap6T per24h, cooldown3,600seconds. Global40T per24h, sample3,600seconds, stale7,200seconds, protected Cycles Ledger floor10T, ICP floor0, runtime self threshold/refill1T and self cap10T. Timelocks remain unchanged. Stopped and uninstalled targets remain ineligible. All17 proposals were executed, no funding operations were unresolved, and both source-ledger balances were0 at preflight and at the fresh direct ledger queries before publication (/private/tmp/sentinel-immediate-before-cycles-ledger.json and /private/tmp/sentinel-immediate-before-icp-ledger.json).

After final green verification, merge the exact reviewed head of PR401, then upgrade only rumi_cycle_sentinel in mainnet-live using robvector and the verified gzip. Use zero-argument post_upgrade, never reinstall or stale InitArgs. Publish vault_frontend assets only using icp sync and rumi_identity, preserving its Wasm. Verify deployed hash, new public fields, policy/controller preservation and actual served browser UI. Empty source accounts mean active automation awaits a user deposit; local simulations are not mainnet mint/delivery proof.

## Completed mainnet publication

PR401 merged at4f6675854233a6c4344d0291558d3fcddb9421d5. Merged production source matches the verified1ff42252 artifact byte for byte. The exact gzip upgrade used robvector, mainnet-live, mode upgrade and empty Candid bytes4449444c0000, without reinstalling. Install session53647 exited0; /private/tmp/sentinel-mainnet-install-2026-09-28.log records success. Authoritative status is Running with deployed module SHA256 df085a633822aa84a611f9cdc9065c5422230c7fff920eaefe06dadea07f04fe, matching the installed gzip. All four Sentinel controllers were preserved.

Frontend assets-only sync session8071 exited0 and published645assets; /private/tmp/sentinel-mainnet-assets-sync-2026-09-28.log. The frontend Wasm remains04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62 and all five controllers are unchanged.

Post-upgrade queries decode the new funding-wallet fields successfully. All16 targets remain enabled with auto-top-up on and the existing3T trigger/2T refill/6T daily cap/3,600second cooldown. All17 governance proposal records are preserved byte for byte, the current signer is authorized, and no unresolved funding operation is returned. Direct post-upgrade Cycles Ledger and ICP Ledger reads confirm0 in the shared receiving accounts. No external-wallet deposit, mainnet mint or canister delivery is claimed. Activation is complete and awaits a user deposit; checks follow the existing hourly policy.

The served https://app.rumiprotocol.com/explorer/telemetry page was verified against the upgraded backend. Deposit cycles and Deposit ICP cards display the correct receiving accounts,10T protected reserve, honest cached balances/freshness, cycles-first behavior and all16 readable automation rules. Both unique receiving-address copy buttons were independently tested and the previous clipboard restored. Screenshot: /private/tmp/sentinel-pr401-live-deposit-panel.png. The initially stale ICP cache correctly displays unavailable spendability and Unknown conversion status; public refresh does not run maintenance, while direct ledger reads confirm the empty account.

Machine-readable local proof: /private/tmp/sentinel-mainnet-release-proof.json. Root verified merge, install, authoritative module identity, preserved state, assets-only publication and served-browser behavior as separate evidence states. The temporary test server was stopped cleanly after all47 owned instances were deleted; shared caches and proof logs remain available.
