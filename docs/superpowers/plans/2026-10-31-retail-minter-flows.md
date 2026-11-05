# Retail minter flow cleanup

**Goal:** Users send/receive assets and approve spending; the app manages status, quotes and completed receipts.

**Scope:** `/bitcoin`, `/doge`, `/ckusdc` (all supported ckERC20 selections). Preserve distinct native-chain confirmation rules, wallet consent, exact amounts and uncertain-outcome holds. Borrowing routes and backend behavior remain unchanged.

**Architecture:** Each existing minter adapter retains its asset-specific calls. A shared isolated OISY action channel spans sequential user-approved calls. Read-only polling and owner-bound persisted receipts restore progress; only authoritative terminal evidence can retire the exact saved request under the same Web Lock. No automatic spending, approval or withdrawal replay.

## Work

- [x] Bitcoin: restore address/checks, automatic bound fee estimates, terminal withdrawal cleanup without losing newer journals, plain progress and optional receipt details.
- [x] DOGE: replace manual-only status checks with durable owner-bound withdrawal recovery and automatic tracking; automatic deposit/address restoration and fees where appropriate.
- [x] ckERC20: reconcile known Ethereum hashes and accepted withdrawal burn receipts, persist pending settlement, automate quote/balance reads, remove user-clear retry controls. Ethereum confirmation or a balance increase alone must not be described as proof of mint finality.
- [x] Extract the existing isolated signer into shared minter infrastructure, preserving ckSOL imports and its ownership/cleanup behavior.
- [x] Verify known/pending/uncertain/failed outcomes, returning sessions, owner/token switches, late responses, concurrent tabs and teardown. Run focused suites, production build, desktop/narrow browser fixtures, and independent adversarial reviews.
- [x] Publish source and frontend assets only after validation; verify paginated inventory, all served route hashes and unchanged canister Wasm/controllers. No real-wallet transaction is part of software verification.

## Boundary

Status reads never authorize spending. A missing hash, empty history or unavailable query is not proof of failure. Known confirmed/reverted Ethereum submissions can retire only their matching submission lock; mint and payout completion need their own evidence. Partial failure stays visible with a useful recovery/support path rather than a user-operated bookkeeping reset.
