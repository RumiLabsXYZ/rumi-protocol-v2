//! Inbound observer for the Solana chain (M2, Task 7).
//!
//! Mirrors `chains::monad::deposit_watch`, pared down for Solana's M2 surface:
//!
//! - **Deposit watch** (runs every tick): poll each `AwaitingDeposit` Solana
//!   vault's custody-address SOL balance via the consensus-safe
//!   `sol_rpc::get_balance` (lamports at `finalized`). When the on-chain balance
//!   covers the declared collateral, flip the vault `AwaitingDeposit ->
//!   MintPending` and enqueue its `Mint` op via the SHARED, chain-agnostic
//!   `chains::vault::verify_deposit_and_enqueue_mint_in_state`. icUSD is only
//!   ever minted against a verified on-chain deposit (the CDP backing invariant).
//!
//! - **Supply gate (M2 detection-only)**: read the icUSD SPL mint's on-chain
//!   `supply` (e8s at `finalized`) via `sol_rpc::get_mint_supply` and compare it
//!   to the canister's recorded `chain_supplies[chain]`. The canister is the SOLE
//!   minter, so with no mint in flight an on-chain supply BELOW `recorded` means
//!   a burn landed. In M2 we only LOG that detection; the burn-recovery path
//!   (notify-then-verify via `submit_burn_proof`) lands in M3. A mint in flight,
//!   an exact match, or a probe error all stay in the cheap path.
//!
//! ## Burn watch is M3, NOT here
//!
//! Unlike Monad, this observer has NO block-by-block burn-log sweep, no cursor
//! (`last_observed_block`), no reorg circuit breaker, and no
//! `processed_burn_keys` dedup set. Solana reads at `finalized` (no reorgs), and
//! the burn-apply path is deferred to M3. The supply gate above is the M2
//! detection seam that M3's recovery will hang off.
//!
//! ## Borrow-across-await discipline (the keystone, mirrors Monad)
//!
//! `run_observer` NEVER holds a `read_state`/`mutate_state` borrow across an
//! `.await`. Every value an await depends on is snapshotted OUT of state under a
//! synchronous `read_state` closure FIRST (the `AwaitingDeposit` vault list is
//! cloned into a `Vec`; the supply-gate inputs are copied scalars); only AFTER
//! the await does it re-enter state via `mutate_state` to apply the transition.
//! A failed per-vault balance read logs and `continue`s (it must NOT abort the
//! observer or skip the remaining vaults).

use ic_canister_log::log;

use crate::chains::config::{ChainId, ChainStatus};
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

use super::hardening;

// ─── Pure helpers (unit-tested) ──────────────────────────────────────────────

/// Decide whether an observed SPL-mint supply indicates an unsubmitted burn.
///
/// Mirrors Monad's `backstop_should_scan`: returns `true` ONLY when no mint op
/// is in flight AND the on-chain supply has DROPPED below the canister's
/// confirmed `chain_supplies[chain]`.
///
/// The canister is the SOLE minter, so a burn (the only thing that lowers
/// supply without the canister minting) strictly LOWERS supply. `onchain >=
/// recorded` therefore means no unsubmitted burn: equal = in sync; GREATER = a
/// mint EXCESS (an RPC-false-negative mint that landed on-chain but was never
/// credited to `chain_supplies`), which is NOT a burn. A mint in flight could
/// legitimately make supply differ, so we never flag during a mint window.
///
/// M2 uses this for DETECTION-ONLY (an INFO log; the codebase has no WARN
/// sink). M3 will hang the notify-then-verify burn recovery off the `true` result.
pub fn supply_drop_detected(
    onchain_supply_e8s: u128,
    recorded_supply_e8s: u128,
    has_inflight_mint: bool,
) -> bool {
    !has_inflight_mint && onchain_supply_e8s < recorded_supply_e8s
}

// ─── Per-chain re-entrancy guard (mirrors monad::deposit_watch) ──────────────
//
// Once the Task-8 timer runs `run_observer` at a short interval, a slow RPC tick
// can still be awaiting when the next timer fires, which would spawn a SECOND
// `run_observer(chain)` concurrently. Two concurrent observers could
// double-enqueue a mint for the same AwaitingDeposit vault. This per-chain guard
// ensures only one `run_observer` per chain runs at a time. The RAII guard is a
// local held across all awaits, so it releases when the async fn returns on ANY
// path.
//
// Self-healing: the map stores the nanosecond timestamp the guard was acquired
// at. On the IC, a trap in a post-await continuation does NOT run `Drop`, so a
// stale entry would otherwise block that chain forever.
// `hardening::inflight_should_acquire` reclaims entries older than
// `hardening::INFLIGHT_STALE_NS` (10 min), self-healing after a trapped tick.

thread_local! {
    static OBSERVER_INFLIGHT: std::cell::RefCell<std::collections::BTreeMap<ChainId, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

struct ObserverGuard(ChainId);
impl Drop for ObserverGuard {
    fn drop(&mut self) {
        OBSERVER_INFLIGHT.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

// ─── Async observer loop ─────────────────────────────────────────────────────

/// Run one observation cycle for the given Solana chain.
///
/// Called by the Task-8 timer dispatch (NOT wired in this task). Control flow:
///   1. re-entrancy guard (acquire-or-skip; held across all awaits)
///   2. mode / invariant-halt skip
///   3. **deposit watch** (runs every tick): balance poll -> flip + enqueue mint
///   4. **supply gate** (M2 detection-only): log a drop; no recovery
///
/// See the module doc for the borrow-across-await discipline this upholds.
pub async fn run_observer(chain: ChainId) {
    // Re-entrancy guard (acquired BEFORE any other work): if a tick for this
    // chain is still in flight (and not stale), skip this one entirely. The RAII
    // guard releases on the future completing (any return path). A stale entry
    // (> INFLIGHT_STALE_NS old) means the previous holder trapped in a post-await
    // continuation and its `Drop` never ran (the later tick reclaims it,
    // self-healing the permanent-block scenario).
    let now_ns = ic_cdk::api::time();
    let _guard = match OBSERVER_INFLIGHT.with(|s| {
        let existing = s.borrow().get(&chain).copied();
        if hardening::inflight_should_acquire(existing, now_ns, hardening::INFLIGHT_STALE_NS) {
            s.borrow_mut().insert(chain, now_ns);
            Some(ObserverGuard(chain))
        } else {
            None
        }
    }) {
        Some(g) => g,
        None => return, // a fresh tick for this chain is already running; skip
    };

    // Guard: skip if in read-only mode or the supply invariant has halted. (No
    // reorg-halt check: Solana reads at `finalized`, so the reorg circuit breaker
    // does not apply here.)
    let should_skip = read_state(|s| s.mode == Mode::ReadOnly || s.multi_chain.invariant_halted);
    if should_skip {
        return;
    }

    // ── Deposit watch (open-then-verify), RUNS EVERY TICK ────────────────────
    //
    // Poll each AwaitingDeposit Solana vault's custody-address SOL balance. Once
    // the on-chain lamports cover the DECLARED collateral, flip the vault
    // AwaitingDeposit -> MintPending and enqueue its Mint op.
    //
    // Borrow discipline: snapshot the small per-vault tuples under ONE
    // `read_state` BEFORE the await loop; never hold a state borrow across
    // `get_balance(...).await`.
    let awaiting: Vec<(u64, String, u128)> = read_state(|s| {
        s.multi_chain
            .chain_vaults
            .values()
            .filter(|v| {
                v.collateral_chain == chain
                    && v.status == crate::chains::vault::ChainVaultStatus::AwaitingDeposit
            })
            .map(|v| (v.vault_id, v.custody_address.clone(), v.collateral_amount_native))
            .collect()
    });

    for (vault_id, custody_address, declared_lamports) in awaiting {
        let lamports = match super::sol_rpc::get_balance(&custody_address).await {
            Ok(bal) => bal,
            Err(e) => {
                // A failed balance read must NOT abort the observer or skip the
                // remaining vaults. Log and move on; the next tick retries this
                // vault.
                log!(
                    INFO,
                    "[solana observer chain={:?}] deposit get_balance failed for vault {} ({}): {}; will retry",
                    chain, vault_id, custody_address, e
                );
                continue;
            }
        };

        if (lamports as u128) < declared_lamports {
            // Not enough on-chain collateral yet; nothing to do this tick.
            continue;
        }

        // NO state borrow held across the await above: re-enter state only now.
        let now = ic_cdk::api::time();
        let transitioned = mutate_state(|s| {
            crate::chains::vault::verify_deposit_and_enqueue_mint_in_state(
                &mut s.multi_chain,
                vault_id,
                lamports as u128,
                now,
            )
        });

        match transitioned {
            Ok(true) => {
                log!(
                    INFO,
                    "[solana observer chain={:?}] deposit verified: vault={} custody={} balance_lamports={} >= declared_lamports={}; mint enqueued",
                    chain, vault_id, custody_address, lamports, declared_lamports
                );
            }
            Ok(false) => {
                // Idempotent no-op (already transitioned by an earlier tick, or a
                // concurrent transition). Nothing to log.
            }
            Err(e) => {
                log!(
                    INFO,
                    "[solana observer chain={:?}] verify_deposit_and_enqueue_mint FAILED for vault {}: {:?}; will retry",
                    chain, vault_id, e
                );
            }
        }
    }

    // ── Supply gate (M2 detection-only) ──────────────────────────────────────
    //
    // Probe the icUSD SPL mint's on-chain `supply` and compare to recorded. With
    // no mint in flight a DROP below `recorded` means a burn landed. In M2 we
    // only LOG that; the recovery path (submit_burn_proof) is M3.
    //
    // Cheap path first: if the SPL mint is unset, skip (an INFO log). If a mint is
    // in flight, skip WITHOUT the RPC (a live mint legitimately makes supply
    // differ, and would also mask a burn in the delta). Only otherwise do we make
    // the `get_mint_supply` outcall. All inputs are snapshotted out of state
    // BEFORE the await.
    let mint = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned());
    let mint = match mint {
        Some(m) => m,
        None => {
            log!(
                INFO,
                "[solana observer chain={:?}] no SPL mint configured; skipping supply gate",
                chain
            );
            return;
        }
    };

    let has_inflight_mint = read_state(|s| {
        s.multi_chain
            .settlement_queues
            .get(&chain)
            .map(|q| q.has_active_mint_op())
            .unwrap_or(false)
    });
    if has_inflight_mint {
        // A mint in flight legitimately differs supply; stay in the cheap path
        // (skip the RPC). M3's submit_burn_proof + the next post-confirm tick
        // reconcile any burn.
        return;
    }

    let recorded = read_state(|s| s.multi_chain.chain_supplies.get(&chain).copied().unwrap_or(0));

    match super::sol_rpc::get_mint_supply(&mint).await {
        Ok(onchain) => {
            if supply_drop_detected(onchain as u128, recorded, has_inflight_mint) {
                log!(
                    INFO,
                    "[solana observer] SPL supply {} < recorded {} on chain {:?}: a burn occurred; recovery via submit_burn_proof (M3)",
                    onchain, recorded, chain
                );
            }
        }
        Err(e) => {
            // A probe failure must NOT abort the observer; deposit-watch already
            // ran. Log and return; the next tick retries.
            log!(
                INFO,
                "[solana observer chain={:?}] supply gate get_mint_supply failed ({}); will retry",
                chain, e
            );
        }
    }
}

// ─── Timer dispatch helper (mirrors monad::observer_tick) ────────────────────

/// Fan-out entry point: run one observation cycle for every registered Solana
/// chain. The per-chain `run_observer` carries its own mode/halt/re-entrancy
/// guards, so this fan-out just snapshots the chain-id list and calls each in
/// turn. NO state borrow is held across the awaits (the chain-id Vec is cloned
/// out of state up front).
///
/// Provided for symmetry with Monad's `observer_tick`; the Task-8 timer wiring
/// decides whether to call this or `run_observer(SOLANA_CHAIN_ID)` directly. No-op
/// when no chain is registered (the Vec is empty).
pub async fn observer_tick() {
    let chains: Vec<ChainId> = read_state(|s| {
        s.multi_chain
            .chain_configs
            .iter()
            .filter(|(_, c)| matches!(c.status, ChainStatus::Registered))
            .map(|(id, _)| *id)
            .collect()
    });
    for chain in chains {
        run_observer(chain).await;
    }
}
