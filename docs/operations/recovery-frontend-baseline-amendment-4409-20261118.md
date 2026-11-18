# Exact UI approval packet: 4409 backend baseline amendment

**Preparation only. No UI staging or activation is authorized or performed by this packet.** Dorian remains the sole proposed publisher after Rob's separate exact approval. All financial and liquidation holds remain intact.

## Decision requested

Approve Dorian to publish the existing verified recovery-menu/manual-queue frontend artifact to `tcfua-yaaaa-aaaap-qrd7q-cai` on `ic`, using existing `rumi_identity` and the unchanged [original dispatch procedure](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/37380ddd3d8585c0b1c85d04127b865bebf2bf79/docs/operations/recovery-frontend-dispatch-20261116.md), with only the amended directory and execution pins below:

1. Stage **327 new immutable keys and 436 content sets** (763 asset operations).
2. Independently verify the complete staged inventory, stored bytes and preservation before activation. Bind the stage proof to this amended manifest.
3. Activate **seven content sets across six existing entry keys**, reaching **1,088 total keys**.
4. Independently verify final served bytes, preserved assets and desktop/mobile recovery-menu and manual-queue behavior. Do not claim the customer outcome from publication alone.

The UI removes the generic global recovery card, restores the explicit wallet-menu recovery entry and restores the manual queue's availability safeguards. No new UI implementation is proposed here. The same previously tested and reviewed artifact is reused without a rebuild or new review campaign.

Retain all **761 historical keys**, every existing property and every untouched encoding. Only the seven explicitly pinned entry encodings change at activation. Preserve discovery, Internet Identity/domain files, frontend/backend modules, settings, controllers, Commit/Prepare permissions and the Stability Pool pause. **No asset deletion, backend installation, fund movement, reserve/cap change, Vault 202 recovery, Vault 208 financial remedy, bot/worker resumption, claim retry or liquidation resumption is included.**

## Why this amendment exists

The separately approved backend upgrade is live. Its [immutable execution receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b667e53a17a965865214784fead4396e97b2e3d7/docs/operations/xrp-composed-partial-live-result-20261118.md), SHA256 `ce364709e88f9690ea0fb2a36bdd3bb5984c5665b37f7aecf73b75be055d7cab`, was independently hash-verified. That approval does not authorize UI publication.

| Observation | Preserved ce014 packet | New live baseline |
| --- | --- | --- |
| Backend module | `ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f` | `4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde` |
| Certified backend Candid SHA256 | `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6` | Same, byte-identical |

The old [ce014 packet at 64bacefa](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/64bacefaf14c91aef2fdea84c753d0a18bd587e9/docs/operations/recovery-frontend-baseline-amendment-ce014-20261117.md), its inputs, and the original 37380ddd packet remain unchanged. Their backend pins are stale; do not run them against 4409, loosen their checks or silently reuse their approval identity.

## Fresh baseline and necessary checks

Independent read-only capture completed **November 18 at 12:54:44 PM PST** (`2026-11-18T20:54:44.044Z`). Backend and frontend are Running. Certified module/controller observations agree with management readbacks. Backend module is exactly 4409 above; frontend remains `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`. Settings hashes, both controller sets and exact Commit/Prepare response bytes are unchanged. Read-only pause observation is `(true)`.

A stable double-read produced all 761 asset rows. Every row, property, encoding hash/size and modification timestamp exactly matches the previous complete inventory. Both public discovery URLs separately returned the preserved bytes: architecture SHA256 `22bcdf6a5f1ad8e4c1b241a46e3a7f1ae5bec0ab16a79e62ce215288e862f702`; II derivation-origin SHA256 `16cf6b5a8eb787b3222ba58308cd6234755b20be869337e1e11e42b51eee53a0`. System curl used its default TLS verification after local Python's CA store lacked the issuer; verification was never disabled. Inventory observations do not independently prove HTTP certification.

Certified 4409 Candid is byte-identical to ce014. `didc check` against the original 59a Candid exited 0. No declarations, source or dependencies were edited. Local-only dispatcher inspection revalidated all 764 artifact files and encoded the unchanged phases; it made no canister call. Read [fresh baseline](evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118/fresh-baseline-confirmation.json), [compatibility](evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118/candid-compatibility.json), [served discovery checks](evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118/discovery-served-check.json) and [data/hash checks](evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118/amendment-checks.json).

## Exact immutable inputs

New directory: [dispatch-4409-20261118](evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118). Compared with ce014, exactly three previously pinned data files change: `state-before.json` replaces backend module; `certified-before.json` replaces backend module and capture time; `dispatch-manifest.json` replaces its module description and resulting state hash. Certified Candid is unchanged. All other pinned file bytes, every executable and complete phase objects are identical. The execution-pin envelope records this separate amendment and no execution authority.

- **Execution pins SHA256:** `6fc4fabb1091ca56aa8b27f8a0dc12802b4acb79db7f5f901d86ef2c13eceee3`.
- **Dispatch manifest SHA256:** `f29280e6137ab3e24ccdae690d7ec2c873ef7dbac1201b4fe6253f98dfc30272`.
- Publisher remains `ec7198cebc81260224a003eaf413bd7db15ae3a95f3cc73617e9f900def541a4`.
- Frontend manifest remains `82768e536040d1a45b77215f3bd48ae1290945e80a44f3d5968735c97a319e82`.
- Artifact archive remains `a765d0410b30fec71f63883635550dcd306cc791a3e8e5348a4cb95c31102780`.
- Previous ce014 execution pins remain `475a8b21dd5b20cc37d235d67253b11a3252164a77d7f9a0c3e6f4672284ddd1`; original pins remain `e12c4db48741d377706eccffb53b399e581e3ccbf4b15d59372f09ca1a6f18e7`.

After separate actual approval, only these substitutions apply to the original procedure:

```sh
DISPATCH=docs/operations/evidence/recovery-frontend-publication-20261116/dispatch-4409-20261118
export PUBLISH_PINS_SHA256=6fc4fabb1091ca56aa8b27f8a0dc12802b4acb79db7f5f901d86ef2c13eceee3
```

All exact runtime preflight, durable flushed-argument, post-flush deadline, exclusive phase-journal, unknown-outcome and independent stage-proof guards remain unchanged. The two phases still require at most 13 calls: two create_batch, nine create_chunks and two commit_batch. No blind retry, executor change or alternate publication path is authorized. A future material baseline change stops for reconciliation.

## Remaining approval boundary

Automatic approval review previously rejected the staging write before process creation because it did not accept older cross-chat references as trusted exact production authorization. No publisher process, journal or asset call started. This is the replacement exact packet for the **existing release-chat approval decision**, not an instruction to retry or a duplicate approval question.

Only baseline preparation and read-only/local verification occurred. There was no staging, activation, publication, backend write or financial/liquidation action. Dorian's receipt establishes the separately deployed backend's broader state preservation; this amendment does not claim an independent re-audit of all vault financial rows. Unrelated working-tree deletions were preserved. Required guarded disk-pressure cleanup found zero eligible worktrees and removed zero.
