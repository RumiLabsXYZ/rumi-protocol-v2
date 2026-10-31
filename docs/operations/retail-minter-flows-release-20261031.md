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
- Adversarial review passed after three rounds: the first identified damaged-journal and uncertain fee-burn recovery fixes; the second passed; a final fresh pair also passed after the Bitcoin transient-retry refinement. Publication readback is recorded below.

## Publication boundary

Existing frontend canister `tcfua-yaaaa-aaaap-qrd7q-cai`, network `ic`, existing operator `rumi_identity`, assets only. Preserve every key/property/encoding in the fresh preflight inventory; stage new immutable assets first, verify them with the old index still live, then atomically publish all mutable encodings. Do not use the older uploader's default first-page-only list call. No Wasm upgrade or new permission is part of this release.

Preflight frontend Wasm: `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`. Backend preservation baseline: `b8f9687ffd55e02eec2d6893e4c6cbd1e1d751b10ece4ccf19173e14263d27ad`. The fresh inventory contains 726 keys and index SHA256 `3a8da5151208606b7e4d64b79a193b9de0677577709c43a54f16547e8702add3`; this supersedes the earlier release's historical inventory. No old asset deletion is requested.

No live wallet approval, token deposit, withdrawal or reimbursement roundtrip is part of software verification. Browser fixtures and served-byte checks prove the UI/software release, not settlement with real funds.

## Deployed readback

- Source PR [#603](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/603) merged at `93df21ffda458fcc6b459ff9b4c8a67b25ca9858` from reviewed source `57424949244d43ba494d43f6a77e1155efac463a`. No repository CI workflow applies to frontend-only paths; the validation above is local source/build/browser evidence.
- Immutable batches 637–642 staged 411 new keys; final batch 643 atomically updated entry/version encodings. All 726 preflight keys remain: 1,137 total. Every existing asset property and untouched encoding was preserved, and all 418 changed/new encodings match the final build. No assets were deleted.
- Served index SHA256: `566f1d92658768a2cc040faf519b848286ff5b9304934926c89c5f395e4aed53`. The index and every listed route bundle were fetched from `https://app.rumiprotocol.com` and matched the build. Frontend/backend remain Running; both Wasm hashes, controllers and fixed allocation/freeze/log settings match preflight. No backend call that changes state or wallet financial action was used.

| Route | Served node | SHA256 |
| --- | --- | --- |
| `/bitcoin` | `7.DKZvh45R.js` | `d392d6f1c9fd4cb240eb24869de8004025ce9daf808204b0d093b7a327d5db73` |
| `/ckusdc` | `9.CDXtZMOq.js` | `e8ea2336920116aa13fa697064ec301534b3231ff8351512265e8c9a5fd2e0ea` |
| `/doge` | `23.DARUrQp6.js` | `4e6ebe83b7ca46a90eac04122985c2243690fa89758ffa5661b4d9abff3c876d` |
| `/solana` | `55.CKk9EBZZ.js` | `74954be573d70cf6307b095f0d0d010b6ba57598d72b9d330a8c5b6dedc8b06d` |

- Post-publication disconnected browser checks rendered `/bitcoin`, `/doge`, `/ckusdc` and `/solana`. ckERC20 loaded its live token list, helper and supply values. No wallet connection or financial control was used. Existing unrelated missing-token logo lookup warnings were present before publication. The scoped build remains the source of connected flow evidence; no paid canary was performed.

## Reusable review lesson

Proposed catalog guidance: distinguish definite no-burn ledger errors from inter-canister transport errors even when both arrive inside a typed minter error. Preserve malformed local journals rather than treating parse failure as absence. Verify those holds after remount and require exact journal lineage before showing completion as well as before writing it. These lessons are backed by the mounted regressions in this release; no persona/catalog files were edited.
