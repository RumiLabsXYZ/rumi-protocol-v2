// Swap logic for the Rumi 3pool.

use crate::math::*;
use crate::types::*;

/// Result of a swap calculation (pre token transfer).
///
/// All token amounts are in token `j`'s native decimals. Imbalance values are in
/// 1e9 fixed-point (see `IMB_SCALE`). `fee_bps_used` is the dynamic fee rate
/// that was actually applied to this trade.
#[derive(Clone, Debug)]
pub struct SwapOutcome {
    pub output_native: u128,
    pub fee_native: u128,
    pub fee_bps_used: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}

/// Calculate the output amount for a swap using the directional dynamic fee curve.
///
/// Steps:
/// 1. Compute `imb_before` from current balances.
/// 2. Solve the curve for the raw output `dy` (pre-fee).
/// 3. Compute `imb_after` from the simulated post-trade balances (pre-fee — the
///    fee is kept in the pool so the balances that determine imbalance are the
///    same whether or not we deduct the fee from dy).
/// 4. Compute the dynamic fee bps and apply to dy.
pub fn calc_swap_output(
    i: usize,
    j: usize,
    dx_native: u128,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_curve: &FeeCurveParams,
) -> Result<SwapOutcome, ThreePoolError> {
    // Validate inputs
    if dx_native == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    if i == j {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    if i >= 3 || j >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }

    // Normalize balances to 18-decimal precision
    let xp = normalize_all(balances, precision_muls);

    // Compute D (the invariant)
    let d = get_d(&xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?;

    // New balance of token i after deposit
    let new_x_i = xp[i] + normalize_balance(dx_native, precision_muls[i]);

    // Solve for new balance of token j
    let new_y_j = get_y(i, j, new_x_i, &xp, amp, d).ok_or(ThreePoolError::InvariantNotConverged)?;

    // dy = old_y - new_y (how much token j the user receives, before fee)
    let dy_normalized = xp[j]
        .checked_sub(new_y_j)
        .ok_or(ThreePoolError::InsufficientLiquidity)?;

    // Imbalance before/after. `after` uses the simulated post-trade native
    // balances (the fee stays in the pool so it does not affect the balance
    // used for imbalance measurement — but denormalizing dy_normalized captures
    // the post-trade balance the user's portion is drawn from).
    let imbalance_before = compute_imbalance(balances, precision_muls);

    let dy_native_pre_fee = denormalize_balance(dy_normalized, precision_muls[j]);
    let mut balances_after = *balances;
    balances_after[i] = balances_after[i].saturating_add(dx_native);
    balances_after[j] = balances_after[j].saturating_sub(dy_native_pre_fee);
    let imbalance_after = compute_imbalance(&balances_after, precision_muls);

    // Compute dynamic fee bps and apply
    let fee_bps_used = compute_fee_bps(imbalance_before, imbalance_after, fee_curve);
    let fee_normalized = dy_normalized * ethnum::U256::from(fee_bps_used as u64)
        / ethnum::U256::from(10_000u64);
    let dy_after_fee = dy_normalized - fee_normalized;

    // Denormalize back to native decimals
    let output_native = denormalize_balance(dy_after_fee, precision_muls[j]);
    let fee_native = denormalize_balance(fee_normalized, precision_muls[j]);

    let is_rebalancing = imbalance_after < imbalance_before;

    Ok(SwapOutcome {
        output_native,
        fee_native,
        fee_bps_used,
        imbalance_before,
        imbalance_after,
        is_rebalancing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn precision_muls() -> [u64; 3] {
        [
            10_000_000_000,     // icUSD 8 dec
            1_000_000_000_000,  // ckUSDT 6 dec
            1_000_000_000_000,  // ckUSDC 6 dec
        ]
    }

    fn default_curve() -> FeeCurveParams {
        FeeCurveParams::default()
    }

    #[test]
    fn test_calc_swap_output_balanced_pool_imbalances() {
        // Perfectly balanced 1M/1M/1M pool. A swap of 1000 icUSD -> ckUSDT
        // nudges it away from balance, so it pays more than min_fee_bps.
        let balances: [u128; 3] = [
            1_000_000 * 100_000_000,
            1_000_000 * 1_000_000,
            1_000_000 * 1_000_000,
        ];
        let dx = 1_000 * 100_000_000u128;
        let curve = default_curve();
        let out = calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve)
            .expect("swap should succeed");

        let expected_ckusdt = 1_000 * 1_000_000u128;
        assert!(out.output_native > expected_ckusdt * 99 / 100);
        assert!(out.output_native < expected_ckusdt);
        assert!(out.fee_native > 0);
        assert!(!out.is_rebalancing);
        assert!(out.imbalance_after > out.imbalance_before);
        assert!(out.fee_bps_used >= curve.min_fee_bps);
        assert!(out.fee_bps_used <= curve.max_fee_bps);
    }

    #[test]
    fn test_calc_swap_output_rebalancing_trade_gets_min_fee() {
        // Imbalanced pool: icUSD over, ckUSDT under. A swap ckUSDT -> icUSD
        // pushes the pool back toward balance. Should pay min_fee_bps.
        let balances: [u128; 3] = [
            2_000_000 * 100_000_000, // 2M icUSD
            500_000 * 1_000_000,     // 500k ckUSDT
            500_000 * 1_000_000,     // 500k ckUSDC
        ];
        let dx = 10_000 * 1_000_000u128; // 10k ckUSDT in
        let curve = default_curve();
        let out = calc_swap_output(1, 0, dx, &balances, &precision_muls(), 500, &curve)
            .expect("swap should succeed");

        assert!(out.is_rebalancing, "rebalancing trade expected");
        assert_eq!(out.fee_bps_used, curve.min_fee_bps);
        assert!(out.imbalance_after < out.imbalance_before);
    }

    #[test]
    fn test_calc_swap_output_imbalancing_scales_fee() {
        // Already imbalanced; doing more of the imbalancing direction scales fee up.
        let balances: [u128; 3] = [
            2_000_000 * 100_000_000,
            500_000 * 1_000_000,
            500_000 * 1_000_000,
        ];
        let dx = 50_000 * 100_000_000u128; // 50k icUSD in (worsens imbalance)
        let curve = default_curve();
        let out = calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve)
            .expect("swap should succeed");

        assert!(!out.is_rebalancing);
        assert!(out.fee_bps_used > curve.min_fee_bps);
        assert!(out.fee_bps_used <= curve.max_fee_bps);
    }

    #[test]
    fn test_calc_swap_output_zero_amount() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(0, 1, 0, &balances, &precision_muls(), 100, &curve);
        assert!(matches!(result, Err(ThreePoolError::ZeroAmount)));
    }

    #[test]
    fn test_calc_swap_output_same_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(1, 1, 1000, &balances, &precision_muls(), 100, &curve);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }

    #[test]
    fn test_calc_swap_output_invalid_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(0, 3, 1000, &balances, &precision_muls(), 100, &curve);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }
}
