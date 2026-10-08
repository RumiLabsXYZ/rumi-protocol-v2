# Rumi backend snapshot preflight

**Status:** source-only operating plan for future review. This document does not authorize a canister stop, snapshot, download, restore, install, deployment, or funds action.

## Target and current evidence

The production backend is `rumi_protocol_backend`, principal `tfesu-vyaaa-aaaap-qrd7a-cai`, from [`canister_ids.json`](../../canister_ids.json) (`ic` mapping). A public-state-tree status read on 2026-10-08 returned module hash `1714712f12525a5058c288bde8f456b09e2e893e4ac51fe7a82992ac07b0ecf1` and these controllers:

- `cpbhu-5iaaa-aaaad-aalta-cai`
- `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`
- `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`

The query was `icp canister status --public tfesu-vyaaa-aaaap-qrd7a-cai --network ic`. A controller-authorized snapshot-list read using the `rumi_identity` identity reported that all 10 of 10 network snapshot slots were occupied on 2026-10-08. Individual snapshot IDs and contents are omitted from this public plan. A controller-only status query was rejected for the other active caller. Therefore running state, cycle balance, memory usage, and controller-authorized settings have **not** been verified here. The listed hash, controllers, and snapshot capacity are point-in-time observations; refresh them immediately before any future operation.

## Preconditions for a separately approved run

1. Use the intended controller identity, `rumi_identity`, and confirm its principal is in the freshly read controller set. Stop if authorization or target identity is unclear.
2. Read and save the complete controller-authorized status and settings, current module hash, running state, memory and cycle figures, and snapshot list. Check the current count, snapshot storage cost, and enough local encrypted disk for the full download. The reported inventory is already at the network limit of 10 snapshots per canister, so a plain `snapshot create` cannot be assumed to succeed.
3. If a new snapshot is needed while the inventory is full, identify the oldest candidate from the fresh list. Before the maintenance window, download that existing snapshot to an encrypted, access-restricted location and validate its metadata, sizes, and file/chunk hashes. Preserve the complete verified copy and its manifest. For this historical snapshot, validate its own integrity; do not require its installed-module hash to equal today's live module hash. The live-hash equality check applies to the new pre-upgrade snapshot. That older snapshot is historical evidence and does not substitute for the new pre-upgrade snapshot.
4. Replacing an existing network snapshot requires a separate explicit approval naming the exact snapshot ID to replace. Only after the local copy has been verified and that approval is recorded may the operator use the CLI's `--replace <approved-snapshot-id>` option. Select the oldest candidate only if the separate approval names it. Never delete a snapshot to make room; `--replace` atomically keeps the old network snapshot until the new snapshot has been created successfully.
5. Record the expected pre-upgrade live module hash and reviewed upgrade artifact hash in the operation record. Compare the fresh live hash with the expected value. A mismatch pauses the run for investigation.
6. Coordinate the maintenance window with the separate pending Stability Pool snapshot owner. Keep the two canisters' inventories, approvals, snapshot IDs, and start/stop confirmations separate. Do not assume approval or completion of one authorizes or completes the other; avoid overlapping stops unless a coordinated window explicitly calls for it.
7. Announce the planned backend interruption to the operators responsible for its callers. Confirm no dependent operation is in flight and that the interruption is acceptable. The backend will reject or delay service while stopped.

## Controlled stop, snapshot, and restart

Use the explicit principal and network so project name resolution cannot select another environment. The commands below are for a future, separately authorized operator; they were not run for this plan.

Because the inventory is full, first preserve the proposed oldest candidate outside the repository. This download is read-only and does not stop the canister. Confirm the destination is encrypted and has enough free space before running it. Do not choose the final replacement ID until separate approval names that exact ID.

```sh
umask 077
BACKEND=tfesu-vyaaa-aaaap-qrd7a-cai
IDENTITY=rumi_identity
OLD_SNAPSHOT_ID='candidate-id-from-fresh-authorized-list'
PRESERVE_DIR="/secure/encrypted/location/rumi-backend-preserved-$OLD_SNAPSHOT_ID"
mkdir -m 700 "$PRESERVE_DIR"
icp canister snapshot download "$BACKEND" "$OLD_SNAPSHOT_ID" \
  --output "$PRESERVE_DIR" --network ic --identity "$IDENTITY"
```

Validate the candidate's metadata, file sizes, and file/chunk hashes using its own metadata and record a SHA-256 manifest in the restricted operation record. Keep all snapshot bytes private. A historical candidate may correctly have a different Wasm hash from the current live canister.

```sh
BACKEND=tfesu-vyaaa-aaaap-qrd7a-cai
IDENTITY=rumi_identity
APPROVED_REPLACE_ID='approved-oldest-snapshot-id'

# Read-only pre-stop record, using an authorized controller identity.
icp canister status "$BACKEND" --network ic --identity "$IDENTITY" --json
icp canister snapshot list "$BACKEND" --network ic --identity "$IDENTITY"

# Stop and wait until status explicitly reports Stopped.
icp canister stop "$BACKEND" --network ic --identity "$IDENTITY"
icp canister status "$BACKEND" --network ic --identity "$IDENTITY" --json

# Only after Stopped is confirmed. Because capacity is currently full, use
# --replace only for the exact ID separately approved after its local copy
# has been verified. Otherwise stop here; do not delete or replace anything.
icp canister snapshot create "$BACKEND" --replace "$APPROVED_REPLACE_ID" \
  --network ic --identity "$IDENTITY"
icp canister snapshot list "$BACKEND" --network ic --identity "$IDENTITY"

# Restart immediately after successful creation and confirm Running.
icp canister start "$BACKEND" --network ic --identity "$IDENTITY"
icp canister status "$BACKEND" --network ic --identity "$IDENTITY" --json
```

If stop fails or the canister is not confirmed `Stopped`, do not create or replace a snapshot. If the approved snapshot ID, its local preservation, or the explicit replacement approval is missing, stop before the maintenance window. If replacement returns an error or ambiguous result, do not blindly retry: inspect the snapshot list first, preserve any returned/created ID, and verify whether the old ID remains. After any create/replace failure or ambiguity, make starting the backend the next action, then confirm `Running`; if start fails, keep the upgrade on hold and escalate to the authorized operator until the canister is running. Never leave the canister stopped while investigating snapshot metadata or local download problems.

## Download and validate

Download only after the canister has been restarted and `Running` is confirmed. Use an encrypted, access-restricted destination outside the repository; a snapshot contains executable code and the complete heap/stable state. Do not add it to Git, logs, tickets, or shared artifacts. Set a restrictive umask and use a new empty directory on a volume with enough verified free space.

```sh
umask 077
set -o pipefail
SNAPSHOT_ID='paste-id-from-the-created-snapshot-record'
SNAPSHOT_DIR="/secure/encrypted/location/rumi-backend-$SNAPSHOT_ID"
IDENTITY=rumi_identity
mkdir -m 700 "$SNAPSHOT_DIR"
icp canister snapshot download tfesu-vyaaa-aaaap-qrd7a-cai "$SNAPSHOT_ID" \
  --output "$SNAPSHOT_DIR" --network ic --identity "$IDENTITY"

# Record the transferred-file digest, then identify its format. The IC
# module_hash is over the installed raw Wasm bytes, not a gzip container.
shasum -a 256 "$SNAPSHOT_DIR/wasm_module.bin"
MODULE_MAGIC="$(od -An -tx1 -N3 "$SNAPSHOT_DIR/wasm_module.bin" | tr -d ' \n')"
if [ "$MODULE_MAGIC" = "1f8b08" ]; then
  gzip -t "$SNAPSHOT_DIR/wasm_module.bin" || exit 1
  gzip -dc "$SNAPSHOT_DIR/wasm_module.bin" | shasum -a 256
elif [ "$MODULE_MAGIC" = "006173" ]; then
  shasum -a 256 "$SNAPSHOT_DIR/wasm_module.bin"
else
  echo "Unrecognized Wasm module encoding; stop validation."
  exit 1
fi
```

Confirm the download completed and inspect `metadata.json` for the expected snapshot ID/timestamp and the recorded Wasm, heap, stable-memory, and chunk-store sizes and hashes. Validate all downloaded files/chunks against snapshot metadata. Record the transferred `wasm_module.bin` SHA-256 for file-integrity evidence. Determine the file format from its magic bytes: if gzip-compressed, verify decompression succeeds and compare the SHA-256 of the decompressed raw Wasm bytes with the fresh live `module_hash`; if raw Wasm, compare its file SHA-256 directly. The compressed-file digest is not interchangeable with the IC `module_hash`. The fresh pre-upgrade snapshot passes only if its installed-module hash equals the live hash recorded immediately before stopping (the 2026-10-08 observation was `1714712f12525a5058c288bde8f456b09e2e893e4ac51fe7a82992ac07b0ecf1`; use the fresh value at execution time). Any mismatch, missing file, unexplained size, or failed hash check means the backup is unverified and the upgrade must remain on hold. If interrupted, use the CLI's `--resume` option against the same directory, then repeat all validation. Retain the snapshot ID, metadata, and local SHA-256 manifest in the restricted operation record, not the snapshot bytes.

## Recovery boundaries and hazards

A network snapshot captures the module and state at one point, including Wasm heap/stable memory, certified data, and chunk store. Restoring requires another stop and replaces the current module and state; updates made after the snapshot are discarded. Backend state can describe work that also affected external ledgers, chains, or other canisters. Restoring older backend state does not reverse those external effects and can create divergence or duplicate-processing risk. Treat restore as a separate recovery decision with a fresh impact review and explicit authorization. This preflight authorizes no restore, upgrade, snapshot deletion/replacement, canister setting change, or cross-canister action.

The snapshot workflow itself performs no token transfer, mint, burn, withdrawal, signing, broadcast, settlement, or funds movement. Do not add any such operation to this runbook.

## References

- [ICP canister snapshots guide](https://docs.internetcomputer.org/guides/canister-management/snapshots/) — stopped-state requirement, CLI sequence, download files, resume behavior, restore semantics, snapshot limit and storage.
- [ICP management canister snapshot reference](https://docs.internetcomputer.org/references/management-canister/) — snapshot methods and controller requirements.
- [ICP interface specification](https://docs.internetcomputer.org/references/ic-interface-spec/) — certified `module_hash` is the SHA-256 hash of the currently installed module.
- [Canister module format](https://docs.internetcomputer.org/references/ic-interface-spec/canister-interface/) — gzip-compressed Wasm is decompressed by the system before parsing as a Wasm module.
- Repository production mapping: [`canister_ids.json`](../../canister_ids.json).
