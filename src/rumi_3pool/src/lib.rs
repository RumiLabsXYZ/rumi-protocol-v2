use candid::Principal;
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;

pub mod types;
pub mod state;
pub mod math;
pub mod swap;
pub mod liquidity;
pub mod transfers;
pub mod admin;
pub mod icrc21;

mod logs;

use crate::types::*;
use crate::state::{mutate_state, read_state};
use crate::math::{get_a, virtual_price};
use crate::swap::calc_swap_output;
use crate::liquidity::{calc_add_liquidity, calc_remove_liquidity, calc_remove_one_coin};
use crate::transfers::{transfer_from_user, transfer_to_user};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: ThreePoolInitArgs) {
    mutate_state(|s| s.initialize(args));
    log!(INFO, "Rumi 3pool initialized. Admin: {}, A: {}, swap_fee: {} bps",
        read_state(|s| s.config.admin),
        read_state(|s| s.config.initial_a),
        read_state(|s| s.config.swap_fee_bps));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi 3pool pre-upgrade: saving state to stable memory");
    state::save_to_stable_memory();
}

#[post_upgrade]
fn post_upgrade() {
    state::load_from_stable_memory();
    log!(INFO, "Rumi 3pool post-upgrade: state restored. LP supply: {}, initialized: {}",
        read_state(|s| s.lp_total_supply),
        read_state(|s| s.is_initialized));
}

// ─── Queries ───

#[query]
pub fn health() -> String {
    "ok".to_string()
}

// ─── Helper: extract precision_muls from config ───

fn get_precision_muls() -> [u64; 3] {
    read_state(|s| {
        [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ]
    })
}

fn get_current_a() -> u64 {
    read_state(|s| {
        let now = ic_cdk::api::time() / 1_000_000_000;
        get_a(
            s.config.initial_a,
            s.config.future_a,
            s.config.initial_a_time,
            s.config.future_a_time,
            now,
        )
    })
}

// ─── Swap ───

#[update]
pub async fn swap(i: u8, j: u8, dx: u128, min_dy: u128) -> Result<u128, ThreePoolError> {
    // 1. Check not paused
    if read_state(|s| s.is_paused) {
        return Err(ThreePoolError::PoolPaused);
    }

    let i_idx = i as usize;
    let j_idx = j as usize;

    // 2. Compute current A and precision_muls
    let amp = get_current_a();
    let precision_muls = get_precision_muls();

    // 3. Read current state
    let (balances, swap_fee_bps, admin_fee_bps, token_i_ledger, token_j_ledger, token_j_symbol) =
        read_state(|s| {
            (
                s.balances,
                s.config.swap_fee_bps,
                s.config.admin_fee_bps,
                s.config.tokens[i_idx].ledger_id,
                s.config.tokens[j_idx].ledger_id,
                s.config.tokens[j_idx].symbol.clone(),
            )
        });

    // 4. Calculate swap output
    let (output, fee) =
        calc_swap_output(i_idx, j_idx, dx, &balances, &precision_muls, amp, swap_fee_bps)?;

    // 5. Slippage check
    if output < min_dy {
        return Err(ThreePoolError::SlippageExceeded);
    }

    // 6. Transfer input token from user to pool
    let caller = ic_cdk::api::caller();
    let token_i_symbol = read_state(|s| s.config.tokens[i_idx].symbol.clone());

    transfer_from_user(token_i_ledger, caller, dx)
        .await
        .map_err(|reason| ThreePoolError::TransferFailed {
            token: token_i_symbol,
            reason,
        })?;

    // 7. Transfer output token from pool to user
    transfer_to_user(token_j_ledger, caller, output)
        .await
        .map_err(|reason| ThreePoolError::TransferFailed {
            token: token_j_symbol,
            reason,
        })?;

    // 8. Update state
    let admin_fee_share = fee * (admin_fee_bps as u128) / 10_000;
    mutate_state(|s| {
        s.balances[i_idx] += dx;
        s.balances[j_idx] -= output + fee;
        s.admin_fees[j_idx] += admin_fee_share;
    });

    log!(INFO, "Swap: {} of token {} -> {} of token {} (fee: {}, admin_fee: {})",
        dx, i, output, j, fee, admin_fee_share);

    Ok(output)
}

// ─── Add Liquidity ───

#[update]
pub async fn add_liquidity(amounts: Vec<u128>, min_lp: u128) -> Result<u128, ThreePoolError> {
    // 1. Check not paused
    if read_state(|s| s.is_paused) {
        return Err(ThreePoolError::PoolPaused);
    }

    // 2. Convert Vec to [u128; 3]
    if amounts.len() != 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    let amounts_arr: [u128; 3] = [amounts[0], amounts[1], amounts[2]];

    // 3. Compute A and precision_muls
    let amp = get_current_a();
    let precision_muls = get_precision_muls();

    // 4. Read current state
    let (old_balances, lp_total_supply, swap_fee_bps) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.swap_fee_bps)
    });

    // 5. Calculate LP tokens to mint
    let (lp_minted, _fees) = calc_add_liquidity(
        &amounts_arr,
        &old_balances,
        &precision_muls,
        lp_total_supply,
        amp,
        swap_fee_bps,
    )?;

    // 6. Slippage check
    if lp_minted < min_lp {
        return Err(ThreePoolError::SlippageExceeded);
    }

    // 7. Transfer each non-zero amount from user to pool
    let caller = ic_cdk::api::caller();
    for k in 0..3 {
        if amounts_arr[k] > 0 {
            let (ledger, symbol) = read_state(|s| {
                (s.config.tokens[k].ledger_id, s.config.tokens[k].symbol.clone())
            });
            transfer_from_user(ledger, caller, amounts_arr[k])
                .await
                .map_err(|reason| ThreePoolError::TransferFailed {
                    token: symbol,
                    reason,
                })?;
        }
    }

    // 8. Update state
    mutate_state(|s| {
        for k in 0..3 {
            s.balances[k] += amounts_arr[k];
        }
        let entry = s.lp_balances.entry(caller).or_insert(0);
        *entry += lp_minted;
        s.lp_total_supply += lp_minted;
        s.is_initialized = true;
    });

    log!(INFO, "AddLiquidity: {:?} -> {} LP for {}", amounts_arr, lp_minted, caller);

    Ok(lp_minted)
}

// ─── Remove Liquidity (proportional) ───

#[update]
pub async fn remove_liquidity(
    lp_burn: u128,
    min_amounts: Vec<u128>,
) -> Result<Vec<u128>, ThreePoolError> {
    // 1. Validate min_amounts length
    if min_amounts.len() != 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    let min_arr: [u128; 3] = [min_amounts[0], min_amounts[1], min_amounts[2]];

    if lp_burn == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }

    let caller = ic_cdk::api::caller();

    // 2. Check caller has enough LP
    let (user_lp, balances, lp_total_supply) = read_state(|s| {
        let user_lp = s.lp_balances.get(&caller).copied().unwrap_or(0);
        (user_lp, s.balances, s.lp_total_supply)
    });

    if user_lp < lp_burn {
        return Err(ThreePoolError::InsufficientLiquidity);
    }

    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }

    // 3. Calculate withdrawal amounts
    let amounts = calc_remove_liquidity(lp_burn, &balances, lp_total_supply);

    // 4. Check each amount >= min_amounts
    for k in 0..3 {
        if amounts[k] < min_arr[k] {
            return Err(ThreePoolError::SlippageExceeded);
        }
    }

    // 5. Deduct LP first (deduct-before-transfer pattern)
    mutate_state(|s| {
        let entry = s.lp_balances.get_mut(&caller).unwrap();
        *entry -= lp_burn;
        s.lp_total_supply -= lp_burn;
        for k in 0..3 {
            s.balances[k] -= amounts[k];
        }
    });

    // 6. Transfer each non-zero amount to user
    for k in 0..3 {
        if amounts[k] > 0 {
            let (ledger, symbol) = read_state(|s| {
                (s.config.tokens[k].ledger_id, s.config.tokens[k].symbol.clone())
            });
            transfer_to_user(ledger, caller, amounts[k])
                .await
                .map_err(|reason| ThreePoolError::TransferFailed {
                    token: symbol,
                    reason,
                })?;
        }
    }

    log!(INFO, "RemoveLiquidity: {} LP -> {:?} for {}", lp_burn, amounts, caller);

    Ok(amounts.to_vec())
}

// ─── Remove One Coin ───

#[update]
pub async fn remove_one_coin(
    lp_burn: u128,
    coin_index: u8,
    min_amount: u128,
) -> Result<u128, ThreePoolError> {
    let idx = coin_index as usize;
    if idx >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }

    let caller = ic_cdk::api::caller();

    // 1. Check caller has enough LP
    let (user_lp, balances, lp_total_supply) = read_state(|s| {
        let user_lp = s.lp_balances.get(&caller).copied().unwrap_or(0);
        (user_lp, s.balances, s.lp_total_supply)
    });

    if user_lp < lp_burn {
        return Err(ThreePoolError::InsufficientLiquidity);
    }

    // 2. Compute A, precision_muls
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let fee_bps = read_state(|s| s.config.swap_fee_bps);

    // 3. Calculate withdrawal
    let (amount, fee) = calc_remove_one_coin(
        lp_burn,
        idx,
        &balances,
        &precision_muls,
        lp_total_supply,
        amp,
        fee_bps,
    )?;

    // 4. Slippage check
    if amount < min_amount {
        return Err(ThreePoolError::SlippageExceeded);
    }

    // 5. Deduct LP and balance first
    let admin_fee_bps = read_state(|s| s.config.admin_fee_bps);
    let admin_fee_share = fee * (admin_fee_bps as u128) / 10_000;

    mutate_state(|s| {
        let entry = s.lp_balances.get_mut(&caller).unwrap();
        *entry -= lp_burn;
        s.lp_total_supply -= lp_burn;
        s.balances[idx] -= amount + fee;
        s.admin_fees[idx] += admin_fee_share;
    });

    // 6. Transfer to user
    let (ledger, symbol) = read_state(|s| {
        (s.config.tokens[idx].ledger_id, s.config.tokens[idx].symbol.clone())
    });

    transfer_to_user(ledger, caller, amount)
        .await
        .map_err(|reason| ThreePoolError::TransferFailed {
            token: symbol,
            reason,
        })?;

    log!(INFO, "RemoveOneCoin: {} LP -> {} of token {} for {} (fee: {})",
        lp_burn, amount, coin_index, caller, fee);

    Ok(amount)
}

// ─── Query Endpoints ───

#[query]
pub fn get_pool_status() -> PoolStatus {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();

    read_state(|s| {
        let vp = virtual_price(&s.balances, &precision_muls, amp, s.lp_total_supply)
            .unwrap_or(0);

        PoolStatus {
            balances: s.balances,
            lp_total_supply: s.lp_total_supply,
            current_a: amp,
            virtual_price: vp,
            swap_fee_bps: s.config.swap_fee_bps,
            admin_fee_bps: s.config.admin_fee_bps,
            tokens: s.config.tokens.clone(),
        }
    })
}

#[query]
pub fn get_lp_balance(user: Principal) -> u128 {
    read_state(|s| s.lp_balances.get(&user).copied().unwrap_or(0))
}

#[query]
pub fn calc_swap(i: u8, j: u8, dx: u128) -> Result<u128, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, swap_fee_bps) = read_state(|s| (s.balances, s.config.swap_fee_bps));

    let (output, _fee) =
        calc_swap_output(i as usize, j as usize, dx, &balances, &precision_muls, amp, swap_fee_bps)?;

    Ok(output)
}

#[query]
pub fn calc_add_liquidity_query(amounts: Vec<u128>, min_lp: u128) -> Result<u128, ThreePoolError> {
    if amounts.len() != 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    let amounts_arr: [u128; 3] = [amounts[0], amounts[1], amounts[2]];
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (old_balances, lp_total_supply, swap_fee_bps) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.swap_fee_bps)
    });
    let (lp_minted, _fees) = calc_add_liquidity(
        &amounts_arr, &old_balances, &precision_muls, lp_total_supply, amp, swap_fee_bps,
    )?;
    let _ = min_lp; // reserved for future use
    Ok(lp_minted)
}

#[query]
pub fn calc_remove_liquidity_query(lp_burn: u128) -> Result<Vec<u128>, ThreePoolError> {
    if lp_burn == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    let (balances, lp_total_supply) = read_state(|s| (s.balances, s.lp_total_supply));
    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }
    let amounts = calc_remove_liquidity(lp_burn, &balances, lp_total_supply);
    Ok(amounts.to_vec())
}

#[query]
pub fn calc_remove_one_coin_query(lp_burn: u128, coin_index: u8) -> Result<u128, ThreePoolError> {
    let idx = coin_index as usize;
    if idx >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    if lp_burn == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, lp_total_supply, fee_bps) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.swap_fee_bps)
    });
    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }
    let (amount, _fee) = calc_remove_one_coin(
        lp_burn, idx, &balances, &precision_muls, lp_total_supply, amp, fee_bps,
    )?;
    Ok(amount)
}

#[query]
pub fn get_admin_fees() -> Vec<u128> {
    read_state(|s| s.admin_fees.to_vec())
}

// ─── Admin Endpoints ───

#[update]
pub fn ramp_a(future_a: u64, future_a_time: u64) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    let now = ic_cdk::api::time() / 1_000_000_000;
    admin::ramp_a(future_a, future_a_time, caller, now)
}

#[update]
pub fn stop_ramp_a() -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    let now = ic_cdk::api::time() / 1_000_000_000;
    admin::stop_ramp_a(caller, now)
}

#[update]
pub async fn withdraw_admin_fees() -> Result<Vec<u128>, ThreePoolError> {
    let caller = ic_cdk::api::caller();
    let fees = admin::withdraw_admin_fees(caller).await?;
    Ok(fees.to_vec())
}

#[update]
pub fn set_paused(paused: bool) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    admin::set_paused(caller, paused)
}

// ─── ICRC-21 / ICRC-28 / ICRC-10 ───

#[update]
pub fn icrc21_canister_call_consent_message(
    request: icrc21::ConsentMessageRequest,
) -> icrc21::Icrc21ConsentMessageResult {
    icrc21::icrc21_canister_call_consent_message(request)
}

#[query]
pub fn icrc28_trusted_origins() -> icrc21::Icrc28TrustedOriginsResponse {
    icrc21::icrc28_trusted_origins()
}

#[query]
pub fn icrc10_supported_standards() -> Vec<icrc21::StandardRecord> {
    icrc21::icrc10_supported_standards()
}
