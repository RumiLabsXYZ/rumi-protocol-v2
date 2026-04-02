// Admin functions for the Rumi 3pool.

use candid::Principal;

use crate::math::get_a;
use crate::state::{mutate_state, read_state};
use crate::transfers::transfer_to_user;
use crate::types::{ThreePoolError, ThreePoolAdminEvent, ThreePoolAdminAction};

/// Minimum time for an A parameter ramp (1 day in seconds).
const MIN_RAMP_TIME: u64 = 86400;

/// Maximum A change factor per ramp (10x in either direction).
const MAX_A_CHANGE: u64 = 10;

/// Start ramping the amplification coefficient toward `future_a` over time.
pub fn ramp_a(
    future_a: u64,
    future_a_time: u64,
    caller: Principal,
    now: u64,
) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }

    // Get current effective A
    let current_a = read_state(|s| {
        get_a(
            s.config.initial_a,
            s.config.future_a,
            s.config.initial_a_time,
            s.config.future_a_time,
            now,
        )
    });

    // Validate timing: must be at least MIN_RAMP_TIME in the future
    if future_a_time < now + MIN_RAMP_TIME {
        return Err(ThreePoolError::Unauthorized); // reuse Unauthorized for invalid params
    }

    // Validate magnitude: at most 10x change in either direction
    if future_a > current_a * MAX_A_CHANGE || future_a * MAX_A_CHANGE < current_a {
        return Err(ThreePoolError::Unauthorized);
    }

    if future_a == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }

    mutate_state(|s| {
        s.config.initial_a = current_a;
        s.config.future_a = future_a;
        s.config.initial_a_time = now;
        s.config.future_a_time = future_a_time;

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::RampA { future_a, future_a_time },
        });
    });

    Ok(())
}

/// Stop an in-progress A ramp, freezing A at its current value.
pub fn stop_ramp_a(caller: Principal, now: u64) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }

    let current_a = read_state(|s| {
        get_a(
            s.config.initial_a,
            s.config.future_a,
            s.config.initial_a_time,
            s.config.future_a_time,
            now,
        )
    });

    mutate_state(|s| {
        s.config.initial_a = current_a;
        s.config.future_a = current_a;
        s.config.initial_a_time = 0;
        s.config.future_a_time = 0;

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::StopRampA { frozen_a: current_a },
        });
    });

    Ok(())
}

/// Withdraw accumulated admin fees, transferring them to the admin.
pub async fn withdraw_admin_fees(caller: Principal) -> Result<[u128; 3], ThreePoolError> {
    let (admin, fees, tokens) = read_state(|s| {
        (s.config.admin, s.admin_fees, s.config.tokens.clone())
    });

    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }

    // Zero out fees first (deduct-before-transfer)
    mutate_state(|s| {
        s.admin_fees = [0; 3];
    });

    // Transfer each non-zero fee to admin
    for k in 0..3 {
        if fees[k] > 0 {
            transfer_to_user(tokens[k].ledger_id, admin, fees[k])
                .await
                .map_err(|reason| ThreePoolError::TransferFailed {
                    token: tokens[k].symbol.clone(),
                    reason,
                })?;
        }
    }

    // Record admin event
    mutate_state(|s| {
        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::WithdrawAdminFees { amounts: fees },
        });
    });

    Ok(fees)
}

/// Pause or unpause the pool.
pub fn set_paused(caller: Principal, paused: bool) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }

    mutate_state(|s| {
        s.is_paused = paused;

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::SetPaused { paused },
        });
    });

    Ok(())
}

/// Update the swap fee (in basis points). Max 100bp (1%).
pub fn set_swap_fee(caller: Principal, fee_bps: u64) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    if fee_bps > 100 {
        return Err(ThreePoolError::InvalidCoinIndex); // reuse for "invalid param"
    }

    mutate_state(|s| {
        s.config.swap_fee_bps = fee_bps;

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::SetSwapFee { fee_bps },
        });
    });

    Ok(())
}

// ─── Authorized Burn Callers ───

/// Add a canister to the authorized burn callers set.
pub fn add_authorized_burn_caller(caller: Principal, canister: Principal) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.burn_callers_mut().insert(canister);

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::AddAuthorizedBurnCaller { canister },
        });
    });
    Ok(())
}

/// Remove a canister from the authorized burn callers set.
pub fn remove_authorized_burn_caller(caller: Principal, canister: Principal) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    mutate_state(|s| {
        s.burn_callers_mut().remove(&canister);

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::RemoveAuthorizedBurnCaller { canister },
        });
    });
    Ok(())
}

/// Get all authorized burn callers.
pub fn get_authorized_burn_callers() -> Vec<Principal> {
    read_state(|s| s.burn_callers().iter().copied().collect())
}

/// Update the admin fee (share of swap fees taken by admin, in basis points). Max 10000.
pub fn set_admin_fee(caller: Principal, fee_bps: u64) -> Result<(), ThreePoolError> {
    let admin = read_state(|s| s.config.admin);
    if caller != admin {
        return Err(ThreePoolError::Unauthorized);
    }
    if fee_bps > 10_000 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }

    mutate_state(|s| {
        s.config.admin_fee_bps = fee_bps;

        let id = s.admin_events().len() as u64;
        s.admin_events_mut().push(ThreePoolAdminEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: ThreePoolAdminAction::SetAdminFee { fee_bps },
        });
    });

    Ok(())
}
