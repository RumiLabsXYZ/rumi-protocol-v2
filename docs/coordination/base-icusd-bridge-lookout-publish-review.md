# Rumi Base bridge: exact sanitized Lookout upload review

Status: **HELD. No upload executed.** Automatic approval review rejected the existing publisher
command because authorization for this exact internal metadata payload and private destination
could not be verified. Do not retry indirectly or use a different executor.

## Destination and operation

Upload the files below through the existing `rumi-bridge.mainnet.json` scoped manifest and
`lookout-rumi-bridge-publisher` identity to the existing private Lookout project
`rumi-base-bridge` (Rumi · Base bridge), displayed at
https://uomsq-giaaa-aaaah-qvceq-cai.icp0.io/. The configured existing backend is
`uhpzm-qaaaa-aaaah-qvcfa-cai` on ICP. No new grants or access changes.

The operation is the existing publisher's changed-only publication, then a read-only no-op
verification. The project inputs remain in the private Rumi repository; nothing is added to
Lookout's public GitHub repository.

## Exact readable inputs

These files are pushed on `codex/base-icusd-bridge` at commit `3fc6c1c6`.
The source revision accepted by the publisher dry-run is
`89b8797df65d655c76ce3131b88d7d74e2fdd459d95fab6b4b9105f580c931f7`.

- [docs/coordination/base-icusd-bridge-lookout.json](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/3fc6c1c6/docs/coordination/base-icusd-bridge-lookout.json)
  SHA-256: `9319bdb6a9364ddd79a898af1364f1973f1f401faa41b8be20d539b846dbcdf8`
- [docs/coordination/base-icusd-bridge-presentation.json](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/3fc6c1c6/docs/coordination/base-icusd-bridge-presentation.json)
  SHA-256: `b52a3d5cf1edd5dcab5844a3faee642829680e8c632e9c0ebb7ce921384fe11a`
- [docs/coordination/base-icusd-bridge-priorities.json](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/3fc6c1c6/docs/coordination/base-icusd-bridge-priorities.json)
  SHA-256: `d725b7d3f00c5009c8d15f2cdb24bd9f2cffc4f5a893f616d019c1ff20c9ecc0`

## What the payload says

There are 35 sanitized task cards. Only the browser-test card's evidence changes: earlier real
staging sign-in is verified, renewed sign-in awaits Touch ID, Base wallet connection is pending,
no browser transfer occurred, and the synthetic test bridge is stopped. The technical status and
new three-sentence plain-English status report those same limits. The 113 glossary entries are
unchanged. Explicit priority memberships previously prepared for badges remain as reviewed;
34 of 35 cards are linked, and the shared baseline card is deliberately unlinked.

The upload contains task metadata, owner roles, scoped proof and public review links. It excludes
credentials, keys, wallet addresses, balances, transaction/provider payloads, private logs and
local filesystem paths. This is internal project progress information made visible to the
existing authorized private Lookout audience; that disclosure is the action needing approval.
It does not merge, deploy or activate the bridge, sign/broadcast transactions, fund anything,
or prove browser settlement or production readiness.

## Requested exact approval

Approve uploading these three fingerprinted sanitized inputs to that existing private Lookout
project using its existing scoped publisher, followed by read-only verification. This approval
would resolve this exact metadata-upload hold only; every bridge, credential, production and
merge boundary remains unchanged.
