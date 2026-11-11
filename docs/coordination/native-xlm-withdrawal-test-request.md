# Request: one focused local test build for the XLM test-only withdrawal

Prepared 2026-11-11. This is a request for review. Nothing in it has been sent or run.

## What is being asked

Explicit approval for **one** focused local build-and-test batch of the native-XLM test-only withdrawal code, run by Mara in the existing native-XLM worktree. The batch only compiles and runs local mocked tests. It does not install anything, deploy, sign, submit a Stellar transaction, move funds, use paid APIs or credits, or use hosted CI. Production XLM stays Frozen and its route guards stay unchanged.

The approval being asked for is new and narrow: this one XLM withdrawal test execution and its delivery to Mara. The earlier repay batch (receipt `ff11bfa4c9666143f5229121681a66cc1e4b158851995fc5daa79b6cfd293ff9`, PASS) and existing scheduling arrangements are unchanged. The repay pass is not authority for this batch.

## Code under test

- Final implementation commit: `b68748969d655e1c161e31bc3f388baaecff33e0` (on top of `42edb2f3`), branch `codex/native-xlm-collateral`.
- Purpose: test-build-only partial withdrawal that records a durable XLM claim instead of any token transfer. Owner only, collateral-ratio limit, the same price-freshness guard as production partial withdraw, claim recorded before the vault is debited. No signing or payout.
- The extended test `xlm_confirm_pic` checks that outsider, ratio-breaking and bad-destination withdrawals are rejected with balances and supply unchanged and debt changing only within the bounded accrued-interest allowance, and that the owner's withdrawal creates one claim with the right custody owner, nonce, exact testnet network id, amount and fee.
- Status: independent source review accepted it as an unbuilt candidate. It has never been compiled or run.

## Exact commands (run in order, stop at the first failure)

Worktree: `/Users/robertripley/coding/rumi-protocol-v2/.worktrees/native-xlm-collateral`, default target directory and existing cache.

1. `cargo build --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown --release --features test_endpoints`
2. `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test --locked --offline -p rumi_protocol_backend --test xlm_confirm_pic` (must report exactly `test result: ok. 1 passed; 0 failed; 0 ignored;`)
3. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (production features off)

Environment: `CARGO_BUILD_JOBS=2`, `BINARYEN_CORES=2`; `POCKET_IC_BIN` only for step 2; `CARGO_TARGET_DIR` and any API or alternate-provider variables removed.

## Limits

- 30 minutes total, including a 60-second closure reserve.
- At least 30 GiB free to start, at least 20 GiB free while running.
- At most 8 GiB disk growth and 8 GiB combined memory for the batch's own processes.
- Stop at the first failure, limit or unknown condition. No retry, no correction, no follow-on run.
- Only the batch's own process groups are shut down, and each must be confirmed empty at the end.

## Runner

The originally prepared runner cannot be used as is. Runner `/private/tmp/xlm-withdraw-b6874896-runner.py` (SHA-256 `47c84a57a4ec999cf2fee9ec1e73b945c69244fd78b89cd341ae17ea73b0497e`, kept unchanged) requires the worktree HEAD to be exactly `b6874896`. The branch has since gained documentation-only commits (board updates and this request), so HEAD is no longer `b6874896` and that runner would stop before starting.

Revised candidate for review: `/private/tmp/xlm-withdraw-b6874896-docdesc-runner.py`, SHA-256 `d78c964a06b72cede2dc52482eab5ed06f32c095281ff8ca47ee6cfc57b76fb4`. Differences from `47c84a57` only:

- The allocation's `sourceHead` must be a full commit hash, must equal the worktree HEAD at run time (checked before every step and at the end), must descend from `b6874896`, and every file changed since `b6874896` must be under `docs/`.
- Adds pins for `main.rs` and `vault.rs` alongside the existing test-file, `Cargo.lock`, ledger, XRC and pocket-ic pins.
- Separate output and lock paths (`/private/tmp/xlm-withdraw-b6874896-docdesc/`, `.lock`), so it cannot collide with the original.

Everything else is identical to the reviewed runners: the three commands, environment, limits, toolchain pins, localhost bind preflight, step 1 test Wasm hash kept unchanged through steps 2 and 3, exclusive lock and output directory, and owned process-group closure. The runner never changes repository source or switches branches; build outputs and logs are written only by the allocated batch.

Input pins (SHA-256 at `b6874896`):

| Input | SHA-256 |
|---|---|
| `src/rumi_protocol_backend/src/main.rs` | `db9ca8832677ab116a7ec87c2c447a4066316e10a9cc11ce27be9574e50f70a1` |
| `src/rumi_protocol_backend/src/vault.rs` | `cddd10ab9f64bd334bf98c5f2757c2987edb15111e617cbeb560d571249326a3` |
| `src/rumi_protocol_backend/tests/xlm_confirm_pic.rs` | `b235a48a63ee740e877d7845621a743d2f4e965e0858b1240f9fa4be939276b9` |
| `Cargo.lock` | `87d52fdd675d71d2201fb511099c3a2f52eefa7575da7fe4bb95e670173a3187` |
| `src/xrc_demo/xrc/xrc.wasm` | `9d48047ad33db38b7b511a3ee44732927a0fbdbc857ddfe83ff007293c247c63` |
| `src/ledger/ic-icrc1-ledger.wasm` | `cb0c3233ebb137f606d7928e90bfd907fe932a941f11b41c23cf0d6cf9c64802` |
| `pocket-ic` binary | `87582439bf456221256c66e86b382a56f5df7a6a8da85738eaa233d2ada3ed47` |
| Toolchain | `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567c 2025-10-21)`, pocket-ic `7.0.0` |

Invocation, only after a new allocation exists: `python3 -I /private/tmp/xlm-withdraw-b6874896-docdesc-runner.py <allocation.json> <allocation-sha256>`. The allocation must name `runnerSha256` = `d78c964a06b72cede2dc52482eab5ed06f32c095281ff8ca47ee6cfc57b76fb4`, `sourceHead` = the run-time HEAD, `exclusive: true`, the worktree above and a future start-by time.

## Previous attempt and why it stopped

Allocation `fd31b869-61d2-470e-a4ba-71538ce142e1` was released. Nothing was delivered, the attempt was not consumed and no runtime started. An action ID for the rejected message was not recorded and is unavailable. The rejection is not being retried or relayed, and no alternate sender was used.

The automatic review's reason, verbatim:

> The message authorizes a new heavy XLM local build for Mara, but the trusted user instructions authorize the NEAR/Cody work and do not explicitly approve this separate XLM execution or destination.

The full record is copied as `docs/coordination/native-xlm-withdrawal-admission-denial-2026-11-11.json`.

## Proposed new admission message

From Cody (thread `01a0eeee-d768-71b1-8090-0ca4e680e5b3`) to Mara (thread `01a17f0e-495a-7df1-b590-8ef1bf5dc4d5`), to be sent only after Rob explicitly approves this separate XLM execution and destination:

> Rob has explicitly approved one separate native-XLM local test build for Mara: the test-only withdrawal at implementation commit b68748969d655e1c161e31bc3f388baaecff33e0, run once with runner /private/tmp/xlm-withdraw-b6874896-docdesc-runner.py (SHA-256 d78c964a06b72cede2dc52482eab5ed06f32c095281ff8ca47ee6cfc57b76fb4) and allocation <new allocation id>, record <allocation path> (SHA-256 <allocation hash>), start before <start-by UTC>. Scope: the three focused commands only (test build, xlm_confirm_pic, production check), 30 minutes including a 60-second closure reserve, 30 GiB free to start and 20 GiB while running, 8 GiB growth and memory, 2 jobs and 2 Binaryen cores. Stop at the first failure or unknown condition, no retry, close only owned processes. No install, deployment, signing, Stellar transaction, funds, paid API, credits or hosted CI. Report the terminal receipt; do not push before independent acceptance.

The angle-bracket fields are filled in by the scheduler when a new allocation is issued.
