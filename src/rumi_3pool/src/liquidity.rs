// Liquidity operations for the Rumi 3pool.

use ethnum::U256;

use crate::math::*;
use crate::types::*;

/// Outcome of an `add_liquidity` computation (dynamic-fee schema).
///
/// `fees_native` are per-token fees retained in the pool (in native decimals),
/// computed using the dynamic fee rate `fee_bps_used`. Imbalance values are
/// in 1e9 fixed-point (see `IMB_SCALE`).
#[derive(Clone, Debug)]
pub struct AddLiquidityOutcome {
    pub lp_minted: u128,
    pub fees_native: [u128; 3],
    pub fee_bps_used: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}

/// Outcome of a `remove_liquidity_one_coin` computation (dynamic-fee schema).
#[derive(Clone, Debug)]
pub struct RemoveOneCoinOutcome {
    pub amount_native: u128,
    pub fee_native: u128,
    pub fee_bps_used: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}

/// Calculate LP tokens to mint for a deposit using the dynamic fee curve.
///
/// The imbalance fee applies per-token to the non-proportional portion of the
/// deposit (the diff between the actual new balance and the proportionally-
/// scaled ideal balance). Balanced deposits have diff ~ 0 per token, so they
/// pay effectively zero fee regardless of `fee_bps_used`.
pub fn calc_add_liquidity(
    amounts: &[u128; 3],
    old_balances: &[u128; 3],
    precision_muls: &[u64; 3],
    lp_total_supply: u128,
    amp: u64,
    fee_curve: &FeeCurveParams,
) -> Result<AddLiquidityOutcome, ThreePoolError> {
    // At least one amount must be > 0
    if amounts.iter().all(|&a| a == 0) {
        return Err(ThreePoolError::ZeroAmount);
    }

    // Compute new balances
    let new_balances: [u128; 3] = [
        old_balances[0] + amounts[0],
        old_balances[1] + amounts[1],
        old_balances[2] + amounts[2],
    ];

    // Normalize
    let old_xp = normalize_all(old_balances, precision_muls);
    let new_xp = normalize_all(&new_balances, precision_muls);

    // D0: invariant before deposit (0 if first deposit)
    let d0 = if lp_total_supply > 0 {
        get_d(&old_xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?
    } else {
        U256::ZERO
    };

    // D1: invariant after deposit
    let d1 = get_d(&new_xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?;

    if d1 <= d0 {
        return Err(ThreePoolError::ZeroAmount);
    }

    // First deposit: mint D1 / 10^10 LP tokens (D is 18-decimal, LP is 8-decimal), no fees.
    // A first deposit sets the reference balance; there is no "before" imbalance.
    if lp_total_supply == 0 {
        let lp_8dec = (d1 / ethnum::U256::from(10_000_000_000u128)).as_u128();
        let imbalance_after = compute_imbalance(&new_balances, precision_muls);
        return Ok(AddLiquidityOutcome {
            lp_minted: lp_8dec,
            fees_native: [0u128; 3],
            fee_bps_used: 0,
            imbalance_before: 0,
            imbalance_after,
            is_rebalancing: false,
        });
    }

    // Dynamic fee rate based on pool-level imbalance before/after the deposit.
    // Note: imbalance is computed on pre-fee balances. Because the per-token
    // fee is applied only to the non-proportional `diff`, a perfectly balanced
    // deposit has diff ~ 0 and pays effectively zero fee, regardless of rate.
    let imbalance_before = compute_imbalance(old_balances, precision_muls);
    let imbalance_after = compute_imbalance(&new_balances, precision_muls);
    let fee_bps_used = compute_fee_bps(imbalance_before, imbalance_after, fee_curve);
    let is_rebalancing = imbalance_after < imbalance_before;

    let mut fees_normalized = [U256::ZERO; 3];
    let mut adjusted_xp = new_xp;

    for k in 0..3 {
        // ideal_balance = old_xp[k] * D1 / D0
        let ideal = old_xp[k] * d1 / d0;

        // diff = |new_xp[k] - ideal|
        let diff = if new_xp[k] > ideal {
            new_xp[k] - ideal
        } else {
            ideal - new_xp[k]
        };

        // fee_k = diff * fee_bps_used / 10000
        fees_normalized[k] = diff * U256::from(fee_bps_used as u64) / U256::from(10_000u64);
        adjusted_xp[k] = new_xp[k] - fees_normalized[k];
    }

    // D2: invariant after fees
    let d2 = get_d(&adjusted_xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?;

    // LP minted = lp_supply * (D2 - D0) / D0
    let lp_minted = U256::from(lp_total_supply) * (d2 - d0) / d0;

    // Denormalize fees to native decimals
    let fees_native: [u128; 3] = [
        denormalize_balance(fees_normalized[0], precision_muls[0]),
        denormalize_balance(fees_normalized[1], precision_muls[1]),
        denormalize_balance(fees_normalized[2], precision_muls[2]),
    ];

    Ok(AddLiquidityOutcome {
        lp_minted: lp_minted.as_u128(),
        fees_native,
        fee_bps_used,
        imbalance_before,
        imbalance_after,
        is_rebalancing,
    })
}

/// Proportional withdrawal: no fees, returns array of amounts in native decimals.
pub fn calc_remove_liquidity(
    lp_burn: u128,
    balances: &[u128; 3],
    lp_total_supply: u128,
) -> [u128; 3] {
    [
        balances[0] * lp_burn / lp_total_supply,
        balances[1] * lp_burn / lp_total_supply,
        balances[2] * lp_burn / lp_total_supply,
    ]
}

/// Single-token withdrawal: user burns LP and receives one token.
/// Returns (amount_native, fee_native).
pub fn calc_remove_one_coin(
    lp_burn: u128,
    coin_index: usize,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    lp_total_supply: u128,
    amp: u64,
    fee_curve: &FeeCurveParams,
    admin_fee_bps: u64,
) -> Result<RemoveOneCoinOutcome, ThreePoolError> {
    if coin_index >= 3 {
        return Err(ThreePoolError::InvalidCoinIndex);
    }
    if lp_burn == 0 {
        return Err(ThreePoolError::ZeroAmount);
    }
    if lp_total_supply == 0 {
        return Err(ThreePoolError::PoolEmpty);
    }

    let xp = normalize_all(balances, precision_muls);

    // D0: current invariant
    let d0 = get_d(&xp, amp).ok_or(ThreePoolError::InvariantNotConverged)?;

    // D1: new invariant after burning LP
    // D1 = D0 - lp_burn * D0 / lp_supply
    let d1 = d0 - U256::from(lp_burn) * d0 / U256::from(lp_total_supply);

    // new_y without fees: what token j balance would be at D1
    let new_y = get_y_d(coin_index, &xp, amp, d1).ok_or(ThreePoolError::InvariantNotConverged)?;

    // Dynamic fee rate: measure imbalance before and after the withdrawal.
    // Two-pass refinement: the LP-fee portion stays in the pool, so the actual
    // post-trade balance is `balance - dy_no_fee + lp_fee`. Pass 1 uses the
    // gross dy_no_fee to seed fee_bps; pass 2 refines using the LP-fee retention.
    let imbalance_before = compute_imbalance(balances, precision_muls);

    let dy_no_fee_native = denormalize_balance(
        xp[coin_index] - new_y,
        precision_muls[coin_index],
    );
    let mut balances_after_gross = *balances;
    balances_after_gross[coin_index] =
        balances_after_gross[coin_index].saturating_sub(dy_no_fee_native);
    let imbalance_after_gross = compute_imbalance(&balances_after_gross, precision_muls);
    let fee_bps_first = compute_fee_bps(imbalance_before, imbalance_after_gross, fee_curve);

    // LP-fee portion (in native units of `coin_index`) that stays in the pool.
    // dy_no_fee * fee_bps_first / 10_000 * (10_000 - admin_fee_bps) / 10_000.
    let lp_fee_first_native = dy_no_fee_native
        * (fee_bps_first as u128)
        * (10_000u128 - admin_fee_bps as u128)
        / 10_000
        / 10_000;
    let dy_minus_lp_fee_native = dy_no_fee_native.saturating_sub(lp_fee_first_native);
    let mut balances_after = *balances;
    balances_after[coin_index] =
        balances_after[coin_index].saturating_sub(dy_minus_lp_fee_native);
    let imbalance_after = compute_imbalance(&balances_after, precision_muls);
    let fee_bps_used = compute_fee_bps(imbalance_before, imbalance_after, fee_curve);
    let is_rebalancing = imbalance_after < imbalance_before;

    // Compute reduced xp with per-token fees applied
    let mut xp_reduced = xp;
    for k in 0..3 {
        // ideal = xp[k] * D1 / D0
        let ideal = xp[k] * d1 / d0;

        // diff = |xp[k] - ideal|
        let diff = if xp[k] > ideal {
            xp[k] - ideal
        } else {
            ideal - xp[k]
        };

        // Subtract fee from the balance
        xp_reduced[k] = xp[k] - diff * U256::from(fee_bps_used as u64) / U256::from(10_000u64);
    }

    // new_y_reduced: what token j balance would be after fee adjustment
    let new_y_reduced = get_y_d(coin_index, &xp_reduced, amp, d1)
        .ok_or(ThreePoolError::InvariantNotConverged)?;

    // dy (amount user receives, after fees) = xp_reduced[coin_index] - new_y_reduced
    let dy = xp_reduced[coin_index] - new_y_reduced;

    // fee = (xp[coin_index] - new_y) - dy
    // This is the difference between the no-fee withdrawal and the after-fee withdrawal
    let dy_no_fee = xp[coin_index] - new_y;
    let fee_normalized = dy_no_fee - dy;

    Ok(RemoveOneCoinOutcome {
        amount_native: denormalize_balance(dy, precision_muls[coin_index]),
        fee_native: denormalize_balance(fee_normalized, precision_muls[coin_index]),
        fee_bps_used,
        imbalance_before,
        imbalance_after,
        is_rebalancing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard test setup: 1M of each token
    fn test_balances() -> [u128; 3] {
        [
            1_000_000 * 100_000_000,   // 1M icUSD (8 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDT (6 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDC (6 dec)
        ]
    }

    fn test_precision_muls() -> [u64; 3] {
        [
            10_000_000_000,     // 10^10 (8 -> 18)
            1_000_000_000_000,  // 10^12 (6 -> 18)
            1_000_000_000_000,  // 10^12 (6 -> 18)
        ]
    }

    fn default_curve() -> FeeCurveParams {
        FeeCurveParams::default()
    }

    #[test]
    fn test_calc_add_liquidity_first_deposit() {
        let old_balances = [0u128; 3];
        let amounts: [u128; 3] = [
            1_000_000 * 100_000_000,   // 1M icUSD
            1_000_000 * 1_000_000,     // 1M ckUSDT
            1_000_000 * 1_000_000,     // 1M ckUSDC
        ];
        let precision_muls = test_precision_muls();
        let lp_supply = 0u128;
        let amp = 100u64;
        let curve = default_curve();

        let out = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, lp_supply, amp, &curve,
        ).expect("first deposit should succeed");
        let lp_minted = out.lp_minted;
        let fees = out.fees_native;

        // D for equal 1M * 3 = ~3M * 10^18, LP = D / 10^10 = ~3M * 10^8
        let three_million_8 = 3_000_000u128 * 100_000_000u128;
        let diff = if lp_minted > three_million_8 {
            lp_minted - three_million_8
        } else {
            three_million_8 - lp_minted
        };
        // Should be very close to 3M * 10^8
        assert!(
            diff < 1_000, // within 1000 units of 10^8
            "first deposit LP should be ~3M*10^8, got {}, diff {}",
            lp_minted, diff
        );

        // No fees on first deposit
        assert_eq!(fees, [0, 0, 0], "first deposit should have no fees");
    }

    #[test]
    fn test_calc_add_liquidity_proportional() {
        let old_balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 100u64;
        let curve = default_curve();

        // First deposit to get LP supply
        let initial_lp = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        // Proportional deposit: 10% more of each token
        let amounts: [u128; 3] = [
            100_000 * 100_000_000,  // 100k icUSD
            100_000 * 1_000_000,    // 100k ckUSDT
            100_000 * 1_000_000,    // 100k ckUSDC
        ];

        let out = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, initial_lp, amp, &curve,
        ).expect("proportional deposit should succeed");
        let lp_minted = out.lp_minted;
        let fees = out.fees_native;

        // Should get ~10% more LP tokens
        let expected_lp = initial_lp / 10;
        let diff = if lp_minted > expected_lp {
            lp_minted - expected_lp
        } else {
            expected_lp - lp_minted
        };
        // Within 0.01% of expected
        assert!(
            diff < expected_lp / 10_000,
            "proportional deposit LP diff too large: got {}, expected ~{}, diff {}",
            lp_minted, expected_lp, diff
        );

        // Fees should be minimal (near zero) for proportional deposit
        // They can be nonzero due to rounding but should be very small
        for (k, fee) in fees.iter().enumerate() {
            assert!(
                *fee < 100, // less than 100 units in native decimals
                "proportional deposit fee[{}] should be minimal, got {}",
                k, fee
            );
        }
    }

    #[test]
    fn test_calc_add_liquidity_imbalanced() {
        let old_balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 100u64;
        let curve = default_curve();

        // First deposit to get LP supply
        let initial_lp = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        // Imbalanced deposit: only icUSD
        let amounts: [u128; 3] = [
            100_000 * 100_000_000,  // 100k icUSD
            0,
            0,
        ];

        let out = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, initial_lp, amp, &curve,
        ).expect("imbalanced deposit should succeed");
        let lp_minted = out.lp_minted;
        let fees = out.fees_native;
        assert!(!out.is_rebalancing, "adding only icUSD to balanced pool is imbalancing");
        assert!(out.fee_bps_used >= curve.min_fee_bps);

        // Should get some LP tokens
        assert!(lp_minted > 0, "should mint some LP");

        // Fees should be nonzero for imbalanced deposit
        assert!(fees[0] > 0, "imbalanced deposit should have fee on token 0, got {}", fees[0]);
        // Tokens 1 and 2 also get fees because of the imbalance
        assert!(fees[1] > 0, "imbalanced deposit should have fee on token 1, got {}", fees[1]);
        assert!(fees[2] > 0, "imbalanced deposit should have fee on token 2, got {}", fees[2]);
    }

    #[test]
    fn test_calc_add_liquidity_zero_amounts() {
        let curve = default_curve();
        let result = calc_add_liquidity(
            &[0, 0, 0], &test_balances(), &test_precision_muls(), 1000, 100, &curve,
        );
        assert!(matches!(result, Err(ThreePoolError::ZeroAmount)));
    }

    // ─── Task 11 tests: remove_liquidity ───

    #[test]
    fn test_calc_remove_liquidity_proportional() {
        let balances = test_balances();
        let lp_supply = 3_000_000u128 * 100_000_000u128; // ~3M * 10^8 (8-decimal LP)

        // Burn 10% of LP
        let lp_burn = lp_supply / 10;

        let amounts = calc_remove_liquidity(lp_burn, &balances, lp_supply);

        // Should get 10% of each balance
        for k in 0..3 {
            let expected = balances[k] / 10;
            assert_eq!(
                amounts[k], expected,
                "proportional withdrawal: amounts[{}] = {}, expected {}",
                k, amounts[k], expected
            );
        }
    }

    #[test]
    fn test_calc_remove_one_coin() {
        let balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 100u64;
        let curve = default_curve();

        // Get initial LP supply from a first deposit
        let lp_supply = calc_add_liquidity(
            &balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        // Burn 10% of LP to get ckUSDT (token 1)
        let lp_burn = lp_supply / 10;

        let out = calc_remove_one_coin(
            lp_burn, 1, &balances, &precision_muls, lp_supply, amp, &curve, 5000,
        ).expect("remove_one_coin should succeed");
        let amount = out.amount_native;
        let fee = out.fee_native;

        // Amount should be meaningful (close to 10% of 3M worth, withdrawn in ckUSDT)
        // Due to single-token withdrawal penalty, it'll be somewhat less than
        // the proportional value
        assert!(amount > 0, "should receive some tokens");

        // Should be roughly 300k ckUSDT (10% of 3M pool value, all in one token)
        // but less due to slippage
        let expected_approx = 300_000 * 1_000_000u128; // 300k ckUSDT in native
        assert!(
            amount < expected_approx,
            "single-coin withdrawal should be less than ideal amount: {} < {}",
            amount, expected_approx
        );
        assert!(
            amount > expected_approx * 95 / 100,
            "single-coin withdrawal should be close to ideal: {} > 95% of {}",
            amount, expected_approx
        );

        // Fee should be nonzero
        assert!(fee > 0, "single-token removal should have a fee, got {}", fee);
        assert!(out.fee_bps_used >= curve.min_fee_bps);
    }

    // ─── Dynamic fee unit tests (Task 9) ───

    /// Build an imbalanced pool: icUSD heavy, ckUSDT/ckUSDC light.
    fn imbalanced_balances() -> [u128; 3] {
        [
            2_000_000 * 100_000_000, // 2M icUSD
            500_000 * 1_000_000,     // 0.5M ckUSDT
            500_000 * 1_000_000,     // 0.5M ckUSDC
        ]
    }

    #[test]
    fn test_add_liquidity_rebalancing_pays_min_fee() {
        // Add the underrepresented tokens to a skewed pool -> rebalancing.
        let precision_muls = test_precision_muls();
        let amp = 500u64;
        let curve = default_curve();

        // Seed supply with a first deposit equal to the skewed state.
        let initial = imbalanced_balances();
        let lp_supply = calc_add_liquidity(
            &initial, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        // Now add only ckUSDT + ckUSDC -> rebalancing.
        let amounts = [0u128, 200_000 * 1_000_000, 200_000 * 1_000_000];
        let out = calc_add_liquidity(
            &amounts, &initial, &precision_muls, lp_supply, amp, &curve,
        ).unwrap();

        assert!(out.is_rebalancing, "deposit of under-weighted tokens is rebalancing");
        assert_eq!(
            out.fee_bps_used, curve.min_fee_bps,
            "rebalancing deposit should pay exactly min_fee_bps"
        );
    }

    #[test]
    fn test_add_liquidity_balanced_pays_zero_fee() {
        // Balanced deposit on a balanced pool -> per-token diff ~ 0 -> fees ~ 0.
        let old_balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 500u64;
        let curve = default_curve();

        let lp_supply = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        let amounts: [u128; 3] = [
            100_000 * 100_000_000,
            100_000 * 1_000_000,
            100_000 * 1_000_000,
        ];
        let out = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, lp_supply, amp, &curve,
        ).unwrap();

        for (k, f) in out.fees_native.iter().enumerate() {
            assert!(
                *f < 100,
                "balanced deposit per-token fee[{}] should be ~0, got {}",
                k, f
            );
        }
    }

    #[test]
    fn test_add_liquidity_imbalancing_scales_fee() {
        // Imbalancing deposit on a balanced pool -> fee_bps > min.
        let old_balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 500u64;
        let curve = default_curve();

        let lp_supply = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        // Huge icUSD-only deposit: large imbalance shift.
        let amounts = [1_000_000 * 100_000_000u128, 0, 0];
        let out = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, lp_supply, amp, &curve,
        ).unwrap();

        assert!(!out.is_rebalancing);
        assert!(
            out.fee_bps_used > curve.min_fee_bps,
            "imbalancing deposit should pay more than min_fee_bps, got {}",
            out.fee_bps_used
        );
        assert!(out.fee_bps_used <= curve.max_fee_bps);
        assert!(out.fees_native[0] > 0);
    }

    #[test]
    fn test_remove_liquidity_one_coin_dynamic_fee() {
        // From a balanced pool, a single-coin withdrawal is imbalancing -> dynamic fee > min.
        let balances = test_balances();
        let precision_muls = test_precision_muls();
        let amp = 500u64;
        let curve = default_curve();

        let lp_supply = calc_add_liquidity(
            &balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        let lp_burn = lp_supply / 20; // 5%
        let out = calc_remove_one_coin(
            lp_burn, 1, &balances, &precision_muls, lp_supply, amp, &curve, 5000,
        ).unwrap();

        assert!(!out.is_rebalancing);
        assert!(out.fee_bps_used >= curve.min_fee_bps);
        assert!(out.fee_bps_used <= curve.max_fee_bps);
        assert!(out.fee_native > 0);
    }

    #[test]
    fn test_remove_liquidity_one_coin_rebalancing_min_fee() {
        // From an icUSD-heavy pool, withdrawing icUSD single-sided reduces imbalance.
        let balances = imbalanced_balances();
        let precision_muls = test_precision_muls();
        let amp = 500u64;
        let curve = default_curve();

        let lp_supply = calc_add_liquidity(
            &balances, &[0u128; 3], &precision_muls, 0, amp, &curve,
        ).unwrap().lp_minted;

        let lp_burn = lp_supply / 50; // 2%
        let out = calc_remove_one_coin(
            lp_burn, 0, &balances, &precision_muls, lp_supply, amp, &curve, 5000,
        ).unwrap();

        assert!(
            out.is_rebalancing,
            "withdrawing the over-weighted token should be rebalancing"
        );
        assert_eq!(out.fee_bps_used, curve.min_fee_bps);
    }

    #[test]
    fn test_remove_liquidity_proportional_has_no_fee_path() {
        // Sanity: `calc_remove_liquidity` does not even touch the fee curve and
        // returns exact pro-rata amounts. This guards against future regressions
        // where a fee path gets added to proportional removal.
        let balances = test_balances();
        let lp_supply = 3_000_000u128 * 100_000_000u128;
        let lp_burn = lp_supply / 7;

        let amounts = calc_remove_liquidity(lp_burn, &balances, lp_supply);
        for k in 0..3 {
            assert_eq!(amounts[k], balances[k] * lp_burn / lp_supply);
        }
    }
}
