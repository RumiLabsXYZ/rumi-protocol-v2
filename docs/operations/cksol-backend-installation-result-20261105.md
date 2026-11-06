# ckSOL backend installation and readback — November 5, 2026

**Result: the exact approved backend is installed and Running. ICP reserve and relay sponsorship remain off.** No frontend publication, reserve funding/activation, financial canary, payout, repayment, mint, reconciliation or other financial call was performed by this operation. The backend mutation window is complete; later work remains under its separate authority.

## Authority and exact artifact

Rob approved item 1 in Merritt's chat; Merritt relayed exclusive execution authority for the [installation-only packet](cksol-installation-only-packet-20261104.md), commit `ab3b4a0cc402981173e962d412b7eb4287231269`, SHA-256 `eba53fa536bc57eaad11c91cf9cbfefc72f04019ea86f9019d470174f6c9bd7b`. That scope included one stop/drain, the specified archived-slot replacement and verified fresh backup, one exact upgrade, restart and readback. It excluded frontend and financial/activation actions.

- Mainnet backend: **`tfesu-vyaaa-aaaap-qrd7a-cai`**.
- Identity: `rumi_identity`, verified principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
- Artifact source: PR #624 merge **`6847982b44f247e0c99f0a453b583d3fab8107a2`**.
- Previous module: `d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95`.
- **Installed gzip/module SHA-256: `d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`.**
- Exact approved Upgrade arguments SHA-256: `88df3b8241946fddd77dd273bf9325e4855794f70fee06272060c57359965a3f`; mode=null, with reviewed source/reserve-off description.

The approved packet, candidate and arguments were rehashed before the first write and again immediately before install. No prior installation dispatch directory/markers existed, the live backend was Running on the expected old module, settings/inventory matched, and fresh live Candid compatibility passed. Only live prices/timestamps and their derived risk values had moved since preparation; fixed settings matched. Existing archive payloads were rehashed. Private scoped protocol, operation/payment and claim baselines were captured before stopping.

## Once-only execution and reconciliation

Times below are November 5 **PST**. Each command had a durable exclusive dispatch marker before invocation; no management mutation was retried. CLI exit results were reconciled with controller-authenticated status, module and snapshot state before the next mutation. The CLI did not expose management request IDs; none are invented. Original command arguments, timestamps and stdout/stderr remain in the protected local execution directory.

| Action | Dispatch → CLI completion | Outcome |
| --- | --- | --- |
| Stop | 17:16:38 → 17:16:39 | Exit 0; authenticated **Stopped** with old module/settings verified |
| Replace exact archived snapshot | 17:17:29 → 17:17:32 | Exit 0; fresh ID returned and nine other entries verified unchanged |
| Download fresh snapshot (read only) | 17:18:00 → 17:18:14 | Exit 0; metadata, payloads and module verified before install |
| Upgrade exact approved gzip | 17:19:08 → 17:19:24 | Exit 0; exact new module and Candid verified while still Stopped |
| Start | 17:20:03 → 17:20:05 | Exit 0; authenticated **Running**, exact module/settings verified |

The stop-dispatch through restart-completion interval was approximately **3m27s**. The [drained-upgrade runbook](rumi-backend-drained-upgrade-runbook-20261101.md) was followed; empty application views were not used as drain proof, and the CLI's running-upgrade guard was not substituted for explicit Stopped status. There are no unknown management outcomes or pending owned write processes.

## Snapshot preservation

Before replacement, all ten live entries matched the approved inventory and archived old snapshot **`00000000000000070000000001f088fe0101`** was reverified. Only that old on-network identity was replaced. Its complete protected local archive remains intact.

Fresh stopped-state snapshot: **`00000000000000160000000001f088fe0101`**, timestamp **1793927851682795038 ns** (November 5 17:17:31 PST). Its native download contains **113,971,168 named payload bytes**; the management aggregate reports 113,971,296 bytes. The 128-byte aggregate difference is retained as accounting metadata, not treated as a failed named-length check.

- Module: 2,297,824 bytes, SHA-256 `d4d60e5176efd4aba0be1b583a9f6a9acc2e6b77b1d971a37d5cc82c65740c95`, exactly the pre-upgrade installed module.
- Wasm memory: 2,555,904 bytes; stable memory: 109,117,440 bytes. Every named length matched metadata and every hash is recorded.
- All globals are representable for native upload; the declared chunk store and its directory are empty.
- All nine other managed IDs, timestamps and sizes remained unchanged, both after replacement and after upgrade/restart. Managed count stays ten.
- Both old and fresh local archives are preserved under the existing protected recovery store. Raw memory/global/state payloads remain private and are not committed. No restore roundtrip or rollback occurred; no automatic stale-state restore is authorized.

[Fresh snapshot verification](evidence/cksol-installation-20261105/fresh-snapshot-verification.json) and [final inventory](evidence/cksol-installation-20261105/snapshots-final.txt).

## Live readback

Controller-authenticated status is **Running** on the exact approved new module. All controllers, environment variables, allocation/freeze/memory/log settings match preflight. Deployed Candid matches the actual candidate's embedded service (including the three preserved Vault 51 recovery methods and new ICP-payment API).

Query readbacks confirm:

- **ICP reserve_available_cycles=0; available=false.** New operator ICP payment is null; next ICP sequence=1. Service fee terms report 500 bps but are inactive.
- **Existing relay budget=0.** Old paid-cycles quote, operator deposit/payment, operation watermark, pending payout page and claim query are byte-identical before/after.
- Operator's sequence-1 FundingRejected receipt, absent funding/refund blocks and deposit ID were retained. No receipt replay/recovery was executed.
- The claim remains ClaimConfirmed, withdrawal #1696, release=null; the complete private query bytes are unchanged. This is retained state, not WaterNeuron settlement or payout proof.
- GeneralAvailability, frozen=false, manual override=false, mint cap and all fixed configuration remain preserved. Normal price/timestamp and derived risk/curve changes are explicitly excluded from fixed-configuration comparison. [Comparison](evidence/cksol-installation-20261105/configuration-comparison.json).

[Installed API terms](evidence/cksol-installation-20261105/after-get_cksol_icp_terms.txt), [retained scoped-state hash comparison](evidence/cksol-installation-20261105/retained-state-checks.json), [final controller status](evidence/cksol-installation-20261105/status-final.json), and [execution report](evidence/cksol-installation-20261105/deployment-report.json) separate installation/availability evidence from financial behavior. These checks do not enumerate every owner or establish an end-to-end paid mint/refund lifecycle.

## Handoff and remaining boundaries

Backend installation/readback is complete. No new production build, broad suite or hosted CI was run. Under the repository low-disk policy, the guarded helper refreshed origin and removed one eligible clean/unused/merged worktree (about 16.1 GiB); 68 blocked worktrees and all current evidence/archives were preserved. No local branch was deleted manually.

The prepared frontend publication plan remains held for an explicit later publication decision and fresh inventory comparison. Reserve funding/activation, canaries and client-money operations remain held; the unresolved running-upgrade callback cause is not claimed fixed by this successful stopped upgrade. Vault 158 preparation is a separate approved scope that was queued behind this completed backend mutation window. Return this result to Merritt; do not infer any additional execution authority.
