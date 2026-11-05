# ckSOL paired frontend — read-only publication plan

**For Merritt review only.** The backend installation question is already pending with Rob in Merritt. This preparation neither duplicates that question nor consumes its authority. No build, batch creation, chunk upload, asset mutation, deletion, canister installation, reserve action, wallet action or customer message occurred.

## Exact composition and live baseline

Use the existing paired frontend artifact from source **`6847982b44f247e0c99f0a453b583d3fab8107a2`**; do not rebuild. Its [full local manifest](evidence/cksol-integrated-6847982b/frontend-manifest.json) has 740 files, SHA-256 `de790d03ea64e9e00d6600b9f2d415ab5135d5ed707c4a9f1a0e46b20b6931b2`. All 740 actual local file hashes were reverified against that manifest before comparison. Backend pairing is described in the [installation-only packet](cksol-installation-only-packet-20261104.md).

Target **`tcfua-yaaaa-aaaap-qrd7q-cai`**, network `ic`, owner/operator `rumi_identity`, principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`. Authenticated frontend status is Running; module `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`; retain every controller and setting in [status baseline](evidence/cksol-frontend-plan-20261104/frontend-status.json). Publication, if later approved, is assets-only and must preserve that module.

The deployed [asset Candid](evidence/cksol-frontend-plan-20261104/frontend-live.did) exposes `list(record {start:opt nat;length:opt nat}) query`, returning keys, content types, every encoding's hash/length/modified time, max_age, headers, raw-access and alias properties. Public query reads were made from `https://icp-api.io`, November 4 **20:27:18–20:27:41 PST**. The server returned at most 100 despite requested length=250. Offset advanced by the actual returned count, not requested length; each pass reached a final empty page at offset 2667. Two complete 28-page reads matched exactly, including properties and encoding metadata.

Complete baseline: **2,667 keys**, **2,861 encodings** (2,667 identity + 194 gzip), all with SHA-256 metadata. There are **1,760 literal `.gz`/`.br` keys** in addition to actual gzip encodings. Preserve both categories. [Complete deduplicated property inventory](evidence/cksol-frontend-plan-20261104/frontend-live-inventory-compact.json) records every key and encoding against all 13 exact property sets, without dropping header values or options. SHA-256 **`5109dec6e72ddb3a48d48e8f1901a0f79ab00025becbf50a3cf165cad7bf0f38`**. The unnormalized double-read record remains local with SHA-256 `a8bdc15bcd7a37d615d1bab025dbaa0d96290ac9c40b060c3fcdcc8635b71510`.

## Proposed diff

[Per-key comparison and proposed content hashes](evidence/cksol-frontend-plan-20261104/frontend-comparison.json), SHA-256 **`6425758d24d204e9bc8987a57811f5ecc3f83d1b331a0cb62e0a5b82be7333de`**:

| Change | Count |
| --- | ---: |
| New immutable keys | 411 |
| Changed existing identity contents | 6 |
| Changed existing gzip encodings | 1 |
| Existing identity contents already identical | 320 |
| Old live keys absent from local build, retained untouched | 2,341 |
| Proposed content-setting operations | 418 |
| Proposed payload bytes (before any chunk reuse) | 5,135,495 |
| Expected total keys / encodings after approved publication | 3,078 / 3,272 |
| Existing property changes / deletes / clears / unsets | **0 / 0 / 0 / 0** |

All 411 new keys are under `/_app/immutable/`. They include the paired build's literal gzip/brotli siblings, preserving the existing storage convention. Each proposed creation names an exact existing template key with the same immutable path class and extension, copying its content type, headers, max_age, alias and raw-access options. There is one property variant per selected class; no new property-policy decision is implied.

The six mutable identity keys are `/_app/version.json`, its literal `.br` and `.gz` siblings, `/index.html`, and its literal `.br` and `.gz` siblings. The seventh content replacement is the **actual gzip encoding of `/index.html`**. All use existing artifact bytes, not newly recompressed data. Candidate index identity SHA-256 `e18a5300bb6363ec581a0d0c27c106fe37232b21abbae52446997a91c1ce591f`; actual index gzip SHA-256 `e3f9bba5d35dff8ec6455342d5a7799e9ffc16d8c12cbc503ca289121c461dc6`. Its baseline identity is `571b5516eb9ef9626142495b6a0110079c1e34a23426311a86b8aaaee6037a71`; baseline gzip `a3eccb2a8f20c60797cbbe5f1b064396cc79fac19be4e463294d78de9d812908`.

Fifty-eight existing gzip encodings differ only in compressed representation while their identity contents already match the artifact. Retain those live bytes; changing them is optional and excluded from this proposal. Keep all older hashed resources and all old keys absent from dist. Exclude `.ic-assets.json`, `.ic-assets.json.br`, and `.ic-assets.json.gz` as uploader control files, not new assets. They are not deletion targets. No new `br` encoding is proposed; existing actual encoding names remain intact.

## Authentication and known balance behavior

[Exact auth/balance checks](evidence/cksol-frontend-plan-20261104/auth-balance-checks.json) establish:

- `/.well-known/ii-alternative-origins` and `/.well-known/ic-domains` candidate contents match their live identity hashes exactly. Preserve their existing properties and all existing auth/domain files.
- Local control-file CSP equals the existing index CSP. Preserve all existing headers verbatim, including the frontend environment `Set-Cookie`, security headers and CORS behavior; do not replace them with a reduced local header set. No property reset or frontend Wasm installation is proposed.
- Both `src/vault_frontend/src/lib/components/swap/LiquidityInterface.svelte` and its deposit-policy spec byte-match balance-fix source `45af74a433c169c08e52bc1ad2bda23e1df215a9` (PR #605). The artifact is built from the integrated source containing these bytes; no balance-fix cherry-pick or rebuild is needed. This is source/artifact provenance, not a newly exercised connected-wallet balance flow.
- All 13 resources directly referenced by the candidate index exist in the paired manifest.

## Conditional publication sequence — no action authorized

1. Re-read the complete live inventory immediately before a separately authorized publication and compare it to this baseline. If another release changed mutable assets/properties, reconcile that concrete composition before replacing the entry point. Do not blindly overwrite another publisher or drop keys. Preserve ownership and the full frontend module/settings baseline.
2. If separately approved, stage and verify only the 411 absent immutable keys, with exact template properties and candidate contents. These hashed assets are inert until referenced by an entry point; staging could occur before backend installation if explicitly authorized. No stage or batch exists from this preparation.
3. Activate the seven mutable content replacements together only after the separately approved backend installation is known and read back as the paired candidate: backend gzip `d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`, live compatible Candid including the ICP-payment/recovery methods, preserved state/settings, and **reserve_available_cycles=0, available=false**, old relay budget=0. This is the proposed ordering for exposing the integrated UX, not a claim that all pre-install publication would corrupt state. It avoids offering a new backend-dependent option before its API is present. The API dependency is concrete: `cksolIcpPayment.ts:212–214` requires `get_cksol_icp_terms`; `:346–351` rejects unavailable/insufficient reserve **before** wallet approval actor/signer setup.
4. A future approved publication should use the existing live asset interface and preserve all existing keys/properties/encodings. Commit the six mutable identity contents and index gzip atomically after immutable content verification; do not permit an old gzip index to keep serving the prior entry. Preserve request/batch IDs and results; if a commit result is uncertain, read back authoritative content state before another commit. No blind retry, clear, unset or delete.
5. Read back the exact affected encoding hashes, all retained keys/properties and the unchanged frontend module/settings; check served index/entry resources and auth/domain assets against the paired manifest. These are direct publication-verification steps, not an optional broad test campaign. No wallet or financial action is needed to verify publication.

The reserve remains off throughout backend installation and frontend publication. Publishing this frontend does **not** enable ICP-sponsored minting: the source reads fresh terms and refuses that path while unavailable, with guidance that an existing SOL deposit does not require another transfer. The old cycles-funded path and receipt/recovery surfaces remain in the paired source. End-to-end reserve use, client-money lifecycle checks, reserve funding/activation and any canary require their separate scope and authority; they are not inferred from this read-only plan.

Return this proposal to Merritt. The backend approval question remains pending in its original location. No additional approval question, hosted CI, optional test gate, build or live mutation was introduced here.
