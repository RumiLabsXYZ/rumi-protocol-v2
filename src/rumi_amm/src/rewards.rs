//! Per-LP reward accumulator helpers for AMM1 earnings distribution.
//!
//! Implements a Masterchef-style scheme: every donation to a pool
//! bumps `acc_reward_per_share`; each LP's pending reward is
//! `(shares * acc_per_share / SCALE) - reward_debt`. The settled
//! amount accumulates in `claimable` and is paid on `claim_rewards`.

use crate::state::REWARD_SCALE;
use crate::types::RewardState;

/// Compute pending reward for an LP given current accumulator and shares.
/// Pure function; no state mutation.
pub fn pending(shares: u128, acc_per_share: u128, reward_debt: u128) -> u128 {
    shares
        .saturating_mul(acc_per_share)
        .checked_div(REWARD_SCALE)
        .unwrap_or(0)
        .saturating_sub(reward_debt)
}

/// Settle: fold pending into claimable. Caller is responsible for
/// resetting `reward_debt = new_shares * acc_per_share / SCALE` after
/// any subsequent share change.
pub fn settle(state: &mut RewardState, shares: u128, acc_per_share: u128) {
    let p = pending(shares, acc_per_share, state.reward_debt);
    state.claimable = state.claimable.saturating_add(p);
}

/// Set reward_debt after a share change so future settles only credit
/// rewards earned from this point forward.
pub fn reset_debt(state: &mut RewardState, shares: u128, acc_per_share: u128) {
    state.reward_debt = shares
        .saturating_mul(acc_per_share)
        .checked_div(REWARD_SCALE)
        .unwrap_or(0);
}

/// Increment the accumulator with a new donation. Caller must have
/// already verified `total_shares > 0`.
pub fn accumulate(acc_per_share: u128, donation: u128, total_shares: u128) -> u128 {
    debug_assert!(total_shares > 0, "accumulate called with zero shares");
    let delta = donation
        .saturating_mul(REWARD_SCALE)
        .checked_div(total_shares)
        .unwrap_or(0);
    acc_per_share.saturating_add(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_zero_when_no_acc() {
        assert_eq!(pending(1_000, 0, 0), 0);
    }

    #[test]
    fn pending_basic_division() {
        // 1000 shares * (5 * SCALE) / SCALE = 5000, minus 0 debt = 5000
        let acc = 5 * REWARD_SCALE;
        assert_eq!(pending(1_000, acc, 0), 5_000);
    }

    #[test]
    fn pending_with_debt_subtracts() {
        let acc = 5 * REWARD_SCALE;
        // 1000 * 5 = 5000 gross, 2000 already credited
        assert_eq!(pending(1_000, acc, 2_000), 3_000);
    }

    #[test]
    fn settle_folds_into_claimable() {
        let mut s = RewardState { reward_debt: 0, claimable: 100 };
        let acc = 5 * REWARD_SCALE;
        settle(&mut s, 1_000, acc);
        assert_eq!(s.claimable, 100 + 5_000);
    }

    #[test]
    fn reset_debt_after_share_change() {
        let mut s = RewardState::default();
        let acc = 7 * REWARD_SCALE;
        reset_debt(&mut s, 2_000, acc);
        // After reset, pending should be zero.
        assert_eq!(pending(2_000, acc, s.reward_debt), 0);
    }

    #[test]
    fn accumulate_distributes_evenly() {
        // Donation of 1000, total shares 100 => acc grows by 10 * SCALE
        let acc1 = accumulate(0, 1_000, 100);
        assert_eq!(acc1, 10 * REWARD_SCALE);
        // A second donation of 500 with 100 shares => +5 * SCALE
        let acc2 = accumulate(acc1, 500, 100);
        assert_eq!(acc2, 15 * REWARD_SCALE);
    }

    #[test]
    fn pro_rata_two_lps() {
        // Two LPs: A has 100 shares, B has 300 shares (total 400).
        // Donate 800. A should earn 200, B should earn 600.
        let acc = accumulate(0, 800, 400);
        assert_eq!(pending(100, acc, 0), 200);
        assert_eq!(pending(300, acc, 0), 600);
    }

    #[test]
    fn late_joiner_does_not_capture_past_rewards() {
        // A has 100 shares, donate 500 (acc = 5 * SCALE). A earns 500.
        // B joins with 100 shares. Their reward_debt is set so pending = 0.
        let acc = accumulate(0, 500, 100);
        let mut b = RewardState::default();
        reset_debt(&mut b, 100, acc);
        assert_eq!(pending(100, acc, b.reward_debt), 0);
        // Next donation of 200 with total 200 shares: A and B each earn 100.
        let acc2 = accumulate(acc, 200, 200);
        assert_eq!(pending(100, acc2, 0), 600); // A: original 500 + 100
        assert_eq!(pending(100, acc2, b.reward_debt), 100); // B: only 100
    }
}
