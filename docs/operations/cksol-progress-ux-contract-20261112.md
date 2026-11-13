# ckSOL progress and completion UX — scoped contract

Owner Ellis. Iris implements scoped UI; independent frontend/service tracing reviews evidence mapping. Existing customer-case board SEC18; no duplicate project or release. Rob asks for reassuring mint progress and unambiguous success. The supplied screenshot shows official Minted evidence while waiting-for-another-deposit copy dominates; do not invent a new stuck-mint incident.

## Accepted behavior

- Prominent panel near the mint action. Persistent, truthful progress with spinner, last successful check and automatic-check/pause/action guidance; no invented percentages, chain stages or ETA.
- Only exact tracked deposit ID plus official Minted amount/block proves mint success. Show exact ckSOL amount, destination ICP account and receipt/details; success remains dominant until explicit Start another deposit.
- Active requests take precedence over previous completed receipts. Correlate owner, payment mode, local journal/sequence and deposit ID; receipt objects do not themselves carry owner fields. Restore only the same owner/request and discard stale session callbacks.
- Payment/refund settlement is distinct from mint success. RefundReady or Complete can follow accepted, rejected or uncertain processing; never upgrade these alone to minted. Pending refund may remain visible separately after a confirmed mint.
- Preserve adaptive15-to60-second read polling, in-flight deduplication, background pause, focus resumption and Check again. Network failure/slow/unknown outcomes remain honest and held; no automatic financial retry or new payment.
- Preserve signer timing, wallet consent, Web Locks, local journals and every financial guard. Observation callbacks are UI-only and must not change protocol control flow.

## Verified source mapping

| Display meaning | Current source evidence |
| --- | --- |
| Wallet confirmation | Local Approving immediately before token approval; signer-channel readiness alone is not approval. |
| Submitting request | Local Starting saved before start_* update; backend acceptance remains unconfirmed. |
| Backend processing | Owner-scoped matching receipt; Funded confirms collection. DepositPending marks dispatch before the minter callback, not official acceptance. |
| Mint processing | Receipt deposit_id joined to that exact ID’s Queued/Swept/Finalized. |
| Mint confirmed | That exact ID’s Minted, including minted_amount and block_index. |
| Processing payment settled | Complete only; no mint claim. |

Sources: /solana/+page.svelte; CksolIcpPaymentPanel.svelte; cksolPayment.ts; cksolIcpPayment.ts; cksolMinterFlow.ts; cksolReadPolling.ts; backend cksol_paid.rs and cksol_icp_paid.rs. Independent tracing read current code; no network or financial call required.

## Validation and release boundaries

The flow under test is: /solana with a synthetic owner → wallet/submission/processing evidence → delayed mint or honest unavailable status → exact Minted confirmation → persistent success until Start another deposit. Essential source/mock cases cover delayed mint, settled payment before mint, stale completed receipt plus fresh request, network error and resume, owner/request restoration and success on desktop/mobile. No real mint, wallet signing or customer data.

Browser plugin/skill is not available; use regular Playwright for the temporary local mocked renderer, with existing dependencies only. Keep screenshot and temporary browser scripts outside Git. Reuse existing mounted page fixtures. Source, local mock rendering, production build and deployed behavior must remain separate claims.

Branchcodex/cksol-mint-status-20261112 is separate from SEC17 exact00cf release. No source swap or automatic bundling into its approved artifact. Backend/generated/dependency/repayment files are outside this task. No deployment approval is inferred. Essential focused tests/local rendering only; no broad audit/test campaign or competing heavy build.
