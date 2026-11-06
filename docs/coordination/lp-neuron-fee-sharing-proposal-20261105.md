# Liquidity + RUMI neuron fee-sharing sketch

Custodian: Orin, under Merritt. November 5, 2026. **Discussion proposal only**, recorded at Rob's request so this thread is not lost. No implementation, emission, token launch, reward payment, governance action or fixed yield is authorized. This is separate from continuous Season2 accounting and does not settle token allocations or Season2's end date.

## Proposed incentive

Keep ordinary LP/lender yield unchanged. Consider an additional, capped bonus for a participant who both provides eligible Rumi liquidity and demonstrates ownership/control of an eligible RUMI neuron. The intent is useful liquidity retention supported by token commitment, not paying merely for holding a neuron.

Use only **collected protocol-owned fees** after existing LP/lender entitlements, refunds and other liabilities, and operating/reserve requirements. No projected fees, borrowed customer funds or assumed token allocation funds the bonus. If there is no realized distributable surplus, there is no bonus budget; promise no fixed APR.

Illustrative surplus division for discussion: **50% LP-neuron bonus / 30% reserve / 20% treasury development**. These adjustable percentages are unapproved. They apply to the residual distributable surplus, not to gross swap fees or user principal, and must not override existing entitlements.

Allocate the bonus budget across eligible pools with separate transparent weights reflecting useful stablecoin liquidity and Stability Pool needs. Neither final pool weights nor arbitrary dollar budgets are proposed without actual fee and liquidity evidence. Show each pool's budget and calculation window separately.

## Illustrative capped commitment boost

Discussion-only parameters: **minimum remaining dissolve delay6 months**; stake-weighted remaining duration when several neurons support the same participant; no separate neuron-age bonus initially.

`eligible_liquidity(t) = min(actual_qualifying_LP_value(t), 10 × conservatively_valued_allocated_locked_RUMI(t))`

`boost(t) = 1 + min(remaining_dissolve_delay(t) / 24 months, 1)`

`score = integral(eligible_liquidity(t) × boost(t), over the epoch)`

`bonus = realized_pool_budget × score / sum(eligible scores)`

This illustrative10% backing requirement prevents a dust neuron from unlocking a bonus on the entire LP position. The duration boost is1.25× at6 months,1.5× at12 months and2× at24 months. A neuron balance is allocated **once** across positions; never count the same underlying position or supporting stake twice. Stake below6 months remaining does not qualify under this sketch. These are proposed examples, not imposed conditions on existing participants or earned points.

Use monthly realized net distributable surplus after existing liabilities, base LP entitlements, essential costs and a reserve floor. The illustrative50% program budget could be divided **60% stablecoin3pool / 30% Stability Pool / 10% other explicitly eligible AMM**. These weights are unapproved. If a category is not ready or has no eligible LP, retain its budget rather than automatically redirect it.

For illustration, **$10,000** net distributable surplus yields **$5,000 program / $3,000 reserve / $2,000 treasury**, with program sub-budgets **$3,000 / $1,500 / $500**. This is arithmetic, not a revenue forecast, funded allocation, automatic payout or fixed APR.

Locked-value conversion needs a manipulation-resistant valuation convention before this formula could be implemented. **No liquid RUMI oracle is presumed to exist.** Conservative pricing or an alternative token-unit schedule remains undecided. Do not treat a thin spot price as reliable support value. A falling support value must not create liability beyond the realized pool budget.

Count only ownership/control that can actually be demonstrated. Following another neuron's votes is not ownership. Avoid assuming an account's display identity, hotkey, controller or delegated voting relationship proves all rights needed to claim this bonus; define the accepted proof and reward recipient explicitly.

Withdrawal stops future bonus accrual on withdrawn liquidity. It does **not** forfeit earned Season1/2 points. Existing LP yield remains governed by its existing rules. Neuron dissolving/unlocking should affect future boost/eligibility under clearly disclosed terms, without retroactively rewriting already earned points. No automatic payout, retroactive condition, promised launch/date, new lock or forfeiture is imposed by this sketch.

## Questions retained for later discussion

- Verify actual revenue ownership before classification; no fee double counting; refunds, bad debts, asset conversion costs, reserves/operating needs, illustrative split and pool weights; treatment of zero-score or unused pool budgets.
- Eligible positions/tokens, time window, deposit valuation and illustrative10% backing and minimum6-month delay; treatment of dissolving neurons and ownership changes during a window.
- Reliable RUMI/LP valuation and manipulation resistance; prevention of cross-position/neuron reuse and double claims.
- Ownership/controller/hotkey proof, recipient binding and privacy-preserving evidence; what is disclosed publicly versus retained privately.
- Required governance authority and any legal/tax questions for the eventual mechanism.

These are unresolved design choices, not newly imposed gates on Season2 history preparation or a request to begin an audit/implementation. Before any later approval request, present the concrete revised proposal with funding evidence and exact scope. Until then, keep this card in Backlog under Orin's custody.
