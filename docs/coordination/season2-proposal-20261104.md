# Season 2: November pilot proposal

Orin → Merritt, November 4, 2026. **For review; no terms, awards, funding or activation approved by this document.**

## Recommendation

Propose a small November points pilot to retain borrowing and existing Rumi liquidity, with the inherited rate table below. Keep all Season 2 accounting separate from finalized Season 1. Use **November 1, 2026, 00:00 UTC** as the already-recorded intended retroactive start and propose an exclusive end of **December 1, 2026, 00:00 UTC** (through November 30). This is a suggested one-month review period, not a previously agreed duration.

Choose the zero-spend, points-only option now. Describe points as a record of qualifying participation, with no guaranteed RUMI conversion, allocation or payout. Preserve the possibility of backdated recognition; do not promise exact awards for an interval whose observations are missing. Token launch/claims and 3pool promotion remain parked.

## Already established versus proposed

- The retained November 1 Season 1/2 discussion records the intended November 1 UTC start, subject to usable records and later rules. It did not approve numerical Season 2 awards.
- Prior finalization evidence records Season 1 points finalized, receipts preserved and both scoring timers disabled. This is historical evidence, not a new live-state attestation. No Season 2 activation or token claims followed from it. [Season 1 finalization source](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/608).
- The inherited fiat-stable rule distinguishes legacy matched 5×, unmatched 3×, historical uplift, and future flat 4×. The older 10× report label was corrected on main. None of these sources automatically establishes Season 2 terms.
- Rob parked token/SNS/DAO/claims and 3pool promotion. Their resumption is not part of this recommendation.

## Proposed rules

Use USD-days, with independent qualifying activities adding together. Preserve holding verification, whole-snapshot minimum selection and protocol-account exclusions rather than inventing new activity categories. No new chain, external venue, referral or promotion bonus.

| Qualifying activity | Proposed rate | Boundary |
| --- | --- | --- |
| Outstanding icUSD vault debt | 1× | Verified owner/debt and eligible time |
| icUSD 3pool deposit | 1× | Recorded deposit capped by verified eligible 3USD holdings |
| ckUSDC/ckUSDT 3pool deposit | Flat 4× | Both compositions; same holding cap, no matching bonus |
| icUSD Stability Pool position | 1× | Verified position |
| 3USD Stability Pool position | 2× | Verified position |
| Rumi AMM liquidity | 2× | Existing supported LP valuation only |
| Qualifying ckUSDC/ckUSDT vault repayment | 5× | Repayment during Season 2; up to 90 days, truncated at pilot end; no Season 1 window carryover |

Propose keeping founder/team positions on the same rules as others, with no per-address cap. Existing registrants may continue; new participants accrue from their recorded first qualifying registration/action, never before it. Registration does not prove every historical holding. Retain all Season 1 originals unchanged. These eligibility choices are part of the proposed package, not historical approval of Season 2.

## Capped budget options

| Option | Cap and funding | Implication |
| --- | --- | --- |
| **A — recommended** | **0 RUMI committed; $0 new spend authorized** | Points-only pilot design/accounting within existing allocation. No redemption promise. Any later allocation needs a separate funding decision. This creates no new paid-credit, build or operating allowance. |
| B — reserve proposal only | Propose **at most 0.5% of total RUMI supply**, from a future DAO treasury allocation | Treasury availability, supply basis, token count, approval and funding are **unknown/unverified**. The 0.5% ceiling is a policy option, not a calibrated optimal subsidy or available balance. Do not announce or obligate it until separately authorized. |

The historical design reserves no independent Season 2 pool; it contemplated a future DAO treasury proposal. Season 1 claim residuals are unknown and cannot be spent twice or assumed available. Selecting A allows design to proceed while token/claims remain parked; it does not require resolving B now. No dollar equivalent, yield, APR or guaranteed token-per-point rate is proposed.

## History coverage and missing observations

This tranche reviewed retained source and closeout evidence; it did **not** establish complete November participant history or run a new live export. The October make-whole conventions are precedent for disclosed modeling, not approval to apply them again.

| Evidence needed | What existing evidence supports | Remaining coverage limit |
| --- | --- | --- |
| Registration and activity timestamps | Engine retains registration time; source identifies relevant event/position categories | November roster and complete event ranges not yet reconciled |
| Debt/repayment intervals | Retained backend events supported bounded October reconciliation | November completeness, starting balances and qualifying repayment classification still unverified |
| 3pool recorded deposits and held 3USD | Existing source caps recorded deposits by verified holdings | Deposit events alone do not establish historical wallet/SP/AMM holdings or valuation |
| Stability Pool/AMM positions | Existing position and valuation logic can be reused prospectively | Historical changes, interest and valuations require evidence; current balances cannot establish past balances |
| Random epoch snapshots | Engine uses two observations and retains the whole lower-total breakdown | Uncaptured November snapshots cannot be recreated as observations after the fact |

**Recommended default:** preserve the intended start but keep the unobserved interval explicitly pending, with no retroactive award posted. At the next separately allocated evidence step, classify each proposed activity's retained coverage. Award nothing solely by extending today's balances backwards. If coverage supports a model, return one bounded, disclosed allocation proposal before any credit action; otherwise report that interval as uncredited/pending. Do not silently give some activities retroactive awards while missing others, or silently cancel the intended backdating.

Prospective scoring can use the inherited observation rules once a separately authorized implementation is available. Keeping a dated, separate draft Season 2 accounting record is the smallest preparation step. Restarting or extending the finalized Season 1 instance is not the implementation plan: the current start method rejects an already-started epoch history. A separate season instance or explicitly segregated implementation still needs scoped engineering and release authority; none is installed or ready by this proposal.

## Decisions actually needed from Rob

1. **Pilot package:** accept or amend the objective, November 30 end, table and eligibility package above. November 1 UTC remains the intended start already discussed.
2. **Missing-history policy:** accept the pending/uncredited default until a disclosed coverage-backed model is reviewed, or direct a different compensation convention. No numerical allocation is being requested here.
3. **Budget posture:** choose A (recommended, no allocation promise), or ask for B to be developed later. B requires a separate funded treasury decision and resumption authority; no funding/claims decision is needed to choose A.

Merritt reviews this recommendation first. No request to approve claims tiers, launch date, governance deployment, promotion, money movement or additional spending is bundled into these decisions.

## Evidence and next slice

Source basis: main `99027b67a52324836dbba872a847843bf56821cc`.

- [Rate/holding logic, accrual.rs:158](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/99027b67a52324836dbba872a847843bf56821cc/src/rumi_points/src/accrual.rs#L158), including flat 4× at lines 175–176 and SP/AMM rates at 185–187.
- [Start lifecycle, epoch.rs:244](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/99027b67a52324836dbba872a847843bf56821cc/src/rumi_points/src/epoch.rs#L244): in-season, already-started and prior commitment checks.
- [Historical rate labels, airdrop-report.py](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/99027b67a52324836dbba872a847843bf56821cc/scripts/airdrop-report.py): legacy 5×/3×, flat 4× and separate historical uplift.
- Original owner evidence remains in the retained Season 1/2 chat and local closeout records. No participant export, private address or raw incident log is published with this proposal.

Next authorized slice after review/allocation: bounded November coverage inventory and an implementation outline for segregated accounting. No build, new harness, broad security study, canister action or reward credit is required to review these terms. No feature delivery shortcut was taken in this documentation tranche.
