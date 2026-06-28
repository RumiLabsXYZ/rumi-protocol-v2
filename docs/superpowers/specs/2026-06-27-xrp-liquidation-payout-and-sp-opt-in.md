# XRP liquidation payout and Stability Pool opt-in spec

Status: **implemented on `codex/xrp-liquidation-payout-ui`; adversarially revised during implementation**.

Date: 2026-06-27.

## Goal

Users must never call canister functions manually to participate in XRP
liquidations. Manual XRP liquidators should provide an XRPL payout address in the
liquidation UI and receive the seized XRP there. Stability Pool depositors should
opt into XRP absorption from the live Stability Pool card by providing an XRPL
payout address.

## Current source facts

- Manual liquidation is rendered by
  `src/vault_frontend/src/lib/components/liquidations/ManualLiquidations.svelte`
  from `/liquidations`.
- Manual XRP vaults appear in `get_liquidatable_vaults_page`; automated unhealthy
  scans skip native XRP and the SP/backend automated paths currently reject
  native XRP.
- Native-XRP collateral payouts are claim-based. `queue_collateral_payout` creates
  an `XrpClaim` instead of an ICRC pending transfer.
- `settle_xrp_claim` and `settle_xrp_claim_with_tag` are already exposed on the
  live backend and wrapped by `XrpVaultService.settleXrpClaim`.
- Manual liquidation currently returns `SuccessWithFee` without the liquidator's
  XRP claim id, so the UI cannot deterministically settle the claim produced by
  the liquidation it just performed.
- The live SP card is `EarnInfoCard.svelte`. `UserAccount.svelte` contains XRP
  opt-in UI code but is not imported anywhere.
- Current source/declarations include `opt_in_native_collateral`, but the live
  `rumi_stability_pool` mainnet canister does not yet expose that method.
- XRP is represented by the synthetic collateral principal
  `5zjma-7dsov-wwsll-yojyc-23tbo-ruxmz-i` (`Principal::from_slice(b"rumi-xrp-native")`).
- Live metadata checks on 2026-06-27 confirmed the same split:
  `rumi_protocol_backend` exposes `settle_xrp_claim(_with_tag)`, while
  `rumi_stability_pool` does not expose `opt_in_native_collateral`.

## Product requirements

### Manual XRP liquidation payout

1. When a liquidatable vault's collateral is native XRP, the manual liquidation
   card must show an XRPL payout-address input before the user can liquidate.
2. The user must not see or be asked to run raw `icp canister call` commands.
3. The user must be able to provide an optional destination tag for destinations
   that require one.
4. The UI must validate basic empty/tag errors client-side:
   - payout address is required for XRP vault liquidations;
   - destination tag, when present, must be a whole number from `0` to
     `4294967295`.
5. The backend must return the exact `XrpClaim` id created for the liquidator's
   reward from each manual native-XRP liquidation path.
6. After a successful manual native-XRP liquidation, the UI must call
   `XrpVaultService.settleXrpClaim(claimId, address, destinationTag?)`.
7. The success message must distinguish the two phases:
   - liquidation accepted and XRP claim created;
   - settlement submitted or confirmed with a tx hash when available.
8. If the settlement step fails after liquidation succeeded, the UI must not imply
   funds are lost. It must tell the user the XRP claim remains outstanding and can
   be retried.
9. The retry path must be UI-owned. The implementation must either mount an
   equivalent live claim-settlement component for manual-liquidation claims or
   persist the pending claim id, destination address, and destination tag in the
   manual liquidation UI until retry/settlement succeeds.
   The retry action must remain visible even after the liquidated vault leaves
   `get_liquidatable_vaults`; a created XRP claim must not disappear merely
   because the vault is no longer liquidatable.
10. The UI must also recover from ambiguous liquidation success. If the
    liquidate call returns an Oisy/network/unknown error after the backend may
    have executed, the manual liquidation view must refresh
    `get_my_xrp_claims`, expose claim metadata including `custody_nonce`/vault
    id, and offer the same settle/retry action without asking for raw calls.
11. Non-XRP manual liquidations must keep the existing flow and not require an XRP
   address.
12. The UI must not claim XRP was received until the settlement call succeeds.

### Stability Pool XRP absorption opt-in and payout

1. A user with an active icUSD SP deposit must be able to opt into XRP absorption
   from the live Stability Pool card, not from dead code.
2. The opt-in UI must be in `EarnInfoCard.svelte` or a component it renders.
3. The XRP row must ask for an XRPL payout address and optional destination tag.
   Exchange destinations often require tags, so the SP path needs parity with the
   manual liquidation path.
4. The live SP card should call a Candid-compatible opt-in endpoint that stores
   both address and optional tag. The existing
   `optInNativeCollateral(xrpPrincipal, address)` can remain as an address-only
   compatibility wrapper, but the XRP UI must use the tag-aware path.
5. XRP opt-out must clear the payout address and destination tag through the
   existing
   `optOutCollateral` path.
6. `native_payout_addresses` is Candid `opt vec`; frontend code must unwrap it as
   `userPosition.native_payout_addresses?.[0] ?? []`.
   `native_payout_destination_tags` must be exposed in `UserStabilityPosition`
   and unwrapped as `userPosition.native_payout_destination_tags?.[0] ?? []`.
7. Native XRP collateral gains must not render an ICRC claim button. XRP payouts
   are external settlement, not an ICRC ledger claim.
8. Normal ICRC collateral routing must keep the current opt-in/opt-out behavior.
9. The visible copy must describe the user action in product terms: provide an
   XRP payout address and optional tag to receive XRP from SP liquidations.
10. SP XRP absorption is not complete with an opt-in button alone. The absorption
   path must define how seized XRP becomes payable to opted-in depositors.
11. The implementation must not create one aggregate XRP claim owned only by the
    SP canister. That would strand funds because the SP canister does not have an
    XRP custody account or redistribution rail.
12. The backend/SP absorption flow must create depositor-specific payable XRP
    claims for non-zero XRP gains. The preferred narrow design is:
    - the SP computes the same pro-rata, opt-in-only allocation it would use for
      collateral gains, but native XRP must **not** be inserted into
      `DepositPosition.collateral_gains`;
    - the registered SP calls a backend native-XRP SP liquidation endpoint with a
      bounded set of intended
      `(depositor principal, payout address, destination tag, drops)` allocations;
    - the backend validates the caller is the registered SP, writes down the vault
      once, and records one `XrpClaim` per allocation with `claimant = depositor`;
    - the SP stores the returned
      `(depositor, claim id, payout address, destination tag, amount)` records so
      the frontend can settle/retry them through UI without raw calls.
13. If automatic settlement is implemented from the app, it must use the stored
    payout address and destination tag. If settlement fails, the live SP card must
    show the pending XRP payout and a retry action.
14. Until the backend native-XRP SP absorption endpoint and SP pending-payout
    storage are present and tested, the SP must keep native-XRP liquidation
    disabled/rejected. Removing the current native-XRP rejects is only allowed in
    the same change set that proves the end-to-end payout path.
15. `claim_collateral` and `claim_all_collateral` must never attempt an ICRC
    transfer for native XRP. Native XRP SP gains live only in pending XRP payout
    records and backend `XrpClaim`s.

## Backend requirements

1. Extend `SuccessWithFee` with a backward-compatible optional field:
   `xrp_claim_id: Option<u64>`.
2. For manual native-XRP liquidator reward claims, populate `xrp_claim_id` with
   the claim id returned by `record_xrp_claim`.
3. For non-XRP liquidations and non-liquidation uses of `SuccessWithFee`, return
   `None`.
4. The liquidator's `xrp_claim_id` must not be confused with protocol-fee or
   owner-excess XRP claims created during the same liquidation.
5. `queue_collateral_payout` should return `Option<u64>` so call sites can record
   the exact liquidator reward claim id when native XRP is involved.
6. Add a tag-aware SP opt-in endpoint:
   `opt_in_native_collateral_with_tag(collateral_type: Principal,
   payout_address: String, destination_tag: Option<u32>)
   -> Result<(), StabilityPoolError>`. The existing
   `opt_in_native_collateral(collateral_type, payout_address)` remains and calls
   the new state helper with `None`.
7. `UserStabilityPosition` must add an optional read field
   `native_payout_destination_tags: Option<BTreeMap<Principal, u32>>` without
   changing `native_payout_addresses`.
8. Add a native-XRP SP preflight endpoint:
   `stability_pool_preflight_xrp_absorb(vault_id: u64,
   expected_icusd_burn_e8s: u64) -> Result<XrpSpAbsorbPreflight, ProtocolError>`.
   It must be callable only by the registered SP, reject frozen/disabled paths
   before any burn, validate that the vault is native XRP and liquidatable, and
   store a preflight reservation keyed by `vault_id`.
9. `XrpSpAbsorbPreflight` contains:
   `{ vault_id: nat64; icusd_burn_e8s: nat64; collateral_received_drops: nat64;
      collateral_price_e8s: nat64; expires_at_ns: nat64 }`.
   The SP must use `collateral_received_drops` from this preflight to compute
   depositor allocations before burning icUSD.
10. Backend state must add Candid/stable-compatible fields:
    - `sp_xrp_absorb_preflights:
       BTreeMap<u64, StoredXrpSpAbsorbPreflight>`
    - `sp_xrp_absorb_results_by_proof:
       BTreeMap<(SpProofLedger, u64), StoredXrpSpAbsorbResult>`
    Missing fields decode to empty maps.
11. `StoredXrpSpAbsorbPreflight` stores
    `{ caller, vault_id, icusd_burn_e8s, collateral_received_drops,
       collateral_price_e8s, expires_at_ns }`.
12. `StoredXrpSpAbsorbResult` stores
    `{ caller, vault_id, icusd_burned_e8s, proof_ledger, proof_block_index,
       allocation_fingerprint, result, accepted_at_ns }`.
13. `MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF` is 10,000 and uses the same bounded
    eviction rule as `sp_chain_absorb_results_by_proof`: keep the just-accepted
    proof key and evict oldest other proof keys when over the cap.
14. Add a native-XRP SP absorption endpoint rather than reusing the existing
   generic SP path that would create a single claim for the SP canister:
   `stability_pool_liquidate_xrp_vault(request: XrpSpAbsorbRequest)
   -> Result<XrpSpAbsorbResult, ProtocolError>`.
15. The request/response records are additive Candid types:
   - `XrpSpPayoutAllocation { claimant: principal; payout_address: text;
      destination_tag: opt nat32; drops: nat64 }`
   - `XrpSpAbsorbRequest { vault_id: nat64; icusd_burned_e8s: nat64;
      proof: SpWritedownProof; allocations: vec XrpSpPayoutAllocation }`
   - `XrpSpPayoutClaim { claimant: principal; claim_id: nat64;
      payout_address: text; destination_tag: opt nat32; drops: nat64 }`
   - `XrpSpAbsorbResult { success: bool; vault_id: nat64;
      liquidated_debt_e8s: nat64; collateral_received_drops: nat64;
      payout_claims: vec XrpSpPayoutClaim; block_index: nat64;
      collateral_price_e8s: nat64 }`
16. The native-XRP SP submit endpoint must reject unless the caller is the
   registered SP canister, the request matches a live preflight reservation for a
   native-XRP vault, the allocation sum equals the post-fee depositor-share
   amount represented by the reserved `collateral_received_drops`, every
   allocation has a non-empty payout address, every destination tag is either
   absent or `u32`, every allocation amount is non-zero, and allocation count is
   in `1..=MAX_XRP_SP_PAYOUT_ALLOCATIONS`. Submit must not re-check live vault
   liquidatability after the SP has burned icUSD; the preflight reservation is
   the liquidatability snapshot for the post-burn submit.
17. A post-burn request must match an unexpired preflight reservation for the
   same `caller`, `vault_id`, and `icusd_burned_e8s`, unless it is an exact replay
   of an already stored proof-key result. If no matching preflight exists, the
   endpoint rejects before mutation.
   `XRP_SP_ABSORB_PREFLIGHT_TTL_NS` must mirror the existing chain SP preflight
   TTL unless implementation evidence shows XRP needs a narrower window. Expired
   preflights may be overwritten by a fresh preflight for the same vault.
18. `MAX_XRP_SP_PAYOUT_ALLOCATIONS` is 500. If an XRP absorption would create more
   than 500 non-zero depositor payouts, the SP must abort before burning icUSD and
   leave the vault for manual liquidation.
19. Allocation ordering and dust are deterministic:
    - sort eligible depositor allocations by principal bytes ascending before
      sending the backend request;
    - compute each depositor's drops as
      `collateral_received_drops * user_consumed_e8s / total_consumed_e8s`;
    - omit zero-drop allocations;
    - assign any remaining dust to the first sorted eligible depositor, even if
      all floor allocations are zero.
20. Native-XRP SP absorption must preserve existing no-double-liquidation and
   no-mutation-on-rejection invariants. Any icUSD burn/depositor accounting and
   backend writedown must be idempotency-protected like the existing SP paths.
   The SP must persist a native-XRP absorb intent before burning, then persist
   the burn proof and backend-accepted result before local depositor payout
   application. A retry after burn must reuse the exact stored proof/request
   without another burn; a retry after backend acceptance must apply local
   payout reminders without another backend call. Malformed backend responses
   must not be persisted as accepted.
21. The SP canister must store pending native-XRP payouts returned by the backend
   so the UI can settle/retry without users calling raw backend functions.
   Pending XRP payout settlement must remain visible even if the absorb consumes
   the user's remaining icUSD position.
22. The native-XRP SP endpoint must be replay-safe:
    - operation key is `(proof.ledger_kind, proof.block_index)`, matching the
      existing chain absorb proof-key pattern;
    - the replay fingerprint is
      `(caller, vault_id, icusd_burned_e8s, proof ledger/block, sorted
      allocations claimant/address/tag/drops)`;
    - the backend must store `XrpSpAbsorbResult` plus that fingerprint before
      replying;
    - retry with the same proof key and fingerprint returns the same result and
      same claim ids;
    - retry with the same proof key and a different fingerprint rejects without
      mutation.
23. Generated Candid and frontend declarations must be regenerated from the
   updated canonical interfaces.
24. The change must preserve compatibility with older deployed backends and older
   frontend clients:
   - newer frontend decoding an older backend sees no claim id;
   - older frontend decoding a newer backend ignores the extra optional field.
25. SP state changes must be Candid/stable-state compatible. Preserve
    `native_payout_addresses` for address-only compatibility and add optional
    tag-aware state rather than changing the existing field's wire type in place.
26. Exact additive SP state fields:
    - `DepositPosition.native_payout_destination_tags:
       Option<BTreeMap<Principal, u32>>`
    - `DepositPosition.pending_native_xrp_payouts:
       Option<BTreeMap<u64, NativeXrpPendingPayout>>`
    - `NativeXrpPendingPayout { claim_id: u64; collateral_type: Principal;
       vault_id: u64; drops: u64; payout_address: String;
       destination_tag: Option<u32>; created_at_ns: u64 }`
27. Add SP methods:
    - `get_my_native_xrp_payouts() -> Vec<NativeXrpPendingPayout>`
    - `ack_native_xrp_payout_settled(claim_id: u64) -> Result<(), StabilityPoolError>`
      which removes only the caller's own pending-payout record after the SP
      canister independently verifies with the backend that the corresponding
      `XrpClaim` is no longer outstanding for that caller. This is a UI cleanup
      acknowledgement, not custody authority: the frontend may call it only after
      `settleXrpClaim` succeeds and a follow-up claim read confirms the backend
      claim is gone, but the canister boundary must enforce the same absence
      check. A submitted/in-flight XRPL payment must keep the SP reminder visible
      for a later confirm/retry. A malicious caller cannot clear a reminder early
      by bypassing the UI, cannot remove another user's row, and cannot affect
      the backend `XrpClaim`.
28. Direct `claim_collateral(xrp_principal)` must return
    `StabilityPoolError::PayoutAddressRequired { collateral: xrp_principal }`
    or a more explicit native-XRP-not-ICRC error before any ICRC ledger call.
    `claim_all_collateral` must skip native XRP and continue claiming normal
    ICRC gains.

## Frontend requirements

1. Add an XRP-specific manual liquidation address/tag state keyed by vault id.
2. Add a small helper for parsing destination tags and identifying native-XRP
   collateral from the synthetic principal.
3. Update `ApiClient` liquidation result mapping to expose `xrpClaimId?: number`.
   Because the backend value is `u64`, mapping to `number` must include a
   `Number.isSafeInteger` guard or the new UI should carry claim ids as strings.
4. Reuse `XrpVaultService.settleXrpClaim`; do not duplicate backend actor calls.
5. Keep Oisy gesture constraints in mind: avoid extra preflight signer calls before
   the approve/liquidate operation. The settlement call is a second explicit phase
   after liquidation succeeds.
6. Add the XRP SP opt-in controls to the live card. `UserAccount.svelte` should
   not remain the only implementation of the feature.
7. Add a live SP pending-XRP-payout surface that reads the SP's pending records
   and lets the connected depositor settle/retry to the stored payout address.
8. Extend `XrpVaultService.getMyClaims` (or an equivalent live helper) to expose
   enough metadata for recovery: claim id, drops, `custody_nonce`/vault id,
   in-flight settlement state, and quarantine state if present.
9. Add `StabilityPoolService.optInNativeCollateralWithTag`,
   `getMyNativeXrpPayouts`, and `ackNativeXrpPayoutSettled` wrappers. The live
   card uses the tag-aware wrapper for XRP and may keep the address-only wrapper
   for older canisters.
10. Do not add new dependencies.

## Security and custody invariants

1. Only the claimant can settle an XRP claim; the backend already enforces this
   through `settle_xrp_claim(_with_tag)`.
2. The UI-provided address/tag must be the exact destination passed to the backend
   settlement call.
3. The backend records in-flight settlement before submit; the UI must surface
   retry/confirm semantics instead of double-submitting hidden operations.
4. A settlement failure after successful liquidation is a pending-claim state, not
   a failed liquidation state.
5. SP XRP absorption is opt-in only. Depositors without a stored payout address
   must not absorb XRP liquidations.
6. Canister interface changes must be Candid-compatible and declaration drift must
   be eliminated before merge.

## Non-goals

- No raw-user CLI workflow.
- No raw CLI settlement for normal users; all settlement/retry paths must be
  reachable from the application UI.
- No automatic background settlement of manual liquidation claims outside a
  visible UI-owned phase.
- No exchange-specific destination-tag recommendation beyond supporting the tag
  field.
- No SP mainnet canister deploy without a separate explicit deploy authorization
  and upgrade safety check.
- No removal of the existing XRP claim panel unless it is proven dead and replaced
  by equivalent live UX.

## Acceptance criteria

### Spec-level acceptance

- The spec covers both requested product paths: manual XRP liquidation payout and
  Stability Pool XRP absorption opt-in.
- The spec names the current live blockers: no manual payout prompt, no claim id
  in liquidation result, SP opt-in UI in dead code, and SP mainnet method not
  deployed.

### Backend acceptance

- Tests fail before implementation for missing `xrp_claim_id` in native-XRP
  manual liquidation results.
- Tests pass after implementation for all manual liquidation paths that can create
  a native-XRP liquidator reward claim.
- Tests prove non-XRP `SuccessWithFee` values return no `xrp_claim_id`.
- A multi-claim native-XRP liquidation test proves the returned claim id is the
  liquidator reward claim, not protocol-fee or owner-excess claims created during
  the same liquidation.
- SP native-XRP absorption tests fail before implementation for the existing
  native-XRP rejection and pass only when opted-in depositors receive pending
  depositor-specific claim ids while non-opted-in depositors do not absorb XRP.
- Tests named `xrp_sp_absorb_does_not_credit_icrc_collateral_gains` and
  `claim_collateral_rejects_native_xrp_before_ledger_call` plus
  `claim_all_collateral_skips_native_xrp` prove native XRP cannot enter the ICRC
  collateral-gains claim path.
- Tests named `xrp_sp_preflight_rejects_non_xrp_vault`,
  `xrp_sp_preflight_stores_reservation`, and
  `xrp_sp_absorb_requires_matching_preflight` prove the burn/preflight boundary.
- Tests named `xrp_sp_absorb_replays_same_proof_same_claim_ids`,
  `xrp_sp_absorb_rejects_conflicting_replay_without_mutation`,
  `xrp_sp_absorb_rejects_over_500_allocations`, and
  `xrp_sp_absorb_assigns_dust_to_first_sorted_depositor` prove replay, fanout,
  and dust behavior.
- Candid and generated declarations show `xrp_claim_id : opt nat64`.

### Frontend acceptance

- Tests fail before implementation for missing manual XRP payout-address/tag
  requirements and missing settlement call after a liquidation returns an XRP
  claim id.
- Tests pass after implementation and prove non-XRP liquidations do not require
  an XRP address.
- Tests fail before implementation for an ambiguous manual liquidation response
  that created an XRP claim but returned no `xrp_claim_id`; tests pass only when
  the UI recovers the claim from `get_my_xrp_claims` and offers settlement.
- Tests fail before implementation for live SP card lacking XRP payout-address
  opt-in behavior.
- Tests pass after implementation and prove `native_payout_addresses` is unwrapped
  correctly, `optInNativeCollateralWithTag` is called for XRP, and ICRC claim
  buttons are hidden for native XRP gains.
- Tests fail before implementation for missing manual pending-claim retry UI and
  missing SP pending-XRP-payout retry UI.
- Tests pass after implementation and prove a failed settlement leaves a visible
  retry action using the same claim id, payout address, and destination tag.
- Tests prove SP opt-in stores and renders the optional destination tag, and SP
  pending-payout settlement uses that stored tag.
- Tests prove SP pending payout acknowledgement removes only the connected
  user's own pending record after backend claim absence is verified, and rejects
  outstanding or foreign claim ids without mutation.

### Verification acceptance

- Relevant backend tests pass.
- Relevant frontend tests pass.
- Frontend build passes.
- `npm run check --workspace=src/vault_frontend` is run; any baseline failures
  must be separated from changed-file failures.
- Candid/declaration regeneration is verified by diff.
- Adversarial verification passes for the spec, the implementation plan, and the
  final implementation.
