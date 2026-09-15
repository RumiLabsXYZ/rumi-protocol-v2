//! Pure history and burn accounting helpers.

use crate::types::{AdvisoryCyclesBalance, Sample};

/// Computes corrected burn for one interval. A missing endpoint or an
/// unresolved funding operation makes the result indeterminate. Saturating
/// addition prevents an adversarial amount from wrapping into a false zero.
pub fn calculate_burn(
    starting_balance: Option<u128>,
    ending_balance: Option<u128>,
    confirmed_topups: &[u128],
    contains_unknown_funding: bool,
) -> Option<u128> {
    if contains_unknown_funding {
        return None;
    }
    let start = starting_balance?;
    let end = ending_balance?;
    let credited = confirmed_topups
        .iter()
        .try_fold(start, |sum, amount| sum.checked_add(*amount))?;
    Some(credited.saturating_sub(end))
}

/// The terminal-summary store is globally bounded and evicts the oldest
/// `resolved_at_secs`.  Below capacity every completed credit is retained. At
/// capacity, an interval is provably covered only when its starting sample is
/// strictly newer than the oldest retained summary. Equality is indeterminate:
/// an evicted credit later in that same second could be omitted by the strict
/// interval filter.
pub fn interval_coverage_complete(
    starting_sample_secs: Option<u64>,
    summary_count: usize,
    summary_capacity: usize,
    oldest_retained_resolved_at_secs: Option<u64>,
) -> bool {
    let Some(start) = starting_sample_secs else {
        return false;
    };
    if summary_count < summary_capacity {
        return true;
    }
    summary_count == summary_capacity
        && oldest_retained_resolved_at_secs.is_some_and(|oldest| start > oldest)
}

// Keep this pure helper's explicit interval inputs visible at callsites; they
// correspond one-for-one to the retained-history coverage contract.
#[allow(clippy::too_many_arguments)]
pub fn calculate_burn_for_interval(
    starting_sample_secs: Option<u64>,
    summary_count: usize,
    summary_capacity: usize,
    oldest_retained_resolved_at_secs: Option<u64>,
    starting_balance: Option<u128>,
    ending_balance: Option<u128>,
    confirmed_topups: &[u128],
    contains_unknown_funding: bool,
) -> Option<u128> {
    interval_coverage_complete(
        starting_sample_secs,
        summary_count,
        summary_capacity,
        oldest_retained_resolved_at_secs,
    )
    .then(|| {
        calculate_burn(
            starting_balance,
            ending_balance,
            confirmed_topups,
            contains_unknown_funding,
        )
    })
    .flatten()
}

/// Converts an interval's total balance delta into a per-hour rate. A zero,
/// regressed, or unrepresentable interval is indeterminate.
pub fn normalize_burn_per_hour(total_burn: u128, elapsed_secs: u64) -> Option<u128> {
    if elapsed_secs == 0 {
        return None;
    }
    total_burn
        .checked_mul(3_600)?
        .checked_div(u128::from(elapsed_secs))
}

pub fn calculate_burn_from_samples(
    previous: Option<&Sample>,
    current: Option<&Sample>,
    confirmed_topups: &[u128],
    contains_unknown_funding: bool,
) -> Option<u128> {
    calculate_burn(
        previous.and_then(|s| {
            s.balance
                .as_ref()
                .and_then(AdvisoryCyclesBalance::low_balance_value)
        }),
        current.and_then(|s| {
            s.balance
                .as_ref()
                .and_then(AdvisoryCyclesBalance::low_balance_value)
        }),
        confirmed_topups,
        contains_unknown_funding,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burn_corrects_confirmed_topups() {
        assert_eq!(
            calculate_burn(Some(1_000), Some(700), &[500], false),
            Some(800)
        );
        assert_eq!(calculate_burn(Some(700), Some(1_000), &[], false), Some(0));
    }

    #[test]
    fn unknown_or_missing_interval_is_indeterminate() {
        assert_eq!(calculate_burn(Some(1_000), Some(700), &[500], true), None);
        assert_eq!(calculate_burn(None, Some(700), &[], false), None);
        assert_eq!(calculate_burn(Some(1_000), None, &[], false), None);
    }

    #[test]
    fn saturating_topups_cannot_wrap_burn_accounting() {
        assert_eq!(calculate_burn(Some(u128::MAX), Some(0), &[1], false), None);
    }

    #[test]
    fn burn_rate_normalizes_variable_intervals() {
        assert_eq!(normalize_burn_per_hour(200, 7_200), Some(100));
        assert_eq!(normalize_burn_per_hour(1, 0), None);
        assert_eq!(normalize_burn_per_hour(u128::MAX, 1), None);
    }

    #[test]
    fn retention_gap_makes_eviction_affected_burn_indeterminate() {
        let credits = vec![1u128; 513];
        assert_eq!(
            calculate_burn_for_interval(
                Some(99),
                512,
                512,
                Some(100),
                Some(1_000),
                Some(500),
                &credits,
                false,
            ),
            None
        );
        assert_eq!(
            calculate_burn_for_interval(
                Some(100),
                512,
                512,
                Some(100),
                Some(1_000),
                Some(500),
                &[513],
                false,
            ),
            None
        );
        assert_eq!(
            calculate_burn_for_interval(
                Some(101),
                512,
                512,
                Some(100),
                Some(1_000),
                Some(500),
                &[513],
                false,
            ),
            Some(1_013)
        );
        assert!(interval_coverage_complete(Some(99), 511, 512, None));
    }
}
