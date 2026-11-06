# ckSOL frontend publication revalidation — November 5, 2026

**Recommendation: reuse the accepted publication scope unchanged for Ansel's approval consolidation. No publication is authorized or performed.** There is no concrete asset, property, encoding, module, settings, interface or retained-artifact drift.

## Exact decision artifact

Reuse [the immutable plan at ba1ef31b](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/ba1ef31b/docs/operations/cksol-frontend-publication-plan-20261104.md), SHA-256 **`700a67b4664bed11ee4f9844a0d740dc80ac28a57f06f0f204464d81389ea821`**. The original file was not edited. Its statements about a pending backend question are historical status, superseded only by the completed installation evidence below; its proposed frontend mutations remain exact.

- Target: `tcfua-yaaaa-aaaap-qrd7q-cai`, mainnet `ic`; operator `rumi_identity`.
- Paired artifact source: integrated PR #624 merge `6847982b44f247e0c99f0a453b583d3fab8107a2`.
- Retained dist manifest SHA-256: `de790d03ea64e9e00d6600b9f2d415ab5135d5ed707c4a9f1a0e46b20b6931b2`; all **740 files** rehashed correctly. No rebuild or recompression.
- Index identity SHA-256: `e18a5300bb6363ec581a0d0c27c106fe37232b21abbae52446997a91c1ce591f`; paired index gzip `e3f9bba5d35dff8ec6455342d5a7799e9ffc16d8c12cbc503ca289121c461dc6`.

## Current read-only evidence

Fresh inventory window: November 5 **17:41:00–17:41:21 PST**. Two complete passes, each 28 pages through the query-only asset list API, matched each other and the accepted full baseline: **2,667 keys**, all properties (including mutable headers/environment cookie), and **2,861 encodings** (2,667 identity / 194 gzip). Server pagination remained capped at 100; actual returned counts were used, with an empty final page at offset 2667. Every hash/length/modified field and property matches the prior baseline; current timestamps in the read record are the only provenance difference. The complete unchanged baseline remains in the accepted plan's deduplicated property inventory; it is not duplicated as another megabyte of unchanged data.

Authenticated frontend status is Running with module **`04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`**, identical controllers/settings and identical asset Candid. The auth/domain files, CSP and known balance-fix provenance remain exactly as established by the original plan. [Fresh revalidation report and page receipts](evidence/cksol-frontend-revalidation-20261105/revalidation.json) plus status/terms evidence are pushed; raw full read is retained locally, SHA-256 `c37403ddcc414b0adc968b1a581325ccc9c94212a611595f09d899fa3fe28a47`.

Recomputed comparison is byte-identical to the accepted comparison, SHA-256 **`6425758d24d204e9bc8987a57811f5ecc3f83d1b331a0cb62e0a5b82be7333de`**. Thus the one exact proposed scope remains:

- **411 new immutable keys**; stage and verify only these absent keys if publication is explicitly approved.
- **Seven atomic mutable content replacements**: six identity contents (`/_app/version.json`, `/index.html`, and their literal `.br`/`.gz` siblings) plus the actual gzip encoding of `/index.html`.
- **418 content sets / 5,135,495 payload bytes** before chunk reuse; expected final **3,078 keys / 3,272 encodings**.
- **Zero deletes, clears, unsets or existing property changes**; all older keys/auth/settings retained. Preserve 58 compressed-only differences rather than performing optional updates. No new encoding/property policy is introduced.

## Backend prerequisite and remaining boundary

The prior conditional backend prerequisite is now met by [installation result 748ee1a4](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/748ee1a4/docs/operations/cksol-backend-installation-result-20261105.md) and refreshed read-only evidence: backend `tfesu-vyaaa-aaaap-qrd7a-cai` is **Running** on exact paired module **`d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`**, ICP reserve **0**, availability **false**, old relay budget **0**. This does not authorize frontend publication, funding, activation, canary or financial actions. Vault 158 preparation is complete under Bram's separate scope per the coordinator; it is not part of this frontend decision.

No build, batch, staging, upload/publication, wallet/funds operation, backend mutation or extra test campaign occurred. All execution remains held for the consolidated human decision. Re-read the relevant inventory immediately before any later authorized publication as already specified in the unchanged plan; no new optional test gate or duplicate approval question is introduced.
