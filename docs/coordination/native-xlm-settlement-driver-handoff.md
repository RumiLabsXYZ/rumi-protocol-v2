# XLM settlement driver: implementation requirements (handoff)

Written 2026-11-14 at `cfeebf55` (source unchanged since `843e4e1c`). Docs only. These are requirements for the next WP4 driver implementation, which will wire the existing dormant helpers to the threshold signer and Horizon. They are not vulnerabilities in the dormant fixture helpers and not new approval gates. Writing the driver is routine implementation; invoking a real signer, live outcalls, installs or funds stays held separately. Line numbers refer to the current branch.

## Existing pieces

- XRP precedent `settle_xrp_claim_as` (`src/rumi_protocol_backend/src/vault.rs:6043`).
- Dormant XLM steps in `src/rumi_protocol_backend/src/state.rs`: `prepare_xlm_settlement_intent` (2200), `admit_xlm_settlement_preflight` (2353, takes `now_unix_ns` at 2359), `admit_and_prepare_xlm_settlement_intent` (2461), `record_xlm_signed_intent` (2528, takes only `claim_id` and `signature`; recomputes the payload hash from the current claim and prepared record at 2595), `apply_xlm_settlement_decision` (2685).
- `decide_evidence_freshness` (`chains/stellar/deposit_evidence.rs:77`); `replacement_decision` (`chains/stellar/transaction_status.rs:487`, `PermitReplacement` at 478); `SubmitAdvisory` and `parse_submit_response` (`chains/stellar/horizon_rpc.rs:667`, `683`); signer `sign_payload_hash` and `derive_stellar_address` (`chains/stellar/ted25519.rs:146`, `115`).

## Requirements

1. **Two guards, as XRP.** `settle_xrp_claim_as` takes a per-claim `GuardPrincipal` (`vault.rs:6072`) and a per-custody `VaultLiquidationGuard::new(claim.custody_nonce)` (`vault.rs:6076`; comment at 6073–6075: one lock per custody address so two payments cannot collide on the account sequence). The XLM driver takes both, keyed the same way, for the whole settle call including awaits.
2. **Capture time after the reads.** Read `ic_cdk::api::time()` after the awaited account and reserve quorum reads complete and pass that value to admission and freshness. Never accept a caller-supplied time or reuse a time read before the awaits.
3. **Bind the signature to what was signed.** Before awaiting the signer, capture the prepared record (at least `payload_hash`, `next_sequence`, time bounds and claim id). After the signer returns, require the claim's current `prepared_intent` to equal that captured snapshot before recording. The dormant recorder recomputes the hash from current state and receives only the signature, so on its own it does not prove the signature was produced over the current payload.
4. **Interrupted signing resumes or holds.** A claim that is Pending with a persisted `prepared_intent` and no signed intent (for example after a failed or interrupted sign) must resume signing that same persisted preparation, or hold. It must not prepare again or replace it blindly; clearing a preparation happens only through the reconciliation outcomes.
5. **Reconcile in-flight first.** If the claim, or a sibling for the same custody owner, nonce and network, is InFlight or holds a signed intent, the driver runs confirmation and reconciliation (status quorum, `replacement_decision`, `apply_xlm_settlement_decision`) before any new signing, as XRP confirms a previous settlement before signing again (`vault.rs:6090–6096`).
6. **Submission results are advisory.** A 200, 400 or 504 from `parse_submit_response` never changes claim status and never grants replacement. Only the status quorum through `replacement_decision` and `apply_xlm_settlement_decision` can move the claim or permit a new payment.

## Minimal mocked acceptance cases for the driver

- **Drift across awaits:** change the claim (gross, destination, network or custody) or a sibling between the quorum reads and admission, and between signing and recording; the driver records nothing and the claim is unchanged.
- **Time capture:** a stale reserve observed before a long await is rejected using the post-read time; a caller cannot supply a time.
- **Signature binding:** replace `prepared_intent` while the signer is pending; the returned signature is not recorded.
- **Prepared resume:** a Pending claim with a persisted preparation and no signed intent resumes the same preparation (same payload hash and sequence) or holds; it never produces a new preparation.
- **Same-custody siblings:** a concurrent settle for another claim on the same custody account is refused by the custody guard; an InFlight or signed sibling forces reconciliation first.
- **Advisory only:** mocked 200 with matching hash, 400 and 504 submission responses each leave the claim InFlight with its signed intent unchanged.

## Status

Validation of the transport source (`843e4e1c`, runner `67e9d11c`) remains unallocated. Rob has now approved the existing Cody–Mara coordination messaging decision, and Cody was notified to check capacity and arrange a fresh grant. Sufficient disk or an idle resource slot does not grant an allocation. No compiler or live action has run for this transport source.

## Material-update closeout

Before closing a material implementation or evidence handoff, reconcile the existing Lookout cards, next actions and priorities with the accepted source and receipts; update both technical `summary` and 2–3 sentence `plainSummary`. Use `native-xlm-lookout-manifest.json` and the existing scoped publisher for dry-run, publish and a second dry-run. Record the old/new revision and require board, presentation, Plain English, priorities and repository association to read back unchanged. Follow `native-xlm-lookout-publishing.md`; if evidence is unchanged, do not publish or change completion dates. Preserve actual denials and all live-action holds. Publication is status housekeeping, not proof of deployment or customer impact.

Latest validation: the allocated original transport runner stopped at 21 passed/1 failed; production check did not run. The test-only trailing-slash expectation correction is pushed at `bdb54802`, not retested. Preserve the consumed allocation and original failure; a fresh source-bound reviewed validation packet is needed before another run. See `native-xlm-horizon-test-receipt-2026-11-21.md`.

Corrected transport validation is now locally PASS: `bdb54802` at `abe253aa`, 22 focused tests and feature-off wasm32 compile check, native 0/0, receipt `a4c4bfa4`. Both owned groups independently closed. Prior failure remains historical. The next step is this document's existing integrated driver implementation and mocked acceptance cases; this PASS does not prove live signing, submission or settlement.

## Current source-executor dependency

Original Mara remains delivery/review owner and the original Mara Junior implementation session remains the source executor. Its retained transcript `89666801-92e0-4873-a0f3-6e345bb78f4c` is present; the latest assistant text is dated 2026-11-15T06:11:03.651Z and no later driver implementation is recorded. The temporary extra-Claude push ended November 16. Current included allowance/reset is unverified; old authentication or usage snapshots do not establish available capacity today. No new inference, replacement session, paid fallback or private-usage UI access retry was made.

The exact dependency for admitting that existing executor is a currently authorized, verified included-capacity route. Cody has been notified through the approved coordination scope. Routine source implementation authority remains Rob's original continuous instruction; the six requirements and mocked acceptance cases above are unchanged and need no new design approval. There is no driver source delta to validate yet, so no new mocked-validation packet or compiler request is presented. Do not manufacture a test campaign before the source is concrete. When that source exists, return its actual delta/review readiness and one bounded mocked batch for fresh allocation; live calls remain held.

Dependency closeout: the existing scoped publisher dry-run/publish/final dry-run exited 0, revision **76 → 77**, source revision `a5c1feb9abc2509486adfd0e7385476f5b0737727b07cbd2b58ad805b862090f`. Board, technical presentation, Plain English summary, priorities and repository association read back unchanged. Historical transport PASS and all holds are preserved. No source or inference work is claimed by this status update.
