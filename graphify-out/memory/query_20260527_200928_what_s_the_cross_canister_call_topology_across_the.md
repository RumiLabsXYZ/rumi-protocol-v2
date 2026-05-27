---
type: "query"
date: "2026-05-27T20:09:28.335566+00:00"
question: "What's the cross-canister call topology across the 7 Rumi canisters?"
contributor: "graphify"
source_nodes: ["rumi_protocol_backend", "rumi_stability_pool", "rumi_amm", "rumi_3pool", "rumi_treasury", "liquidation_bot", "rumi_analytics"]
---

# Q: What's the cross-canister call topology across the 7 Rumi canisters?

## Answer

Mapped via grep of ic_cdk::call patterns across all 7 Rust canister sources. (Graph signal was unreliable due to fuzzy-dedup false merges + missing TS→Rust resolution, so used ground-truth grep instead.)\n\n**Concrete non-ledger inter-canister edges:**\n\n1. **rumi_protocol_backend (the orchestration hub)** — 142 ic_cdk::call sites total. Outbound non-ledger:\n   - → liquidation_bot :: notify_liquidatable_vaults\n   - → rumi_stability_pool :: notify_liquidatable_vaults, receive_donation, receive_interest_revenue, register_collateral\n   - → rumi_treasury :: deposit\n   - → rumi_amm :: notify_reward_received\n\n2. **rumi_amm → backend + 3pool** — 36 ic_cdk::call sites. Outbound non-ledger:\n   - → rumi_protocol_backend :: get_icp_usd_price_e8s\n   - → rumi_3pool :: get_all_lp_holders, icrc1_total_supply (treating 3pool as 3USD ledger)\n   - → icusd_ledger :: icrc1_transfer, icrc1_balance_of\n\n3. **liquidation_bot → backend + external DEX** — 21 ic_cdk::call sites. Outbound:\n   - → rumi_protocol_backend :: bot_claim_liquidation, bot_confirm_liquidation, bot_cancel_liquidation\n   - → ICPSwap pool (external) :: quote, depositFromAndSwap, metadata\n   - → ckusdc_ledger, icp_ledger :: icrc1_transfer, icrc1_balance_of, icrc1_fee\n\n4. **rumi_analytics (pure read-only consumer)** — 26 ic_cdk::call sites. All read-side:\n   - → rumi_amm :: 5 methods (swap/liquidity events, pools)\n   - → rumi_3pool :: 7 methods (incl. v1 and v2 event endpoints, swap/liquidity)\n   - → rumi_protocol_backend :: 5 methods (status, vaults, totals, events)\n   - → rumi_stability_pool :: get_pool_events, get_pool_event_count\n\n5. **rumi_stability_pool** — 20 ic_cdk::call sites, mostly ledger transfers. One outbound to 'pool_canister'::get_pool_status (likely the 3pool or AMM stats).\n\n6. **rumi_3pool** — 24 ic_cdk::call sites, ALL ledger calls (icrc1_transfer / icrc2_transfer_from / icrc1_balance_of). 3pool initiates ZERO calls to other Rumi canisters — it's a pure leaf in the Rumi graph.\n\n7. **rumi_treasury** — 2 ic_cdk::call sites total, both for icrc1_transfer to ledgers. Receives most calls passively (from backend); rarely initiates.\n\n**Topology shape:** rumi_protocol_backend is the orchestrator with outbound edges to SP, AMM, treasury, and bot. liquidation_bot calls BACK into backend (3 bot_* methods). rumi_analytics is a pure pull-side consumer touching 4 other canisters. rumi_3pool and rumi_treasury are near-leaves. SP has one outbound to a generic 'pool_canister'.\n\n**No surprising couplings.** Architecture matches the README mental model. Notable: rumi_3pool's leaf status validates the design — it's self-contained DEX logic with only ledger I/O, which is why the recent AMM1 work (rumi_amm) adds NEW edges to 3pool rather than pulling logic into 3pool itself.\n\n**Worth a future check:** the 'pool', 'pool_canister', 'pool_principal' references in backend code are dynamic principal lookups — verify on-mainnet that these resolve to expected canisters (rumi_stability_pool / rumi_amm) and not stale principal IDs.

## Source Nodes

- rumi_protocol_backend
- rumi_stability_pool
- rumi_amm
- rumi_3pool
- rumi_treasury
- liquidation_bot
- rumi_analytics