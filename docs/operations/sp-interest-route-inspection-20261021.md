# Draft: inspect retained interest delivery routes

## Purpose

The developer-only `get_pending_treasury_interest_mint_recovery_page` query returns the exact retained mint tuple and phase, but omits the stored `TreasuryInterestMintRoute`. The query name covers several destination types. A pool-directed tuple does not reveal the source collateral required to reproduce source-specific depositor allocation.

This small patch adds a `route` field to `TreasuryInterestMintRecoveryRow` and clones it directly from the existing journal row during the same synchronous state read. It uses the existing paginated query, developer/anonymous caller checks, cursor semantics and page cap. The persisted journal already stores the route.

## Scope

- Source base: `cc224b3edf0f098779cb87d1c09bb245affd8c55`.
- Add the existing route enum to the response, including `Treasury`, `StabilityPool { collateral_type }` and `ThreePool`.
- Copy the retained enum exactly; do not infer it from the destination principal, current configuration, mint amount or another journal. Missing legacy state fields retain their existing decoder semantics.
- Keep stable-state definitions, minting, notification delivery, acknowledgment, accounting, timers and pause controls unchanged.

The read-only query is one proposed inspection path. Equivalent authenticated retained-state evidence could supply the same input. This patch alone does not settle delayed interest or prove the deployed pool's source. Receiver accounting, receipt reconciliation and the pool's existing restrictions remain separate work.

## Contract checks before release preparation

1. Type-check the modified Rust response and exact stored enum.
2. Generate the complete Candid interface from Rust through the repository's canonical export path; regenerate declaration artifacts with `scripts/regenerate-declarations.sh rumi_protocol_backend`.
3. Verify the new service remains a subtype of the prior interface. Existing clients may ignore the additional response field; this requires an actual compatibility check, not a claim based only on the source diff.
4. Review that caller checks and bounded cursor traversal are unchanged, every returned route is cloned from the same retained row, and no persisted or financial transition changed.
5. For a later authorized rollout, bind the exact release artifact, installed bytes, source and interface; obtain a fresh protected backup and current execution authority.
6. After any authorized rollout, preserve typed query evidence keyed by operation nonce, mint block and exact retained route; reconcile it with the ledger and receiver evidence before preparing any allocation.

## Current status

**Source preparation; no rollout authority.** The Rust response addition and generated Candid/JavaScript/TypeScript declarations are implemented. The following deterministic checks passed:

- Offline locked `cargo check` for the backend library on `wasm32-unknown-unknown`.
- Complete Rust Candid generation through a temporary native full-source exporter. The exporter invokes `candid::export_service!()` after the handlers; only its native entry point is changed. The original main source, Cargo manifest and lockfile are preserved exactly.
- `didc check` confirms new-service subtyping against the pinned prior interface.
- A strict structural comparison, projecting out only the generated route field/type, confirms the rest of the complete interface is unchanged. The projected file is verification evidence; the checked-in interface is the unmodified full Rust-generated output.
- Canonical declaration regeneration and `git diff --check`.

The first exporter placed the macro before the handlers and emitted an empty service. That output was rejected and preserved; it was never copied into the source interface. The corrected exporter produced the complete interface and passed the checks above.

Two independent source-preparation reviews passed: exact retained-route cloning, unchanged authentication/pagination/persisted state, full Rust export and matching declarations. This is not a release or live-accounting verdict. An exact release Wasm, release preflight and live rollout remain separate. No production upgrade, global resume, mint, transfer, journal deletion or credit is authorized by this document. No unit or PocketIC tests have been run for this preparation.
