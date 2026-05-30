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
//! `run_observer` runs TWO independent watches each tick:
//!
//! - **Deposit watch** (balance poll → flip `AwaitingDeposit`→`MintPending`,
//!   enqueue mint) runs UNCONDITIONALLY every tick. It uses only the
//!   consensus-safe `get_balance(addr, "latest")` read and is fully decoupled
//!   from the block-height path. A block-height read failure (the Layer-2
//!   EVM-RPC consensus issue that blocked Gate-4 on staging) must NOT skip
//!   deposit detection.
//! - **Burn watch** scans Burn events at finality and applies them through
//!   `apply_burn_to_state`. It is GATED: it runs only when the burn-watch cursor
//!   (`last_observed_block[chain]`) is seeded (`!= 0`) AND a fresh finalized
//!   block height is available. The reorg `is_reorg` check (Task 11) sits
//!   between `fetch_block_numbers` and the log scan. A block-read failure
//!   degrades the tick to deposit-only.

use ic_canister_log::log;

use crate::chains::config::{ChainId, ChainStatus};
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::supply::{apply_supply_delta, SupplyDelta};
use crate::logs::INFO;
use crate::state::{mutate_state, read_state};
use crate::Mode;

use super::chain_vault::{verify_deposit_and_enqueue_mint_in_state, ChainVaultStatus};
use super::evm_rpc::{decode_burn_log, fetch_block_numbers, get_balance, get_logs, BURN_EVENT_TOPIC0};
use super::{hardening, tecdsa};

// ─── Pure helpers ─────────────────────────────────────────────────────────────

/// Classification of an `apply_burn_to_state` failure, so the observer loop can
/// decide whether to SKIP the burn (advancing the cursor past it) or HALT.
///
/// This is the keystone of the C-1 supply-divergence fix. The pre-fix code
/// returned a flat `Result<(), String>` and the observer `break`-ed on ANY
/// error, stalling the cursor and forcing the whole range to re-scan. With no
/// idempotency, that re-applied already-applied partial burns, silently
/// double-decrementing `debt_e8s` and `chain_supplies` together (so the
/// Timer-B self-check never fired). The typed error lets the loop:
///   - SKIP an `InvalidBurn` (permanent-invalid: unknown vault / over-repay) —
///     it can never succeed, so advancing past it is safe and keeps the
///     observer + settlement-finality live;
///   - HALT on a `SupplyInvariant` (halt-class) without advancing the cursor.
#[derive(Debug)]
pub enum BurnApplyError {
    /// Permanent-invalid burn (unknown vault / over-repay beyond remaining
    /// debt). Skippable: the cursor may advance past it; it will never succeed.
    /// Carries a human-readable reason for the WARN log.
    InvalidBurn(String),
    /// Halt-class supply-invariant failure (`apply_supply_delta` rejected the
    /// decrement: underflow, divergence, or an already-set self-check halt).
    /// The protocol must NOT advance the cursor past this; the invariant
    /// machinery halts.
    SupplyInvariant(crate::chains::supply::SupplyInvariantError),
}

impl std::fmt::Display for BurnApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BurnApplyError::InvalidBurn(msg) => write!(f, "InvalidBurn({})", msg),
            BurnApplyError::SupplyInvariant(e) => write!(f, "SupplyInvariant({:?})", e),
        }
    }
}

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
///
/// ## Permissionless payer (intentional; see IcUSD.sol review)
///
/// `IcUSD.burn(amount, target_vault_id)` is public: ANY holder can burn their
/// OWN icUSD citing ANY `vault_id`, and this function decrements THAT vault's
/// debt without checking the burner owns it. This is a deliberate "anyone can
/// repay a vault" design, NOT a theft vector: the burner destroys their own
/// tokens, and the freed collateral is only ever released by the separate,
/// owner-authorized (status==Open) `withdraw_chain_collateral`/`close_chain_vault`
/// path — never off the burn. The supply invariant stays balanced (supply and
/// debt both drop by `amount`). The only effect of a third-party burn is to
/// over-collateralize the target vault (a gift). A future phase may add a
/// burner==owner constraint if griefing (uninvited debt repayment) becomes a
/// concern; for Phase 1b it is accepted.
pub fn apply_burn_to_state(
    state: &mut MultiChainStateV2,
    burn: &super::evm_rpc::BurnLog,
    total_debt_e8s: u128,
) -> Result<(), BurnApplyError> {
    // Step 1: vault lookup (read-only — no mutation on failure).
    // Unknown vault is a PERMANENT-INVALID burn (e.g. a permissionless Burn
    // citing a closed/never-existed vault) → InvalidBurn (skippable).
    let (chain, current_debt) = {
        let vault = state.chain_vaults.get(&burn.vault_id).ok_or_else(|| {
            BurnApplyError::InvalidBurn(format!("apply_burn: unknown vault_id {}", burn.vault_id))
        })?;
        (vault.collateral_chain, vault.debt_e8s)
    };

    // Step 2: debt-exceeds check (no mutation on failure). Over-repaying a
    // vault's remaining debt is PERMANENT-INVALID (the on-chain burn already
    // happened, but it can never be applied here) → InvalidBurn (skippable).
    if burn.amount_e8s > current_debt {
        return Err(BurnApplyError::InvalidBurn(format!(
            "apply_burn: burn amount {} exceeds vault {} debt {}",
            burn.amount_e8s, burn.vault_id, current_debt
        )));
    }

    // Compute the post-burn total debt that apply_supply_delta will compare
    // against the post-delta chain_supplies sum. total_debt_e8s is the
    // pre-burn sum of foreign-chain vault debts (total_chain_vault_debt_e8s);
    // after this burn the total drops by burn.amount_e8s.
    let post_burn_total = total_debt_e8s.saturating_sub(burn.amount_e8s);

    // Step 3: supply delta — validates and mutates chain_supplies, or rejects
    // entirely (chain_supplies unchanged on Err). A failure here is HALT-CLASS
    // (underflow / divergence / already-halted) → SupplyInvariant: the caller
    // must NOT advance the cursor.
    apply_supply_delta(state, chain, SupplyDelta::Decrease(burn.amount_e8s), post_burn_total)
        .map_err(BurnApplyError::SupplyInvariant)?;

    // Step 4: only reached when supply delta succeeded — decrement vault debt.
    // No-mutation-on-rejection guarantee is intact: every `return Err`/`?`
    // above happens BEFORE this point, so a rejected burn leaves BOTH
    // chain_supplies (untouched by apply_supply_delta on its Err path) and
    // debt_e8s (untouched, decremented only here) unchanged.
    let vault = state
        .chain_vaults
        .get_mut(&burn.vault_id)
        .expect("vault present: checked above");
    vault.debt_e8s -= burn.amount_e8s;

    Ok(())
}

// ─── Timer A tick (fan-out) ──────────────────────────────────────────────────

/// Timer A entry point: run one observation cycle for every registered+enabled
/// chain. The per-chain `run_observer` carries its own mode/halt/re-entrancy
/// guards, so this fan-out just snapshots the chain-id list and calls each in
/// turn. NO state borrow is held across the awaits — the chain-id Vec is cloned
/// out of state up front.
///
/// No-op when no chain is registered (the Vec is empty), so it is safe to
/// register on the staging canister before Monad is configured (Task 15 PocketIC
/// smoke test asserts this).
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

// ─── Per-chain re-entrancy guard (Task 13 review; wired Task 15) ───────────────
//
// Once Timer A runs at a short interval, a slow RPC tick can still be awaiting
// when the next timer fires, which would spawn a SECOND `run_observer(chain)`
// concurrently. Two concurrent observers could double-apply the same Burn log or
// double-enqueue a mint for the same AwaitingDeposit vault. This per-chain guard
// ensures only one `run_observer` per chain runs at a time. The RAII guard is a
// local held across all awaits, so it releases when the async fn returns on ANY
// path.

thread_local! {
    static OBSERVER_INFLIGHT: std::cell::RefCell<std::collections::BTreeSet<ChainId>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
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

/// Run one observation cycle for the given chain.
///
/// Called by Timer A (configured in Task 12). Control flow:
///   1. re-entrancy guard / mode-halt-reorg skip / contract check /
///      `refresh_hot_wallet_balance` (all unchanged)
///   2. **deposit watch** — runs EVERY tick (see below), decoupled from blocks
///   3. **burn watch** — gated on a seeded cursor and fresh finalized height
///
/// ## Deposit watch (runs every tick)
///
/// Polls each `AwaitingDeposit` vault's custody-address native balance via the
/// consensus-safe `get_balance(addr, "latest")`. When the on-chain balance
/// covers the declared collateral it flips the vault to `MintPending` and
/// enqueues its mint. This path does NOT depend on `fetch_block_numbers`, so a
/// block-height read failure (Layer-2 EVM-RPC consensus issue) never skips
/// deposit detection.
///
/// ## Burn watch (gated)
///
/// - If `last_observed_block[chain] == 0` (unseeded): log an activation hint and
///   SKIP burn-watch (no genesis crawl — there are no pre-activation events).
/// - Else fetch the finalized height via the consensus-safe specific-block probe
///   (`fetch_block_numbers`); on `Err`, log and skip burn-watch only (deposit
///   watch already ran). Run the `is_reorg` debounce; on confirmed reorg, halt
///   the chain (cursor not advanced). If `finalized > last_observed`, scan Burn
///   logs `last_observed+1 ..= finalized` and for each: (1) skip it if its
///   identity key is already in `processed_burn_keys` (idempotent — already
///   applied on a prior tick), (2) read the current foreign-chain debt total
///   (before this burn), (3) `mutate_state(apply_burn_to_state)`. On `Ok`:
///   record the key + emit `ChainBurnObserved`. On `InvalidBurn` (permanent —
///   unknown vault / over-repay): WARN-log, record the key as a permanent skip,
///   and CONTINUE (the cursor must advance past poison). On `SupplyInvariant`
///   (halt-class): log + stop the range WITHOUT advancing the cursor or
///   recording the key (the un-halt re-scan re-attempts it).
///
///   The cursor advances to `finalized` UNLESS a halt-class failure stopped the
///   range. After a successful advance, `processed_burn_keys` is pruned of every
///   entry at `block <= finalized` (those blocks can never be re-scanned), so
///   the set stays bounded. This combination (persisted dedup + skip-invalid-
///   continue + halt-without-advance) is the C-1 supply-divergence fix: the
///   already-applied prefix is never re-applied on any re-scan, a poison burn
///   no longer stalls the cursor, and a genuine divergence still halts.
pub async fn run_observer(chain: ChainId) {
    // Re-entrancy guard (acquired BEFORE any other work): if a tick for this
    // chain is still in flight, skip this one entirely. The RAII guard releases
    // on the future completing (any return path), so the next tick re-acquires.
    let _guard = match OBSERVER_INFLIGHT.with(|s| {
        if s.borrow().contains(&chain) {
            None
        } else {
            s.borrow_mut().insert(chain);
            Some(ObserverGuard(chain))
        }
    }) {
        Some(g) => g,
        None => return, // a tick for this chain is already running; skip
    };

    // Guard: skip if in read-only mode, the invariant has halted, or this chain
    // is reorg-halted (Task 11). A reorg-halted chain stops BOTH the observer
    // and the settlement worker until `clear_reorg_halt` (Task 14) is called.
    let should_skip = read_state(|s| {
        s.mode == Mode::ReadOnly
            || s.multi_chain.invariant_halted
            || s.multi_chain.reorg_halted.get(&chain).copied().unwrap_or(false)
    });
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

    // ── Hot-wallet gas-balance refresh (Task 11) ─────────────────────────────
    //
    // Derive the settlement (minter) address and cache its native MON balance so
    // the submit-path gas gate (`hardening::hot_wallet_ok`) has data and the
    // Task-14 query surface can report it. Keeping the tECDSA + RPC cost here
    // (once per observer tick) avoids paying it on every settlement submit.
    // Tolerant of errors: a failed derive or balance read logs and continues —
    // it must NOT abort the observer (reads/burn-watch must still run).
    refresh_hot_wallet_balance(chain).await;

    // Read the burn-watch cursor BEFORE running deposit-watch. The deposit-watch
    // path is consensus-safe (balance-only) and runs every tick regardless of
    // the cursor; it only needs `last_observed` for the cosmetic
    // `DepositObserved.block_number` (a balance poll has no single tx/block).
    let last_observed = read_state(|s| {
        s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0)
    });

    // ── Deposit watch (open-then-verify, Task 12) — RUNS EVERY TICK ──────────
    //
    // Poll each AwaitingDeposit vault's custody-address native (MON) balance.
    // Once the on-chain balance covers the DECLARED collateral, flip the vault
    // AwaitingDeposit -> MintPending and enqueue its Mint op (icUSD is only ever
    // minted against a verified on-chain deposit — the CDP backing invariant).
    //
    // This is DECOUPLED from the block-height path (`fetch_block_numbers`): it
    // uses ONLY the consensus-safe `get_balance(addr, "latest")` read, never a
    // volatile block tag. A block-height read failure (Layer-2 EVM-RPC
    // consensus issue) must NOT skip deposit detection — that was the Gate-4
    // blocker on staging (the two early-returns below used to sit BEFORE this
    // loop). Borrow discipline: snapshot the small per-vault tuples under one
    // `read_state` BEFORE the await loop; never hold a state borrow across
    // `get_balance(...).await`.
    let now = ic_cdk::api::time();
    let awaiting: Vec<(u64, String, u128)> = read_state(|s| {
        s.multi_chain
            .chain_vaults
            .values()
            .filter(|v| {
                v.collateral_chain == chain && v.status == ChainVaultStatus::AwaitingDeposit
            })
            .map(|v| (v.vault_id, v.custody_address.clone(), v.collateral_amount_e18))
            .collect()
    });

    for (vault_id, custody_address, declared_e18) in awaiting {
        let balance = match get_balance(chain, &custody_address).await {
            Ok(bal) => bal,
            Err(e) => {
                // A failed balance read must NOT abort the observer — log and
                // move on; the next tick retries this vault.
                log!(
                    INFO,
                    "[observer chain={:?}] deposit get_balance failed for vault {} ({}): {}; will retry",
                    chain, vault_id, custody_address, e
                );
                continue;
            }
        };

        if balance < declared_e18 {
            // Not enough on-chain collateral yet; nothing to do this tick.
            continue;
        }

        let transitioned = mutate_state(|s| {
            verify_deposit_and_enqueue_mint_in_state(&mut s.multi_chain, vault_id, balance, now)
        });

        match transitioned {
            Ok(true) => {
                crate::storage::record_event(&crate::event::Event::DepositObserved {
                    chain_id: chain,
                    vault_id,
                    custody_address: custody_address.clone(),
                    amount_e18: balance,
                    // Balance-poll observation, not a single transfer tx — there
                    // is no one tx hash to attribute the deposit to. The block
                    // number is cosmetic; use the current cursor (`last_observed`)
                    // since deposit-watch does not read a fresh block height.
                    tx_hash: String::new(),
                    block_number: last_observed,
                    timestamp: now,
                });
                log!(
                    INFO,
                    "[observer chain={:?}] deposit verified: vault={} custody={} balance_e18={} >= declared_e18={}; mint enqueued",
                    chain, vault_id, custody_address, balance, declared_e18
                );
            }
            Ok(false) => {
                // Idempotent no-op (already transitioned by an earlier tick, or a
                // concurrent transition). Nothing to emit.
            }
            Err(e) => {
                log!(
                    INFO,
                    "[observer chain={:?}] verify_deposit_and_enqueue_mint FAILED for vault {}: {:?}; will retry",
                    chain, vault_id, e
                );
            }
        }
    }

    // ── Burn watch — GATED on a seeded cursor + new blocks ───────────────────
    //
    // Everything below depends on a finalized block height. It is gated so a
    // block-read failure (or an unseeded chain) degrades the observer to
    // deposit-only — deposit-watch already ran above, so deposits still flow.

    // Unseeded sentinel: `last_observed == 0` means the burn-watch cursor was
    // never seeded to the chain tip. We do NOT crawl from genesis (no
    // pre-activation events exist). Log the activation hint and skip burn-watch.
    // (Logging every tick is intentional — staging needs this signal until the
    // operator seeds the cursor.)
    if last_observed == 0 {
        log!(
            INFO,
            "[observer chain={:?}] last_observed_block is 0 (unseeded); burn-watch inactive — call set_last_observed_block(chain, <current tip>) to activate",
            chain
        );
        return;
    }

    // Fetch finalized block number (consensus-safe specific-block probe). A
    // failure logs and skips burn-watch ONLY — deposit-watch already ran, so we
    // must NOT abort the whole tick.
    let finalized = match fetch_block_numbers(chain).await {
        Ok((_latest, fin)) => fin,
        Err(e) => {
            log!(INFO, "[observer chain={:?}] fetch_block_numbers failed: {}; skipping burn-watch this tick (deposit-watch ran)", chain, e);
            return;
        }
    };

    // ── Reorg check (Task 11) ────────────────────────────────────────────────
    //
    // A finalized-block regression deeper than this chain's `finality_depth` is
    // SUSPECTED to be a reorg past finality. But `fetch_block_numbers` queries
    // ONE provider at a time (no quorum), so a single stale/lagging read could
    // regress the finalized block transiently. We therefore require the
    // suspicion to PERSIST across `hardening::REORG_CONFIRM_TICKS` consecutive
    // observer ticks before halting; a single non-suspect tick resets the streak
    // (`hardening::on_reorg_tick`), so a transient blip self-heals. Only on the
    // K-th consecutive suspect tick do we halt the chain (observer + settlement)
    // and emit ChainReorgDetected; the cursor is NOT advanced. Recovery is
    // operator-gated via `clear_reorg_halt` (Task 14), which MUST reset BOTH
    // `reorg_halted` AND `reorg_suspect_streak` for the chain. Default depth to
    // the Monad testnet value (1) if the config is somehow unreadable.
    let finality_depth = read_state(|s| {
        s.multi_chain.chain_configs.get(&chain).map(|c| c.finality_depth)
    })
    .unwrap_or(1);
    let suspected = hardening::is_reorg(last_observed, finalized, finality_depth);
    let streak = read_state(|s| {
        s.multi_chain.reorg_suspect_streak.get(&chain).copied().unwrap_or(0)
    });
    let (new_streak, should_halt) = hardening::on_reorg_tick(streak, suspected);
    mutate_state(|s| {
        s.multi_chain.reorg_suspect_streak.insert(chain, new_streak);
    });

    if should_halt {
        let depth = last_observed.saturating_sub(finalized);
        mutate_state(|s| {
            s.multi_chain.reorg_halted.insert(chain, true);
        });
        crate::storage::record_event(&crate::event::Event::ChainReorgDetected {
            chain_id: chain,
            observed_block: finalized,
            reorg_depth: depth,
            timestamp: ic_cdk::api::time(),
        });
        log!(
            INFO,
            "[observer chain={:?}] REORG CONFIRMED ({}/{} ticks): finalized {} < last_observed {} by {} (> finality {}); halting chain",
            chain, new_streak, hardening::REORG_CONFIRM_TICKS, finalized, last_observed, depth, finality_depth
        );
        return;
    } else if suspected {
        // Below the confirmation threshold: do NOT halt and do NOT advance the
        // cursor (finalized < last_observed means there is nothing new to scan
        // anyway). Wait for the next tick to confirm or clear the suspicion.
        log!(
            INFO,
            "[observer chain={:?}] suspected reorg, streak {}/{}, not halting yet (finalized {} < last_observed {})",
            chain, new_streak, hardening::REORG_CONFIRM_TICKS, finalized, last_observed
        );
        return;
    }
    // Not suspected: the streak was reset to 0 above, so a real reorg needs K
    // CONSECUTIVE suspect ticks. Fall through to the normal nothing-new check.

    if finalized <= last_observed {
        // Nothing new to observe (burn-watch). Deposit-watch already ran.
        return;
    }

    let from_block = last_observed + 1;

    let raw_burn_logs = match get_logs(chain, &contract, BURN_EVENT_TOPIC0, from_block, finalized).await {
        Ok(logs) => logs,
        Err(e) => {
            log!(INFO, "[observer chain={:?}] get_logs(burn) failed: {}; will retry on next tick", chain, e);
            return;
        }
    };

    // ── Per-burn handling: dedup + skip-poison-and-continue (C-1) ────────────
    //
    // `burn_ok` now means ONLY "no halt-class failure occurred" — it gates the
    // cursor advance. It is NOT cleared by a skippable burn (decode failure or
    // InvalidBurn): those advance past their offending log so a single poison
    // burn can never stall the cursor (the silent-double-apply trigger).
    //
    // Idempotency: every burn carries an on-chain identity key
    // (`{tx_hash}:{vault_id}:{amount_e8s}`) recorded in `processed_burn_keys`
    // once handled (applied OR permanently skipped). On any re-scan of the same
    // range, an already-keyed burn is `continue`-d BEFORE `apply_burn_to_state`,
    // so the already-applied prefix is NEVER re-applied — this is the core fix
    // for the C-1 supply-divergence (debt_e8s + chain_supplies double-decrement).
    let mut burn_ok = true;
    for (topics, data, tx_hash, block_number) in &raw_burn_logs {
        let burn = match decode_burn_log(topics, data, tx_hash, *block_number) {
            Ok(b) => b,
            Err(e) => {
                // SKIP, do not break: in production `get_logs` is topic-filtered
                // by the real RPC so only Burn logs arrive; a decode failure here
                // is genuinely anomalous (malformed log) and stalling the cursor
                // on it would re-introduce the C-1 stall. Log and move past it.
                // (We cannot dedup-key an undecodable log — it has no parsed
                // identity — but it is topic-filtered out on re-scan in
                // production, and even if re-seen it just re-skips harmlessly.)
                log!(INFO, "[observer chain={:?}] decode_burn_log failed at block {}: {}; skipping (not stalling cursor)", chain, block_number, e);
                continue;
            }
        };

        // Idempotency key: tx_hash uniquely identifies the tx; vault_id+amount
        // disambiguate the (unlikely) multi-burn tx. If already processed at this
        // block, this burn's debt/supply decrement already happened — skip it so
        // a re-scan never double-applies.
        let key = format!("{}:{}:{}", burn.tx_hash, burn.vault_id, burn.amount_e8s);
        let already_processed = read_state(|s| {
            s.multi_chain
                .processed_burn_keys
                .get(&burn.block_number)
                .map(|set| set.contains(&key))
                .unwrap_or(false)
        });
        if already_processed {
            log!(
                INFO,
                "[observer chain={:?}] burn already processed (dedup): vault={} amount_e8s={} block={} tx={}; skipping",
                chain, burn.vault_id, burn.amount_e8s, burn.block_number, burn.tx_hash
            );
            continue;
        }

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
                // Record the dedup key. The entire burn loop runs synchronously
                // (no `.await` between the apply above and here, nor across loop
                // iterations), so the apply and this record commit in the SAME
                // atomic message slice — a trap rolls BOTH back together. Thus
                // the invariant "key present iff debt/supply already decremented"
                // always holds, and a re-scan can never re-apply a recorded burn.
                mutate_state(|s| {
                    s.multi_chain
                        .processed_burn_keys
                        .entry(burn.block_number)
                        .or_default()
                        .insert(key.clone());
                });
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
            Err(BurnApplyError::InvalidBurn(msg)) => {
                // PERMANENT-INVALID (unknown vault / over-repay). It can never
                // succeed, so record its key as a PERMANENT SKIP and continue —
                // the cursor advances past it. This is what stops a single
                // poison burn from stalling the cursor (and thus stops the
                // re-scan that silently double-applied the good prefix).
                log!(
                    INFO,
                    "[observer chain={:?}] skipping invalid burn (vault={} amount_e8s={} block={} tx={}): {}",
                    chain, burn.vault_id, burn.amount_e8s, burn.block_number, burn.tx_hash, msg
                );
                mutate_state(|s| {
                    s.multi_chain
                        .processed_burn_keys
                        .entry(burn.block_number)
                        .or_default()
                        .insert(key.clone());
                });
                continue;
            }
            Err(BurnApplyError::SupplyInvariant(e)) => {
                // HALT-CLASS (underflow / divergence / already-halted): do NOT
                // advance the cursor and do NOT record the key, so the un-halt
                // re-scan re-attempts this burn. Stop the range here.
                log!(
                    INFO,
                    "[observer chain={:?}] apply_burn_to_state HALT-CLASS failure for tx {} vault {}: {:?}; not advancing cursor",
                    chain, burn.tx_hash, burn.vault_id, e
                );
                burn_ok = false;
                break;
            }
        }
    }

    // ── Advance cursor (only when no halt-class failure occurred) ────────────
    //
    // `burn_ok` is true unless a SupplyInvariant (halt-class) failure broke the
    // loop. Skippable failures (decode / InvalidBurn) leave it true so the
    // cursor advances past the poison.
    if burn_ok {
        mutate_state(|s| {
            s.multi_chain.last_observed_block.insert(chain, finalized);
            // Prune the idempotency set of entries the next scan can never
            // re-reach. The next scan starts at `finalized + 1`, so any block
            // <= finalized is permanently behind the cursor and its keys are no
            // longer needed for dedup. This keeps `processed_burn_keys` bounded.
            // On a halt-break (above), we DO NOT prune — the un-halt re-scan
            // restarts from the same `last_observed + 1` and must stay idempotent.
            let stale: Vec<u64> = s
                .multi_chain
                .processed_burn_keys
                .range(..=finalized)
                .map(|(&block, _)| block)
                .collect();
            for block in stale {
                s.multi_chain.processed_burn_keys.remove(&block);
            }
        });
    }
}

/// Refresh the cached settlement-address MON balance for `chain` (Task 11).
///
/// Derives the settlement (minter) address via `settlement_derivation_path` +
/// `derive_evm_address`, reads its native balance via `get_balance`, and caches
/// it in `hot_wallet_balance_e18`. Used by the submit-path gas gate and the
/// Task-14 query surface. Errors are logged and swallowed — a failed refresh
/// leaves the previous cached value in place (or, on a fresh chain, leaves the
/// cache unpopulated, which the gas gate treats as fail-open). Borrow
/// discipline: no `read_state`/`mutate_state` borrow is held across an `.await`.
async fn refresh_hot_wallet_balance(chain: ChainId) {
    // Resolve the settlement address via the per-chain cache (Task 11 M1) — the
    // address is deterministic, so we avoid a tECDSA derive on every observer
    // tick. We only need the address here (not the path).
    let addr = match tecdsa::cached_settlement_address(chain).await {
        Ok((_path, addr)) => addr,
        Err(e) => {
            log!(INFO, "[observer chain={:?}] hot-wallet cached_settlement_address failed: {}; skipping balance refresh", chain, e);
            return;
        }
    };
    match get_balance(chain, &addr).await {
        Ok(bal) => {
            mutate_state(|s| {
                s.multi_chain.hot_wallet_balance_e18.insert(chain, bal);
            });
            if !hardening::hot_wallet_ok(bal) {
                log!(
                    INFO,
                    "[observer chain={:?}] hot-wallet balance {} e18 below threshold {} e18 (settlement={})",
                    chain, bal, hardening::HOT_WALLET_MIN_E18, addr
                );
            }
        }
        Err(e) => {
            log!(INFO, "[observer chain={:?}] hot-wallet get_balance failed: {}; keeping cached value", chain, e);
        }
    }
}
