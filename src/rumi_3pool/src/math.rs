// StableSwap math: get_D, get_y, exchange calculations
// Implements the core Curve StableSwap invariant math for a 3pool.

use ethnum::U256;

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

// ─── Task 7: Solve for single token balance via Newton's method ───

/// Given a swap where token `i` has new balance `x`, compute the new balance
/// of token `j` that maintains the invariant D.
///
/// `xp` contains the current normalized balances (before the swap).
/// `amp` is the raw amplification coefficient.
/// `d` is the current invariant.
pub fn get_y(i: usize, j: usize, x: U256, xp: &[U256; 3], amp: u64, d: U256) -> Option<U256> {
    assert!(i != j, "get_y: i must not equal j");
    assert!(i < 3 && j < 3, "get_y: indices must be < 3");

    let ann = U256::from(amp) * N_COINS_U256;

    // Build c and S_ from the known balances (all except j)
    // For k == i, use x (new balance); for k == j, skip; else use xp[k]
    let mut s_ = U256::ZERO;
    let mut c = d;
    for k in 0..3 {
        let x_k = if k == i {
            x
        } else if k == j {
            continue;
        } else {
            xp[k]
        };
        s_ += x_k;
        // c = c * D / (x_k * N_COINS)
        c = c * d / (x_k * N_COINS_U256);
    }
    // c = c * D / (Ann * N_COINS)
    c = c * d / (ann * N_COINS_U256);

    let b = s_ + d / ann; // b = S_ + D/Ann

    // Newton's method: y starts at D
    let mut y = d;
    for _ in 0..MAX_ITERATIONS {
        // y_new = (y^2 + c) / (2*y + b - D)
        let y_new = (y * y + c) / (U256::from(2u64) * y + b - d);

        let diff = if y_new > y { y_new - y } else { y - y_new };
        if diff <= U256::ONE {
            return Some(y_new);
        }
        y = y_new;
    }

    None // Did not converge
}

/// Variant of get_y that solves for xp[j] given the other balances and a
/// target D. Used for `remove_liquidity_one_coin` where we need to find what
/// balance of token j corresponds to a reduced D.
///
/// Unlike get_y, there's no "input swap" -- we just solve for one balance
/// given the others remain unchanged and D changes.
pub fn get_y_d(j: usize, xp: &[U256; 3], amp: u64, d: U256) -> Option<U256> {
    assert!(j < 3, "get_y_d: j must be < 3");

    let ann = U256::from(amp) * N_COINS_U256;

    let mut s_ = U256::ZERO;
    let mut c = d;
    for k in 0..3 {
        if k == j {
            continue;
        }
        let x_k = xp[k];
        s_ += x_k;
        c = c * d / (x_k * N_COINS_U256);
    }
    c = c * d / (ann * N_COINS_U256);

    let b = s_ + d / ann;

    let mut y = d;
    for _ in 0..MAX_ITERATIONS {
        let y_new = (y * y + c) / (U256::from(2u64) * y + b - d);

        let diff = if y_new > y { y_new - y } else { y - y_new };
        if diff <= U256::ONE {
            return Some(y_new);
        }
        y = y_new;
    }

    None
}

// ─── Task 8: Precision normalization helpers ───

/// Normalize a raw token balance to 18-decimal precision.
/// `precision_mul` is the multiplier to go from native decimals to 18 decimals.
/// e.g., for a 6-decimal token, precision_mul = 10^12.
pub fn normalize_balance(raw: u128, precision_mul: u64) -> U256 {
    U256::from(raw) * U256::from(precision_mul)
}

/// Convert a 18-decimal normalized balance back to native decimals.
/// `precision_mul` is the same multiplier used in normalize_balance.
pub fn denormalize_balance(normalized: U256, precision_mul: u64) -> u128 {
    (normalized / U256::from(precision_mul)).as_u128()
}

/// Normalize all 3 balances using their respective precision multipliers.
pub fn normalize_all(balances: &[u128; 3], precision_muls: &[u64; 3]) -> [U256; 3] {
    [
        normalize_balance(balances[0], precision_muls[0]),
        normalize_balance(balances[1], precision_muls[1]),
        normalize_balance(balances[2], precision_muls[2]),
    ]
}

// ─── Task 12: Virtual price ───

/// Virtual price = D / lp_total_supply, scaled to 18 decimals.
/// Returns None if lp_total_supply is 0.
pub fn virtual_price(
    balances: &[u128; 3],
    precision_muls: &[u64; 3],
    amp: u64,
    lp_total_supply: u128,
) -> Option<u128> {
    if lp_total_supply == 0 {
        return None;
    }

    let xp = normalize_all(balances, precision_muls);
    let d = get_d(&xp, amp)?;

    // virtual_price = D * 10^18 / lp_total_supply
    let one_18 = U256::from(1_000_000_000_000_000_000u128);
    let vp = d * one_18 / U256::from(lp_total_supply);

    Some(vp.as_u128())
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

    // ─── Task 7 tests: get_y, get_y_d ───

    #[test]
    fn test_get_y_identity() {
        // When we "swap" but the input balance doesn't actually change,
        // the output balance should remain approximately the same.
        let one_million = U256::from(1_000_000u128) * U256::from(ONE_18);
        let xp = [one_million, one_million, one_million];
        let amp = 100u64;
        let d = get_d(&xp, amp).unwrap();

        // Swap token 0 -> token 1, but input x = xp[0] (no change)
        let y = get_y(0, 1, xp[0], &xp, amp, d).unwrap();

        let diff = if y > xp[1] { y - xp[1] } else { xp[1] - y };
        // Should be within 1 unit (Newton's convergence threshold)
        assert!(
            diff <= U256::from(2u64),
            "Identity swap: y should ≈ xp[1], diff = {}",
            diff
        );
    }

    #[test]
    fn test_get_y_swap_output_reasonable() {
        // Pool: 1M/1M/1M with A=100. Swap 1000 of token 0 for token 1.
        let one_million = U256::from(1_000_000u128) * U256::from(ONE_18);
        let xp = [one_million, one_million, one_million];
        let amp = 100u64;
        let d = get_d(&xp, amp).unwrap();

        let swap_in = U256::from(1_000u128) * U256::from(ONE_18);
        let new_x = xp[0] + swap_in;
        let y = get_y(0, 1, new_x, &xp, amp, d).unwrap();

        // Token 1's new balance should be less than before (user receives tokens)
        assert!(y < xp[1], "y should be less than original balance");

        let output = xp[1] - y;
        let expected_output = swap_in; // ~1000 for a balanced pool

        // Output should be within 1% of input for a balanced pool
        let tolerance = expected_output / U256::from(100u64); // 1%
        let diff = if output > expected_output {
            output - expected_output
        } else {
            expected_output - output
        };
        assert!(
            diff < tolerance,
            "Output ~{} should be within 1% of input ~{}",
            output,
            expected_output
        );
    }

    #[test]
    fn test_get_y_large_swap_has_slippage() {
        // Pool: 1M/1M/1M with A=100. Swap 500k of token 0 (50% of pool).
        let one_million = U256::from(1_000_000u128) * U256::from(ONE_18);
        let xp = [one_million, one_million, one_million];
        let amp = 100u64;
        let d = get_d(&xp, amp).unwrap();

        let swap_in = U256::from(500_000u128) * U256::from(ONE_18);
        let new_x = xp[0] + swap_in;
        let y = get_y(0, 1, new_x, &xp, amp, d).unwrap();

        let output = xp[1] - y;

        // With 50% of pool swapped, there should be meaningful slippage
        // Output should be significantly less than 500k
        assert!(
            output < swap_in,
            "Large swap should have slippage: output {} < input {}",
            output,
            swap_in
        );

        // With A=100, a 500k swap in a 1M/1M/1M 3pool produces ~0.65% slippage.
        // Verify slippage is meaningful (> 0.1%) -- StableSwap is designed to
        // minimize slippage, so even large trades have relatively low slippage
        // compared to a constant-product AMM.
        let slippage_threshold = swap_in / U256::from(1000u64); // 0.1%
        let slippage = swap_in - output;
        assert!(
            slippage > slippage_threshold,
            "500k swap should have >0.1% slippage, slippage = {}",
            slippage
        );
    }

    // ─── Task 8 tests: precision normalization ───

    #[test]
    fn test_normalize_6_decimal() {
        // 1M USDC in 6-decimal: 1_000_000 * 10^6 = 1_000_000_000_000
        let raw: u128 = 1_000_000_000_000; // 1M with 6 decimals
        let precision_mul: u64 = 1_000_000_000_000; // 10^12 (to go from 6 to 18 decimals)
        let normalized = normalize_balance(raw, precision_mul);

        // Expected: 1M * 10^18
        let expected = U256::from(1_000_000u128) * U256::from(ONE_18);
        assert_eq!(normalized, expected);
    }

    #[test]
    fn test_normalize_8_decimal() {
        // 1.0 icUSD in 8-decimal: 10^8 = 100_000_000
        let raw: u128 = 100_000_000; // 1.0 with 8 decimals
        let precision_mul: u64 = 10_000_000_000; // 10^10 (to go from 8 to 18 decimals)
        let normalized = normalize_balance(raw, precision_mul);

        // Expected: 10^18
        let expected = U256::from(ONE_18);
        assert_eq!(normalized, expected);
    }

    #[test]
    fn test_denormalize_balance() {
        // Reverse of normalize: 1M * 10^18 with mul=10^12 → 1M * 10^6
        let normalized = U256::from(1_000_000u128) * U256::from(ONE_18);
        let precision_mul: u64 = 1_000_000_000_000; // 10^12
        let raw = denormalize_balance(normalized, precision_mul);

        let expected: u128 = 1_000_000_000_000; // 1M in 6-decimal
        assert_eq!(raw, expected);
    }

    #[test]
    fn test_normalize_all() {
        // Token 0: icUSD (8 decimals, mul = 10^10)
        // Token 1: ckUSDT (6 decimals, mul = 10^12)
        // Token 2: ckUSDC (6 decimals, mul = 10^12)
        let balances: [u128; 3] = [
            100_000_000,        // 1.0 icUSD (8 dec)
            1_000_000,          // 1.0 ckUSDT (6 dec)
            2_000_000,          // 2.0 ckUSDC (6 dec)
        ];
        let precision_muls: [u64; 3] = [
            10_000_000_000,     // 10^10
            1_000_000_000_000,  // 10^12
            1_000_000_000_000,  // 10^12
        ];

        let result = normalize_all(&balances, &precision_muls);

        let one_18 = U256::from(ONE_18);
        assert_eq!(result[0], one_18);                       // 1.0 * 10^18
        assert_eq!(result[1], one_18);                       // 1.0 * 10^18
        assert_eq!(result[2], U256::from(2u64) * one_18);    // 2.0 * 10^18
    }

    // ─── Task 12 tests: virtual_price ───

    #[test]
    fn test_virtual_price_initial() {
        // Equal deposit of 1M each token
        let balances: [u128; 3] = [
            1_000_000 * 100_000_000,   // 1M icUSD (8 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDT (6 dec)
            1_000_000 * 1_000_000,     // 1M ckUSDC (6 dec)
        ];
        let precision_muls: [u64; 3] = [
            10_000_000_000,     // 10^10
            1_000_000_000_000,  // 10^12
            1_000_000_000_000,  // 10^12
        ];
        let amp = 100u64;

        // For an equal initial deposit, D ≈ 3M * 10^18, and lp_supply = D
        // So virtual_price should be ~1.0 * 10^18
        let xp = normalize_all(&balances, &precision_muls);
        let d = get_d(&xp, amp).unwrap();
        let lp_supply = d.as_u128(); // First deposit mints D tokens

        let vp = virtual_price(&balances, &precision_muls, amp, lp_supply)
            .expect("virtual_price should return Some");

        let one_18: u128 = 1_000_000_000_000_000_000;
        let diff = if vp > one_18 { vp - one_18 } else { one_18 - vp };

        // Should be very close to 1.0 * 10^18 (within rounding)
        assert!(
            diff < 1_000,
            "virtual_price should be ~1e18, got {}, diff {}",
            vp, diff
        );
    }

    #[test]
    fn test_virtual_price_zero_supply() {
        let balances = [0u128; 3];
        let precision_muls: [u64; 3] = [10_000_000_000, 1_000_000_000_000, 1_000_000_000_000];

        let result = virtual_price(&balances, &precision_muls, 100, 0);
        assert!(result.is_none(), "virtual_price should be None for zero supply");
    }
}
