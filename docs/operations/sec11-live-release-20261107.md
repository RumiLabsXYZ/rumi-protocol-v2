# SEC11 XRP deposit release — November 7, 2026

Dorian owns this release under Rob's existing XRP customer-repair deployment/publication authorization. Ellis retains the unified case and held customer message; Miles owns the deposit source. No new financial, customer deposit/withdrawal, signature, refund, processing or reserve/issuance-activation action is included.

## Composition and checks

SEC10 was deployed as upload/module `a7f9af49f6e5139730e025f6be0533ffb0d7afa6d5eaaec7d02a607b998a6834`, artifact source `bd253ddf1ebd233347452603e1e63c6d924820b1`. Its original live-base source was6847982b. The separately reviewed SEC11 delta `ef8a694a4490bf06e3d36086399b9261a61e84c1` was cherry-picked without conflicts atop the deployed SEC10/evidence branch as `e0de879b`; vault.rs and xrc.rs remain byte-identical to the deployed SEC10 port. Main's withdrawal helper/call sites are retained. The broader main-branch XRPL issuance feature is not imported.

The necessary integrated-source checks passed: six SEC10 tests, eight SEC11 tests, canonical self_check Candid compatibility test without regeneration, and canonical backend binding generation with no drift. New Candid is backward-compatible with the deployed API and adds only the reviewed DTO, two owner-authenticated updates and transform query. The seven frontend tests, parsed Svelte components and unchanged32-error project-wide typecheck baseline are source-lane evidence reused for unchanged frontend files; no clean project-wide typecheck is claimed. Ellis and Merritt independently reviewed the exact SEC11 source and returned PASS.

One exact changed-source production backend build and frontend bundle remain pending. Artifact hashes, executable size, actual exports/embedded API, mandatory frontend auth-asset checks and current live inventory/property/auth comparisons must be recorded before mutation. The former SEC10 executor and hashes must not be substituted for this new candidate.

## Upgrade and state preservation

Use the existing controller identity `rumi_identity` on mainnet backend `tfesu-vyaaa-aaaap-qrd7a-cai`. Reconfirm current Running module a7f9af49, controllers/settings and scoped receipt/claim/terms/payout/config baselines. Drain through controller-confirmed Stopped before install; archive the exact oldest managed snapshot completely before replacing only that slot with a fresh stopped-state backup; verify all9 others and download/verify the fresh snapshot before the exact upgrade. Dispatch every management write once with durable markers, hold unknown outcomes, and verify installed module/API/settings and Running before retained-state reads. No uninstall, standalone snapshot delete, restore or stale-state rollback is included.

The new origin/receipt maps decode old state with serde defaults; new private journal records support ordered replay. **After SEC11 receipts/journal records exist, downgrading to an older binary or old snapshot is unsupported.** Preserve new schema and receipts and recover forward. A deployment problem does not authorize restoring older state, discarding dedup receipts, reseeding an empty baseline or retrying a consumed payment.

## Frontend preservation and evidence limits

Publish assets only to existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai`, preserving its module/controllers/settings/authorization, every old key/encoding/property, immutable files, auth origins and SPA behavior. Use the existing staged new-assets then mutable entry-point publication pattern; no clear/delete/unset/property/auth mutation. Fresh inventory and full independent readback must match the planned additive changes. Prevent the known same-call duplicate-chunk-ID issue before commit by separating identical payloads across chunk calls and checking chunk references are unique. Do not replay an uncertain commit.

The resulting UI is intended to expose the original vault address, copy it and verify a user-supplied direct native XRP Payment hash exactly once. A published UI and passing tests are not a real customer deposit, successful withdrawal, paid mint or settlement. The deliberate public Explorer/points projection omission is tracked in [SEC11-DEF-01](../coordination/demo-deferred/sec11-xrp-repeat-deposits.md); it is not a deployment gate for the custody repair.
