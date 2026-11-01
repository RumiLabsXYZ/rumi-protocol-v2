# ckSOL ICP processing deposit — source review record

This change lets users pay processing in ICP rather than sourcing cycles. Rumi collects a capped deposit, supplies the minter's cycles from a finite allocation, charges the measured net minter cost plus a disclosed 5% service fee and separate operating/ledger fees, and returns unused ICP at the frozen accepted quote rate. Payment settlement and the official minter's SOL/ckSOL completion are separate receipts.

## Verification and limits

- 116 focused frontend tests passed across ICP service/panel, existing cycles service/page, minter and signer suites. They cover insufficient ICP, disabled reserve, existing SOL deposits, uncertain/invalid journals, identity changes, both payment modes and refund controls.
- 37 focused backend ckSOL tests passed, including pricing/cap rounding, isolated escrow/refund state, exact retry tuples, additive state defaults, old cycles-credit protection and a two-user dispatch/balance regression. Source-generated Candid compatibility passed, and canonical declarations were regenerated with the supported script.
- Production frontend build and Internet Identity/custom-domain asset checks passed. Whole-frontend svelte-check reports 30 errors and 66 warnings in 25 unrelated files, with no changed-file diagnostics.
- A real browser rendered the actual ICP payment component using simulated inputs with all financial calls replaced. Desktop quote/mint/refund states and 390px insufficient-ICP, pending-refund and uncertain-funding states were exercised; no horizontal overflow or relevant console errors were observed. Mounted parent-page tests verify ICP-first address gating and the pre-dispatch reservation distinction.
- Independent backend and integration reviews each found one concrete P2: dispatched cycles counted twice as liquid backing, and counterpart payment journals not rechecked inside the shared owner lock. Both were corrected, pinned by focused regressions, and accepted on scoped re-review.

No backend installation, reserve enablement/funding, new wallet financial action or client recovery occurred. New reserve allocation defaults to zero. Existing cycles-funded requests and unknown-outcome holds are preserved. BadFee/aged ICP refunds remain recorded for reconciliation rather than silently changing the approved fee. Earned ICP is not yet extracted or converted automatically.

Actual omissions and before-client-money conditions are recorded in `docs/coordination/demo-deferred/cksol-processing-ui-20261030.md`, CKSOL-ICP-DEF-1 through DEF-3. The full mocked asynchronous lifecycle and a real authorized operator-owned financial canary have not been performed. Source review is not client-money readiness.

## Independent XRP subproject

Incoming XRP memo guidance is on its own branch and draft PR #607. It explains that each Rumi vault has its own incoming address, while preserving the outgoing recipient destination-tag field. It changes no backend or financial behavior and is not deployed by this change. The user's specific exchange and fund locations remain unverified pending public identifiers.
