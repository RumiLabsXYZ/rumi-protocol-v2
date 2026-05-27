---
type: "query"
date: "2026-05-27T17:54:02.904517+00:00"
question: "Should Backend Admin Endpoints be split into smaller, more focused modules? (cohesion 0.0147)"
contributor: "graphify"
source_nodes: ["main.rs", "get_protocol_status()", "set_treasury_principal()", "inspect_message()"]
---

# Q: Should Backend Admin Endpoints be split into smaller, more focused modules? (cohesion 0.0147)

## Answer

No, the low cohesion is not a code smell here. The community is literally ALL 136 nodes from a single file: src/rumi_protocol_backend/src/main.rs. main.rs is the Candid endpoint dispatcher — every #[update]/#[query] handler lives there (get_protocol_status, get_event_count, get_vaults_page, get_redemption_rate, set_treasury_principal, set_stability_pool_principal, coingecko_transform, inspect_message, …). The low cohesion score (0.0147) measures inter-node call density inside the community, but Candid endpoints are SUPPOSED to be independent by design — each handler parses its args, delegates to one or two domain helpers in other files (vault.rs, event.rs, treasury.rs), and returns. They almost never call each other directly. The cohesion metric is misleading for entry-point modules.\n\nIs splitting main.rs into endpoint groups (admin.rs, queries.rs, lifecycle.rs) a good idea independently? Mild yes-for-readability, but with constraints: (1) all #[update]/#[query] handlers must be visible to ic-cdk's Candid auto-generation, so you can't actually move them to other files without re-exporting (ic-cdk macros only recognize handlers defined at crate root or pub-use'd back to it). (2) the ic-cdk world frequently parks endpoints in a single main.rs by convention and tooling expects that. (3) at 5862 lines (per the grep at L5862) main.rs is large but not pathological for a CDP protocol with this many surface endpoints. Recommendation: leave as-is unless you have an editor-navigation reason to break it up. The graph cohesion warning is a false signal for IC canister entry points and you can ignore it.

## Source Nodes

- main.rs
- get_protocol_status()
- set_treasury_principal()
- inspect_message()