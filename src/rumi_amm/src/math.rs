// Constant-product AMM math.
//
// All arithmetic uses u128 to match ICRC-1 token amounts.
// Fee calculation: total_fee is deducted from input before the swap math,
// then protocol_fee is carved out of total_fee. The remainder stays in reserves (accrues to LPs).

use crate::types::AmmError;

/// Compute swap output for a constant-product pool.
///
/// Given reserves (reserve_in, reserve_out) and `amount_in` of the input token,
/// returns (amount_out, total_fee, protocol_fee).
///
/// Fee is taken from `amount_in` first:
///   fee = amount_in * fee_bps / 10_000
///   effective_in = amount_in - fee
///   amount_out = reserve_out * effective_in / (reserve_in + effective_in)
///
/// Protocol fee is carved from total fee:
///   protocol_fee = fee * protocol_fee_bps / 10_000
///   lp_fee = fee - protocol_fee   (stays in reserves)
pub fn compute_swap(
    reserve_in: u128,
    reserve_out: u128,
    amount_in: u128,
    fee_bps: u16,
    protocol_fee_bps: u16,
) -> Result<(u128, u128, u128), AmmError> {
    if amount_in == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if reserve_in == 0 || reserve_out == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }

    // Total fee deducted from input
    let total_fee = amount_in
        .checked_mul(fee_bps as u128)
        .ok_or(AmmError::MathOverflow)?
        / 10_000;

    let effective_in = amount_in
        .checked_sub(total_fee)
        .ok_or(AmmError::FeeBpsOutOfRange)?;

    // Constant product: dy = reserve_out * dx / (reserve_in + dx)
    let numerator = reserve_out
        .checked_mul(effective_in)
        .ok_or(AmmError::MathOverflow)?;
    let denominator = reserve_in
        .checked_add(effective_in)
        .ok_or(AmmError::MathOverflow)?;

    let amount_out = numerator / denominator;

    if amount_out == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }

    // Protocol's share of the fee
    let protocol_fee = total_fee
        .checked_mul(protocol_fee_bps as u128)
        .ok_or(AmmError::MathOverflow)?
        / 10_000;

    Ok((amount_out, total_fee, protocol_fee))
}

/// Compute LP shares to mint for an initial liquidity deposit.
/// Uses geometric mean: sqrt(amount_a * amount_b).
/// Locks MINIMUM_LIQUIDITY to the zero address on first deposit.
pub const MINIMUM_LIQUIDITY: u128 = 1_000;

pub fn compute_initial_lp_shares(amount_a: u128, amount_b: u128) -> Result<u128, AmmError> {
    if amount_a == 0 || amount_b == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let product = amount_a
        .checked_mul(amount_b)
        .ok_or(AmmError::MathOverflow)?;
    let shares = isqrt(product);
    if shares <= MINIMUM_LIQUIDITY {
        return Err(AmmError::InsufficientLiquidity);
    }
    Ok(shares)
}

/// Compute LP shares for a proportional deposit into an existing pool.
/// shares = min(amount_a * total_shares / reserve_a, amount_b * total_shares / reserve_b)
pub fn compute_proportional_lp_shares(
    amount_a: u128,
    amount_b: u128,
    reserve_a: u128,
    reserve_b: u128,
    total_shares: u128,
) -> Result<u128, AmmError> {
    if amount_a == 0 || amount_b == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if reserve_a == 0 || reserve_b == 0 || total_shares == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    let shares_a = amount_a
        .checked_mul(total_shares)
        .ok_or(AmmError::MathOverflow)?
        / reserve_a;
    let shares_b = amount_b
        .checked_mul(total_shares)
        .ok_or(AmmError::MathOverflow)?
        / reserve_b;
    let shares = shares_a.min(shares_b);
    if shares == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    Ok(shares)
}

/// Compute token amounts returned when burning LP shares.
/// amount_a = shares * reserve_a / total_shares
/// amount_b = shares * reserve_b / total_shares
pub fn compute_remove_liquidity(
    shares: u128,
    reserve_a: u128,
    reserve_b: u128,
    total_shares: u128,
) -> Result<(u128, u128), AmmError> {
    if shares == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if shares > total_shares {
        return Err(AmmError::InsufficientLpShares {
            required: shares,
            available: total_shares,
        });
    }
    let amount_a = shares
        .checked_mul(reserve_a)
        .ok_or(AmmError::MathOverflow)?
        / total_shares;
    let amount_b = shares
        .checked_mul(reserve_b)
        .ok_or(AmmError::MathOverflow)?
        / total_shares;
    if amount_a == 0 && amount_b == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    Ok((amount_a, amount_b))
}

/// Integer square root (Newton's method).
fn isqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_basic() {
        // 1M/1M pool, swap 100k in with 0.3% fee
        let (out, fee, proto_fee) = compute_swap(1_000_000, 1_000_000, 100_000, 30, 0).unwrap();
        assert_eq!(fee, 300);
        assert_eq!(proto_fee, 0);
        // effective_in = 99_700
        // out = 1_000_000 * 99_700 / (1_000_000 + 99_700) = 90_661
        assert_eq!(out, 90_661);
    }

    #[test]
    fn test_swap_with_protocol_fee() {
        let (out, fee, proto_fee) = compute_swap(1_000_000, 1_000_000, 100_000, 30, 5000).unwrap();
        assert_eq!(fee, 300);
        assert_eq!(proto_fee, 150);
        // Output unchanged — protocol fee only affects accounting, not swap math
        assert_eq!(out, 90_661);
    }

    #[test]
    fn test_swap_zero_amount() {
        assert!(matches!(
            compute_swap(1000, 1000, 0, 30, 0),
            Err(AmmError::ZeroAmount)
        ));
    }

    #[test]
    fn test_initial_lp_shares() {
        let shares = compute_initial_lp_shares(1_000_000, 4_000_000).unwrap();
        // sqrt(1e6 * 4e6) = sqrt(4e12) = 2_000_000
        assert_eq!(shares, 2_000_000);
    }

    #[test]
    fn test_proportional_lp_shares() {
        let shares = compute_proportional_lp_shares(
            500_000, 500_000, 1_000_000, 1_000_000, 1_000_000,
        )
        .unwrap();
        assert_eq!(shares, 500_000);
    }

    #[test]
    fn test_remove_liquidity() {
        let (a, b) = compute_remove_liquidity(500_000, 1_000_000, 2_000_000, 1_000_000).unwrap();
        assert_eq!(a, 500_000);
        assert_eq!(b, 1_000_000);
    }

    #[test]
    fn test_swap_fee_bps_over_10000() {
        let result = compute_swap(1_000_000, 1_000_000, 100_000, 20_000, 0);
        assert!(matches!(result, Err(AmmError::FeeBpsOutOfRange)));
    }

    #[test]
    fn test_proportional_shares_zero_reserve() {
        let result = compute_proportional_lp_shares(100, 100, 0, 1000, 1000);
        assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
    }

    #[test]
    fn test_proportional_shares_zero_total_shares() {
        let result = compute_proportional_lp_shares(100, 100, 1000, 1000, 0);
        assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
    }

    #[test]
    fn test_remove_liquidity_zero_output() {
        let result = compute_remove_liquidity(1, 1_000_000_000, 1_000_000_000, 1_000_000_000_000);
        assert!(matches!(result, Err(AmmError::InsufficientLiquidity)));
    }

    #[test]
    fn test_isqrt() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(10), 3); // floor
        assert_eq!(isqrt(1_000_000), 1_000);
    }
}
