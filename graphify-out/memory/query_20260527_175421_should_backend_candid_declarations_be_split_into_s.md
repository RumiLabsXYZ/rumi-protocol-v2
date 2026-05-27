---
type: "query"
date: "2026-05-27T17:54:21.912260+00:00"
question: "Should Backend Candid Declarations be split into smaller, more focused modules? (cohesion 0.0169)"
contributor: "graphify"
source_nodes: ["icpswap_pool.did.d.ts", "rumi_protocol_backend.did.d.ts"]
---

# Q: Should Backend Candid Declarations be split into smaller, more focused modules? (cohesion 0.0169)

## Answer

No, and the community label is slightly misleading — there are actually TWO 'Backend Candid Declarations' communities, both with similar cohesion: Community 1 (118 nodes from src/declarations/icpswap_pool/icpswap_pool.did.d.ts) and Community 4 (88 nodes from src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts). 100% of nodes in each community come from a single auto-generated TypeScript declaration file emitted by dfx/didc from the corresponding .did Candid interface.\n\nThese files are flat lists of TypeScript type aliases and interfaces (Account, AccountBalance, AddLimitOrderInfo, ClaimArgs, …) with no call relationships between them — they're type DECLARATIONS, not code. The 'cohesion' metric measures inter-node connectivity, which is essentially zero by definition for a type-declaration file. Low cohesion here is a false alarm.\n\nThe correct remediation is not to split these files (you literally cannot — they're regenerated on every dfx build from the .did source) but to either: (a) exclude src/declarations/ from the graphify corpus via .gitignore-style filter, or (b) accept that auto-generated declarations will always show as 'low cohesion communities' and treat them as background noise.\n\nThe 118-node icpswap_pool surface is large because ICPSwap exposes a wide ABI (limit orders, liquidity, deposits, withdraws, claims, swaps). The 88-node rumi_protocol_backend surface is large because Rumi exposes vault management + collateral admin + interest config + redemption + audit-event queries + Candid consent messages. Both reflect the API surface they describe — nothing to refactor.

## Source Nodes

- icpswap_pool.did.d.ts
- rumi_protocol_backend.did.d.ts