# Vault 208: liquidation price, recovery-target comparison and proposed remedy

**The native-XRP Stability Pool path retired the full debt, rather than stopping
at the XRP recovery collateral ratio.** This was one recorded liquidation, not
a sequence of partial liquidations. It took 92.14% of the reconstructed XRP
collateral; about 58.00% would have restored the configured XRP recovery target under the proposed corrected policy.
New Stability Pool liquidation admissions are now disabled under Rob's approval.
They must stay disabled until the fix is verified and Rob separately approves
resuming. This document proposes compensation; it authorizes no payment, debt
change, clawback, liquidation, recovery or resumption.

## Price and incident

The execution oracle price reconstructs to **$0.937824869 per XRP**. The last XRP
price event before the liquidation reports exactly that price. Applying it to
the recorded debt with the 7.5% asset-specific liquidation bonus reproduces the
actual gross seizure, depositor gain and protocol claim **to the drop**. The
next XRP price update is after the liquidation. That quote was as of
12:38:00 p.m. Pacific and recorded at 12:39:36.290663 p.m.; the next quote
was recorded at 12:44:35.962354 p.m., after the 12:44:27.323216 liquidation.
This is an incident-price
reconstruction corroborated by recorded amounts, not today's price or a claim
that a retained preflight receipt was retrieved.

- Incident: November 14, 2026, **12:44:27.323215836 p.m. Pacific**
  (`2026-11-14T20:44:27.323215836Z`).
- Independently queried icUSD ledger block **46098** and the account index:
  a burn from the Stability Pool default account for **281,245,001,091 e8s**,
  memo `RUMI-LIQ-004:` followed by big-endian vault ID208, at
  `2026-11-14T20:44:26.923748086Z`. The primary ledger agrees with the index.
  See [ledger block](evidence/vault208-comparison-20261115/icusd-ledger-burn46098.txt).
- Pool event **2645**, caller backend `tfesu-vyaaa-aaaap-qrd7a-cai`, and one
  matching pool-history record: **2,812.45001091 icUSD** consumed,
  **3,217.077995 XRP** recorded as pool-side credits/payout claims for six depositors.
- Authenticated developer `get_xrp_claims()` captured protocol-fee claim **19**,
  custody nonce **208**, **6.747541 XRP**, created at the incident timestamp.
  That method returns an empty list for other callers; an empty anonymous
  response is not evidence that claims were paid or absent.
- Gross borrower collateral debit: **3,223.825536 XRP** (pool + protocol claim).
- Live vault and an independently retained November 14 post-incident capture
  both show **275.174474 XRP**, **zero debt**, **zero accrued interest**.
- Before-collateral is reconstructed as pool gain + protocol claim + residual:
  **3,499.000010 XRP**. Debt immediately cleared is the proved burn amount,
  **2,812.45001091 icUSD**, including any interest in that live debt.

The borrower-effective settlement price is **0.8723952272 icUSD per gross XRP**
(debt retired divided by XRP debited). It differs from the oracle because of the
7.5% liquidation bonus. The configured 117% normal-mode trigger corresponds to
about **$0.9404305525/XRP** for this position; that is a threshold, not the
execution oracle. Recovery-mode eligibility can use the higher borrow threshold;
the reconstructed pre-liquidation CR, **116.675824%**, is below both thresholds.
The incident mode is not recorded in the pool receipt and has not been
independently established. Neither the normal-mode trigger price above nor
today's protocol mode is presented as proof of its execution-time mode.

## Actual versus the proposed corrected recovery-target policy

The per-XRP configuration has borrow threshold **128%**, recovery multiplier
**1.033333333333333333**, recovery target **132.2666666667%**, liquidation
bonus **7.5%**, six collateral decimals and protocol share **3% of the bonus**.
This is the per-asset recovery target, not the separate legacy global getter.
**The comparison and compensation below use this proposed corrected-policy
benchmark. They do not claim that the deployed incident code already enforced
this target.** Its XRP path requested full debt; the ordinary normal-mode helper
targeted the borrow threshold, while the recovery-mode helper used a separate
global recovery target. Rob's requested per-vault recovery-CR behavior is being
implemented against the explicit per-XRP configuration target here.
Current values are corroborated by bounded post-incident Admin events: only
upgrades appear, with no collateral-ratio/bonus/recovery-multiplier changes.
The config-to-incident link relies on recorded setters and state-preserving
upgrades; no retained pre-liquidation heap/preflight snapshot was obtained.

The required repayment is `(target × debt − collateral × price) / (target − bonus)`.
A conservative repayment rounded up to one icUSD e8, followed by the existing
collateral/bonus/protocol flooring rules, gives:

| Measure | Proposed per-XRP recovery target | Actual liquidation |
| --- | ---: | ---: |
| Gross XRP taken from borrower | **2,029.427587 XRP** | **3,223.825536 XRP** |
| XRP credited to pool depositors | 2,025.179948 XRP | 3,217.077995 XRP |
| Protocol XRP claim | 4.247639 XRP | 6.747541 XRP |
| icUSD debt retired | **1,770.46294114** | **2,812.45001091** |
| XRP remaining in vault | **1,469.572423 XRP** | **275.174474 XRP** |
| icUSD debt remaining | **1,041.98706977** | **0** |
| Remaining collateral ratio at incident price | **132.266666744%** | No debt |

The actual gross debit and before-collateral are reconstructed from the
recorded pool-side amount, authenticated protocol claim and retained vault row.

The continuous repayment is 1,770.46294113477 icUSD. Native-unit flooring can
allow a slightly smaller discrete amount; the table uses a conservative closed
form rounded up, not a claim of a global search for the smallest raw unit.
Sub-drop/e8 quantization does not explain a 1,194 XRP difference. Debt was far
above the 1 icUSD dust threshold; this was not a necessary dust close. The
position could reach the recovery target without full repayment.

[Reproducible calculation and integer outputs](evidence/vault208-comparison-20261115/calculation.json)
and [calculation source](evidence/vault208-comparison-20261115/calculate-comparison.py)
bind the inputs and rounding used here.

## Cause and deployment provenance

`State::recommended_liquidation_amount_for` explicitly returns full debt for
native XRP. The Stability Pool uses that recommendation; backend
`xrp_sp_absorb_sizing` requires the burn to match the full live debt. This
bypasses the ordinary partial-sizing route. There is no technical proof that
claim settlement requires full liquidation; the full-debt exception conflicts
with the intended recovery-target policy.

The incident occurred after the November 14 SEC19 backend release, source
`4d8322a71059f146f6ec44c95add49789be2b6f8`, module
`8f6f5933e54fa44205760b9a8d1c3869135df43825eadfee70b20c62d0a0bd16`.
Its source already contains the exception. The release receipt and bounded
post-incident upgrade events bind this chronology. Git blame traces the
full-debt recommendation to `be436d19d` in August; it was not introduced by the
November 15 security upgrades. Current backend source `f72ede81` retains it;
current module is `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111`.

[SEC19 live receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/3ffe0de7/docs/operations/add-margin-live-result-20261114.md),
[incident-source exception](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/4d8322a71059f146f6ec44c95add49789be2b6f8/src/rumi_protocol_backend/src/state.rs),
and [current source](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/f72ede81/src/rumi_protocol_backend/src/state.rs)
support that distinction. The ICP-only bot did not execute this XRP liquidation.

## Recommended borrower remedy, subject to separate approval

The excess collateral debit is **1,194.397949 XRP**, but it also eliminated
**1,041.98706977 icUSD** of debt that should have remained. Returning the entire
extra XRP while keeping zero debt would overcompensate the principal reduction.
On the protocol's $1/icUSD accounting basis, at the incident oracle:

`1,194.397949 × $0.937824869 − $1,041.98706977 = $78.149030284793681`.

**Recommend offering protocol-funded compensation for that additional penalty,
while preserving the borrower's zero debt.** Benchmarks rounded up to payment
precision are **78.149031 ckUSDC** or **83.330090 XRP valued at the incident
price**. These are alternative compensation denominations, not two payments.
Delivery fees are additional protocol cost; current XRP market value can differ
from this incident-price benchmark. This is a proposed make-good under the explicitly selected per-XRP policy
benchmark, not proof of a pre-existing legally or mechanically fixed entitlement.
This calculation excludes market movement,
opportunity costs and independently unverified transfer fees.

For comparison, the actual remaining position was worth about **$258.06546503**
at the incident price. The intended remaining collateral less remaining debt
was worth **$336.21449532**. The proposed compensation closes that difference
without creating a new borrower obligation.

Preferred payment asset/recipient, available protocol funding and any prior
compensation are **unverified** in this analysis. Before proposing an executable
payment, verify the owner's preferred asset and
recipient, identify protocol-owned funding and live balance/fee coverage, check
there is no prior compensation, and freeze one exact idempotent payment packet
for Rob's approval. Do not take compensation from the borrower's retained XRP,
other borrowers or pool depositor claims. The outstanding developer fee claim
is only 6.747541 XRP; it is not enough by itself to fund the proposed XRP amount.
No protocol funding source has yet been asserted available for this payment.

An alternative owner-consented position reconstruction would return the extra
collateral while restoring/funding the corresponding debt, but requires
separate borrower consent, supply/accounting design and exact approval. It is
not the recommended automatic remedy. No unilateral reborrow, debt write,
reversal or claim clawback is authorized.

## Safety and remaining work

The approved backend SP-admission hold is verified true; the pool remains
unpaused for ordinary operations. [Containment receipt](vault208-containment-20261115.md)
records the exact action and separate resume-approval requirement. The source
fix is being prepared with focused recovery-target, conservation, legacy retry
and durable hold-replay checks. No fixed software is yet installed, and no
compensation has been sent.

Full per-vault history hit the instruction limit; this investigation used
bounded direct events/timestamps, the full retained 65-entry liquidation
history, exact pool event2645, authenticated claims and retained post-incident
state instead. Query observations are not a claim of independently verified
XRPL finality or full certified inclusion proofs. The six depositor payouts'
complete XRPL settlement chain has not been independently reconstructed here.
