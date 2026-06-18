//! Chain-vault interest accrual math + harvest (Phase 1b Task 12, Option B).
//!
//! Mirrors the ICP-native `State::accrue_single_vault` factor formula
//! (`new_debt = ceil(debt * (1 + rate * elapsed / NANOS_PER_YEAR))`, round UP =
//! protocol favor, overflow => defer) but for foreign-chain vaults whose debt is
//! denominated in e8s `u128`. Unlike ICP, accrued interest is NOT folded into
//! `debt_e8s` here (that would break the chain supply invariant); it is only
//! realized when the on-chain interest mint confirms (see `evm/settlement.rs`).

use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;

use crate::numeric::NANOS_PER_YEAR;

/// Interest (e8s) accrued on `debt_e8s` at `apr_bps` (fraction x 10^-4; 200 =
/// 2.00% APR) over `elapsed_ns` nanoseconds. Rounds UP (protocol favor). Returns
/// 0 on zero debt / zero elapsed / zero rate, and DEFERS (returns 0, never
/// panics) on any Decimal overflow — mirroring ICP's overflow-defer.
pub fn accrued_chain_interest_e8s(debt_e8s: u128, apr_bps: u64, elapsed_ns: u64) -> u128 {
    if debt_e8s == 0 || apr_bps == 0 || elapsed_ns == 0 {
        return 0;
    }
    // Defer (not panic) if debt cannot be represented as a Decimal.
    let debt = match Decimal::from_u128(debt_e8s) {
        Some(d) => d,
        None => return 0,
    };
    let rate = Decimal::from(apr_bps) / Decimal::from(10_000u64);
    let factor =
        Decimal::ONE + rate * Decimal::from(elapsed_ns) / Decimal::from(NANOS_PER_YEAR);
    // new_debt = ceil(debt * factor); the accrued interest is the delta. Defer
    // on a Decimal->u128 overflow (unreachable at real debt scales).
    let new_debt = match (debt * factor).ceil().to_u128() {
        Some(n) => n,
        None => return 0,
    };
    new_debt.saturating_sub(debt_e8s)
}

#[cfg(test)]
mod tests {
    use super::accrued_chain_interest_e8s;
    const E8: u128 = 100_000_000;
    const NANOS_PER_YEAR: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

    #[test]
    fn two_percent_full_year_on_100_icusd_is_2_icusd() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, NANOS_PER_YEAR), 2 * E8);
    }

    #[test]
    fn half_year_is_half_the_interest() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, NANOS_PER_YEAR / 2), E8);
    }

    #[test]
    fn rounds_up_protocol_favor() {
        // A 1ns window on 100 icUSD yields a sub-e8s interest that must ceil to 1.
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, 1), 1);
    }

    #[test]
    fn zero_debt_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(0, 200, NANOS_PER_YEAR), 0);
    }

    #[test]
    fn zero_elapsed_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 200, 0), 0);
    }

    #[test]
    fn zero_bps_zero_interest() {
        assert_eq!(accrued_chain_interest_e8s(100 * E8, 0, NANOS_PER_YEAR), 0);
    }

    #[test]
    fn overflow_defers_to_zero_not_panic() {
        // u128::MAX exceeds Decimal's range -> defer (0), never panic.
        assert_eq!(accrued_chain_interest_e8s(u128::MAX, 200, NANOS_PER_YEAR), 0);
    }
}
