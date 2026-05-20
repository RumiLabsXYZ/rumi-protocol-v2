//! Admin-gated endpoints for the Rumi AMM.
//!
//! Pure code move from `lib.rs` (no behavior change). Every function here
//! is authorised via `crate::caller_is_admin()`, except `create_pool`,
//! which falls back to a permissionless path when `pool_creation_open`
//! is true.

use candid::{Nat, Principal};
use ic_cdk::update;
use ic_canister_log::log;
use std::collections::BTreeMap;

use crate::caller_is_admin;
use crate::derive_subaccount;
use crate::logs::INFO;
use crate::make_pool_id;
use crate::state::{mutate_state, read_state};
use crate::transfers::transfer_to_user;
use crate::types::*;
use crate::PoolGuard;

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

    // Validate fee_bps for all callers (admin included) to prevent creating
    // permanently broken pools where compute_swap would always error
    if args.fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
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
            lp_rewards: BTreeMap::new(),
            acc_reward_per_share: 0,
            pending_no_lp: 0,
            total_rewards_distributed: 0,
            processed_donation_nonces: std::collections::VecDeque::new(),
            reward_balance_snapshot: 0,
        };

        log!(INFO, "Pool created: {} (fee: {} bps, admin: {})", pool_id, args.fee_bps, is_admin);
        s.pools.insert(pool_id.clone(), pool);
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::CreatePool {
            pool_id: pool_id.clone(),
            token_a,
            token_b,
            fee_bps: args.fee_bps,
        });
        Ok(pool_id)
    })
}

#[update]
fn set_fee(pool_id: PoolId, fee_bps: u16) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    if fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.fee_bps = fee_bps;
        log!(INFO, "Pool {} fee set to {} bps", pool_id, fee_bps);
        s.record_admin_event(caller, AmmAdminAction::SetFee { pool_id: pool_id.clone(), fee_bps });
        Ok(())
    })
}

#[update]
fn set_protocol_fee(pool_id: PoolId, protocol_fee_bps: u16) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    if protocol_fee_bps > 10_000 {
        return Err(AmmError::FeeBpsOutOfRange);
    }
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.protocol_fee_bps = protocol_fee_bps;
        log!(INFO, "Pool {} protocol fee set to {} bps", pool_id, protocol_fee_bps);
        s.record_admin_event(caller, AmmAdminAction::SetProtocolFee { pool_id: pool_id.clone(), protocol_fee_bps });
        Ok(())
    })
}

#[update]
async fn withdraw_protocol_fees(pool_id: PoolId) -> Result<(u128, u128), AmmError> {
    caller_is_admin()?;

    // Acquire per-pool lock to prevent concurrent fee withdrawals
    let _pool_guard = PoolGuard::new(pool_id.clone())?;

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
        let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
        pool.protocol_fees_a = 0;
        pool.protocol_fees_b = 0;
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
            let pool = s.pools.get_mut(&pool_id).expect("pool must exist: verified at start of withdraw_protocol_fees");
            pool.protocol_fees_a += rollback_a;
            pool.protocol_fees_b += rollback_b;
        });
    }

    if !errors.is_empty() {
        return Err(AmmError::TransferFailed {
            token: "protocol_fees".to_string(),
            reason: errors.join("; "),
        });
    }

    log!(INFO, "Protocol fees withdrawn from {}: ({}, {})", pool_id, withdrawn_a, withdrawn_b);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::WithdrawProtocolFees {
            pool_id: pool_id.clone(),
            amount_a: withdrawn_a,
            amount_b: withdrawn_b,
        });
    });
    Ok((withdrawn_a, withdrawn_b))
}

#[update]
fn pause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = true;
        log!(INFO, "Pool {} paused", pool_id);
        s.record_admin_event(caller, AmmAdminAction::PausePool { pool_id: pool_id.clone() });
        Ok(())
    })
}

#[update]
fn unpause_pool(pool_id: PoolId) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let pool = s.pools.get_mut(&pool_id).ok_or(AmmError::PoolNotFound)?;
        pool.paused = false;
        log!(INFO, "Pool {} unpaused", pool_id);
        s.record_admin_event(caller, AmmAdminAction::UnpausePool { pool_id: pool_id.clone() });
        Ok(())
    })
}

#[update]
fn set_pool_creation_open(open: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.pool_creation_open = open);
    log!(INFO, "Pool creation open: {}", open);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::SetPoolCreationOpen { open });
    });
    Ok(())
}

#[update]
fn set_admin(new_admin: Principal) -> Result<(), AmmError> {
    caller_is_admin()?;
    if new_admin == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    let old_admin = read_state(|s| s.admin);
    mutate_state(|s| s.admin = new_admin);
    log!(INFO, "Admin transferred: {} -> {}", old_admin, new_admin);
    Ok(())
}

#[update]
fn set_maintenance_mode(enabled: bool) -> Result<(), AmmError> {
    caller_is_admin()?;
    mutate_state(|s| s.maintenance_mode = enabled);
    log!(INFO, "Maintenance mode: {}", enabled);
    mutate_state(|s| {
        s.record_admin_event(ic_cdk::caller(), AmmAdminAction::SetMaintenanceMode { enabled });
    });
    Ok(())
}

/// Configure which principal is allowed to call `notify_reward_received`.
/// Required before AMM1 earnings distribution can begin. Only callable
/// by admin. Set to the rumi_protocol_backend canister principal.
#[update]
fn set_protocol_backend_principal(principal: Principal) -> Result<(), AmmError> {
    caller_is_admin()?;
    if principal == Principal::anonymous() {
        return Err(AmmError::Unauthorized);
    }
    mutate_state(|s| s.protocol_backend_principal = Some(principal));
    log!(INFO, "Protocol backend principal set to: {}", principal);
    mutate_state(|s| {
        s.record_admin_event(
            ic_cdk::caller(),
            AmmAdminAction::SetProtocolBackendPrincipal { backend: principal },
        );
    });
    Ok(())
}

/// Admin recovery endpoint: burn whatever balance the AMM canister holds
/// at a specific (ledger, subaccount) by transferring it to the ledger's
/// minting account. ICRC-1 ledgers treat transfers to the minting
/// account as burns and charge no fee on top.
///
/// Built as the recovery path for the 2026-05-19 AMM1 pool_id-mismatch
/// incident, where `donate_icusd_to_amm1` minted icUSD into the AMM's
/// `sha256("rumi_amm:rewards:3USD_ICP")` subaccount even though no pool
/// is keyed off that pool_id (the actual 3USD/ICP pool ID is
/// `make_pool_id(token_a, token_b)`). The stuck balance is unbacked
/// icUSD and must be burned, not redirected.
///
/// Caller must be admin. `subaccount` must be exactly 32 bytes. If the
/// canister's balance at the subaccount is at or below the ledger fee
/// (typical `min_burn_amount`), returns `Ok(0)` (no-op). Otherwise
/// transfers the full balance to the ledger's minting account and
/// returns the burned amount on success.
///
/// Note: this burns the balance observed at the start of the call. If a
/// concurrent mint arrives during the inter-canister awaits, the residual
/// won't be burned — re-run the endpoint to clear it.
#[update]
async fn admin_burn_subaccount_balance(
    ledger: Principal,
    subaccount: Vec<u8>,
) -> Result<u128, AmmError> {
    use icrc_ledger_types::icrc1::account::Account;
    use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};

    caller_is_admin()?;

    // Subaccounts on ICRC-1 are fixed at 32 bytes.
    if subaccount.len() != 32 {
        return Err(AmmError::InvalidInput {
            reason: format!("subaccount must be 32 bytes, got {}", subaccount.len()),
        });
    }
    let mut sub = [0u8; 32];
    sub.copy_from_slice(&subaccount);
    // Render the subaccount as a 64-char lowercase hex string for the
    // admin-event audit trail. Avoids pulling in the `hex` crate.
    let subaccount_hex: String =
        sub.iter().map(|b| format!("{:02x}", b)).collect();

    // 1. Query the canister's balance at the target subaccount.
    // Saturation note: ICRC-1 balances are bounded well below u128::MAX in
    // any realistic scenario (icUSD total supply ~$10^4 e8s as of writing).
    // Saturating to u128::MAX on overflow is the safer failure mode here:
    // it would make the `balance <= fee` check below trivially false, so
    // the transfer attempt itself would surface the impossible value as a
    // ledger error rather than silently truncating.
    let acct = Account {
        owner: ic_cdk::id(),
        subaccount: Some(sub),
    };
    let balance_call: Result<(Nat,), _> =
        ic_cdk::call(ledger, "icrc1_balance_of", (acct,)).await;
    let balance: u128 = match balance_call {
        Ok((n,)) => n.0.try_into().unwrap_or(u128::MAX),
        Err((code, msg)) => {
            return Err(AmmError::TransferFailed {
                token: "icUSD".to_string(),
                reason: format!(
                    "icrc1_balance_of rejected: {:?} {}",
                    code, msg,
                ),
            });
        }
    };

    // 2. Query the ledger's transfer fee. Same saturation rationale as above.
    let fee_call: Result<(Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    let fee: u128 = match fee_call {
        Ok((n,)) => n.0.try_into().unwrap_or(u128::MAX),
        Err((code, msg)) => {
            return Err(AmmError::TransferFailed {
                token: "icUSD".to_string(),
                reason: format!("icrc1_fee rejected: {:?} {}", code, msg),
            });
        }
    };

    // 3. No-op if the balance is below `min_burn_amount`. ICRC-1 ledgers
    // enforce `amount >= min_burn_amount` (typically equal to the transfer
    // fee) on burns, so a balance at or below the fee cannot be burned.
    if balance <= fee {
        log!(
            INFO,
            "[admin_burn_subaccount_balance] no-op: balance={} fee={} for ledger {}",
            balance,
            fee,
            ledger,
        );
        return Ok(0);
    }

    // 4. Resolve the ledger's minting account (required to burn).
    let minting_call: Result<(Option<Account>,), _> =
        ic_cdk::call(ledger, "icrc1_minting_account", ()).await;
    let minting_account = match minting_call {
        Ok((Some(a),)) => a,
        Ok((None,)) => {
            return Err(AmmError::TransferFailed {
                token: "icUSD".to_string(),
                reason: "ledger has no minting account; cannot burn".to_string(),
            });
        }
        Err((code, msg)) => {
            return Err(AmmError::TransferFailed {
                token: "icUSD".to_string(),
                reason: format!(
                    "icrc1_minting_account rejected: {:?} {}",
                    code, msg,
                ),
            });
        }
    };

    // 5. Transfer the full balance from {self, sub} to the minting account.
    // ICRC-1 ledgers treat transfers to the minting account as burns and
    // charge no fee, so the source subaccount nets to 0.
    let amount_to_burn = balance;
    let args = TransferArg {
        from_subaccount: Some(sub),
        to: minting_account,
        amount: Nat::from(amount_to_burn),
        fee: None,
        memo: Some(b"amm1_pool_id_recovery_burn".to_vec().into()),
        created_at_time: Some(ic_cdk::api::time()),
    };
    let transfer_call: Result<(Result<Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;

    let caller = ic_cdk::caller();
    match transfer_call {
        Ok((Ok(block_index),)) => {
            let block_index_u64: u64 = block_index.0.clone().try_into().unwrap_or(u64::MAX);
            log!(
                INFO,
                "[admin_burn_subaccount_balance] burned {} from ledger {} subaccount via burn-to-minting-account at block {}",
                amount_to_burn,
                ledger,
                block_index_u64,
            );
            mutate_state(|s| {
                s.record_admin_event(
                    caller,
                    AmmAdminAction::AdminBurnSubaccount {
                        ledger,
                        subaccount_hex: subaccount_hex.clone(),
                        amount_burned: amount_to_burn,
                        block_index: block_index_u64,
                    },
                );
            });
            Ok(amount_to_burn)
        }
        // A Duplicate from the ledger means the burn already landed at
        // duplicate_of. Treat as success on the canonical amount, matching
        // the pattern used by `transfer_to_user` / `burn_token_on_ledger`.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            let block_index_u64: u64 = duplicate_of.0.clone().try_into().unwrap_or(u64::MAX);
            log!(
                INFO,
                "[admin_burn_subaccount_balance] burn deduped at ledger {} (previously landed at block {})",
                ledger,
                block_index_u64,
            );
            mutate_state(|s| {
                s.record_admin_event(
                    caller,
                    AmmAdminAction::AdminBurnSubaccount {
                        ledger,
                        subaccount_hex: subaccount_hex.clone(),
                        amount_burned: amount_to_burn,
                        block_index: block_index_u64,
                    },
                );
            });
            Ok(amount_to_burn)
        }
        Ok((Err(e),)) => Err(AmmError::TransferFailed {
            token: "icUSD".to_string(),
            reason: format!("icrc1_transfer error: {:?}", e),
        }),
        Err((code, msg)) => Err(AmmError::TransferFailed {
            token: "icUSD".to_string(),
            reason: format!("icrc1_transfer call rejected: {:?} {}", code, msg),
        }),
    }
}

/// Admin: force-remove a pending claim without transferring (e.g., after manual resolution).
#[update]
fn resolve_pending_claim(claim_id: u64) -> Result<(), AmmError> {
    let caller = ic_cdk::caller();
    caller_is_admin()?;
    mutate_state(|s| {
        let before = s.pending_claims.len();
        s.pending_claims.retain(|c| c.id != claim_id);
        if s.pending_claims.len() == before {
            return Err(AmmError::ClaimNotFound);
        }
        log!(INFO, "Pending claim #{} force-resolved by admin", claim_id);
        s.record_admin_event(caller, AmmAdminAction::ResolvePendingClaim { claim_id });
        Ok(())
    })
}
