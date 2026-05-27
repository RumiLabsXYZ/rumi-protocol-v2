---
type: "query"
date: "2026-05-27T17:49:52.000830+00:00"
question: "Why does compute_redemption_shortfall() bridge Backend Accrual + Admin Tests and Backend sub-cluster (11 nodes)?"
contributor: "graphify"
source_nodes: ["compute_redemption_shortfall()", "redeem_then_accrue()", "accrue_redemption_shortfall_at()", "event.rs", "audit_pocs_red_002_redemption_deficit.rs"]
---

# Q: Why does compute_redemption_shortfall() bridge Backend Accrual + Admin Tests and Backend sub-cluster (11 nodes)?

## Answer

compute_redemption_shortfall() is the pure production function at src/rumi_protocol_backend/src/event.rs:L2243. It lives in community 3 (event.rs, 99 nodes). It is called by both production (accrue_redemption_shortfall_at() at L2274 right next door) AND by the test helper redeem_then_accrue() in community 180. That cross-community call is what gives it betweenness 0.076. The bridge is the exact mirror of Q1's bridge — same edge from the opposite side. Why does the test reach across? Because the wrapper accrue_redemption_shortfall_at() calls record_deficit_accrued → record_event → ic_cdk::api::time(), which panics outside canister context. The pure inner function compute_redemption_shortfall() has no canister-clock dependency (it explicitly takes the price_decimal as a parameter and just does saturating math on the seized-vs-target ICUSD difference), so the test can call it directly to verify the math without triggering the event-log path. Conclusion: no fix needed. The function's purity (taking timestamp/price as args) is what makes it test-friendly and is also what creates the bridge. One small data-quality note: the cross-file call is marked INFERRED in the graph rather than EXTRACTED, which is a tree-sitter limitation on Rust cross-module call resolution — the call site at audit_pocs_red_002_redemption_deficit.rs:L126 is in fact direct.

## Source Nodes

- compute_redemption_shortfall()
- redeem_then_accrue()
- accrue_redemption_shortfall_at()
- event.rs
- audit_pocs_red_002_redemption_deficit.rs