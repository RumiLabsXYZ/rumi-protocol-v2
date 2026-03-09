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
}
