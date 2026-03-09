use candid::Principal;
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;

pub mod types;
pub mod state;
pub mod math;
pub mod swap;
pub mod liquidity;
pub mod transfers;

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
