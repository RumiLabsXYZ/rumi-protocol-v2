# SEC19 collateral top-up release decision — HELD

Proposed single approval: Dorian may coordinate one bounded frontend build and paired backend/frontend production release of exact source **4d8322a71059f146f6ec44c95add49789be2b6f8**, branch `codex/add-margin-recovery-20261113`, using the sequence below. Ellis independently verifies artifacts and live readback. Previous SEC18 approval6e3fdaa7 covered the already-live progress update; it is not reused for this backend change.

## Behavior and accepted checks

Restore receipt-bound top-ups to existing ICRC vaults, including ckSOL. A confirmed exact pull credits the matching surviving vault once or becomes a separate owner/ledger/amount-bound held liability if the target is gone, changed or protocol-controlled. Ordered replay, deduplication and explicit receipt ACK preserve that liability. No automatic refund is introduced. Unknown outcomes remain held for reconciliation. Legacy add-margin stays disabled.

The matching frontend opens OISY in the user's click, preserves owner/session, converts using verified token decimals, and distinguishes credit from recovery. Missing/invalid decimals reject before approval. No eight-decimal fallback remains. A recovery notice can be explicitly acknowledged without paying again or erasing the held liability. All four live SEC18 files remain byte-identical to ab44413d.

[Source verification result](add-margin-source-result-20261113.md): six new backend units, one old-journal regression, two outcome tests, canonical generation/bindings and both compatibility checks passed. Three PocketIC lifecycle tests and134 frontend tests passed. Two independent final reviewers accepted exact4d8322a7 after the ACK and metadata corrections. The zero-debt closure test exercises the shared removed-target branch; liquidation ordering is source-verified, not actually executed. No live wallet or customer transaction acceptance is claimed.

## Existing verified backend artifact

Reuse the completed source-matched production-profile metadata/shrink/O1 artifact. Do not rebuild backend or repeat accepted tests. Runtime Rust/Cargo/cycle-manager/flaky-ledger bytes in4d8322a7 are identical to builte2a0122a. Later commits change only fixture funding, canonical declarations and frontend source.

[Artifact pins](evidence/add-margin-release-20261113/artifact-pins.json):

- Optimized Wasm SHA256 **2961d2d84008e43756c25c3361b59717676ea92d68eb82951648dc5a4703ea08**,9,894,334bytes.
- Deterministic `gzip -9 -n` SHA256 **8f6f5933e54fa44205760b9a8d1c3869135df43825eadfee70b20c62d0a0bd16**,2,405,092bytes. Independent decompression equals the optimized bytes. This gzip is the install payload and expected installed module hash under the same accepted executor convention as SEC17; do not confuse it with the uncompressed artifact hash.
- Embedded canonical Candid SHA256 **d8b0142e705e860640557b7ea6253a2a6027d7e96f3b15cfee9ba51fff0d2c97**, byte-equal to committed canonical Candid.
- Executable-code section8,957,230bytes, below the preserved10,223,616-byte project guard. This is not a claim about the network limit.
- All397 declared service methods match query/update exports. All407 export names/order match the prior released artifact, including the unchanged internal timer export.

Artifacts are preserved in `/private/tmp/rumi-sec19-release-20261113`, alongside all previous frozen releases. No binary is exported to the repository.

## One fresh frontend build and mechanical outputs

Approval explicitly permits Dorian chat01a1a800-7fde-71b2-a128-2b2325c4b232 and Cody chat01a0eeee-d768-71b1-8090-0ca4e680e5b3 to message **in both directions** for this one lifecycle: exact source/runner/capacity request; fresh allocation path/UUID/hash/deadline; own start/native/result/artifact/resource/closure receipts. It does not permit unrelated private usage payloads or paid/API fallback. Ellis's existing independent verification and CTO/release updates continue.

Cody must independently verify current capacity and issue a fresh exclusive allocation, with a start deadline within10minutes. No consumed source-validation/SEC17/SEC18 grant is reused. Approval is not a resource grant.

1. Reuse Dorian's existing producer `.worktrees/xrp-customer-withdrawal-address-20261106`. Verify tracked-clean exactab44413d, its current740files against frozen SEC18 manifests and all retained dependency links. Preserve the producer/frozen output and prior records, then detach only this producer to exact4d8322a7. Do not reset the dirty primary checkout or other owners' worktrees; no cleanup.
2. Run once `python3 -I build-once.py {freshAllocationPath} {freshAllocationSha256}`. [Readable runner](evidence/add-margin-release-20261113/build-once.py) SHA256 **074eb4c7b9e919b62cfe18e72088ca58a8cbffbe1a01c20505b6aca89d3c47e5**. Runtime namespace `/private/tmp/rumi-sec19-release-20261113`. It runs only `npm run build --workspace=src/vault_frontend`, with installed offline dependencies and mandatory auth/domain postbuild. No backend build, dependency installation or repeated review/test campaign.
3. Total15minutes includes60seconds owned closure; Node heap6GiB, aggregate ownedRSS8GiB, filesystem free-space-loss guard8GiB,20GiB free-space floor. Capture exclusive dispatch evidence, native outcome and all owned closure. Failure, uncertainty or a bound stops without automatic rebuild.
4. Freeze and push actual full frontend manifest/version, additive plan, publisher/readback scripts and execution pins before any production mutation. Verify every file/compression hash, accepted source inclusion, unchanged auth/domain bytes, current backend artifact and interface/export/gzip checks. Ellis independently verifies these frozen outputs.

The version, resulting hashes/new immutable-key counts and returned batch IDs are mechanical outputs of this exact source and pipeline. This single approval covers their verified publication below; producing the pins does not request another approval. Changed source, expanded mutation scope or failed essential checks stop the release.

## Target and fresh preservation preflight

Use only networkic, identityrumi_identity, backend **tfesu-vyaaa-aaaap-qrd7a-cai** and existing frontend **tcfua-yaaaa-aaaap-qrd7q-cai**. No whole-stack deployment, new canister, reinstall, permission/controller/settings/mode change or reserve activation.

Last independently verified baseline: backend installed gzip module **6428d1039a9f54e691acfb8cd6c8d5c4cdba8d675b7350f3ad5913815d5575f7**, Candidf0b0fcc2480788b93b4707f707e7dbc8bf9b77c39df0a4ddf3594f54bdf44b6d; frontend module04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62, version1794583637373,5220keys/5414encodings. These are dated expectations, not a fresh read. Reconcile any newer release before writes.

Freshly verify exact source/output/script pins, identity, both modules/API/settings/controllers, full stable frontend inventory and Prepare/Commit lists. Capture vault/debt/interest/config/custody/claim state, balances, all pending payouts and liabilities, SEC11/SEC12 receipts/watermarks/holds, repayment state, Vault51 automatic state and existing reserve settings. Do not clear saved customer requests. Read-only captures must not invoke a financial continuation.

## Paired production sequence

1. Reuse the accepted additive publisher. Stage only new content-addressed `/_app/immutable/` keys using existing property templates. Existing-key changes are limited to the seven existing index/version content sets and their encodings. Preserve every old key/property/untouched encoding and auth/domain asset. No deletes, clear, unset or property edits. Verify the complete staged inventory and new served bytes before backend mutation; leave the current entry point active.
2. Reuse the accepted [SEC11 stopped-state preservation procedure](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L15) and SEC17 executor pattern. Stop/drain and controller-confirm Stopped. Fully archive and verify the oldest managed snapshot before replacing only that slot with a fresh stopped-state backup. Preserve all other9snapshot entries and download/verify the fresh backup. No standalone snapshot deletion or restoration. Compare the actual active state tree and reconcile evidenced interest/timer progression.
3. Install only the frozen gzip once in **upgrade** mode using [exact args](evidence/add-margin-release-20261113/upgrade-args.did), SHA256 **522cf980fc7d90c91859c051f2ce726107d1a02adc1cb35ef4c083e9dc6de824**. `mode=null` preserves current mode. Preserve settings/controllers. While stopped, verify installed module **8f6f5933e54fa44205760b9a8d1c3869135df43825eadfee70b20c62d0a0bd16**, canonical APId8b0142e and exact settings. Start once, then reconcile stopped-baseline state and every protected liability/receipt/hold; allow only independently evidenced natural interest/timer changes.
4. Activate only the seven prepared frontend entry content sets after the new backend is verified. This keeps the enabled UI paired with its recovery-capable backend. Verify complete stable final inventory, actual served bytes, primary domain/version/auth, applicable SPA fallbacks, source inclusion and desktop/mobile top-up/recovery/ckSOL/repayment/OISY layout. Ellis independently checks public bytes/inventory, certified modules/controllers/API and applicable actual browser composition. Authenticated synthetic cases and emulator tests remain distinct from a real customer transaction.
5. Publish one final source/build/backend/frontend/preservation/readback receipt with actual IDs, hashes and native outcomes. Report software delivery separately from any later user-authorized vault213 top-up or customer acceptance.

Every management/asset write needs exclusive durable dispatch evidence and known returned IDs. Unknown outcome stops for readback, never a write retry or alternate route. Retain the prior source/artifacts and recover forward; new private settlement events must not be discarded by an old-writer rollback.

## Preserved automatic behavior and financial boundary

Preserve the already-approved Vault51 request1696/burn3435761 automatic checks and same-vault adoption preference, existing repayment policy, all prior journals/claims/payouts and CKICP reserve-off settings. Read their current records; do not assume they stayed unchanged. This release adds no manual sweep, new request, reserve funding, price/fee policy, forgiveness action or payout preference.

No customer/Rob approval, transfer, mint, repayment, withdrawal, close, cancellation, saved-state clearing, swap, financial retry or live canary is authorized. Enabling the recovered top-up route permits future separate user-authorized deposits; this release does not add Rob's13.01909ckSOL to vault213 on his behalf. Held recovery is recorded, not a promise of an automatically delivered refund. Actual financial acceptance remains unverified.

Until Rob approves this pushed decision, the proposed frontend production build and all installation/publication steps remain held. Source validation, artifact packaging and proposal preparation are complete under the existing repair instruction. This is the single central release request, without an optional global audit, new harness or repeated source campaign.
