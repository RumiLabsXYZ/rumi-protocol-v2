# Vault 158: recovery evidence and follow-up lessons

Investigation: November 2, 2026. Baseline source: `a71156d644dfa151a40b9008e971054075ae484f`.
This note records read-only evidence and recommended follow-up work. It does not authorize deployment, a transfer, unpausing, or a claim-state change.

## What the receipts establish

Vault 158's bot claim is generation 1, history record 229, in `PaymentShortfall`, with one persisted swap intent and no pending payment journal. The backend still reports the claim active.

| Leg | Ledger block | Amount | Additional sender fee |
| --- | --- | --- | --- |
| Backend to bot | ICP 39835209 | 2.36773835 ICP | 0.0001 ICP |
| Bot to pool | ICP 39835236 | 2.26020933 ICP | 0.0001 ICP |
| Pool to bot | ckUSDC 806697 | 5.545309 ckUSDC | 0.01 ckUSDC, paid by pool |

Input block 39835236 and output block 806697 carry the same pool memo, hex `000000000011be56`. The stored intent precedes both. The input was read through the official ICP index and then directly through the ledger-advertised archive `q3fc5-haaaa-aaaaa-aaahq-cai.get_blocks`; the output was additionally checked directly against the ckUSDC ledger. Both account histories show no later movement at investigation time. The ckUSDC balance reconciles exactly: 26.190995 before the swap + 5.545309 receipt = 31.736304 now. These are dated observations, not permanent spend authorization.

The debt amount is 5.13841815 icUSD. The protocol's upward conversion to six decimals requires a 5.138419 ckUSDC payment. With the observed 0.01 ckUSDC fee, the claim consumes 5.148419 and retains 0.396890 ckUSDC surplus. The actual remaining claim ICP is 2.36773835 - 2.26020933 - 0.0001 = 0.10742902 ICP, before any treasury transfer fee. Unrelated account balances must not subsidize either leg implicitly.

## What failed, and what remains uncertain

The swap produced a ledger receipt. The bot's historical zero reservation is therefore not proof that the swap failed or that funds disappeared. The initial post-swap observation did not recognize the proceeds. Its precise timing/cause remains unproven; the balance read is an inter-canister call, so calling it a stale frontend query would be unjustified.

The persistent failure is established in source: `reserve_measured_post_swap_delta` and `write_short_payment_recovery` in `src/liquidation_bot/src/process.rs` can leave a `PaymentShortfall` claim without a payment journal. Existing retry and short-payment top-up methods require a payment journal or an original short-payment receipt. Neither resumes this state. `process_pending` also returns whenever a held claim remains, stopping new queued liquidation processing.

The safety checks did useful work: no unrelated balance was silently allocated to debt, no unproven debt reduction occurred, and the backend did not release collateral already sent out merely because a timeout elapsed. The missing component is a supported path from uncertain observation to verified settlement.

## Recovery scope

Recover the existing claim from exact verified receipts, preserve their association durably, and create one claim-bound payment attempt using the existing confirmation mechanism. Never reswap the same collateral or treat the pool-to-bot receipt as the bot-to-backend payment proof. Persist the exact outgoing tuple before dispatch and preserve ambiguous outcomes across retries/upgrades. Account for the separately charged ICP swap fee explicitly; do not change historical swap amounts to conceal a fee adjustment.

Installation alone must not initiate recovery. A subsequent explicitly authorized operator action must be necessary. After settlement, read back the backend claim, debt/collateral, bot history/journals, balances, surplus and treasury obligation. A completed partial liquidation does not imply the remaining vault is healthy.

The bounded implementation separates evidence preparation from execution, requires the worker to remain paused during execution, and restricts continuation to this payment and treasury record 229. Explicit execution includes the disclosed treasury obligation; it does not authorize starting the ordinary queue. Generic no-effect retries must not replace this recovery's immutable tuple outside its recorded lifecycle. Completion must consume both the backend claim and the bot's original local claim, while retaining the recovery evidence for audit/replay protection.

## Systemic follow-ups (not gates on the bounded recovery)

1. **V158-F1: receipt reconciliation after an inconclusive balance observation.** Separate `AwaitingSwapReceipt`, verified insufficient output, and an actual outgoing payment failure. Use bounded read-only reconciliation of the persisted swap attempt before declaring an economic shortfall. Never use a delay alone as proof, blindly repeat the swap, or authorize another swap from collateral whose first outcome is unresolved. A generic solution needs reliable pool-operation correlation and receipt reuse protection, not just sender/time matching.
2. **V158-F2: actionable status and alerts.** Expose sanitized claim phase, age, last observation, verified receipts and the next permitted action. Distinguish zero newly attributed proceeds from a zero wallet balance. Report whether a held claim is stopping the bot queue. Alert once on meaningful transitions or overdue reconciliation; do not add noisy periodic messages. Existing explorer code reconciles historical failures with active backend claims, but `fetchActiveBotClaimVaultIds` currently maps read errors to an empty set; an unavailable read should display unknown rather than make a live claim disappear.
3. **V158-F3: explicit fee and reservation accounting throughout the bot.** Extend the exact recovery accounting to ordinary swaps and bounded top-ups. Track input amount, sender fee, net output, payment debit, residual ICP and held surplus as distinct values. Add a conservation assertion per asset. At the baseline, treasury sizing in both normal and resumed payment paths subtracts swap input amount but not its separate fee; changing the general fee policy needs a scoped decision rather than silently borrowing shared float.
4. **V158-F4: isolate held claims without weakening custody.** Consider allowing unrelated claims to proceed only after per-claim asset reservations and scheduling are proven independent. Removing the current early return is not a safe standalone fix while accounts and recovery obligations are shared. This is deferred architecture work, not a prerequisite for resolving 158.

Suggested order: finish bounded recovery, then F1/F2 together, then F3's general accounting cleanup. Evaluate F4 only if throughput warrants the additional machinery. These follow-ups are recommendations, not newly imposed release gates.

## Verification lessons and proposed catalog addition

Test the boundary that failed: a swap call reports output while the first balance observation reports no increase; the exact receipt becomes available later; restart/repeated reconciliation still produce at most one payment. Also test a mismatched/reused receipt, an ambiguous payment reply, and preservation of pre-existing balances. A test that only confirms that zero delta causes a hold verifies safety but misses whether the operation can ever finish.

The existing `reservation_pool_underdelivers_uses_delta` test explicitly describes a May 18 vault 69 incident with router-reported output and zero observed wallet delta. That source comment is historical context, not fresh verification of vault 69. It demonstrates that this observation pattern was anticipated; the missing coverage is its subsequent reconciliation and completion, not another assertion that zero observed delta must not be spent.

Use actual ledger wire fixtures for recovery tests. This incident's native ICP input has a pool `spender`, a zero legacy numeric memo and the correlation bytes in `icrc1_memo`. The index exposes absent transaction `created_at_time`, while the native archived representation supplies the block time in that field. Do not infer deduplication parameters or reject a real delegated transfer from a synthetic direct-transfer fixture. The history record must likewise remain a fixture with its observed zero proceeds; the subsequently proven output is separate evidence.

Proposed durable persona/catalog guidance (not applied to global memory): every custody-related held state should document its evidence source, permitted reconciliation transition, responsible operator, and replay invariant. Reconciliation should be a first-class lifecycle operation. Historical error labels, aggregate balances, and timeouts are observations, not settlement proofs.

No claim is made here that the precise initial observation defect has been reproduced, that the new recovery code is deployed, or that vault 158 is financially settled.
