# Borrow mint developer recovery on PocketIC 9

This audit runs the borrow-mint recovery integration cases against a backend Wasm built from this worktree's current source. The standalone package isolates PocketIC client 9.0.2 from the workspace's PocketIC 6 dependency. It does not change production Rust or the original ignored test file.

## Source and artifacts

- Source commit: `acae0daea866d61a9947a1a7ceda475f43adc176` (`codex/backend-pocketic9-proofs-20261010`)
- Backend build: `cargo build --offline --release --target wasm32-unknown-unknown -p rumi_protocol_backend --bin rumi_protocol_backend --features test_endpoints`
- Backend raw Wasm SHA-256: `6e539c6073b628745b57338fc69128f82cf698a93fa455c108328f5676e60aaf`
- Backend Wasm length: 13,462,125 bytes
- Wasm code section length: 11,376,330 bytes (10.849 MiB), below PocketIC 9's 11 MiB code-section ceiling
- Flaky ledger build: `cargo build --offline --release --target wasm32-unknown-unknown -p flaky_ledger`
- Flaky ledger Wasm SHA-256: `a10ab2ff6fc3e007f16be1b9a191016fe0ebb324eb785fb723cb6b32d37f5b91`
- Checked-in XRC fixture SHA-256: `9d48047ad33db38b7b511a3ee44732927a0fbdbc857ddfe83ff007293c247c63`
- PocketIC client: 9.0.2; server: `pocket-ic-server 9.0.3`

## Run

```bash
POCKET_IC_BIN=/absolute/path/to/pocket-ic-9.0.3 \
  audits/borrow-mint-pocketic9/run.sh
```

The runner builds backend and flaky-ledger Wasms in this audit package's isolated `target/`, supplies their exact paths to the test process, verifies the server version, and runs Cargo offline. The backend Wasm came from the worktree root shown above (Cargo compiler output confirmed `.worktrees/backend-pocketic9-proofs-20261010/src/rumi_protocol_backend/src/main.rs`).

Result with loopback access after adding the owner-authorized exact-receipt case: **4 passed, 0 failed, 0 ignored**. The first three cases took 12.29 seconds before the owner case was added; the current four-test result is recorded in `RESULT.md`.

- `developer_reconciles_exact_committed_mint_for_owner_once`
- `developer_scans_exact_committed_mint_for_owner_once`
- `developer_clears_typed_too_old_with_complete_nonempty_history`
- `owner_reconciles_exact_committed_mint_for_owner_once`

These cases cover owner attribution, denial for a stranger, wrong-block rejection, one-time debt application, no duplicate mint/block, stale-journal rejection, and complete nonempty history before absence recovery. The third case checks that a typed `TooOld` is cleared only after scanning the complete history, including its configured archive boundary.

The original test file is annotated ignored for the incompatible workspace PocketIC 6/server 7 route. This package copies those cases, removes their ignore markers, adds the owner-caller regression, adapts the PocketIC 9 response type, and points its XRC fixture include at the checked-in file. The scenarios use a deliberately configured flaky ledger to simulate a committed mint with a lost response and a typed `TooOld` duplicate; they do not reproduce a production ledger's real timestamp-expiry implementation. This is local integration evidence for this exact source commit, not deployment, live-ledger, or funds evidence.
