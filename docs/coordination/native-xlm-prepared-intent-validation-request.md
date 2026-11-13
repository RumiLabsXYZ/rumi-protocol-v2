# Request: focused validation of the dormant XLM prepared settlement intent

Prepared 2026-11-12. For review. Nothing here has been run, and earlier partial- and full-withdrawal grants are consumed and are not authority for this batch. A fresh exclusive allocation is required.

## What would be validated

Implementation `98b35c9a6f5bc167f468dd70f76cdb3bd913a1c9` (dormant, test-only `prepare_xlm_settlement_intent` and the new optional `prepared_intent` claim field; see `native-xlm-prepared-intent-source-2026-11-12.md`). Run-time HEAD may be any documentation-only descendant (only `docs/` changed since the implementation); currently `414df3e4` plus this note's commit.

## Commands (two, in order, stop at first failure)

1. `cargo test --locked --offline -p rumi_protocol_backend --lib xlm_prepare_intent_tests`
   - Must print exactly one summary line: `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; N filtered out;`
   - 5 is the exact number of `#[test]` functions in `state.rs` `mod xlm_prepare_intent_tests` at the implementation commit (`happy_path_binds_every_field`, `rejections_leave_claim_byte_identical`, `fee_above_u32_with_large_reserve_and_gross_is_rejected`, `double_prepare_is_rejected_and_first_preserved`, `old_claim_without_prepared_intent_decodes_as_none`). The filter matches no other test, so a zero-match run cannot pass.
2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (production, features off).

Environment: `CARGO_BUILD_JOBS=2`, `BINARYEN_CORES=2`; `CARGO_TARGET_DIR`, `POCKET_IC_BIN` and any API or alternate-provider variables removed.

## Limits and safety

30 minutes including a 60-second closure reserve; 30 GiB free to start (allocation-set) and at least 20 GiB while running; at most 8 GiB disk growth and 8 GiB memory for owned processes; no other cargo or rustc running at start. Stop at the first failure, limit or unknown condition; no retry. Only owned process groups are shut down and each must be confirmed empty. Native exit codes and log hashes are recorded. No install, deployment, signing, Stellar transaction, funds, paid API, credits or hosted CI.

## Runner

Candidate `/private/tmp/xlm-prepintent-98b35c9a-docdesc-runner.py`, SHA-256 `da56c2242a3f4063f7e5662e940b9493bfb774a5c2f1f7144b684f8a5fa0a019`, derived from the accepted runner `cb1c89da` (unchanged on disk). Differences:

- Implementation base `98b35c9a`; documentation-only descendant rule and allocation `sourceHead` = run-time HEAD unchanged.
- Input pins (SHA-256 at the implementation): `state.rs` `a13d17fb7eafe8ebbc28772f9044fb1601b790e121e4eb176d50417eb11f0452`, `vault.rs` `a21dba881cde962db4a289c7936023f6b4cb12025f1b7ecefe6f97d2734924b6`, `chains/stellar/deposit_evidence.rs` `00b8c36a8cd0fe838c9e6d8b6b3c90b68c8296302edcd0294b6c07793193d676`, `Cargo.lock` `87d52fdd675d71d2201fb511099c3a2f52eefa7575da7fe4bb95e670173a3187`; toolchain `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`.
- The two commands above; acceptance requires exactly one summary line matching the exact count.
- Removed because neither command uses them: PocketIC binary and version pins, the XRC and ledger Wasm pins, the localhost-bind preflight, and the test-Wasm artifact hash checks. No artifact proof is claimed.
- New output and lock paths (`/private/tmp/xlm-prepintent-98b35c9a-docdesc/`, `.lock`).

Unchanged: allocation record and hash gate (exclusive, start-by deadline, limits only tightened), occupancy check, exclusive lock and output directory against replay, per-step source and toolchain checks, resource monitoring, owned process-group closure, native exits and log hashes. The runner never changes repository source or switches branches; build outputs and logs are written only by the allocated batch.

Invocation, only after a new allocation exists: `python3 -I /private/tmp/xlm-prepintent-98b35c9a-docdesc-runner.py <allocation.json> <allocation-sha256>`, with the allocation naming this runner hash, `sourceHead` = run-time HEAD, `exclusive: true`, the native-xlm-collateral worktree and a future start-by time.
