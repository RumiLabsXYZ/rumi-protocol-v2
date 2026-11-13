# SEC19 corrected lifecycle tests — fresh allocation required

Run only the three existing-harness `sec19_` PocketIC tests at pushed source **7dd8cb29b878cec9d01f256457a631ff2b27f08a**. Runtime Rust, Cargo inputs, cycle-manager and flaky-ledger source are byte-identical to validated e2a0122a. Commit9e8a5f5a fixes only fixture funding;7dd8cb29 commits the four canonically generated interface files. Independent frontend edits remain unstaged and outside this batch.

## Retained results and concrete correction

Nine backend tests already passed. The [continuation stages](evidence/add-margin-pic-20261113/retained/steps.json) also passed canonical Candid generation, bindings, old-service subtype compatibility, declared/generated compatibility, release Wasm build, metadata, shrink and O1 optimization. The [artifact receipt](evidence/add-margin-pic-20261113/retained/test-artifacts.json) records executable code8,957,230bytes, below the preserved10,223,616-byte project guard. Retain these results and artifacts without rebuilding or rerunning them.

The [three failed cases](evidence/add-margin-pic-20261113/retained/pic-sec19.log) all stopped at shared setup line659 before any top-up scenario: the fixture approved tokens, paying10,000, then attempted to deposit its entire remaining5,000,000,000 balance while another10,000 transfer fee was required. Fixture minting is now `amount + 20_000`, covering both fees; assertions for exact debit, credit, dedup, held recovery, owner filtering and ACK survival remain unchanged. This does not establish that the scenarios pass; they must run again after fresh allocation.

Allocation6714e939 is consumed. Its [session](evidence/add-margin-pic-20261113/retained/validation-session.json) ended243.57s, test exit101/driver1/outer1, with expected PocketIC server children still present after the failed driver exited. The runner terminated its owned group and recorded allOwnedClosed; the [independent census](evidence/add-margin-pic-20261113/retained/independent-group-closure.json) confirms group84778 empty. Peak ownedRSS2,173,583,360bytes, filesystem free-space-loss687,063,040bytes, minimum free35,470,196,736bytes. All evidence remains retained. Never reuse that allocation or dispatch marker.

## Exact final batch

[Runner](evidence/add-margin-pic-20261113/run-once.py), [steps](evidence/add-margin-pic-20261113/steps.py), [manifest](evidence/add-margin-pic-20261113/manifest.json) and [tool pin](evidence/add-margin-pic-20261113/tool-pins.json) are prepared at runtime `/private/tmp/rumi-sec19-pic-20261113`; no dispatch or allocation yet.

Preflight exact HEAD/backend bytes and production-source equality to e2a0122a. Check the retained optimized backend SHA256 **2961d2d84008e43756c25c3361b59717676ea92d68eb82951648dc5a4703ea08** and ledger SHA256 **e7f4ebb3b53f5239a6922ad6df836cc09a4d20fa6efc7ef7ba049b8c61a0eaf2**. Independently extracted public Candid metadata matches the committed canonical file byte for byte, SHA256 **d8b0142e705e860640557b7ea6253a2a6027d7e96f3b15cfee9ba51fff0d2c97**. Use those existing artifacts directly; no artifact export or rebuild.

One command: offline `cargo test -p rumi_protocol_backend --test vault_collateral_pull_pic sec19_ -- --test-threads=1`, with the exact pinned server7.0.0, matched artifact environment and existing warm target cache. Require exactly three passed/zero failed, then unchanged backend inputs. No completed-unit/interface rerun, new harness, broad suite or dependency installation.

One fresh exclusive Cody allocation,15minutes total including60seconds owned closure; one Cargo job/Binaryen core,8GiB aggregate ownedRSS and filesystem free-space-loss guard,20GiB free floor. Preserve all prior evidence, binaries, cache and release artifacts. No cleanup or bound increase.

The copied runner now explicitly handles the existing harness's localhost server lifetime: after a successful native driver, only remaining executables matching the exact pinned PocketIC path in the owned process group may receive bounded SIGTERM/SIGKILL cleanup without being classified as an unexpected child. Record that cleanup and independently prove the group and outer runner empty. Any other survivor, native/test failure, uncertainty or resource limit still stops the batch without automatic retry. This is owned closure under the existing reserve, not a new test harness or acceptance relaxation.

No production release, reserve, snapshot, controller, permission, signer, approval, transfer, financial action or customer-state mutation is included. Production collateral deposits remain paused. After successful validation and integration review, prepare a separate exact release proposal.
