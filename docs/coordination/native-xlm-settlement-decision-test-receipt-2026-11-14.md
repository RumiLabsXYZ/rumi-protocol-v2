# XLM settlement decision application: focused local validation result

Recorded 2026-11-14. Result: **PASS**, independently verified. Unit-test and compile evidence only, with fixture decisions.

## What ran

- Allocation `7da2ec5a-cf31-43a2-b387-56af6165736f`, exclusive, record SHA-256 `1bc29dcecbaf6bd1e09893f5da7776a17f7c3b953c4622e5d5526172cecfd1c7`; slot released afterwards. Earlier grants were not reused.
- Runner SHA-256 `81fed1019dca73bc01d270aec6c9c6fd083092956f0c002c42a275b526f0e794`, invoked once.
- Implementation `f1f0ed8217fd365ebeb750341c6cc7ae84418cf7`; run-time HEAD `c0b3e5fb4c945224008ae1e09b317c84b91b0b69` (only `docs/coordination/` files differ). Pins: `state.rs` `a64154c7…`, `chains/stellar/transaction_status.rs` `43aad1b2…`, `vault.rs` `a21dba88…`, `chains/stellar/deposit_evidence.rs` `00b8c36a…`, `Cargo.lock` `87d52fdd…`; `rustc 1.92.0`, `cargo 1.92.0`.
- 2026-11-14 20:28:13Z to 20:29:06Z (54 s). Limits: 30 minutes including 60 s closure, 30 GiB startup and 20 GiB runtime free floors, 8 GiB growth and memory, 2 jobs.

| Step | Exit | Result | Log SHA-256 |
|---|---|---|---|
| 1. `cargo test --locked --offline -p rumi_protocol_backend --lib -- xlm_prepare_intent_tests xlm_admission_tests xlm_signed_intent_tests xlm_apply_decision_tests` | 0 | `test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1500 filtered out` | `831a1e739d0d0dcb6a79a976484ea5ce0461c9611b5e611917c377748ca91980` |
| 2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (features off) | 0 | clean | `695f1cd9b2cd053c881d35164b93c30de6b2d85a632989b3dd5eeec2e3733ff3` |

- 7 decision-application tests, plus 5 signed-intent, 10 preflight and 5 prepare tests as regression, all passed.
- Resources: lowest free disk 29.9 GiB, growth 0.24 GiB, peak memory 2.94 GiB.
- Both owned process groups closed and independently confirmed empty; source clean after the run.
- Terminal receipt SHA-256 `a65c3d4be63214eece364915e714db34cc4096f3985d986f76173e842f038369`.

## What this proves

Given a caller-supplied decision fixture, the test-only method applies it to an in-flight claim only when the supplied and embedded hashes parse strictly (either hex case) to the stored digest: settled keeps intents, hold changes nothing, quarantine keeps intents, replacement clears both intents back to Pending, and a failed transaction's fee is charged once from gross within the prepared fee and reserved budget, keeping the per-payment cap and allowing a fresh prepare. Every rejection changes nothing. Earlier signed-intent, preflight and prepare behaviour is unchanged; the production build compiles with the test-only code excluded.

## What it does not prove

- Decisions are untrusted fixtures, not a real provider status quorum, and no real fee was charged on any network.
- No real signing, submission to Horizon or payout exists.
- Nothing ran on testnet or with real funds. Production XLM remains Frozen.
