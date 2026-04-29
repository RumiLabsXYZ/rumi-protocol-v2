use candid::Principal;
use ic_cdk::{query, update, init, pre_upgrade, post_upgrade};
use ic_canister_log::log;
use std::time::Duration;

pub mod types;
pub mod state;
pub mod storage;
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
use crate::state::{mutate_state, read_state, ThreePoolState};
use crate::math::{get_a, virtual_price};
use crate::swap::calc_swap_output;
use crate::liquidity::{calc_add_liquidity, calc_remove_liquidity, calc_remove_one_coin};
use crate::transfers::{transfer_from_user, transfer_to_user};
use crate::logs::INFO;

// ─── Init / Upgrade ───

#[init]
fn init(args: ThreePoolInitArgs) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Reinstall mode
    // wipes stable memory before init runs (per IC spec), so this primarily
    // documents intent and guards against future IC behavior changes.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    mutate_state(|s| s.initialize(args));
    setup_timers();
    log!(INFO, "Rumi 3pool initialized. Admin: {}, A: {}, swap_fee: {} bps",
        read_state(|s| s.config.admin),
        read_state(|s| s.config.initial_a),
        read_state(|s| s.config.swap_fee_bps));
}

#[pre_upgrade]
fn pre_upgrade() {
    log!(INFO, "Rumi 3pool pre-upgrade: flushing SlimState to stable cell");
    let slim = state::snapshot_slim();
    storage::set_slim(slim);
}

#[post_upgrade]
fn post_upgrade() {
    // Step 1: BEFORE touching any storage::* thread-local, try to read a
    // legacy raw-offset-0 Candid blob. The first storage::* access lazily
    // initializes MemoryManager, which destructively writes MGR magic at
    // offset 0, so this read MUST happen first.
    let legacy = storage::migration::read_legacy_blob();

    // The drain is gated on the SlimState `storage_migrated` flag, which
    // is only readable after touching storage::*. We capture the legacy
    // blob first (in RAM), then ask the SlimState cell whether the drain
    // already ran on a previous upgrade. If it has, the legacy bytes (if
    // any) are stale and we discard them.
    let already_drained = storage::get_slim().storage_migrated;

    match legacy {
        Some(legacy_state) if !already_drained => {
            log!(INFO, "Rumi 3pool post-upgrade: draining legacy state");

            // Hydrate heap state with bounded fields from the legacy blob.
            let heap = ThreePoolState {
                config: legacy_state.config.clone(),
                balances: legacy_state.balances,
                admin_fees: legacy_state.admin_fees,
                lp_total_supply: legacy_state.lp_total_supply,
                lp_tx_count: legacy_state.lp_tx_count,
                last_block_hash: legacy_state.last_block_hash,
                is_paused: legacy_state.is_paused,
                is_initialized: legacy_state.is_initialized,
                ..ThreePoolState::default()
            };
            state::replace_state(heap);

            // Drain every collection into stable structures.
            storage::migration::drain_legacy_state(legacy_state);

            // Defensive cross-check: recompute the ICRC-3 hash chain from
            // the drained blocks. Trap on mismatch — better to fail loudly
            // than certify a wrong tip.
            let blocks = storage::blocks::iter_all();
            let recomputed = certification::recompute_hash_chain(&blocks);
            let legacy_tip = read_state(|s| s.last_block_hash);
            if recomputed != legacy_tip {
                ic_cdk::trap(&format!(
                    "post_upgrade drain: ICRC-3 hash chain mismatch. \
                     legacy={:?} recomputed={:?}",
                    legacy_tip, recomputed
                ));
            }

            // Flush SlimState with storage_migrated=true. snapshot_slim
            // reads the cell's current flag (false on first drain), so we
            // override it explicitly afterward.
            let mut slim = state::snapshot_slim();
            slim.storage_migrated = true;
            storage::set_slim(slim);

            log!(INFO, "Rumi 3pool post-upgrade: drain complete. \
                LP supply: {}, holders: {}, blocks: {}, swap_v2: {}",
                read_state(|s| s.lp_total_supply),
                storage::lp_balance_len(),
                storage::blocks::len(),
                storage::swap_v2::len());
        }
        _ => {
            // Normal path. SlimState is already in the stable cell; no
            // legacy blob to drain (or it was stale).
            let slim = storage::get_slim();
            state::hydrate_from_slim(&slim);
            log!(INFO, "Rumi 3pool post-upgrade: loaded from SlimState. \
                LP supply: {}, holders: {}, blocks: {}",
                slim.lp_total_supply,
                storage::lp_balance_len(),
                storage::blocks::len());
        }
    }

    // Set certified ICRC-3 tip from the now-live blocks log.
    if let Some(h) = read_state(|s| s.last_block_hash) {
        let len = storage::blocks::len();
        if len > 0 {
            certification::set_certified_tip(len - 1, &h);
        }
    }

    setup_timers();
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

    let snapshot = read_state(|s| {
        if s.lp_total_supply == 0 {
            return None; // No LPs — virtual_price is meaningless.
        }
        let vp = virtual_price(&s.balances, &precision_muls, amp, s.lp_total_supply)?;
        Some(VirtualPriceSnapshot {
            timestamp_secs: ic_cdk::api::time() / 1_000_000_000,
            virtual_price: vp,
            lp_total_supply: s.lp_total_supply,
        })
    });
    if let Some(snap) = snapshot {
        storage::vp_snap::push(snap);
    } else {
        return;
    }
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
    storage::swap_v1::range(start, length)
}

/// Query total number of swap events.
#[query]
pub fn get_swap_event_count() -> u64 {
    storage::swap_v1::len()
}

/// Query liquidity events for explorer. Returns events in the requested range.
#[query]
pub fn get_liquidity_events(start: u64, length: u64) -> Vec<LiquidityEvent> {
    storage::liq_v1::range(start, length)
}

/// Query total number of liquidity events.
#[query]
pub fn get_liquidity_event_count() -> u64 {
    storage::liq_v1::len()
}

/// Query admin events for explorer. Returns events in the requested range.
#[query]
pub fn get_admin_events(start: u64, length: u64) -> Vec<ThreePoolAdminEvent> {
    storage::admin_ev::range(start, length)
}

/// Query total number of admin events.
#[query]
pub fn get_admin_event_count() -> u64 {
    storage::admin_ev::len()
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
    let (balances, fee_curve, admin_fee_bps, token_i_ledger, token_j_ledger, token_j_symbol) =
        read_state(|s| {
            (
                s.balances,
                s.config.fee_curve.unwrap_or_default(),
                s.config.admin_fee_bps,
                s.config.tokens[i_idx].ledger_id,
                s.config.tokens[j_idx].ledger_id,
                s.config.tokens[j_idx].symbol.clone(),
            )
        });

    // 4. Calculate swap output using the dynamic fee curve
    let outcome =
        calc_swap_output(i_idx, j_idx, dx, &balances, &precision_muls, amp, &fee_curve, admin_fee_bps)?;
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
    //
    // The pool sent `output` to the user and reserves `admin_fee_share` of the
    // fee for admin withdrawal. The LP-fee portion (`fee - admin_fee_share`)
    // stays inside `s.balances[j_idx]` so it accrues to LPs via virtual_price.
    // Therefore the internal balance must only decrease by `output + admin_fee_share`,
    // not by `output + fee` (which would double-deduct the LP fee).
    let admin_fee_share = fee * (admin_fee_bps as u128) / 10_000;
    mutate_state(|s| {
        s.balances[i_idx] += dx;
        s.balances[j_idx] -= output + admin_fee_share;
        s.admin_fees[j_idx] += admin_fee_share;

        // Compute virtual price after the swap. `None` (e.g. lp_total_supply==0
        // or invariant fails to converge) falls back to 0 as a sentinel.
        let lp_supply = s.lp_total_supply;
        let vp_after = virtual_price(&s.balances, &precision_muls, amp, lp_supply).unwrap_or(0);
        let balances_after = s.balances;

        // Record swap event v2 (dynamic-fee schema). Appends to the stable
        // swap_v2 log. `migrated: false` distinguishes live writes from the
        // one-shot backfill populated during the Phase A drain.
        let id = storage::swap_v2::len();
        storage::swap_v2::push(SwapEventV2 {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            token_in: i,
            token_out: j,
            amount_in: dx,
            amount_out: output,
            fee,
            fee_bps: outcome.fee_bps_used,
            imbalance_before: outcome.imbalance_before,
            imbalance_after: outcome.imbalance_after,
            is_rebalancing: outcome.is_rebalancing,
            pool_balances_after: balances_after,
            virtual_price_after: vp_after,
            migrated: false,
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
    let (old_balances, lp_total_supply, fee_curve) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.fee_curve.unwrap_or_default())
    });

    // 5. Calculate LP tokens to mint (dynamic fee curve)
    let liq_outcome = calc_add_liquidity(
        &amounts_arr,
        &old_balances,
        &precision_muls,
        lp_total_supply,
        amp,
        &fee_curve,
    )?;
    let lp_minted = liq_outcome.lp_minted;

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
        let cur = storage::lp_balance_get(&caller);
        storage::lp_balance_set(caller, cur + lp_minted);
        s.lp_total_supply += lp_minted;
        s.is_initialized = true;
        // Log mint block for ICRC-3 index
        s.log_block(Icrc3Transaction::Mint {
            to: caller,
            amount: lp_minted,
            to_subaccount: None,
        });
    });

    // Record liquidity event v2 (dynamic-fee schema). v1 writes are stopped —
    // v1 entries remain as frozen historical state for the migration.
    mutate_state(|s| {
        let lp_supply = s.lp_total_supply;
        let vp_after = virtual_price(&s.balances, &precision_muls, amp, lp_supply).unwrap_or(0);
        let balances_after = s.balances;
        let id = storage::liq_v2::len();
        storage::liq_v2::push(LiquidityEventV2 {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::AddLiquidity,
            amounts: amounts_arr,
            lp_amount: lp_minted,
            coin_index: None,
            fee: None,
            fee_bps: Some(liq_outcome.fee_bps_used),
            imbalance_before: liq_outcome.imbalance_before,
            imbalance_after: liq_outcome.imbalance_after,
            is_rebalancing: liq_outcome.is_rebalancing,
            pool_balances_after: balances_after,
            virtual_price_after: vp_after,
            migrated: false,
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
    let user_lp = storage::lp_balance_get(&caller);
    let (balances, lp_total_supply) = read_state(|s| (s.balances, s.lp_total_supply));

    if user_lp < lp_burn {
        return Err(ThreePoolError::InsufficientLiquidity);
    }

    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }

    let precision_muls = get_precision_muls();
    let amp = get_current_a();
    let imbalance_before = crate::math::compute_imbalance(&balances, &precision_muls);

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
        let cur = storage::lp_balance_get(&caller);
        storage::lp_balance_set(caller, cur - lp_burn);
        s.lp_total_supply -= lp_burn;
        for k in 0..3 {
            s.balances[k] -= amounts[k];
        }
        // Log burn block for ICRC-3 index
        s.log_block(Icrc3Transaction::Burn {
            from: caller,
            amount: lp_burn,
            from_subaccount: None,
        });
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

    // Record liquidity event v2. Proportional remove preserves pool weights,
    // so it is neither rebalancing nor imbalancing and pays no fee.
    mutate_state(|s| {
        let lp_supply = s.lp_total_supply;
        let vp_after = virtual_price(&s.balances, &precision_muls, amp, lp_supply).unwrap_or(0);
        let balances_after = s.balances;
        let imbalance_after = crate::math::compute_imbalance(&balances_after, &precision_muls);
        let id = storage::liq_v2::len();
        storage::liq_v2::push(LiquidityEventV2 {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::RemoveLiquidity,
            amounts,
            lp_amount: lp_burn,
            coin_index: None,
            fee: None,
            fee_bps: None,
            imbalance_before,
            imbalance_after,
            is_rebalancing: false,
            pool_balances_after: balances_after,
            virtual_price_after: vp_after,
            migrated: false,
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
    let user_lp = storage::lp_balance_get(&caller);
    let (balances, lp_total_supply) = read_state(|s| (s.balances, s.lp_total_supply));

    if user_lp < lp_burn {
        return Err(ThreePoolError::InsufficientLiquidity);
    }

    // 2. Compute A, precision_muls
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (fee_curve, admin_fee_bps) = read_state(|s| {
        (s.config.fee_curve.unwrap_or_default(), s.config.admin_fee_bps)
    });

    // 3. Calculate withdrawal (dynamic fee curve)
    let roc_outcome = calc_remove_one_coin(
        lp_burn,
        idx,
        &balances,
        &precision_muls,
        lp_total_supply,
        amp,
        &fee_curve,
        admin_fee_bps,
    )?;
    let amount = roc_outcome.amount_native;
    let fee = roc_outcome.fee_native;

    // 4. Slippage check
    if amount < min_amount {
        return Err(ThreePoolError::SlippageExceeded);
    }

    // 5. Deduct LP and balance first
    let admin_fee_share = fee * (admin_fee_bps as u128) / 10_000;

    // The pool sends `amount` to the user and reserves `admin_fee_share` for
    // admin withdrawal. The LP-fee portion stays inside `s.balances[idx]` so
    // virtual_price grows for remaining LPs. Subtracting `amount + fee` would
    // double-deduct the LP fee.
    mutate_state(|s| {
        let cur = storage::lp_balance_get(&caller);
        storage::lp_balance_set(caller, cur - lp_burn);
        s.lp_total_supply -= lp_burn;
        s.balances[idx] -= amount + admin_fee_share;
        s.admin_fees[idx] += admin_fee_share;
        // Log burn block for ICRC-3 index
        s.log_block(Icrc3Transaction::Burn {
            from: caller,
            amount: lp_burn,
            from_subaccount: None,
        });
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

    // Record liquidity event v2 (dynamic-fee schema).
    mutate_state(|s| {
        let lp_supply = s.lp_total_supply;
        let vp_after = virtual_price(&s.balances, &precision_muls, amp, lp_supply).unwrap_or(0);
        let balances_after = s.balances;
        let mut amounts = [0u128; 3];
        amounts[idx] = amount;
        let id = storage::liq_v2::len();
        storage::liq_v2::push(LiquidityEventV2 {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::RemoveOneCoin,
            amounts,
            lp_amount: lp_burn,
            coin_index: Some(coin_index),
            fee: Some(fee),
            fee_bps: Some(roc_outcome.fee_bps_used),
            imbalance_before: roc_outcome.imbalance_before,
            imbalance_after: roc_outcome.imbalance_after,
            is_rebalancing: roc_outcome.is_rebalancing,
            pool_balances_after: balances_after,
            virtual_price_after: vp_after,
            migrated: false,
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

    let precision_muls = get_precision_muls();
    let amp = get_current_a();
    let imbalance_before =
        read_state(|s| crate::math::compute_imbalance(&s.balances, &precision_muls));

    // Update balance — NO LP minted
    mutate_state(|s| {
        s.balances[idx] += amount;
    });

    // Record liquidity event v2
    mutate_state(|s| {
        let lp_supply = s.lp_total_supply;
        let vp_after = virtual_price(&s.balances, &precision_muls, amp, lp_supply).unwrap_or(0);
        let balances_after = s.balances;
        let imbalance_after = crate::math::compute_imbalance(&balances_after, &precision_muls);
        let mut amounts = [0u128; 3];
        amounts[idx] = amount;
        let id = storage::liq_v2::len();
        storage::liq_v2::push(LiquidityEventV2 {
            id,
            timestamp: ic_cdk::api::time(),
            caller,
            action: LiquidityAction::Donate,
            amounts,
            lp_amount: 0,
            coin_index: Some(token_index),
            fee: None,
            fee_bps: None,
            imbalance_before,
            imbalance_after,
            is_rebalancing: imbalance_after < imbalance_before,
            pool_balances_after: balances_after,
            virtual_price_after: vp_after,
            migrated: false,
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
    storage::lp_balance_get(&user)
}

/// Returns all LP holders and their balances, sorted by balance descending.
#[query]
pub fn get_all_lp_holders() -> Vec<(Principal, u128)> {
    let mut holders: Vec<(Principal, u128)> = storage::lp_balance_iter()
        .into_iter()
        .filter(|(_, b)| *b > 0)
        .collect();
    holders.sort_by(|a, b| b.1.cmp(&a.1));
    holders
}

#[query]
pub fn calc_swap(i: u8, j: u8, dx: u128) -> Result<u128, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, fee_curve, admin_fee_bps) = read_state(|s| {
        (s.balances, s.config.fee_curve.unwrap_or_default(), s.config.admin_fee_bps)
    });

    let outcome =
        calc_swap_output(i as usize, j as usize, dx, &balances, &precision_muls, amp, &fee_curve, admin_fee_bps)?;

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
    let (old_balances, lp_total_supply, fee_curve) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.fee_curve.unwrap_or_default())
    });
    let outcome = calc_add_liquidity(
        &amounts_arr, &old_balances, &precision_muls, lp_total_supply, amp, &fee_curve,
    )?;
    let _ = min_lp; // reserved for future use
    Ok(outcome.lp_minted)
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
    let (balances, lp_total_supply, fee_curve, admin_fee_bps) = read_state(|s| {
        (s.balances, s.lp_total_supply, s.config.fee_curve.unwrap_or_default(), s.config.admin_fee_bps)
    });
    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }
    let outcome = calc_remove_one_coin(
        lp_burn, idx, &balances, &precision_muls, lp_total_supply, amp, &fee_curve, admin_fee_bps,
    )?;
    Ok(outcome.amount_native)
}

#[query]
pub fn get_admin_fees() -> Vec<u128> {
    read_state(|s| s.admin_fees.to_vec())
}

/// Returns all virtual_price snapshots for APY calculation and historical charts.
#[query]
pub fn get_vp_snapshots() -> Vec<VirtualPriceSnapshot> {
    storage::vp_snap::iter_all()
}

// ─── Bot Query Endpoints ───
//
// Pure helpers below are independently unit-tested. The `#[query]` wrappers
// just plumb live state into them.

/// Pure quote_swap: simulates a swap against the supplied balances.
pub fn pure_quote_swap(
    i: u8,
    j: u8,
    dx: u128,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_curve: &FeeCurveParams,
    lp_total_supply: u128,
    admin_fee_bps: u64,
) -> Result<QuoteSwapResult, ThreePoolError> {
    let i_idx = i as usize;
    let j_idx = j as usize;
    let outcome = calc_swap_output(i_idx, j_idx, dx, balances, precision_muls, amp, fee_curve, admin_fee_bps)?;
    let vp_before = virtual_price(balances, precision_muls, amp, lp_total_supply).unwrap_or(0);
    // Mirror on-chain accounting: only `output + admin_fee_share` leaves the
    // pool. The LP-fee portion stays in `s.balances[j]` and accrues to VP.
    let admin_fee_share = outcome.fee_native * (admin_fee_bps as u128) / 10_000;
    let mut balances_after = *balances;
    balances_after[i_idx] = balances_after[i_idx].saturating_add(dx);
    balances_after[j_idx] = balances_after[j_idx]
        .saturating_sub(outcome.output_native + admin_fee_share);
    let vp_after = virtual_price(&balances_after, precision_muls, amp, lp_total_supply).unwrap_or(0);
    Ok(QuoteSwapResult {
        token_in: i,
        token_out: j,
        amount_in: dx,
        amount_out: outcome.output_native,
        fee_native: outcome.fee_native,
        fee_bps: outcome.fee_bps_used,
        imbalance_before: outcome.imbalance_before,
        imbalance_after: outcome.imbalance_after,
        is_rebalancing: outcome.is_rebalancing,
        virtual_price_before: vp_before,
        virtual_price_after: vp_after,
    })
}

/// Pure quote_optimal_rebalance: ternary search over dx in [1, balances[i]].
pub fn pure_quote_optimal_rebalance(
    i: u8,
    j: u8,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_curve: &FeeCurveParams,
    admin_fee_bps: u64,
) -> Result<OptimalRebalanceQuote, ThreePoolError> {
    let i_idx = i as usize;
    let j_idx = j as usize;
    if i_idx >= 3 || j_idx >= 3 || i_idx == j_idx {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    let imbalance_before = crate::math::compute_imbalance(balances, precision_muls);
    let upper_bound = balances[i_idx];
    let empty = OptimalRebalanceQuote {
        token_in: i,
        token_out: j,
        dx: 0,
        amount_out: 0,
        fee_bps: 0,
        imbalance_before,
        imbalance_after: imbalance_before,
        profit_bps_estimate: 0,
    };
    if upper_bound == 0 {
        return Ok(empty);
    }
    let probe = (precision_muls[i_idx] as u128).max(1).min(upper_bound);
    let probe_outcome =
        calc_swap_output(i_idx, j_idx, probe, balances, precision_muls, amp, fee_curve, admin_fee_bps);
    if !matches!(&probe_outcome, Ok(o) if o.is_rebalancing) {
        return Ok(empty);
    }

    let try_dx = |dx: u128| -> Option<crate::swap::SwapOutcome> {
        if dx == 0 { return None; }
        calc_swap_output(i_idx, j_idx, dx, balances, precision_muls, amp, fee_curve, admin_fee_bps).ok()
    };
    let drop_for = |dx: u128| -> i128 {
        match try_dx(dx) {
            Some(o) if o.is_rebalancing => imbalance_before as i128 - o.imbalance_after as i128,
            _ => i128::MIN,
        }
    };

    let mut lo: u128 = 1;
    let mut hi: u128 = upper_bound;
    for _ in 0..50 {
        if hi <= lo + 1 { break; }
        let m1 = lo + (hi - lo) / 3;
        let m2 = hi - (hi - lo) / 3;
        if drop_for(m1) < drop_for(m2) {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    let mid = lo + (hi - lo) / 2;
    let mut best_dx: u128 = 0;
    let mut best_drop: i128 = -1;
    for cand in [lo, mid, hi] {
        let d = drop_for(cand);
        if d > best_drop {
            best_drop = d;
            best_dx = cand;
        }
    }
    if best_dx == 0 || best_drop <= 0 {
        return Ok(empty);
    }
    let outcome = try_dx(best_dx).expect("best_dx valid");
    Ok(OptimalRebalanceQuote {
        token_in: i,
        token_out: j,
        dx: best_dx,
        amount_out: outcome.output_native,
        fee_bps: outcome.fee_bps_used,
        imbalance_before,
        imbalance_after: outcome.imbalance_after,
        profit_bps_estimate: imbalance_before - outcome.imbalance_after,
    })
}

/// Pure simulate_swap_path: applies hops against a local mutable balances copy.
pub fn pure_simulate_swap_path(
    path: &[(u8, u8, u128)],
    initial_balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_curve: &FeeCurveParams,
    lp_total_supply: u128,
    admin_fee_bps: u64,
) -> Result<Vec<QuoteSwapResult>, ThreePoolError> {
    let mut balances = *initial_balances;
    let mut out = Vec::with_capacity(path.len());
    for &(i, j, dx) in path {
        let i_idx = i as usize;
        let j_idx = j as usize;
        if i_idx >= 3 || j_idx >= 3 || i_idx == j_idx {
            return Err(ThreePoolError::InvalidCoinIndex);
        }
        let q = pure_quote_swap(i, j, dx, &balances, precision_muls, amp, fee_curve, lp_total_supply, admin_fee_bps)?;
        // Mirror on-chain accounting: only `amount_out + admin_fee_share` leaves
        // the pool. LP fee stays in the pool and grows VP for subsequent hops.
        let admin_fee_share = q.fee_native * (admin_fee_bps as u128) / 10_000;
        balances[i_idx] = balances[i_idx].saturating_add(dx);
        balances[j_idx] = balances[j_idx].saturating_sub(q.amount_out + admin_fee_share);
        out.push(q);
    }
    Ok(out)
}



/// Quote a swap without mutating state. Wraps `calc_swap_output` and adds the
/// virtual_price impact of the simulated trade so a bot can score it.
#[query]
pub fn quote_swap(i: u8, j: u8, dx: u128) -> Result<QuoteSwapResult, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, fee_curve, lp_total_supply, admin_fee_bps) = read_state(|s| {
        (s.balances, s.config.fee_curve.unwrap_or_default(), s.lp_total_supply, s.config.admin_fee_bps)
    });
    pure_quote_swap(i, j, dx, &balances, &precision_muls, amp, &fee_curve, lp_total_supply, admin_fee_bps)
}

/// Snapshot of the live pool: balances, weights, imbalance, vp, fee curve, A.
#[query]
pub fn get_pool_state() -> PoolStateView {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    read_state(|s| {
        let xp = crate::math::normalize_all(&s.balances, &precision_muls);
        let normalized_balances: [u128; 3] = [
            xp[0].as_u128(),
            xp[1].as_u128(),
            xp[2].as_u128(),
        ];
        let imbalance = crate::math::compute_imbalance(&s.balances, &precision_muls);
        let vp = virtual_price(&s.balances, &precision_muls, amp, s.lp_total_supply).unwrap_or(0);
        PoolStateView {
            balances: s.balances,
            normalized_balances,
            imbalance,
            virtual_price: vp,
            lp_total_supply: s.lp_total_supply,
            fee_curve: s.config.fee_curve.unwrap_or_default(),
            amp,
        }
    })
}

/// Live fee curve parameters.
#[query]
pub fn get_fee_curve_params() -> FeeCurveParams {
    read_state(|s| s.config.fee_curve.unwrap_or_default())
}

/// Ternary search for the dx that maximally reduces imbalance for a swap from
/// token `i` to token `j`, constrained to strictly rebalancing trades. Returns
/// dx=0 when no rebalancing trade exists.
#[query]
pub fn quote_optimal_rebalance(i: u8, j: u8) -> Result<OptimalRebalanceQuote, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, fee_curve, admin_fee_bps) = read_state(|s| {
        (s.balances, s.config.fee_curve.unwrap_or_default(), s.config.admin_fee_bps)
    });
    pure_quote_optimal_rebalance(i, j, &balances, &precision_muls, amp, &fee_curve, admin_fee_bps)
}

/// Sequentially apply a list of swap quotes against a local mutable copy of
/// the pool balances. Does not mutate canister state. Returns one
/// `QuoteSwapResult` per hop, in order. If any hop fails the entire call
/// errors.
#[query]
pub fn simulate_swap_path(path: Vec<(u8, u8, u128)>) -> Result<Vec<QuoteSwapResult>, ThreePoolError> {
    let amp = get_current_a();
    let precision_muls = get_precision_muls();
    let (balances, fee_curve, lp_total_supply, admin_fee_bps) = read_state(|s| {
        (s.balances, s.config.fee_curve.unwrap_or_default(), s.lp_total_supply, s.config.admin_fee_bps)
    });
    pure_simulate_swap_path(&path, &balances, &precision_muls, amp, &fee_curve, lp_total_supply, admin_fee_bps)
}

/// Returns merged imbalance snapshots from swap and liquidity v2 events,
/// sorted newest-first, paginated. Each snapshot is a (timestamp,
/// imbalance_after, virtual_price_after, kind) tuple.
#[query]
pub fn get_imbalance_history(limit: u64, offset: u64) -> Vec<ImbalanceSnapshot> {
    read_state(|s| {
        let mut all: Vec<ImbalanceSnapshot> = Vec::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated) {
            all.push(ImbalanceSnapshot {
                timestamp: e.timestamp,
                imbalance_after: e.imbalance_after,
                virtual_price_after: e.virtual_price_after,
                event_kind: ImbalanceEventKind::Swap,
            });
        }
        for e in s.liquidity_events_v2().iter().filter(|e| !e.migrated) {
            all.push(ImbalanceSnapshot {
                timestamp: e.timestamp,
                imbalance_after: e.imbalance_after,
                virtual_price_after: e.virtual_price_after,
                event_kind: ImbalanceEventKind::Liquidity,
            });
        }
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        let total = all.len() as u64;
        if offset >= total {
            return Vec::new();
        }
        let start = offset as usize;
        let end = ((offset + limit).min(total)) as usize;
        all[start..end].to_vec()
    })
}

/// Total number of v2 liquidity events.
#[query]
pub fn get_liquidity_event_count_v2() -> u64 {
    storage::liq_v2::len()
}

/// Paginated newest-first read of the v2 liquidity event log.
#[query]
pub fn get_liquidity_events_v2(limit: u64, offset: u64) -> Vec<LiquidityEventV2> {
    read_state(|s| {
        let events = s.liquidity_events_v2();
        let total = events.len() as u64;
        if offset >= total {
            return Vec::new();
        }
        let take = ((total - offset).min(limit)) as usize;
        let end = (total - offset) as usize;
        let start = end - take;
        let mut out: Vec<LiquidityEventV2> = events[start..end].to_vec();
        out.reverse();
        out
    })
}

/// Paginated newest-first read of the v2 swap event log.
#[query]
pub fn get_swap_events_v2(limit: u64, offset: u64) -> Vec<SwapEventV2> {
    read_state(|s| {
        let events = s.swap_events_v2();
        let total = events.len() as u64;
        if offset >= total {
            return Vec::new();
        }
        // Newest first: index from the end.
        let take = ((total - offset).min(limit)) as usize;
        let end = (total - offset) as usize;
        let start = end - take;
        let mut out: Vec<SwapEventV2> = events[start..end].to_vec();
        out.reverse();
        out
    })
}

// ─── Explorer Endpoints (E1-E14) ───
//
// These are pure query methods over the v2 event log. Aggregations iterate
// the in-memory Vec, which is fine at current event volumes. If aggregation
// performance ever becomes hot we can cache running totals on event append.

/// Returns the cutoff timestamp (ns) such that events with `timestamp >= cutoff`
/// fall inside the requested window. `AllTime` returns 0.
fn window_cutoff_ns(window: StatsWindow, now: u64) -> u64 {
    let secs: u64 = match window {
        StatsWindow::Last24h => 24 * 3600,
        StatsWindow::Last7d => 7 * 24 * 3600,
        StatsWindow::Last30d => 30 * 24 * 3600,
        StatsWindow::AllTime => return 0,
    };
    now.saturating_sub(secs * 1_000_000_000)
}

/// Floor a nanosecond timestamp to the start of its bucket. Returns 0 if
/// `bucket_secs == 0` (caller should validate).
fn bucket_floor(ts_ns: u64, bucket_secs: u64) -> u64 {
    if bucket_secs == 0 {
        return 0;
    }
    let bucket_ns = bucket_secs.saturating_mul(1_000_000_000);
    (ts_ns / bucket_ns) * bucket_ns
}

// ── E1: liquidity events by principal ──
#[query]
pub fn get_liquidity_events_by_principal(
    principal: Principal,
    start: u64,
    length: u64,
) -> Vec<LiquidityEventV2> {
    read_state(|s| {
        s.liquidity_events_v2()
            .iter()
            .filter(|e| e.caller == principal)
            .skip(start as usize)
            .take(length as usize)
            .cloned()
            .collect()
    })
}

// ── E2: swap events by principal ──
#[query]
pub fn get_swap_events_by_principal(
    principal: Principal,
    start: u64,
    length: u64,
) -> Vec<SwapEventV2> {
    read_state(|s| {
        s.swap_events_v2()
            .iter()
            .filter(|e| e.caller == principal)
            .skip(start as usize)
            .take(length as usize)
            .cloned()
            .collect()
    })
}

// ── E3: swap events in a time range ──
#[query]
pub fn get_swap_events_by_time_range(
    from_ts: u64,
    to_ts: u64,
    limit: u64,
) -> Vec<SwapEventV2> {
    read_state(|s| {
        s.swap_events_v2()
            .iter()
            .filter(|e| e.timestamp >= from_ts && e.timestamp < to_ts)
            .take(limit as usize)
            .cloned()
            .collect()
    })
}

// (E4 = `get_admin_events`, defined above with the other event readers.)

// ─── Aggregated stats (E5-E9) ───

// ── E5: pool stats over a window ──
#[query]
pub fn get_pool_stats(window: StatsWindow) -> PoolStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut volume_per_token = [0u128; 3];
        let mut fees = [0u128; 3];
        let mut count = 0u64;
        let mut arb_count = 0u64;
        let mut arb_volume = [0u128; 3];
        let mut weighted_fee_bps: u128 = 0;
        let mut total_volume: u128 = 0;
        let mut swappers: std::collections::BTreeSet<Principal> =
            std::collections::BTreeSet::new();

        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            count += 1;
            let ti = e.token_in as usize;
            let to = e.token_out as usize;
            volume_per_token[ti] = volume_per_token[ti].saturating_add(e.amount_in);
            fees[to] = fees[to].saturating_add(e.fee);
            weighted_fee_bps =
                weighted_fee_bps.saturating_add((e.fee_bps as u128).saturating_mul(e.amount_in));
            total_volume = total_volume.saturating_add(e.amount_in);
            swappers.insert(e.caller);
            if e.is_rebalancing {
                arb_count += 1;
                arb_volume[ti] = arb_volume[ti].saturating_add(e.amount_in);
            }
        }

        let mut adds = 0u64;
        let mut removes = 0u64;
        for e in s
            .liquidity_events_v2()
            .iter()
            .filter(|e| !e.migrated && e.timestamp >= cutoff)
        {
            match e.action {
                LiquidityAction::AddLiquidity => adds += 1,
                LiquidityAction::RemoveLiquidity | LiquidityAction::RemoveOneCoin => removes += 1,
                LiquidityAction::Donate => {}
            }
        }

        PoolStats {
            swap_count: count,
            swap_volume_per_token: volume_per_token,
            total_fees_collected: fees,
            unique_swappers: swappers.len() as u64,
            liquidity_added_count: adds,
            liquidity_removed_count: removes,
            avg_fee_bps: if total_volume == 0 {
                0
            } else {
                (weighted_fee_bps / total_volume) as u32
            },
            arb_swap_count: arb_count,
            arb_volume_per_token: arb_volume,
        }
    })
}

// ── E6: imbalance stats over a window ──
#[query]
pub fn get_imbalance_stats(window: StatsWindow) -> ImbalanceStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    let precision_muls = get_precision_muls();
    read_state(|s| {
        let current = crate::math::compute_imbalance(&s.balances, &precision_muls);
        let mut min_v = u64::MAX;
        let mut max_v = 0u64;
        let mut sum: u128 = 0;
        let mut count: u128 = 0;
        let mut samples: Vec<(u64, u64)> = Vec::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            let imb = e.imbalance_after;
            if imb < min_v {
                min_v = imb;
            }
            if imb > max_v {
                max_v = imb;
            }
            sum += imb as u128;
            count += 1;
            samples.push((e.timestamp, imb));
        }
        ImbalanceStats {
            current,
            min: if count == 0 { current } else { min_v },
            max: if count == 0 { current } else { max_v },
            avg: if count == 0 { current } else { (sum / count) as u64 },
            samples,
        }
    })
}

// ── E7: fee bucket distribution + rebalancing share ──
#[query]
pub fn get_fee_stats(window: StatsWindow) -> FeeStats {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    // Half-open buckets [lo, hi) covering 1..=99 bps. The last bucket is
    // intentionally [75, 100) so a fee_bps == 99 lands in it.
    let bucket_edges: [(u16, u16); 5] = [(1, 10), (10, 25), (25, 50), (50, 75), (75, 100)];
    read_state(|s| {
        let mut buckets: Vec<FeeBucket> = bucket_edges
            .iter()
            .map(|(lo, hi)| FeeBucket {
                min_bps: *lo,
                max_bps: *hi,
                swap_count: 0,
                volume_per_token: [0; 3],
            })
            .collect();
        let mut rebalancing = 0u64;
        let mut total = 0u64;
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            total += 1;
            if e.is_rebalancing {
                rebalancing += 1;
            }
            for b in buckets.iter_mut() {
                if e.fee_bps >= b.min_bps && e.fee_bps < b.max_bps {
                    b.swap_count += 1;
                    let ti = e.token_in as usize;
                    b.volume_per_token[ti] = b.volume_per_token[ti].saturating_add(e.amount_in);
                    break;
                }
            }
        }
        let pct = if total == 0 {
            0
        } else {
            ((rebalancing.saturating_mul(10_000)) / total) as u32
        };
        FeeStats {
            buckets,
            rebalancing_swap_count: rebalancing,
            rebalancing_swap_pct: pct,
        }
    })
}

// ── E8: top swappers by volume in a window ──
#[query]
pub fn get_top_swappers(window: StatsWindow, limit: u64) -> Vec<(Principal, u64, u128)> {
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut acc: std::collections::BTreeMap<Principal, (u64, u128)> =
            std::collections::BTreeMap::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            let entry = acc.entry(e.caller).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(e.amount_in);
        }
        let mut v: Vec<(Principal, u64, u128)> = acc
            .into_iter()
            .map(|(p, (c, vol))| (p, c, vol))
            .collect();
        v.sort_by(|a, b| b.2.cmp(&a.2));
        v.truncate(limit as usize);
        v
    })
}

// ── E9: top LP holders by balance ──
#[query]
pub fn get_top_lps(limit: u64) -> Vec<(Principal, u128, u32)> {
    let total = read_state(|s| s.lp_total_supply).max(1);
    let mut v: Vec<(Principal, u128, u32)> = storage::lp_balance_iter()
        .into_iter()
        .map(|(p, lp)| (p, lp, ((lp.saturating_mul(10_000)) / total) as u32))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(limit as usize);
    v
}

// ─── Time series (E10-E13) ───

// ── E10: bucketed swap volume series ──
#[query]
pub fn get_volume_series(window: StatsWindow, bucket_seconds: u64) -> Vec<VolumePoint> {
    if bucket_seconds == 0 {
        return Vec::new();
    }
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut map: std::collections::BTreeMap<u64, [u128; 3]> =
            std::collections::BTreeMap::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            let entry = map.entry(bucket).or_insert([0; 3]);
            let ti = e.token_in as usize;
            entry[ti] = entry[ti].saturating_add(e.amount_in);
        }
        map.into_iter()
            .map(|(t, v)| VolumePoint {
                timestamp: t,
                volume_per_token: v,
            })
            .collect()
    })
}

// ── E11: bucketed pool balance series (last balance in each bucket wins) ──
#[query]
pub fn get_balance_series(window: StatsWindow, bucket_seconds: u64) -> Vec<BalancePoint> {
    if bucket_seconds == 0 {
        return Vec::new();
    }
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut map: std::collections::BTreeMap<u64, [u128; 3]> =
            std::collections::BTreeMap::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            map.insert(bucket, e.pool_balances_after);
        }
        map.into_iter()
            .map(|(t, b)| BalancePoint {
                timestamp: t,
                balances: b,
            })
            .collect()
    })
}

// ── E12: bucketed virtual price series (sourced from VP snapshots) ──
#[query]
pub fn get_virtual_price_series(
    window: StatsWindow,
    bucket_seconds: u64,
) -> Vec<VirtualPricePoint> {
    if bucket_seconds == 0 {
        return Vec::new();
    }
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    let snapshots = storage::vp_snap::iter_all();
    let mut map: std::collections::BTreeMap<u64, u128> = std::collections::BTreeMap::new();
    for snap in snapshots.iter() {
        let ts_ns = snap.timestamp_secs.saturating_mul(1_000_000_000);
        if ts_ns < cutoff {
            continue;
        }
        let bucket = bucket_floor(ts_ns, bucket_seconds);
        map.insert(bucket, snap.virtual_price);
    }
    map.into_iter()
        .map(|(t, vp)| VirtualPricePoint {
            timestamp: t,
            virtual_price: vp,
        })
        .collect()
}

// ── E13: bucketed average fee bps series (volume-weighted) ──
#[query]
pub fn get_fee_series(window: StatsWindow, bucket_seconds: u64) -> Vec<FeePoint> {
    if bucket_seconds == 0 {
        return Vec::new();
    }
    let now = ic_cdk::api::time();
    let cutoff = window_cutoff_ns(window, now);
    read_state(|s| {
        let mut sums: std::collections::BTreeMap<u64, (u128, u128)> =
            std::collections::BTreeMap::new();
        for e in s.swap_events_v2().iter().filter(|e| !e.migrated && e.timestamp >= cutoff) {
            let bucket = bucket_floor(e.timestamp, bucket_seconds);
            let entry = sums.entry(bucket).or_insert((0, 0));
            entry.0 = entry
                .0
                .saturating_add((e.fee_bps as u128).saturating_mul(e.amount_in));
            entry.1 = entry.1.saturating_add(e.amount_in);
        }
        sums.into_iter()
            .map(|(t, (w, v))| FeePoint {
                timestamp: t,
                avg_fee_bps: if v == 0 { 0 } else { (w / v) as u32 },
            })
            .collect()
    })
}

// ─── Pool health (E14) ───

#[query]
pub fn get_pool_health() -> PoolHealth {
    let now = ic_cdk::api::time();
    let precision_muls = get_precision_muls();
    read_state(|s| {
        let current_imbalance = crate::math::compute_imbalance(&s.balances, &precision_muls);
        let params = s.config.fee_curve.unwrap_or_default();

        // imbalance_trend_1h: compare current vs the imbalance recorded by the
        // first swap that landed in the last hour. If no swaps in the last
        // hour, trend is 0.
        let one_hour_ago = now.saturating_sub(3600 * 1_000_000_000);
        let past_imb = s
            .swap_events_v2()
            .iter()
            .find(|e| !e.migrated && e.timestamp >= one_hour_ago)
            .map(|e| e.imbalance_before)
            .unwrap_or(current_imbalance);
        let trend = current_imbalance as i64 - past_imb as i64;
        let imbalance_trend_1h: i32 = trend.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        let last_swap_age_seconds = s
            .swap_events_v2()
            .iter()
            .rev()
            .find(|e| !e.migrated)
            .map(|e| now.saturating_sub(e.timestamp) / 1_000_000_000)
            .unwrap_or(u64::MAX);

        // fee_at_max_imbalance_swap: fee that would be charged if a hypothetical
        // worst-case imbalancing trade pushed imbalance to saturation.
        let fee_at_max =
            crate::math::compute_fee_bps(current_imbalance, params.imb_saturation, &params);

        // arb_opportunity_score: linear in current imbalance up to saturation.
        let score = if params.imb_saturation == 0 {
            0
        } else {
            ((current_imbalance.min(params.imb_saturation) as u128 * 100)
                / params.imb_saturation as u128) as u8
        };

        PoolHealth {
            current_imbalance,
            imbalance_trend_1h,
            last_swap_age_seconds,
            fee_at_min: params.min_fee_bps,
            fee_at_max_imbalance_swap: fee_at_max,
            arb_opportunity_score: score,
        }
    })
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

#[update]
pub fn set_fee_curve_params(params: crate::types::FeeCurveParams) -> Result<(), ThreePoolError> {
    let caller = ic_cdk::api::caller();
    admin::set_fee_curve_params(caller, params)
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
    if !storage::burn_caller_contains(&caller) {
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
    let caller_lp = storage::lp_balance_get(&caller);
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
        let cur = storage::lp_balance_get(&caller);
        storage::lp_balance_set(caller, cur.saturating_sub(args.lp_amount));
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
                    from_subaccount: None,
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
                let cur = storage::lp_balance_get(&caller);
                storage::lp_balance_set(caller, cur + args.lp_amount);
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
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            // Audit Wave-3: a Duplicate from the ledger means the burn already
            // landed at `duplicate_of`. The corresponding tokens are already
            // out of supply, so return success.
            let idx: u64 = duplicate_of.0.try_into().unwrap_or(0);
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

#[cfg(test)]
mod bot_endpoint_tests {
    use super::*;
    use crate::swap::calc_swap_output;

    fn precision_muls() -> [u64; 3] {
        [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000]
    }

    fn balanced_pool() -> [u128; 3] {
        [1_000_000 * 100_000_000, 1_000_000 * 1_000_000, 1_000_000 * 1_000_000]
    }

    fn imbalanced_pool() -> [u128; 3] {
        [2_000_000 * 100_000_000, 500_000 * 1_000_000, 500_000 * 1_000_000]
    }

    #[test]
    fn quote_swap_matches_calc_swap_output() {
        let bal = balanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let dx = 1_000 * 100_000_000u128;
        let q = pure_quote_swap(0, 1, dx, &bal, &pms, 500, &fc, 1_000_000_000_000, 5000).unwrap();
        let raw = calc_swap_output(0, 1, dx, &bal, &pms, 500, &fc, 5000).unwrap();
        assert_eq!(q.amount_out, raw.output_native);
        assert_eq!(q.fee_native, raw.fee_native);
        assert_eq!(q.fee_bps, raw.fee_bps_used);
        assert_eq!(q.imbalance_before, raw.imbalance_before);
        assert_eq!(q.imbalance_after, raw.imbalance_after);
        assert_eq!(q.is_rebalancing, raw.is_rebalancing);
        assert!(q.virtual_price_before > 0);
        assert!(q.virtual_price_after > 0);
    }

    #[test]
    fn quote_optimal_rebalance_balanced_pool_returns_zero() {
        // Both directions in a perfectly balanced pool should yield no
        // rebalancing trade.
        let bal = balanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let q = pure_quote_optimal_rebalance(0, 1, &bal, &pms, 500, &fc, 5000).unwrap();
        assert_eq!(q.dx, 0);
        assert_eq!(q.profit_bps_estimate, 0);
    }

    #[test]
    fn quote_optimal_rebalance_imbalanced_pool_returns_nonzero() {
        // Pool is icUSD-heavy: pushing ckUSDT in (1 -> 0) is rebalancing.
        let bal = imbalanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let q = pure_quote_optimal_rebalance(1, 0, &bal, &pms, 500, &fc, 5000).unwrap();
        assert!(q.dx > 0, "expected nonzero dx for rebalancing direction");
        assert!(q.amount_out > 0);
        assert!(q.profit_bps_estimate > 0);
        assert!(q.imbalance_after < q.imbalance_before);
        assert_eq!(q.fee_bps, fc.min_fee_bps);
    }

    #[test]
    fn quote_optimal_rebalance_wrong_direction_returns_zero() {
        // icUSD -> ckUSDT in an icUSD-heavy pool would be imbalancing.
        let bal = imbalanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let q = pure_quote_optimal_rebalance(0, 1, &bal, &pms, 500, &fc, 5000).unwrap();
        assert_eq!(q.dx, 0);
    }

    #[test]
    fn simulate_swap_path_chains_correctly() {
        let bal = balanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let path = vec![
            (0u8, 1u8, 1_000 * 100_000_000u128),
            (1u8, 2u8, 500 * 1_000_000u128),
        ];
        let results = pure_simulate_swap_path(&path, &bal, &pms, 500, &fc, 1_000_000_000_000, 5000).unwrap();
        assert_eq!(results.len(), 2);
        // Second hop's imbalance_before should equal first hop's imbalance_after
        // (within the chained-balances simulation).
        assert_eq!(results[1].imbalance_before, results[0].imbalance_after);
        assert!(results[0].amount_out > 0);
        assert!(results[1].amount_out > 0);
    }

    #[test]
    fn simulate_swap_path_propagates_errors() {
        let bal = balanced_pool();
        let pms = precision_muls();
        let fc = FeeCurveParams::default();
        let path = vec![(0u8, 0u8, 100u128)]; // same index
        assert!(pure_simulate_swap_path(&path, &bal, &pms, 500, &fc, 1, 5000).is_err());
    }

    // ─── Pagination tests ───
    //
    // get_imbalance_history and get_swap_events_v2 are thin wrappers around the
    // event vecs. The pagination math is the load-bearing piece, so we test it
    // directly with synthetic vecs.

    fn paginate_newest_first<T: Clone>(events: &[T], limit: u64, offset: u64) -> Vec<T> {
        let total = events.len() as u64;
        if offset >= total {
            return Vec::new();
        }
        let take = ((total - offset).min(limit)) as usize;
        let end = (total - offset) as usize;
        let start = end - take;
        let mut out: Vec<T> = events[start..end].to_vec();
        out.reverse();
        out
    }

    #[test]
    fn pagination_newest_first_full_page() {
        let v: Vec<u32> = (0..10).collect();
        let page = paginate_newest_first(&v, 5, 0);
        assert_eq!(page, vec![9, 8, 7, 6, 5]);
    }

    #[test]
    fn pagination_newest_first_with_offset() {
        let v: Vec<u32> = (0..10).collect();
        let page = paginate_newest_first(&v, 3, 5);
        // skip newest 5, take next 3
        assert_eq!(page, vec![4, 3, 2]);
    }

    #[test]
    fn pagination_newest_first_offset_past_end() {
        let v: Vec<u32> = (0..3).collect();
        let page = paginate_newest_first(&v, 10, 100);
        assert!(page.is_empty());
    }

    #[test]
    fn pagination_newest_first_partial_last_page() {
        let v: Vec<u32> = (0..5).collect();
        let page = paginate_newest_first(&v, 10, 3);
        // skip newest 3 (indices 4,3,2), then take up to 10 of remaining (1,0)
        assert_eq!(page, vec![1, 0]);
    }
}

#[cfg(test)]
mod explorer_tests {
    use super::*;

    fn mk_swap(id: u64, ts_ns: u64, ti: u8, to: u8, amount_in: u128, fee_bps: u16, is_reb: bool, imb_after: u64) -> SwapEventV2 {
        SwapEventV2 {
            id,
            timestamp: ts_ns,
            caller: Principal::anonymous(),
            token_in: ti,
            token_out: to,
            amount_in,
            amount_out: amount_in / 2,
            fee: amount_in / 1000,
            fee_bps,
            imbalance_before: imb_after.saturating_sub(10),
            imbalance_after: imb_after,
            is_rebalancing: is_reb,
            pool_balances_after: [100, 200, 300],
            virtual_price_after: 1_000_000_000_000_000_000u128 + id as u128,
            migrated: false,
        }
    }

    // ─── window_cutoff_ns ───

    #[test]
    fn window_cutoff_alltime_returns_zero() {
        assert_eq!(window_cutoff_ns(StatsWindow::AllTime, 1_000_000_000_000), 0);
    }

    #[test]
    fn window_cutoff_24h_subtracts_correctly() {
        let now = 100 * 24 * 3600 * 1_000_000_000u64;
        let cutoff = window_cutoff_ns(StatsWindow::Last24h, now);
        assert_eq!(cutoff, now - 24 * 3600 * 1_000_000_000);
    }

    #[test]
    fn window_cutoff_saturates_at_zero() {
        // now smaller than the window: cutoff saturates to 0
        assert_eq!(window_cutoff_ns(StatsWindow::Last30d, 5), 0);
    }

    // ─── bucket_floor ───

    #[test]
    fn bucket_floor_basic() {
        let bsec: u64 = 60;
        let bns = bsec * 1_000_000_000;
        // ts that lands inside bucket #5 (5 * 60s)
        let ts = 5 * bns + 17_000_000_000;
        assert_eq!(bucket_floor(ts, bsec), 5 * bns);
    }

    #[test]
    fn bucket_floor_zero_bucket_returns_zero() {
        assert_eq!(bucket_floor(123456789, 0), 0);
    }

    #[test]
    fn bucket_floor_exact_boundary() {
        let bsec: u64 = 3600;
        let bns = bsec * 1_000_000_000;
        assert_eq!(bucket_floor(7 * bns, bsec), 7 * bns);
    }

    // ─── Pool stats aggregation math ───

    #[test]
    fn pool_stats_aggregation_math() {
        // Three swaps in different directions; one is rebalancing.
        let mut events = Vec::new();
        events.push(mk_swap(0, 1_000, 0, 1, 1000, 20, false, 100));
        events.push(mk_swap(1, 2_000, 0, 2, 2000, 50, false, 200));
        events.push(mk_swap(2, 3_000, 1, 0, 500, 1, true, 80));

        // Replicate the body of get_pool_stats with cutoff=0.
        let mut volume_per_token = [0u128; 3];
        let mut fees = [0u128; 3];
        let mut count = 0u64;
        let mut arb_count = 0u64;
        let mut weighted_fee_bps: u128 = 0;
        let mut total_volume: u128 = 0;
        let mut swappers: std::collections::BTreeSet<Principal> = std::collections::BTreeSet::new();
        for e in &events {
            count += 1;
            volume_per_token[e.token_in as usize] += e.amount_in;
            fees[e.token_out as usize] += e.fee;
            weighted_fee_bps += (e.fee_bps as u128) * e.amount_in;
            total_volume += e.amount_in;
            swappers.insert(e.caller);
            if e.is_rebalancing { arb_count += 1; }
        }

        assert_eq!(count, 3);
        assert_eq!(arb_count, 1);
        // Volume in: token0 = 1000+2000 = 3000, token1 = 500
        assert_eq!(volume_per_token, [3000, 500, 0]);
        // Volume-weighted avg fee bps:
        //   (20*1000 + 50*2000 + 1*500) / 3500 = 120500 / 3500 = 34
        let avg = (weighted_fee_bps / total_volume) as u32;
        assert_eq!(avg, 34);
        assert_eq!(swappers.len(), 1); // all anonymous
    }

    #[test]
    fn fee_stats_buckets_classify_correctly() {
        // Hand-classify a couple of swaps and verify the bucket logic.
        let bucket_edges: [(u16, u16); 5] = [(1, 10), (10, 25), (25, 50), (50, 75), (75, 100)];
        let bucketize = |fee_bps: u16| -> Option<usize> {
            for (i, (lo, hi)) in bucket_edges.iter().enumerate() {
                if fee_bps >= *lo && fee_bps < *hi {
                    return Some(i);
                }
            }
            None
        };
        assert_eq!(bucketize(1), Some(0));
        assert_eq!(bucketize(9), Some(0));
        assert_eq!(bucketize(10), Some(1));
        assert_eq!(bucketize(24), Some(1));
        assert_eq!(bucketize(25), Some(2));
        assert_eq!(bucketize(50), Some(3));
        assert_eq!(bucketize(74), Some(3));
        assert_eq!(bucketize(75), Some(4));
        assert_eq!(bucketize(99), Some(4));
        assert_eq!(bucketize(0), None); // below min
        assert_eq!(bucketize(100), None); // above max
    }

    // ─── Time series bucketing correctness ───

    #[test]
    fn volume_series_buckets_by_minute() {
        let bsec: u64 = 60;
        let bns = bsec * 1_000_000_000;
        let events = vec![
            // Two swaps in bucket 5
            mk_swap(0, 5 * bns + 1, 0, 1, 1000, 5, false, 100),
            mk_swap(1, 5 * bns + 30_000_000_000, 0, 1, 2000, 5, false, 110),
            // One swap in bucket 7
            mk_swap(2, 7 * bns + 10, 1, 0, 500, 5, true, 90),
        ];
        let mut map: std::collections::BTreeMap<u64, [u128; 3]> = std::collections::BTreeMap::new();
        for e in &events {
            let b = bucket_floor(e.timestamp, bsec);
            let entry = map.entry(b).or_insert([0; 3]);
            entry[e.token_in as usize] += e.amount_in;
        }
        assert_eq!(map.len(), 2);
        assert_eq!(map[&(5 * bns)], [3000, 0, 0]);
        assert_eq!(map[&(7 * bns)], [0, 500, 0]);
    }

    #[test]
    fn fee_series_volume_weighted_average() {
        let bsec: u64 = 60;
        let bns = bsec * 1_000_000_000;
        let events = vec![
            mk_swap(0, bns + 1, 0, 1, 1000, 10, false, 100),
            mk_swap(1, bns + 2, 0, 1, 3000, 50, false, 110),
        ];
        // Expected avg fee_bps = (10*1000 + 50*3000) / 4000 = 160000/4000 = 40
        let mut sums: std::collections::BTreeMap<u64, (u128, u128)> = std::collections::BTreeMap::new();
        for e in &events {
            let b = bucket_floor(e.timestamp, bsec);
            let entry = sums.entry(b).or_insert((0, 0));
            entry.0 += (e.fee_bps as u128) * e.amount_in;
            entry.1 += e.amount_in;
        }
        let (w, v) = sums[&bns];
        assert_eq!((w / v) as u32, 40);
    }

    // ─── Pool health heuristic ───

    #[test]
    fn pool_health_score_zero_at_balanced() {
        let params = FeeCurveParams::default();
        let imb = 0u64;
        let score = ((imb.min(params.imb_saturation) as u128 * 100) / params.imb_saturation as u128) as u8;
        assert_eq!(score, 0);
    }

    #[test]
    fn pool_health_score_saturates_at_100() {
        let params = FeeCurveParams::default();
        let imb = params.imb_saturation * 5;
        let score = ((imb.min(params.imb_saturation) as u128 * 100) / params.imb_saturation as u128) as u8;
        assert_eq!(score, 100);
    }

    #[test]
    fn pool_health_score_linear_at_half_saturation() {
        let params = FeeCurveParams::default();
        let imb = params.imb_saturation / 2;
        let score = ((imb.min(params.imb_saturation) as u128 * 100) / params.imb_saturation as u128) as u8;
        assert_eq!(score, 50);
    }

    #[test]
    fn pool_health_fee_at_max_uses_compute_fee_bps() {
        let params = FeeCurveParams { min_fee_bps: 1, max_fee_bps: 99, imb_saturation: 250_000_000 };
        // Worst-case hypothetical: imb_after = saturation -> should equal max_fee_bps
        let fee = crate::math::compute_fee_bps(0, params.imb_saturation, &params);
        assert_eq!(fee, params.max_fee_bps);
    }

    // ─── Filter pagination correctness (skip/take by principal) ───

    #[test]
    fn by_principal_skip_take_pagination() {
        let alice = Principal::anonymous();
        let bob = Principal::management_canister();
        let mut events: Vec<SwapEventV2> = Vec::new();
        for i in 0..10u64 {
            let mut e = mk_swap(i, 1_000 + i, 0, 1, 100, 5, false, 50);
            e.caller = if i % 2 == 0 { alice } else { bob };
            events.push(e);
        }
        // Filter by alice => 5 events. Skip 1, take 2 => events with original ids 2, 4.
        let page: Vec<u64> = events
            .iter()
            .filter(|e| e.caller == alice)
            .skip(1)
            .take(2)
            .map(|e| e.id)
            .collect();
        assert_eq!(page, vec![2, 4]);
    }

    #[test]
    fn by_time_range_filter_inclusive_exclusive() {
        let events = vec![
            mk_swap(0, 100, 0, 1, 10, 5, false, 50),
            mk_swap(1, 200, 0, 1, 10, 5, false, 50),
            mk_swap(2, 300, 0, 1, 10, 5, false, 50),
            mk_swap(3, 400, 0, 1, 10, 5, false, 50),
        ];
        // [200, 400) => ids 1 and 2
        let ids: Vec<u64> = events
            .iter()
            .filter(|e| e.timestamp >= 200 && e.timestamp < 400)
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }
}
