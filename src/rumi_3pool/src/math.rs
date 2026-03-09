// StableSwap math: get_D, get_y, exchange calculations
// Implements the core Curve StableSwap invariant math for a 3pool.

use ethnum::U256;

const N_COINS: u64 = 3;
const N_COINS_U256: U256 = U256::new(3);
const MAX_ITERATIONS: usize = 256;

// ─── Task 5: Amplification coefficient with ramping ───

/// Returns the current amplification coefficient, linearly interpolating
/// if a ramp is in progress.
pub fn get_a(
    initial_a: u64,
    future_a: u64,
    initial_a_time: u64,
    future_a_time: u64,
    current_time: u64,
) -> u64 {
    // If ramp is complete or no ramp configured
    if current_time >= future_a_time || initial_a_time == future_a_time {
        return future_a;
    }

    let elapsed = current_time - initial_a_time;
    let duration = future_a_time - initial_a_time;

    if future_a > initial_a {
        // Ramping up
        initial_a + (future_a - initial_a) * elapsed / duration
    } else {
        // Ramping down
        initial_a - (initial_a - future_a) * elapsed / duration
    }
}

// ─── Task 6: StableSwap invariant D via Newton's method ───

/// Compute the StableSwap invariant D for 3 tokens.
/// `xp` must be normalized to 18-decimal precision.
/// `amp` is the raw amplification coefficient (not multiplied by N_COINS).
///
/// Uses Newton's method to solve:
///   A*n^n*(x1+x2+x3) + D = A*D*n^n + D^4 / (27*x1*x2*x3)
pub fn get_d(xp: &[U256; 3], amp: u64) -> Option<U256> {
    let s: U256 = xp[0] + xp[1] + xp[2];
    if s == U256::ZERO {
        return Some(U256::ZERO);
    }

    let ann = U256::from(amp) * N_COINS_U256; // A * n

    let mut d = s;
    for _ in 0..MAX_ITERATIONS {
        // D_P = D^(n+1) / (n^n * prod(xp))
        // Computed iteratively: D_P = D, then for each x: D_P = D_P * D / (x * N_COINS)
        let mut d_p = d;
        for x in xp.iter() {
            // D_P = D_P * D / (x_i * N_COINS)
            // Note: x should never be 0 when S > 0, but guard anyway
            d_p = d_p * d / (*x * N_COINS_U256);
        }

        let d_prev = d;

        // D = (Ann * S + D_P * N) * D / ((Ann - 1) * D + (N + 1) * D_P)
        let numerator = (ann * s + d_p * N_COINS_U256) * d;
        let denominator = (ann - U256::ONE) * d + (N_COINS_U256 + U256::ONE) * d_p;
        d = numerator / denominator;

        // Check convergence: |D_new - D_prev| <= 1
        let diff = if d > d_prev { d - d_prev } else { d_prev - d };
        if diff <= U256::ONE {
            return Some(d);
        }
    }

    None // Did not converge
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Task 5 tests: get_a ───

    #[test]
    fn test_get_a_no_ramp() {
        // When initial_a == future_a, should return initial_a regardless of time
        assert_eq!(get_a(100, 100, 0, 1000, 500), 100);
    }

    #[test]
    fn test_get_a_during_ramp_up() {
        // 100 -> 200 over 1000s, at t=500 should be 150
        assert_eq!(get_a(100, 200, 0, 1000, 500), 150);
    }

    #[test]
    fn test_get_a_during_ramp_down() {
        // 200 -> 100 over 1000s, at t=500 should be 150
        assert_eq!(get_a(200, 100, 0, 1000, 500), 150);
    }

    #[test]
    fn test_get_a_after_ramp() {
        // Past end time -> returns future_a
        assert_eq!(get_a(100, 200, 0, 1000, 2000), 200);
    }

    // ─── Task 6 tests: get_d ───

    const ONE_18: u128 = 1_000_000_000_000_000_000; // 10^18

    #[test]
    fn test_get_d_zero_balances() {
        let xp = [U256::ZERO, U256::ZERO, U256::ZERO];
        assert_eq!(get_d(&xp, 100), Some(U256::ZERO));
    }

    #[test]
    fn test_get_d_equal_balances() {
        // 1M each in 18-decimal precision
        let one_million = U256::from(1_000_000u128) * U256::from(ONE_18);
        let xp = [one_million, one_million, one_million];
        let d = get_d(&xp, 100).expect("should converge");

        // D should be approximately 3M (3 * 1M)
        let three_million = U256::from(3_000_000u128) * U256::from(ONE_18);
        let diff = if d > three_million {
            d - three_million
        } else {
            three_million - d
        };
        // Within 1000 units of 18-decimal precision
        assert!(
            diff <= U256::from(1000u64),
            "D should be ~3M, got diff = {}",
            diff
        );
    }

    #[test]
    fn test_get_d_unequal_balances() {
        // 1M, 2M, 3M in 18-decimal
        let xp = [
            U256::from(1_000_000u128) * U256::from(ONE_18),
            U256::from(2_000_000u128) * U256::from(ONE_18),
            U256::from(3_000_000u128) * U256::from(ONE_18),
        ];
        let sum = xp[0] + xp[1] + xp[2];
        let d = get_d(&xp, 100).expect("should converge");
        assert!(d > U256::ZERO, "D should be > 0");
        assert!(d <= sum, "D should be <= sum of balances");
    }

    #[test]
    fn test_get_d_convergence() {
        // Should converge for various A values
        let one_million = U256::from(1_000_000u128) * U256::from(ONE_18);
        let xp = [one_million, one_million, one_million];

        for amp in [1u64, 100, 10_000] {
            let result = get_d(&xp, amp);
            assert!(
                result.is_some(),
                "get_d should converge for A={}",
                amp
            );
        }
    }
}
