# Vault202 manual-queue diagnosis and bounded UI candidate

2026-11-14. Adrian owns the vault/claim incident and financial recovery diagnosis. Ellis owns this frontend source correction and verification. SEC20 remains completed; this is a separate incident. No publication or financial recovery authority is inferred from the report.

## Confirmed cause

The manual queue's paged backend query selects by collateral ratio and excludes pending nICP claims, but does not exclude `bot_processing`. Its `CandidVault` projection does not include that lock. The frontend normalizes every returned row and previously enabled execution based on connection, amount and local processing state alone. A refresh therefore does not reliably remove a bot-locked vault. OISY deliberately skips a new pre-click query to preserve its signer gesture window; the backend correctly rejects a second seizure.

The installed API already exposes public `get_bot_claim_vault_ids`, returning current protocol claim IDs rather than the bot's permanent failure history. Adrian's captured readbacks show202 in both that set and the liquidation queue. His bot record231 is `TransferFailed`, with95482 raw ckUSDC received,110908 required gross, and0 paid. These readbacks support a stalled protected claim, not an actively progressing liquidation. Financial assets, shortfall and proof-backed recovery remain Adrian's accountable investigation. Clearing the lock alone is not a recovery.

## Minimal frontend remedy

Based exactly on live SEC20 source `dc4b8f52a1e76a36791defb5f66005276d00d4a5`:

- Read the existing public claim-ID query alongside the queue. Keep reserved rows visible with **Reserved for bot liquidation** guidance and disabled manual controls. Use muted Reserved/Unavailable/Checking labels; preserve Approving/Liquidating labels during an actual in-flight action.
- Disable manual actions while availability is loading or unavailable, and guard before any approval/liquidation dispatch. No added pre-signing await for OISY.
- Remember an explicit backend lock rejection. A new rejection invalidates an older status read; only a fresh successful availability read can clear that local UI hold. This changes no backend lock or claim.
- Preserve existing XRP payout-claim settlement retries independently of manual-liquidation availability. Rename the summary to at-risk positions and avoid claiming every position is healthy when the queue is empty.

No backend, Candid, generated declaration, dependency, policy or wallet-service changes. Only `ManualLiquidations.svelte` plus a new mounted regression suite.

## Verification and boundaries

Red regressions reproduced the missing reserved state, missing failure hold and repeated action invitation. Final11 checks pass:5 mounted availability tests and6 retained XRP pending-claim checks. Mounted cases cover typed-array claim IDs, read failure, normal unlocked action, later lock rejection, explicit refresh, disabled loading, and older-read-versus-new-rejection ordering. Two independent reviewers returned final PASS after a concrete label-priority finding was fixed and asserted in the race test. `git diff --check` passes.

Actual source components rendered with mocked public queries and wallet at1280×900 and390×844 show the reserved row, disabled controls and readable status without horizontal overflow. Failure mode keeps every manual button disabled. All fixture financial-call counters stay0. Screenshots reflect the same static reserved/unavailable presentation as the final source; final label-order correction only affects already-processing actions, covered by the mounted race test. [Evidence](evidence/manual-liquidation-availability-20261114).

Availability is a snapshot, not a reservation: the backend remains authoritative for a claim acquired after the read. An orphan `bot_processing` flag without a claim ID is not exposed by this public query and is learned from the backend rejection. Adding a complete public lock projection is possible future work, not a required gate for this confirmed202 mismatch.

This is a verified **source candidate**, not a production build or deployment. No liquidation, allowance, force unlock, wallet signing, claim retry, top-up or funds movement occurred. Future publication needs its own scoped release authorization, fresh build allocation, exact artifacts and served-byte/layout verification. Adrian's separate claim recovery needs its own concrete proof and financial authorization.
