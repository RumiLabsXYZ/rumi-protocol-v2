# BOT-001 PocketIC 9 proof checkpoint

This checkpoint records the exact production-Wasm runtime run before isolating
PocketIC 9 into this audit package.

- Production gzip Wasm SHA-256: `4b2d0f8bce2694f54672a9ed8546048dfa2c26f95bfcab1df14512a10873f140`
- Decompressed raw Wasm SHA-256: `571d45032008755e36d590f29ab8fe67b6c077ff0888bc132b2ff18c3b578a99`
- PocketIC Rust crate: `9.0.2`
- PocketIC server version output: `pocket-ic-server 9.0.3`
- Runtime command: `cargo test --offline -p rumi_protocol_backend --features pocketic9_wrapper --test audit_pocs_bot_001_auto_cancel_balance_pic -- --nocapture`
- Environment: `RUMI_BACKEND_TEST_WASM` pointed to the raw Wasm with the SHA-256 above; `POCKET_IC_BIN` pointed to the cached PocketIC 9.0.3 binary through a `/private/tmp/pocket-ic` symlink.
- Runtime result: `running 9 tests`; `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 22.13s`.
- Existing PocketIC 6 target compile-only result: `Finished test profile`; its executable was produced successfully.
- `git diff --check`: passed.

The run installed the exact decompressed production backend Wasm bytes in
PocketIC. Its ICRC ledger and XRC canisters came from the repository's checked-in
fixtures `src/ledger/ic-icrc1-ledger.wasm` and
`src/xrc_demo/xrc/xrc.wasm`. The result proves backend behavior against those
fixtures; it does not prove native ICP production-ledger archive semantics. No
artifact was optimized, rebuilt, installed outside PocketIC, or changed by
this proof.

## Standalone audit package

The isolated runner is `run.sh`. It verifies the raw Wasm hash before testing,
checks server output for `pocket-ic-server 9.0.3`, creates a temporary
`pocket-ic` executable symlink under `${TMPDIR:-/tmp}` before checking the
server version (the cached executable may require that basename), and removes
only temporary fixtures it created. Example invocation:

```bash
POCKET_IC_SERVER_BIN=/path/to/pocket-ic-server \
  audits/bot001-pocketic9/run.sh /path/to/exact-backend.wasm
```

Standalone result with the same exact artifact: `running 9 tests`; `test
result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
finished in 20.93s`. The runner's Cargo invocation is offline and uses a
separate audit `Cargo.lock`; the root workspace lockfile and backend
dependencies remain unchanged.

After correcting the runner to validate the cached server through its
temporary `pocket-ic` symlink, the final rerun again verified raw Wasm SHA-256
`571d45032008755e36d590f29ab8fe67b6c077ff0888bc132b2ff18c3b578a99` and
server output `pocket-ic-server 9.0.3`, then passed all nine tests in 24.05s.

Final portability check used a relative `POCKET_IC_SERVER_BIN` path; the runner
canonicalized it, used `${TMPDIR:-/tmp}`, verified the same artifact and server
hash/version, and passed all nine tests in 20.72s. A deliberately wrong Wasm
input was rejected by the SHA-256 check before Cargo or PocketIC was invoked.
