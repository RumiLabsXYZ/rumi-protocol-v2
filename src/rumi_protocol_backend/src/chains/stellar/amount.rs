//! Exact XLM amount conversion and spendable-balance arithmetic.
//!
//! Stellar represents native XLM amounts in stroops, with 10,000,000 stroops
//! per XLM. The minimum reserve is network/account-state dependent, so callers
//! must pass the current minimum reserve instead of relying on a constant here.

use core::fmt;

/// Number of stroops in one XLM.
pub const STROOPS_PER_XLM: u64 = 10_000_000;
/// Number of decimal places supported by Stellar amounts.
pub const XLM_DECIMALS: usize = 7;

/// Errors returned when converting or accounting for XLM amounts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmountError {
    /// The input was empty or did not contain a whole-number component.
    InvalidAmount,
    /// The input contained a non-decimal character or more than one decimal point.
    InvalidCharacter,
    /// The amount had more than seven fractional decimal places.
    TooManyDecimalPlaces,
    /// The amount or sum of deductions exceeded the `u64` stroop range.
    Overflow,
    /// The live base reserve is zero or sponsorship counts cannot describe a
    /// valid non-negative account reserve requirement.
    InvalidReserveInputs,
    /// Reserve, pending obligations, and fee budget exceeded the balance.
    InsufficientBalance,
}

impl fmt::Display for AmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount => f.write_str("invalid XLM amount"),
            Self::InvalidCharacter => f.write_str("invalid character in XLM amount"),
            Self::TooManyDecimalPlaces => {
                f.write_str("XLM amount has more than seven decimal places")
            }
            Self::Overflow => f.write_str("XLM amount exceeds the stroop range"),
            Self::InvalidReserveInputs => f.write_str("invalid Stellar reserve inputs"),
            Self::InsufficientBalance => {
                f.write_str("balance does not cover reserve, obligations, and fee budget")
            }
        }
    }
}

impl std::error::Error for AmountError {}

/// Calculate an account's live minimum XLM balance in stroops.
///
/// Stellar's minimum balance is `(2 + subentry_count + num_sponsoring -
/// num_sponsored) * base_reserve`. The base reserve must come from the current
/// ledger; it is not a fixed protocol constant. Sponsoring adds reserve units
/// to this account, while sponsored entries remove them.
pub fn minimum_balance_stroops(
    base_reserve_in_stroops: u64,
    subentry_count: u32,
    num_sponsoring: u32,
    num_sponsored: u32,
) -> Result<u64, AmountError> {
    if base_reserve_in_stroops == 0 {
        return Err(AmountError::InvalidReserveInputs);
    }

    let reserve_units = 2_u64
        .checked_add(u64::from(subentry_count))
        .and_then(|units| units.checked_add(u64::from(num_sponsoring)))
        .ok_or(AmountError::Overflow)?;
    let reserve_units = reserve_units
        .checked_sub(u64::from(num_sponsored))
        .ok_or(AmountError::InvalidReserveInputs)?;

    reserve_units
        .checked_mul(base_reserve_in_stroops)
        .ok_or(AmountError::Overflow)
}

/// Parse a non-negative decimal XLM amount into stroops without floating point.
///
/// The input must have a whole-number component and may have one to seven
/// fractional digits. Signs, whitespace, separators, and exponent notation are
/// rejected so callers cannot silently round user-provided amounts.
pub fn parse_xlm_to_stroops(amount: &str) -> Result<u64, AmountError> {
    if amount.is_empty() {
        return Err(AmountError::InvalidAmount);
    }

    let mut parts = amount.split('.');
    let whole = parts.next().ok_or(AmountError::InvalidAmount)?;
    let fractional = parts.next();
    if parts.next().is_some() {
        return Err(AmountError::InvalidCharacter);
    }
    if whole.is_empty() || fractional == Some("") {
        return Err(AmountError::InvalidAmount);
    }

    let whole_stroops = parse_digits(whole)?
        .checked_mul(STROOPS_PER_XLM)
        .ok_or(AmountError::Overflow)?;

    let fractional_stroops = match fractional {
        None => 0,
        Some(digits) => {
            if digits.len() > XLM_DECIMALS {
                return Err(AmountError::TooManyDecimalPlaces);
            }
            let value = parse_digits(digits)?;
            value
                .checked_mul(10_u64.pow((XLM_DECIMALS - digits.len()) as u32))
                .ok_or(AmountError::Overflow)?
        }
    };

    whole_stroops
        .checked_add(fractional_stroops)
        .ok_or(AmountError::Overflow)
}

fn parse_digits(digits: &str) -> Result<u64, AmountError> {
    if digits.is_empty() {
        return Err(AmountError::InvalidAmount);
    }

    digits.bytes().try_fold(0_u64, |value, byte| {
        if !byte.is_ascii_digit() {
            return Err(AmountError::InvalidCharacter);
        }
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or(AmountError::Overflow)
    })
}

/// Format stroops as an exact XLM decimal with seven fractional digits.
pub fn format_stroops(stroops: u64) -> String {
    let whole = stroops / STROOPS_PER_XLM;
    let fractional = stroops % STROOPS_PER_XLM;
    format!("{whole}.{fractional:07}")
}

/// Calculate spendable XLM after the live minimum reserve and known deductions.
///
/// `pending_obligations` and `fee_budget` are denominated in stroops. This
/// function does not derive the reserve requirement: Stellar account reserve
/// depends on current network settings and account state, which the caller must
/// supply as `live_minimum_reserve`.
pub fn spendable_stroops(
    balance: u64,
    live_minimum_reserve: u64,
    pending_obligations: &[u64],
    fee_budget: u64,
) -> Result<u64, AmountError> {
    let obligations = pending_obligations
        .iter()
        .try_fold(0_u64, |total, amount| {
            total.checked_add(*amount).ok_or(AmountError::Overflow)
        })?;
    let required = live_minimum_reserve
        .checked_add(obligations)
        .and_then(|total| total.checked_add(fee_budget))
        .ok_or(AmountError::Overflow)?;
    balance
        .checked_sub(required)
        .ok_or(AmountError::InsufficientBalance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_exact_stroop_precision() {
        assert_eq!(parse_xlm_to_stroops("0").unwrap(), 0);
        assert_eq!(parse_xlm_to_stroops("1").unwrap(), STROOPS_PER_XLM);
        assert_eq!(parse_xlm_to_stroops("12.34").unwrap(), 123_400_000);
        assert_eq!(parse_xlm_to_stroops("0.0000001").unwrap(), 1);
        assert_eq!(parse_xlm_to_stroops("1.1234567").unwrap(), 11_234_567);
        assert_eq!(format_stroops(11_234_567), "1.1234567");
        assert_eq!(format_stroops(1), "0.0000001");
    }

    #[test]
    fn accepts_u64_max_and_rejects_amount_overflow() {
        assert_eq!(
            parse_xlm_to_stroops("1844674407370.9551615").unwrap(),
            u64::MAX
        );
        assert_eq!(
            parse_xlm_to_stroops("1844674407370.9551616"),
            Err(AmountError::Overflow)
        );
    }

    #[test]
    fn rejects_invalid_syntax_and_excess_precision() {
        for input in ["", ".1", "1.", " 1", "1 ", "-1", "+1", "1e2", "1,000"] {
            assert!(parse_xlm_to_stroops(input).is_err(), "accepted {input:?}");
        }
        assert_eq!(
            parse_xlm_to_stroops("0.00000001"),
            Err(AmountError::TooManyDecimalPlaces)
        );
        assert_eq!(
            parse_xlm_to_stroops("1.2.3"),
            Err(AmountError::InvalidCharacter)
        );
    }

    #[test]
    fn computes_spendable_from_live_reserve_obligations_and_fee_budget() {
        assert_eq!(
            spendable_stroops(20_000_000, 5_000_000, &[1_000_000, 2_000_000], 10_000),
            Ok(11_990_000)
        );
    }

    #[test]
    fn computes_live_minimum_balance_with_sponsorship_counts() {
        // (2 + 4 subentries + 3 sponsored by this account - 2 sponsored for
        // this account) * 5,000,000 stroops.
        assert_eq!(minimum_balance_stroops(5_000_000, 4, 3, 2), Ok(35_000_000));
        assert_eq!(minimum_balance_stroops(5_000_000, 0, 0, 0), Ok(10_000_000));
    }

    #[test]
    fn rejects_invalid_or_overflowing_live_reserve_inputs() {
        assert_eq!(
            minimum_balance_stroops(0, 0, 0, 0),
            Err(AmountError::InvalidReserveInputs)
        );
        assert_eq!(
            minimum_balance_stroops(1, 0, 0, 3),
            Err(AmountError::InvalidReserveInputs)
        );
        assert_eq!(
            minimum_balance_stroops(u64::MAX, 0, 0, 0),
            Err(AmountError::Overflow)
        );
    }

    #[test]
    fn reports_spendable_underflow_and_obligation_overflow() {
        assert_eq!(
            spendable_stroops(10, 5, &[4], 2),
            Err(AmountError::InsufficientBalance)
        );
        assert_eq!(
            spendable_stroops(10, 0, &[u64::MAX, 1], 0),
            Err(AmountError::Overflow)
        );
        assert_eq!(
            spendable_stroops(u64::MAX, u64::MAX, &[], 1),
            Err(AmountError::Overflow)
        );
    }
}
