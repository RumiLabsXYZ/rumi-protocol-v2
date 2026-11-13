# SEC19 collateral top-ups: source verification result

Candidate **4d8322a71059f146f6ec44c95add49789be2b6f8**, branch `codex/add-margin-recovery-20261113`, restores the operation-ID ICRC top-up route and the matching frontend. Production remains paused. No vault213 deposit or customer financial action was performed.

## Resulting behavior

A confirmed, exact owner/operation/ledger/amount/memo-bound transfer credits the matching surviving vault once. If that target has disappeared, changed ownership/configuration, become protocol-controlled or cannot represent the sum, the same confirmed receipt creates one separate held collateral liability. It does not report a vault credit. Unknown outcomes retain their request and require reconciliation; retries cannot manufacture another credit or liability.

The existing pending-payout model holds that owner/ledger/gross-amount liability without automatic dispatch. Acknowledging the explicit recovery notice retires only the operation receipt; the unpaid liability remains. SafeNoEffect ACK uses the existing durable cancellation fence. Public credit history and the private settlement journal replay in order without applying the amount twice. Legacy add-margin remains disabled.

The client preserves owner/wallet/session across queue and await boundaries, opens the existing OISY channel in the explicit click, and distinguishes held collateral from success. Missing or invalid collateral decimals now reject before conversion or approval; they never default to eight. Saved-request restore/cancel controls retain their existing authority checks. The four exact live SEC18 files are copied from ab44413d, preserving the progress update.

## Deterministic evidence

- Six new backend units, one old-journal compatibility regression and two transfer-outcome classification tests passed. [Retained first-run receipt](evidence/add-margin-validation-20261113/first-run/completed-checks.json).
- Canonical Candid and declarations regenerated; actual prior-service subtype and declared/generated compatibility passed. The four generated files add only an optional recovery record. [Stages](evidence/add-margin-pic-20261113/retained/steps.json).
- Source-matched backend/flaky-ledger Wasm passed metadata, shrink, O1 and executable-code guard checks. [Artifact pins](evidence/add-margin-release-20261113/artifact-pins.json). All397 declared service methods match their query/update exports; all407 export names/order match the released artifact, including its internal timer method. [Comparison](evidence/add-margin-release-20261113/export-comparison.json).
- All three corrected PocketIC cases passed: committed lost reply with exact retry and one credit; committed lost reply followed by zero-debt target closure, positive receipt recovery and one owner-filtered liability surviving ACK; typed no-effect rejection and ACK without a pull. [Actual log](evidence/add-margin-pic-20261113/passed/pic-sec19.log), [native steps](evidence/add-margin-pic-20261113/passed/steps.json).
- The final six frontend suites passed134/134, including nine-decimal amounts, session changes, no automatic recovery approval/dispatch/ACK, explicit notice ACK, and decoding actual generated-schema records with wide payout references. [Log](evidence/add-margin-release-20261113/frontend-134.log).
- The missing-first-then-nine-decimal metadata regression reproduced old-code success with a wrong scale. The corrected path and four invalid-decimal cases pass. [Red log](evidence/add-margin-release-20261113/metadata-red.log).
- Cody independently verified exact source/artifacts, native outcomes, resource limits and empty owned groups/outer runner/PocketIC server, then released LOCAL. [Receipt](evidence/add-margin-release-20261113/cody-independent-validation.json), [closure](evidence/add-margin-release-20261113/cody-owned-closure.json).

The first allocation stopped at its filesystem-loss guard; nine passing tests were retained. A continuation passed interfaces/Wasm but failed the three cases at initial fixture funding, before top-up. The fixture now funds approval and pull fees without weakening balance assertions. Only those three tests were rerun under a fresh grant, producing3/3. Both earlier stopped runs remain preserved in the linked evidence; no allocation was reused.

## Independent review

Iris and the second final reviewer independently returned PASS on final4d8322a7 and [all25 pinned files](evidence/add-margin-release-20261113/final-source-pins.json). Early review corrected the SafeNoEffect ACK regression. Final review found the missing-metadata scale race; its two-file correction was reproduced and independently rechecked against actual red/green logs. No confirmed blocker remains within the source scope.

The removed-target PocketIC case uses closure, **not actual liquidation**. Source review verifies closure and full liquidation share `remove_vault_and_unindex`, ordinary top-ups and liquidation share the vault lock, and reconciliation after lock release selects held recovery for removed/protocol-controlled targets. No liquidation-specific ordering defect was found. This is a source argument plus a shared removed-target test, not execution of an actual liquidation.

Source/PocketIC proof does not establish a live wallet signing flow, production state preservation, delivered refund, successful vault213 top-up or customer acceptance. A production frontend build is still unrun. The separate exact release proposal preserves those distinctions and the existing reserve and financial-action boundaries.
