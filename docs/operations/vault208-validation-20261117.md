# Vault 208 offline validation and artifact proof — November 17, 2026

**The focused tests and one source-matched backend build are complete. This is offline artifact proof, not installation readiness.** No deployment, compensation, financial action or liquidation resumption was performed. The current deployed retry-bound backend remains untouched, with the Stability Pool new-liquidation pause freshly read as true.

## Source and actual tests

The production sizing correction remains the `c2cbffc8` implementation. Three test-only corrections followed: byte-typed stable-cell reopen defaults (`686ba519`), claims identified by their actual custody owner/nonce rather than a nonexistent vault ID (`a72fde2c`), and a dispatch fixture using its intended $100 debt rather than $1, with explicit non-dust and 130% collateral-ratio premises (`9b477232`). Production math was not changed to satisfy those tests.

One isolated-cache batch on `9b4772322af3a5cb5cb544cb98a518e4ff1eecd0` passed **145 XRP tests, four stable hold tests and one exact canonical Candid exporter test**. Backward compatibility, canonical declaration regeneration and strict backend/declaration equality passed. The earlier compilation failure and 144-pass/one-fixture-failure batch remain retained; their allocations were closed and never replayed.

The final diff check initially failed on 50 trailing-space blank comment lines in each generated Candid copy. The unmodified exporter output and original failing receipt/log remain unchanged. Mechanical line-end-whitespace normalization then passed strict equality against raw output, backward compatibility, declaration equality and `git diff --check`. No API was hand-merged or native test rerun for formatting. Canonical JS/TypeScript generation adds only the optional hold argument; source-exported documentation explains the larger DID text diff.

[Native results and log hashes](evidence/vault208-validation-20261117/native-results.json) preserve the original final-stage whitespace failure and its separate mechanical resolution.

## One source-matched artifact

The generated-source freeze **`afedab2993e87744a9a4e1a14efa59934815e975`**, tree `8755049c0b042fc375ed7275a7a8c996c434e812`, produced these exact private artifacts:

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Original Cargo Wasm | 16,448,348 | `d4549d07a3312823fac2ec489a8efe9dcde691321aba61ae833fd9b606e77e14` |
| Optimized Wasm | 11,452,345 | `7fc4d5ba5dddc58611b765ee24899e92659496eeea2dba9d5238678409cff193` |
| Deterministic gzip | 2,797,987 | `8718b644c1f42da904e1a8d20e9aa7bb584b73253e2a0fddc336d411ead31f37` |

All seven canonical commands exited zero: offline/locked/jobs-one backend release compiler, raw copy, Candid metadata, shrink, O1 optimization, deterministic gzip and mainnet marker guard. Cargo JSON binds the exact backend binary source/output, default features and size profile `z`. All 134 source pins, lockfile, recipe and tool inputs remained unchanged. Embedded canonical Candid is byte-exact (`92d6e8353fcd61194f0e9034245b44293105d1f8a3978dd7ce3fb854d6681524`); gzip round trip and zero mtime pass. Raw, optimized and compressed bytes remain preserved privately. No second producer or native test repeat ran.

The [original Wasm receipt](evidence/vault208-validation-20261117/wasm-original-receipt.json) remains **STOPPED** with SHA256 `70d1f3e276535db0e07e5d385f33503a4f592d7ee6c4d068a4101246debfd3b3`. Its two reporting/criterion defects are separately reconciled, not erased:

- The private runner introduced an older 10 MiB minus 256 KiB assertion (10,223,616 bytes). No accepted current Rumi source gate requiring that number was found. This artifact has 10,320,295 bytes of executable code, so that private assertion failed; this was not a replica rejection. Rob's current instructions identify a 12 MiB network limit. The already accepted [Rumi release packet, line 17](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/2f5b2b62476979e2c4e611a06ba8d478628d3c45/docs/operations/xrp-cycle-bound-deployment-20261116.md#L17) explicitly uses 12 MiB with the established 256 KiB reserve and admits its 10,317,840-byte code section. Its [live receipt, lines 3–5](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/8a42c5212f7f7a8a688c51e5c4d1b4bf856b17ac/docs/operations/xrp-cycle-bound-live-result-20261117.md#L3) records exact approval and installation. Existing source `Cargo.toml:22–27` and `icp.yaml:617–618` retain the reserve. The prior private release runner `/private/tmp/rumi-cycle-burn-20261116/run-release-2ff2723a.py:134` uses `12_582_912 - 262_144`. Against that existing criterion, this same artifact passes with **2,000,473 bytes beyond the reserve**. No guard source was changed and no speculative size repair is required by this offline task.
- The inherited section parser reused the name `start` for a payload offset after all monitored commands, producing an invalid negative elapsed value. The precomputed active deadline and resource accounting were unaffected. The executed runner and receipt are unchanged. Exclusive consume epoch to original terminal-receipt mtime gives **220.640224 seconds of wall time**, within the 1,800-second allocation; this is a wall-clock reconciliation, not a repaired monotonic sample.

[Separate read-only artifact and timing reconciliation](evidence/vault208-validation-20261117/wasm-result-reconciliation.json) records every artifact/log hash and those source citations. Cody independently verified all seven process groups empty, unchanged source/inputs, Cargo provenance, embedded Candid and compressed bytes. Peak observed owned RSS was 2,044,641,280 bytes; maximum observed physical growth 163,217,408 bytes; final free space 45,723,877,376 bytes. Resource bounds were preserved and the allocation was released.

## Live boundary and next scope

Fresh read-only checks after validation confirm installed module `ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f` and `get_sp_writedown_disabled() = true`. This older offline candidate was not installed. Any later deployment proposal must preserve the current retry bound and pause support; source composition and an exact deployment decision are separate work, not a gate added to these completed offline tests/build. Compensation and resumption remain separately unauthorized. No hosted CI, main merge, frontend/SP/bot build or financial canary is claimed.

Proposed durable engineering lessons for the existing persona/catalog owner: financial fixtures should assert raw-unit and eligibility premises; custody tests should use persisted identity tuples; timers and binary-section cursors need distinct names; a helper assertion must be checked against the accepted product criterion before it is presented as required repair work. No memory/catalog edit or new process is introduced here.
