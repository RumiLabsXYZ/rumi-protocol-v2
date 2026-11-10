# Season 2 accounting implementation evidence

Owner: Orin. Status: tested source candidate; not installed or activated.

The campaign is continuous from November 1, 2026 00:00 UTC. Token allocation and
the campaign end date remain separate decisions. The finalized Season 1 instance
and its credits are preserved. This candidate uses a separate, freshly initialized
instance; it is not a reset or upgrade of the Season 1 canister.

## Implemented boundary

An admin-only, bounded import records historical category credits separately from
ordinary snapshot ledger entries, together with the participant state needed for
prospective accrual. Ordered batch hashes make an exact retry idempotent and reject
a changed retry. An additive stable cell retains import status and receipts across
upgrades. Existing stable layouts are unchanged.

Initialization names a future prospective cutoff T0, with no decided campaign end.
The import metadata names the logical November 1 start and the exclusive historical
interval ending at T0. Polling and epoch activation remain disabled during import.
Completion requires all expected participant rows, all four source bindings and
cursors, and no unresolved categories. Fresh epoch zero inherits the flat 4x fiat
stable pool policy. A manifest hash binds the actual batch transcript, total,
source canisters and cursors. It does not prove historical inputs or arithmetic.

The offline calculator emits provisional results only. Its event-time integral is
an explicitly disclosed model; it cannot recreate random A/B captures that never
occurred. Unknown categories remain null rather than silently becoming zero. No
calculator result authorizes or performs a production credit import.

## Functional evidence

- Existing points library suite: 194 tests passed before the final focused test
  additions; the four import tests and two binary tests subsequently passed.
- Canonical declarations regenerated with
  `bash scripts/regenerate-declarations.sh rumi_points`.
- Canonical `icp build rumi_points --environment local` passed after the manifest
  binding correction. Embedded Candid passed strict comparison against the service
  declaration with `didc check`.
- The existing PocketIC ingestion suite's
  `season2_import_upgrade_and_first_epoch` test passed using the built Wasm: a
  synthetic historical receipt survived an actual upgrade and the first
  prospective epoch added one ordinary ledger entry without duplicating that
  receipt. This is a disposable fixture, not live Season 2 accrual.
- The test measured management memory of 151,493,408 bytes after its first epoch.
  This is a one-row local fixture, not sustained production usage or deployed
  network cycle pricing.

## Built artifact

Canonical optimized artifact SHA-256:
`dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17`.
Module size: 3,018,333 bytes. Executable code section: 2,457,901 bytes.
Deterministic gzip SHA-256:
`967e22d162579f5626283b094bd0a1457f09c009f34ed455b676f252975def8c`;
747,354 bytes. These identify the candidate built after the manifest correction;
they are not deployed hashes.

## Remaining release actions

Finish the private numerical reconciliation and approve its explicitly stated
historical method. Capture the final T0 participant state and exact source cursors,
with sufficient time to import and start before the first required capture; a
missed capture requires replanning rather than claiming ordinary snapshot proof.
Use the distinct Season 2 selector and explicit initialization, then verify target,
controller, artifact and funding in the separate release packet.

Creation, funding, installation, imported awards and activation remain unexecuted.
No token funding, claim enablement or promotion is included. Private participant
snapshots and raw source archives are intentionally excluded from this document.
