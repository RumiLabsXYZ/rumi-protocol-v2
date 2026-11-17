# Vault 208 source validation — November 17, 2026

Status: focused native tests and interface generation verified; one source-matched backend Wasm build remains pending. No deployment, compensation, financial action or liquidation resumption is included.

The production sizing correction remains the `c2cbffc8` implementation. Three test-only corrections followed: explicitly byte-typed stable-cell reopen defaults (`686ba519`), claims identified through their actual custody owner/nonce rather than a nonexistent vault ID (`a72fde2c`), and a dispatch fixture using the intended $100 debt rather than $1, with explicit non-dust and 130% collateral-ratio premises (`9b477232`). No production math was changed to make those tests pass.

## Actual verification

One isolated-cache batch on source `9b4772322af3a5cb5cb544cb98a518e4ff1eecd0` passed 145 XRP tests, four stable hold tests and the one exact canonical Candid exporter test. Backward compatibility with the retained pre-change Candid, canonical declaration regeneration and strict backend/declaration equality passed. All seven owned process groups closed, with no resource-bound breach. The two earlier failed batches are retained: compilation failed before tests, then 144 XRP tests passed and the mistyped fixture failed. Neither old allocation was replayed.

The final diff check initially failed on 50 trailing-space blank comment lines in each generated Candid copy. The unmodified exporter output and original failing log/receipt remain private and unchanged. A mechanical line-end-whitespace normalization of those generated files now passes strict equality against the raw exporter output, backward compatibility, declaration equality and `git diff --check`. No API was hand-merged and no compiled test was rerun for that formatting-only resolution. Canonical JS/TypeScript generation adds only the optional hold argument. The larger Candid diff also contains source-exported method documentation.

[Native results and log hashes](evidence/vault208-validation-20261117/native-results.json) preserve the actual final-stage failure and the separate mechanical resolution rather than rewriting the run as wholly successful.

## Remaining boundary

Freeze these generated files before Cody admits the one separate backend-only offline release build. The build must preserve raw, optimized and gzip artifacts and check embedded Candid, code size, production markers and source identity. Existing native results are reused unless source changes invalidate them; no frontend, Stability Pool, bot or hosted-CI build is queued here.

The current later production backend and its pause are separate from this older offline candidate. A successful build is not permission to overwrite it. Composition, exact deployment approval, compensation and a new explicit approval to resume Stability Pool liquidations remain separate. Keep new Stability Pool liquidations paused throughout.

Proposed durable engineering lesson: financial test fixtures should assert their raw-unit and eligibility premises, and custody tests should use the actual persisted identity tuple. This is a proposal for the existing persona/catalog owner, not a memory or catalog edit.
