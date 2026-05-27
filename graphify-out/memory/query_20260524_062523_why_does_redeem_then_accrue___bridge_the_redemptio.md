---
type: "query"
date: "2026-05-24T06:25:23.230645+00:00"
question: "Why does redeem_then_accrue() bridge the redemption-deficit POC cluster to the production accrual cluster, and does production exercise the same shortfall accounting the POC asserts?"
contributor: "graphify"
source_nodes: ["redeem_then_accrue()", "compute_redemption_shortfall()", "accrue_redemption_shortfall_at()", "record_event()"]
---

# Q: Why does redeem_then_accrue() bridge the redemption-deficit POC cluster to the production accrual cluster, and does production exercise the same shortfall accounting the POC asserts?

## Answer

The bridge is structurally real but mislabeled INFERRED — the POC at tests/audit_pocs_red_002_redemption_deficit.rs:126 directly calls production compute_redemption_shortfall via the import at line 66. Production composes redeem -> accrue_redemption_shortfall_at -> compute_redemption_shortfall + state.accrue_deficit_shortfall + record_event(DeficitAccrued). The POC helper redeem_then_accrue manually composes compute_redemption_shortfall + state.accrue_deficit_shortfall but DELIBERATELY skips record_event because record_event calls ic_cdk::api::time() which panics outside a canister context (comment at lines 132-137). To keep the production composition gated, the POC adds grep-the-source assertions at L274-300 that string-match for 'pub fn accrue_redemption_shortfall_at(' and verify the redemption entry path still calls it. This is brittle — a refactor that renames or inlines accrue_redemption_shortfall_at would silently bypass coverage. More robust: extract a non-time-dependent inner helper, or hide ic_cdk::api::time() behind a trait the test can mock.

## Source Nodes

- redeem_then_accrue()
- compute_redemption_shortfall()
- accrue_redemption_shortfall_at()
- record_event()