use candid::Principal;
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;
use sha2::{Sha256, Digest};
use std::collections::BTreeMap;

pub mod types;
pub mod state;
pub mod math;
pub mod transfers;
mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state};
use crate::math::{compute_swap, compute_initial_lp_shares, compute_proportional_lp_shares,
                   compute_remove_liquidity, MINIMUM_LIQUIDITY};
use crate::transfers::{transfer_from_user, transfer_to_user};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: AmmInitArgs) {
    mutate_state(|s| s.initialize(args));
    log!(INFO, "Rumi AMM initialized. Admin: {}", read_state(|s| s.admin));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi AMM pre-upgrade: saving state");
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade(_args: AmmInitArgs) {
    state::load_from_stable_memory();
    log!(INFO, "Rumi AMM post-upgrade: state restored. {} pools",
        read_state(|s| s.pools.len()));
}

// ─── Helpers ───

fn caller_is_admin() -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    let admin = read_state(|s| s.admin);
    if caller != admin {
        return Err(AmmError::Unauthorized);
    }
    Ok(())
}

/// Derive a deterministic 32-byte subaccount from a pool ID and token label.
fn derive_subaccount(pool_id: &str, token_label: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pool_id.as_bytes());
    hasher.update(b"_");
    hasher.update(token_label.as_bytes());
    let result = hasher.finalize();
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&result);
    sub
}

/// Build pool ID from two token principals (sorted for determinism).
fn make_pool_id(token_a: Principal, token_b: Principal) -> PoolId {
    let a = token_a.to_text();
    let b = token_b.to_text();
    if a <= b {
        format!("{}_{}", a, b)
    } else {
        format!("{}_{}", b, a)
    }
}

/// Record a failed outbound transfer as a pending claim so the user can retry.
fn record_pending_claim(
    pool_id: &PoolId,
    claimant: Principal,
    token: Principal,
    subaccount: [u8; 32],
    amount: u128,
    reason: &str,
) -> u64 {
    mutate_state(|s| {
        let id = s.next_claim_id;
        s.next_claim_id += 1;
        s.pending_claims.push(PendingClaim {
            id,
            pool_id: pool_id.clone(),
            claimant,
            token,
            subaccount,
            amount,
            reason: reason.to_string(),
            created_at: ic_cdk::api::time() / 1_000_000_000,
        });
        log!(INFO, "Pending claim #{} recorded: {} owes {} of token {} (pool {})",
            id, claimant, amount, token, pool_id);
        id
    })
}

// ─── Admin Endpoints ───

#[update]
fn create_pool(args: CreatePoolArgs) -> Result<PoolId, AmmError> {
    // Admin exempt from maintenance mode — can set up pools while canister is locked
    if read_state(|s| s.maintenance_mode) && caller_is_admin().is_err() {
        return Err(AmmError::MaintenanceMode);
    }

    let is_admin = caller_is_admin().is_ok();

    if !is_admin {
        // Permissionless path: gate must be open, constant product only, fee clamped
        if !read_state(|s| s.pool_creation_open) {
            return Err(AmmError::PoolCreationClosed);
        }
        if args.curve != CurveType::ConstantProduct {
            return Err(AmmError::Unauthorized);
        }
        if args.fee_bps < 1 || args.fee_bps > 1000 {
            return Err(AmmError::FeeBpsOutOfRange);
        }
    }

    if args.token_a == args.token_b {
        return Err(AmmError::InvalidToken);
    }

    let pool_id = make_pool_id(args.token_a, args.token_b);

    mutate_state(|s| {
        if s.pools.contains_key(&pool_id) {
            return Err(AmmError::PoolAlreadyExists);
        }

        let subaccount_a = derive_subaccount(&pool_id, "token_a");
        let subaccount_b = derive_subaccount(&pool_id, "token_b");

        // Ensure token_a/token_b are stored in sorted order matching pool_id
        let (token_a, token_b) = if args.token_a.to_text() <= args.token_b.to_text() {
            (args.token_a, args.token_b)
        } else {
            (args.token_b, args.token_a)
        };

        let pool = Pool {
            token_a,
            token_b,
            reserve_a: 0,
            reserve_b: 0,
            fee_bps: args.fee_bps,
            protocol_fee_bps: 0, // 100% to LPs initially
            curve: args.curve,
            lp_shares: BTreeMap::new(),
            total_lp_shares: 0,
            protocol_fees_a: 0,
            protocol_fees_b: 0,
            paused: false,
            subaccount_a,
            subaccount_b,
        };

        log!(INFO, "Pool created: {} (fee: {} bps, admin: {})", pool_id, args.fee_bps, is_admin);
        s.pools.insert(pool_id.clone(), pool);
        Ok(pool_id)
    })
}

#[update]
fn set_fee(pool_id: PoolId, fee_bps: u16) -> Result<(), AmmError> {
    caller_is_admin()?;
    if fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.fee_bps = fee_bps;
        log!(INFO, "Pool {} fee set to {} bps", pool_id, fee_bps);
        Ok(())
    })
}

#[update]
fn set_protocol_fee(pool_id: PoolId, protocol_fee_bps: u16) -> Result<(), AmmError> {
    caller_is_admin()?;
    if protocol_fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.protocol_fee_bps = protocol_fee_bps;
        log!(INFO, "Pool {} protocol fee set to {} bps", pool_id, protocol_fee_bps);
        Ok(())
    })
}

#[update]
async fn withdraw_protocol_fees(pool_id: PoolId) -> Result<(u128, u128), AmmError> {
    caller_is_admin()?;

    let (token_a, token_b, sub_a, sub_b, fees_a, fees_b, admin) = read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
        Ok::<_, AmmError>((
            pool.token_a, pool.token_b,
            pool.subaccount_a, pool.subaccount_b,
            pool.protocol_fees_a, pool.protocol_fees_b,
            s.admin,
        ))
    })?;

    if fees_a == 0 && fees_b == 0 {
        return Ok((0, 0));
    }

    // Optimistic deduct: zero out fees in state BEFORE transferring.
    mutate_state(|s| {
        if let Some(pool) = s.pools.get_mut(&pool_id) {
            pool.protocol_fees_a = 0;
            pool.protocol_fees_b = 0;
        }
    });

    let mut withdrawn_a = 0u128;
    let mut withdrawn_b = 0u128;
    let mut errors = Vec::new();

    if fees_a > 0 {
        match transfer_to_user(token_a, sub_a, admin, fees_a).await {
            Ok(_) => withdrawn_a = fees_a,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_a failed: {}. Rolling back.", reason);
                errors.push(format!("token_a: {}", reason));
            }
        }
    }

    if fees_b > 0 {
        match transfer_to_user(token_b, sub_b, admin, fees_b).await {
            Ok(_) => withdrawn_b = fees_b,
            Err(reason) => {
                log!(INFO, "WARN: withdraw_protocol_fees transfer_b failed: {}. Rolling back.", reason);
                errors.push(format!("token_b: {}", reason));
            }
        }
    }

    // Roll back any fees that failed to transfer
    let rollback_a = fees_a - withdrawn_a;
    let rollback_b = fees_b - withdrawn_b;
    if rollback_a > 0 || rollback_b > 0 {
        mutate_state(|s| {
            if let Some(pool) = s.pools.get_mut(&pool_id) {
                pool.protocol_fees_a += rollback_a;
                pool.protocol_fees_b += rollback_b;
            }
        });
    }

    if !errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "protocol_fees".to_string(),
            reason: errors.join("; "),
        });
    }

    log!(INFO, "Protocol fees withdrawn from {}: ({}, {})", pool_id, withdrawn_a, withdrawn_b);
    Ok((withdrawn_a, withdrawn_b))
}

#[update]
fn pause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = true;
        log!(INFO, "Pool {} paused", pool_id);
        Ok(())
    })
}

#[update]
fn unpause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = false;
        log!(INFO, "Pool {} unpaused", pool_id);
        Ok(())
    })
}

#[update]
fn set_pool_creation_open(open: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.pool_creation_open = open);
    log!(INFO, "Pool creation open: {}", open);
    Ok(())
}

#[update]
fn set_maintenance_mode(enabled: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.maintenance_mode = enabled);
    log!(INFO, "Maintenance mode: {}", enabled);
    Ok(())
}

// ─── Claims ───

/// Retry a failed outbound transfer. The original claimant or admin can call this.
#[update]
async fn claim_pending(claim_id: u64) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();

    let claim = read_state(|s| {
        s.pending_claims
            .iter()
            .find(|c| c.id == claim_id)
            .cloned()
            .ok_or(AmmError::ClaimNotFound)
    })?;

    let is_admin = caller_is_admin().is_ok();
    if caller != claim.claimant && !is_admin {
        return Err(AmmError::Unauthorized);
    }

    transfer_to_user(claim.token, claim.subaccount, claim.claimant, claim.amount)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: claim.token.to_string(),
            reason,
        })?;

    mutate_state(|s| {
        s.pending_claims.retain(|c| c.id != claim_id);
    });

    log!(INFO, "Pending claim #{} resolved: {} received {} of token {}",
        claim_id, claim.claimant, claim.amount, claim.token);

    Ok(())
}

/// View all pending claims.
#[query]
fn get_pending_claims() -> Vec<PendingClaim> {
    read_state(|s| s.pending_claims.clone())
}

/// Admin: force-remove a pending claim without transferring (e.g., after manual resolution).
#[update]
fn resolve_pending_claim(claim_id: u64) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| {
        let before = s.pending_claims.len();
        s.pending_claims.retain(|c| c.id != claim_id);
        if s.pending_claims.len() == before {
            return Err(AmmError::ClaimNotFound);
        }
        log!(INFO, "Pending claim #{} force-resolved by admin", claim_id);
        Ok(())
    })
}

// ─── Core AMM ───

#[update]
async fn swap(
    pool_id: PoolId,
    token_in: Principal,
    amount_in: u128,
    min_amount_out: u128,
) -> Result<SwapResult, AmmError> {
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }

    let caller = ic_cdk::caller();

    // Read pool state
    let (token_a, token_b, reserve_a, reserve_b, fee_bps, protocol_fee_bps, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.fee_bps, pool.protocol_fee_bps,
                pool.subaccount_a, pool.subaccount_b,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    // Determine direction
    let (reserve_in, reserve_out, sub_in, sub_out, ledger_in, ledger_out, is_a_to_b) =
        if token_in == token_a {
            (reserve_a, reserve_b, sub_a, sub_b, token_a, token_b, true)
        } else if token_in == token_b {
            (reserve_b, reserve_a, sub_b, sub_a, token_b, token_a, false)
        } else {
            return Err(AmmError::InvalidToken);
        };

    // Compute swap
    let (amount_out, total_fee, protocol_fee) =
        compute_swap(reserve_in, reserve_out, amount_in, fee_bps, protocol_fee_bps)?;

    if amount_out < min_amount_out {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_out,
            actual: amount_out,
        });
    }

    // Pull input tokens from user
    transfer_from_user(ledger_in, caller, sub_in, amount_in)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: "input".to_string(),
            reason,
        })?;

    // Input tokens are now on-ledger in our subaccount — record immediately
    // so state matches on-chain reality even if the output transfer fails.
    mutate_state(|s| {
        if let Some(pool) = s.pools.get_mut(&pool_id) {
            if is_a_to_b {
                pool.reserve_a += amount_in - protocol_fee;
                pool.protocol_fees_a += protocol_fee;
            } else {
                pool.reserve_b += amount_in - protocol_fee;
                pool.protocol_fees_b += protocol_fee;
            }
        }
    });

    // Send output tokens to user
    match transfer_to_user(ledger_out, sub_out, caller, amount_out).await {
        Ok(_) => {
            // Output sent — deduct from reserves
            mutate_state(|s| {
                if let Some(pool) = s.pools.get_mut(&pool_id) {
                    if is_a_to_b {
                        pool.reserve_b -= amount_out;
                    } else {
                        pool.reserve_a -= amount_out;
                    }
                }
            });
        }
        Err(reason) => {
            // Output transfer failed — rollback input reserve change.
            // The input tokens are stuck in our subaccount but reserves
            // won't reflect them, keeping state consistent. Admin can
            // recover via a future reconciliation endpoint if needed.
            mutate_state(|s| {
                if let Some(pool) = s.pools.get_mut(&pool_id) {
                    if is_a_to_b {
                        pool.reserve_a -= amount_in - protocol_fee;
                        pool.protocol_fees_a -= protocol_fee;
                    } else {
                        pool.reserve_b -= amount_in - protocol_fee;
                        pool.protocol_fees_b -= protocol_fee;
                    }
                }
            });
            return Err(AmmError::TransferFailed {
                token: "output".to_string(),
                reason,
            });
        }
    }

    log!(INFO, "Swap on {}: {} in -> {} out (fee: {}, proto: {})",
        pool_id, amount_in, amount_out, total_fee, protocol_fee);

    Ok(SwapResult {
        amount_out,
        fee: total_fee,
    })
}

#[update]
async fn add_liquidity(
    pool_id: PoolId,
    amount_a: u128,
    amount_b: u128,
    min_lp_shares: u128,
) -> Result<u128, AmmError> {
    if read_state(|s| s.maintenance_mode) {
        return Err(AmmError::MaintenanceMode);
    }

    let caller = ic_cdk::caller();

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a, pool.subaccount_b,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    // Compute shares
    let shares = if total_shares == 0 {
        // First deposit — use geometric mean
        compute_initial_lp_shares(amount_a, amount_b)?
    } else {
        compute_proportional_lp_shares(amount_a, amount_b, reserve_a, reserve_b, total_shares)?
    };

    if shares < min_lp_shares {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_lp_shares,
            actual: shares,
        });
    }

    // Pull both tokens from user.
    // If token_b transfer fails after token_a succeeded, refund token_a.
    transfer_from_user(token_a, caller, sub_a, amount_a)
        .await
        .map_err(|reason| AmmError::TransferFailed {
            token: "token_a".to_string(),
            reason,
        })?;

    if let Err(reason) = transfer_from_user(token_b, caller, sub_b, amount_b).await {
        // Refund token_a back to user (best-effort — log if refund also fails)
        if let Err(refund_err) = transfer_to_user(token_a, sub_a, caller, amount_a).await {
            log!(INFO, "CRITICAL: token_b transfer failed AND token_a refund failed: {}. \
                 {} token_a stuck in pool subaccount for {}.", refund_err, amount_a, pool_id);
        }
        return Err(AmmError::TransferFailed {
            token: "token_b".to_string(),
            reason,
        });
    }

    // Update state
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).expect("pool exists");

        if pool.total_lp_shares == 0 {
            // First deposit: lock MINIMUM_LIQUIDITY to zero address
            let user_shares = shares - MINIMUM_LIQUIDITY;
            pool.lp_shares.insert(Principal::anonymous(), MINIMUM_LIQUIDITY);
            *pool.lp_shares.entry(caller).or_insert(0) += user_shares;
            pool.total_lp_shares = shares;

            log!(INFO, "Initial liquidity for {}: {} shares ({} locked)",
                pool_id, shares, MINIMUM_LIQUIDITY);
        } else {
            *pool.lp_shares.entry(caller).or_insert(0) += shares;
            pool.total_lp_shares += shares;
        }

        pool.reserve_a += amount_a;
        pool.reserve_b += amount_b;
    });

    log!(INFO, "Add liquidity to {}: ({}, {}) -> {} shares for {}",
        pool_id, amount_a, amount_b, shares, caller);

    Ok(shares)
}

#[update]
async fn remove_liquidity(
    pool_id: PoolId,
    lp_shares: u128,
    min_amount_a: u128,
    min_amount_b: u128,
) -> Result<(u128, u128), AmmError> {
    let caller = ic_cdk::caller();

    let (token_a, token_b, reserve_a, reserve_b, total_shares, sub_a, sub_b, user_shares, paused) =
        read_state(|s| {
            let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;
            let user_shares = pool.lp_shares.get(&caller).copied().unwrap_or(0);
            Ok::<_, AmmError>((
                pool.token_a, pool.token_b,
                pool.reserve_a, pool.reserve_b,
                pool.total_lp_shares,
                pool.subaccount_a, pool.subaccount_b,
                user_shares,
                pool.paused,
            ))
        })?;

    if paused {
        return Err(AmmError::PoolPaused);
    }

    if lp_shares > user_shares {
        return Err(AmmError::InsufficientLpShares {
            required: lp_shares,
            available: user_shares,
        });
    }

    let (amount_a, amount_b) = compute_remove_liquidity(lp_shares, reserve_a, reserve_b, total_shares)?;

    if amount_a < min_amount_a || amount_b < min_amount_b {
        return Err(AmmError::InsufficientOutput {
            expected_min: min_amount_a.max(min_amount_b),
            actual: amount_a.min(amount_b),
        });
    }

    // Burn LP shares and update reserves FIRST (optimistic),
    // then transfer tokens. This ensures the protocol never overpays
    // if a transfer fails mid-way.
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).expect("pool exists");
        let entry = pool.lp_shares.get_mut(&caller).expect("user has shares");
        *entry -= lp_shares;
        if *entry == 0 {
            pool.lp_shares.remove(&caller);
        }
        pool.total_lp_shares -= lp_shares;
        pool.reserve_a -= amount_a;
        pool.reserve_b -= amount_b;
    });

    // Send tokens to user. If either fails, shares are already burned
    // but tokens remain in the pool subaccount. Admin can reconcile.
    let mut transfer_errors = Vec::new();

    if amount_a > 0 {
        if let Err(reason) = transfer_to_user(token_a, sub_a, caller, amount_a).await {
            log!(INFO, "WARN: remove_liquidity transfer_a failed for {}: {}. \
                 {} tokens stuck in subaccount.", pool_id, reason, amount_a);
            transfer_errors.push(format!("token_a: {}", reason));
        }
    }

    if amount_b > 0 {
        if let Err(reason) = transfer_to_user(token_b, sub_b, caller, amount_b).await {
            log!(INFO, "WARN: remove_liquidity transfer_b failed for {}: {}. \
                 {} tokens stuck in subaccount.", pool_id, reason, amount_b);
            transfer_errors.push(format!("token_b: {}", reason));
        }
    }

    if !transfer_errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "output".to_string(),
            reason: transfer_errors.join("; "),
        });
    }

    log!(INFO, "Remove liquidity from {}: {} shares -> ({}, {}) for {}",
        pool_id, lp_shares, amount_a, amount_b, caller);

    Ok((amount_a, amount_b))
}

// ─── Query Endpoints ───

#[query]
fn get_pool(pool_id: PoolId) -> Option<PoolInfo> {
    read_state(|s| s.pools.get(&pool_id).map(|p| p.to_info(&pool_id)))
}

#[query]
fn get_pools() -> Vec<PoolInfo> {
    read_state(|s| {
        s.pools.iter().map(|(id, p)| p.to_info(id)).collect()
    })
}

#[query]
fn get_quote(pool_id: PoolId, token_in: Principal, amount_in: u128) -> Result<u128, AmmError> {
    read_state(|s| {
        let pool = s.pools.get(&pool_id).ok_or(AmmError::PoolNotFound)?;

        let (reserve_in, reserve_out) = if token_in == pool.token_a {
            (pool.reserve_a, pool.reserve_b)
        } else if token_in == pool.token_b {
            (pool.reserve_b, pool.reserve_a)
        } else {
            return Err(AmmError::InvalidToken);
        };

        let (amount_out, _, _) = compute_swap(
            reserve_in, reserve_out, amount_in, pool.fee_bps, pool.protocol_fee_bps,
        )?;
        Ok(amount_out)
    })
}

#[query]
fn get_lp_balance(pool_id: PoolId, user: Principal) -> u128 {
    read_state(|s| {
        s.pools
            .get(&pool_id)
            .and_then(|p| p.lp_shares.get(&user).copied())
            .unwrap_or(0)
    })
}

#[query]
fn is_pool_creation_open() -> bool {
    read_state(|s| s.pool_creation_open)
}

#[query]
fn is_maintenance_mode() -> bool {
    read_state(|s| s.maintenance_mode)
}

#[query]
fn health() -> String {
    let pool_count = read_state(|s| s.pools.len());
    format!("Rumi AMM OK — {} pool(s)", pool_count)
}
