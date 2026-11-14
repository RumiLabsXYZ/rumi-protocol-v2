# XLM prepared settlement intent: focused local validation result

Recorded 2026-11-14. Result: **PASS**, independently accepted. Unit-test and compile evidence only.

## Approval

Rob approved the focused prepared-intent check (five unit tests plus production check). That approval is answered and consumed by this single run. An earlier wait was caused by writable disk below the 30 GiB startup floor; floors were not lowered, and the run started only after capacity was available and a fresh exclusive slot was issued. Cody has released the slot. Earlier grants were not reused.

## What ran

- Allocation `c158cebc-8599-4da4-aa5e-b642d5b871f6`, exclusive, record SHA-256 `40c9cabe407aefe8d6134df52fefc3cfae74331e780a10df369a78100d991fa4`.
- Runner SHA-256 `da56c2242a3f4063f7e5662e940b9493bfb774a5c2f1f7144b684f8a5fa0a019`, invoked once.
- Implementation `98b35c9a6f5bc167f468dd70f76cdb3bd913a1c9`; run-time HEAD the approved producer `10b064c919518744bb9c4eef5bdce344a2d1fcf7` (only `docs/coordination/` files differ). Pins: `state.rs` `a13d17fb…`, `vault.rs` `a21dba88…`, `chains/stellar/deposit_evidence.rs` `00b8c36a…`, `Cargo.lock` `87d52fdd…`.
- Toolchain: `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`.
- 2026-11-14 18:37:33Z to 18:38:35Z (62 s). Limits: 30 minutes including 60 s closure, 30 GiB startup and 20 GiB runtime free floors, 8 GiB growth and memory, 2 jobs.

| Step | Exit | Result | Log SHA-256 |
|---|---|---|---|
| 1. `cargo test --locked --offline -p rumi_protocol_backend --lib xlm_prepare_intent_tests` | 0 | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1500 filtered out` | `4fd23705fde4618fc0ee049f820f5f48dd83c2a0cd0862b5128958c9356659d7` |
| 2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (features off) | 0 | clean | `f80b1b156f28daeab729beaff8efddc283dbfd5708266e0bc81d93b61d2d1fd1` |

- Tests that passed: `happy_path_binds_every_field`, `rejections_leave_claim_byte_identical`, `fee_above_u32_with_large_reserve_and_gross_is_rejected`, `double_prepare_is_rejected_and_first_preserved`, `old_claim_without_prepared_intent_decodes_as_none`.
- Resources: lowest free disk 30.4 GiB, growth 0.61 GiB, peak memory 2.96 GiB.
- Both owned process groups closed and independently confirmed empty. Source clean after the run.
- Terminal receipt SHA-256 `5f7f2a1d57310ec6c9e916fd81270df24c2fe5fb16e0d2573aebb366b9def3ac`.

## What this proves

The dormant test-only preparation binds every field of an unsigned settlement intent, leaves the stored claim byte-identical on every rejection, rejects an oversize fee and a second prepare, and older stored claims load without a prepared intent. The production build compiles with the test-only method excluded.

## What it does not prove

- The method is test-only, wired to no endpoint, and treats the account snapshot as untrusted fixture data with no quorum or freshness proof.
- Signing, submission, reconciliation of unknown or expired transactions, settlement status changes and payout are not built.
- Nothing ran on a live network or with real funds. Production XLM remains Frozen.
