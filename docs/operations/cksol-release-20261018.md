# ckSOL release receipt — 2026-10-18

User authorization: deploy and activate ckSOL with ckETH risk parameters and the official chain-key Solana minter integration.

## Verified source and artifacts

PR [#585](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/585) is merged. Reviewed source `cc224b3edf0f098779cb87d1c09bb245affd8c55` and merge `ed99a4d787d6836e03d7a87ff94c47893713a1ff` have identical trees. CI run `39108440694` passed. Retained evidence supports 82 frontend tests, 10 backend tests, Candid source compatibility, production frontend build and authentication-asset verification. Full frontend typechecking has the same 30 pre-existing errors as the pre-feature baseline. Two independent source reviewers and two release-plan reviewers passed; an additional independent pass verified artifact and backup hashes.

Fresh canonical backend release build passed. Public Candid metadata, shrink with names retained, and deterministic gzip produced:

| Artifact | SHA-256 | Bytes |
| --- | --- | ---: |
| Uploaded gzip | `3b51aca404bb8da8994b55273114571a33486b99c3275547c809746254128566` | 3,069,494 |
| Decompressed Wasm | `fd5c8ebc3916767784887ba55b977bb8bbb76bb407e4d904951025ec3beceb3b` | 12,912,783 |
| Exact source archive | `c3aeb677dc9963f36bc8d1366dacd6cd0332be485809e699572aa38d9aafe699` | — |

Executable code is 12,253,978 bytes. Under the current 12,582,912-byte network code limit, this preserves the 262,144-byte project reserve and leaves another 66,790 bytes. Embedded Candid equals source; the new interface is a subtype of freshly retrieved deployed Candid. The old frozen-artifact verifier pins another release and was not used to validate these hashes.

## Published frontend

Assets-only sync completed and reported 725 assets. The installed frontend Wasm remains `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`.

Exact served-byte comparisons passed for the custom-domain index, [/solana](https://app.rumiprotocol.com/solana), the new route chunk, minter-service chunk and certified gateway index. Index SHA-256 is `0fbeae31c3b67b591df023008a54c67060966ee9d67ee507d67ecfbe0e9a5677`. A live signed-out browser confirmed the page and disabled deposit address while backend processing is unavailable.

## Backend request and remaining activation

Backend target: `tfesu-vyaaa-aaaap-qrd7a-cai`, mainnet, named identity `rumi_identity`. The identity principal was verified as an existing controller. Upgrade arguments preserve mode and record the PR585 description.

A fresh pre-upgrade snapshot was created while stopped, downloaded, and every payload length/hash verified. Its saved Wasm matches the observed old module. Before reusing an occupied snapshot slot, the oldest snapshot was also downloaded completely and verified. Both backups and exact release artifacts remain preserved locally. Snapshot restoration after intervening traffic is not authorized by this receipt.

Exactly one live backend upgrade command was submitted in this release attempt. Automatic approval review rejected it, claiming an identical production upgrade is already running. The rejected command did not create its execution log. No retry, alternate executor or indirect upgrade was attempted. The alleged other request has not been identified or given an authoritative terminal disposition.

A bounded start restored backend availability after the snapshot. Post-rejection readback confirms Running on old module `e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352`; ckSOL configuration is absent. These observations do not prove cancellation of another request. Registration and sponsorship setters have not been called.

Preserve the existing 22,000 icUSD global cap, disabled reserve redemptions and paused Stability Pool. Do not replay or clear pending obligations as part of this release.

The duplicate-request claim was investigated as described below. Two independent reviewers approved exactly one fresh request through the same automatic approval path after current preflight and backup. If accepted, remaining steps are exact installed-hash/Candid/state verification, ckSOL registration copying live ckETH settings, SP registration readback and a fresh SOL/USD price. The funding requirement was resolved on October 18 at 18:17 UTC. After the user funded the verified deployment identity with 10T cycles, one authorized 6T top-up completed. Live readback confirmed 11,773,772,508,862 actual backend cycles, above the 11T dispatch threshold, and the unchanged previous backend module. No upgrade retry, collateral registration or relay-budget setter was sent. The proposed finite 10T sponsorship allocation remains unset pending the backend upgrade; allocation and actual cycles remain distinct.

This receipt proves frontend publication and release preparation. It does not prove backend deployment, collateral activation, a connected-wallet mint/redeem, or settlement.


## Duplicate-request investigation and bounded reconsideration

The only actual backend upgrade submission identified in this chat is `call_d8eb4e7684894126a9bc97910385ba92`, submitted at 2026-10-18 08:03:23.875 UTC. Its running wrapper was cell 149. `call_50fc61606c0344b6a5ea85aa9a67fd79` at 08:13:19.335 UTC was a wait on that cell, not a second install. It returned terminal `CreateProcess ... Rejected` at 08:13:19.458 UTC. No shell execution log or install command-execution item was created. The deployment turn is completed.

The rejection said that an identical production upgrade request was already running and prohibited alternate or indirect execution. It also explicitly allowed additional checks to prove authorization or low risk before trying again. Earlier matching calls in this chat were help invocations, help cancellation, or read-only diagnostics. None submitted another backend upgrade.

Two independent audits inspected structured tool calls in Rumi session records: one covered 75 rollouts created October 16–18, and the other 59 rollouts created October 17–18 plus relevant older sessions with recent activity. Neither identified an earlier actual backend upgrade before the rejected submission. Bounded desktop command/history and review-feedback reads likewise identified no matching pending install or original competing approval event. No internal approval ID was recovered. These are bounded negative findings; they do not establish global absence or cancel an unidentified request. A self-match in the rejection is a hypothesis, not an established cause.

The supported approval-reconsideration API requires an original serialized guardian event, which was not available. No event was fabricated, no approval policy or database was changed, and no alternative executor was used. Both Charles and Dana independently concluded that exactly one fresh submission of the same frozen artifact, command and target through the same automatic approval reviewer is supported reconsideration on materially new evidence. A further rejection must be honored without another retry loop.

Fresh preflight at 23:05 UTC confirms the old backend still Running, ckSOL absent, the same controllers, cap 2,200,000,000,000 raw, reserve redemptions disabled, Stability Pool emergency-paused, mode GeneralAvailability and manual override false. Actual backend cycles were 11,645,765,573,789 at the subsequent status read. The old deployment lacks the new Treasury stable-transfer inspection method; that query's failure is unavailable visibility, not proof of an empty queue. Frozen artifact hashes and fresh Candid compatibility were reverified. Fresh snapshot `00000000000000110000000001f088fe0101` was taken at 23:08:46 UTC after completely archiving and verifying the oldest occupied snapshot2. The backend was immediately restarted on the old module. All fresh snapshot payload lengths and hashes were verified after download; saved Wasm equals the observed old module. The single reconsideration has not yet been submitted.
