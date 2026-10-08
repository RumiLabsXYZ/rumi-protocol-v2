# Fiat stable points mainnet preflight

Date: 2026-10-07 (America/Los_Angeles)

This is a read-only preflight for the points release at commit
`4b4b1680a89b37ab82641ae1e1cb6addf98dcdb5` (`origin/main` resolves to the same
commit). No canister mutation, deployment, timer change, epoch tick, snapshot,
seed read, or credential export was performed.

## Mainnet mapping and authority

`icp.yaml` declares the `mainnet-live` environment on network `ic` and includes
both `rumi_points` and `vault_frontend`. The checked-in mapping is
`.icp/data/mappings/mainnet-live.ids.json`:

| Name | Mainnet canister |
| --- | --- |
| `rumi_points` | `bfnu3-6aaaa-aaaab-qhanq-cai` |
| `vault_frontend` | `tcfua-yaaaa-aaaap-qrd7q-cai` |

The named ICP identity `rumi_identity` exists and resolves to
`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`. The default
identity was left unchanged (`robvector`). `rumi_identity` is a controller of
`rumi_points`, and `get_points_config` reports that same principal as the
points admin. The admin-only `get_epoch_status_admin` query succeeds under
`rumi_identity` and traps as unauthorized under `robvector` and `anonymous`.

The read-only management status for `rumi_points` reports `Running`, module hash
`b092cffb6046b384ca0e827128d1b6fb500f27dbd41da94c6cff2d1aa045dc90`, and
controllers `cpbhu-5iaaa-aaaad-aalta-cai` plus the `rumi_identity` principal.
The frontend reports `Running`, module hash
`04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`, and its
environment variables include `PUBLIC_CANISTER_ID:rumi_points` with the mapped
points principal.

## Current public state

Read-only Candid calls against `bfnu3-6aaaa-aaaab-qhanq-cai` reported:

- current epoch index `18`, with an open epoch;
- the first snapshot is already exposed as fired and the second snapshot remains
  withheld by the public API; its exact time is intentionally omitted here;
- snapshot seed commitment present and 18 revealed seed records;
- epoch driver currently disabled;
- poller enabled at 300 seconds;
- 15 registered principals and 9 excluded principals;
- point ledger length `401`;
- source cursors for backend, 3pool, stability pool, and AMM of `512663`,
  `110`, `1870`, and `5` respectively;
- configured source canisters are `tfesu-vyaaa-aaaap-qrd7a-cai`,
  `fohh4-yyaaa-aaaap-qtkpa-cai`, `tmhzi-dqaaa-aaaap-qrd6q-cai`, and
  `ijlzs-2yaaa-aaaap-quaaq-cai`;
- configured asset ledgers are icUSD `t6bor-paaaa-aaaap-qrd5q-cai`, 3USD
  `fohh4-yyaaa-aaaap-qtkpa-cai`, ckUSDC `xevnm-gaaaa-aaaar-qafnq-cai`, ckUSDT
  `cngnf-vqaaa-aaaar-qag4q-cai`, and ICP `ryjl3-tyaaa-aaaaa-aaaba-cai`;
- cycle status is healthy, with balance `3,933,969,354,646` cycles, stable
  memory `117,506,048` bytes, and a `1,000,000,000,000` cycle low watermark.

Season-bound and snapshot timestamps, pending commitments, and revealed seed
bytes were queried only as needed for local diagnosis and are intentionally
absent from this public evidence.

## Deployed Candid versus current source

The deployed `candid:service` metadata was fetched to the private path
`/private/tmp/rumi-points-release-live-candid.did`. Its SHA-256 is
`05fa0809b6b84809081dbeaeb1e73558f60285d4fc02dbdc153f0850d2f6d6`, while the
checked-in current Candid is
`a935728e07fe7910340c2c0f720962547408ec0666dd29db1fc05b35fe7f82f9`.

The live interface is the pre-transition shape. It omits the current source's
`EpochStatus.legacy_transition_held`, `EpochStatus.legacy_reseed_pending`, and
`RevealedSeed.derivation_entropy` fields. The remaining Result aliases and
record/variant labels are structurally compatible despite different generated
alias numbering/order. This is live artifact drift that must be expected in the
upgrade review; it does not prove that the current Wasm has been built or
installed.

## Upgrade compatibility gate

The current source deliberately fences a legacy active epoch during
`post_upgrade`:

- `src/rumi_points/src/main.rs:48-57` restores stable state, calls
  `prepare_legacy_state_after_upgrade`, then re-registers timers.
- `src/rumi_points/src/epoch.rs:58-67` classifies an open epoch at index greater
  than zero with no stored entropy as requiring review.
- `src/rumi_points/src/epoch.rs:316-336` sets the durable legacy hold and clears
  the pending reseed marker before timers or update ingress resume.
- `src/rumi_points/src/epoch.rs:377-394` explicitly preserves an already-open
  legacy epoch and performs no capture or close.

The live Candid is pre-transition, and the live state is an open epoch 18 with a
pending second snapshot. The old state therefore has no `current_entropy` or
`secure_seed_chain_v1` marker for the new code to treat as a secure chain. The
current code will hold this epoch for review on upgrade; it will not silently
continue the remaining legacy snapshot schedule. The current live driver being
disabled does not remove the open-epoch compatibility issue.

This exact shape is covered by the ignored PocketIC regression
`src/rumi_points/tests/pocket_ic_ingest.rs:441-595`:
`legacy_points_upgrade_holds_epoch_18_between_snapshots`. The test models a
legacy epoch 18 after snapshot A, upgrades to the current Wasm, and asserts the
durable hold, unchanged epoch/open state, unchanged points, unchanged source
cursor, and no poll mutation across a second upgrade. It is source/test
evidence; the ignored test was not run in this preflight because its pinned
legacy and PocketIC Wasm inputs were not available here.

## Fiat policy boundary and operator path

The working-tree policy implementation schedules flat 4x only at the next epoch
boundary (`src/rumi_points/src/state.rs:895-923`) and uses a fixed immutable
ledger prefix for bounded historical topups (`src/rumi_points/src/state.rs:1029-1083`).
Closing the matching legacy epoch applies only the remaining rows after that
cutoff (`src/rumi_points/src/epoch.rs:700-716`). These paths do not shorten or
reseed the open epoch, and they are independent of the mandatory legacy seed
hold described above.

The working-tree Rust entry points now include `get_fiat_stable_points_policy`,
`activate_fiat_stable_4x`, and `apply_fiat_stable_topups`
(`src/rumi_points/src/main.rs:133-139`, `:207-220`), but the checked-in
`src/rumi_points/rumi_points.did` does not yet declare them. The deployed Candid
also predates these methods. Until the Candid/declaration artifact is regenerated
and the resulting Wasm is independently built and hashed, there is no verified
live operator ingress for the policy in this release candidate.

For the seed-security transition, the only automatic reseed path is for an
unopened legacy epoch that remains inside its safe window
(`src/rumi_points/src/epoch.rs:377-482`). An already-open legacy epoch is held
for explicit operator review; there is no source path that silently closes it,
rewrites its cursor, or resumes its publicly predictable remaining snapshots.

The local `icp deploy --help` command stalled both under the default sandbox and
with host escalation. The successful read-only calls used `icp 1.3.0`, cwd
`/private/tmp/rumi-points-release`, `rumi_identity` where authentication was
needed, and no custom ICP environment variables. A post-stall process/lock check
found no surviving `icp` process and no holder of the global settings lock;
unprivileged process listing itself was denied by the sandbox. This is a tooling
diagnostic only and is not deployment evidence.

## Operator runbook (prepared, not executed)

The expected post-upgrade state is: epoch 18 remains open and held for the
security transition, snapshot B remains incomplete, the epoch driver remains
disabled, and the poll configuration remains enabled while its write guard is
fenced. Cutover 19 is pending. This is a safe upgrade-into-hold outcome; it is
not uninterrupted legacy continuation, automatic recovery, or evidence that
flat 4x is already active. The deployment authorization does not authorize an
override of the PTS-002 review hold.

1. Build and independently hash the reviewed `rumi_points` Wasm. Create a
   management snapshot for the points canister only, immediately before the
   install. The exact CLI shape verified by the coordinator is:

   ```bash
   icp canister snapshot create rumi_points \
     --environment mainnet-live --identity rumi_identity
   ```

   Record the snapshot identifier privately. Do not snapshot or mutate another
   canister as part of this points release.

2. Install only the points artifact as an upgrade, using the explicit reviewed
   Wasm path. Do not use the name-based deploy pipeline for this existing live
   canister:

   ```bash
   icp canister install bfnu3-6aaaa-aaaab-qhanq-cai \
     --environment mainnet-live \
     --identity rumi_identity \
     --mode upgrade \
     --wasm "$POINTS_WASM" \
     --args '(null)'
   ```

   Before proceeding, verify the installed module hash, controller set, and
   deployed Candid. A hash mismatch, Candid mismatch, or failed admin query is
   a stop condition.

3. Query the policy anonymously and verify that it is still legacy for the
   current held epoch. The expected result is no current flat-4x activation;
   do not publish hidden timestamps or seed material:

   ```bash
   icp canister call rumi_points get_fiat_stable_points_policy '()' \
     --environment mainnet-live --identity anonymous --query
   ```

4. Build and publish regenerated frontend assets after the points upgrade checks pass, before activation or any new audit-source rows are emitted:

   ```bash
   npm run build --workspace vault_frontend
   icp sync vault_frontend --environment mainnet-live --identity rumi_identity
   ```

   This sync targets `vault_frontend` assets only; it does not install or upgrade
   its Wasm. The public points page already has a dynamic pending-policy path in
   `src/vault_frontend/src/routes/points/+page.svelte:111-142`, backed by
   `src/vault_frontend/src/lib/services/pointsService.ts:116-145` and
   `src/vault_frontend/src/lib/utils/fiatStablePointsPolicy.ts:104-145`.
   Verify it says the current epoch retains the legacy rule while epoch 19 is
   scheduled, and distinguishes scheduled, processing, completed historical-prefix and remaining legacy-epoch adjustments;
   it must not render flat 4x as current before the live policy query says so.

5. With the named admin identity, schedule the next-boundary policy and process
   historical corrections in bounded batches. These are separate authenticated
   updates; neither clears the epoch hold nor advances epoch 18:

   ```bash
   icp canister call rumi_points activate_fiat_stable_4x '()' \
     --environment mainnet-live --identity rumi_identity

   icp canister call rumi_points apply_fiat_stable_topups '(1000 : nat32)' \
     --environment mainnet-live --identity rumi_identity
   ```

   Repeat the topup call only while the returned durable progress advances, and
   stop when its `complete` field is true. Then verify the public policy reports
   `historical_complete=true`. Reconcile the returned cursor and completion
   markers after every batch. Missing principals, arithmetic overflow, a
   non-advancing cursor, or duplicate correction rows are stop conditions.

6. Re-read status as both anonymous and admin, then reconcile each registered
   principal by comparing `get_principal_state` totals with the sum of its
   paginated `get_principal_point_entries` rows, including separately identifiable
   historical adjustment rows. Also verify `get_point_ledger_len`, ingest status,
   and the configured source cursors. Keep future snapshot times and commitments
   out of public evidence.

Do not call `force_epoch_tick`, enable the epoch driver, disable or re-enable the
poller, clear the legacy hold, read seed bytes, or invent a legacy recovery
override. Recovery of the open epoch 18 is a deferred operator-review action;
it is not silently added as a prerequisite to the fiat-stable 4x policy or
historical migration.

## Release disposition

| Gate | Result |
| --- | --- |
| Mapping and target IDs | PASS |
| Named identity exists | PASS |
| Admin/controller authority | PASS for `rumi_identity` |
| Public points/ingest/cycle state | PASS, read-only evidence collected |
| Frontend mapping to points canister | PASS from live settings |
| Current artifact freshness/module match | UNKNOWN; no current Wasm build was performed |
| Deployed Candid parity | DRIFT: live interface predates the current transition fields |
| Upgrade continuation of active legacy epoch | BLOCKED: uninterrupted continuation is unsafe; upgrade into the durable security hold is the expected state |
| Fiat-stable cutover and historical migration | PENDING: may be scheduled/processed separately; current epoch remains legacy and cutover 19 is not active |

The concrete release sequence is the points-only snapshot, explicit upgrade,
anonymous/admin post-status checks, regenerated `vault_frontend` asset sync,
policy scheduling and bounded historical reconciliation. Old cached clients
may need to reload because their old Candid decoders lack the new source variants. The open epoch 18 recovery decision remains
with the named operator/review task and must not be inferred from deployment
authorization. This report provides no deploy or live-clearance claim.

For isolated checkout reconstruction, the source of truth is
`icp.yaml:635-642` for the `rumi_points` recipe (`@dfinity/rust@v3.2.0`, package
`rumi_points`, current Candid path, `shrink: true`) and
`icp.yaml:794-809` for the `mainnet-live` membership. Use the explicit artifact install command above; artifact freshness and module hash must be checked before that action.

## Refreshed live target during implementation

An independent authorized security release upgraded points during this task. Before any fiat release mutation, the coordinator refreshed status and verified the new points module hash is `5860109f5dde4bacc1e02480c563bd3f38ef7f6a07674029d32a475d944585a5`. Its pinned source is `f72d1459`; `git diff 4b4b1680 f72d1459 -- src/rumi_points src/declarations/rumi_points` is empty, so this release retains all that deployed points source. The earlier `b092...` hash is historical preflight evidence, not the current module.

The refreshed live admin status confirms epoch 18, A complete/B incomplete, `legacy_transition_held=true`, `legacy_reseed_pending=false`, and `driver_enabled=false`. Poll configuration remains enabled but fenced by the security hold. Backend ingest cursor advanced to 512671 before the hold; the other source cursors remained 110/1870/5. The ledger remains 401 rows. The new fiat-policy method is still absent. The fiat release will preserve this established held state, rather than introducing a new live pause.

The pinned pre-randomness Wasm `/private/tmp/rumi_points_pre_randomness_5a20.wasm` became available from the security verification, with exact SHA-256 `d3fc3919ab50d75f245c2ad9539fb5c83a8da38a25c22d36b8f97e82ba0a3fbc`. The fiat candidate must rerun the existing legacy epoch-18 upgrade regression against it.
