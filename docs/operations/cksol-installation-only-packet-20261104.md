# ckSOL integrated-source installation-only packet — November 4, 2026

**For Merritt review and the canonical approval question; no execution is authorized by this packet.** This supersedes the obsolete 93d9 artifact in the earlier upgrade packet. Both production artifacts now come from integrated PR #624 source. Backend installation, downtime/backup operations, frontend publication, reserve funding/activation, canaries and financial actions have not occurred.

## Source and build provenance

Pinned source: PR #624 merge **`6847982b44f247e0c99f0a453b583d3fab8107a2`**, tree `093eb8506a1532f8568348721c7f06614c65719c`. Build checkout HEAD was documentation-only `73d88a684af60ce7786284f12b8754af7674967b`; comparison against the merge found no delta under `src`, manifests, lockfiles, `icp.yaml` or scripts. Thus the full PR #613 recovery source/state/APIs, ckSOL ICP flow and merged frontend explorer/balance behavior are in the build inputs. This is source preservation, not a fresh connected-wallet or financial-behavior test.

Single serial build session `62203`: November 4 **20:13:36–20:15:31 PST**, approximately **1m54s**. Cargo release compile 1m24s; canonical public Candid metadata, shrink/keep-name, O1/keep-name, then deterministic `gzip -9 -n`; frontend build followed packaging and exited 0. The explicit shared-target output was copied directly to the isolated evidence directory, avoiding the previous `icp build` wrapper's relative-copy mismatch. No source or lockfile changes, duplicate tests, dependency update or hosted CI were performed.

Backend and local production path-dependency roots were timestamp-refreshed before Cargo to invalidate known cross-worktree cache reuse, with unchanged hashes in [freshness.json](evidence/cksol-integrated-6847982b/freshness.json). The compile log explicitly identifies this worktree's `rumi_cycle_manager` and backend paths. Rust/Cargo 1.92.0; ic-wasm 0.9.10; Node 24.13.1; npm 11.8.0; didc 0.4.0. Exact command times/arguments are in [sessions.json](evidence/cksol-integrated-6847982b/sessions.json).

Preflight found a transient unrelated compiler; a fresh inventory was empty before start. Free disk began at 67,852,332 KiB and ended at 67,140,428 KiB: approximately 695 MiB net volume growth, below the 6 GB cap. Shared target began at 20,136,492 KiB and ended at 20,137,304 KiB; new local artifact evidence used 25,756 KiB. Read-only process inventory after completion found no Cargo/rustc/ic-wasm process, and the owned orchestration exited 0. The 30-minute slot is released. Worktree inventory: zero eligible, 75 blocked; nothing removed.

## Backend candidate — exact identity

Local preserved directory: `/Users/robertripley/.codex/worktrees/solana-icp-processing/rumi-protocol-v2/build-evidence/cksol-integrated-6847982b/`. Logs and binaries remain local; readable reports/hashes are pushed.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `backend-raw.wasm` | 13,869,749 | `53ac689d9be3f5234dff69a18ad220a32800631a874ef2cfde6d0b241988ec9d` |
| `backend-optimized.wasm` | 9,618,614 | `3fca699dbecf214fb4f1693244e89a38677cb09635018abb0ed8e61fb4527d5f` |
| **`backend-optimized.wasm.gz` — upload candidate** | **2,323,301** | **`d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`** |

Executable code section: raw 9,845,480 bytes; optimized **8,697,159 bytes**. With the 12,582,912-byte network code-section limit and unchanged 262,144-byte reserve, headroom beyond reserve is **3,623,609 bytes**. Total module size, executable section and gzip transport bytes are distinct. ICP CLI 1.3.0 automatically uses 1 MiB chunks above its 2 MiB message threshold; this candidate needs three chunks and no source shrinking campaign. See [pinned install implementation](https://github.com/dfinity/icp-cli/blob/v1.3.0/crates/icp-cli/src/operations/install.rs#L184).

Gzip decompression byte-matches the optimized module. Extracted embedded Candid byte-matches the source DID plus the extractor's single trailing newline. **The embedded Candid from this exact optimized Wasm passes `didc check` against freshly retrieved live Candid**, exit 0; [comparison result](evidence/cksol-integrated-6847982b/compatibility.txt). Live Candid SHA-256 `48d202ada181389c1b473067e3b23396fae0fe6fd75055b1e126e73a268487d5`; embedded SHA-256 `eff9065a3c2015d84ee46e0b600af4c020a2dbdc12a3e9171f229e230e7e20fd`. All 402 Wasm exports were parsed; required Vault 51 recovery and ckSOL ICP methods are present. [Artifact report](evidence/cksol-integrated-6847982b/artifact-report.json).

## Paired frontend artifact — not part of installation authority

`npm run build --workspace=src/vault_frontend` passed, including the canonical postbuild Internet Identity/custom-domain asset check. The complete local dist manifest has **740 files / 10,892,798 payload bytes**. [Frontend manifest](evidence/cksol-integrated-6847982b/frontend-manifest.json) SHA-256 `de790d03ea64e9e00d6600b9f2d415ab5135d5ed707c4a9f1a0e46b20b6931b2`; index SHA-256 `e18a5300bb6363ec581a0d0c27c106fe37232b21abbae52446997a91c1ce591f`; `.ic-assets.json` SHA-256 `8145e46da570b70f4989acd92399df5449988c3feadabdfcd1bee5b24bc286c8`. Frontend warnings did not fail the build/auth check.

No live asset-map/encoding comparison, asset batch or served-byte readback was performed. Frontend publication remains a separate later packet that must inventory all live keys/properties/encodings and preserve them. Installing the backward-compatible backend does not itself publish the new ICP UX or establish end-to-end availability. The reserve remains off; do not describe this as a launched funded service.

## Target and read-only preflight

- Network **`ic`**; backend **`tfesu-vyaaa-aaaap-qrd7a-cai`**.
- Identity **`rumi_identity`**; verified controller principal **`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`**.
- Refreshed controller-authenticated status within the build window: **Running**, current module **`d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95`**, cycles 10,553,405,374,973; memory 1,120,880,323 bytes. [Full settings/controller baseline](evidence/cksol-integrated-6847982b/status.json); preserve every controller, allocation, freezing/memory limit, environment variable and log setting.
- Fresh protocol queries preserve GeneralAvailability, frozen=false, manual override=false, mint cap and collateral configuration; raw query results remain in the local evidence directory. [Read arguments/timestamps](evidence/cksol-integrated-6847982b/reads.json). Readbacks are per-time observations and must be refreshed immediately before separately approved maintenance.
- Existing relay budget is **0**. Operator-bound deposit is null; retained paid-cycles receipt sequence 1 is FundingRejected / InsufficientAllowance(0), with no funding/refund block or deposit ID. Preserve it without retries. Operator operation status has active=null, watermark=0. Pending payout page is empty with no next cursor. Paid quote next_sequence=5 is not a global owner/journal inventory. These scoped reads do not prove drain or absence of all unresolved operations.
- New ICP reserve endpoints are not in the currently deployed service. The candidate initializes absent reserve state to zero; after a future installation require `get_cksol_icp_terms.reserve_available_cycles=0`, `available=false`, and old relay budget=0. No setter, top-up or financial call is part of this packet.

## Proposed installation-only scope — for explicit future authorization

The [accepted stop/drain runbook](rumi-backend-drained-upgrade-runbook-20261101.md) applies. A direct running upgrade remains unsafe for the unresolved callback seam. This record is a concrete proposal, not a management-call authorization.

1. Reconfirm controller/status/current module/settings, source/candidate/args hashes, relevant held financial journals, and exact snapshot inventory. Coordinate the single operator and downtime. If the baseline, source, candidate, or unresolved outcomes differ, hold for review rather than substituting silently.
2. Apply the already-established [archive-before-replace snapshot sequence](cksol-upgrade-only-packet-20261104.md#snapshot-policy-clarification--source-inspection-only). The refreshed [inventory](evidence/cksol-integrated-6847982b/snapshots.txt) has all ten slots occupied; oldest is **`00000000000000070000000001f088fe0101`**, September 13, 2026 21:30:08 UTC. Before replacement, preserve its complete native upload-compatible metadata/module/Wasm memory/stable memory/chunk store; verify every declared length, timestamp and SHA-256. **The current oldest snapshot archive is now downloaded and verified**, as recorded in the archive addendum below. Recheck its manifest and the live inventory before any separately approved replacement; preservation does not authorize replacement.
3. The future approval must explicitly name this one old on-network snapshot identity for replacement, one stop/drain, one fresh stopped-state snapshot and its verified download, one exact backend upgrade, and one reconciled start. No standalone delete or other snapshot pruning is proposed. Submit stop once and record request/result/time; continue only after controller-authenticated **Stopped**. Empty owner reads, CLI success, timeout, or Stopping are not drain proof.
4. Once stopped and old archive preservation is verified, compare all ten entries again, then execute the explicitly authorized `snapshot create --replace` for that exact old ID once. Preserve and verify the returned fresh snapshot and all nine other IDs unchanged before installation. Unknown snapshot outcome means hold/reconcile; never repeat or switch executor. No snapshot restore, obsolete-state rollback or unverified slot deletion is authorized.
5. Reconfirm candidate/args/current module/settings; submit the exact upgrade below **once** only after explicit approval and all preconditions. Retain original request/result. On timeout or any unknown installation outcome, inspect authoritative status/module/request evidence before another action; do not reinstall, retry or automatically restore. CLI automatic stop/start behavior is not a substitute for preconfirmed Stopped status.
6. Only after installation outcome and module are known, perform the separately approved start once. Verify Running, uploaded gzip identity, deployed Candid, preserved controllers/settings/config/mode/cap, scoped receipt and hold retention, new ICP terms reserve=0/available=false, and relay budget=0. Do not claim minting, settlement or refund behavior from a healthy query. If stop/install/start is uncertain, remain held for reconciliation; no automatic cancel/restart/replay. Explicit start-cancellation authority may be used only after reconciling a failed/indefinite stop, per runbook.

Reference command, **not executed**:

```sh
icp canister install tfesu-vyaaa-aaaap-qrd7a-cai \
  --network ic --identity rumi_identity --mode upgrade \
  --wasm /Users/robertripley/.codex/worktrees/solana-icp-processing/rumi-protocol-v2/build-evidence/cksol-integrated-6847982b/backend-optimized.wasm.gz \
  --args-file docs/operations/evidence/cksol-integrated-6847982b/upgrade-args.did
```

Canonical [upgrade arguments](evidence/cksol-integrated-6847982b/upgrade-args.did), SHA-256 **`88df3b8241946fddd77dd273bf9325e4855794f70fee06272060c57359965a3f`**:

```candid
(variant { Upgrade = record {
  mode = null;
  description = opt "ckSOL ICP processing with deployed Vault 51 recovery; source 6847982b; reserve off"
}})
```

`mode=null` preserves the live mode and does not enable the reserve. This packet proposes no setter, reserve funding, ICP/cycles conversion, token approval, SOL processing, repayment, payout, reconciliation, customer message, or financial canary. Vault 158 remains a separate held subproject.

## Evidence limits and handoff

Prior focused source results remain 21 backend / 1 generator / 9 frontend plus live Candid compatibility. This slot performed only the admitted production builds and artifact/readback checks, not duplicate tests or a full lifecycle suite. Both path-triggered hosted workflows were intentionally unrun at PR #624 head e9466ecb; hosted CI is not claimed green. Full policy audit found no independent hosted mandate and no new source-merge gate. Later release procedures may select additional evidence explicitly; do not manufacture it as an implied prerequisite here.

Full asynchronous lifecycle/client-money evidence and reserve safety remain unproven. The callback root cause remains unresolved; stop/drain is the accepted mitigation. Return this packet and its SHA-256 to Merritt for review and the actual approval question. Current holds remain: management/snapshot mutations, installation, frontend upload, reserve funding/activation, canary and all financial/customer actions. No such action occurred during preparation.

## Oldest snapshot archive — completed read-only preservation

Routine archive preservation was separately authorized and completed November 4, 2026 at approximately 20:22 PST. No matching archive was found in the inspected primary recovery store and prior ckSOL evidence locations. ICP CLI 1.3.0 `snapshot download` was used for **only** existing snapshot `00000000000000070000000001f088fe0101` on backend `tfesu-vyaaa-aaaap-qrd7a-cai`, mainnet, identity `rumi_identity`.

The pinned CLI implementation calls `read_canister_snapshot_metadata` and `read_canister_snapshot_data`; it never calls snapshot take/load/upload/delete in this path. These are read operations even though the management interface permits replicated ingress. [Official read semantics](https://docs.internetcomputer.org/references/ic-interface-spec/management-canister/#ic-method-read_canister_snapshot_metadata) return existing metadata/data; unlike `take_canister_snapshot` or upload, they do not create, replace or alter a managed snapshot. The only writes here were local archive/evidence files.

Protected canonical archive directory:

`/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/rumi-cksol-integrated-20261104/snapshot-00000000000000070000000001f088fe0101`

Directory mode 0700; metadata/payload files 0600; Git-ignore protection verified. Raw globals, memory and state remain local and are not committed. The native layout contains metadata, module, Wasm/stable memory and the declared chunk-store directory, plus local SHA256SUMS and verification provenance. [Pushed verification report](evidence/cksol-integrated-6847982b/archive-verification.json), SHA-256 **`2cdd98a0b929d066b09f677c0762501e92cd2292060cf8425e90d753c7c02174`**, binds exact canister/network/snapshot/path and records:

| File | Verified bytes | SHA-256 |
| --- | ---: | --- |
| metadata.json | 518 | `9fa1e635d4f48e888d11dd06eb95fcbf8f6ef7e5e99c6494a45a654cdacd7295` |
| wasm_module.bin | 2,121,582 | `bc75ca47f86d475b40d13e8198bae77d1e65aa4ce4981335ce3aae1d44a59b83` |
| wasm_memory.bin | 2,359,296 | `ceb3ab821c77f911ab9d34cb10cd8bafc9b4d5b38dd74b1023780a6ad1bbf1bb` |
| stable_memory.bin | 83,951,616 | `f83ed27784df09c227249fa91cd71a157b24ec89986a0b55d7fd5d9e91558456` |

Every binary length matches its specific metadata field; payload total is **88,432,494 bytes**. Snapshot timestamp **1789335008780980551 ns** matches September 13, 2026 21:30:08 UTC in the live inventory. All metadata globals are representable for native upload; the declared chunk store is empty and its directory has no payloads. We do not equate the inventory's aggregate size with the sum of named payloads. The snapshot module is historical and is not expected to match today's installed backend.

The full before/after managed inventories byte-match (SHA-256 in report), so all ten IDs/timestamps/sizes are unchanged. Post-read authenticated status remains Running with module `d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95` and settings identical to the build preflight. No restore roundtrip was performed or implied; native-format completeness and preservation checks passed. No create/delete/replace/restore, stop/start/install, new build, reserve operation or financial action occurred.

This resolves the previously unperformed **old-slot archive preservation** step. The remaining proposed fresh snapshot replacement/download, stopped upgrade and reconciled start still require the exact explicit future execution authority described above. Keep both archives when a fresh one is later authorized; never automatically restore this old state over intervening traffic. Return the updated packet to Merritt for review and the canonical approval question.
