# Read-only verification amendment: preserved HTTP Content-Type override

The combined frontend release [packet482a61e5](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/482a61e5631e6b16218bc2c636714528c3f0f3c0/docs/operations/recovery-oisy-frontend-publication-20261118.md) staged successfully as batch705. **Activation remains held until actual independent stage PASS.** No stage replay, artifact rebuild or live property change is needed or authorized by this amendment.

The original stage verifier stopped at HTTP MIME for the existing protected file `/.well-known/ii-alternative-origins`: base asset `content_type` is `application/octet-stream`, but its preserved explicit custom header is **`Content-Type: application/json`**. HTTP correctly returns the override. Dorian independently confirmed this exact configuration in the pre-upload journal. Its identity content is242bytes/SHA256 `db093198ac4a3e48f3b01b069843bfbe5bf342b2cd7fc200b56b82ae936e92ff`. The verifier incorrectly used only the base field for its expected HTTP header. [Baseline and failure evidence](evidence/recovery-oisy-frontend-publication-20261118/verification-header-amendment/correction-evidence.json).

## Exact correction

A separate [read-only verifier copy](evidence/recovery-oisy-frontend-publication-20261118/verification-header-amendment/verify-dispatch.mjs) uses the unique case-insensitive explicit Content-Type header when present, falling back to the base asset type. Duplicate overrides reject. The HTTP response is still compared exactly after the same media-type normalization. Wrong MIME still fails.

Apart from path relocation/imports and this effective-MIME expression, the original verifier code is unchanged. **All payload hashes/lengths, full inventory, properties, untouched encodings/timestamps, module/settings/controllers/permissions/pause, second stable capture and HTTP/stored-byte checks remain exact.** Five tiny helper checks pass: explicit override, case-insensitive name, base fallback, duplicate refusal, wrong type remains unequal. No optional campaign, dependencies or product change.

The original482a packet, all dispatch files/pins, publisher, artifact and upload payload stay byte-identical. A code-hash launcher pins the new local read-only files and requires the unchanged original execution pins before importing the corrected verifier.

- New [verification pins](evidence/recovery-oisy-frontend-publication-20261118/verification-header-amendment/verification-pins.json) SHA256: **`557318eafe31f65a28e1b8de87b9c9688ac77386dff3b57d27276e15e42be66e`**.
- Corrected verifier SHA256: `c5a1298e9b82db49518523e313e3a02da98a7a9e951386bbcd0613cb75d2e184`.
- Original execution pins remain `2c8841cb670453414f5f9601d11445104fa2e364321be45c60ee19e6036824ce`.
- **Unchanged dispatch plan:** `6f0d7f9bf28a32e6a6447092634fb550ad61fb222146374897ca5983c7e7de68`.
- Artifact manifest remains `925290813075ca2149d41a963237643c3b6e5897068d6027ca5591f66866af75`; archive remains `790306bbd2016646614f8c086004a6014c73bf9cd4e686836aaecc591ad72a37`.

Use the new pinned launcher for both independent phases:

```sh
export PUBLISH_PINS_SHA256=2c8841cb670453414f5f9601d11445104fa2e364321be45c60ee19e6036824ce
export VERIFICATION_PINS_SHA256=557318eafe31f65a28e1b8de87b9c9688ac77386dff3b57d27276e15e42be66e
node docs/operations/evidence/recovery-oisy-frontend-publication-20261118/verification-header-amendment/run-verification.mjs stage OUTPUT
```

Activation still requires an actual PASS proof for the same immutable plan, with its actual file hash supplied to the unchanged publisher. Failed checks do not create a PASS proof. The new check changes no canister state or permission. Dorian remains sole publisher under the current independently verified human direction; no new product decision is required for correcting this read-only expectation. No automatic-review rejection is bypassed, and no financial/SP/liquidation/worker/reserve/claim action is included.
