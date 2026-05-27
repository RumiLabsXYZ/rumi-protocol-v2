---
type: "query"
date: "2026-05-27T20:07:55.419265+00:00"
question: "Are there orphan endpoints in main.rs that no frontend calls?"
contributor: "graphify"
source_nodes: ["main.rs", "get_protocol_config()", "open_vault_with_deposit", "add_margin_with_deposit", "partial_repay_to_vault"]
---

# Q: Are there orphan endpoints in main.rs that no frontend calls?

## Answer

No true dead code in the Candid surface. 181 public endpoints, 36 user-facing without direct frontend src references + 19 admin without frontend src references = 55 'no frontend caller'. Breaking down the 55:\n\n**~25 individual getters subsumed by get_protocol_config()** — get_global_icusd_mint_cap, get_interest_pool_share, get_liquidation_bonus, get_liquidation_frozen, get_min_icusd_amount, get_protocol_3usd_reserves, get_recovery_cr_multiplier, get_recovery_target_cr, get_stability_pool_config, get_three_pool_canister, get_amm1_canister, get_amm1_pool_id, get_pending_amm1_donations_count, get_icp_usd_price_e8s, get_treasury_principal, get_vault_count, get_vault_history_paged, get_fees_for_collateral, get_consumed_writedown_proofs, etc. Frontend prefers the consolidated get_protocol_config() / get_protocol_status() endpoints. These getters are NOT dead — they're a per-field surface for dfx debugging and external tooling.\n\n**~6 inter-canister methods** — stability_pool_liquidate, stability_pool_liquidate_debt_burned, stability_pool_liquidate_with_reserves, bot_cancel_liquidation, bot_claim_liquidation, bot_confirm_liquidation, reset_bot_budget. Called by rumi_stability_pool / liquidation_bot canisters inter-canister. Not orphans; correctly absent from frontend.\n\n**~5 admin/recovery tools** — clear_liquidation_breaker, clear_stuck_operations, recover_pending_transfer, admin_correct_vault_debts, set_liquidation_frozen. Manual dfx operator tooling. Correctly absent from frontend.\n\n**~3 deprecated-shape variants worth a future look** — add_margin_with_deposit (vs add_margin_to_vault), open_vault_with_deposit (vs open_vault_and_borrow), partial_repay_to_vault (vs repay_to_vault). These are alternate-shape endpoints from earlier API iterations. The newer compound methods (e.g. repay_and_close_vault from PR #199) suggest the codebase is consolidating; these variants may be safe to deprecate in a future Candid surface trim. Verify usage on mainnet before removing — analytics / explorer might still hit them.\n\n**Specific HTTPS-outcall transform**: coingecko_transform is an IC-system transform function, never user-called.\n\n**Methodology caveat**: graph-based 'inbound edge from frontend' lookup is unreliable because TS→Rust call resolution is missing in the graph extractor. Used direct grep against frontend src/ (excluding declarations/ and dist/) for ground truth. Graph signal would have been misleading here.

## Source Nodes

- main.rs
- get_protocol_config()
- open_vault_with_deposit
- add_margin_with_deposit
- partial_repay_to_vault