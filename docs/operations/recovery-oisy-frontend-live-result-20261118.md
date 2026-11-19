# Combined frontend repair — live result, November 18, 2026

**Published and independently verified.** The live Stability Pool frontend now serves the OISY fresh/resume-deposit click fix together with the reviewed recovery-menu/manual-queue restoration. Actual customer wallet signing and deposit completion remain **unverified**; no agent initiated either. All existing financial and liquidation holds remain intact.

## Authority and exact executed release

Dorian independently read Rob's direct **get it fixed now** instruction, message `01a1f1d9-32c2-7643-8268-0c27219cc489`, after the combined frontend/publication scope was described in Merritt's chat. Dorian was sole publisher. Ellis independently verified stage, final bytes and UI reachability. No additional routine approval was requested; execution review accepted both exact phases.

The immutable [release packet](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/482a61e5631e6b16218bc2c636714528c3f0f3c0/docs/operations/recovery-oisy-frontend-publication-20261118.md) SHA256 is `d1145e7580b699f02f1df5bffa9ea427ae1f21ba0f91a57cc468908e342d1361`. Built source is `8e6f6f01671e615feb577d19400b22c41937a936`, frontend tree `763da242b7e26d1e96236fa7f7737f50d8d1ca64`. Only the deposit component and its regression spec differ from the previously reviewed recovery source. Reused proof includes 27 passing focused tests and the 51.379-second source-matched build; there was no extra build or general review campaign during publication.

- Artifact archive SHA256: `790306bbd2016646614f8c086004a6014c73bf9cd4e686836aaecc591ad72a37`.
- Complete 764-file manifest SHA256: `925290813075ca2149d41a963237643c3b6e5897068d6027ca5591f66866af75`.
- Execution pins SHA256: `2c8841cb670453414f5f9601d11445104fa2e364321be45c60ee19e6036824ce`.
- Dispatch plan SHA256: `6f0d7f9bf28a32e6a6447092634fb550ad61fb222146374897ca5983c7e7de68`.

Only existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai` on `ic` was mutated, using existing `rumi_identity`. Exactly 13 asset calls succeeded: two `create_batch`, nine `create_chunks`, two `commit_batch`. Batch **705** staged 327 immutable keys/436 content sets, committing **7:37:50 PM PST**. Independent stage proof was checked before batch **706** activated seven encodings across six existing entry keys, committing **7:45:55 PM PST**. There was no phase replay, uncertain call result, deletion, unset, property update, permission change or alternate executor.

## Independent live proof and preservation

[Machine receipt](evidence/recovery-oisy-frontend-live-20261118/live-receipt.json) records all 13 operator call argument hashes, sizes, times/results, preserved state and direct served-hash checks. Full independent proofs are retained as deterministic gzip files; hashes below identify their uncompressed JSON bytes.

| Proof | Actual verified state | SHA256 |
| --- | --- | --- |
| [Stage proof](evidence/recovery-oisy-frontend-live-20261118/verified-stage.json.gz) | 1,088 asset rows, 436 stored encodings, 331 HTTP keys; active index/version still old | `c389f914a91f2e35e50ed714bd1134432e2b680a46ab815b900e56ed3274e1fd` |
| [Final proof](evidence/recovery-oisy-frontend-live-20261118/verified-final.json.gz) | 1,088 asset rows, 443 stored encodings, 333 HTTP keys; final capture **7:47:45 PM PST** | `eb2849adef4117fbb557b2ade60e9e57b1e03ebfd340df309a750bcbcd0bcea2` |

Every historical key, property and untouched encoding/hash/size/modification time was preserved, including the 327 historical keys absent from the build. Discovery, Internet Identity and custom-domain assets are preserved. Frontend/backend remain Running with modules `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62` and `4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde`; certified Candid, controllers, settings and Commit/Prepare permissions match the baseline. The backend SP hold still reads **true**. This does not claim that the backend flag controls every SP rail; the separate pending resumption packet explains its coverage limitation. No backend, bot, reserve, Vault 202, claim retry, compensation, client transaction or liquidation-resumption call was made.

One read-only verifier defect was corrected during verification: existing `/.well-known/ii-alternative-origins` has an explicit preserved `Content-Type: application/json` override despite a base asset type of `application/octet-stream`. The original verifier ignored that header and stopped without producing a PASS. The [immutable correction](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/f1aa846bff171e0cd7968cd7af8026f6a6045e73/docs/operations/recovery-oisy-verifier-header-amendment-20261118.md), SHA256 `0216d4e4d4f0087afc2beeec6e26eb34d6f8636030717dc7dddfbd68acfd2458`, honors a unique explicit override and rejects duplicates. All original publication inputs, payloads, publisher and preservation checks stayed unchanged. No live MIME/property change, rebuild or re-upload was needed.

## Actual served/UI observations and limits

Dorian independently fetched the production index and fixed deposit chunk over the primary domain. Index SHA256 is `4dc3b76aa0e9727f15d85f3f896e76831707bfa179ad1d9be56c6654426743fd`; `/_app/immutable/chunks/IKGYMvoi.js` SHA256 is `719b9ce0310a60290421cbb0625c3f094bdee8c35a5c23b1a8fe6bfc46399772`. Both exactly match the built artifact. Both compiled deposit branches enter the existing signer channel before awaiting service work. Live version is `1795059056753`.

Ellis loaded the actual primary-domain Stability Pool page in an isolated browser at desktop 1440 and mobile 390 widths: the new app/root/deposit chunks load, the generic recovery card is absent, and the public SP page renders without horizontal overflow. The manual route loads 51 vaults with zero currently eligible. No live reserved row was observed and no wallet was connected: authenticated recovery-menu/reserved-row behavior retains the earlier source/mock proof, not a new live connected-customer demonstration. An observed mobile manual-route overflow (452 pixels at a 390-pixel viewport) is a separate follow-up outside the changed deposit component, not claimed fixed by this repair. Eight background HTTP400 responses for the native-XRP synthetic alias were also recorded outside the changed branches; no clean-console claim is made. [UI evidence and four screenshots](evidence/recovery-oisy-frontend-live-20261118/ellis-ui-checks.json) preserve these observations and limitations.

HTTP body hashes and certificate-header observations passed. Independent cryptographic HTTP-certificate validation is not claimed. The client must load the new page to use these bytes; actual wallet signing, ledger receipts and completed deposit remain outside this release verification. Earlier baseline-only packets/artifacts remain preserved and were not silently modified or executed.
