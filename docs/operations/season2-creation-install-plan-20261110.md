# Season 2 creation and installation plan — HELD

This is a reviewable plan, not execution authority. Execute only after Rob approves
the [final decision packet](../coordination/season2-release-decision-20261110.md)
by **2026-11-10 18:45 UTC (10:45 AM Pacific)**. Proposed T0 is **19:00 UTC
(11 AM Pacific)**, nat64 **1794337200000000000**. If approval is late or setup
cannot finish in time, HOLD. Do not silently move T0. This preparation executed
no creation, installation, cohort query, import or activation.

## Pins and budget

Use the existing locked Season 2 checkout, branch
`codex/season2-accounting-20261110`, at its `icp.yaml` root, with `icp 1.3.0`.
Selector **rumi_points_season2**, environment **mainnet-season2**, network **ic**.
Never run a whole-stack deploy or target S1.

Controller/admin: `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`
(identity `rumi_identity`). Funding source is this identity's default/null
subaccount on cycles ledger `um5iw-rqaaa-aaaaq-qaaba-cai`.
Observed balance: **4,300,650,044,313 cycles**. Proposed creation amount:
**900,000,000,000**. Selected 13-node subnet creation fee: **500,000,000,000**
deducted from that amount, leaving **400,000,000,000 before install**. Additional
ledger fee: **100,000,000**. Total source debit: **900,100,000,000**; expected
remaining source balance: **3,400,550,044,313**, absent intervening changes.
Incoming 4T credit does not authorize spending it all. No S1 funding is proposed.

Subnet: `jtdsg-3h6gi-hs7o5-z2soi-43w3z-soyl3-ajnp3-ekni5-sw553-5kw67-nqe`.
Pricing revision: `83f29e0803496ba6aaea3d6b43f6c6bebea12dd9`.
Settings: zero reserved compute, zero fixed memory, normal 2,592,000-second
30-day freezing threshold, controller-only logs.

| Input | SHA256 |
| --- | --- |
| Gzip, 747,354 bytes | `967e22d162579f5626283b094bd0a1457f09c009f34ed455b676f252975def8c` |
| Module, 3,018,333 bytes | `dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17` |
| Exact init text | `370b484ec894e849984ffdfcf377ab23ef6f573e26a838474e1a0d470398e179` |
| Encoded init, 139 bytes | `be20ba1f4f068c00dd0a503942da86c0f0b948f853fae65e2c192cddae535c3a` |

Public H0: `d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8`.
S0 remains private with Orin. No seed belongs in source or the transcript.
Reuse existing candidate source `87f78737`; no rebuild. The gzip is locally staged
at the ignored path below. Verify gzip, decompressed module and init hashes
before use. [Machine-readable pins](evidence/season2-release-preparation-20261110/creation-install-pins.json).

## Commands prepared for approval

Before authorized creation, freshly verify identity/principal, source balance,
ledger fee, subnet CMC eligibility, 13-node count and deployed pricing revision.
Confirm S2 mapping is `{}` and S1 mapping unchanged. Drift means stop and
reconcile; never silently substitute a subnet, increase funds or reuse a canister.

```sh
icp canister create rumi_points_season2 \
  --environment mainnet-season2 --identity rumi_identity \
  --subnet jtdsg-3h6gi-hs7o5-z2soi-43w3z-soyl3-ajnp3-ekni5-sw553-5kw67-nqe \
  --cycles 900000000000 \
  --controller fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae \
  --compute-allocation 0 --memory-allocation 0 \
  --freezing-threshold 2592000 --json
```

Record the returned ID and creation/debit receipt. Independently read
`.icp/data/mappings/mainnet-season2.ids.json`: exactly the S2 selector must map
to that fresh ID, distinct from S1 `bfnu3-6aaaa-aaaab-qhanq-cai`. Verify empty
module, pinned sole controller/settings and expected pre-install balance.
Unknown/interrupted outcome means HOLD and reconcile ledger/mapping; never
automatically repeat creation.

After exact hash verification, proposed installation on the verified empty ID:

```sh
icp canister install rumi_points_season2 \
  --environment mainnet-season2 --identity rumi_identity \
  --mode install \
  --wasm .recovery-artifacts/season2-release-20261110/candidate.wasm.gz \
  --args-file docs/operations/evidence/season2-release-preparation-20261110/season2-init.did
```

An uncertain result means HOLD and read back module/state; never replay with
reinstall or upgrade. Read back Running status, exact decompressed module hash,
admin, T0, H0, open end, settings, actual managed memory, idle burn and cycles.
Confirm both timers off and empty instance. Installed is not activated or awarded.
The generic 1T health indicator may be false at 400B; it is separate from replica
freezing and is not changed.

## Cutoff ordering and separate authority

The final decision packet controls historical method/caps, private manifest and
transcript, fresh 15-owner public-position queries and activation. Those queries
were previously held by approval review; none run here. This plan itself does
not authorize begin/import/finalization, seed reveal, timers or `start_season`.

`begin_season2_import` requires future T0: after install/readback and **before
19:00 UTC**. `import_season2_batch` rejects `now < T0`: cutoff-state capture,
final batches and finalization occur **at or after T0**. Full import need not
finish before T0. If beginning cannot happen in time, HOLD without moving T0.

Before `start_season`, explicitly verify private S0 matches H0 and first snapshot
A remains more than 30 minutes in the future. Code does not reject an
already-passed A; successful start alone does not prove timing. Keep exact
future snapshot times private. A running poll/open epoch is separate from
ordinary awards, observed after epoch close, normally weekly. No token claims,
end-date decision, S1 writes or additional funding authority are included.
