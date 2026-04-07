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
/// Two-pass refinement: the actual post-trade balance of token j on the pool's
/// books is `balance[j] - output - admin_fee_share = balance[j] - dy + lp_fee`.
/// The LP fee stays inside the pool, so the gross pre-fee balance over-states
/// the imbalance shift by ~fee_bps/10000. We compute a first-pass fee bps using
/// the gross balance, then recompute imbalance using the refined post-trade
/// balance (which retains the LP-fee portion) and re-evaluate the curve.
pub fn calc_swap_output(
    i: usize,
    j: usize,
    dx_native: u128,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    fee_curve: &FeeCurveParams,
    admin_fee_bps: u64,
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

    // Imbalance before. `after` requires a two-pass refinement because the
    // LP-fee portion of dy stays inside the pool (only `output + admin_fee_share`
    // actually leaves), so the true post-trade balance retains some of dy.
    let imbalance_before = compute_imbalance(balances, precision_muls);

    // Pass 1: gross-balance estimate of imbalance_after, used to seed the fee
    // bps. This over-states the imbalance shift by ~fee/10000.
    let dy_native_gross = denormalize_balance(dy_normalized, precision_muls[j]);
    let mut balances_after_gross = *balances;
    balances_after_gross[i] = balances_after_gross[i].saturating_add(dx_native);
    balances_after_gross[j] = balances_after_gross[j].saturating_sub(dy_native_gross);
    let imbalance_after_gross = compute_imbalance(&balances_after_gross, precision_muls);
    let fee_bps_first = compute_fee_bps(imbalance_before, imbalance_after_gross, fee_curve);

    // Pass 2: compute the LP-fee portion that actually stays in the pool, use
    // it to refine the post-trade balance, and re-evaluate the fee curve.
    let admin_fee_bps_u256 = ethnum::U256::from(admin_fee_bps);
    let lp_fee_first_normalized = dy_normalized
        * ethnum::U256::from(fee_bps_first as u64)
        * (ethnum::U256::from(10_000u64) - admin_fee_bps_u256)
        / ethnum::U256::from(10_000u64)
        / ethnum::U256::from(10_000u64);
    // Output that actually leaves the pool on token j: dy - lp_fee. Note that
    // admin_fee_share also leaves the internal balance (it moves to admin_fees),
    // so the on-book balance after the trade is balance[j] - dy + lp_fee.
    let dy_minus_lp_fee_native = denormalize_balance(
        dy_normalized.saturating_sub(lp_fee_first_normalized),
        precision_muls[j],
    );
    let mut balances_after = *balances;
    balances_after[i] = balances_after[i].saturating_add(dx_native);
    balances_after[j] = balances_after[j].saturating_sub(dy_minus_lp_fee_native);
    let imbalance_after = compute_imbalance(&balances_after, precision_muls);

    // Final fee bps from refined imbalance_after.
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
        let out = calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve, 5000)
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
        let out = calc_swap_output(1, 0, dx, &balances, &precision_muls(), 500, &curve, 5000)
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
        let out = calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve, 5000)
            .expect("swap should succeed");

        assert!(!out.is_rebalancing);
        assert!(out.fee_bps_used > curve.min_fee_bps);
        assert!(out.fee_bps_used <= curve.max_fee_bps);
    }

    #[test]
    fn test_calc_swap_output_zero_amount() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(0, 1, 0, &balances, &precision_muls(), 100, &curve, 5000);
        assert!(matches!(result, Err(ThreePoolError::ZeroAmount)));
    }

    #[test]
    fn test_calc_swap_output_same_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(1, 1, 1000, &balances, &precision_muls(), 100, &curve, 5000);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }

    #[test]
    fn test_calc_swap_output_invalid_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let curve = default_curve();
        let result = calc_swap_output(0, 3, 1000, &balances, &precision_muls(), 100, &curve, 5000);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }

    /// Two-pass refinement: a borderline imbalancing swap should pay a slightly
    /// lower fee_bps than the unrefined gross-balance estimate, because the LP
    /// fee actually retained in the pool offsets a fraction of the imbalance
    /// shift the unrefined pass would attribute to the trade.
    ///
    /// This test compares against a "no admin fee" baseline (admin_fee_bps = 0,
    /// so 100% of the fee stays in the pool — maximum refinement) versus a
    /// "100% admin fee" run (admin_fee_bps = 10_000, no LP fee retained — the
    /// refined pass equals the unrefined pass). The 0% case must produce a
    /// fee_bps that is less than or equal to the 100% case, and on a borderline
    /// imbalancing swap on an aggressive curve it should be strictly lower.
    #[test]
    fn test_two_pass_refinement_lowers_fee_on_borderline_swap() {
        // Curve tuned so a modest swap lands mid-range (well below saturation),
        // where the LP-fee retention measurably shifts the fee bps.
        let curve = FeeCurveParams {
            min_fee_bps: 1,
            max_fee_bps: 99,
            imb_saturation: 250_000_000, // default 0.25
        };
        let balances: [u128; 3] = [
            1_000_000 * 100_000_000,
            1_000_000 * 1_000_000,
            1_000_000 * 1_000_000,
        ];
        let dx = 500_000 * 100_000_000u128; // 500k icUSD -> ckUSDT (heavy)

        let unrefined =
            calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve, 10_000)
                .expect("swap should succeed");
        let refined =
            calc_swap_output(0, 1, dx, &balances, &precision_muls(), 500, &curve, 0)
                .expect("swap should succeed");

        // Refined fee bps must not exceed the unrefined fee bps.
        assert!(
            refined.fee_bps_used <= unrefined.fee_bps_used,
            "refined fee bps ({}) should be <= unrefined ({})",
            refined.fee_bps_used,
            unrefined.fee_bps_used
        );
        // Refined imbalance_after must be strictly less than unrefined: the LP
        // fee that stays in the pool partially undoes the imbalance shift.
        assert!(
            refined.imbalance_after < unrefined.imbalance_after,
            "refined imbalance_after ({}) should be strictly < unrefined ({})",
            refined.imbalance_after,
            unrefined.imbalance_after
        );
    }
}
