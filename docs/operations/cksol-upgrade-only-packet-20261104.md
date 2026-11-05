# ckSOL backend upgrade-only packet — 2026-11-04

> Historical candidate record. The corrected integrated-source artifact and current proposal are in [the installation-only packet](cksol-installation-only-packet-20261104.md); this older artifact remains obsolete.
**Disposition: HOLD; return to Merritt for review. Do not present the current candidate for execution approval.** Fresh deployed Candid comparison failed: the candidate removes three live Vault 51 recovery methods. This is a demonstrated compatibility failure, separate from the pending vault 158 preparation question. No build, stop/start, installation, asset upload, reserve action, financial canary, or customer message was performed in this preparation.

## Exact target and candidate

- Network: `ic` (ICP mainnet); backend `tfesu-vyaaa-aaaap-qrd7a-cai`.
- Operator identity: `rumi_identity`, principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`, verified as a current controller.
- Source: merged `93d9ece7a9a5f27bac63db207ab9a3e81ed02f63`; build checkout documentation HEAD `72eeea12c92583da2471e5f798bd4b752f30b36d`. Build record: [release evidence](cksol-release-decision-2026-11-04.md).
- Exact local candidate: `/Users/robertripley/.codex/worktrees/solana-icp-processing/rumi-protocol-v2/build-evidence/cksol-release-72eeea12/backend-artifacts/backend-gzip-input.wasm.gz`.
- Gzip SHA-256 `e1aa05c71b9c11d3d7c35e65818745f72f835e323dea37fbc20a2a259fb2b5ad`; 2,314,894 bytes. Optimized module SHA-256 `95b0546faafe25932407417c48e57edad840656d8b080524d9379ed8f7569619`; raw SHA-256 `0d4c050ecd89d40b04bcdb93990d972dcd2b5722679089d8151b031a1ec15b9f`. Executable code 8,676,536 bytes. The gzip hash is the upload/module-hash comparison for this compressed artifact; retain both compressed and decompressed identities.
- Installed ICP CLI 1.3.0 supports automatic chunked upload for this size. Use explicit `--wasm`, bypassing the build wrapper's relative output-copy mismatch. No shrinking campaign is required by the transport evidence.

## Current readbacks and compatibility hold

Read-only observations were collected November 4, 2026 around 7:49–7:50 PM PST (November 5 03:49–03:50 UTC). [Evidence directory](evidence/cksol-upgrade-20261104/) retains deployed Candid, controller-authenticated status, snapshots, query arguments/timestamps, configuration, operation/payment views, compatibility failure, and SHA-256 manifest. These are point-in-time observations; refresh immediately before any separately approved maintenance.

Authenticated status: Running; module `d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95`; cycles 10,559,358,746,709; reserved cycles 0. Controllers are the operator above, `cpbhu-5iaaa-aaaad-aalta-cai`, and `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`. Preserve all settings and environment variables in `status.json`, including compute/memory allocation 0, freezing threshold 2,592,000, reserved-cycles limit 5T, Wasm-memory limit 3,221,225,472 and Controllers log visibility.

Protocol queries report GeneralAvailability, frozen=false, manual_mode_override=false, global mint cap 2,200,000,000,000 e8s, and nonzero protocol debt. Preserve the full captured configuration, collateral policies, balances, liabilities and journal records. These queries do not prove all calls have drained.

`didc check candidate.did live.did` exits 1: `Method reconcile_vault51_icp_payout is only in the expected type`. Service inventory also identifies live-only `refresh_vault51_redemption_claim` and `withdraw_vault51_settled_icp`. See [exact comparison](evidence/cksol-upgrade-20261104/compatibility.txt) and [deployed service](evidence/cksol-upgrade-20261104/live.did). Removing existing recovery endpoints is outside ckSOL upgrade scope. Bram owns the bounded read-only provenance/composition inventory for these deployed methods and coupled recovery changes; Merritt must review preservation/integration of the current deployed surface before a new candidate is approved; this packet does not authorize source changes or another build.

## Operation and payment observations

- Existing relay budget query returns 0. Paid-cycles quote reports next_sequence=5, funding_cycles=1,001,000,000,000, approval_amount=1,001,100,000,000, ledger_fee=100,000,000 and refund reserve=100,000,000.
- Operator-scoped `get_cksol_deposit` returns null. `get_cksol_payment` retains sequence 1, FundingRejected, no funding/refund block or deposit ID, with InsufficientAllowance(0). Preserve this journal; no retry/reconciliation was executed.
- Operator `get_vault_operation_status` reports active=null and acknowledged_through=0. `get_pending_payouts(null,opt 100)` returns no items and no next cursor.
- These scoped reads do **not** enumerate all owners or all asynchronous operations. The deployed interface has no global ckSOL journal enumerator; sequence=5 is not proof that every earlier owner has no held payment. Preserve existing owner-bound receipts/holds and reconcile known unresolved owner records before maintenance. No customer financial action is included.
- Deployed Candid has no ICP-payment terms/journal endpoints yet. Therefore the new ICP reserve cannot be live-read before installation. Source initialization defaults it to zero; after a future approved upgrade, query `get_cksol_icp_terms` and require reserve_available_cycles=0 and available=false, and preserve old relay budget=0. Do not call setters to fund or enable either reserve.

## Proposed maintenance sequence — blocked and unexecuted

The [accepted stop/drain runbook](rumi-backend-drained-upgrade-runbook-20261101.md) governs. This proposal becomes usable only after a compatible candidate is reviewed and the packet is updated with its source, artifact and argument hashes, current readbacks and explicit execution approval.

1. Confirm exact target/controller, candidate hashes and live Candid compatibility again; record the current module and settings. Coordinate the single operator and expected downtime. Preserve protocol mode, caps, holds and journal evidence.
2. Snapshot inventory currently contains ten existing snapshots (IDs and timestamps in `snapshots.txt`). This preparation neither verified an archive nor created/deleted/restored a snapshot. The prior release policy archived and verified an occupied slot before replacing it with a fresh stopped-state snapshot. The applicable preservation-first sequence is already defined by the prior ckSOL releases; see the clarification below. A future execution grant must explicitly include replacement of the selected archived snapshot identity. Never silently delete a snapshot or restore an old snapshot over newer financial state.
3. Submit stop once, retain request/result/time, and query controller-authenticated status until explicitly Stopped. Timeout, Stopping/unknown status, or uncertainty about the stop request means HOLD; do not install. Do not rely on empty owner views as drain proof.
4. Follow the approved snapshot policy after confirmed stop. Reconfirm target, artifact, old module, arguments and controller settings; install once with explicit upgrade mode. The intended command shape below is reference text only and MUST NOT be executed with this incompatible candidate.

```sh
icp canister install tfesu-vyaaa-aaaap-qrd7a-cai \
  --network ic --identity rumi_identity --mode upgrade \
  --wasm /Users/robertripley/.codex/worktrees/solana-icp-processing/rumi-protocol-v2/build-evidence/cksol-release-72eeea12/backend-artifacts/backend-gzip-input.wasm.gz \
  --args-file docs/operations/evidence/cksol-upgrade-20261104/upgrade-args.did
```

Arguments generated by the canonical script (SHA-256 `46d5c27dba094abdd140068b4ef2ada800c734a1633d355285d4ee2246147fbb`):

```candid
(variant { Upgrade = record {
  mode = null;
  description = opt "ckSOL ICP processing: merged source 93d9ece7; reserve remains off"
}})
```

5. Preserve install request/result; on any timeout or unknown result, stop and reconcile management status/module hash before another action. Never repeat install, uninstall, reinstall or auto-restore. The CLI's automatic stop/start guard is not the accepted drain proof; begin installation only on an independently confirmed Stopped target.
6. Only after the installation result and module are known, issue the separately approved start once. Verify Running through controller status; compare module hash, Candid, all controllers/settings, configuration/mode/caps, retained operation/payment records and financial holds. Read new ICP terms and old relay budget as above. If the outcome/hash differs, remain held for reconciliation; no automatic rollback, retry, reserve activation or canary. An uncertain stop cancellation also requires reconciliation and explicit start authority.

## Frontend and remaining evidence

Frontend assets are a separate later packet. A backend-only upgrade does not expose the new ICP UX to customers and must not be reported as an end-to-end launch. Before any later assets publication, compare the complete live asset map, properties and every encoding, preserve existing keys, and bind the frontend build to the compatible installed backend. No live asset inventory or upload was included here. Reserve remains off, so backend deployment alone cannot authorize processing with the new reserve.

Full mocked asynchronous lifecycle and an operator-owned financial canary remain unperformed later client-money evidence. The direct-running-upgrade callback issue remains unresolved; preserve explicit stop/drain. No new test campaign or independent readiness review was run for this held proposal. Return this readable packet to Merritt for review; do not ask Rob for execution approval until the compatibility and snapshot-policy questions are resolved. Vault 158 preparation remains a separate question.

## Bram provenance handoff

Bram identified [PR #613](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/613), branch `origin/codex/vault51-claim-release`, as the deployed recovery composition. Local Git objects confirm source commits `acab050bc70818087c4168180df3326a4431e37e` and `e503f9db01e63388434a7e8d54418eb317a3d377` contain the coupled backend, generated declarations, frontend and test changes. Deployment record commit `a73f2a02e40f982e5debc592f58c90be9d3adb74` records source `e503f9db` and gzip `d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95`, matching this packet's authenticated live hash. Bram found no accepted removal decision.

Preserve the full two-commit source composition, including ClaimRelease stable state/journaling, pending-release collateral fencing, exact payout reconciliation, bounded repayment, ICRC-21 consent and the corresponding frontend/declarations. Merely adding endpoint stubs does not preserve the deployed recovery behavior. The next safe dependency is Merritt/Bram review of integration of that complete source composition into the ckSOL base, followed by separately allocated build/compatibility verification and a refreshed approval packet. This handoff does not perform or authorize that integration or rebuild. WaterNeuron settlement remains pending per the recovery record; vault 158 remains distinct.

## Snapshot-policy clarification — source inspection only

The [current network resource table](https://docs.internetcomputer.org/references/resource-limits/#subnet-limits) specifies **10 snapshots per canister**. The captured inventory contains ten; this is actual capacity, not an inferred cleanup preference. The applicable ckSOL precedent is [October 23 release, line 24](cksol-paid-release-20261023.md#upgrade-and-backup-procedure), corroborated by the [October 31 deployed readback, line 26](cksol-guided-mint-release-20261031.md#deployed-readback): archive the oldest occupied slot completely before replacing it with a fresh stopped-state snapshot. This answers the policy question; no new general policy decision or blanket archive campaign is required.

The earlier [narrow capacity amendment, lines 9–17](sp-interest-inspection-backup-capacity-amendment-20261021.md#exact-additional-action-proposed) supplies the preservation checks, but its historical action authorization is not a new grant for this release. Its inspection-specific stop/download/start-before-upgrade sequence does not override the ckSOL stopped snapshot then upgrade/restart sequence. Current preparation remains read-only.

Recommended concrete execution scope, to include in the eventual canonical release authorization:

1. Refresh authenticated inventory and select exactly its oldest entry. The current captured candidate is `00000000000000070000000001f088fe0101` (September 13, 2026 21:30:08 UTC); if inventory differs, re-evaluate instead of substituting silently.
2. Before stop/replacement, download that exact snapshot to a new protected directory through ICP CLI 1.3.0's native `snapshot download --output` path. Verify snapshot timestamp against inventory, metadata and each declared payload length; retain SHA-256 for metadata, module, Wasm memory, stable memory, and every declared chunk-store payload. Do not assume inventory aggregate bytes equal the sum of three files: the prior record observed a 128-byte difference. Compare each named field instead. No download or restore roundtrip has been performed here.
3. Preserve the complete native upload-compatible layout. [Pinned CLI download source](https://github.com/dfinity/icp-cli/blob/v1.3.0/crates/icp-cli/src/commands/canister/snapshot/download.rs) and [transfer implementation](https://github.com/dfinity/icp-cli/blob/v1.3.0/crates/icp-cli/src/operations/snapshot_transfer.rs) store `metadata.json`, `wasm_module.bin`, `wasm_memory.bin`, `stable_memory.bin`, and `wasm_chunk_store/`. The command reads snapshot metadata/data and writes local files; it does not create/delete a managed snapshot. `--resume` is supported for interrupted local downloads. Retain provenance tying directory, target and snapshot ID; successful download alone is not restore proof.
4. After compatible-artifact review and explicit stop/drain authority, stop once and verify Stopped. Recompare the full inventory. Only with explicit authority to replace this one archived on-network identity, call `snapshot create --replace <exact ID>` once. CLI help confirms the prior identity is deleted after successful replacement. Do not issue a separate delete. Preserve the original request/result and all nine other inventory entries unchanged.
5. Download and verify the returned fresh snapshot as above, bind its metadata/module to the current stopped backend, and preserve both local payload sets. Proceed to the separately approved upgrade/start only with a verified current recovery point and unambiguous snapshot result. An uncertain replacement stops the sequence for authoritative inventory/request reconciliation; never repeat or swap executor. A failed backup does not authorize installation. Availability recovery follows the runbook's explicit reconciled start authority.

Required by the cited accepted release procedure: one fully preserved old slot, a verified fresh recovery point, confirmed drain and exact single-dispatch/reconciliation. Optional and deferred: exporting all ten snapshots, an automated backup service, extra copies beyond the protected evidence store, and a restore drill. None is introduced as a release gate. Snapshot restoration, unrelated deletion/pruning, funding and reserve activation remain outside scope. This clarification establishes the proposed sequence, not evidence that an archive exists or permission to execute it.
