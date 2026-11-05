# ckSOL ICP-processing release decision

**Decision: source is merged; release is not ready for installation or reserve use.** Keep the processing reserve at zero/off. This record is a release-preparation decision, not authority to build, install, fund, activate, run a financial canary, or move funds.

## Source composition

- Current merged source is `main` at `93d9ece7a9a5f27bac63db207ab9a3e81ed02f63` (PR #610). The backend ICP escrow/accounting path and frontend ICP quote/consent flow are in this source. PR #620 is also merged at `0f40a0a874f51cc773b17c4c2c3e4ac94100e065` and is part of this base.
- PR #605 remains open at `45af74a433c169c08e52bc1ad2bda23e1df215a9`. Its two changed files, `LiquidityInterface.svelte` and `LiquidityInterface.depositPolicy.spec.ts`, match the corresponding files at current `main` byte-for-byte. The balance behavior is therefore already present in the merged source inspected here; no additional PR #605 source cherry-pick is indicated. PR #605's description reports a prior live asset publication. That report is not a current served-byte readback, so compare the live asset map before any future asset upload and preserve all current asset keys and encodings.
- PR #610's required hosted checks passed: frontend validation run `41956856184` and archived-receipt run `41956856170`. These are not a source-matched production build or a complete ckSOL backend lifecycle suite. The focused preflight fixture passed 35/35 after the rent/dust regression was fixed. The independent backend and frontend adversarial reviews passed. The checked-in source record reports focused backend and lifecycle scenario results, with limitations below.

## Callback and journal handling

The direct-running-upgrade callback trap is not fixed by the merged source. Local upgrade controls reproduced late callback/cleanup traps when an outstanding call crossed a running upgrade; the exact production `Arc counter overflow` cause remains unproven. Use the accepted stop/drain procedure in [`rumi-backend-drained-upgrade-runbook-20261101.md`](rumi-backend-drained-upgrade-runbook-20261101.md): submit stop once, rely only on controller-authenticated status `stopped` as drain proof, preserve request IDs and outcomes, and do not retry an uncertain management call. If stop cannot be confirmed, hold the upgrade; an authorized start may cancel a stop after reconciling its outcome. Do not force an upgrade, uninstall, or reinstall.

Before any later maintenance action, read back the operation and payment journals, ledger receipts, minter status, and unresolved holds for the exact canister and owners. A cycles callback/refund or settled ICP processing payment is not proof that the SOL deposit minted to ckSOL. Resolve uncertain outcomes against authoritative receipts before retrying. Preserve unknown SOL outcomes and their associated financial holds.

## Artifact inventory and next resource

- No backend `rumi_protocol_backend` Wasm or compressed Wasm matching `93d9ece7` is present in this checkout. The checked-in `src/vault_frontend/dist` has 62 files; `index.html` is timestamped `2026-11-01 11:29:21 -0800` with SHA-256 `19c644767c39948b39a017968a68a67bdfa652be616a307563eb84fbf826add3`. It predates the merged source and is not a release candidate.
- The exact backend build pipeline is configured in `icp.yaml`: `cargo build --release --target wasm32-unknown-unknown --package rumi_protocol_backend`, embed the Candid service metadata, `ic-wasm shrink --keep-name-section`, `ic-wasm optimize O1 --keep-name-section`, then `gzip -9 -n`. The frontend build is `npm run build --workspace=src/vault_frontend`; its postbuild runs `scripts/verify-frontend-auth-assets.mjs`.
- The required next resource is one allocated, disk-capable build/CI slot for these source-matched builds and artifact checks. The host currently has about 7% free disk (69 GiB). The required worktree cleanup inventory found 0 eligible worktrees; no build was started. After capacity is allocated, build both artifacts from the same pinned source, record raw and uploaded Wasm hashes, executable code size, frontend manifest/auth-asset checks, and confirm the frontend asset set against live `list` pagination before preparing an install packet.

## Remaining checks and action holds

Four opt-in PocketIC lifecycle scenarios are recorded as passing, including escrow conservation and upgrades during pending funding, minter, and refund calls. The pending-funding scenario still logs a late callback trap whose cause is unverified. The full mocked asynchronous lifecycle and an authorized operator-owned financial canary remain unperformed; source tests do not prove deployed callback behavior, payment settlement, mint completion, or production reserve safety.

No install, reserve funding, reserve enablement, or financial canary is authorized by this record. Keep reserve at zero/off. Any later stopped upgrade, reserve funding or enablement, and customer-money canary must remain separate, explicitly authorized actions with source-matched artifacts and current live readbacks. The accepted stop/drain mitigation does not enforce safety for another deploy path.
