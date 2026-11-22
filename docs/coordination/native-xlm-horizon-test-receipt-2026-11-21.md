# Horizon transport focused check — November 21, 2026 Pacific

Outcome: **COMMAND_FAILURE**, one consumed allocation, no retry. Implementation `843e4e1c6f6caf2854a47c59cf96c3150743528b`; clean docs-only runtime HEAD `4bb7c2a2256aedba7104209ae4b001a0bf25b1f6`. Original byte-identical runner SHA256 `67e9d11c7426064c4f2309dd99ca08b55c2e8d747aa717a3534718615043093b`.

Allocation `4b85c8c0-e93e-4ffb-b744-ec8bf1554110`, packet `/private/tmp/dv-xlm-horizon-approved-once-20261122/allocation-4b85c8c0-e93e-4ffb-b744-ec8bf1554110.json`, SHA256 `6eca3b178010c2a1de6bba0393ed14b6fcac258fa5bde26b4bf79656b00f73d1`. Runner started 2026-11-22T06:09:28.685114Z, ended 06:10:22.097508Z (53.485581 seconds). This is November 21 Pacific time.

- Command 1: `cargo test --locked --offline -p rumi_protocol_backend --lib -- chains::stellar::horizon_rpc::tests`; native exit **101**, summary **21 passed; 1 failed; 0 ignored; 0 measured; 1509 filtered out**.
- Command 2: feature-off wasm32 production check **not run** under first-failure stop.
- Runner native exit **2**. Receipt `/private/tmp/xlm-horizonsubmit-843e4e1c-docdesc/receipt.json`, SHA256 `f8f12c7181efc6f9b8ae082bcf1df0a128cba55cc5d240e5ef71d5e1f9a43f74`.
- Step 1 log SHA256 `3059c411e94e250383188c31b335a7e26f7846a07519af22bfc2a232b86965ac`.
- Owned PID/PG 73682 closed in receipt and independently absent from native `ps`; source clean after batch. Minimum writable free 91,856,547,840 bytes; physical growth 659,935,232 bytes; peak owned RSS 2,582,593,536 bytes. Bounds respected. Rust/Cargo 1.92.0 verified by runner.

Failure: `submit_request_is_exact_post_form` unwrapped `None` at `horizon_rpc.rs:1159` only for the trailing-slash base. The existing `is_valid_base_path` contract explicitly requires fixed paths without trailing slashes (lines 189–205). All preceding exact POST/body/base64/transform assertions in that test completed. Correction `bdb54802` changes only the test expectation to rejection, documenting the canonical path rule; no production URL validation, endpoint, state, Candid or generated interface changes. `git diff --check` passed. The correction has not been Cargo-tested. Fresh source-bound review/allocation is required; the original runner/input pin cannot be replayed against modified source.

Rob's Cody–Mara messaging approval remains valid. The separate rejected instruction to create replacement runner logic remains stopped; exact-original recovery did not change runner bytes. This receipt does not authorize replacement-runner creation, runtime retries, signing, broadcast, provider outcalls, installation, deployment or funds. Production XLM stays Frozen; no customer payout or testnet readiness is proved.

Status closeout: existing scoped Lookout publisher dry-run, publish and final dry-run exited 0. Revision **74 → 75**, source revision `422b57213520bc803541fa3853a9b604f765c4d096159858c4d8e359d050f2a4`; board, presentation, Plain English summary, priorities and repository association all read back unchanged. Cody independently verified the terminal receipt/log, native failure, resource bounds and empty owned group, and recorded allocation `CONSUMED_FAILED_RELEASED`. No retest was admitted.
