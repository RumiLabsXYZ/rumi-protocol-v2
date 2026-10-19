# ckSOL release receipt — 2026-10-18

Status: backend and ckSOL collateral remain deployed and activated. Public sponsored SOL deposit processing was subsequently disabled at the user’s request; see the policy update below. The supported same-path reconsideration succeeded; no competing upgrade was identified or cancelled.

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

Exact served-byte comparisons passed for the custom-domain index, [/solana](https://app.rumiprotocol.com/solana), the new route chunk, minter-service chunk and certified gateway index. Index SHA-256 is `0fbeae31c3b67b591df023008a54c67060966ee9d67ee507d67ecfbe0e9a5677`. An initial signed-out browser check confirmed the page blocked new deposit addresses while backend processing was unavailable; the post-activation observations below supersede that initial state.

## Backend deployment and collateral activation

Backend `tfesu-vyaaa-aaaap-qrd7a-cai` was upgraded once through the original automatic approval path after the bounded reconsideration described below. The accepted install process exited 0 and reported success. Live readback confirms Running with the exact uploaded gzip SHA-256 `3b51aca404bb8da8994b55273114571a33486b99c3275547c809746254128566`. Retrieved deployed Candid is byte-equal and strict-equal to the reviewed artifact. Existing controllers remain unchanged, including the verified `rumi_identity` principal.

A fresh snapshot `00000000000000110000000001f088fe0101` was taken at 23:08:46 UTC while stopped. The backend was immediately restarted. Every downloaded payload length and hash was verified; saved Wasm equals the previous deployed module `e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352`. The oldest occupied snapshot2 was fully downloaded and verified before its slot was reused. Both previous and current backups remain privately preserved. Restoring stale state after intervening traffic is not authorized by this receipt.

`register_cksol_collateral` returned `Ok`. Live backend and Stability Pool registries contain Active ckSOL, ledger `ls5lp-lqaaa-aaaar-qb5oa-cai`, nine decimals and symbol ckSOL. All sixteen copied risk fields match the live ckETH configuration exactly, including recovery settings, rate curve, redemption terms and debt limits. The minimum collateral amount is converted from eighteen to nine decimals with ceiling rounding. The configured oracle pair is SOL/USD through XRC.

The funding requirement was resolved by the user's 10T funding of the verified deployment identity and exactly one authorized 6T backend top-up. The remaining identity balance was not transferred. After upgrade, actual backend cycles were 11,582,808,200,446 before sponsorship activation, above the relay's 11T dispatch threshold. `set_cksol_relay_budget` was called once with 10,000,000,000,000 cycles and returned `Ok`; independent live readback confirms that finite remaining allocation. Allocation is distinct from actual cycles and does not constitute a transfer or a mint.

The first stored SOL/USD sample is 103.921181088, source timestamp 2026-10-18 23:24:00 UTC. The callback trace at 23:25:45.938 UTC records that received rate immediately before synchronous validation; the subsequently stored matching price/timestamp establishes successful acceptance. The sample was approximately 105.938 seconds old, inside the 120-second freshness bound. Passive polling remains at its normal 300-second cadence; the cache is not continuously fresh, and price-sensitive operations refresh a stale sample on demand. No oracle/cadence override was applied.

Current public signed-out UI observations confirm the `/solana` funding-unavailable warning disappears after activation, while requesting a deposit address still requires connecting a wallet. Following its borrowing link selects ckSOL on the main borrowing page and displays the copied parameters. No wallet was connected and no transaction was submitted.

## Preserved state and verification limits

The 22,000 icUSD global cap, paused Stability Pool and disabled reserve redemptions remain unchanged. Global static configuration, mode/manual override and existing controllers match pre-upgrade state. All nine existing collateral configurations are unchanged except current price and price timestamp. Existing collateral quantities and vault counts match; debt and prices changed through ordinary accrual/oracle updates. Observed pending payout/refund/donation/Stability Pool notification/Treasury mint pages and counts remain unchanged. The old deployment lacked Treasury stable-transfer queue inspection; the new endpoint reports an empty page. No recovery/replay or queue clearing was sent.

This release establishes backend and frontend publication, collateral registration and relay enablement. A real SOL deposit, ckSOL mint, withdrawal, connected-wallet flow and settlement roundtrip have not been tested. The existing frontend typecheck baseline remains thirty errors; focused tests, builds and interface checks passed as recorded above. Two independent final live evidence reviewers, Frank and Grace, returned PASS with no confirmed blocking issues after inspecting the installed bytes/interface, backup payload hashes, preserved state, ckSOL risk settings, SP registration, finite relay allocation, public UI/assets and oracle sample. Their conclusions retain the unrun real-transaction limitation and bounded historical approval-cause uncertainty.

## Duplicate-request investigation and bounded reconsideration

The original backend upgrade submission was `call_d8eb4e7684894126a9bc97910385ba92`, submitted at 2026-10-18 08:03:23.875 UTC. Its running wrapper was cell 149. `call_50fc61606c0344b6a5ea85aa9a67fd79` at 08:13:19.335 UTC was a wait on that cell, not a second install. It returned terminal `CreateProcess ... Rejected` at 08:13:19.458 UTC. No shell execution log or install command-execution item was created. The deployment turn is completed.

The rejection said that an identical production upgrade request was already running and prohibited alternate or indirect execution. It also explicitly allowed additional checks to prove authorization or low risk before trying again. Earlier matching calls in this chat were help invocations, help cancellation, or read-only diagnostics. None submitted another backend upgrade.

Two independent audits inspected structured tool calls in Rumi session records: one covered 75 rollouts created October 16–18, and the other 59 rollouts created October 17–18 plus relevant older sessions with recent activity. Neither identified an earlier actual backend upgrade before the rejected submission. Bounded desktop command/history and review-feedback reads likewise identified no matching pending install or original competing approval event. No internal approval ID was recovered. These are bounded negative findings; they do not establish global absence or cancel an unidentified request. A self-match in the rejection is a hypothesis, not an established cause.

The supported approval-reconsideration API requires an original serialized guardian event, which was not available. No event was fabricated, no approval policy or database was changed, and no alternative executor was used. Both Charles and Dana independently concluded that exactly one fresh submission of the same frozen artifact, command and target through the same automatic approval reviewer is supported reconsideration on materially new evidence. A further rejection must be honored without another retry loop.

Fresh preflight at 23:05 UTC confirms the old backend still Running, ckSOL absent, the same controllers, cap 2,200,000,000,000 raw, reserve redemptions disabled, Stability Pool emergency-paused, mode GeneralAvailability and manual override false. Actual backend cycles were 11,645,765,573,789 at the subsequent status read. The old deployment lacks the new Treasury stable-transfer inspection method; that query's failure is unavailable visibility, not proof of an empty queue. Frozen artifact hashes and fresh Candid compatibility were reverified. Fresh snapshot `00000000000000110000000001f088fe0101` was taken at 23:08:46 UTC after completely archiving and verifying the oldest occupied snapshot2. The backend was immediately restarted on the old module. All fresh snapshot payload lengths and hashes were verified after download; saved Wasm equals the observed old module. Exactly one same-path reconsideration was subsequently accepted. The resulting install completed successfully; current deployment and activation receipts are recorded above.


## Subsequent user policy update: public sponsorship disabled

After reviewing the finite allowance, the user instructed Rumi to stop sponsoring public ckSOL deposit processing and to leave actual cycles in the backend. One `set_cksol_relay_budget(0)` call returned `Ok`; an independent live query confirmed the budget is zero. New sponsored deposit starts are disabled. Existing deposit reconciliation and ckSOL collateral registration were not changed. No cycles withdrawal or transfer was performed.

The previous 10T allocation and successful enablement above are historical activation receipts, superseded by this policy change. A browser user-funded processing flow has not been implemented; ordinary ingress calls cannot attach cycles directly.
