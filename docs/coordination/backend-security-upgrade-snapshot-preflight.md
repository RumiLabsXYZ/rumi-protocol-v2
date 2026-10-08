# Rumi backend snapshot preflight

**Status:** source-only operating plan for future review. This document does not authorize a canister stop, snapshot, download, restore, install, deployment, or funds action.

## Target and current evidence

The production backend is `rumi_protocol_backend`, principal `tfesu-vyaaa-aaaap-qrd7a-cai`, from [`canister_ids.json`](../../canister_ids.json) (`ic` mapping). A public-state-tree status read on 2026-10-08 returned module hash `1714712f12525a5058c288bde8f456b09e2e893e4ac51fe7a82992ac07b0ecf1` and these controllers:

- `cpbhu-5iaaa-aaaad-aalta-cai`
- `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`
- `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`

The query was `icp canister status --public tfesu-vyaaa-aaaap-qrd7a-cai --network ic`. A controller-only status query was rejected for the active caller. Therefore running state, cycle balance, memory usage, snapshot count/size, and controller-authorized settings have **not** been verified here. The listed hash and controllers are a point-in-time public observation; the authorized operator must refresh them immediately before any future operation.

## Preconditions for a separately approved run

1. Name the controller-authorized operator and use the intended production identity. Confirm its principal is in the freshly read controller set; stop if authorization or target identity is unclear.
2. Read and save the complete controller-authorized status and settings, current module hash, running state, memory and cycle figures, and snapshot list. Check snapshot capacity (the network permits at most 10 per canister), existing snapshot IDs, snapshot storage cost, and enough local encrypted disk for the full snapshot. Do not replace or delete an existing snapshot to make room without a separate decision.
3. Record the expected pre-upgrade module hash and the reviewed upgrade artifact hash in the operation record. Compare the fresh live hash with the expected value. A mismatch pauses the run for investigation.
4. Coordinate the maintenance window with the separate pending Stability Pool snapshot owner. Keep the two canisters' inventories, approvals, snapshot IDs, and start/stop confirmations separate. Do not assume approval or completion of one authorizes or completes the other; avoid overlapping stops unless a coordinated window explicitly calls for it.
5. Announce the planned backend interruption to the operators responsible for its callers. Confirm no dependent operation is in flight and that the interruption is acceptable. The backend will reject or delay service while stopped.

## Controlled stop, snapshot, and restart

Use the explicit principal and network so project name resolution cannot select another environment. The commands below are for a future, separately authorized operator; they were not run for this plan.

```sh
BACKEND=tfesu-vyaaa-aaaap-qrd7a-cai

# Read-only pre-stop record, using an authorized controller identity.
icp canister status "$BACKEND" --network ic --json
icp canister snapshot list "$BACKEND" --network ic

# Stop and wait until status explicitly reports Stopped.
icp canister stop "$BACKEND" --network ic
icp canister status "$BACKEND" --network ic --json

# Only after Stopped is confirmed, create a new snapshot. Save the returned ID.
icp canister snapshot create "$BACKEND" --network ic
icp canister snapshot list "$BACKEND" --network ic

# Restart immediately after successful creation and confirm Running.
icp canister start "$BACKEND" --network ic
icp canister status "$BACKEND" --network ic --json
```

If stop fails or the canister is not confirmed `Stopped`, do not create a snapshot. If creation returns an error or ambiguous result, do not blindly retry: inspect the snapshot list first and preserve any returned/created ID. After any creation failure or ambiguity, make starting the backend the next action, then confirm `Running`; if start fails, keep the upgrade on hold and escalate to the authorized operator until the canister is running. Never leave the canister stopped while investigating snapshot metadata or local download problems.

## Download and validate

Download only after the canister has been restarted and `Running` is confirmed. Use an encrypted, access-restricted destination outside the repository; a snapshot contains executable code and the complete heap/stable state. Do not add it to Git, logs, tickets, or shared artifacts. Set a restrictive umask and use a new empty directory on a volume with enough verified free space.

```sh
umask 077
SNAPSHOT_ID='paste-id-from-the-created-snapshot-record'
SNAPSHOT_DIR="/secure/encrypted/location/rumi-backend-$SNAPSHOT_ID"
mkdir -m 700 "$SNAPSHOT_DIR"
icp canister snapshot download tfesu-vyaaa-aaaap-qrd7a-cai "$SNAPSHOT_ID" \
  --output "$SNAPSHOT_DIR" --network ic

# Record a local file digest without printing snapshot contents.
shasum -a 256 "$SNAPSHOT_DIR/wasm_module.bin"
```

Confirm the download completed and inspect `metadata.json` for the expected snapshot ID/timestamp and the recorded Wasm, heap, stable-memory, and chunk-store sizes and hashes. Check that the downloaded `wasm_module.bin` SHA-256 equals the fresh pre-stop live module hash recorded above (the 2026-10-08 observation was `1714712f12525a5058c288bde8f456b09e2e893e4ac51fe7a82992ac07b0ecf1`; use the fresh value at execution time). Confirm all files/chunks match metadata and the expected sizes. If interrupted, use the CLI's `--resume` option against the same directory, then repeat all validation. A mismatch, missing file, unexplained size, or failed hash check means the backup is unverified and the upgrade must remain on hold. Retain the snapshot ID, metadata, and local SHA-256 manifest in the restricted operation record, not the snapshot bytes.

## Recovery boundaries and hazards

A network snapshot captures the module and state at one point, including Wasm heap/stable memory, certified data, and chunk store. Restoring requires another stop and replaces the current module and state; updates made after the snapshot are discarded. Backend state can describe work that also affected external ledgers, chains, or other canisters. Restoring older backend state does not reverse those external effects and can create divergence or duplicate-processing risk. Treat restore as a separate recovery decision with a fresh impact review and explicit authorization. This preflight authorizes no restore, upgrade, snapshot deletion/replacement, canister setting change, or cross-canister action.

The snapshot workflow itself performs no token transfer, mint, burn, withdrawal, signing, broadcast, settlement, or funds movement. Do not add any such operation to this runbook.

## References

- [ICP canister snapshots guide](https://docs.internetcomputer.org/guides/canister-management/snapshots/) — stopped-state requirement, CLI sequence, download files, resume behavior, restore semantics, snapshot limit and storage.
- [ICP management canister snapshot reference](https://docs.internetcomputer.org/references/management-canister/) — snapshot methods and controller requirements.
- Repository production mapping: [`canister_ids.json`](../../canister_ids.json).
