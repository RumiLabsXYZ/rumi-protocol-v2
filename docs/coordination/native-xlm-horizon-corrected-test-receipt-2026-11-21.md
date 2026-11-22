# Corrected Horizon transport validation — November 21, 2026 Pacific

Outcome **PASS**, one fresh allocation consumed; no automatic retry or live action. Implementation `bdb548020b30c7c8d7b22e6d79173af40accc653`; clean docs-only runtime HEAD `abe253aa0c03265f87e3a383e50440c30c8c538d`.

Rob directly approved scope A of `b4bd427e62907453f5805fd56dfbe850cacc30e4` in Cody's chat. The new runner changed only BASE, the corrected Horizon input hash, fresh file/output/lock names and descriptive comments. AST comparison after reversing those literals matched the original; syntax and all six source pins passed without child execution during preparation. Original runner and consumed failed allocation `4b85c8c0` remain untouched. No broader replacement logic was created.

- Runner `/private/tmp/xlm-horizonsubmit-bdb54802-docdesc-runner.py`, SHA256 `0d397ffbef1f9f4c2032f2e88262dbd2d3b9d375b096d1db4f389f18eb0a81c5`.
- Allocation `4a6bc92c-0aca-4c82-af4c-0c353a6865eb`, packet SHA256 `37f7f054af6d645514430f94b0c3af007203071ecbe010fac239fdc19b6d81a2`.
- Command 1: `cargo test --locked --offline -p rumi_protocol_backend --lib -- chains::stellar::horizon_rpc::tests`, native **0**, exact **22 passed; 0 failed; 0 ignored; 0 measured; 1509 filtered out**.
- Command 2: `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown`, native **0**, feature off. Runner native **0**.
- Started 2026-11-22T06:41:50.786850Z, ended 06:42:16.472623Z, 25.723165625 seconds (November 21 Pacific).
- Receipt `/private/tmp/xlm-horizonsubmit-bdb54802-docdesc/receipt.json`, SHA256 `a4c4bfa40f09cc2f1d1858f7818a8c7bea3f33c2a28b9ae8d49944e2d3982f47`.
- Step 1 log SHA256 `620a077acb307212f8ee38ba16398861d4da0f99c54d49b7a1044d65c15de2e4`.
- Step 2 log SHA256 `e16c60fc03a16929faaf743e60044d85ba95b4df47e851dfa6d61e3fed5d6591`.
- Native process inventory independently found owned PGs **84961 and 85053 empty**; both log hashes independently matched receipt. Runner allOwnedClosed/sourceAfterClean true and actual Git status clean.
- Minimum writable free 90,845,216,768 bytes; maximum physical growth 258,056,192 bytes; peak owned RSS 1,616,855,040 bytes. Bounds respected: 30/20 GiB startup/runtime, 8 GiB growth/RSS, jobs 2, 1800 seconds including 60 closure. Rust/Cargo 1.92.0 matched pinned versions.

Evidence ceiling: locally tested dormant Horizon request builder, deterministic reducer and advisory parser, with the narrow fixture correction; feature-off compilation passes. No transform export, integrated payout driver, provider outcall, real signature, submission, testnet installation, customer payout or activation. Production XLM remains Frozen. Next is the already accepted settlement-driver handoff and mocked integration, with a fresh allocation required for any compiler batch. Retain the original failed run in `native-xlm-horizon-test-receipt-2026-11-21.md`.

Closeout: Cody independently verified receipt, exact test count, native exits, logs, resources and owned closure, and recorded allocation `CONSUMED_PASS_RELEASED`. Scoped Lookout dry-run/publish/final dry-run all exited 0: revision **75 → 76**, source revision `b8e2a4912eba2e58a9b2c80d5f1bc2d5cb0bad88fcb9a28074e4cbbe6e7b2cbb`; board, technical presentation, Plain English summary, priorities and repository association read back unchanged. No duplicate runtime or new work stream started.

Next accepted dependency is the existing WP4 integrated settlement-driver source in `native-xlm-settlement-driver-handoff.md`: reuse per-claim and per-custody guards, take freshness time after quorum reads, bind the post-sign record to the captured prepared snapshot, resume or hold an interrupted preparation, reconcile in-flight claims first, and keep POST responses advisory. Original Mara remains delivery/review owner; the existing implementation team remains the source owner. Rob's original continuous focused implementation/correction instruction remains the authority for routine source work. Any compiler needs a fresh allocation; live/provider/signer/broadcast/testnet/deployment/funds boundaries are unchanged. No new team or extra spending follows from this closeout.
