---
type: "query"
date: "2026-05-27T17:50:38.128357+00:00"
question: "Why does replay() bridge Backend sub-cluster (13 nodes) and Backend Accrual + Admin Tests?"
contributor: "graphify"
source_nodes: ["replay()", "init()", "post_upgrade()", "event.rs", "record_event()"]
---

# Q: Why does replay() bridge Backend sub-cluster (13 nodes) and Backend Accrual + Admin Tests?

## Answer

replay() at src/rumi_protocol_backend/src/event.rs:L1258 is the canister-upgrade state-reconstruction function. It pattern-matches every Event variant (Init, OpenVault, CloseVault, LiquidateVault, PartialLiquidateVault, RedistributeVault, BorrowFromVault, RedemptionOnVaults, …) and rebuilds State by replaying the event log forward. Community 156 (13 nodes) is the canister lifecycle module: init(), post_upgrade(), setup_timers(), register_xrc_fetch_timer, register_interest_treasury_timer, register_vault_check_timer, check_invariants(), capture_protocol_snapshot(), validate_collateral_state(), and 3 admin timer setters. Community 3 (event.rs, 99 nodes) is where every record_*() function lives. Interesting clustering quirk: replay() and the record_* functions are all defined in the SAME file (event.rs), but Leiden split the file across two communities because the call graph cleanly separates record-side (called from vault/redemption flows) from replay-side (called only from post_upgrade). replay() is intrinsically a cross-cutting bridge — by design, every event-producing function feeds it through its variant. You cannot eliminate this bridge without abandoning the event-sourced state model. Betweenness 0.046 reflects exactly that: replay() is THE single point through which every event type passes during upgrade. Conclusion: bridge is structural and correct. The fact that two communities contain code from the same file is a clustering artifact worth understanding (event.rs is doing two distinct jobs: recording during normal operation, replaying during upgrade), not a code smell.

## Source Nodes

- replay()
- init()
- post_upgrade()
- event.rs
- record_event()