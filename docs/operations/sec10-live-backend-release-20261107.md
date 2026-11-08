# SEC10 XRP withdrawal release — November 7, 2026

Release owner: Dorian. Rob requested deployment of the XRP withdrawal fix in Ellis's case chat. Customer messaging and financial/canary actions remain held. SEC11 repeat deposits and address display remain a separate active source lane owned by Miles.

## Frozen source composition

The controller-authenticated live module is `d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`, bound by the November 5 installation report to source `6847982b44f247e0c99f0a453b583d3fab8107a2`. Identity `rumi_identity` resolves to controller `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` on backend `tfesu-vyaaa-aaaap-qrd7a-cai`, mainnet.

This release starts at that exact deployed source and applies only the three Rust-file patch from accepted SEC10 commit `c02c4bffffd10521362d3cae3047efdb4626cf42` (merged in PR628). The production patch applies without conflicts; the test-only XRPL issuance exclusion assertion is omitted because that state does not exist on the live base: main.rs 33 additions/7 removals, vault.rs 251 additions, xrc.rs 27 additions/2 removals. Every other tracked build input remains equal to the deployed source. Current main `5edd1ff1e49d68dc77f50a109017ab6d834bc218` also includes about 28,000 lines of XRPL issuance work beyond live; those changes are outside this bounded release. No Candid, generated declarations, stable-state schema, dependency, manifest, upgrade hook or timer setup changes are introduced.

Exact zero-debt owned NativeXrp withdrawals may skip oracle work; any positive debt including dust retains freshness and CR admission. Partial withdrawal re-reads current owner, freeze, status and locks at mutation. Deposit minimum is not a withdrawal minimum. Native XRP cadence is capped at 300 seconds, preserving any shorter interval and other collateral behavior. Existing accrued-interest and conversion rounding semantics are preserved; this does not introduce a new ceiling-rounding policy.

## Essential verification and preservation

A fresh six-test SEC10 run and one canonical backend release build are the planned deterministic checks for the live-base port. The previous accepted patch tests/CI remain provenance, not fresh test proof on this composition. No broad suite, additional fault campaign or frontend build is included. Production Candid must match deployed Candid; artifact identity, Wasm size, current live module, settings and source composition must be checked before installation. Focused independent review covers the port and release procedure.

The accepted drained-upgrade runbook requires controller-authenticated Stopped before install. Dispatch stop, snapshot replacement, upgrade and start once each, persisting a local dispatch marker before invoking the CLI. Unknown outcomes halt for reconciliation; no automatic retry, running upgrade, uninstall or reinstall. Preserve all controllers/settings and mode; use mode=null upgrade arguments. Financial features, reserve and sponsorship settings remain unchanged. Retain private pre/post ckSOL receipt, XRP claim, nICP claim, pending payout and operation baselines. Readbacks establish retention, not customer withdrawal or settlement.

All ten snapshot slots are occupied. The current oldest entry is `00000000000000080000000001f088fe0101`, taken September 14, 2026 00:02:07 UTC. Its complete native archive has been downloaded read-only to the protected recovery store and named payload lengths/hashes verified. The intended preservation sequence replaces only that archived managed slot with a fresh Stopped-state snapshot, verifies all nine other entries unchanged, downloads and verifies the fresh snapshot before installing, and preserves both local archives. No standalone snapshot delete, restore or stale-state rollback is included. Exact replacement authority must be resolved against existing action holds before dispatch.

## Status

Prepared source composition; tests/build, artifact hashes, release review and live execution are pending. This document makes no deployed or customer-outcome claim. The shared target `/private/tmp/rumi-cksol-icp-target` belongs to this serial test/build slot; Miles may edit source independently. Heavy jobs use two Cargo workers.

## Live-base test adaptation

The initial focused compile stopped at E0609: the accepted patch test referenced `State.xrpl_icusd.vault_exclusions`, a field belonging to the separate issuance feature absent from the live base. Removed only that assertion and its preceding fixture reset (three test-only lines); production behavior is unchanged. The same six focused test functions are being rerun with the now-warm dependency cache.

## Frozen production artifact and completed preparation

All six live-base SEC10 tests pass after the documented test-only adaptation. One serial production build completed in 6m48s using two Cargo workers and timestamp-refreshed path inputs with unchanged hashes. Canonical metadata, shrink, O1 optimization and deterministic `gzip -9 -n` packaging completed. No dependencies or source inputs changed during build.

Source: `bd253ddf1ebd233347452603e1e63c6d924820b1`. Raw Wasm 13,867,792 bytes, SHA256 `22a6837e8e7fa1def7bb0e15df89fc4a19b51f884eeefbc5eb63075a9ec3d465`. Optimized Wasm 9,616,603 bytes, SHA256 `acf0afc8fb31d8afd306b39aa658db09f698e4b11803af4fd2dd68305ba3222e`. Upload gzip 2,322,664 bytes, SHA256 `a7f9af49f6e5139730e025f6be0533ffb0d7afa6d5eaaec7d02a607b998a6834`. Executable code section 8,695,731 bytes, below the unchanged project guard 10,223,616 bytes (the project guard is distinct from the current network limit).

The embedded Candid passes strict structural equality with live. All 392 declared methods have matching Wasm exports, with no missing or unexpected application exports; the only extra is the existing CDK internal timer executor, also present in the verified deployed artifact. Direct candid-extractor is unsupported because production intentionally omits the test-only export_service pointer; no changed method signatures/types or new extractor harness are involved.

Ellis and Merritt returned independent bounded source/composition/preservation review PASS. The six focused tests, production build, metadata/export checks, hashes, and archive verification are the release evidence; earlier PR628 CI remains original-source provenance. No new broad suite or hosted CI was run. Source and binary readiness remain separate from the actual stop/drain/backup/install/restart and retained-state readback.

The normal scoped deployment authorization includes replacement of only fully archived snapshot8 with the fresh stopped-state backup while preserving the other nine snapshots. The exact once-only execution procedure and immutable pins are retained under [the evidence directory](evidence/sec10-live-release-20261107/). Every management write records its exclusive dispatch marker before invocation; any uncertain outcome halts for reconciliation. No standalone delete, restore or financial action is included.
