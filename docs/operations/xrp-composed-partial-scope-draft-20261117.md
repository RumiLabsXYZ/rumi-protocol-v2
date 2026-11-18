# Composed XRP recovery sizing and retry-bound deployment scope — draft

**NOT READY FOR DEPLOYMENT APPROVAL.** The composed source is pushed and focused source review passes; composed native tests and the one source-matched backend build have not run. No artifact/hash is approved or proposed as installed. This draft makes the requested scope reviewable while exact permissions are pending.

Rob's original message 01a1ecfc-abdf-7af2-885e-82daa8c4cf89 authorizes Adrian and Dorian to combine the partial-liquidation fix with the live cycle-waste correction, verify it, and bring an exact deployment scope for approval. It does not authorize production installation or resumption. Dorian owns composition/build/release; Adrian owns sizing/claim review.

## Frozen runtime source and retained behavior

Source 1165a8973cb2040c732753a8bf24e7825ff639c7, tree 7e82248d0fa69bf9fa18077262c1b78eff5e2fe2, descends from the verified deployed release receipt 8a42c521 and runtime 2ff2723a. Only state.rs and vault.rs change. The sizing-only transplant comes from tested source afedab2993e87744a9a4e1a14efa59934815e975 and test corrections through 9b477232. No alternate private stable hold, new MemoryId, Candid/API or State schema change is included.

Native-XRP dispatch and preflight use the same per-asset recovery-target cap, with native drop rounding. Full debt is permitted only for existing dust/underwater cases. Invalid math fails closed, non-XRP sizing is unchanged, and a fresh or repeated over-cap burn is rejected without overwriting a retained tuple. Partial settlement retains residual debt/collateral, proportional interest and gross depositor/developer claim conservation.

Adrian's accepted finding V208-COMPOSE-EXPIRY-1 is fixed in cad2523f: an expired retained reservation refuses new admission, but its immutable tuple remains available for proof-backed settlement of an already-burned amount. The regression compares old 1.20/new 1.30 prices, no-mutation refusal and exact legacy settlement. The actual post-burn matcher/commit and all live automatic-origin/signer/sibling replacement guards remain unchanged.

[Focused source review](evidence/xrp-composed-partial-20261117/adrian-focused-source-review.json) is PASS with explicit limits. [Byte-preservation checks](evidence/xrp-composed-partial-20261117/composition-source-checks.json) bind unchanged pause support, main/lib/storage, contracts/declarations, helper and build inputs. This is source proof only, not composed test/artifact proof.

## Required focused verification still pending

One source-bound native batch: LIB xrp_ expected 149, LIB sp_upgrade_hold_tests (4), exact normal BIN check_candid_interface_compatibility (1). No regeneration is expected because the API is byte-identical. Current certified ce014 Candid and candidate DID already pass directional compatibility; typed argument encode/decode and diff checks pass. Unchanged State snapshot proofs are reused.

Then one offline/locked/jobs1 backend-only release build through the existing metadata/shrink/O1/deterministic-gzip/mainnet-guard recipe. Record raw/optimized/gzip hashes, Cargo source provenance/default features, exact embedded Candid and actual executable section against 12 MiB minus 256 KiB. Native checks are not repeated during the build. No optional broad CI/review campaign, new cache/worktree, frontend/SP/bot build or live burn/signature/payment test is requested. All enforced checks remain.

## Proposed future deployment decision — not authorized by this draft

Only target backend tfesu-vyaaa-aaaap-qrd7a-cai, network ic, existing rumi_identity. Expected old module ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f, Running, existing settings/controllers and SP pause=true. Final packet must replace this draft's missing artifact section with actual composed producer results/hashes before requesting installation. Any baseline drift halts instead of overwriting a later writer.

1. Refresh exact target/operator/module/settings/controllers, certified Candid, complete ten-snapshot inventory and relevant configuration/vault/claim/outbox/reserve/bot holds. Preserve raw readbacks privately.
2. Fully archive and verify only oldest snapshot 00000000000000170000000001f088fe0101, timestamp 1794099176324026000, server size 113996773, in the local private recovery directory. Raw state must not be pushed. That archive download was rejected before CreateProcess for missing exact payload/destination export authority; its direct read-only question is pending. No bytes were downloaded and no remote mutation occurred. If the user defers this archive export, the final deployment decision must explicitly include it before any replacement.
3. Under a later exact deployment approval: stop only backend, positively establish Stopped, then create a fresh snapshot replacing ONLY independently archived oldest17. Retain the other nine slots 18..20. No standalone delete/restore. Download/fsync/full-metadata+size+hash verification of fresh stopped backup must pass; its Wasm must bind to ce014 before install. Include both oldest17 export and fresh stopped-state export in the actual preservation approval, with exact local destination.
4. Preserve the complete stopped image. Snapshot creation does not invoke Rust pre_upgrade: a serialized stable State can be stale. Compare against actual active stopped vault state using the established structurally linked comparator; reconcile pre-stop interest/timers and post-start prices/CR separately.
5. Dispatch only the exact future frozen gzip once with the canonical helper's typed args. [Readable args](evidence/xrp-composed-partial-20261117/upgrade-args.did) and [hex](evidence/xrp-composed-partial-20261117/upgrade-args.hex) set mode=null and preserve_sp_liquidation_pause=opt true; hex SHA256 26cbae560b9d7020ff305b4775f0429b5164fb490a11e7aee711ba246a406cb3. Regenerate identical bytes immediately before install; no default/reinstall/unnamed artifact. Record durable intent/unknown-outcome before dispatch.
6. Verify exact new module/settings and embedded Candid while Stopped, then start only backend. Prove Running, SP pause=true, preserved IDs/owners/collateral/claims/escrows/tuples/liabilities/outboxes/configuration and independent anonymous pause check. Compare every debt/accrual to active stopped state, accounting only observed normal accrual/timer deltas. Do not use production payout/claim retry as a test.

On an unknown outcome, reconcile receipts and live module/status before any retry. Before installation, unchanged old code may be restarted only as the bounded abort path with pause rechecked. After installed invariant failure, stop/contain; no old-writer rollback, reinstall or financial snapshot restore. Any restore/source-changed recovery requires its own decision.

## Holds and actual blockers

All financial and liquidation holds remain. No SP resume, compensation, retroactive Vault 208 remedy, Vault 202 recovery, reserve/cap activation, treasury/funding/conversion, operator claim payment/retry, bot upgrade/worker resumption, controller/permission change or UI publication is included. Existing separately authorized automatic Vault 51 work remains unchanged. UI packet 64bacefa is separate.

Automatic review rejected Cody→Dorian private test/build scheduling details for unrecognized destination/payload authorization. One direct coordination question is pending; no allocation/test/build has started, and no rejected payload is being relayed through Adrian. Automatic review separately rejected oldest17 private snapshot export before process creation; one read-only export question is pending. Neither request adds deployment or financial authority. No alternate executor/route or blind retry is permitted.

Current runtime source is preserved; the exact deployment packet is not ready until composed native/build proof and frozen artifact hashes exist. 0 eligible worktrees were removed; all protected/dirty/unmerged work preserved.
