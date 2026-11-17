# Recovery frontend artifact and bounded publication handoff

Owner: Ellis, sole frontend artifact producer and independent verifier. Dorian remains the sole publisher. This carries the existing approved SEC20/SEC21 restoration to production; it adds no financial or permission action.

## Source, checks and artifact

PR #671 merged into canonical main at `0edaf018bbc2b9c5ab648c9244001437e3af5ea4`. Its tree equals the exact tested head `f12337079e7b0ee420920e9210fa97599a65cff8`; the single Frontend validation run `43982949272`, attempt 1, and job `133703764015` completed successfully. The existing 39 focused local tests, two combined source reviews and connected mocked desktop/mobile checks remain valid. No source campaign was repeated.

Build source `434d51fa39a4467138fc14e2991979eb26b7300c` is an evidence-only successor of that merge. Frontend tree `996afcb56c07c39bb9707a3f701811f0e408288c` and scripts tree `5f5bd567dedc872a0ee32a514c74805813b0ced1` exactly match canonical source. One admitted warm-dependency production build passed under allocation `738d87fe-67d8-4c0b-a056-b8e2950eb310`: exit 0, 19.377 seconds, peak owned RSS 2,257,960,960 bytes, maximum measured physical growth 103,333,888 bytes. Source and dependency pins were unchanged; PG 58855 was independently verified empty. No installation, repeated tests, Rust build, extra producer or publication occurred. Node 24.13.1/npm 11.8.0 were pinned for this local artifact; CI used its canonical Node 22 job. No claim of byte-identical cross-toolchain builds is made.

- [Frozen artifact archive](evidence/recovery-frontend-publication-20261116/frontend-artifact.tar.gz): 8,197,894 bytes; SHA256 `a765d0410b30fec71f63883635550dcd306cc791a3e8e5348a4cb95c31102780`.
- [Complete file manifest](evidence/recovery-frontend-publication-20261116/artifact-manifest.json): all 764 files, 11,318,429 bytes; manifest SHA256 `82768e536040d1a45b77215f3bd48ae1290945e80a44f3d5968735c97a319e82`.
- [Terminal build receipt](evidence/recovery-frontend-publication-20261116/receipt.json), [independent closure](evidence/recovery-frontend-publication-20261116/independent-closure.json), [redacted build log](evidence/recovery-frontend-publication-20261116/build.redacted.log) and [evidence index](evidence/recovery-frontend-publication-20261116/evidence-index.json) retain actual results and hashes.

Auth/domain verification passed. Both discovery identity files exactly match live batch 704. The recipe's favicon copy is included. Built version is `1794874100541`; index SHA256 `eca66896f782580ef0b515604eeabb6645fc600bbcaec5f6e52e9ce1dbc7b5b0`.

The built root `/_app/immutable/nodes/0.DDqiCHqT.js`, SHA256 `21f0fe594231aeb9dc538d70b563aca9c02223c235420e815caf0e85e9afcfcb`, omits the generic recovery prompt/button and contains Wallet → Recover a vault action. Queue bundle `/_app/immutable/nodes/49.DoMq6N1O.js`, SHA256 `9a27bd0441a01ef18fe3a472f68b3ccc27a2968c396c160026810e96c160f872`, includes the reserved-row message and existing public claim query. The PR #666 recovery script and session protections remain unchanged.

## Focused artifact reviews

Both independent artifact/preservation reviewers returned **PASS with no blockers** against corrected packet commit `cfe7d35b6170719e2df92c6f093f86f3a8fdb226`. They independently checked the complete archive/manifest, all 764 files and byte counts, source-tree equality, corrected 327/6/428/327 classification, excluded configuration sidecars, discovery/auth bytes, root/menu/queue markers and evidence hashes. [Review results](evidence/recovery-frontend-publication-20261116/artifact-reviews.json) retain the scope and remaining publisher work. This passes artifact readiness; the exact dispatch/encoding manifest and fresh baseline remain required before writing. In particular, include changed `/index.html` gzip content in the atomic activation, alongside the six explicit entry assets. No further build or source-test campaign is requested.

## Existing authority and exact publication scope

Merritt's original human messages `01a1db85-3658-7c01-8637-73e787fa6eb4` and `01a1dd32-aece-7ee0-b545-2ddbc605e891` authorize the already approved recovery-menu and queue restoration after verification. Cody independently read those original user instructions. No duplicate unchanged-scope approval is requested. Resource allocations only admitted validation/build and did not grant publication or financial authority.

Target only the existing frontend asset canister `tcfua-yaaaa-aaaap-qrd7q-cai`, environment `mainnet-live`, existing authorized identity `rumi_identity`. No module install, controller/settings change, permission change, backend/bot state-changing call, customer wallet signing, reserve change, recovery transaction or Stability Pool resumption is included.

[The preservation plan](evidence/recovery-frontend-publication-20261116/artifact-preservation-plan.json) compares the complete artifact against the November 16 stable double-read baseline of 761 assets. Excluding upload configuration, there are 327 new immutable keys, six changed existing entry keys, 428 unchanged build keys and 327 old keys absent from this build. **Retain all 327 old keys; delete no asset or encoding.** An additive publication yields 1,088 asset keys. The complete build archive contains `.ic-assets.json` and its generated `.br`/`.gz` sidecars; these three upload-configuration files are not public asset keys and must be excluded from dispatch. Preserve every old property's value and each untouched encoding's bytes, hash, size and modification timestamp. Keep both discovery files and the existing II/domain files byte-identical.

Dorian must freeze the exact asset dispatch/encoding manifest against a fresh stable inventory, settings, permissions and certified frontend/backend module/controller/API baseline immediately before writing. Stage only new immutable resources, verify their served identity bytes and any uploaded encodings, then atomically activate the six existing entry keys: `/index.html`, `/_app/version.json`, and each one's explicit `.br` and `.gz` assets. Include required changed encodings in the same activation batch; preserve properties and untouched encodings. Do not use a bare sync that removes historical keys. Unknown publication outcomes require readback before another write.

## Completion evidence still outstanding

This is artifact readiness, not proof of publication or a customer transaction. Before marking the UI card done, independently verify final full inventory and all planned encoding hashes/properties, unchanged modules/settings/controllers/permissions, active primary-domain version/index/root/queue bytes, both discovery/auth files, and actual desktop/mobile route layout. Retain the distinction between connected mocked flow checks and real wallet use. No customer allowance, collateral deposit or financial canary is part of this release. All unrelated security classifications and financial holds remain unchanged.
