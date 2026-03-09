// Liquidity operations for the Rumi 3pool.

use ethnum::U256;

use crate::math::*;
use crate::types::*;

/// Calculate LP tokens to mint for a deposit.
/// Returns (lp_to_mint_u128, fees_native[3]).
pub fn calc_add_liquidity(
    amounts: &[u128; 3],
    old_balances: &[u128; 3],
    precision_muls: &[u64; 3],
    lp_total_supply: u128,
    amp: u64,
    fee_bps: u64,
) -> Result<(u128, [u128; 3]), ThreePoolError> {
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

    // First deposit: mint D1 LP tokens, no fees
    if lp_total_supply == 0 {
        return Ok((d1.as_u128(), [0u128; 3]));
    }

    // Imbalance fee: fee_bps * N_COINS / (4 * (N_COINS - 1))
    // For N=3: fee_bps * 3 / (4 * 2) = fee_bps * 3 / 8
    let imbalance_fee_bps = fee_bps * 3 / (4 * 2);

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

        // fee_k = diff * imbalance_fee_bps / 10000
        fees_normalized[k] = diff * U256::from(imbalance_fee_bps) / U256::from(10_000u64);
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

    Ok((lp_minted.as_u128(), fees_native))
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
    fee_bps: u64,
) -> Result<(u128, u128), ThreePoolError> {
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

    // Imbalance fee: fee_bps * N / (4 * (N-1))
    let imbalance_fee_bps = fee_bps * 3 / (4 * 2);

    // Compute reduced xp with fees applied
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
        xp_reduced[k] = xp[k] - diff * U256::from(imbalance_fee_bps) / U256::from(10_000u64);
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

    Ok((
        denormalize_balance(dy, precision_muls[coin_index]),
        denormalize_balance(fee_normalized, precision_muls[coin_index]),
    ))
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
        let fee_bps = 4u64;

        let (lp_minted, fees) = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, lp_supply, amp, fee_bps,
        ).expect("first deposit should succeed");

        // D for equal 1M * 3 = ~3M * 10^18
        let three_million_18 = 3_000_000u128 * 1_000_000_000_000_000_000u128;
        let diff = if lp_minted > three_million_18 {
            lp_minted - three_million_18
        } else {
            three_million_18 - lp_minted
        };
        // Should be very close to 3M * 10^18
        assert!(
            diff < 1_000, // within 1000 units of 10^18
            "first deposit LP should be ~3M*10^18, got {}, diff {}",
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
        let fee_bps = 4u64;

        // First deposit to get LP supply
        let (initial_lp, _) = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, fee_bps,
        ).unwrap();

        // Proportional deposit: 10% more of each token
        let amounts: [u128; 3] = [
            100_000 * 100_000_000,  // 100k icUSD
            100_000 * 1_000_000,    // 100k ckUSDT
            100_000 * 1_000_000,    // 100k ckUSDC
        ];

        let (lp_minted, fees) = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, initial_lp, amp, fee_bps,
        ).expect("proportional deposit should succeed");

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
        let fee_bps = 4u64;

        // First deposit to get LP supply
        let (initial_lp, _) = calc_add_liquidity(
            &old_balances, &[0u128; 3], &precision_muls, 0, amp, fee_bps,
        ).unwrap();

        // Imbalanced deposit: only icUSD
        let amounts: [u128; 3] = [
            100_000 * 100_000_000,  // 100k icUSD
            0,
            0,
        ];

        let (lp_minted, fees) = calc_add_liquidity(
            &amounts, &old_balances, &precision_muls, initial_lp, amp, fee_bps,
        ).expect("imbalanced deposit should succeed");

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
        let result = calc_add_liquidity(
            &[0, 0, 0], &test_balances(), &test_precision_muls(), 1000, 100, 4,
        );
        assert!(matches!(result, Err(ThreePoolError::ZeroAmount)));
    }

    // ─── Task 11 tests: remove_liquidity ───

    #[test]
    fn test_calc_remove_liquidity_proportional() {
        let balances = test_balances();
        let lp_supply = 3_000_000u128 * 1_000_000_000_000_000_000u128; // ~3M * 10^18

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
        let fee_bps = 4u64;

        // Get initial LP supply from a first deposit
        let (lp_supply, _) = calc_add_liquidity(
            &balances, &[0u128; 3], &precision_muls, 0, amp, fee_bps,
        ).unwrap();

        // Burn 10% of LP to get ckUSDT (token 1)
        let lp_burn = lp_supply / 10;

        let (amount, fee) = calc_remove_one_coin(
            lp_burn, 1, &balances, &precision_muls, lp_supply, amp, fee_bps,
        ).expect("remove_one_coin should succeed");

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
    }
}
