# October points outage: proposed final allocation method

Status: proposed compensation; unapproved, unpinned and unapplied. The October19
recovery resume succeeded. Actual capture, close and normal reopening remain
separate runtime observations. No missed-credit completion is claimed.

## Exact coverage

Preserve the original417 entries, whose domain-separated Candid prefix digest is
`9e9f4594e29ec2cb6223f247a03ad78b816f1d291829982573edada585d0a24d`.

| Row | Covered interval | Ordinary-credit offset |
| --- | --- | --- |
|18 | October5–12 UTC, whole scheduled epoch | All actual18 awards and legacy corrections |
|19 and later fully missed epochs | Each whole scheduled epoch | Any actual ordinary awards for that epoch |
| Actual reopened epoch | Scheduled start through immutable actual opening | None from the following normal interval |

Include the full census-participant by affected-period matrix, including
explicit zeros for exclusions and registration at or after the covered interval’s end. Refresh the census and retained
source tails through actual opening before sealing. No fabricated snapshots,
backdated opening, missing-epoch summary or invented seed reveal is allowed.

## Proposed generous targets

Missing random captures do not determine a unique historical award. These
proposed conditional targets can exceed an uninterrupted program's award and change
final pro-rata airdrop shares. They do not expand or transfer the token pool.
Upgrade-risk acceptance alone does not approve this economic choice.

- **18:** per participant, use `7*A + floor(7*A/3)`, with A the original saved
  total A18 weight. Whole-snapshot min cannot exceed A, and the unmatched
  correction adds at most one third. Separately valid repayment windows are
  separate; retained original windows are empty. Do not invent5x windows from
  ordinary icUSD repayments.
- **Later intervals:** separately floor each source's
  `weight * exact_covered_nanoseconds / nanoseconds_per_day`, then sum.
- **3pool:** full original recorded composition plus positive qualifying adds,
  ignoring removals: `icUSD + 4*(ckUSDC+ckUSDT)`. Verification cannot increase a
  recorded leg. This generous cap avoids inventing historical virtual prices.
- **SP:** actual A18 SP balances plus positive events from no later than A18,
  including event1869 before the saved cursor. Include actual3USD minted by
  deposits and positive corrections; ignore decreases. Give each participant
  the whole observed global-interest allowance. Weight icUSD1x and3USD2x.
  The first retained event does not establish zero-balance genesis.
- **Debt:** later observed owner debt plus all recorded repayments from
  epoch19 start through the final history freeze, including repayments after
  the covered endpoint, and all applicable public debt-loss allowances; add global dust and consumed icUSD-burn allowances to
  each participant. Current observed bounds are `50,000*(212+1)=10,650,000raw`
  dust and167,588,756raw burn allowance each. Refresh after final position
  reads. Unexpected debt-loss, creation or correction paths fail closed.
  Generic3USD writedowns emit public debt losses; never assume VP<=1.
- **AMM:** original A18/current LP balances and retained history support zero
  for the original cohort. Refresh through the cutoff; handle additions and
  newly discovered participants explicitly.

The debt-side bound relies on nonnegative accrual, visibility of debt reductions in inspected source, and retained lifecycle evidence. It is bounded operational evidence, not an exact counterfactual proof; historical SP provenance remains a disclosed limitation.

## Application and verification

Whole-epoch credits are `max(target-ordinary_earned,0)`. The reopened epoch's
prefix target is separate from future normal earnings. Never reduce an earned
excess. Bind every row, exact interval, input hash, assumption, integer target
and the original prefix into a reviewed numerical manifest and Candid digest.

The deployed recovery release has pinNone and cannot apply compensation. A
further narrowly reviewed release must pin the final allocation; the already
used authorization for one upgrade does not authorize that release implicitly.
Stage/apply through the bounded mechanism, with atomic total updates and durable
OutageMakeWhole receipts. Verify every participant/period receipt, totals and
idempotency before claiming missed points credited.

## Preview, not a sealed allocation

Original15 cohort target18:629,424.09740874points. Proposed full19 target from
later observations:712,257.48451886points. These are targets before offsetting
ordinary awards. The worksheet covers through October19 19:38:36 UTC with later
loss/proof checks; it does not seal the eventual reopening prefix. Final numbers,
allocation/evidence digests and release identity follow actual opening and
independent numerical review. Do not apply this preview.
