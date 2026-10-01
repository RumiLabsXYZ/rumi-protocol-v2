# Native-XRP reservation patch proposal

**Status:** proposal only. No repository source files were changed and the rejected write was not retried.  
**Source anchor:** `7f7485ec26ae8211334327c11704c32a16b9893b`.  
**Patch artifact:** `xrp-reservation-proposal.patch` (review-only unified diff for `src/rumi_protocol_backend/src/vault.rs`).

## Decision requested

The audit found a concrete timing gap: after an XRP Stability Pool burn, an unresolved backend preflight remains acceptable for submit after its TTL, while vault mutations become allowed at TTL. If the owner repays/closes or a permissionless liquidator changes the position during that gap, the backend correctly rejects a stale debt/collateral write-down, but the already-burned SP value has no recovery path.

The proposed conservative repair treats a persisted preflight as unresolved until consumed or explicitly released. It closes expiry as a mutation authorization but creates a liveness cost: if the SP loses its intent, cannot restart, or does not call release, the XRP vault stays blocked. The current SP release path can unblock failures known to happen before burn; there is no proof-based endpoint to recover a lost or ambiguously committed burn. Rob should decide whether to accept this temporary/permanent lock risk or authorize a broader proof-based reconciliation contract before this proposal is applied.

## Exact proposed behavior

1. In `stability_pool_preflight_xrp_absorb_in_state`, after caller/freeze/disable checks and before new sizing:
   - Same registered SP caller plus same vault and burn amount returns the persisted preflight snapshot unchanged, including original collateral output, price, and expiry. This supports a retry after the preflight state write succeeded but its response was lost.
   - A different caller or burn amount for that vault is rejected. It cannot replace the unresolved reservation with new economics.
2. `ensure_no_active_xrp_sp_absorb_preflight` blocks vault debt/collateral mutations whenever a reservation remains, regardless of its timestamp.
3. `matching_xrp_absorb_preflight` continues to allow a post-burn submit from an expired-but-present reservation. Existing current-debt and current-collateral conservation checks remain unchanged.
4. The existing registered-SP release endpoint remains the only manual clearing path. The Stability Pool currently calls release only from `abandon_unburned_native_xrp_absorb`, for pre-burn failure branches, and clears local intent only while status is `Prepared`, proof is absent, and backend result is absent.
5. Successful absorb still consumes/removes the reservation. The expiry timestamp is retained for response compatibility but is no longer a backend mutation unlock.

The proposed patch deliberately adds no state field, Candid method, or generated declaration. It edits only the backend helper and focused unit tests.

## Existing pre-burn release evidence

- Backend release is caller- and amount-bound to the registered SP in `vault.rs:3260-3282`.
- SP `abandon_unburned_native_xrp_absorb` clears only a prepared intent with no burn proof, then calls that release endpoint: `stability_pool/src/liquidation.rs:417-445`.
- Preflight response mismatch, no payout allocation, allocation error, minting-account fetch error, and the burn call error route through this pre-burn abandon flow at `liquidation.rs:1103-1242`.
- Once a proof is saved, backend submission errors preserve `Burned` or `BackendRejected` state and retry; that path does not release: `liquidation.rs:953-1015`, `1028-1049`.

A caveat remains: the code treats an icUSD ledger burn call error as unburned (`liquidation.rs:1230-1242`). This proposal does not prove IC inter-canister reply-loss semantics or change that classification. It therefore preserves existing behavior rather than claiming a no-proof outcome is universally certain. A proof-based reconciliation contract should include this ambiguity.

## Liveness and recovery tradeoff

- **Known pre-burn error with live SP:** the SP calls release; mutations resume, including after TTL.
- **SP holds a proof-bearing intent but is offline:** mutation remains blocked until the SP resumes and submits, or the registered SP explicitly releases. The existing SP code should never release after proof persistence.
- **SP state loss, unresolved preflight response, or ambiguous burn-call outcome:** same amount retry reuses the stored sizing snapshot. A changed burn amount is rejected until release. There is no backend method that can prove an old burn was absent, reconcile a proof-bearing intent, refund/reissue SP value, or safely release a potentially committed burn. The vault may remain unavailable.
- **No claimed TTL guarantee:** TTL no longer bounds this lock. The proposal is conservative for accounting, but it is not a complete availability/recovery solution.

## Proposed regression tests

All state-level tests belong in `src/rumi_protocol_backend/src/vault.rs` inside `xrp_sp_absorb_contract_tests`:

- Replace `xrp_sp_active_preflight_blocks_vault_mutations_until_expiry` with `xrp_sp_preflight_retry_reuses_reserved_snapshot_and_rejects_mismatch`: exact caller/amount retry after expiry returns the original struct; changed amount is rejected and stored economics remain identical; mutation guard stays closed.
- Extend `xrp_sp_absorb_submit_honors_expired_preflight_after_burn`: assert the expired reservation still blocks mutations immediately before submitting the original request/proof; then assert the absorb succeeds and consumes the same reservation.
- Extend `releasing_unburned_preflight_unblocks_vault_operations`: after the expiry time, confirm the unresolved preflight still blocks; use the existing registered-SP release call; confirm the block clears and a second release is idempotent.
- Keep existing negative/conservation coverage: `xrp_sp_absorb_rejects_when_vault_debt_below_reserved_burn`, `xrp_sp_absorb_rejects_when_vault_collateral_below_reserved_seizure`, and proof replay tests.

Existing SP journal tests in `src/stability_pool/src/liquidation.rs` that establish the counterpart behavior:

- `xrp_absorb_submit_failure_after_burn_retries_exact_request_to_success` at line 3316: retry submits the stored proof/request without preflight or a second burn.
- `xrp_absorb_invalid_backend_result_keeps_burned_intent_retryable` at line 3397: invalid response retains proof-bearing intent and a later retry submits again.
- Existing no-proof release helper is at `liquidation.rs:430-445`; there is no current named test composing backend expiry, vault mutation rejection, and successful original proof absorption across the canister boundary.

The proposal was not compiled or executed. Tests listed are proposed; current component tests were only read during the audit.

## Native-chain sibling comparison

- **Generic chain SP path:** `main.rs:3928-3975` and `6819-6863` store a 15-minute `StoredChainSpAbsorbPreflight`; `matching_chain_absorb_preflight` requires an exact caller/burn match and an unexpired timestamp. On expiry, `stability_pool_liquidate_chain_vault` falls back to re-deriving a current snapshot at `main.rs:6722-6755`, then `apply_sp_chain_liquidation_absorb_in_state` requires the burned amount to equal live debt at `chains/evm/settlement.rs` (state transition) and records `pending_chain_burn_e8s` plus a `ChainLiqClaim`. The gate is therefore different: it can reject a proof after expiry if current debt/liquidatability no longer matches, but this is an EVM-backed external-chain liability/pending-burn/claim accounting rail, not XRPL native custody and XRP payout claims. This patch is intentionally scoped to the audited XRP backend boundary; a chain-absorb outage/mutation/retry audit should be separate before adapting the rule.
- **Native SOL:** no native-SOL SP preflight/burn-proof absorb endpoint was found. Solana code is a separate developer-gated, Devnet-configured rail; the legacy/native SOL adapter finding remains an activation/accounting question rather than a sibling implementation of XRP's reservation.

## Auto-review outcome

Auto-review rejected the proposed source mutation and stated: “The patch changes a shared vault guard from expiring to indefinite reservation locks, potentially causing broad irreversible service denial if the Stability Pool loses its intent or cannot release it; the user authorized a security review, not this exact high-blast-radius recovery policy.” The rejected source write was not retried through another path. This file and the adjacent patch are review-only artifacts, not an attempt to bypass that decision.

## Alternative requiring a broader contract

A proof-aware reservation protocol could add explicit states such as `Prepared`, `BurnCommitted(proof)`, `Accepted`, and `ProvenNotCommitted`; heartbeat/pin only after durable burn proof; and an authenticated reconciliation method that lets the backend safely resolve a proof against vault changes. It must answer ambiguous ledger-call outcomes and provide an operator-visible recovery path without permitting an unbacked write-down or releasing a committed burn. That requires state evolution, Candid interface changes, SP/backend coordination, upgrade compatibility, and integrated tests. No part of that broader design is implemented here.
