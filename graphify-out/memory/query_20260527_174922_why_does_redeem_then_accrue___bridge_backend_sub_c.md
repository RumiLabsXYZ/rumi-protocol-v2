---
type: "query"
date: "2026-05-27T17:49:22.155308+00:00"
question: "Why does redeem_then_accrue() bridge Backend sub-cluster (11 nodes) and Backend Accrual + Admin Tests?"
contributor: "graphify"
source_nodes: ["redeem_then_accrue()", "compute_redemption_shortfall()", "accrue_redemption_shortfall_at()", "audit_pocs_red_002_redemption_deficit.rs", "event.rs"]
---

# Q: Why does redeem_then_accrue() bridge Backend sub-cluster (11 nodes) and Backend Accrual + Admin Tests?

## Answer

redeem_then_accrue() is a test-only helper at src/rumi_protocol_backend/tests/audit_pocs_red_002_redemption_deficit.rs:L117. The bridge is legitimate test scaffolding, not a code smell. The 11-node community is the RED-002 audit POC test file (5 #[test] scenarios + 6 helpers). Community 3 is event.rs (99 nodes) which houses the production accrual/event-log module. redeem_then_accrue() deliberately re-implements the production flow by calling production compute_redemption_shortfall() (event.rs:L2243) and state.accrue_deficit_shortfall() directly, instead of the natural wrapper accrue_redemption_shortfall_at() (event.rs:L2265). It does this because accrue_redemption_shortfall_at() additionally records a DeficitAccrued event via record_event(), which calls ic_cdk::api::time() — that panics outside a canister context (PocketIC is fine but the unit test path is not), a known limitation documented in the file comment at L132-137. Conclusion: the bridge node is expected and correct. It is a deliberate fork that mirrors production state mutation while sidestepping the event-log dependency. No action needed.

## Source Nodes

- redeem_then_accrue()
- compute_redemption_shortfall()
- accrue_redemption_shortfall_at()
- audit_pocs_red_002_redemption_deficit.rs
- event.rs