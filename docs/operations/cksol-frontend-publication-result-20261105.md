# ckSOL frontend publication — November 5, 2026

**Publication completed; authoritative and served verification passed.** The exact paired artifact is live on `tcfua-yaaaa-aaaap-qrd7q-cai` and `app.rumiprotocol.com`. ICP reserve remains **0/unavailable**, relay budget **0**. No build, wallet approval, mint, canary, recovery execution, funding, activation or funds movement was performed.

## Authority and exact scope

Merritt independently verified Rob's direct message “i approve rumi frontend publication” in Ansel chat `01a1a4cf-9be2-7161-ba55-cc51212d60cb`, turn `01a1ae9c-b4cf-7880-9e08-9c7eba349d8f`, user message `01a1ae9c-b9a9-75c0-a248-4f570efb77f8`, and assigned exclusive ownership.

The approved [immutable plan](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/ba1ef31b4d2bf3faeca96213a00232aa7b526708/docs/operations/cksol-frontend-publication-plan-20261104.md) SHA-256 is `700a67b4664bed11ee4f9844a0d740dc80ac28a57f06f0f204464d81389ea821`. Revalidation `324f2fec` SHA-256 is `1ccecd0905abb4c924649627d5d94b93c4db51e6bfca4db09a79b22096208adb`.

- Target/network: `tcfua-yaaaa-aaaap-qrd7q-cai`, `ic`.
- Publisher: `rumi_identity`, principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
- Source: `6847982b44f247e0c99f0a453b583d3fab8107a2`.
- Retained manifest SHA-256: `de790d03ea64e9e00d6600b9f2d415ab5135d5ed707c4a9f1a0e46b20b6931b2`.
- Comparison SHA-256: `6425758d24d204e9bc8987a57811f5ecc3f83d1b331a0cb62e0a5b82be7333de`.

All 740 artifact files rehashed. The immediate complete double inventory matched all 2,667 baseline keys/properties/encodings. No source/dependency/workflow changes or recompression occurred.

## Execution, including reconciled rejection

Durable exclusive dispatch markers and original CLI responses remain in the protected local recovery store. Only `create_batch`, `create_chunks`, and `commit_batch` were allowed as write methods; no asset/property/permission deletion or canister install method was called.

1. The initial successful API-version query required an exact ArrayBuffer for the local Candid decoder. Its saved response was reconciled before any mutation; the query was not replayed.
2. Immutable batch **680** was created at **18:15:16 PST**. Its first commit at **18:15:34** was explicitly rejected with `chunk not found`: six IDs were shared between equal-content assets at distinct keys. Complete double-read reconciliation proved **all 2,667 keys, properties and encoding metadata unchanged**, including the entry point. No partial or ambiguous publication was assumed.
3. Six separate copies corrected only those duplicate chunk references in the same batch. A newly encoded commit—not a blind replay—succeeded at **18:19:14 PST**, with the unchanged 411 additions. Every committed content set used a distinct ID and exact approved bytes/properties. The successful correction is consistent with per-use chunk consumption on this deployed interface; it is not a universal asset-canister claim.
4. All 411 staged assets/properties, every old asset and **413 raw HTTPS reads** (411 additions plus two auth/domain files) passed before activation.
5. Entry batch **681** was created at **18:23:28 PST** and committed at **18:23:33**. Its **seven SetAssetContent operations were atomic**: six mutable identity contents (index/version and literal gzip/brotli siblings), plus the actual gzip encoding of `/index.html`. Distinct IDs were used for equal-content sets. There were no creates or property changes in this entry commit.

[Dispatches](evidence/cksol-frontend-publication-20261105/dispatches.json), [reconciliation](evidence/cksol-frontend-publication-20261105/stage-reconciliation.json), and [operation audit](evidence/cksol-frontend-publication-20261105/committed-operation-audit.json). All outcomes are reconciled; no owned mutation remains pending.

## Final authoritative and served verification

| Result | Verified value |
| --- | ---: |
| New immutable keys | 411 |
| Atomic mutable replacements | 7 |
| Content sets / payload bytes | 418 / 5,135,495 |
| Final keys / encodings | **3,078 / 3,272** |
| Older keys absent locally, retained | 2,341 |
| Deletes / clears / unsets / existing property changes | **0 / 0 / 0 / 0** |

The complete final double-read verifies every affected content hash and all retained keys, properties and untouched encodings, including modification metadata. Fifty-eight compressed-only differences remained untouched. Literal compressed keys and actual gzip encodings were preserved separately; uploader control files were excluded.

[Final inventory](evidence/cksol-frontend-publication-20261105/final-inventory.json), [stage verification](evidence/cksol-frontend-publication-20261105/stage-verification.json), and [entry verification](evidence/cksol-frontend-publication-20261105/activate-verification.json) distinguish publication from readback. All 411 additions and seven mutable encodings were verified as raw HTTPS response bytes on the canister domain, along with the unchanged auth/domain assets.

Primary custom-domain readback at **18:25:54 PST** independently returned HTTP 200 and exact bytes for identity/gzip root plus both auth/domain files:

- Index identity: `e18a5300bb6363ec581a0d0c27c106fe37232b21abbae52446997a91c1ce591f`, 1,822 bytes.
- Index gzip: `e3f9bba5d35dff8ec6455342d5a7799e9ffc16d8c12cbc503ca289121c461dc6`, 645 bytes.
- II alternative origins: `01d37f58e287d02a74d7767b81d8d214de14d14f062654d660b2b5da3aadd8fd`.
- IC domains: `d04261cb158bd5c519857f330ae347351cfbfe0ffbf2a2e1c3a7974065c08cf4`.

[Primary-domain results](evidence/cksol-frontend-publication-20261105/primary-domain-readback.json). This proves served bytes/auth assets, not a newly tested login or wallet financial lifecycle. The known balance fix remains in the exact source/artifact.

## Preserved state and remaining boundaries

Frontend is Running with unchanged Wasm **`04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`**, controllers, settings/environment variables and all existing headers—including CSP, environment cookie and CORS properties. [Status](evidence/cksol-frontend-publication-20261105/frontend-status.json).

The paired backend remains Running on **`d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`**. [Terms](evidence/cksol-frontend-publication-20261105/backend-terms.txt) confirm reserve_available_cycles=0 and available=false; relay budget=0. [Publication report](evidence/cksol-frontend-publication-20261105/publication-report.json).

No build, hosted CI, wallet approval, canary or financial/recovery call occurred. Publication does not activate/fund the reserve or prove an end-to-end paid mint/refund. Native client-money lifecycle/activation evidence remains separate. The assets-only window is complete; no additional authority is inferred. Future maintenance still follows the accepted stop/drain procedure; the callback root cause is not claimed fixed.
