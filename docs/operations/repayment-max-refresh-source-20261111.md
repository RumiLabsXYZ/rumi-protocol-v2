# Repayment Max refresh — source candidate, not deployed

This separate frontend candidate follows swap support cleanup `6c668e7f76e18c2c0abb63a99f7394be202aa26e`, which follows frozen SEC12 producer25e1441f. It does not change the frozen740file bundle, old publication question, backend or wallet services.

## Reproduction and fix

The actual mounted VaultCard regression reproduces a previously selected Max exceeding the new safe cap after a debt baseline refresh. The client interpolates debt each second and resets it from a refreshed backend value; a lower refreshed value can lower Max. The precise refresh/timing event in the original screenshot remains unobserved. The safe partial limit is intentional: backend vault.rs9327–9340 snaps repayments leaving at most max(1percent debt,0.01icUSD) to full debt. Wallets unable to cover full debt must remain outside that boundary and the configured minimum debt.

An untouched Max selection is now only reduced to a refreshed lower safe limit while idle, with a review notice. It never increases automatically. Typing clears Max tracking, so typed amounts remain unchanged; an invalid amount gets explicit Choose Max again guidance. Max/input freeze while a request is pending. Positive manual amounts are disabled when the safe maximum becomes zero. Action/token/reopening/success reset tracking. Existing full-close, native-XRP, claim repayment, approval and backend rules remain.

## Fee proof and correction

Public query on November11Pacific/November12UTC03:45:14 reported icUSD ledger `t6bor-paaaa-aaaap-qrd5q-cai` defaultfee100000e8s (0.001icUSD). A separate live `icrc1_minting_account` query returned backend `tfesu-vyaaa-aaaap-qrd7a-cai` with defaultsubaccount. Backend management.rs2573–2594 `transfer_icusd_from` sends repayment to that exact account. Per the [official ICRC1 minting-account standard](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-1/README.md#minting-account), this is a fee-free burn. OISY ApiClient always approves; ProtocolManager's standard path approves only if allowance is insufficient. Therefore the existing0.001 reserve is for approval, and no second transferfee is required. An initial two-fee hypothesis was corrected and reverted before commit/build; it is not part of this candidate. Numeric fee budgeting remains equal to the accepted base, with an explicit approval-reserve name and accurate customer copy. Allowance status and past transaction charges were not queried or inferred.

## Verification

- Two mounted regressions failed on baseline: stale selectedMax and missing manual-recovery guidance; corrected behavior passes.
- Final46tests/7suites PASS: seven mounted repayment tests, prior repayment cap/contract, swap-help/inspection/installed-signer and saved-request protections. Do not sum the overlapping earlier31/45runs as distinct tests.
- Two existing independent reviewers inspected the final burn-fee correction and source: both PASS, no blockers. One stale-notice reset finding was fixed and rechecked. No additional team, production build or broad audit.
- Typecheck retains existing30errors/66warnings; no new changed-line errors. Full-suite/production-build/hostedCI were not run. Existing unusedCSS/accessibility warnings remain.
- Actual layout+VaultCard with synthetic stores/transport browser QA: desktop1280x800 selected300.89901599; refresheddebt lowers input and HTMLmax together to300.899014, Repay enabled. Mobile390x844 manual301 stays unchanged/disabled with explicit guidance; Max restores safe amount/enables button. No horizontal bodyoverflow. Existing recovery/expandedportfolio/SeasonBar remain composed. Screenshots and DOM checks passed. One fixture-only missing mock export caused startup failure, corrected before checks; no subsequent relevant errors. No real repayment, approval, cancellation, signing or customer wallet data was used.

## Release handoff

Dorian reviews this exact separate candidate before any fresh build/composition. The old4ea7146a decision covers only prior frozen1794442789073 and is not silently repinned. Source correctness does not establish deployment, a successful repayment or recovery of the customer's later saved request. Existing owner acceptance records remain distinct. No backend/reserve or funds movement is authorized by this document.
