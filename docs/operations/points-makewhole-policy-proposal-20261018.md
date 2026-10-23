# Proposed policy for points missed during the outage

**Decision requested; not approved or applied.** This proposes an economic
make-whole rule because some scheduled random snapshots were never captured.
An exact counterfactual award cannot be uniquely recovered from those missing
observations. Credits must identify compensation honestly; history is retained.

This policy decision does not authorize a repeated upgrade, clear the original
execution-request hold, or delay an otherwise reviewed normal resume.

## Proposed rule

1. Preserve every existing earned entry, matched 5x reward and historical
   unmatched adjustment. Preserve flat 4x from epoch19 and all normal source,
   freshness, eligibility and anti-farming guards for future operation.
2. Cover the complete scheduled intervals: epoch18 is October5–12 UTC and
   epoch19 October12–19 UTC. Any later missed interval must also be addressed
   if restoration is delayed; it cannot be discarded by rebasing an opening.
3. For epoch18, use the conservative snapshot ceiling
   `floor(4 * 7 * total(saved_A_weights) /3)` **per principal** as its total floor.
   The whole-snapshot A/B minimum cannot exceed A's uncorrected total; the
   existing unmatched correction cannot add more than one third of that total.
   Any separately valid repayment window must be accounted for outside this
   snapshot ceiling. Retained current windows are empty and retained backend
   records do not identify an eligible repayment asset; do not invent one.
4. For epoch19, reconstruct source maxima over the entire affected epoch using
   the retained source records, actual balances and recorded composition. Use
   1x debt, the existing SP/AMM rates, and flat 4x fiat-stable 3pool policy.
   Include proved debt losses and a bounded allowance for unlogged dust
   forgiveness. Use the resulting conservative maximum snapshot weight for the
   full scheduled epoch, subject to registration/exclusion rules.
5. Historical verification/valuation gaps require explicit entries in the final
   input manifest. A recorded-composition cap may be used conservatively where
   the historical verification factor cannot be recovered; this can overcredit.
   Exclude recorded pool weights when continuous zero verification holdings are
   actually substantiated, rather than treating a failed read as zero.
6. Freeze and independently review a complete per-principal allocation and its
   evidence digest after the applicable input period is observed. Subtract all
   ordinary credits for that epoch; add only `max(floor - earned,0)` in the
   separate `OutageMakeWhole` audit log. Never subtract an already-earned excess.

## Concrete size of the decision

The original15saved captures produce these epoch18 totals:

| Basis | Points, e8 scale converted for display |
| --- | ---: |
| Full saved A plus its own existing unmatched correction, a reference | 496,571.72870858 |
| Proposed conservative snapshot floor | **629,424.09740874** |
| Additional allowance above that saved-A reference | **132,852.36870016** |

This is deliberately more generous than replaying only known inputs. It can
exceed the unobserved actual award and can change final pro-rata airdrop shares.
It does not expand the token pool, mint funds, or assert a financial transaction.
No source multiplier is changed for normal accrual; the extra is compensation.

For scale only, the current **conditional** epoch19 worksheet through
October18 07:11:06 UTC is593,628.94107182points. Its normalized seven-day reference
is659,653.15408335points. **Neither is the final epoch19 allocation:** the latter
projects beyond observations, and historical/source exceptions remain open.
Do not approve or pay either number as a completed full-period reconciliation.

The debt-loss worksheet includes72distinct current/closed vaults and one extra
positive-borrow cycle, allowing up to0.03650000icUSD of unlogged dust losses under
its stated source assumptions. A new offline read of the preserved backend State
finds all9persisted base/override rates nonnegative and all selected multipliers
positive. That State is a saved blob, not a fresh live-heap observation; combine
it with retained history and the upgrade transition before asserting an
interval-wide invariant. SP source provenance and continuous valuation remain
explicit limitations, not a new blanket gate on ordinary borrowing or Treasury.

## Acceptance and execution boundary

An affirmative decision accepts the conservative compensation policy and its
possible overcredit, rather than asserting exact replay of missing randomness.
It does not approve a particular future allocation with unclosed exceptions.
The final numerical manifest, compatible implementation and actual production
receipts still require review. Original upgrade-request disposition and normal
release preconditions remain in force; no retry or alternate executor is granted.

Declining this proposal leaves the economic choice unresolved. Merely resuming
the old candidate does not satisfy the separate requirement to cover missed time.
