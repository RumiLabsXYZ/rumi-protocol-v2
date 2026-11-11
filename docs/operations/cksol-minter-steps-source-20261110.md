# ckSOL mint steps and polling — reviewed source, November 10

## Scope and pins

Source branch `codex/cksol-minter-steps-20261110`, exact tip `94ffd5d51b7d12c79f7432e0361bc1654ef6bb5c`, based on corrected pending Vault51 candidate `097bf775`. The full change relative to that base is six frontend files: the existing Solana route, ICP payment panel and their existing mounted fixtures, plus a small read-only scheduler and tests. No backend, Candid, financial-service implementation, persistent journal or dependency changes.

The live frontend was independently reported by Dorian as SEC12 version `1794195655486`/source `f1d2e77e`. The separately staged Vault51 frontend built at `42478174` was not activated at the release handoff. Dorian retains that existing publisher reservation; this patch must be composed with that lineage before a new source-matched build. This record does not authorize a build, merge, deployment, reserve activation or customer transaction.

## User-visible flow

1. Check the processing method, maximum ICP debit or temporary cycles balance, material fees and readiness before a new SOL send.
2. Send SOL mainnet to the same connected-account address; show detected, waiting, unavailable or held status with visible Check again. A balance observation never starts minting. Previous success is explicitly historical; Start another deposit rearms detection.
3. Deliberately mint or continue the existing funded/approved request through the existing payment/session/lock guards. Routine background reads do not change the action to Checking request or repeatedly disable an already-ready button. Unknown outcomes remain held.
4. Show the last verified mint separately from the connected wallet balance. A newly confirmed Minted block performs one final account/status-generation-bound ledger read; a failure removes the stale balance and offers a manual refresh.

Technical receipts, raw IDs and detailed fee availability remain collapsed. Maximum cost and the retained5% minter-charge service fee remain disclosed before approval. Payment settlement alone is not mint completion.

## Polling contract

Active waiting, mint or payment/recovery reads start at15 seconds and back off to30 then60 seconds when unchanged. Meaningful progress, an explicit Check again or new-deposit watch resets the delay. Hidden tabs/disconnect/unmount cancel scheduled checks; in-flight results remain session-bound. Full completion requires mint success plus known payment/refund settlement or authoritative absence. A failed ICP receipt lookup or local fallback is not that proof: it stays active, disables another payment, and retries read-only. After completion, focus does not restart a perpetual loop; Check again reads once, and a new deposit is deliberate. Reads never invoke a payment/mint action.

## Verification

- **126 passing tests in six suites:** mounted page45, mounted ICP panel14, scheduler4, existing cycles payment39, ICP payment18, advisory balance6.
- Three concrete review findings were reproduced before their corrections: external receipt progress failed to reset a backed-off timer; direct terminal mint left the wallet balance stale; unavailable authoritative ICP status with no local journal falsely looked settled. All corresponding mounted regressions now pass.
- **Both final independent reviews PASS** on exact tip94ffd5d5, no confirmed blockers. Earlier contrary review verdicts apply to superseded candidates and remain useful regression evidence.
- Final project typecheck still reports **30 pre-existing errors and66 warnings**, with no diagnostics in changed source. No clean project-wide typecheck is claimed.
- Actual Svelte page rendered with mocked wallet/chain services in Playwright: desktop1200px and mobile390×844; meaningful page/title, no framework overlay, no app console errors/warnings, no horizontal overflow. Read-only detection submitted zero payments; explicit mock Mint submitted once; completed Start another deposit restarted waiting without payment; unknown status blocked minting; unavailable detection was honestly labelled. Receipts were collapsed.
- Browser plugin was unavailable; the existing Playwright connector was used. Initial temporary preview alias setup errors were fixed before final QA. The preview used existing repo CSS; its pre-existing CSS import-order warning remains. No production build ran. Screenshots and raw test/browser logs remain outside Git.

The localhost preview was stopped and its test tab closed. Worktree inventory/guarded pressure cleanup found zero eligible removals and removed nothing. The existing dependency symlink and unrelated worktrees were preserved. Customer financial acceptance and actual served deployment remain separate future proof.
