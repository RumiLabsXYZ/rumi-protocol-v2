use candid::Principal;
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;
use std::time::Duration;

pub mod types;
pub mod state;
pub mod math;
pub mod swap;
pub mod liquidity;
pub mod transfers;
pub mod admin;
pub mod icrc21;
pub mod icrc_token;
pub mod icrc3;
pub mod certification;

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
    setup_timers();
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

    // ── One-time migration: LP token 18 → 8 decimals ──
    // Divide all LP balances by 1e10, reset block log, and re-log mints.
    // Safe to run once: after this upgrade, lp_total_supply will be ~1e8 scale.
    // Detect by checking if supply is > 1e15 (would be impossible at 8-decimal scale
    // since that would mean >10M LP tokens, but our pool only has ~$84 TVL).
    mutate_state(|s| {
        const SCALE_DOWN: u128 = 10_000_000_000; // 1e10
        if s.lp_total_supply > 1_000_000_000_000_000 {
            // Scale down all LP balances
            let mut new_total: u128 = 0;
            let holders: Vec<(candid::Principal, u128)> = s.lp_balances.iter()
                .map(|(p, b)| (*p, *b))
                .collect();
            s.lp_balances.clear();
            for (principal, old_balance) in &holders {
                let new_balance = old_balance / SCALE_DOWN;
                if new_balance > 0 {
                    s.lp_balances.insert(*principal, new_balance);
                    new_total += new_balance;
                }
            }
            s.lp_total_supply = new_total;

            // Reset ICRC-3 block log — old blocks have 18-decimal amounts
            *s.blocks_mut() = Vec::new();
            s.last_block_hash = None;
            s.lp_tx_count = Some(0);

            // Log fresh mint blocks for each holder with their new 8-decimal balances
            for (principal, new_balance) in &holders {
                let new_bal = new_balance / SCALE_DOWN;
                if new_bal > 0 {
                    s.log_block(Icrc3Transaction::Mint { to: *principal, amount: new_bal });
                }
            }

            // Also clear VP snapshots — old ones used 18-decimal supply
            *s.snapshots_mut() = Vec::new();

            ic_canister_log::log!(crate::logs::INFO,
                "LP decimal migration 18→8: scaled {} holders, new total supply: {}",
                holders.len(), new_total);
        } else {
            // Normal startup: recompute hash chain and set certified data
            let hash = certification::recompute_hash_chain(s.blocks());
            s.last_block_hash = hash;
            if let Some(ref h) = s.last_block_hash {
                let last_idx = s.blocks().len().saturating_sub(1) as u64;
                certification::set_certified_tip(last_idx, h);
            }
        }
    });

    // Idempotent backfill of v2 event vecs from legacy v1 events.
    // Uses sentinel values for the new dynamic-fee fields (pre-migration unknown).
    mutate_state(|s| {
        let (sw, lq) = s.migrate_events_to_v2();
        if sw > 0 || lq > 0 {
            log!(INFO, "3pool v2 event backfill: {} swap, {} liquidity entries added", sw, lq);
        }
    });

    setup_timers();
    log!(INFO, "Rumi 3pool post-upgrade: state restored. LP supply: {}, initialized: {}, blocks: {}",
        read_state(|s| s.lp_total_supply),
        read_state(|s| s.is_initialized),
        read_state(|s| s.blocks().len()));
}

// ─── Timers ───

/// Set up recurring timers for VP snapshots.
fn setup_timers() {
    // Take an immediate snapshot so we have data right away.
    take_vp_snapshot();
    // Then every 6 hours.
    ic_cdk_timers::set_timer_interval(Duration::from_secs(6 * 60 * 60), || {
        take_vp_snapshot();
    });
}

/// Record a virtual_price snapshot for APY calculations.
fn take_vp_snapshot() {
    let precision_muls = get_precision_muls();
    let amp = get_current_a();

    mutate_state(|s| {
        if s.lp_total_supply == 0 {
            return; // No LPs — virtual_price is meaningless.
        }
        let vp = match virtual_price(&s.balances, &precision_muls, amp, s.lp_total_supply) {
            Some(v) => v,
            None => return,
        };
        let now_secs = ic_cdk::api::time() / 1_000_000_000;
        let lp_supply = s.lp_total_supply;
        let snapshot = VirtualPriceSnapshot {
            timestamp_secs: now_secs,
            virtual_price: vp,
            lp_total_supply: lp_supply,
        };
        s.snapshots_mut().push(snapshot);
    });
    log!(INFO, "VP snapshot taken");
}

// ─── Queries ───

#[query]
pub fn health() -> String {
    "ok".to_string()
}

/// Query swap events for explorer. Returns events in the requested range.
#[query]
pub fn get_swap_events(start: u64, length: u64) -> Vec<SwapEvent> {
    read_state(|s| {
        let events = s.swap_events();
        let total = events.len() as u64;
        if start >= total {
            return vec![];
        }
        let end = (start + length).min(total) as usize;
        events[start as usize..end].to_vec()
    })
}

/// Query total number of swap events.
#[query]
pub fn get_swap_event_count() -> u64 {
    read_state(|s| s.swap_events().len() as u64)
}

/// Query liquidity events for explorer. Returns events in the requested range.
#[query]
pub fn get_liquidity_events(start: u64, length: u64) -> Vec<LiquidityEvent> {
    read_state(|s| {
        let events = s.liquidity_events();
        let total = events.len() as u64;
        if start >= total {
            return vec![];
        }
        let end = (start + length).min(total) as usize;
        events[start as usize..end].to_vec()
    })
}

/// Query total number of liquidity events.
#[query]
pub fn get_liquidity_event_count() -> u64 {
    read_state(|s| s.liquidity_events().len() as u64)
}

/// Query admin events for explorer. Returns events in the requested range.
#[query]
pub fn get_admin_events(start: u64, length: u64) -> Vec<ThreePoolAdminEvent> {
    read_state(|s| {
        let events = s.admin_events();
        let total = events.len() as u64;
        if start >= total {
            return vec![];
        }
        let end = (start + length).min(total) as usize;
        events[start as usize..end].to_vec()
    })
}

/// Query total number of admin events.
#[query]
pub fn get_admin_event_count() -> u64 {
    read_state(|s| s.admin_events().len() as u64)
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

    // 4. Calculate swap output. Task 7: signature now takes `&FeeCurveParams`
    // and returns a `SwapOutcome`. Wiring fee_curve through PoolConfig + v2
    // event emission lands in Task 8; here we preserve existing behavior with
    // a default curve and ignore the new fields.
    let _ = swap_fee_bps;
    let _default_curve = crate::types::FeeCurveParams::default();
    let outcome = calc_swap_output(
        i_idx,
        j_idx,
        dx,
        &balances,
        &precision_muls,
        amp,
        &_default_curve,
    )?;
    let output = outcome.output_native;
    let fee = outcome.fee_native;

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

        // Record swap event for explorer
        let id = s.swap_events().len() as u64;
        s.swap_events_mut().push(SwapEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            token_in: i,
            token_out: j,
            amount_in: dx,
            amount_out: output,
            fee,
        });
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
        // Log mint block for ICRC-3 index
        s.log_block(Icrc3Transaction::Mint { to: caller, amount: lp_minted });
    });

    // Record liquidity event
    mutate_state(|s| {
        let id = s.liquidity_events().len() as u64;
        s.liquidity_events_mut().push(LiquidityEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::AddLiquidity,
            amounts: amounts_arr,
            lp_amount: lp_minted,
            coin_index: None,
            fee: None,
        });
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
        // Log burn block for ICRC-3 index
        s.log_block(Icrc3Transaction::Burn { from: caller, amount: lp_burn });
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

    // Record liquidity event
    mutate_state(|s| {
        let id = s.liquidity_events().len() as u64;
        s.liquidity_events_mut().push(LiquidityEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::RemoveLiquidity,
            amounts,
            lp_amount: lp_burn,
            coin_index: None,
            fee: None,
        });
    });

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
        // Log burn block for ICRC-3 index
        s.log_block(Icrc3Transaction::Burn { from: caller, amount: lp_burn });
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

    // Record liquidity event
    mutate_state(|s| {
        let id = s.liquidity_events().len() as u64;
        let mut amounts = [0u128; 3];
        amounts[idx] = amount;
        s.liquidity_events_mut().push(LiquidityEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::RemoveOneCoin,
            amounts,
            lp_amount: lp_burn,
            coin_index: Some(coin_index),
            fee: Some(fee),
        });
    });

    log!(INFO, "RemoveOneCoin: {} LP -> {} of token {} for {} (fee: {})",
        lp_burn, amount, coin_index, caller, fee);

    Ok(amount)
}

// ─── Donate (yield injection) ───

/// Donate tokens to the pool, increasing virtual_price for all LP holders.
/// No LP tokens are minted — this is pure yield injection.
/// Permissionless: anyone (admin, treasury, or user) can donate.
#[update]
pub async fn donate(token_index: u8, amount: u128) -> Result<(), ThreePoolError> {
    if read_state(|s| s.is_paused) {
        return Err(ThreePoolError::PoolPaused);
    }
    if amount == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }

    let idx = token_index as usize;
    if idx >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }

    // Must have LP holders to donate to
    if read_state(|s| s.lp_total_supply) == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }

    // Transfer token from caller to pool
    let caller = ic_cdk::api::caller();
    let (ledger, symbol) = read_state(|s| {
        (s.config.tokens[idx].ledger_id, s.config.tokens[idx].symbol.clone())
    });

    transfer_from_user(ledger, caller, amount)
        .await
        .map_err(|reason| ThreePoolError::TransferFailed {
            token: symbol.clone(),
            reason,
        })?;

    // Update balance — NO LP minted
    mutate_state(|s| {
        s.balances[idx] += amount;
    });

    // Record liquidity event
    mutate_state(|s| {
        let id = s.liquidity_events().len() as u64;
        let mut amounts = [0u128; 3];
        amounts[idx] = amount;
        s.liquidity_events_mut().push(LiquidityEvent {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::Donate,
            amounts,
            lp_amount: 0,
            coin_index: Some(token_index),
            fee: None,
        });
    });

    log!(INFO, "Donate: {} of {} (token {}) from {}", amount, symbol, token_index, caller);

    Ok(())
}

/// Receive a donation that was already transferred to the pool (e.g. minted directly).
/// Admin or controller only. Verifies the pool's on-chain ledger balance covers the
/// claimed amount before updating internal accounting.
#[update]
pub async fn receive_donation(token_index: u8, amount: u128) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    let admin = read_state(|s| s.config.admin);
    if caller != admin && !ic_cdk::api::is_controller(&caller) {
        return Err(ThreePoolError::Unauthorized);
    }
    if read_state(|s| s.is_paused) {
        return Err(ThreePoolError::PoolPaused);
    }
    if amount == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    let idx = token_index as usize;
    if idx >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    if read_state(|s| s.lp_total_supply) == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }

    // Verify the pool actually holds enough tokens on the ledger
    let (ledger, symbol) = read_state(|s| {
        (s.config.tokens[idx].ledger_id, s.config.tokens[idx].symbol.clone())
    });
    let pool_id = ic_cdk::id();
    let balance: Result<(candid::Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (icrc_ledger_types::icrc1::account::Account {
            owner: pool_id,
            subaccount: None,
        },),
    )
    .await;
    let on_chain_balance: u128 = match balance {
        Ok((nat,)) => nat.0.try_into().unwrap_or(0),
        Err((code, msg)) => {
            log!(INFO, "receive_donation: balance check failed: {:?} {}", code, msg);
            return Err(ThreePoolError::TransferFailed {
                token: symbol,
                reason: format!("balance check failed: {:?} {}", code, msg),
            });
        }
    };
    let expected_min = read_state(|s| s.balances[idx]) + amount;
    if on_chain_balance < expected_min {
        log!(INFO, "receive_donation: on-chain balance {} < expected {}", on_chain_balance, expected_min);
        return Err(ThreePoolError::TransferFailed {
            token: symbol,
            reason: format!("on-chain balance {} < expected {}", on_chain_balance, expected_min),
        });
    }

    mutate_state(|s| {
        s.balances[idx] += amount;
    });

    log!(INFO, "ReceiveDonation: {} of {} (token {}) from {}", amount, symbol, token_index, caller);

    Ok(())
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

/// Returns all LP holders and their balances, sorted by balance descending.
#[query]
pub fn get_all_lp_holders() -> Vec<(Principal, u128)> {
    read_state(|s| {
        let mut holders: Vec<(Principal, u128)> = s.lp_balances
            .iter()
            .filter(|(_, &balance)| balance > 0)
            .map(|(&p, &b)| (p, b))
            .collect();
        holders.sort_by(|a, b| b.1.cmp(&a.1));
        holders
    })
}

#[query]
pub fn calc_swap(i: u8, j: u8, dx: u128) -> Result<u128, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, _swap_fee_bps) = read_state(|s| (s.balances, s.config.swap_fee_bps));
    let curve = crate::types::FeeCurveParams::default();

    let outcome =
        calc_swap_output(i as usize, j as usize, dx, &balances, &precision_muls, amp, &curve)?;

    Ok(outcome.output_native)
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

/// Returns all virtual_price snapshots for APY calculation and historical charts.
#[query]
pub fn get_vp_snapshots() -> Vec<VirtualPriceSnapshot> {
    read_state(|s| s.snapshots().clone())
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

#[update]
pub fn set_swap_fee(fee_bps: u64) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    admin::set_swap_fee(caller, fee_bps)
}

#[update]
pub fn set_admin_fee(fee_bps: u64) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    admin::set_admin_fee(caller, fee_bps)
}

// ─── Authorized Burn Caller Admin ───

#[update]
pub fn add_authorized_burn_caller(canister: Principal) -> Result<(), ThreePoolError> {
    admin::add_authorized_burn_caller(ic_cdk::caller(), canister)
}

#[update]
pub fn remove_authorized_burn_caller(canister: Principal) -> Result<(), ThreePoolError> {
    admin::remove_authorized_burn_caller(ic_cdk::caller(), canister)
}

#[query]
pub fn get_authorized_burn_callers() -> Vec<Principal> {
    admin::get_authorized_burn_callers()
}

// ─── Authorized Redeem-and-Burn ───

/// Authorized redeem-and-burn: an authorized canister burns its LP tokens
/// and a corresponding amount of one token is removed from pool reserves
/// and burned on that token's ledger.
///
/// General-purpose function for protocol operations like stability pool
/// liquidations and peg management.
#[update]
pub async fn authorized_redeem_and_burn(
    args: AuthorizedRedeemAndBurnArgs,
) -> Result<RedeemAndBurnResult, ThreePoolError> {
    let caller = ic_cdk::caller();

    // 1. Authorization check
    if !read_state(|s| s.burn_callers().contains(&caller)) {
        return Err(ThreePoolError::NotAuthorizedBurnCaller);
    }

    // 2. Resolve token index
    let (token_idx, token_symbol) = read_state(|s| {
        for (i, tc) in s.config.tokens.iter().enumerate() {
            if tc.ledger_id == args.token_ledger {
                return Ok((i, tc.symbol.clone()));
            }
        }
        Err(ThreePoolError::InvalidCoinIndex)
    })?;

    // 3. Validate LP balance
    let caller_lp = read_state(|s| s.lp_balances.get(&caller).copied().unwrap_or(0));
    if caller_lp < args.lp_amount {
        return Err(ThreePoolError::InsufficientLpBalance {
            required: args.lp_amount,
            available: caller_lp,
        });
    }

    // 4. Validate pool has enough of the target token
    let pool_balance = read_state(|s| s.balances[token_idx]);
    if pool_balance < args.token_amount {
        return Err(ThreePoolError::InsufficientPoolBalance {
            token: token_symbol.clone(),
            required: args.token_amount,
            available: pool_balance,
        });
    }

    // 5. Validate slippage: compare LP-to-token ratio against virtual price
    let vp = read_state(|s| {
        let pms = [
            s.config.tokens[0].precision_mul,
            s.config.tokens[1].precision_mul,
            s.config.tokens[2].precision_mul,
        ];
        let a = get_a(
            s.config.initial_a, s.config.future_a,
            s.config.initial_a_time, s.config.future_a_time,
            ic_cdk::api::time(),
        );
        virtual_price(&s.balances, &pms, a, s.lp_total_supply)
    });

    if let Some(vp) = vp {
        // Expected token value of the LP being burned (in 18-dec)
        let expected_value_18 = args.lp_amount as u128 * vp / 100_000_000; // LP is 8-dec
        // token_amount in 18-dec for comparison
        let token_decimals = read_state(|s| s.config.tokens[token_idx].decimals);
        let token_amount_18 = args.token_amount * 10u128.pow((18 - token_decimals) as u32);

        // Check slippage: token_amount should not exceed expected_value * (1 + slippage)
        let max_token_18 = expected_value_18 * (10_000 + args.max_slippage_bps as u128) / 10_000;
        if token_amount_18 > max_token_18 {
            let actual_bps = if expected_value_18 > 0 {
                ((token_amount_18 - expected_value_18) * 10_000 / expected_value_18) as u16
            } else {
                u16::MAX
            };
            return Err(ThreePoolError::BurnSlippageExceeded {
                max_bps: args.max_slippage_bps,
                actual_bps,
            });
        }
    }

    // 6. Deduct LP and pool balance BEFORE the async burn call (deduct-before-transfer)
    mutate_state(|s| {
        if let Some(lp) = s.lp_balances.get_mut(&caller) {
            *lp -= args.lp_amount;
        }
        s.lp_total_supply -= args.lp_amount;
        s.balances[token_idx] -= args.token_amount;
    });

    // 7. Burn the token on its ledger (transfer to minting account)
    let burn_result = burn_token_on_ledger(args.token_ledger, args.token_amount).await;

    match burn_result {
        Ok(block_index) => {
            // Log ICRC-3 block for the LP burn
            mutate_state(|s| {
                s.log_block(Icrc3Transaction::Burn {
                    from: caller,
                    amount: args.lp_amount,
                });
            });

            log!(INFO, "AuthorizedRedeemAndBurn: {} burned {} LP, {} {} destroyed (block {})",
                caller, args.lp_amount, args.token_amount, token_symbol, block_index);

            Ok(RedeemAndBurnResult {
                token_amount_burned: args.token_amount,
                lp_amount_burned: args.lp_amount,
                burn_block_index: block_index,
            })
        }
        Err(reason) => {
            // Rollback: restore LP and pool balance
            mutate_state(|s| {
                *s.lp_balances.entry(caller).or_insert(0) += args.lp_amount;
                s.lp_total_supply += args.lp_amount;
                s.balances[token_idx] += args.token_amount;
            });

            log!(INFO, "AuthorizedRedeemAndBurn: FAILED for {} — rolling back. Reason: {}", caller, reason);

            Err(ThreePoolError::BurnFailed {
                token: token_symbol,
                reason,
            })
        }
    }
}

/// Burn tokens by transferring to the minting account (ICRC-1 burn standard).
async fn burn_token_on_ledger(ledger: Principal, amount: u128) -> Result<u64, String> {
    use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
    use icrc_ledger_types::icrc1::account::Account;

    // Query the minting account from the ledger
    let minting_result: Result<(Option<Account>,), _> = ic_cdk::call(
        ledger, "icrc1_minting_account", ()
    ).await;

    let minting_account = match minting_result {
        Ok((Some(account),)) => account,
        Ok((None,)) => {
            return Err("Ledger has no minting account — cannot burn".to_string());
        }
        Err(e) => {
            return Err(format!("Failed to query minting account: {:?}", e));
        }
    };

    let transfer_args = TransferArg {
        to: minting_account,
        amount: amount.into(),
        fee: None,
        memo: None,
        created_at_time: Some(ic_cdk::api::time()),
        from_subaccount: None,
    };

    let result: Result<(Result<candid::Nat, TransferError>,), _> = ic_cdk::call(
        ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match result {
        Ok((Ok(block_index),)) => {
            let idx: u64 = block_index.0.try_into().unwrap_or(0);
            Ok(idx)
        }
        Ok((Err(e),)) => Err(format!("Transfer error: {:?}", e)),
        Err(e) => Err(format!("Call error: {:?}", e)),
    }
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

// ─── ICRC-1 Token Endpoints ───

#[query]
pub fn icrc1_name() -> String {
    icrc_token::icrc1_name()
}

#[query]
pub fn icrc1_symbol() -> String {
    icrc_token::icrc1_symbol()
}

#[query]
pub fn icrc1_decimals() -> u8 {
    icrc_token::icrc1_decimals()
}

#[query]
pub fn icrc1_fee() -> candid::Nat {
    icrc_token::icrc1_fee()
}

#[query]
pub fn icrc1_total_supply() -> candid::Nat {
    icrc_token::icrc1_total_supply()
}

#[query]
pub fn icrc1_minting_account() -> Option<icrc_ledger_types::icrc1::account::Account> {
    icrc_token::icrc1_minting_account()
}

#[query]
pub fn icrc1_balance_of(account: icrc_ledger_types::icrc1::account::Account) -> candid::Nat {
    icrc_token::icrc1_balance_of(account)
}

#[query]
pub fn icrc1_metadata() -> Vec<(String, icrc_ledger_types::icrc::generic_metadata_value::MetadataValue)> {
    icrc_token::icrc1_metadata()
}

#[query]
pub fn icrc1_supported_standards() -> Vec<icrc21::StandardRecord> {
    icrc21::icrc10_supported_standards()
}

#[update]
pub fn icrc1_transfer(
    args: icrc_ledger_types::icrc1::transfer::TransferArg,
) -> Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError> {
    icrc_token::icrc1_transfer(ic_cdk::api::caller(), args)
}

// ─── ICRC-2 Endpoints ───

#[update]
pub fn icrc2_approve(
    args: icrc_ledger_types::icrc2::approve::ApproveArgs,
) -> Result<candid::Nat, icrc_ledger_types::icrc2::approve::ApproveError> {
    icrc_token::icrc2_approve(ic_cdk::api::caller(), args)
}

#[query]
pub fn icrc2_allowance(
    args: icrc_ledger_types::icrc2::allowance::AllowanceArgs,
) -> icrc_ledger_types::icrc2::allowance::Allowance {
    icrc_token::icrc2_allowance(args)
}

#[update]
pub fn icrc2_transfer_from(
    args: icrc_ledger_types::icrc2::transfer_from::TransferFromArgs,
) -> Result<candid::Nat, icrc_ledger_types::icrc2::transfer_from::TransferFromError> {
    icrc_token::icrc2_transfer_from(ic_cdk::api::caller(), args)
}

// ─── ICRC-3 Endpoints ───

#[query]
pub fn icrc3_get_blocks(args: Vec<icrc3::GetBlocksArgs>) -> icrc3::GetBlocksResult {
    icrc3::icrc3_get_blocks(args)
}

#[query]
pub fn icrc3_get_archives(args: icrc3::GetArchivesArgs) -> icrc3::GetArchivesResult {
    icrc3::icrc3_get_archives(args)
}

#[query]
pub fn icrc3_get_tip_certificate() -> Option<icrc3::Icrc3DataCertificate> {
    icrc3::icrc3_get_tip_certificate()
}

#[query]
pub fn icrc3_supported_block_types() -> Vec<icrc3::SupportedBlockType> {
    icrc3::icrc3_supported_block_types()
}
