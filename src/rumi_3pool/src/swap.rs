// Swap logic for the Rumi 3pool.

use crate::math::*;
use crate::types::*;

/// Calculate the output amount for a swap, before any token transfers.
/// Returns (output_amount_native, fee_amount_native) in token j's native decimals.
pub fn calc_swap_output(
    i: usize,
    j: usize,
    dx_native: u128,
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    swap_fee_bps: u64,
) -> Result<(u128, u128), ThreePoolError> {
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

    // Apply swap fee
    let fee_normalized = dy_normalized * ethnum::U256::from(swap_fee_bps)
        / ethnum::U256::from(10_000u64);
    let dy_after_fee = dy_normalized - fee_normalized;

    // Denormalize back to native decimals
    let output = denormalize_balance(dy_after_fee, precision_muls[j]);
    let fee = denormalize_balance(fee_normalized, precision_muls[j]);

    Ok((output, fee))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_swap_output() {
        // Setup: 1M of each token in native decimals
        // icUSD: 8 decimals, ckUSDT: 6 decimals, ckUSDC: 6 decimals
        let balances: [u128; 3] = [
            1_000_000 * 100_000_000,   // 1M icUSD (8 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDT (6 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDC (6 dec)
        ];
        let precision_muls: [u64; 3] = [
            10_000_000_000,     // 10^10 (8 -> 18)
            1_000_000_000_000,  // 10^12 (6 -> 18)
            1_000_000_000_000,  // 10^12 (6 -> 18)
        ];
        let amp = 100u64;
        let swap_fee_bps = 4u64; // 0.04%

        // Swap 1000 icUSD for ckUSDT
        let dx = 1_000 * 100_000_000u128; // 1000 icUSD in native
        let (output, fee) = calc_swap_output(0, 1, dx, &balances, &precision_muls, amp, swap_fee_bps)
            .expect("swap should succeed");

        let expected_ckusdt = 1_000 * 1_000_000u128; // 1000 ckUSDT in native

        // Output should be between 99% and 100% of 1000 ckUSDT
        assert!(
            output > expected_ckusdt * 99 / 100,
            "output {} should be > 99% of {}", output, expected_ckusdt
        );
        assert!(
            output < expected_ckusdt,
            "output {} should be < {}", output, expected_ckusdt
        );

        // Fee should be > 0
        assert!(fee > 0, "fee should be > 0, got {}", fee);
    }

    #[test]
    fn test_calc_swap_output_zero_amount() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];

        let result = calc_swap_output(0, 1, 0, &balances, &precision_muls, 100, 4);
        assert!(matches!(result, Err(ThreePoolError::ZeroAmount)));
    }

    #[test]
    fn test_calc_swap_output_same_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];

        let result = calc_swap_output(1, 1, 1000, &balances, &precision_muls, 100, 4);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }

    #[test]
    fn test_calc_swap_output_invalid_index() {
        let balances: [u128; 3] = [100_000_000, 1_000_000, 1_000_000];
        let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];

        let result = calc_swap_output(0, 3, 1000, &balances, &precision_muls, 100, 4);
        assert!(matches!(result, Err(ThreePoolError::InvalidCoinIndex)));
    }
}
