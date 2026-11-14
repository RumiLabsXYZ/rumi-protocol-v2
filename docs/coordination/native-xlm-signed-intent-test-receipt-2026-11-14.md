# XLM signed settlement intent recording: focused local validation result

Recorded 2026-11-14. Result: **PASS**, independently verified. Unit-test and compile evidence only, with a fixture signature.

## What ran

- Allocation `589054a4-83df-4065-8264-2fd2968652ff`, exclusive, record SHA-256 `67ae8b77bafda024ad7198f9326105a96cc39a2506772fbd44851003540d1776`; slot released afterwards. Earlier grants were not reused.
- Runner SHA-256 `370e949d51d234f04d2b284b2313855846842d0b9e630df429fbc339c1dea82f`, invoked once.
- Implementation `a1760abb3088b38f7435fa28264d4501ae0c849b`; run-time HEAD `8a26dcacbc6cb3869caf3fd2b84ae8686b3ab6bd` (only `docs/coordination/` files differ). Pins: `state.rs` `9fcfa3af…`, `vault.rs` `a21dba88…`, `chains/stellar/deposit_evidence.rs` `00b8c36a…`, `Cargo.lock` `87d52fdd…`; `rustc 1.92.0`, `cargo 1.92.0`.
- 2026-11-14 20:07:45Z to 20:08:42Z (57 s). Limits: 30 minutes including 60 s closure, 30 GiB startup and 20 GiB runtime free floors, 8 GiB growth and memory, 2 jobs.

| Step | Exit | Result | Log SHA-256 |
|---|---|---|---|
| 1. `cargo test --locked --offline -p rumi_protocol_backend --lib -- xlm_prepare_intent_tests xlm_admission_tests xlm_signed_intent_tests` | 0 | `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 1500 filtered out` | `bc67029f476949e2334e92366f0027712f6fa9e296872c8fec2ec37f5cd3b367` |
| 2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (features off) | 0 | clean | `112adcdacc5564e2ec26e6f7d379f2d58e7f269406ccfd1e783e9efb5198ac71` |

- 5 signed-intent tests, plus the 10 preflight and 5 prepare tests as regression, all passed.
- Resources: lowest free disk 37.0 GiB, growth 0.72 GiB, peak memory 3.15 GiB.
- Both owned process groups closed and independently confirmed empty; source clean after the run.
- Terminal receipt SHA-256 `4e2ae10ef05343749ed1f56a496a14d39bb4a5ee58dab8fce71f7d1142a9213e`.

## What this proves

With a caller-supplied fixture signature, the test-only recording step rechecks every prepared binding and the payload hash against the current claim, rejects drift, repeats and conflicting signatures, denies when another unresolved claim for the same custody account is in flight or holds a prepared or signed intent (Settled siblings and other custody accounts do not block), builds the exact envelope with the existing codec, and stores the signed intent while marking the claim InFlight, changing nothing on any rejection. Earlier preflight and prepare behaviour is unchanged, and the production build compiles with the test-only code excluded.

## What it does not prove

- The signature is an untrusted fixture and is not verified; no real threshold signing has run.
- No submission to Horizon, status reconciliation, settlement or payout exists.
- Nothing ran on testnet or with real funds. Production XLM remains Frozen.
