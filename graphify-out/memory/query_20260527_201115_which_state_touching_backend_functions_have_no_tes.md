---
type: "query"
date: "2026-05-27T20:11:15.287750+00:00"
question: "Which State-touching backend functions have no test coverage?"
contributor: "graphify"
source_nodes: ["State", "interpolate_multiplier()", "resolve_curve()", "get_borrowing_fee_multiplier()", "set_liquidation_bonus()"]
---

# Q: Which State-touching backend functions have no test coverage?

## Answer

**Methodology caveat first**: graph-claimed test coverage is unreliable. Cross-checked 24 graph-claimed-untested State methods against grep of src/rumi_protocol_backend/tests/. Only 10 are actually untested; 14 are graph false-negatives (tests call them, graph just missed the edge). Confirms the recurring theme: graphify's cross-file Rust call resolution is incomplete, so use grep as ground truth for coverage questions.

**10 State methods with NO test references (grep-verified):**

*Fee-curve math (5 methods, highest leverage to test):*
- interpolate_multiplier — fee curve interpolation core helper
- resolve_curve — fee curve resolution per collateral
- resolve_anchor — fee curve anchor lookup
- get_borrowing_fee_multiplier — composed borrow fee calc
- get_dynamic_interest_rate_for — dynamic interest per collateral

These are all part of the dynamic-borrowing-fee surface. They take primitive inputs and return Decimals — perfect for property tests. A regression here would silently mis-charge borrowers without tripping any current test. **Highest recommendation: add a tests/audit_pocs_fee_curve.rs with property-based coverage** (boundary conditions, monotonicity, anchor walk-back).

*Per-collateral config getters (3 methods):*
- get_recovery_cr_for — recovery CR per collateral
- get_healthy_cr_for — healthy CR per collateral
- total_collateral_for — aggregate query

Lower risk because they're pure config lookups. A targeted unit test asserting per-collateral defaults match documented mainnet values would close the gap with minimal effort.

*Admin setter (1 method):*
- set_liquidation_bonus — admin-only mutation. No test means no regression coverage on the validation path. A small Pocket-IC test confirming (a) only admin can call, (b) bounds-check rejects out-of-range, (c) state actually updates would be valuable.

*The 14 false-negative confirmations* (graph said untested, grep proved otherwise): open_vault (18 test files), icp_collateral_type (9), get_collateral_config (3), get_min_liquidation_ratio_for, get_min_collateral_ratio_for (2 each), get_liquidation_ratio_for, total_borrowed_icusd_amount, accrue_single_vault, total_debt_for_collateral, repay_to_vault, get_borrowing_fee, weighted_average_interest_rate, unindex_vault_cr, reindex_vault_cr. All well-tested; the graph just missed cross-module Rust call resolution.

**Net recommendation**: prioritize adding fee-curve unit/property tests. state.rs and main.rs are 7-day-old and 3-day-old respectively (active dev), and the fee-curve methods are the high-leverage untested surface in that recent code.

## Source Nodes

- State
- interpolate_multiplier()
- resolve_curve()
- get_borrowing_fee_multiplier()
- set_liquidation_bonus()