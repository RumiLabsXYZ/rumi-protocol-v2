# UI publication baseline amendment after the approved backend upgrade

**Preparation only. UI staging and activation remain held with Dorian.** This amendment reconciles the separately approved November 17 backend installation; it grants no UI or financial authority. The historical [37380ddd dispatch](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/37380ddd3d8585c0b1c85d04127b865bebf2bf79/docs/operations/recovery-frontend-dispatch-20261116.md) and its frozen inputs are preserved unchanged.

## Verified reason for the amendment

Dorian's [immutable live-result receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/8a42c5212f7f7a8a688c51e5c4d1b4bf856b17ac/docs/operations/xrp-cycle-bound-live-result-20261117.md), document SHA256 `f8d8c7ed24452b6b6a74e7f5163561ae3ce6ba13234a98d9314aa789749e7ced`, records Rob's separate backend approval and installed retry-bound/pause support. Its document hash was independently matched. Independent live readback completed at `2026-11-17T16:34:10.248Z` (November 17 Pacific) confirms the following exact changes:

| Expected observation | Old historical UI baseline | New observed baseline |
| --- | --- | --- |
| Backend module | `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111` | `ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f` |
| Certified backend Candid SHA256 | `74cd6c8f7a8b0667bf10f2cd50b1a3c81e55ff863b0cb9520fb8eca1795e03fb` | `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6` |

The backend is Running. Its settings hash and controller set are unchanged. The Stability Pool pause reads true. The frontend module remains `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`; its settings/controllers and Commit/Prepare lists are unchanged. All 761 frontend asset rows, including every property, encoding hash, size and modification timestamp, exactly equal the previous complete inventory. Both discovery files and existing II/domain files remain unchanged. [Fresh confirmation](evidence/recovery-frontend-publication-20261116/dispatch-ce014-20261117/fresh-baseline-confirmation.json) retains the actual readback.

The certified new Candid and retained old Candid passed the existing tool's directional compatibility check: `didc check backend-ce014.did backend-59a.did`, exit 0. [Compatibility evidence](evidence/recovery-frontend-publication-20261116/dispatch-ce014-20261117/candid-compatibility.json) records both full hashes. No declarations were regenerated or edited, no backend/source build was run and no frontend tests were repeated.

## Exact data-only amendment

The new [dispatch directory](evidence/recovery-frontend-publication-20261116/dispatch-ce014-20261117) is a separate copy. Every publisher, verifier, reader, decoder, materializer, encoder and boundary-check byte is identical to the final reviewed dispatcher. Only three previously pinned data files change: `state-before.json` replaces the exact backend module; `certified-before.json` replaces the exact backend module/Candid observations and capture time; `dispatch-manifest.json` replaces its backend-module description and the resulting baseline-state hash. All payload phases and operations remain identical.

- Old execution pins remain `e12c4db48741d377706eccffb53b399e581e3ccbf4b15d59372f09ca1a6f18e7`.
- Amended execution pins: **`475a8b21dd5b20cc37d235d67253b11a3252164a77d7f9a0c3e6f4672284ddd1`**.
- Amended dispatch manifest: **`3ddef12644ea1eb922b4799d6150b30e48e7e56cb66c4e52b9b3e53e295c62ee`**.
- Publisher remains **`ec7198cebc81260224a003eaf413bd7db15ae3a95f3cc73617e9f900def541a4`**.
- Complete frontend manifest remains `82768e536040d1a45b77215f3bd48ae1290945e80a44f3d5968735c97a319e82`; archive remains `a765d0410b30fec71f63883635550dcd306cc791a3e8e5348a4cb95c31102780`.

[Amendment checks](evidence/recovery-frontend-publication-20261116/dispatch-ce014-20261117/amendment-checks.json) prove those exact differences, unchanged code, identical phase objects and old-packet preservation. Local-only inspection revalidated all 764 artifact files and encoded the same stage/activation operations with no canister call. Counts remain 327 new immutable keys, 436 stage content sets, seven activation sets across six existing entry keys, and final 1,088 keys. No delete, unset, property change, permission/controller change or financial action is added.

## Held execution handoff

Do not run the old 59a-pinned dispatcher against the ce014 backend, and do not treat backend approval or discovery consent as UI approval. Dorian must reconcile this amendment in his existing direct UI approval packet/question without duplicating questions or silently changing pins. The UI execution hold and automatic-review boundary remain. No staging, activation, journal consumption, backend write, reserve/worker change, Vault 202 recovery, Vault 208 sizing release or Stability Pool resumption occurred in this work.

After actual UI authorization and Dorian's phase release, use the original dispatch procedure with only these path/pin replacements:

```sh
DISPATCH=docs/operations/evidence/recovery-frontend-publication-20261116/dispatch-ce014-20261117
export PUBLISH_PINS_SHA256=475a8b21dd5b20cc37d235d67253b11a3252164a77d7f9a0c3e6f4672284ddd1
```

Reuse the existing verified artifact; do not rebuild or rematerialize it unnecessarily. Runtime readers still require exact current modules/settings/controllers/API/permissions/hold and full asset rows before upload and commit. All deadline, durable-argument, one-use journal, unknown-outcome and independent stage-proof gates are unchanged. Stage proof must bind to the amended dispatch-manifest hash above; old-plan proofs are not accepted. Final served bytes, preservation and desktop/mobile checks remain after actual publication. No UI publication or customer outcome is claimed by this amendment.

## Exact approval requested

This is the exact amended packet for Rob's decision. Approve Dorian to publish
only the existing, verified frontend artifact to frontend
`tcfua-yaaaa-aaaap-qrd7q-cai`, following the unchanged
[original dispatch procedure](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/37380ddd3d8585c0b1c85d04127b865bebf2bf79/docs/operations/recovery-frontend-dispatch-20261116.md)
with the amended path and pins above:

1. Stage **327 new immutable asset keys and 436 content sets**.
2. Independently verify the complete staged state before activation.
3. Activate **seven content sets across six existing entry keys**, reaching
   **1,088 total keys**, then independently verify served bytes, preserved assets
   and the desktop/mobile recovery-menu and queue behavior.

Retain all **761 historical keys** and every property and untouched encoding;
only the seven explicitly pinned entry encodings change during activation.
Preserve discovery and Internet Identity/domain files, both canister modules,
settings, controllers, permissions and the Stability Pool pause. No asset deletion,
backend installation, fund movement, reserve activation, Vault 202 recovery,
Vault 208 sizing release, bot/worker action or liquidation resumption is included.
All financial and liquidation holds remain intact.

Automatic approval review rejected the previous staging write before process
creation because it did not accept the older cross-chat references as trusted
exact production authorization. No publisher process, journal or asset call
started. This amended readable packet is for the existing release-chat approval
decision; preparation does not authorize execution or bypass that rejection.
