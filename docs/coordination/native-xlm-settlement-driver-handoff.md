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

Validation of the transport source (`843e4e1c`, runner `67e9d11c`) remains waiting for shared-machine capacity and an existing coordination question. This note does not change that status or the Lookout board.
