# SEC10 XRP withdrawal release — November 7, 2026

Release owner: Dorian. Rob requested deployment of the XRP withdrawal fix in Ellis's case chat. Customer messaging and financial/canary actions remain held. SEC11 repeat deposits and address display remain a separate active source lane owned by Miles.

## Frozen source composition

The controller-authenticated live module is `d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`, bound by the November 5 installation report to source `6847982b44f247e0c99f0a453b583d3fab8107a2`. Identity `rumi_identity` resolves to controller `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` on backend `tfesu-vyaaa-aaaap-qrd7a-cai`, mainnet.

This release starts at that exact deployed source and applies only the three Rust-file patch from accepted SEC10 commit `c02c4bffffd10521362d3cae3047efdb4626cf42` (merged in PR628). The patch applies without conflicts: main.rs 33 additions/7 removals, vault.rs 254 additions, xrc.rs 27 additions/2 removals. Every other tracked build input remains equal to the deployed source. Current main `5edd1ff1e49d68dc77f50a109017ab6d834bc218` also includes about 28,000 lines of XRPL issuance work beyond live; those changes are outside this bounded release. No Candid, generated declarations, stable-state schema, dependency, manifest, upgrade hook or timer setup changes are introduced.

Exact zero-debt owned NativeXrp withdrawals may skip oracle work; any positive debt including dust retains freshness and CR admission. Partial withdrawal re-reads current owner, freeze, status and locks at mutation. Deposit minimum is not a withdrawal minimum. Native XRP cadence is capped at 300 seconds, preserving any shorter interval and other collateral behavior. Existing accrued-interest and conversion rounding semantics are preserved; this does not introduce a new ceiling-rounding policy.

## Essential verification and preservation

A fresh six-test SEC10 run and one canonical backend release build are the planned deterministic checks for the live-base port. The previous accepted patch tests/CI remain provenance, not fresh test proof on this composition. No broad suite, additional fault campaign or frontend build is included. Production Candid must match deployed Candid; artifact identity, Wasm size, current live module, settings and source composition must be checked before installation. Focused independent review covers the port and release procedure.

The accepted drained-upgrade runbook requires controller-authenticated Stopped before install. Dispatch stop, snapshot replacement, upgrade and start once each, persisting a local dispatch marker before invoking the CLI. Unknown outcomes halt for reconciliation; no automatic retry, running upgrade, uninstall or reinstall. Preserve all controllers/settings and mode; use mode=null upgrade arguments. Financial features, reserve and sponsorship settings remain unchanged. Retain private pre/post ckSOL receipt, XRP claim, nICP claim, pending payout and operation baselines. Readbacks establish retention, not customer withdrawal or settlement.

All ten snapshot slots are occupied. The current oldest entry is `00000000000000080000000001f088fe0101`, taken September 14, 2026 00:02:07 UTC. Its complete native archive has been downloaded read-only to the protected recovery store and named payload lengths/hashes verified. The intended preservation sequence replaces only that archived managed slot with a fresh Stopped-state snapshot, verifies all nine other entries unchanged, downloads and verifies the fresh snapshot before installing, and preserves both local archives. No standalone snapshot delete, restore or stale-state rollback is included. Exact replacement authority must be resolved against existing action holds before dispatch.

## Status

Prepared source composition; tests/build, artifact hashes, release review and live execution are pending. This document makes no deployed or customer-outcome claim. The shared target `/private/tmp/rumi-cksol-icp-target` belongs to this serial test/build slot; Miles may edit source independently. Heavy jobs use two Cargo workers.
