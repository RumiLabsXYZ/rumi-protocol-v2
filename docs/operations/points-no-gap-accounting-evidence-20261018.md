# Points catch-up accounting: recovered evidence and numerical references

Status: investigation and illustrative compensation references. Processing is
still held. No allocation is sealed, approved, implemented or applied by this
document. This does not authorize an upgrade or override the original request
disposition hold.

## Current preservation proof

The October 18 06:56:28–32 UTC query batch completed all 16 calls successfully.
The original 417 ledger rows, leaderboard totals, held epoch, source cursors,
configuration, policy and revealed historical seed still match the pre-upgrade
baseline. Fourteen comparable response files match byte for byte. Management
status is Running on the original points module; that is query availability,
not processing restoration.

An offline native reader decoded MemoryId 11 in the preserved points snapshot 2
using the exact historical `ic-stable-structures 0.6.7` and CBOR layout. All 15
original epoch 18 A weight records were recovered. MemoryIds 9 and 12 also
recover the actual saved source and asset bindings. The original stable-memory
file SHA256 remains
`8728e2290bc947f9ae38a661248ea5528dc244dfe327a8ebf543cd7099dc4487`
before and after each read. The reader only initializes a RAM copy; no snapshot
restore or canister update occurs. The live asset bindings agree with the saved
bindings, and none of the 15 participants is in the current exclusion list.

## Typed source history

The offline Candid utility checks saved responses against the saved service
return types, encodes them, decodes them and requires equality before writing
full-precision JSON. This catches typing/roundtrip errors; it is not a certified
query or an independent consensus proof.

| Evidence | Result |
| --- | --- |
| 3USD ICRC-3 blocks | All 2,489 blocks, exact contiguous ranges, all parent links match |
| 3USD balance reconstruction | All 15 live participant wallet balances and total supply match; live log length remains 2,489 at 07:08 UTC |
| Complete backend slices after saved cursor | 10,699 events, IDs 512,671–523,369, all typed query/decoder calls succeeded |
| Earlier backend slices | Another 4,000 records, IDs 508,671–512,670; history starts October 4 19:28 UTC, before saved A18 |
| SP history | All 1,896 IDs, exactly contiguous across 19 pages |
| 3pool V2 liquidity history | All 110 IDs in two forward pages, with exact continuation and terminal status |
| AMM liquidity history | All five IDs; current LP queries return zero for all 15 participants |
| Virtual-price diagnostics | 121 retained observations in 30 days, 26 within epoch 19; these are samples, not continuous price history |
| Current participant source reads | 15 vault lists, 15 SP positions and 15 AMM balances, all successful and typed |

The historical 3USD implementation stored balances by owner principal. Requested
subaccounts were preserved in transaction metadata. A preliminary standard
account-tuple reconstruction failed at block 2,251; that failure remains saved.
The owner-keyed reconstruction has no negative balances and its total supply
equals the sum of balances, then matches the live reads. This ledger-specific
model must not be generalized to unrelated ICRC ledgers. Parent-link agreement
has not been anchored to a certified tip in this investigation.

The legacy 3pool V1 history has 18 rows. It is a separate log, not the points
poller's 110-row V2 history. The corrected analysis uses the complete V2 log.
An invalid target principal and an unquoted Candid reserved field label caused
earlier local pre-call failures; those receipts remain preserved. Neither was a
failed production mutation or an uncertain submitted request.

## What the missed-period data says

Epoch 18 is October 5–12 UTC. Its original A exists; original B did not complete.
Epoch 19 is October 12–19 UTC; its secure randomness and A/B captures were not
created. An exact counterfactual epoch 19 award is consequently not determined
by the retained events alone.

During observed epoch 19 history there are 31 backend repayments, one borrow,
33 collateral withdrawals and 33 closes. No liquidation or redemption appears
in that complete slice. The 14 SP events are only collateral opt-ins/opt-outs;
no SP deposit, withdrawal or interest receipt appears in that interval. No
3pool V2 or AMM liquidity event occurs in epoch 19. These are log observations,
not proof that an undocumented implementation could not mutate state without
an event; deployed SP source provenance remains a stated limitation.

One participant's current whole legacy weight is lower than saved A18 after
repayments in the later epoch. Thus a late B is a changed observation time and
must not be represented as the original missed snapshot. The earlier PR583
resume mechanism preserves the old credit period but does not alone prove the
new no-gap requirement. In addition, its epoch 19 rebase omits the time before
restart, as documented in the companion assessment.

## Numerical compensation references, not normal accrual

The private per-principal worksheet is `accounting-reference-ceilings.json` in
the protected points evidence store. It has explicit assumptions and remains
an investigation artifact. No financial/user records are included here.

1. **Epoch 18 saved-A reference.** Under the existing smaller-whole-snapshot
   rule, the ordinary min(A,B) contribution cannot exceed saved A. Scaling that
   saved A over the original seven days, preserving matched 5x and including
   the existing unmatched 3x-to-4x correction formula, yields an aggregate
   **496,571.72870858 points**. This is a generous compensation reference, not
   a reconstructed missing B or an amount already credited. It is **not a
   guaranteed upper bound after correction**: a smaller B can contain more
   unmatched exposure and receive a larger unmatched correction than A. The
   uncorrected A reference is 472,068.07305658 points. A separate generic
   corrected-snapshot ceiling is **629,424.09740874 points**, computed per
   principal as `floor(4 * seven_day_A_total / 3)`. This follows because the
   unmatched top-up cannot exceed one third of the selected uncorrected total.
   It is a mathematical ceiling, not a proposed payout. Repayment-window
   treatment remains separate; all 15 retained principal repayment lists are
   empty, and no missing repayment-asset identity is invented.
2. **Epoch 19 time-scaled illustration to October 18 07:20 UTC.** A worksheet
   uses current debt plus observed later-epoch repayments as a debt reference,
   recorded 3pool composition at the established flat 4x policy, and
   observed SP balances. It excludes all pool weights where verified 3USD units
   across the three venues are continuously zero under the retained-history
   model; a stale nominal deposit is not awarded in that case. AMM contributes
   zero in this worksheet. The result is **594,211.14277139 points** for that
   illustrative interval. This is not an exact historical balance, sealed
   allocation, strict historical ceiling, or approved reward-policy change.
   In exact deployed PR527 `vault.rs::withdraw_and_close_vault_internal`
   (lines 10122–10138) and partial withdrawal (around line 9915), up to
   `DUST_DEBT_THRESHOLD = 50,000` raw debt can be forgiven without a separate
   repayment event. The current-debt-plus-repayments reference does not account
   for that unlogged amount. A final no-underpayment calculation must account
   for it; this worksheet is not that proof. The source reads are about 07:11,
   backend history freezes about 07:17, and the illustrative endpoint is 07:20.
   That final interval is extrapolation, not observed coverage; the calls were
   not an atomic capture. Nonzero LP holders receive a
   full-composition ceiling; historical verification between price samples is
   not established by this calculation.

Any final make-whole policy must explicitly resolve the reference assumptions
and possible overcompensation, preserve the old earned rows and multiplier
cutover, freeze exact inputs and coverage bounds, and use additive durable
per-epoch/per-principal idempotency records. Its missed and future coverage
intervals must be disjoint. These references do not justify silently changing
normal A/B scoring, backdating current balances as historical captures, or
crediting an interval twice.

## Execution and remaining work

The authorized points update was sent once in existing Support case 17015292
after Rob confirmed the login account. A fresh case reload still shows that
sent update without an authoritative original-request disposition. No new
packet was sent. The current tool inventory exposes no applicable approval
queue cancellation/status control. The separate rejected cross-chat handoff
remains unsent and was not relayed indirectly.

The concrete remaining work is to choose and implement the auditable
missed-period allocation, independently review its actual retained-state and
idempotency behavior, obtain original-request disposition, and perform one
freshly revalidated upgrade/replay/reconciliation/resume sequence. The earlier
candidate's timing safeguard remains in force for its original execution path;
this investigation does not bypass it or establish a replacement path.

Only local native evidence utilities were compiled. No production Rust source,
generated declaration, canister, driver, cursor, reward balance, timer or fund
state was changed. No tests were added or run.
