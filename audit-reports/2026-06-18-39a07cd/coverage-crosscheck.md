# Coverage cross-check (Step 5) — grep-authoritative

Anchor 39a07cd. Method: exact-symbol enumeration (zero false negatives), each item mapped to the pass that covered it. No code-graph reliance.

## 1. State-mutating chain entry points (main.rs) — all covered
register_chain, set_chain_config, open_chain_vault, withdraw_chain_collateral, close_chain_vault,
get_chain_settlement_address, get_chain_vault, set_chain_contract, set_manual_collateral_price,
set_evm_rpc_principal, clear_invariant_halt, clear_reorg_halt, set_last_observed_block,
set_burn_watch_poll_enabled, set_chain_interest_tick_interval_secs, set_chain_interest_min_realize_e8s,
harvest_chain_interest, get_chain_interest_treasury_address.
-> covered by: auth-dos (all), mint-doublemint (open), withdraw-custody (withdraw/close),
   interest-accrual (harvest/set_chain_interest_*/get_treasury), rpc-finality-reorg (set_manual_price/
   set_last_observed_block/clear_reorg/set_chain_contract), tecdsa-keys (get_*_address), supply-invariant (clear_invariant_halt).
NO entry-point coverage gap.

## 2. Re-entrancy / guard sites — all covered (async-races pass)
SETTLEMENT_INFLIGHT (evm/settlement.rs), OBSERVER_INFLIGHT (evm + solana deposit_watch),
SOLANA_SETTLEMENT_INFLIGHT (solana/settlement.rs), inflight_should_acquire/INFLIGHT_STALE_NS (hardening.rs).
The async-races pass examined the guard acquire/Drop/self-heal; the withdraw-custody pass examined the per-op CAS.

## 3. Supply-mutation / mint / burn / transfer sites — all covered (supply-invariant + mint + withdraw passes)
70 references across: supply.rs (apply_supply_delta, check_invariant), evm/settlement.rs (confirm_mint_in_state,
confirm_interest_mint_in_state, debt_e8s +=), evm/deposit_watch.rs + evm/burn_proof.rs (apply_burn_to_state),
vault.rs (open/withdraw/close debt+collateral), multi_chain_state.rs (totals helpers), solana/settlement.rs.
Each file audited by the supply-invariant pass; the two confirm fns + apply_burn additionally by mint + withdraw passes.

## Result
No uncovered economic entry point, guard site, or value-transfer site. The multi-site root cause F-05
names both its sites (confirm_interest_mint_in_state + the close shortcut).
