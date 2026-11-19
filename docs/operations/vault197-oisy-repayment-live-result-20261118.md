# Vault 197 repayment frontend repair — live result

**Published and independently verified on November 18, 2026. The client can refresh the app and retry the intended Vault 197 repayment.** Actual customer consent, ledger payment and completed repayment remain unverified. No agent connected a wallet, signed, pressed a financial action or retried a repayment. All existing financial and liquidation holds remain intact.

## Exact authority and executed artifact

Dorian independently read Rob's direct approval `01a1f29d-437c-7803-ae51-99c5c6259320`: **Approved. Have Dorian publish the repayment fix under packet 8151703f now and have Ellis verify it. Let me know when she can retry repaying vault 197.** Dorian was sole publisher; Ellis independently verified stage, final bytes and actual public UI loading.

The executed [packet at 8151703f257c1336dddb3ac9aca2cc0c496ef1e4](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/8151703f257c1336dddb3ac9aca2cc0c496ef1e4/docs/operations/vault197-oisy-repayment-publication-20261118.md), SHA256 `f5f071758d3ebb6e31f174cdec84867895bced9af12ea5b26dc34e567651fa18`, binds source `3294c9561664d2ec48070ac0bab50862913f22dc`, full tree `4d04bbcaafc1a13ac5f3fe4931ee9913acfe45f0`. Its seven frontend source/test changes compose the deployed deposit/recovery fixes. Existing source proof is 44 passing focused tests, two fresh final independent reviews and one 54.580-second source-matched build. No additional build, artifact substitution or general review campaign occurred during publication. No new canonical main merge or hosted CI result is claimed.

- Artifact archive SHA256: `3915e35932d1b47df5a8a8345c61c1416fc4a26fec4ea3642a8a54913a4ac82e`.
- Complete 764-file manifest SHA256: `c0e20f5654625c32ca442e1be7dc49119d5cf9de0622be85740440e0a0bae6ae`.
- Execution pins SHA256: `a76995fdbf35c441f93f63a25d1eef021787b4b07860b85ba249e7e2ac73a6dc`.
- Dispatch plan SHA256: `36cc96e745d8710b6ccf586c17a806139bb59157bbfc10c3b2b3374a7c8ee123`.
- Corrected MIME verification pins SHA256: `1605c9471177b2ac76adf34530f584a8210deb5b228e56ea79d5789f5f38b0b1`.

Only existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai` on `ic` was mutated, using existing `rumi_identity`. Exactly **12 successful asset calls** were made: two `create_batch`, eight `create_chunks`, two `commit_batch`. Batch **707** staged **267 new immutable keys / 356 encodings**, committing **11:05:23 PM PST**. Actual independent stage PASS was checked before batch **708** activated **seven encodings across six existing entry keys**, committing **11:10:15 PM PST**. There was no phase replay, uncertain call outcome, asset deletion, property/permission change, backend or financial call.

## Exact readbacks and preservation

The [machine receipt](evidence/vault197-oisy-repayment-live-20261118/live-receipt.json) retains operator call argument hashes, sizes, times/results, preserved state and independent direct served-hash checks. Complete independent proofs are stored as deterministic gzip files; the following hashes identify their uncompressed JSON bytes.

| Proof | Actual verified state | SHA256 |
| --- | --- | --- |
| [Stage](evidence/vault197-oisy-repayment-live-20261118/verified-stage.json.gz) | 1,355 asset rows, 356 planned stored encodings, 271 HTTP keys; active index/version still old | `4dc58f3df525c98f413553be8f72da35506097514d81894525dcb0efa27e7369` |
| [Final](evidence/vault197-oisy-repayment-live-20261118/verified-final.json.gz) | 1,355 asset rows, 363 planned stored encodings, 273 HTTP keys; verified **11:12:02 PM PST** | `c81c089be6c52ca0199bdb7f1f683ec481dd1080a465edd76b8ef5255c242da7` |

All **1,088 historical keys**, properties and untouched encodings/hash/size/modification times were preserved, including 594 historical keys absent from the new build. Discovery, Internet Identity and custom-domain assets are unchanged. Frontend/backend remain Running with modules `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62` and `4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde`; certified Candid, controllers, settings and Commit/Prepare permissions match the baseline. The backend SP hold still reads **true**. This flag is not described as covering every SP rail; its separate pending resumption decision remains held. No reserve, bot/worker, Vault 202, claim retry, backend installation or liquidation resumption occurred.

Preparation caught two stale hash literals in the materializer before extraction or a live write. The final approved packet corrected them, refreshed dependent pins, and passed actual extraction of all 764 files plus inspection against that extracted copy. The artifact and dispatch plan never changed. Earlier immutable packet versions remain preserved. The existing explicit Content-Type override correction was retained in actual stage/final verification.

## Actual served/UI observations and retry boundary

Dorian independently fetched the public index and repayment signer chunk. Index SHA256 is `86dc697e74e73a07cbf6eab275bdcb5d3051c186d8b4b76bc6f7dc75989dd7f1`; `/_app/immutable/chunks/Bv_H2GCB.js` SHA256 is `26fddf5c54e2107835e27f0a4bbeea8e6fb36208fa195576f50b74ce352f254b`. Both exactly match the approved artifact.

Ellis's actual isolated browser at the primary-domain `/vaults` page loads version **`1795064489138`** and the new handler graph: `Bv_H2GCB.js` above plus `CFyzoWdq.js`, SHA256 `b7be781b580f385c738f1e538e5550e8938e85111045a7895f0a93d3a7b84973`. Desktop 1440 and mobile 390 render without horizontal overflow. [Independent live receipt and UI evidence](evidence/vault197-oisy-repayment-live-20261118/independent-live-receipt.json) include actual desktop/mobile browser results and screenshot hashes. No wallet was connected or financial action clicked. Known native-XRP alias background HTTP400s remain a deferred observation; no clean-console claim is made.

The served behavior uses one uniquely named, action-owned signer/agent for approval and repayment, preserving captured caller/session, amount/token routing, approval expiry, existing backend methods and unknown-outcome handling. Source/SDK regressions cover the click and pending-handshake cleanup; these are not a real customer consent test. HTTP body/hash and certificate-header observations passed; independent cryptographic HTTP-certificate validation is not claimed.

**Retry-ready means the approved repair is served and the client can refresh the same-wallet app and retry the intended 149.99 icUSD repayment on Vault 197. It does not mean a repayment has already succeeded.** Actual customer consent and settlement need their own observation; no automatic retry or bypass of pending/uncertain-state handling is added.
