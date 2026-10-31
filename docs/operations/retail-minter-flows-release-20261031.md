# Retail minter flow cleanup — 2026-10-31

Scope: Bitcoin (`/bitcoin`), DOGE (`/doge`), and every supported ckERC20 selection (`/ckusdc`). The user requested the same cleanup as the guided ckSOL flow, with frontend publication when ready. Backend canisters, borrowing pages, controllers and wallet permissions are unchanged.

## Result

- Bitcoin and DOGE restore reusable addresses and watch deposits automatically while the page is open, including automatic retry after temporary Bitcoin check errors. Withdrawal estimates refresh as inputs change; saved accepted withdrawals resume status tracking on return. Confirmed payout or completed reimbursement becomes a collapsed receipt without an acknowledgement/reset step.
- ckERC20 refreshes fees/balances, restores known Ethereum submissions, and tracks exact mint events and accepted withdrawal burn receipts. Mint completion joins the exact accepted deposit to the minter's minted event; it does not infer completion from a generic balance increase. Ethereum reverted submissions require finalized/canonical receipt proof before their exact journal lock is released.
- Approvals, Ethereum sends and withdrawal requests still require explicit user consent. Scoped signer windows open in the click handler, remain isolated from other sessions, and close idempotently without hiding accepted results. No spending is replayed automatically.
- Missing hashes, damaged journals, uncertain ledger burns, partial burns, unavailable status and nonterminal reimbursement stay preserved for recovery/support. A delayed response cannot remove a newer request or display completion for it. This deliberately keeps uncertain financial effects out of the ordinary repeat-submission flow.

## Validation

- 636 tests in 21 frontend suites passed, including mounted minter, borrowing regression, signer ownership, fee expiry, remount, wallet generation, concurrent-tab journal and ambiguous-response tests. Production build and Internet Identity/custom-domain asset verification passed. `git diff --check` passed.
- Whole-frontend svelte-check retains 30 errors and 66 warnings in 25 unrelated files; changed files have no diagnostics.
- Live public Candid was fetched for BTC/DOGE/ckERC20. The additive ckERC20 projection successfully decoded deployed `get_events` (empty and genesis) and an empty `withdrawal_status` result. Live calls are read-only and do not establish a paid lifecycle.
- Chrome tested actual page components with simulated accounts/minters and all spending blocked: automatic addresses and confirmations, responsive layouts at 390px, automatic redeem quotes, token selection/input reset, restored terminal payout receipt. The temporary viewport override was reset. No user financial information is included in this record.
- Adversarial review passed after three rounds: the first identified damaged-journal and uncertain fee-burn recovery fixes; the second passed; a final fresh pair also passed after the Bitcoin transient-retry refinement. Publication readback is recorded below after completion.

## Publication boundary

Existing frontend canister `tcfua-yaaaa-aaaap-qrd7q-cai`, network `ic`, existing operator `rumi_identity`, assets only. Preserve every key/property/encoding in the fresh preflight inventory; stage new immutable assets first, verify them with the old index still live, then atomically publish all mutable encodings. Do not use the older uploader's default first-page-only list call. No Wasm upgrade or new permission is part of this release.

Preflight frontend Wasm: `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`. Backend preservation baseline: `b8f9687ffd55e02eec2d6893e4c6cbd1e1d751b10ece4ccf19173e14263d27ad`. The fresh inventory contains 726 keys and index SHA256 `3a8da5151208606b7e4d64b79a193b9de0677577709c43a54f16547e8702add3`; this supersedes the earlier release's historical inventory. No old asset deletion is requested.

No live wallet approval, token deposit, withdrawal or reimbursement roundtrip is part of software verification. Browser fixtures and served-byte checks prove the UI/software release, not settlement with real funds.
