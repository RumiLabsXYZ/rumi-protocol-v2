//! Inbound observer for the Monad chain (Phase 1b, Task 9).
//!
//! ## Pure helpers (unit-tested)
//!
//! - `credit_deposit_to_state`: credits on-chain collateral deposits to a
//!   ChainVaultV1 record (increments `collateral_amount_e18`).
//! - `apply_burn_to_state`: atomically decrements `chain_supplies` and vault
//!   `debt_e8s` when an on-chain Burn event is observed at finality.
//!
//! ## Mutation ordering in apply_burn_to_state (correctness guarantee)
//!
//! The function enforces a strict no-mutation-on-rejection guarantee:
//!
//! 1. Look up vault (reject if unknown — no mutation).
//! 2. Reject if `burn.amount_e8s > debt_e8s` (no mutation).
//! 3. Call `apply_supply_delta(state, chain, Decrease(amount), new_total_debt)`.
//!    `apply_supply_delta` validates underflow, divergence, and halt BEFORE
//!    mutating chain_supplies; on any error it returns `Err` with state
//!    untouched.
//! 4. ONLY after (3) succeeds: decrement `vault.debt_e8s`.
//!
//! This means a rejected burn (for any reason) leaves BOTH `chain_supplies`
//! and `debt_e8s` unchanged — the tests in `tests_deposit_watch` assert this.
//!
//! ## Async observer loop (run_observer)
//!
//! `run_observer` scans Burn events at finality for the given chain and applies
//! them through `apply_burn_to_state`. Deposit (collateral) watch is
//! implemented as a minimal stub in Phase 1b: the pure helper
//! `credit_deposit_to_state` is fully tested and ready; the async balance-check
//! loop is deferred to the Task 23 manual integration (it requires a stable
//! EVM RPC connection to Monad testnet). A TODO comment marks the stub clearly.
//!
//! Reorg handling is NOT implemented here; Task 11 adds the `is_reorg` check
//! after fetching the finalized block number. The structure of `run_observer`
//! intentionally leaves a seam for Task 11 to insert between the
//! `fetch_block_numbers` call and the log-scan loop.

use ic_canister_log::log;

use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::supply::{apply_supply_delta, SupplyDelta};
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

use super::evm_rpc::{decode_burn_log, fetch_block_numbers, get_balance, get_logs, BURN_EVENT_TOPIC0};

// ─── Pure helpers ─────────────────────────────────────────────────────────────

/// Credit a confirmed on-chain deposit to a ChainVaultV1 record.
///
/// Increments `collateral_amount_e18` by `amount_e18` (saturating — overflow
/// of a u128 collateral balance is not a realistic failure mode but we guard
/// it anyway). Returns `Err` if the vault is not found.
pub fn credit_deposit_to_state(
    state: &mut MultiChainStateV2,
    vault_id: u64,
    amount_e18: u128,
) -> Result<(), String> {
    let vault = state
        .chain_vaults
        .get_mut(&vault_id)
        .ok_or_else(|| format!("credit_deposit: unknown vault_id {}", vault_id))?;
    vault.collateral_amount_e18 = vault.collateral_amount_e18.saturating_add(amount_e18);
    Ok(())
}

/// Apply a confirmed on-chain Burn event to protocol state.
///
/// Decrements `chain_supplies[chain]` and `vault.debt_e8s` together.
/// The caller must pass `total_debt_e8s` — the pre-burn sum of
/// `chain_vault.debt_e8s` across all foreign-chain vaults (i.e.
/// `MultiChainStateV2::total_chain_vault_debt_e8s()` at the moment of
/// the call). The function internally computes the expected post-burn
/// total as `total_debt_e8s - burn.amount_e8s` and passes that to
/// `apply_supply_delta` so the invariant check can verify:
///   `new_chain_supplies_sum == post_burn_total_debt`.
/// ICP-native debt (`State::total_borrowed_icusd_amount`) is a separate
/// pool and must NOT be passed here.
///
/// ## Mutation ordering (correctness guarantee)
///
/// 1. Vault lookup — reject (no mutation) if unknown.
/// 2. Debt-exceeds check — reject (no mutation) if `amount > debt`.
/// 3. `apply_supply_delta` — validates and mutates `chain_supplies` or
///    rejects entirely (no mutation to any field on error).
/// 4. Only after (3) succeeds: `vault.debt_e8s -= amount_e8s`.
///
/// Any rejection path returns `Err` with BOTH `chain_supplies` and
/// `debt_e8s` unchanged.
pub fn apply_burn_to_state(
    state: &mut MultiChainStateV2,
    burn: &super::evm_rpc::BurnLog,
    total_debt_e8s: u128,
) -> Result<(), String> {
    // Step 1: vault lookup (read-only — no mutation on failure)
    let (chain, current_debt) = {
        let vault = state
            .chain_vaults
            .get(&burn.vault_id)
            .ok_or_else(|| format!("apply_burn: unknown vault_id {}", burn.vault_id))?;
        (vault.collateral_chain, vault.debt_e8s)
    };

    // Step 2: debt-exceeds check (no mutation on failure)
    if burn.amount_e8s > current_debt {
        return Err(format!(
            "apply_burn: burn amount {} exceeds vault {} debt {}",
            burn.amount_e8s, burn.vault_id, current_debt
        ));
    }

    // Compute the post-burn total debt that apply_supply_delta will compare
    // against the post-delta chain_supplies sum. total_debt_e8s is the
    // pre-burn sum of foreign-chain vault debts (total_chain_vault_debt_e8s);
    // after this burn the total drops by burn.amount_e8s.
    let post_burn_total = total_debt_e8s.saturating_sub(burn.amount_e8s);

    // Step 3: supply delta — validates and mutates chain_supplies, or rejects
    // entirely (chain_supplies unchanged on Err)
    apply_supply_delta(state, chain, SupplyDelta::Decrease(burn.amount_e8s), post_burn_total)
        .map_err(|e| format!("{:?}", e))?;

    // Step 4: only reached when supply delta succeeded — decrement vault debt
    let vault = state
        .chain_vaults
        .get_mut(&burn.vault_id)
        .expect("vault present: checked above");
    vault.debt_e8s -= burn.amount_e8s;

    Ok(())
}

// ─── Async observer loop ─────────────────────────────────────────────────────

/// Run one observation cycle for the given chain.
///
/// Called by Timer A (configured in Task 12). Scans Burn logs from
/// `last_observed_block + 1` to the current finalized block and applies
/// them to state. Advances `last_observed_block` to `finalized` at the end.
///
/// ## Burn watch
///
/// For each raw Burn log decoded via `decode_burn_log`, the observer:
/// 1. Reads the current total supply (as-of-this-moment, before this burn).
/// 2. Computes `new_total_debt = current_total - amount_e8s`.
/// 3. Calls `mutate_state(apply_burn_to_state)` and records a
///    `ChainBurnObserved` event on success.
/// 4. On failure: logs the error clearly and continues to the next log.
///    The block cursor does NOT advance past a failing burn — if any burn
///    in a range fails, the entire range is retried on the next tick.
///    (Per Phase 1b spec: "log + continue is acceptable but the error must
///    be visible". A future hardening pass can add per-burn retry tracking.)
///
/// ## Deposit watch (STUB — Phase 1b)
///
/// The pure helper `credit_deposit_to_state` is fully tested. The async
/// custody-balance poll loop is deferred to Task 23 (manual integration).
/// This stub logs that deposit watch is not yet active.
///
/// ## Reorg handling
///
/// NOT implemented here. Task 11 inserts an `is_reorg` check after
/// `fetch_block_numbers` and before the log scan. The structure below
/// leaves a clear seam for that insertion.
pub async fn run_observer(chain: ChainId) {
    // Guard: skip if in read-only mode or invariant has halted.
    let should_skip = read_state(|s| s.mode == Mode::ReadOnly || s.multi_chain.invariant_halted);
    if should_skip {
        return;
    }

    // Read the icUSD contract address for this chain. Return early if unset.
    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
    let contract = match contract {
        Some(c) => c,
        None => {
            log!(INFO, "[observer chain={:?}] no contract address configured; skipping", chain);
            return;
        }
    };

    let last_observed = read_state(|s| {
        s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0)
    });

    // Fetch finalized block number.
    // TASK 11 SEAM: insert reorg check here, between fetch_block_numbers and
    // the log scan below.
    let finalized = match fetch_block_numbers(chain).await {
        Ok((_latest, fin)) => fin,
        Err(e) => {
            log!(INFO, "[observer chain={:?}] fetch_block_numbers failed: {}", chain, e);
            return;
        }
    };

    if finalized <= last_observed {
        // Nothing new to observe.
        return;
    }

    let from_block = last_observed + 1;

    // ── Burn watch ──────────────────────────────────────────────────────────

    let raw_burn_logs = match get_logs(chain, &contract, BURN_EVENT_TOPIC0, from_block, finalized).await {
        Ok(logs) => logs,
        Err(e) => {
            log!(INFO, "[observer chain={:?}] get_logs(burn) failed: {}; will retry on next tick", chain, e);
            return;
        }
    };

    let mut burn_ok = true;
    for (topics, data, tx_hash, block_number) in &raw_burn_logs {
        let burn = match decode_burn_log(topics, &data, &tx_hash, *block_number) {
            Ok(b) => b,
            Err(e) => {
                log!(INFO, "[observer chain={:?}] decode_burn_log failed at block {}: {}", chain, block_number, e);
                burn_ok = false;
                break;
            }
        };

        // Snapshot the pre-burn foreign-chain vault debt total (each burn
        // decrements one vault's debt_e8s, so we re-read before each burn
        // to get the correct pre-burn total for the invariant check).
        // total_chain_vault_debt_e8s sums only chain_vaults, which is the
        // correct pool for the Phase 1b foreign-chain-only supply invariant.
        // ICP-native total_borrowed_icusd_amount is a separate pool and is
        // deliberately excluded here.
        let current_total: u128 = read_state(|s| s.multi_chain.total_chain_vault_debt_e8s());

        let burn_clone = burn.clone();
        let result = mutate_state(|s| {
            apply_burn_to_state(&mut s.multi_chain, &burn_clone, current_total)
        });

        match result {
            Ok(()) => {
                let now = ic_cdk::api::time();
                crate::storage::record_event(&crate::event::Event::ChainBurnObserved {
                    chain_id: chain,
                    vault_id: burn.vault_id,
                    amount_e8s: burn.amount_e8s,
                    tx_hash: burn.tx_hash.clone(),
                    block_number: burn.block_number,
                    timestamp: now,
                });
                log!(
                    INFO,
                    "[observer chain={:?}] burn applied: vault={} amount_e8s={} block={} tx={}",
                    chain, burn.vault_id, burn.amount_e8s, burn.block_number, burn.tx_hash
                );
            }
            Err(e) => {
                log!(
                    INFO,
                    "[observer chain={:?}] apply_burn_to_state FAILED for tx {} vault {}: {}",
                    chain, burn.tx_hash, burn.vault_id, e
                );
                burn_ok = false;
                break;
            }
        }
    }

    // ── Deposit watch (STUB) ─────────────────────────────────────────────────
    //
    // The pure helper `credit_deposit_to_state` is implemented and tested.
    // The async custody-balance poll is deferred to Task 23 (manual
    // integration with Monad testnet). When Task 23 lands, replace this block
    // with a loop over open chain_vaults for this chain, calling `get_balance`
    // on each `custody_address`, computing the delta, and calling
    // `credit_deposit_to_state` + emitting `DepositObserved`.
    //
    // The `get_balance` function is already available from `evm_rpc`.
    let _ = get_balance; // suppress unused-import warning until Task 23

    // ── Advance cursor (only on full success) ───────────────────────────────

    if burn_ok {
        mutate_state(|s| {
            s.multi_chain.last_observed_block.insert(chain, finalized);
        });
    }
}
