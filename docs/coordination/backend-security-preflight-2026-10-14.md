# Backend security upgrade: exact snapshot preflight (2026-10-14 UTC)

This is a source and read-only live-state record, not authorization to stop,
snapshot, upgrade, raise the borrowing cap, retry Treasury/Stability Pool
installation, or clear any financial obligation.

## Source and candidate

- `origin/main` and this isolated worktree were both
  `4046155f591b732ecafb8ef00144abfcec6d0c1a` at the build.
- `CARGO_BUILD_JOBS=2 icp build rumi_protocol_backend --environment mainnet-live`
  succeeded. The gzip upload artifact at
  `.icp/cache/artifacts/rumi_protocol_backend` had SHA-256
  `85c4da2760e5394f2046467975bfa9ed6910de1afceb7a198141704c00865d9a`.
  Its decompressed Wasm had SHA-256
  `8b49fc1f8ee06cb9c34cda3d7f5a0706099aa2785937a0922eabef9504c14252`,
  size 12,805,933 bytes, and executable code section 12,153,827 bytes
  (429,085 below the 12 MiB network limit). `ic-wasm check-endpoints`
  matched the authored backend DID with only lifecycle/timer/main exports hidden;
  `didc check` accepted the authored DID against the saved 2026-10-13
  installed-service metadata. `CARGO_BUILD_JOBS=2 cargo test --locked -p
  rumi_protocol_backend --lib` passed 1,273 tests, with 0 failed and 1 ignored.
  The build worktree retained only an untracked `target` symlink to the shared
  Cargo cache. An exact-installed-predecessor PocketIC migration test was not
  rerun for this candidate; unit tests do not replace it.
- This artifact is source/build evidence only. It is not installed. The
  production predecessor was still hash
  `e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352`
  in a fresh controller-authorized management status read at 2026-10-14
  about 02:05 UTC.

## Live peers and mixed-version behavior

- Treasury `tlg74-oiaaa-aaaap-qrd6a-cai` still had hash
  `4163a25f7e1d5119768cf16d6c75b6c98925682b0a89e99ced5cfe5aa0107300`.
  The currently installed service lacks the new once-only fee and interest
  bookkeeping methods. The candidate backend preserves failed notifications
  in durable outboxes; it does not retry them through the legacy non-idempotent
  deposit method. A backend-only upgrade may keep capped borrowing available,
  but it does not prove Treasury bookkeeping or historical fees settled.
- Stability Pool `tmhzi-dqaaa-aaaap-qrd6q-cai` still had hash
  `54245b779feb30c4da7ac3f60e9cb62afb99f1accd2ea5affc7adda8a28e4754`.
  A fresh public pool-status query showed `emergency_paused = true` and 13
  depositors. A backend-only upgrade does not resume or fix pool operations.
- The observed live global icUSD mint cap remained 22,000 icUSD. Preserve
  this cap through the upgrade; increasing it is a separate risk decision.
  Do not treat a build or upgrade as proof of new borrowing capacity.

## Snapshot capacity and exact replacement identity

At 2026-10-14 about 02:05 UTC, controller-authorized `icp canister snapshot
list tfesu-vyaaa-aaaap-qrd7a-cai --network ic --identity rumi_identity --json`
returned exactly ten IDs. Each ID below is transcribed from that response:

| ID | Size (bytes) |
| --- | ---: |
| `00000000000000010000000001f088fe0101` | 88,055,633 |
| `00000000000000020000000001f088fe0101` | 88,195,248 |
| `00000000000000030000000001f088fe0101` | 88,383,591 |
| `00000000000000040000000001f088fe0101` | 88,413,621 |
| `00000000000000050000000001f088fe0101` | 88,413,621 |
| `00000000000000060000000001f088fe0101` | 88,353,475 |
| `00000000000000070000000001f088fe0101` | 88,432,622 |
| `00000000000000080000000001f088fe0101` | 87,908,265 |
| `000000000000000c0000000001f088fe0101` | 88,036,004 |
| `000000000000000f0000000001f088fe0101` | 88,617,450 |

The user previously approved replacing
`00000000000000000000000001f088fe0101`. **That ID is absent from the live
list.** This proposal names a different oldest snapshot,
`00000000000000010000000001f088fe0101`; the earlier approval does not cover
replacing it. The oldest snapshot was previously downloaded to a private,
mode-0700 local directory; its files are mode 0600. The host path is withheld
from this proposal because the downloaded snapshot contains complete
canister state.
Its metadata and component file sizes and SHA-256 values matched a recorded
manifest (manifest SHA-256
`a5887adca6b713cd4e769de5023af668991cebb1b8cc7f628ead975f3059a0ff`).
Recheck those bytes immediately before any replacement; this copy is a
historical backup, not a fresh snapshot of the currently running backend.
Keep both the historical and fresh downloads in access-restricted local storage;
do not upload or publish their contents. Retain them through the reviewed
upgrade and recovery window. Any eventual deletion needs a separate retention
decision and proof that the recovery obligation has ended.

## Proposed exact operation, only after approval

1. Immediately before stopping, re-read the backend's installed hash through
   controller-authorized management status and its financial state through
   authorized queries; relist all ten snapshot IDs, borrowing cap,
   debt and supply, pending borrow/fee and Treasury/SP outboxes, and in-flight
   financial operations. Stop the snapshot replacement if the installed
   predecessor, snapshot list, or obligations differ materially from the
   reviewed evidence. Exact-artifact migration and mixed-version checks are
   separate preconditions for a later *upgrade*, not for snapshot replacement
   alone; no accepted finding makes them a gate for the latter.
2. Recheck the downloaded oldest snapshot's file sizes and SHA-256 manifest;
   keep the local copy and nine other network snapshots intact. Confirm enough
   free disk for any needed additional evidence.
3. Under a separately reviewed release sequence, stop the backend and wait
   for confirmed `Stopped` status; `Stopping` can still finish in-flight
   callbacks and mutate financial state. Queries to a fully stopped canister
   cannot provide a fresh financial inventory. Treat the fresh snapshot as
   the post-drain state boundary; do not claim that the pre-stop inventory is
   an exact view of its bytes. With the backend stopped, use
   `icp canister snapshot create
   tfesu-vyaaa-aaaap-qrd7a-cai --replace
   00000000000000010000000001f088fe0101 --network ic --identity
   rumi_identity --json` **once**. The ICP CLI/API atomically replaces the
   old snapshot only after creating the new one. Capture the returned new ID.
4. Relist snapshots; require ten entries, the new ID present, the named old ID
   absent, and the other nine unchanged. Prepare a mode-0700 directory and
   restrictive umask before downloading the fresh snapshot; require mode-0600
   files. Hash its components and verify that its embedded Wasm matches the
   observed live predecessor. Record its timestamp and bytes as the final
   stopped-state recovery point, and record
   component hashes without publishing the state. If a
   source-matched offline decoder is available, compare the snapshot's
   financial state with the last pre-stop inventory; otherwise label the
   between-read callback delta unknown and compare after starting.
5. **Snapshot approval alone does not authorize installation.** Only if a
   separate backend-upgrade authorization and release review are complete may
   the exact approved candidate be installed immediately, **without starting
   the old backend between snapshot and install**. Start the backend only
   after the install result and management
   status are authoritative; then re-read hash, cap, debt, supply, operations,
   and outboxes and reconcile any difference with the stopped-state snapshot.
   If the upgrade is deferred or the snapshot step fails without changing the
   installed module, restart the old backend promptly, confirm `Running`, and
   perform the same financial readback. A later upgrade needs a new stop and
   fresh snapshot; the earlier snapshot cannot cover intervening operations.
   If install outcome or migration state is ambiguous, do not blindly restart
   or restore: establish the installed hash from management status while
   stopped. Establish financial state only through a source-matched offline
   snapshot decoder or a separately reviewed controlled start and readback.
   There is no validated backend restore runbook in this proposal; rollback
   readiness is a separate release artifact and cannot be inferred from the
   presence of a snapshot. Restoring a snapshot after the canister has run
   again could erase later state while external ledger transfers remain, so
   reconcile those operations before any rollback. Keep the borrowing cap and
   unresolved outbox rows; never infer a transfer outcome from canister status.

The Treasury and Stability Pool install approval collisions remain unresolved.
This proposal does not resubmit either install. OpenAI support case 17015292
is awaiting a specialist's authoritative outcome for the original Treasury
request. A hash still showing old Wasm does not prove that request is terminal.

Official snapshot semantics: <https://docs.internetcomputer.org/guides/canister-management/snapshots/>.
