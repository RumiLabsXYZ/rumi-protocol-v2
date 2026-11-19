# Vault 197 OISY repayment popup repair

This source/test checkpoint is followed by the [actual built artifact and publication packet](vault197-oisy-repayment-publication-20261118.md); its build-not-started statements below describe the earlier checkpoint.

The repayment click opened the direct SDK signer, but its bound allowance and backend actor paths used a separate legacy PNP signer sharing the default popup name. The existing legacy idle closer can close or replace that window before consent. This is a supported source/served-route diagnosis, not a claim that the customer's funds moved or that their exact browser transport error was captured.

The exact observed case is Vault 197 / 149.99 icUSD. Its debt snapshot made this an ordinary partial repayment. The repair preserves the requested amount, caller/session checks, token routing, approval amounts and expiry, backend methods and unknown-outcome handling.

## Exact source and change

- Runtime/test source: `3294c9561664d2ec48070ac0bab50862913f22dc`.
- Built frontend baseline preserved: `8e6f6f01671e615feb577d19400b22c41937a936`; this candidate composes its deployed Stability Pool deposit and recovery-menu/queue fixes.
- Exactly seven frontend source/test paths differ; scripts, declarations, package manifests and lockfile are unchanged.
- `repaymentOisySigner.ts` opens one uniquely named popup synchronously from the click, disables idle auto-close, and binds one SDK agent to the captured owner. Both ledger approval and backend repayment use that agent.
- Component destruction invalidates the repayment context and closes its owned action. Cleanup closes the captured popup immediately, then closes any channel established after cancellation. Canceled actions cannot become ready or reopen their popup.
- Other-wallet routing and add-margin behavior are unchanged. No global PNP/auth rewrite or allowance increase is introduced.

## Focused verification

All **44 tests pass** across the bound repayment API, actual-SDK signer lifecycle, repayment card and deployed OISY Stability Pool deposit regression. Coverage includes the exact case, ordinary/stable/close routes, refused or lost approval replies, post-approval stale context, lost repayment replies, same-owner reconnection, failed opening, destruction during opening, legacy-window closure and idempotent cleanup.

Two independent initial reviews found an SDK lifecycle race: `closeChannel()` only closes an already-established channel. The real-Signer deferred-handshake regression failed before correction (3 passed / 1 failed), then the complete 44-test run passed. Two fresh final independent reviews pass on the corrected source, with no confirmed blockers. Earlier component fixture failures and the original genuine legacy-actor regression are retained privately; they are not represented as passing runs.

[Source, test and review receipt](evidence/vault197-oisy-repayment-20261118/source-test-review.json) and [redacted actual focused log](evidence/vault197-oisy-repayment-20261118/focused-tests.redacted.log) retain the proof boundaries. The served pre-fix API chunk was `CJGH-0fQ.js`, SHA256 `723dbb1381b9bfe21ebe97c1bcf7867a19a9c14b88e69b36c6528ae7424a92ab`; the private read-only capture records its two actor routes.

## Build readiness and release boundary

One immutable tracked export (646 files) and a fresh bounded build request are prepared. Reuse the prior verified offline frontend build recipe, runner and warm dependencies; no install or new worktree is required. The old completed grants remain consumed. The request binds source `3294c956`, tree `4d04bbcaafc1a13ac5f3fe4931ee9913acfe45f0`, runner SHA256 `f1b01040d9cbba121e2f1a92c8049b5e1c01564a71ad7a3b7f23b92aecf8f330`, and the seven-path source delta. The private request SHA256 is `74859407733cfab5a353074661f8de8ba99700dc94feb278fcc573ef5c325401`; the [redacted request](evidence/vault197-oisy-repayment-20261118/build-request.redacted.json) preserves the public pins and limits.

Bounds: one `npm run build --workspace vault_frontend`, offline, 120 seconds execution plus 60 seconds closure, 6 GiB owned RSS, 2 GiB physical growth, 20 GiB free-space floor and 8 MiB logs. The runner must execute with process-inspection access; the earlier SP build's sandbox failure and consumed grants are not replayed. No build has started for this candidate.

Dorian remains the sole release owner and must resolve current exact publication authority and the live baseline before any asset mutation. Merritt remains the single customer-case coordinator. Source tests and reviews do not prove real wallet consent, backend repayment, deployment or a resolved customer outcome. No client signing or financial retry was performed. Backend, reserve, bot, worker, Vault 202 recovery and liquidation changes remain outside this repair.

The shared checkout's 194 unrelated tracked deletions remain untouched and unstaged. The required worktree inventory/guarded cleanup found zero eligible and removed zero worktrees.
