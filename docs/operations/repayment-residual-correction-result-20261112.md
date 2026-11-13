# Repayment residual correction — source and focused check result

Implementation candidate: `00cf550d3fcb97870cd4643883d5abd8900fa1a2`, based on `240db4cd15647a1871dba5e4774ff7d791963b88`. Owner Ellis; Rowan implemented backend; Iris independently checked accounting. This is preparation evidence, not a production release or customer financial outcome.

Ordinary repayment requires authorization of at least 0.1 icUSD. Each rail caps the actual payment to the authorization and current debt without the old percentage-based upward snap. After a successful payment, interest is accrued through callback time and a positive remainder strictly below 0.1 is forgiven; exactly 0.1 remains debt. Borrowing and liquidation minimum rules stay unchanged.

Paid and forgiven debt are applied separately. Only paid interest earns distributions. Forgiveness reduces debt and records the loss in the existing forgiveness total; it does not claim a token burn, new stable backing or fee revenue. A new optional repayment-event checkpoint preserves debt, accrued interest, accrual clock and forgiven amount for replay; old omitted-field CBOR events retain their prior behavior.

The frontend Max reserves one approval fee and uses spendable funds without increasing the approved payment. Wallet-limited near-full repayment remains ordinary repayment even if forgiveness clears the debt. Estimates match stable-token authorization truncation and distinguish remaining debt and forgiveness. API validation uses the fixed repayment minimum rather than the configurable borrowing minimum; explicit repay-and-close retains positive subminimum repayment, including existing collateral and native-XRP semantics.

## Essential evidence

- Six backend residual/accounting/accrual/replay/legacy-CBOR tests passed, zero failed or ignored.
- Four stable-rounding tests passed, including no upward principal snap, fee overflow and fractional conversion coverage.
- Canonical Candid generation and canonical declaration generation passed; the generated diff is four files containing only the new optional event checkpoint. Source-versus-declared compatibility and new-versus-prior service compatibility passed.
- Twenty frontend tests across four suites passed: settlement limits/precision, input contract, mounted Max/close/refresh behavior and actual API-method dispatch under a higher borrowing minimum.
- `git diff --check` passed. Test warnings remain; no full typecheck, broad suite, paid CI, PocketIC run or production Wasm build was performed for this preparation scope.

The first batch stopped after six native passing tests because the owner's runner incorrectly parsed a regex match. No later results were inferred. A corrected, freshly coordinated continuation executed the remaining checks; completed backend tests were not rerun. Native exits, counts and log hashes are in [the check receipt](repayment-residual-checks-20261112.json).

## Preserved scope and proof limits

Merritt explicitly accepted preserving existing <=0.0005 icUSD unpaid withdrawal/close exceptions verbatim, including the native-XRP zero-debt guard, to avoid stranding historical tiny debts. SEC17 introduces no unpaid <0.1 exception. The older withdrawal/replay asymmetry is separate backlog.

All three repayment recorder calls are confined to successful ledger-result branches; failed/rejected/unknown results return errors without the new forgiveness. This is source inspection, not a real-ledger transport-uncertainty test. Sequential distinct settlements and replay are covered; no generic duplicate-event or cross-ingress unknown-retry journal guarantee is claimed.

No automatic collateral withdrawal or closure follows forgiveness. Explicit compound actions keep their established intent and failure recovery. Actual customer recovery, repayment, backend installation, frontend publication and reserve activation are unexecuted for this candidate. The previous approved frontend release remains unchanged.

## Release handoff

Dorian is the sole release coordinator. Prepare one exact backend-plus-frontend build/install decision from this pinned candidate, with the additive event schema and all preserved exceptions stated. Earlier approval for source240 frontend assets is not extended to a new backend artifact. No production action is authorized by this receipt.
