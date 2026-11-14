# XLM settlement funding and sequencing preflight: source complete (UNBUILT)

Recorded 2026-11-14. Status: source accepted by independent review as an **unbuilt** candidate. Not compiled; unit tests are **written, not run**. No production admission and no quorum proof is claimed.

## Code

Implementation commit `d38b6e46a25f182d3e1a3b429dfef4c66b04252b`, `src/rumi_protocol_backend/src/state.rs` only (+564 lines), all under `#[cfg(any(test, feature = "test_endpoints"))]` and wired to no endpoint.

- `State::admit_xlm_settlement_preflight` (read-only, typed `XlmAdmitError`), in order:
  1. Claim exists, is Pending, has no prepared or signed intent; claim network matches the selected network.
  2. The custody address attributed to the claim's custody owner, nonce and network is unique and equals the account snapshot's account.
  3. Reserve evidence is present and fresh under the existing `decide_evidence_freshness` rules; missing evidence is an explicit denial.
  4. Sequencing: one in-progress payment per custody account. Any sibling claim for the same custody owner, nonce and network that is not Settled and has a prepared intent, a signed intent or InFlight status denies. Settled siblings are skipped.
  5. Funding with checked arithmetic: native balance minus the live minimum reserve (existing `minimum_balance_stroops`), minus selling liabilities and the gross of every unresolved sibling (any status other than Settled, including Failed and Quarantined), must cover this claim's gross. The fee is inside gross (net + fee = gross), and the actual fee is bounded later by `prepare_xlm_settlement_intent`.
- `State::admit_and_prepare_xlm_settlement_intent`: the recommended admission wrapper. Runs the preflight, then the unchanged `prepare_xlm_settlement_intent`, which remains public and directly callable.
- Inputs (`AccountSnapshot`, `LatestLedgerReserve`) are plain parsed data. Their types carry no proof that a provider quorum ran; they are untrusted test inputs.
- `mod xlm_admission_tests`: 10 unit tests (funding boundary and one stroop short; reserve, sponsorship and liability shifts; gross-only sibling accounting; Settled and other-custody exclusion; prepared, signed or in-flight sibling denial; stale, missing or mismatched evidence; claim preconditions; overflow; sponsorship underflow; successful admit and prepare with net + fee = gross). Every denial leaves stored claims byte-identical.

Unchanged: `prepare_xlm_settlement_intent` and its 5 passing tests (receipt `5f7f2a1d57310ec6c9e916fd81270df24c2fe5fb16e0d2573aebb366b9def3ac`); existing deposit credit logic, which separately and more conservatively reserves gross plus fee for unsettled claims.

Decisions applied: unresolved means every status except Settled; a sibling with any intent blocks until reconciliation, with no new status policy.

## Prepared focused validation (NOT RUN)

Runner `/private/tmp/xlm-admission-d38b6e46-docdesc-runner.py`, SHA-256 `9bc4624cbd78b1439b27e75979599c00037f1cfeafe50c793f8e4b2da1182ce4`, a copy of the accepted runner `da56c224` with only the header, base commit, `state.rs` pin, test command, exact count and output and lock paths changed.

1. `cargo test --locked --offline -p rumi_protocol_backend --lib -- xlm_prepare_intent_tests xlm_admission_tests`: exactly one summary `test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; N filtered out;` (10 new, 5 prior prepare tests as a direct-wrapper regression).
2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (features off).

Pins at the implementation: `state.rs` `811268aa64a891273a36038f2a9db4b391dceea1d82876b7f564416108d978c7`, `vault.rs` `a21dba88…`, `chains/stellar/deposit_evidence.rs` `00b8c36a…`, `Cargo.lock` `87d52fdd…`; toolchain `rustc 1.92.0`, `cargo 1.92.0`. Needs a fresh exclusive allocation.
