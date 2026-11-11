# OISY 3pool swap owner repair — source handoff, November 11

## Exact scope

Accepted code: `6e6f32fa955fc464ad20aa40e2d832ec6931cfd8`, branch `codex/oisy-swap-owner-20261111`, based on accepted ckSOL `94ffd5d51b7d12c79f7432e0361bc1654ef6bb5c` and Vault51 lineage `097bf775`. Delta from ckSOL: six frontend files, two runtime source files plus four tests. ckSOL files, backend, Candid, dependency manifests and lockfiles are unchanged. The correction targets direct stablecoin swaps through the Rumi 3pool; other OISY routes retain their existing implementations.

## Cause and evidence boundary

The installed SDK is `@icp-sdk/signer` 5.3.1. It sends the agent's configured account as `icrc49_call_canister.sender`. The old 3pool path captured the wallet before an asynchronous fee lookup and then borrowed a module-global signer without verifying the live signer owner or wallet generation before approval/execution. That allowed an obsolete account or session to reach signing.

The reported message exactly matches OISY's published `SENDER_NOT_ALLOWED` error. The published signer compares the sender to its owner before consent and before execution. The same numeric502/text rejection was reproduced with the real installed SDK against a local mock transport using different expected and wallet owners. Correct cross-library principal construction and outgoing sender text were checked. The initiating state of Rob's actual wallet was not observed; do not present the tested stale-owner mechanism as a captured live session trace.

Primary published sources read November11:

- [Exact error mapping](https://github.com/dfinity/oisy-wallet-signer/blob/main/src/handlers/signer-errors.handlers.ts)
- [Sender check before consent](https://github.com/dfinity/oisy-wallet-signer/blob/main/src/services/signer.service.ts)
- [Consent precedes execution](https://github.com/dfinity/oisy-wallet-signer/blob/main/src/signer.ts)
- [OISY account response uses its authenticated owner](https://github.com/dfinity/oisy-wallet/blob/main/src/frontend/src/lib/components/signer/SignerAccounts.svelte)

## Minimal repair

- Create a fresh action-owned signer/channel with a unique popup name; open synchronously from the swap click before a fee, queue or network await. Keep it open for the action; clean up in finally without masking the financial result.
- Pin the expected principal, wallet provider and session generation. Check liveness before each mutating step and after intervening awaits. Query OISY's accounts before approval and again before swap, require the expected owner, and never silently adopt a different returned account.
- Preserve exact approval cap, spender, input and minimum output. Do not retry on owner rejection or unknown transport results.
- Distinguish rejection before approval from rejection of swap after a confirmed approval. A changed session after accepted swap reports completion for the original account and warns against repeating it.

## Submission boundary and morning action

The particular call rejected by OISY's exact502 handler is rejected before execution. The old generic UI does not identify which of the sequential requests failed: an earlier allowance approval may already exist if rejection occurred at the swap request. No customer allowance, transaction history, balances or swap outcome was verified here. Unknown transport failures remain unknown; they are not relabelled as owner rejection. No approval, transfer, swap replay or wallet operation was performed on Rob's behalf.

After an independently verified publication, the morning user step is to refresh the app, reconnect the same intended OISY account and review the prior attempt/approval history before a deliberate new swap. A real OISY account/consent/swap result remains unverified until that user action. Do not instruct replay from the old unpatched page.

## Verification

- Old code failed the new owner/session binding regressions.
- **62 focused tests passed in four suites:** action-owned real SDK/mock transport7; 3pool binding/stage cases13; existing deposit-policy8; router34. Includes principal/provider/same-owner session replacement during fee lookup, change after approval, owner mismatch before and after approval, exact remote502 at each step, unknown transport without retry, cleanup failure, accepted swap on an old session, exact monetary args, and unchanged Plug/II paths.
- **Both independent final source reviews PASS** exact code6e6f32fa; no confirmed blockers. Both traced the actual click chain through SwapInterface, the false-negative guard and router to synchronous popup opening.
- Project typecheck remains **30 existing errors and66 warnings**, matching the prior baseline; no new diagnostics. An initial extra test-only SDK argument typing error was corrected before acceptance. No clean whole-project typecheck claim.
- Public live swap-service bundle `/_app/immutable/chunks/PMwL7kef.js` independently matched the frozen SEC12 build, SHA256 `8b0bb7620c6490c4ad6fa758d493bd3a1e01d37fcfc4e6898a74b45b5480b635`, confirming the diagnosed old implementation is served. Publisher also verified live frontend version `1794195655486` and root/auth assets.
- Existing ckSOL126-test/two-review/mock-browser evidence remains valid and reused because those source files are unchanged. No duplicate build or financial canary ran.

## Release lane

Dorian owns the next serial frontend build/preparation slot. Compose this source with the accepted ckSOL and Vault51 lineage, preserve the frozen740-file bundle and old stage686, and create a distinct source-matched frontend decision. The existing backend `43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09` is reported Running with the compatible397-method API; the separately held backend guard correction `312ebe03`/packet `097bf775` is not an automatic gate on this frontend-only fix. Do not install any backend or infer publication/funding/financial permission from this handoff. Production build, artifact/inventory verification, publication and actual wallet success are separate proof stages.

Proposed reusable lesson: a principal-keyed cached agent is not proof of the live wallet owner's identity. Pin the action/session, verify the signer owner, and distinguish a signer pre-execution rejection from an uncertain post-dispatch transport failure. This is a catalog-update proposal, not an edit to persona or memory files.
