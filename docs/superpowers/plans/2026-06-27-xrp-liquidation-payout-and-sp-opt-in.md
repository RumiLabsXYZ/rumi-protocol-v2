# XRP liquidation payout and Stability Pool opt-in implementation plan

Status: **implemented on `codex/xrp-liquidation-payout-ui`; adversarially revised during implementation**.

Spec: `docs/superpowers/specs/2026-06-27-xrp-liquidation-payout-and-sp-opt-in.md`.

## Ground Rules

- Use test-driven changes: add or update failing focused tests before production
  code for each phase.
- Keep native-XRP SP rejection in place until the end-to-end SP payout path is
  implemented and tested.
- Do not deploy backend or SP canisters from this branch without separate
  explicit deploy authorization.
- Regenerate Candid/declarations only after canonical `.did`/Rust interface
  changes are complete.
- Use subagents with disjoint ownership:
  - backend worker: `src/rumi_protocol_backend/**`
  - SP worker: `src/stability_pool/**`
  - frontend worker: `src/vault_frontend/**` and `src/declarations/**` after
    regeneration

## Phase 1: Manual XRP Liquidation Claim Id

Tests first:

- Update `src/rumi_protocol_backend/tests/xrp_native_e2e_pic.rs` so
  `xrp_native_liquidation_is_claim_based` asserts the returned
  `SuccessWithFee.xrp_claim_id` is present.
- Extend the test to inspect `get_xrp_claims` and prove the returned id belongs
  to the liquidator reward claim, not developer protocol-fee or owner-excess
  claims.
- Add a non-XRP liquidation/operation assertion that `xrp_claim_id` is absent.

Implementation:

- Add `xrp_claim_id: Option<u64>` to `SuccessWithFee`.
- Make `queue_collateral_payout` return `Option<u64>`.
- Thread the returned claim id into complete, partial, and stable-token manual
  native-XRP liquidation `SuccessWithFee` results.
- Leave non-XRP results as `None`.

Verification:

- Focused backend tests for the updated XRP liquidation path.
- Candid compatibility test for `rumi_protocol_backend`.

## Phase 2: Manual Liquidation UI Payout and Retry

Tests first:

- Add pure helper tests under `src/vault_frontend/src/lib/...` for:
  - XRP collateral detection by synthetic principal;
  - payout address/tag validation;
  - exact destination-tag bounds: absent, `0`, and `4294967295` accepted;
    negative, fractional, and `4294967296` rejected;
  - safe claim-id mapping from `u64`;
  - settlement phase calls `XrpVaultService.settleXrpClaim`;
  - failed settlement preserves a retryable pending claim with the same claim id,
    payout address, and destination tag;
  - failed settlement copy states the claim remains outstanding and never says
    XRP was received;
  - success copy has two phases: claim created, then settlement submitted or
    confirmed;
  - ambiguous liquidation failure recovers relevant claims from
    `get_my_xrp_claims`.

Implementation:

- Extend `VaultOperationResult` and `ApiClient` liquidation mappings with
  `xrpClaimId`.
- Extend `XrpVaultService.getMyClaims` to expose `custodyNonce`, settlement, and
  quarantine metadata needed for recovery.
- Add XRP payout address/tag state keyed by vault id in
  `ManualLiquidations.svelte`.
- For XRP vaults, require address before liquidating, then settle the returned
  claim id to the supplied address/tag.
- If settlement fails, show a retry action in the manual liquidation UI.
  Keep that retry action visible even if the vault disappears from the
  liquidatable-vault list after the liquidation succeeds.
- If liquidation returns an ambiguous error, refresh `get_my_xrp_claims` and
  expose any claim whose `custodyNonce` matches the vault id.
- Keep non-XRP liquidation UI unchanged.

Verification:

- Focused frontend helper/service tests.
- `npm test --workspace=src/vault_frontend -- --run`.

## Phase 3: SP XRP Opt-In State and Live Card

Tests first:

- Add SP state tests for:
  - `opt_in_native_collateral_with_tag` stores address and optional tag;
  - address-only `opt_in_native_collateral` remains compatible;
  - `get_user_position` exposes both address and tag as optional fields;
  - `opt_out_collateral` clears both address and tag;
  - old snapshots decode with empty tag and pending-payout maps.
- Add frontend helper/service tests for:
  - unwrapping `native_payout_addresses` and
    `native_payout_destination_tags`;
  - calling `optInNativeCollateralWithTag` for XRP;
  - hiding ICRC claim buttons for native XRP gains/payouts.
- Add at least one live-card integration test or static regression test that
  fails if XRP opt-in remains only in dead `UserAccount.svelte` and not in
  `EarnInfoCard.svelte` or a component it renders.

Implementation:

- Add `native_payout_destination_tags` to `DepositPosition` and
  `UserStabilityPosition` as optional Candid fields.
- Add `opt_in_native_collateral_with_tag` in SP state and canister API.
- Keep `opt_in_native_collateral` as an address-only wrapper.
- Port the XRP opt-in UI into the live `EarnInfoCard.svelte` path or a child it
  renders; do not rely on dead `UserAccount.svelte`.
- Add `StabilityPoolService.optInNativeCollateralWithTag`.

Verification:

- Focused SP lib tests.
- Focused frontend tests.

## Phase 4: SP XRP Absorption and Pending Payouts

Tests first:

- Backend tests:
  - caller validation rejects non-SP callers without mutation;
  - frozen/disabled preflight rejection occurs before mutation;
  - native-XRP validation rejects non-XRP vaults without mutation;
  - preflight liquidatable check rejects recovered/healthy vaults without mutation;
  - `xrp_sp_preflight_rejects_non_xrp_vault`;
  - `xrp_sp_preflight_stores_reservation`;
  - `xrp_sp_absorb_requires_matching_preflight`;
  - expired/mismatched preflight rejects before mutation;
  - empty allocation vectors reject before mutation;
  - allocation sum mismatch against `collateral_received_drops` rejects before
    mutation;
  - empty payout address and zero-drop allocations reject before mutation;
  - `xrp_sp_absorb_replays_same_proof_same_claim_ids`;
  - `xrp_sp_absorb_rejects_conflicting_replay_without_mutation`;
  - `xrp_sp_absorb_rejects_over_500_allocations`;
  - `xrp_sp_absorb_assigns_dust_to_first_sorted_depositor`.
  - 10,000-entry replay-result bounded eviction keeps the just-accepted proof
    key and evicts older unrelated keys.
  - old backend snapshots without `sp_xrp_absorb_preflights` and
    `sp_xrp_absorb_results_by_proof` decode with empty maps.
- SP tests:
  - native XRP allocations are opt-in only;
  - non-opted-in depositors do not burn or receive pending payouts;
  - `xrp_sp_absorb_does_not_credit_icrc_collateral_gains`;
  - over-500 non-zero allocations abort before icUSD burn is attempted;
  - pending payouts store claim id, vault id, address, optional tag, and drops;
  - `claim_collateral(xrp)` rejects before ledger calls;
  - `claim_all_collateral` skips native XRP while still claiming ICRC gains.
  - `ack_native_xrp_payout_settled` removes only the caller's own pending record
    after backend claim absence is verified, rejects still-outstanding claims,
    and rejects/no-ops for another user's claim id without mutation.

Implementation:

- Backend:
  - Add `XrpSpAbsorbPreflight`, `XrpSpPayoutAllocation`,
    `XrpSpAbsorbRequest`, `XrpSpPayoutClaim`, and `XrpSpAbsorbResult`.
  - Add `StoredXrpSpAbsorbPreflight` and `StoredXrpSpAbsorbResult` state maps
    with default-empty migration behavior and 10,000-entry bounded replay
    storage.
  - Add `stability_pool_preflight_xrp_absorb`.
  - Add `stability_pool_liquidate_xrp_vault`, replaying by proof key and
    rejecting conflicting fingerprints.
  - Write down the XRP vault once and create one backend `XrpClaim` per
    depositor allocation.
  - Add a registered-SP-only claim-status endpoint so the SP can verify an XRP
    claim is absent before clearing a local payout reminder.
- SP:
  - Add a `NativeXrpAbsorbIntent` journal that records prepared intent before
    burn, burn proof after burn, and backend result before local apply.
  - Retry burned intents with the exact stored proof/request and no second burn.
    Retry backend-accepted intents by applying local payout reminders without a
    second backend call.
  - Validate backend results before persisting them as accepted; malformed
    results remain retryable from the burned state.
  - Add `NativeXrpPendingPayout` and pending-payout state.
  - Add deterministic allocation builder: principal-byte sort, 500 max
    allocations, dust to first sorted eligible depositor.
  - Add SP execution path that preflights, computes allocations from
    `collateral_received_drops`, aborts before burn if non-zero payout count
    exceeds 500 or no eligible payout exists, then burns, submits the
    proof/request, and records pending payouts from backend result.
  - Add `get_my_native_xrp_payouts` and `ack_native_xrp_payout_settled`.
  - Make `ack_native_xrp_payout_settled` verify the backend claim is absent for
    the caller before mutating pending-payout state.
  - Remove or bypass current native-XRP SP rejects only after the above tests
    pass.

Verification:

- Focused backend and SP tests.
- Candid compatibility tests.

## Phase 5: Frontend SP Pending Payout Settlement

Tests first:

- Add helper/service tests proving pending XRP payouts render with stored
  address/tag, call `XrpVaultService.settleXrpClaim`, and call
  `ackNativeXrpPayoutSettled` only after settlement success plus a follow-up
  claim read proving the backend claim is no longer listed. A submitted or
  in-flight XRPL payment must keep the SP reminder visible for confirm/retry.
- Add wrong-caller/foreign-record and still-outstanding tests at the canister or
  pure-state boundary. Frontend tests must also prove they do not try to clear a
  reminder while `hasOutstandingClaim` still reports the backend claim.

Implementation:

- Add `StabilityPoolService.getMyNativeXrpPayouts` and
  `ackNativeXrpPayoutSettled`.
- Add pending-payout UI to the live SP card/child component.
- Keep settlement retry visible and user-driven.
  It must render for stored pending payouts even if the user no longer has a
  positive icUSD SP balance after absorption.
- Do not call `ackNativeXrpPayoutSettled` after the first submit-only
  `settleXrpClaim` success; acknowledge only after claim absence is verified.
  The SP canister must independently verify the same absence before removing the
  pending record.

Verification:

- Frontend unit tests.
- `npm run check --workspace=src/vault_frontend`; separate baseline failures
  from changed-file failures.

## Phase 6: Regeneration and Final Verification

- Run `npm run regenerate-declarations -- rumi_protocol_backend` if supported or
  `scripts/regenerate-declarations.sh rumi_protocol_backend`.
- Run `scripts/regenerate-declarations.sh rumi_stability_pool`.
- Assign one integration owner for all Candid/declaration regeneration and diff
  review because this crosses backend, SP, and `src/declarations`.
- Review declaration diffs for the exact optional fields and new methods.
- Confirm `xrp_claim_id : opt nat64` remains Candid-compatible for old/new
  backend/frontend skew.
- Run focused backend/SP/frontend tests.
- Run broader feasible suites:
  - `cargo test -p rumi_protocol_backend --lib`
  - `cargo test -p stability_pool --lib`
  - `npm test --workspace=src/vault_frontend -- --run`
  - `npm run build --workspace=src/vault_frontend`
- Run adversarial verification on implementation before commit/PR.
