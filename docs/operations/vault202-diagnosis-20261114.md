# Vault 202 liquidation diagnosis — November 14, 2026

Owner: Adrian, existing liquidation recovery lane. Investigation only: no
unlock, cancellation, reconciliation update, pause/unpause, liquidation,
deployment or financial call was performed. The separate Vault51 and other
recovery holds remain unchanged.

## Finding

Vault202 is held after a completed swap produced insufficient **net** ckUSDC
for its required repayment plus ledger fee. It is not an actively progressing
liquidation. At 21:04:55 UTC (13:04:55 Pacific), bot record231 attempted claim
generation3. Fresh repeated query reads return `TransferFailed` in history and
`PaymentShortfall` in the durable claim journal. The backend reports exactly
one open bot claim, vault202. Processing is unpaused, but this held status has
no automatic continuation and `admin_retry_stuck_claim` deliberately refuses
a claim without an accepted payment proof. No repayment journal, collateral
return or treasury intent exists for this claim.

The bot is Running module
`256dc8101b0e0d616bb7c660b17168a8ad11d146869802bb483849c08875359e`.
The backend is Running
`8f6f5933e54fa44205760b9a8d1c3869135df43825eadfee70b20c62d0a0bd16`,
the separately verified SEC19 release. The bot's installed artifact is the
November7 source-matched fee release, not the older local primary checkout.

## Verified asset trail

Direct ledger block queries independently corroborate the account indexes:

| Movement | Block | Amount and fee |
| --- | --- | --- |
| Backend sends claim ICP to bot | ICP40431390 | 5,142,859e8 received; 10,000e8 outbound fee; gross claim5,152,859e8 |
| Pool pulls ICP from bot | ICP40431407 | 4,918,638e8 input plus10,000e8 input fee |
| Pool sends ckUSDC to bot | ckUSDC815252 | 95,482e6 received plus10,000e6 pool withdrawal fee |

The two swap legs share pool memo `0000000000126b37`. Principals/default ICP
account identifiers were independently derived and matched to backend, bot
and pool. The frozen pre-swap ckUSDC balance26,621,238e6 plus the receipt95,482e6
equals the observed bot balance26,716,720e6 exactly. The last bot-account
transaction is that receipt; there is no subsequent outgoing repayment.
These are ledger/query observations, not a claim of independently validated
certificate hash-chain inclusion.

The remaining claim-owned ICP is
`5,142,859 - 4,918,638 - 10,000 = 214,221e8` (0.00214221ICP).
The rest of the bot's observed40,884,033e8 ICP and26,716,720e6 ckUSDC must not
be assigned to this claim merely because they share its wallet.

Claim debt is10,090,702e8 (0.10090702icUSD). Exact minimum net ckUSDC repayment
is `ceil(10,090,702 / 100) = 100,908e6`; payment fee is10,000e6. Required
gross is110,908e6, against95,482e6 verified claim output: a **15,426e6
(0.015426ckUSDC) shortfall**. No payment was sent, as intended by the guard.
The public vault debt at the first query is10,090,752e8; do not equate the
fixed claim write-down with the vault's subsequently accrued total debt.

Backend proof-journal tail scanning reached its actual end and corroborates
generation3 reservation with no consumed generation3 payment proof. The
preceding generation2/vault216 completed successfully with its exact receipt;
this is a distinct small-amount economic failure, not proof that the previous
fee fix failed or that Vault158 regressed.

## Why the bot became stuck

[Installed bot source](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/2edd5e01b9501d2349265674ec11f0aaa22add49/src/liquidation_bot/src/process.rs)
sizes the initial input from oracle debt value with a5% buffer
(`calculate_swap_amount`), without targeting both fixed ckUSDC fees. The
prepared swap minimum is a slippage-adjusted **pool quote**, not the net amount
that must be spendable after the pool's output fee. Here that minimum103,373e6
already falls below required repayment gross110,908e6; the actual bot receipt
is95,482e6 after the pool withdrawal fee. The observed gross pool output is
105,482e6, consistent with the recorded effective price, then10,000e6 is lost
to that withdrawal fee. The remaining collateral's approximate value at the
claim price is only0.0046ckUSDC, before another swap's costs, and cannot fill
the0.015426ckUSDC gap. The bounded top-up correctly did not spend it on an
insufficient second swap. PaymentShortfall then preserves the claim for
explicit recovery instead of silently drawing from unrelated wallet funds.

## Why manual liquidation is still offered

Both deployed `get_liquidatable_vaults` query forms filter economic collateral
ratio without excluding `bot_processing`. `CandidVault` omits that lock field.
The public query returned vault202, while the execution entrypoint's
`reject_if_bot_processing` correctly rejects a second seizure. The actual raw
flag is not exposed by these query interfaces; the lock is evidenced by the
open backend claim and reported manual-entrypoint rejection, not by pretending
the public projection contains a raw flag. Ellis owns the separate minimal
queue/status correction. Keep the execution guard intact.

## Rob's pre-funded USDC buffer

Rob's follow-up correctly identifies a funded buffer. Ledger block630116,
retrieved through the ledger's exact archive callback, records20,000,000e6
(20ckUSDC) from Rob's account to the bot on May20 at01:37:07 UTC (May19
18:37:07 Pacific). The current bot balance is26,716,720e6. Contiguous account
index pages cover every subsequent transaction:85,855,264e6 incoming pool
credits minus83,591,703e6 gross outgoing debits produce a2,263,561e6 net
increase since the funding transaction. The implied balance immediately after
that deposit is24,453,159e6. No further external credit appears in that interval.
The funds are fungible; this arithmetic does not individually tag the original
20ckUSDC or classify every legacy liability as free reserve.

The deployed `BotConfig` and `BotState` contain **no automatic buffer/reserve
enable flag, reserve budget, per-claim subsidy cap or reserve-allocation
ledger**. This is not a disabled switch or empty balance. In
`compute_swap_reservation`, spendable output is bounded by the measured
post-swap delta and router result; the26,621,238e6 pre-existing balance is
explicitly excluded. PaymentShortfall never allocates from that old balance.
The observed difference26,716,720-26,621,238=95,482e6 exactly corroborates this
path. Aggregate surplus tracking is accounting-only and explicitly says it
is not reused for later claims. The residual top-up endpoint applies to an
existing short payment receipt, not this pre-payment held claim.

**No additional funding is presently indicated.** The implementation lacks
the controlled reserve-use path Rob expected. A bounded reserve policy should
keep reserve allocations separate from claim proceeds and older held69/89/169
records, enforce a per-claim and total cap, durably allocate before dispatch,
and account for fees exactly. Do not fix this by removing per-claim accounting
or treating all26.716720ckUSDC as unencumbered funds. Historical deposit receipt
and Rob's intended purpose explain the product gap; they are not evidence that
such a policy was implemented, enabled or automatically exercised.

## Recommended bounded recovery and actual decision boundary

Finish the already-executed claim rather than clear its lock or attempt a
second liquidation. Prepare a generation3-bound recovery continuation that:

1. Verifies record231, the exact claim/memos, the three receipt blocks, current
   fees, absence of any payment/return/treasury journal and the measured output.
2. Allocates exactly15,426e6 from the **already-funded buffer**, after confirming
   that portion is unencumbered and binding the allocation durably to this
   claim. No new funding is proposed. The exact remedy approval must cover
   controlled reserve spending; investigation authority does not lift the
   financial hold or make the rest of the shared wallet spendable.
3. Freezes one payment tuple for100,908e6 net plus10,000e6 fee, preserving the
   exact vault202/generation3 memo. Persist before dispatch, reconcile unknown
   outcomes by exact receipt, and confirm only through proof-bound backend
   confirmation. No short payment followed by speculative retry.
4. Reuses the ordinary residual-ICP journal/recipient/fee procedure after
   successful confirmation; it may allocate only214,221e8 gross residual
   (204,221e8 net if the live10,000e8 fee is unchanged). Preserve any accrued
   residual debt and do not claim this automatically closes the entire vault.

The installed one-off Vault158 recovery methods cannot safely be reused by
changing their input: their source is intentionally pinned to158/generation1
and receipt229. Generic residual-top-up methods also require a prior payment
receipt, which this claim does not have. A small source change is required for
this continuation. This document is a concrete diagnosis and implementation
handoff, **not a ready execution packet or request to approve an unbuilt
recovery**. The eventual exact release/financial decision must identify the
reviewed implementation, bytes, allocation source and frozen call tuple before
any action; no financial approval is inferred from investigation authority.

To prevent recurrence, prepare fee-aware bot admission/swap sizing that includes
pool withdrawal and backend payment fees, respects actual claim-owned input
and all ICP fees, and defers uneconomic small liquidations to ordinary manual/
Stability Pool handling **before seizing collateral**. Exercise this exact
small-claim fixture and retain existing per-claim accounting/proof guards.
Add the bounded reserve-spending behavior Rob intended, rather than leaving
the existing buffer inaccessible to ordinary fee shortfalls. This recurrence
fix and the queue correction are separate from authorizing the already-held
claim's financial resolution.

## Evidence and limits

[Structured diagnosis](evidence/vault202-diagnosis-20261114/diagnosis.json) records
the integer arithmetic and observed identities. The
[evidence manifest](evidence/vault202-diagnosis-20261114/raw-evidence-manifest.json)
pins retained private raw query/interface/log responses without publishing
customer account records or private filesystem paths.

Bot controller logs contain no incident-time entries, and its `http_request`
query is not exported; neither gap is treated as evidence of no activity.
An attempted backend filtered-event scan exceeded its instruction budget;
it was not retried and is not reported as an empty result. No source build or
test suite was run for this read-only diagnosis. The deterministic arithmetic,
same-state repeated claim reads, explicit journal state, exhaustive recent
account interval and three direct ledger receipts support the findings above.
Guarded worktree cleanup removed zero: all existing worktrees were protected,
dirty, missing or unmerged. The new evidence-only sparse checkout contains no
build outputs, dependencies or production source changes.
