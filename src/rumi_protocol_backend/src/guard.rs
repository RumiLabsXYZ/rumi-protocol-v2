use crate::state::mutate_state;
use candid::Principal;
use std::marker::PhantomData;
use ic_cdk::api::time;
use ic_canister_log::log;

const MAX_CONCURRENT: usize = 100;

// Add a timeout duration for guards
const GUARD_TIMEOUT_NANOS: u64 = 5 * 60 * 1_000_000_000; // 5 minutes in nanoseconds

// Track operation state
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum OperationState {
    #[default]
    InProgress,
    Completed,
    Failed,
}

/// Guards a block from executing twice when called by the same user and from being
/// executed [MAX_CONCURRENT] or more times in parallel.
#[must_use]
pub struct GuardPrincipal {
    principal: Principal,
    _created_at: u64,
    _operation_name: String,
    _marker: PhantomData<GuardPrincipal>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GuardError {
    AlreadyProcessing,
    TooManyConcurrentRequests,
    StaleOperation,
}

impl GuardPrincipal {
    /// Attempts to create a new guard for the current block. Fails if there is
    /// already a pending request for the specified principal or if there
    /// are at least [MAX_CONCURRENT] pending requests.
    pub fn new(principal: Principal, operation_name: &str) -> Result<Self, GuardError> {
        mutate_state(|s| {
            let current_time = time();

            // Clean up stale guards before processing new request
            let mut stale_principals = Vec::new();
            for guard_principal in s.principal_guards.iter() {
                if let Some(timestamp) = s.principal_guard_timestamps.get(guard_principal) {
                    if current_time.saturating_sub(*timestamp) > GUARD_TIMEOUT_NANOS {
                        if let Some(op_name) = s.operation_names.get(guard_principal) {
                            log!(crate::INFO,
                                "[guard] Removing stale operation: {} for principal: {} (age: {}s)",
                                op_name,
                                guard_principal.to_string(),
                                current_time.saturating_sub(*timestamp) / 1_000_000_000
                            );
                        }
                        stale_principals.push(*guard_principal);
                    }

                    // Also check for operations marked as failed
                    if let Some(state) = s.operation_states.get(guard_principal) {
                        if *state == OperationState::Failed {
                            stale_principals.push(*guard_principal);
                        }
                    }
                } else {
                    stale_principals.push(*guard_principal);
                }
            }

            // Remove stale guards
            for p in stale_principals {
                s.principal_guards.remove(&p);
                s.principal_guard_timestamps.remove(&p);
                s.operation_states.remove(&p);
                s.operation_names.remove(&p);
            }

            // Check if this principal already has a guard
            if s.principal_guards.contains(&principal) {
                let op_name = s.operation_names.get(&principal)
                    .cloned()
                    .unwrap_or_default();
                let timestamp = s.principal_guard_timestamps.get(&principal)
                    .copied()
                    .unwrap_or_default();
                let age_seconds = current_time.saturating_sub(timestamp) / 1_000_000_000;

                if age_seconds > (GUARD_TIMEOUT_NANOS / 1_000_000_000) / 2 {
                    log!(crate::INFO,
                        "[guard] Operation '{}' for principal {} is stale ({}s old), allowing new request",
                        op_name, principal.to_string(), age_seconds
                    );
                    s.principal_guards.remove(&principal);
                    s.principal_guard_timestamps.remove(&principal);
                    s.operation_states.remove(&principal);
                    s.operation_names.remove(&principal);
                } else {
                    log!(crate::INFO,
                        "[guard] Operation '{}' for principal {} is already in progress ({}s old)",
                        op_name, principal.to_string(), age_seconds
                    );
                    return Err(GuardError::AlreadyProcessing);
                }
            }

            if s.principal_guards.len() >= MAX_CONCURRENT {
                return Err(GuardError::TooManyConcurrentRequests);
            }

            // Add the guard
            s.principal_guards.insert(principal);
            s.principal_guard_timestamps.insert(principal, current_time);
            s.operation_states.insert(principal, OperationState::InProgress);
            s.operation_names.insert(principal, operation_name.to_string());

            log!(crate::INFO,
                "[guard] Created new guard for principal {} operation '{}'",
                principal.to_string(), operation_name
            );

            Ok(Self {
                principal,
                _created_at: current_time,
                _operation_name: operation_name.to_string(),
                _marker: PhantomData,
            })
        })
    }

    /// Mark this operation as complete
    pub fn complete(self) {
        mutate_state(|s| {
            if let Some(state) = s.operation_states.get_mut(&self.principal) {
                *state = OperationState::Completed;
            }
        });
    }

    /// Mark this operation as failed
    pub fn fail(self) {
        mutate_state(|s| {
            if let Some(state) = s.operation_states.get_mut(&self.principal) {
                *state = OperationState::Failed;
            }
        });
    }
}

impl Drop for GuardPrincipal {
    fn drop(&mut self) {
        // Always release the guard when the struct goes out of scope.
        // The guard exists to prevent concurrent access during an operation;
        // once the Rust function returns (success or failure), the lock must be freed.
        mutate_state(|s| {
            s.principal_guards.remove(&self.principal);
            s.principal_guard_timestamps.remove(&self.principal);
            s.operation_states.remove(&self.principal);
            s.operation_names.remove(&self.principal);
        });
    }
}

thread_local! {
    /// Vault ids with a vault-mutating operation (liquidation OR owner
    /// write-op) currently in flight across an `await`. Transient (heap):
    /// in-flight operations never span a canister upgrade, and ic-cdk's
    /// `call_on_cleanup` runs this guard's `Drop` even when a post-`await`
    /// continuation traps, so the entry is always released. Same pattern the
    /// 3pool/amm `PoolGuard`s use.
    static LIQUIDATING_VAULTS: std::cell::RefCell<std::collections::HashSet<u64>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// True while a `VaultLiquidationGuard` is held for `vault_id`. Used by the
/// redemption water-fill (AR-B-001, audit 2026-06-09) to skip vaults that a
/// liquidation or owner write-op is mid-flight on: redemption is synchronous,
/// so by skipping locked vaults it can never interleave between another
/// operation's pre-`await` snapshot and its post-`await` commit.
pub fn is_vault_liquidating(vault_id: u64) -> bool {
    LIQUIDATING_VAULTS.with(|set| set.borrow().contains(&vault_id))
}

/// Per-vault operation lock. Serializes EVERY vault-mutating flow that spans
/// an `await` (any liquidator — two humans, the stability pool, a human + the
/// SP — and, since the 2026-06-09 audit, owner write-ops: borrow, repay,
/// partial-withdraw, add-margin, close) on a single vault across the
/// snapshot -> external-call -> re-cap -> payout/commit sequence.
///
/// BK-001/002 (audit 2026-06-05): `GuardPrincipal` keys on the CALLER, so two
/// different liquidators racing the same vault both pass it. Each snapshots the
/// vault's full collateral before its `await`; the post-`await` re-cap
/// (ASYNC-001) prevents the vault-accounting underflow/wrap, but the collateral
/// PAYOUT (`pending_margin_transfers`) still uses the stale per-caller snapshot,
/// so the loser is paid the full pre-state collateral out of the SHARED
/// collateral pool — draining other vaults' backing. Keying the lock on
/// `vault_id` (not the caller) makes the whole sequence atomic per vault and
/// closes the economic over-seize the re-cap alone left open.
///
/// AR-B-003 (audit 2026-06-09): the same race exists between a liquidation and
/// an OWNER write-op (repay / partial-withdraw / borrow), whose post-`await`
/// commits asserted the pre-`await` snapshot still held — trapping after an
/// irreversible transfer, or over-paying from the shared pool. Owner write-ops
/// now hold this lock too, so liquidation-vs-user-op interleavings on one vault
/// are excluded, and the redemption water-fill skips locked vaults
/// (`is_vault_liquidating`).
#[must_use]
pub struct VaultLiquidationGuard(u64);

impl VaultLiquidationGuard {
    /// Acquire the operation lock for `vault_id`. Returns
    /// `TemporarilyUnavailable` if another operation on the same vault is in
    /// flight; the caller should back off (the stability pool, per project
    /// rule, must NOT retry — it falls through to manual, which is correct).
    pub fn new(vault_id: u64) -> Result<Self, crate::ProtocolError> {
        LIQUIDATING_VAULTS.with(|set| {
            let mut set = set.borrow_mut();
            if set.contains(&vault_id) {
                return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                    "Another operation on vault #{vault_id} is in flight; retry shortly"
                )));
            }
            set.insert(vault_id);
            Ok(Self(vault_id))
        })
    }
}

impl Drop for VaultLiquidationGuard {
    fn drop(&mut self) {
        LIQUIDATING_VAULTS.with(|set| {
            set.borrow_mut().remove(&self.0);
        });
    }
}

thread_local! {
    /// Foreign-CHAIN vault ids with a liquidation in flight across an `await`
    /// (the bot swap: begin -> submit -> confirm). Separate id-space from the
    /// ICP `LIQUIDATING_VAULTS` (chain vault ids live in
    /// `MultiChainState.chain_vaults`, keyed independently). Transient/heap;
    /// ic-cdk `call_on_cleanup` drops the guard even on a post-`await`
    /// continuation trap, so the lock is always released. The DURABLE lock is the
    /// persisted `pending_liquidation` marker (spec §10); this heap guard just
    /// serializes the async swap against concurrent triggers within one canister.
    static CHAIN_LIQUIDATING_VAULTS: std::cell::RefCell<std::collections::HashSet<u64>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Per-vault lock for a foreign-chain liquidation held across the async swap
/// (spec §10). Mirrors `VaultLiquidationGuard` but on the chain vault id-space.
#[must_use]
pub struct ChainVaultLiquidationGuard(u64);

impl ChainVaultLiquidationGuard {
    /// Acquire the chain-liquidation lock for `vault_id`. Returns
    /// `TemporarilyUnavailable` if another liquidation on the same chain vault is
    /// in flight (the caller must NOT retry — per the no-retry rule it falls
    /// through to the next tier/manual).
    pub fn new(vault_id: u64) -> Result<Self, crate::ProtocolError> {
        CHAIN_LIQUIDATING_VAULTS.with(|set| {
            let mut set = set.borrow_mut();
            if set.contains(&vault_id) {
                return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                    "Another liquidation on chain vault #{vault_id} is in flight; retry shortly"
                )));
            }
            set.insert(vault_id);
            Ok(Self(vault_id))
        })
    }
}

impl Drop for ChainVaultLiquidationGuard {
    fn drop(&mut self) {
        CHAIN_LIQUIDATING_VAULTS.with(|set| {
            set.borrow_mut().remove(&self.0);
        });
    }
}

thread_local! {
    /// In-flight borrow reservations per collateral type (icUSD e8s). A borrow
    /// checks the debt ceiling / global mint cap, then mints icUSD across an
    /// `await`, then records the debt. Concurrent borrows from DIFFERENT owners
    /// both pass the pre-await check against the same committed aggregate and
    /// both mint, blowing past the cap (BK-003). This map bridges the gap: a
    /// borrow reserves its amount here (counted by every concurrent borrow's
    /// cap check) before minting, and releases on Drop. Transient/heap; ic-cdk
    /// `call_on_cleanup` drops the guard on continuation-trap, so reservations
    /// never leak.
    static BORROW_RESERVATIONS: std::cell::RefCell<std::collections::HashMap<Principal, u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Atomically check-and-reserve borrow headroom against in-flight reservations.
///
/// BK-003 (audit 2026-06-05): enforces the per-collateral debt ceiling and the
/// global icUSD mint cap across the mint `await`, so two distinct owners cannot
/// each pass the pre-await check and jointly exceed the cap.
#[must_use]
pub struct BorrowReservationGuard {
    collateral: Principal,
    amount: u64,
}

impl BorrowReservationGuard {
    /// Reserve `amount` for `collateral` iff, counting all in-flight
    /// reservations, the per-collateral ceiling and the global mint cap both
    /// still hold. The committed aggregates (`current_collateral_debt`,
    /// `current_global_borrowed`) are read by the caller and passed in. Returns
    /// `Err` (no reservation taken) when a concurrent in-flight borrow has
    /// already consumed the headroom.
    pub fn try_reserve(
        collateral: Principal,
        amount: u64,
        current_collateral_debt: u64,
        debt_ceiling: u64,
        current_global_borrowed: u64,
        global_cap: u64,
    ) -> Result<Self, String> {
        BORROW_RESERVATIONS.with(|r| {
            let mut map = r.borrow_mut();
            let coll_reserved: u64 = map.get(&collateral).copied().unwrap_or(0);
            let total_reserved: u64 = map.values().copied().sum();

            if current_collateral_debt
                .saturating_add(coll_reserved)
                .saturating_add(amount)
                > debt_ceiling
            {
                return Err(format!(
                    "Borrow would exceed debt ceiling incl in-flight ({} + {} + {} > {})",
                    current_collateral_debt, coll_reserved, amount, debt_ceiling
                ));
            }
            if current_global_borrowed
                .saturating_add(total_reserved)
                .saturating_add(amount)
                > global_cap
            {
                return Err(format!(
                    "Borrow would exceed global icUSD mint cap incl in-flight ({} + {} + {} > {})",
                    current_global_borrowed, total_reserved, amount, global_cap
                ));
            }

            *map.entry(collateral).or_insert(0) += amount;
            Ok(Self { collateral, amount })
        })
    }
}

impl Drop for BorrowReservationGuard {
    fn drop(&mut self) {
        BORROW_RESERVATIONS.with(|r| {
            let mut map = r.borrow_mut();
            if let Some(v) = map.get_mut(&self.collateral) {
                *v = v.saturating_sub(self.amount);
                if *v == 0 {
                    map.remove(&self.collateral);
                }
            }
        });
    }
}

#[cfg(test)]
mod vault_liquidation_guard_tests {
    use super::*;

    #[test]
    fn vault_liquidation_guard_is_exclusive_per_vault() {
        // BK-001/002 fence: the lock is per-vault, not per-caller.
        let g1 = VaultLiquidationGuard::new(42).expect("first acquire for vault 42");
        // A second liquidator (any caller) racing the SAME vault is rejected.
        assert!(
            VaultLiquidationGuard::new(42).is_err(),
            "second concurrent liquidation of vault 42 must be rejected",
        );
        // A different vault is independent and acquires concurrently.
        let g2 = VaultLiquidationGuard::new(43).expect("different vault acquires independently");
        drop(g1);
        // Once vault 42's liquidation finishes (guard dropped), it can be re-acquired.
        let _g3 = VaultLiquidationGuard::new(42).expect("re-acquire vault 42 after release");
        drop(g2);
    }

    #[test]
    fn chain_vault_liquidation_guard_is_exclusive_and_independent_of_icp() {
        let g1 = ChainVaultLiquidationGuard::new(7).expect("first acquire chain vault 7");
        assert!(
            ChainVaultLiquidationGuard::new(7).is_err(),
            "second concurrent chain liquidation of vault 7 must be rejected",
        );
        // Separate id-space from the ICP guard: the same id locks independently.
        let _icp = VaultLiquidationGuard::new(7).expect("ICP guard for id 7 is independent");
        drop(g1);
        let _g2 = ChainVaultLiquidationGuard::new(7).expect("re-acquire chain vault 7 after release");
    }

    #[test]
    fn borrow_reservation_enforces_ceiling_across_concurrent_inflight() {
        // BK-003 fence: a second in-flight borrow that would push committed debt
        // + in-flight reservations over the ceiling is rejected, even though the
        // committed aggregate alone still has headroom.
        let coll = Principal::from_slice(&[7]);
        let ceiling = 1_000u64;
        let global = u64::MAX;

        // Committed debt 600, ceiling 1000. First borrow of 300 reserves -> 900 headroom used.
        let g1 = BorrowReservationGuard::try_reserve(coll, 300, 600, ceiling, 600, global)
            .expect("first in-flight borrow fits (600+0+300<=1000)");
        // Second concurrent borrow of 300 sees committed 600 + reserved 300 + 300 = 1200 > 1000 -> rejected.
        assert!(
            BorrowReservationGuard::try_reserve(coll, 300, 600, ceiling, 600, global).is_err(),
            "second concurrent borrow must be rejected once in-flight reservations are counted",
        );
        // A smaller second borrow that fits (600+300+100=1000) is allowed.
        let _g2 = BorrowReservationGuard::try_reserve(coll, 100, 600, ceiling, 600, global)
            .expect("second borrow of 100 fits exactly at the ceiling");
        drop(g1);
        // After the first releases, headroom frees up again.
        let _g3 = BorrowReservationGuard::try_reserve(coll, 300, 600, ceiling, 600, global)
            .expect("headroom freed after first reservation dropped");
    }

    #[test]
    fn borrow_reservation_enforces_global_cap() {
        // Different collaterals still share the global mint cap.
        let c1 = Principal::from_slice(&[1]);
        let c2 = Principal::from_slice(&[2]);
        let big_ceiling = u64::MAX;
        let global = 1_000u64;
        let g1 = BorrowReservationGuard::try_reserve(c1, 700, 0, big_ceiling, 0, global)
            .expect("first global reservation fits");
        assert!(
            BorrowReservationGuard::try_reserve(c2, 400, 0, big_ceiling, 0, global).is_err(),
            "second collateral's borrow must respect the shared global cap (0+700+400>1000)",
        );
        drop(g1);
    }
}

#[must_use]
pub struct TimerLogicGuard(());

impl TimerLogicGuard {
    pub fn new() -> Option<Self> {
        mutate_state(|s| {
            if s.is_timer_running {
                return None;
            }
            s.is_timer_running = true;
            Some(TimerLogicGuard(()))
        })
    }
}

impl Drop for TimerLogicGuard {
    fn drop(&mut self) {
        mutate_state(|s| {
            s.is_timer_running = false;
        });
    }
}

#[must_use]
pub struct FetchXrcGuard(());

impl FetchXrcGuard {
    pub fn new() -> Option<Self> {
        mutate_state(|s| {
            if s.is_fetching_rate {
                return None;
            }
            s.is_fetching_rate = true;
            Some(FetchXrcGuard(()))
        })
    }
}

impl Drop for FetchXrcGuard {
    fn drop(&mut self) {
        mutate_state(|s| {
            s.is_fetching_rate = false;
        });
    }
}

thread_local! {
    /// Protocol-wide SOL settlement danger-zone marker (security review fix,
    /// 2026-07-24). Stores the acquisition timestamp, or `None` when free.
    ///
    /// Every `SolClaim`, regardless of which vault or claimant it belongs to,
    /// settles against ONE shared durable-nonce account (design doc §5.1).
    /// Concretely that means genuine parallelism between two DIFFERENT
    /// claimants settling two DIFFERENT claims was always illusory: Solana
    /// itself only ever lands one of their signed transactions, because a
    /// durable nonce is single-use. Before this guard, nothing in the
    /// canister's OWN bookkeeping reflected that fact: `GuardPrincipal` is
    /// keyed on the CALLER principal and `VaultLiquidationGuard` is keyed on
    /// the source vault (`custody_nonce`), so two different claimants racing
    /// settlement on two different vaults pass BOTH locks and both proceed
    /// concurrently. Every one of them then reads the same live nonce,
    /// reconciles (finding no sibling settlement yet, since none has written
    /// one), signs, and submits. Solana lands exactly one (that part stays
    /// safe, nonce uniqueness rules out a double-pay). But every loser's retry
    /// lands on `AmbiguousAdvanced`; only the FIRST loser to retry still finds
    /// the winner's `Confirmed` sibling and clears itself cleanly, because
    /// `reconcile_sol_other_inflight_claims`'s `Paid` handling then REMOVES
    /// the winning claim (so the evidence that would have explained the
    /// REMAINING losers' ambiguity is gone by the time they retry), and they
    /// are quarantined pending `admin_resolve_sol_claim`. A single routine
    /// Stability Pool absorption can fan out to `MAX_SOL_SP_PAYOUT_ALLOCATIONS`
    /// claimants, so this race can strand most of a whole liquidation's
    /// depositors behind manual admin action.
    ///
    /// `SolSettlementInflightGuard` closes that gap by serializing the whole
    /// danger zone (read the live nonce, reconcile siblings, sign, persist the
    /// `SolSettlement`, submit) protocol-wide: at most one `settle_sol_claim`
    /// call may be inside it at a time, across every claim and every vault. A
    /// second concurrent caller is turned away with a clear, retryable
    /// `TemporarilyUnavailable` before it ever reads the nonce or signs
    /// anything (never a quarantine, never a signature). The justification is
    /// simply that the canister's bookkeeping should agree with the reality
    /// already imposed by the Solana layer, rather than race it.
    ///
    /// Transient/heap, not persisted state: in-flight settlement attempts
    /// never span a canister upgrade. Self-healing, mirroring
    /// `chains::solana::settlement::SOLANA_SETTLEMENT_INFLIGHT`: a trap in a
    /// post-`await` continuation does not run `Drop` on the IC, so without a
    /// staleness check a crashed settlement would wedge the rail forever.
    /// `chains::solana::hardening::inflight_should_acquire` (reused verbatim,
    /// not reimplemented) reclaims an entry older than
    /// `chains::solana::hardening::INFLIGHT_STALE_NS` (10 minutes).
    static SOL_SETTLEMENT_INFLIGHT: std::cell::RefCell<Option<u64>> =
        const { std::cell::RefCell::new(None) };
}

/// RAII guard for the protocol-wide SOL settlement danger zone. See
/// `SOL_SETTLEMENT_INFLIGHT`'s doc comment for the cross-claimant fan-out race
/// this closes. `Drop` unconditionally clears the marker, so it releases on
/// EVERY exit path out of `settle_sol_claim` (including every early `return`
/// on an error branch), without the caller needing to remember to release it
/// manually (unlike `GuardPrincipal`, which requires an explicit `.fail()` /
/// `.complete()` call on each exit path).
#[must_use]
pub struct SolSettlementInflightGuard(());

impl SolSettlementInflightGuard {
    /// Acquire the guard. `now_ns` is passed in by the caller (rather than
    /// this function calling `ic_cdk::api::time()` itself) so the acquire
    /// decision stays a pure, deterministic function of its inputs and is
    /// directly unit-testable, including the staleness self-heal, without a
    /// canister call context.
    ///
    /// Returns `TemporarilyUnavailable` if another `settle_sol_claim` call is
    /// already inside the danger zone and its entry is not yet stale.
    pub fn new(now_ns: u64) -> Result<Self, crate::ProtocolError> {
        SOL_SETTLEMENT_INFLIGHT.with(|g| {
            let mut slot = g.borrow_mut();
            if crate::chains::solana::hardening::inflight_should_acquire(
                *slot,
                now_ns,
                crate::chains::solana::hardening::INFLIGHT_STALE_NS,
            ) {
                *slot = Some(now_ns);
                Ok(Self(()))
            } else {
                Err(crate::ProtocolError::TemporarilyUnavailable(
                    "another SOL settlement is in flight, try again shortly".to_string(),
                ))
            }
        })
    }

    /// Test-only introspection: whether the marker is currently held. Lets
    /// tests assert the guard's post-drop state without depending on
    /// `settle_sol_claim` itself, which cannot be driven outside a canister
    /// call context (see `sol_p2b_tests`'s module doc comment).
    #[cfg(test)]
    pub(crate) fn is_held_for_test() -> bool {
        SOL_SETTLEMENT_INFLIGHT.with(|g| g.borrow().is_some())
    }
}

impl Drop for SolSettlementInflightGuard {
    fn drop(&mut self) {
        SOL_SETTLEMENT_INFLIGHT.with(|g| {
            *g.borrow_mut() = None;
        });
    }
}

#[cfg(test)]
mod sol_settlement_inflight_guard_tests {
    use super::*;

    /// Fix-1 fence: a second concurrent settlement attempt must be refused
    /// with the retryable error, never silently allowed to sign alongside the
    /// first. Mirrors `vault_liquidation_guard_is_exclusive_per_vault` but for
    /// the canister-wide (not per-vault) SOL settlement lock.
    #[test]
    fn blocks_a_second_concurrent_acquire_and_returns_the_retryable_error() {
        // Guarantee a clean slate: other tests in this binary share the same
        // thread_local (single-threaded test binary, but order is undefined).
        SOL_SETTLEMENT_INFLIGHT.with(|g| *g.borrow_mut() = None);

        let g1 = SolSettlementInflightGuard::new(1_000).expect("first acquire is free");
        assert!(SolSettlementInflightGuard::is_held_for_test());

        let second = SolSettlementInflightGuard::new(1_001);
        match second {
            Err(crate::ProtocolError::TemporarilyUnavailable(msg)) => {
                assert!(
                    msg.contains("in flight"),
                    "error message should be clearly retryable, got: {msg}"
                );
            }
            Err(other) => panic!("expected TemporarilyUnavailable, got {other:?}"),
            Ok(_) => panic!("a second concurrent settlement must NOT be allowed to acquire"),
        }

        drop(g1);
        assert!(
            !SolSettlementInflightGuard::is_held_for_test(),
            "dropping the first guard must release the marker"
        );
        let _g2 = SolSettlementInflightGuard::new(1_002).expect("free again after release");
    }

    /// Fix-1 fence: a crashed/trapped settlement (whose `Drop` never ran)
    /// must not wedge the rail forever. Once the held entry is older than
    /// `INFLIGHT_STALE_NS`, a fresh attempt reclaims it instead of being
    /// refused, exactly mirroring `chains::solana::hardening`'s self-heal.
    #[test]
    fn self_heals_after_the_staleness_window_so_a_crashed_settlement_cannot_wedge_the_rail() {
        SOL_SETTLEMENT_INFLIGHT.with(|g| *g.borrow_mut() = None);

        let acquired_at = 5_000_000_000_000u64;
        let g1 = SolSettlementInflightGuard::new(acquired_at).expect("first acquire is free");
        // Simulate the holder having trapped in a post-await continuation:
        // its `Drop` never runs, so leak the guard rather than dropping it.
        std::mem::forget(g1);
        assert!(SolSettlementInflightGuard::is_held_for_test());

        // Still fresh (just under the threshold): a new attempt is refused.
        let still_fresh = acquired_at + crate::chains::solana::hardening::INFLIGHT_STALE_NS - 1;
        assert!(
            SolSettlementInflightGuard::new(still_fresh).is_err(),
            "an entry younger than the staleness window must still block"
        );

        // Past the threshold: the previous (trapped) holder's entry is
        // reclaimed, self-healing the would-be-permanent block.
        let now_stale = acquired_at + crate::chains::solana::hardening::INFLIGHT_STALE_NS;
        let g2 = SolSettlementInflightGuard::new(now_stale)
            .expect("a stale entry must self-heal, not wedge the rail forever");
        drop(g2);
        // Clean up for other tests sharing this thread_local.
        SOL_SETTLEMENT_INFLIGHT.with(|g| *g.borrow_mut() = None);
    }
}
