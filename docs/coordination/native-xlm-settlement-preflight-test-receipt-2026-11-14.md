# XLM settlement funding and sequencing preflight: focused local validation result

Recorded 2026-11-14. Result: **PASS**, independently accepted. Unit-test and compile evidence only.

## What ran

- Allocation `2f7f305d-9657-48bc-86ca-92e0ce2062b9`, exclusive, record SHA-256 `663a2feea749c5f20dbe36257a8b5d5a1fbdb28d0bb33d7a195fd3202dabb28c`, issued under Rob's standing continuation directive; slot released afterwards. Earlier grants were not reused.
- Runner SHA-256 `9bc4624cbd78b1439b27e75979599c00037f1cfeafe50c793f8e4b2da1182ce4`, invoked once.
- Implementation `d38b6e46a25f182d3e1a3b429dfef4c66b04252b`; run-time HEAD `4b5806b1550c0cfc559e58d10add9fd1ac002c32` (only `docs/coordination/` files differ). Pins: `state.rs` `811268aa…`, `vault.rs` `a21dba88…`, `chains/stellar/deposit_evidence.rs` `00b8c36a…`, `Cargo.lock` `87d52fdd…`.
- Toolchain: `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`.
- 2026-11-14 19:35:19Z to 19:36:22Z (63 s). Limits: 30 minutes including 60 s closure, 30 GiB startup and 20 GiB runtime free floors, 8 GiB growth and memory, 2 jobs.

| Step | Exit | Result | Log SHA-256 |
|---|---|---|---|
| 1. `cargo test --locked --offline -p rumi_protocol_backend --lib -- xlm_prepare_intent_tests xlm_admission_tests` | 0 | `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1500 filtered out` | `36624f90fc40230a83949c451941c849665635cfbfa16d36b879f653133b2e57` |
| 2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (features off) | 0 | clean | `86f893ab11a5826d97fcbb9cbaeb0415daf061bced90ec33c3a986f4fd5bca0f` |

- 10 new preflight tests and the 5 earlier prepare tests (regression) all passed.
- Resources: lowest free disk 37.9 GiB, growth 0.64 GiB, peak memory 3.11 GiB.
- Both owned process groups closed and independently confirmed empty. Source clean after the run.
- Terminal receipt SHA-256 `0aa05cb51a66f27002448741091d7d56ecda49a5c46c74ed9e7ecab0b59cf17d`.

## What this proves

The test-only preflight admits a claim exactly at the gross funding boundary and denies one stroop short; accounts for the live minimum reserve, sponsorship and selling liabilities; reduces spendable by unresolved sibling gross only and excludes Settled and other-custody claims; allows one in-progress payment per custody account; denies stale, missing or mismatched evidence and overflow; and changes nothing on any denial. Admit-and-prepare stores the prepared intent. The earlier prepare behaviour is unchanged, and the production build compiles with the test-only code excluded.

## What it does not prove

Inputs are plain parsed data, not quorum proof. No signing, submission, reconciliation or payout exists. Nothing ran live. Production XLM remains Frozen.
