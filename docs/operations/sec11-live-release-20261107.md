# SEC11 XRP deposit release — November 7, 2026

Dorian owns this release under Rob's existing XRP customer-repair deployment/publication authorization. Ellis retains the unified case and held customer message; Miles owns the deposit source. No new financial, customer deposit/withdrawal, signature, refund, processing or reserve/issuance-activation action is included.

## Composition and checks

SEC10 was deployed as upload/module `a7f9af49f6e5139730e025f6be0533ffb0d7afa6d5eaaec7d02a607b998a6834`, artifact source `bd253ddf1ebd233347452603e1e63c6d924820b1`. Its original live-base source was6847982b. The separately reviewed SEC11 delta `ef8a694a4490bf06e3d36086399b9261a61e84c1` was cherry-picked without conflicts atop the deployed SEC10/evidence branch as `e0de879b`; vault.rs and xrc.rs remain byte-identical to the deployed SEC10 port. Main's withdrawal helper/call sites are retained. The broader main-branch XRPL issuance feature is not imported.

The necessary integrated-source checks passed: six SEC10 tests, eight SEC11 tests, canonical self_check Candid compatibility test without regeneration, and canonical backend binding generation with no drift. New Candid is backward-compatible with the deployed API and adds only the reviewed DTO, two owner-authenticated updates and transform query. The seven frontend tests, parsed Svelte components and unchanged32-error project-wide typecheck baseline are source-lane evidence reused for unchanged frontend files; no clean project-wide typecheck is claimed. Ellis and Merritt independently reviewed the exact SEC11 source and returned PASS.

One exact changed-source production backend build and frontend bundle have passed. Artifact hashes, executable size, actual exports/embedded API, mandatory frontend auth-asset checks and current live inventory/property/auth comparisons must be recorded before mutation. The former SEC10 executor and hashes must not be substituted for this new candidate.

## Upgrade and state preservation

Use the existing controller identity `rumi_identity` on mainnet backend `tfesu-vyaaa-aaaap-qrd7a-cai`. Reconfirm current Running module a7f9af49, controllers/settings and scoped receipt/claim/terms/payout/config baselines. Drain through controller-confirmed Stopped before install; archive the exact oldest managed snapshot completely before replacing only that slot with a fresh stopped-state backup; verify all9 others and download/verify the fresh snapshot before the exact upgrade. Dispatch every management write once with durable markers, hold unknown outcomes, and verify installed module/API/settings and Running before retained-state reads. No uninstall, standalone snapshot delete, restore or stale-state rollback is included.

The new origin/receipt maps decode old state with serde defaults; new private journal records support ordered replay. **After SEC11 receipts/journal records exist, downgrading to an older binary or old snapshot is unsupported.** Preserve new schema and receipts and recover forward. A deployment problem does not authorize restoring older state, discarding dedup receipts, reseeding an empty baseline or retrying a consumed payment.

## Frontend preservation and evidence limits

Publish assets only to existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai`, preserving its module/controllers/settings/authorization, every old key/encoding/property, immutable files, auth origins and SPA behavior. Use the existing staged new-assets then mutable entry-point publication pattern; no clear/delete/unset/property/auth mutation. Fresh inventory and full independent readback must match the planned additive changes. Prevent the known same-call duplicate-chunk-ID issue before commit by separating identical payloads across chunk calls and checking chunk references are unique. Do not replay an uncertain commit.

The resulting UI is intended to expose the original vault address, copy it and verify a user-supplied direct native XRP Payment hash exactly once. A published UI and passing tests are not a real customer deposit, successful withdrawal, paid mint or settlement. The deliberate public Explorer/points projection omission is tracked in [SEC11-DEF-01](../coordination/demo-deferred/sec11-xrp-repeat-deposits.md); it is not a deployment gate for the custody repair.

## Frozen artifact and publication packet

Build source **`0eff84c93742ef6679a2ef09b1bd72e34b511665`**, integrated code e0de879b plus release documentation only. The one changed-source Cargo production build passed in58.30seconds; canonical metadata/shrink/O1/gzip packaging passed. Frontend bundle and mandatory auth-asset postbuild passed in15.83seconds. No package installation or dependency change was needed.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Raw backend Wasm | 14,049,429 | e2980a2693bb548ad839b5a88650b5f75ca60ee3631b0a3aefd3085f6dc3d089 |
| Optimized Wasm | 9,753,571 | a79d8f5d54482c52591ecd83b59a1051341beefb835ce0850a7f5408a7cd0232 |
| Uploaded deterministic gzip | 2,365,879 | b81583ae81421b88d3bd64bf27976b338b2b09748e9dd6b863849b40ac6fd736 |

Executable code section8,826,325bytes is below unchanged projectguard10,223,616bytes. Independent actual-artifact inspection confirmed all hashes/sizes, gzip decompression to optimized bytes, embedded canonical Candid and all395 application exports; no missing/unexpected application export, only the already-existing CDK timer executor. Generated API is backward-compatible with the392-method deployed interface. The compiled UI contains both deposit methods and controls and is wired to the actual mainnet backend/host; this is compiled-byte proof, not a real customer action.

Fresh stable double-read frontend inventory: **3,078 existing keys /3,272 encodings**, all retained. Exact additive plan: **306 new keys**, **6 existing mutable index/version identities**, and **1 existing gzip encoding**, totaling313 content sets and4,989,710bytes. Expected final keycount3,384. No existing immutable key is changed;425 identities match;2,647 old nonlocal keys are preserved. Zero delete/clear/unset/property/auth mutation and zero property-template ambiguity.57 compressed-only re-encodings are left unchanged because their identity bytes are unchanged. Auth-origin/domain build assets pass.

The current frontend is Running on unchanged module04e565b3; rumicontroller and existing upload permission lists were read without granting access. The current backend is Running on exacta7f9af49. Oldest managed backend snapshot **`000000000000000c0000000001f088fe0101`** has been fully archived read-only:88,035,876 named bytes, native metadata/lengths/hashes/globals verified, empty declared chunk store. Before backend install, replace only that archived slot with a fresh Stopped-state backup and preserve all9 other entries; verify/download the fresh backup.

Execution order after exact-packet review: fresh frontend inventory/module/permission preflight; stage only306 new asset keys and independently verify served bytes with allold assets unchanged; refresh private backend baselines; execute the pinned once-only stop/Stopped/backup/install/restart procedure; confirm retained state and holds; then activate only the7 approved mutable content sets and verify complete inventory, served changed bytes, primary domain and authorization. Identical chunk payloads are split across calls and every returned reference must be unique before commit. Unknown management or commit outcomes halt without replay.

The exact [backend pins/procedure](evidence/sec11-live-release-20261107/execution-pins.json), [frontend pins/procedure](evidence/sec11-live-release-20261107/publication-pins.json), full manifest/comparison, compact complete live inventory and backup verification are pushed under this evidence directory. Binary and private state payloads remain local. No SEC11 live mutation or customer action is claimed yet.
