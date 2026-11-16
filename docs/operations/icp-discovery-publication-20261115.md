# Rumi ICP connector discovery publication decision

Status: prepared scope; no discovery manifest created/generated/published, no live write.
Canonical base: 9b7669e799d4e6e6cc6d52174c06e87e3569994c.

## Operator decision required

The current authoritative ICP MCP App Operator Terms are version 1.1, effective September 2, 2026:
https://internetcomputer.org/icp-mcp/app-operator-terms/
Publishing /.well-known/ic-architecture constitutes acceptance for Rumi's operator.
The operator must confirm authority to bind Rumi and acceptance before manifest creation or publication.
Participation permits public metadata retrieval, indexing/caching/listing, queries and user-authorized
state-changing calls signed with those users' application-specific Internet Identity identities.
The manifest is a canister declaration, not a method allowlist or read-only restriction.
Existing canister authorization remains the security boundary. Registration is optional; no email is proposed.
Removal disables new connector access once observed; contract termination is separate notice to DFINITY.

## Smallest implementation after acceptance

Generate extensionless /.well-known/ic-architecture with schema version 1.0.0 at release time
from canonical canister_ids.json, selecting network explicitly and failing on absent IDs.
Declare these deliberately selected Rumi-operated application services only:

| Registry name | Mainnet ID verified in canonical configuration | Role |
| --- | --- | --- |
| vault_frontend | tcfua-yaaaa-aaaap-qrd7q-cai | Rumi application frontend |
| rumi_protocol_backend | tfesu-vyaaa-aaaap-qrd7a-cai | Collateral vaults and protocol accounting |
| rumi_stability_pool | tmhzi-dqaaa-aaaap-qrd6q-cai | Stability Pool deposits, gains and liquidation accounting |
| rumi_analytics | dtlu2-uqaaa-aaaap-qugcq-cai | Protocol analytics and historical metrics |
| rumi_3pool | fohh4-yyaaa-aaaap-qtkpa-cai | StableSwap pool and internal 3USD LP token ledger |
| icusd_ledger | t6bor-paaaa-aaaap-qrd5q-cai | Rumi icUSD token ledger |
| icusd_index | 6niqu-siaaa-aaaap-qrjeq-cai | Rumi icUSD transaction index |

Do not derive declarations by scraping every principal from the bundle. No external ledgers,
minters or DEXes are included. No treasury, bot or controller surface is added to this initial scope.

Also generate /.well-known/ii-derivation-origin as text/plain containing the existing frontend
canister origin https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io, not the custom app domain.
Canonical auth.ts lines 405-407 already pin this derivation origin; publishing the declaration
must not change login configuration, principal derivation, or alternative-origins.

Integrate generation into the existing frontend build/release path, preserving the existing
postbuild Internet Identity/domain verification. Generated IDs must not be a hard-coded static
manifest. Extend existing .ic-assets.json rules for explicit application/json and text/plain
content types at the two exact paths. Existing .well-known ignore:false remains.

## Publication and verification

1. Reconcile current canonical frontend and SEC20/SEC21 restoration with existing owner before any upload.
2. Prepare only the two discovery assets; no broad UI build is needed for this static-only live change.
3. Capture current complete asset inventory, content encodings/hashes/properties, frontend module,
   settings and upload permissions. Freshly reconcile immediately before committing.
4. Through the existing asset batch publication path, create/set only the two exact keys. No delete,
   clear, unset, unrelated key/property change, asset-canister upgrade, controller or permission change.
   Persist dispatch/result evidence; stop and reconcile any unknown outcome before retry.
5. Independently compare the entire post-publication inventory: every prior asset/encoding/property
   unchanged, only the two intended discovery additions. Read back exact body/hash/content-type from
   certified app origin; verify neither endpoint returns the HTML fallback.
6. Run connector open_app, confirm all selected services declared and the existing correct derivation
   origin resolved; fetch Candid for Stability Pool and backend; run one bounded anonymous query
   (for example get_protocol_status if current Candid marks it query). No identity derivation or signed
   financial call as a canary. Report missing Candid/query availability honestly.

Preserve writedown_disabled=true Stability Pool liquidation hold. No backend/frontend Wasm upgrade,
financial action, controller/settings change, auth change or worker resumption is authorized here.
Vault202 packet 7d0d9b69 remains a separate approval decision.

## Observations and current blockers

Connector open_app currently finds only undeclared bundle/header candidates and defaults to the
custom app origin because the derivation declaration is absent. Tamsin reported Candid discovery
rejected before contacting Stability Pool because the architecture endpoint returns text/html.
Canonical source confirms the effective pinned II origin above.

The official service-discoverability guidance requires operator acceptance before creating,
generating or publishing the architecture manifest. Until that decision, no discovery asset is created.
A coordination message was separately rejected by automatic approval review for unrecognized
destination authorization. It remains unsent; do not reroute it through another chat.

No optional audit, OQL implementation, API-doc backend method, full UI rebuild, financial canary
or repeat build/test campaign is required for this bounded static discovery change.
