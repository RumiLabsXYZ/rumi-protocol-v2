use crate::state::mutate_state;
use candid::Principal;
use std::marker::PhantomData;
use ic_cdk::api::time;
use ic_canister_log::log;

const MAX_CONCURRENT: usize = 100;

// Add a timeout duration for guards
const GUARD_TIMEOUT_NANOS: u64 = 5 * 60 * 1_000_000_000; // 5 minutes in nanoseconds

// Track operation state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperationState {
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
    operation_name: String,
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
                operation_name: operation_name.to_string(),
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
