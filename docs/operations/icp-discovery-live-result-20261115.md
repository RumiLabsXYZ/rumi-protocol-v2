# Rumi ICP discovery publication result

Published and verified through the ICP connector under Rob’s direct operator acceptance of Terms v1.1.
Exact source: 7ca180793fa58beecba22ea98f4bc46507754854. Asset batch: **704**.

- https://app.rumiprotocol.com/.well-known/ic-architecture — HTTP200 application/json;
  1,048 bytes; SHA256 `22bcdf6a5f1ad8e4c1b241a46e3a7f1ae5bec0ab16a79e62ce215288e862f702`.
- https://app.rumiprotocol.com/.well-known/ii-derivation-origin — HTTP200 text/plain;
  44 bytes; SHA256 `16cf6b5a8eb787b3222ba58308cd6234755b20be869337e1e11e42b51eee53a0`.

Seven deliberate Rumi-operated services are declared. No external services, bot, treasury, permission
or method-allowlist declaration was added. The manifest does not itself restrict methods to read-only.

Actual connector open_app discovers all seven as declared. Narrowed live Candid retrieval succeeded
for backend get_sp_writedown_disabled and Stability Pool get_pool_event_count. Actual anonymous
connector queries returned `(true)` and `(2_684 : nat64)` respectively. Full interfaces exceeded the
connector’s 5,120-byte reply cap; narrowing the requested methods succeeded without changing a canister.
API-doc/OQL surfaces remain absent and were not required for these Candid calls.

The published identity declaration matches the existing frontend auth mapping exactly. The connector
normalizes its returned canister URL from icp0.io to ic0.app. No auth configuration was changed, no
identity/signature was requested, and authenticated principal equivalence was not tested.

Full before/after double-read inventory: **759 to 761 assets**, exactly the two intended additions.
Every prior asset, encoding hash/size/modified value and property is unchanged. Frontend and backend
modules/settings and Prepare/Commit permission lists are unchanged. The Stability Pool admission
pause reads true before, after, and through the connector. Vault202 and every other financial,
installation, controller and signing hold remain separate. No backend upgrade, financial transaction,
worker resumption, permission change or broad frontend publication occurred.

HTTPS with ordinary TLS verification returned exact bytes/content types and the gateway certificate
header. No independent cryptographic HTTP-certificate validation is claimed. Local Python trust-store
verification failed before reading any body; the subsequent Node TLS read succeeded without disabling TLS.

Source generation passed against the frozen artifacts and removes discovery output for unapproved
local/staging networks. The existing auth/domain verifier remains first in postbuild. No full UI build
or paid CI was needed or run for this static-only publication. Ellis independently carried the three
runtime paths byte-identically into the restoration branch at **1b42e9048077dbf0b34505cf4a12e0f97a2311e1**,
verified generated-byte equality and local removal, and owns its normal canonical PR/required checks/merge.
That main merge is pending; no canonical-main-merged claim is made here. Future UI publication must
retain both new keys and the generator integration.

[Terminal receipt](evidence/icp-discovery-20261115/terminal-receipt.json) links actual connector replies,
full asset inventories, dispatch arguments/results, hold/settings/permission readbacks and served responses.
Shared worktree inventory/cleanup: zero eligible removals, all protected/dirty/unmerged worktrees preserved.
