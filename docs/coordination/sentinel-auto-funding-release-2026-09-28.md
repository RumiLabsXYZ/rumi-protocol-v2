# Cycle Sentinel automatic funding release evidence

Date: 2026-09-28
Status: Final source frozen; deployment pending verification.
PR: https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/401
Base: ac59a6519730744dd30c33958b959577dec12123

## Authorization and result

Rob requested implementation with subagents, activation and publication. He explicitly approved the pushed reserve policy in commit 51f4a20e, SHA-256 a984e3c3f2895cc2f67b84ea18306db31febf86fabff61ba4920801fc4a04d0c. That artifact remains unchanged. No deposit from another wallet was authorized.

Deposited Cycles Ledger funds pay first after protected reserves and holds. A fresh proven deficit permits an immutable ICP payment and CMC MINT into the Sentinel/default Cycles Ledger account. The separate durable conversion budget bounds new commitments by the governed global cap; actual exact canister withdrawals retain their own caps and cooldowns. Unknown outcomes retain holds, including across upgrade. Legacy direct top-up operations retain their frozen semantics.

The telemetry page has Deposit cycles and Deposit ICP cards above the registry, receiving-account copy buttons, raw/spendable/protected balances, timestamps, conversion status, scheduled checks and readable target policies. Runtime fuel is separate. The correct legacy ICP destination is a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e, verified through SDK and independent SHA224/CRC32.

## Deterministic evidence

- Frozen full backend lib suite: 607 passed, 0 failed, terminal exit 0, 238.59s; /private/tmp/rumi-sentinel-final-lib.log.
- Final focused review regressions: 6/6 passed, terminal exit 0; /private/tmp/rumi-sentinel-final-review-focused.log.
- Frontend funding tests: 10/10 passed, terminal exit 0; /private/tmp/sentinel-frontend-test-final.log.
- Frontend production build and full asset recipe: terminal exit 0; /private/tmp/sentinel-frontend-build-final3.log and /private/tmp/sentinel-frontend-recipe-build.log. Domain/icon files present in dist.
- Scoped format and whitespace checks passed. Canonical DID regenerated into JS/TS declarations; didc backwards service check against origin/main passed.
- Full frontend typecheck retains 28 baseline errors in 23 unrelated files; telemetry and funding helper have no errors. /private/tmp/sentinel-frontend-check.log.
- Built desktop and 390x844 browser proof: correct destinations, missing balances unavailable, panel fits; /private/tmp/sentinel-pr401-preview-final.png and /private/tmp/sentinel-pr401-preview-mobile.png. Both receiving-address copy buttons independently verified. Old live backend decode errors are expected until backend-first deployment.
- Final production artifact and 15 new real timer/PocketIC flows plus complete integration suite remain pending. Cancelled/superseded builds are not passing evidence.

## Independent review

Two isolated first-round reviewers returned FAIL with concrete issues. Accepted fixes cover:

1. Shared mints must allow floor and partial deficits; old 110% minimum remains only for DirectTopUp. Constructor, custom decode and whole-state validation now agree.
2. Low-runtime self recovery refreshes its source before new admission at the 3,600-second production cadence and rechecks current policy/runtime after the await.
3. A pending ICP debit preserves its pre-payment cache until settlement; a post-payment refresh cannot subtract the same payment twice.
4. New conversion admission uses the actual IC clock after rate lookup; already-paid retry snapshots remain unchanged.

Reports: /private/tmp/sentinel-pr401-review-a1.md and /private/tmp/sentinel-pr401-review-b1.md. Fresh isolated reviews of the repaired output remain pending; no readiness claim is made from announced repairs alone.

## Mainnet preflight and bounded release

Sentinel joh3a-5aaaa-aaaap-quy6a-cai is Running; module 18e39e8ad9eae91be9f52a2e5ad349877f9fc9855d581d2a4c13b1d9b9049626. robvector is a signer/controller. vault_frontend tcfua-yaaaa-aaaap-qrd7q-cai is Running; asset-controller identity is rumi_identity and module 04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62. Controllers/signers/policies are preserved.

All 16 registered targets already have enabled and auto_topup true; threshold 3T / refill 2T / cap 6T per 24h / cooldown 3,600 seconds. Global 40T per 24h, sample interval 3,600 seconds, stale after 7,200 seconds, protected Cycles Ledger floor 10T, ICP floor 0, runtime self threshold/refill 1T and self daily cap 10T. Stopped and uninstalled targets remain ineligible. All 17 governance proposals executed, no unresolved funding operations, both source-ledger balances 0. Read-only snapshots: /private/tmp/sentinel-live-preupgrade.txt and /private/tmp/sentinel-release-prep.txt.

After green source/review/artifact gates, merge PR 401. Build/inspect the production Sentinel Wasm without test endpoints, extract/compare Candid and record hashes. Upgrade only rumi_cycle_sentinel in mainnet-live using robvector, mode upgrade, args '()'. Never reinstall or use the stale init record. Publish vault_frontend assets with explicit icp sync using rumi_identity, without changing its Wasm. Verify deployed hashes, live public fields, configuration, controllers, flags and served browser bundle. Empty source accounts mean automation awaits a user deposit; local proof is not mainnet mint/delivery proof.
