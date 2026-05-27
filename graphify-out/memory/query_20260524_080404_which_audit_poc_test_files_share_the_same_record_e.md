---
type: "query"
date: "2026-05-24T08:04:04.783530+00:00"
question: "Which audit POC test files share the same record_event ic_cdk::api::time bypass workaround pattern as the RED-002 POC?"
contributor: "graphify"
source_nodes: ["record_event()", "compute_redemption_shortfall()", "accrue_redemption_shortfall_at()", "record_margin_transfer()", "compute_redemption_fee()"]
---

# Q: Which audit POC test files share the same record_event ic_cdk::api::time bypass workaround pattern as the RED-002 POC?

## Answer

Of 38 audit POC files in src/rumi_protocol_backend/tests/audit_pocs_*.rs, only 3 mention the ic_cdk::api::time() panic outside canister context: red_002_redemption_deficit, liq_001_pending_margin_race, and tests.rs:1840. They use 3 distinct strategies: (A) RED-002 inlines the mutation and skips record_event, with grep-the-production-source as compensating control at L274-300 — brittle, rename breaks it. (B) LIQ-001 hand-mirrors the map removal and defers event-record coverage to pocket_ic_tests with an explicit comment at L163 — clean decomposition. (C) Pure-function tests (redemption-fee tests in tests.rs:1840, INT-001) call only the math primitive — self-contained, no risk. Zero of the 32 non-pic POCs call record_*() directly, confirmed by grep. RED-002 is the unique outlier with the brittle grep pattern. Fix path: extract accrue_redemption_shortfall_at into a pure inner do_accrue_redemption_shortfall(now, ...) + a thin wrapper that adds record_at_now; OR abstract ic_cdk::api::time() behind a trait/Clock the tests can mock. Option 2 kills the entire workaround class.

## Source Nodes

- record_event()
- compute_redemption_shortfall()
- accrue_redemption_shortfall_at()
- record_margin_transfer()
- compute_redemption_fee()