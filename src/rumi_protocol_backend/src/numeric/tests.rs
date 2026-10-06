use super::*;
use rust_decimal_macros::dec;

#[test]
fn checked_icusd_collateral_conversion_rejects_realistic_18_decimal_overflow() {
    // Required floor: 1 icUSD * 150% / $0.05 * 10^18 = 30e18 raw
    // units, above u64::MAX. Returning zero here lets a debt-bearing
    // vault withdraw its entire collateral.
    let required = ICUSD::new(150_000_000);
    let price = Decimal::new(5, 2);
    assert_eq!(try_icusd_to_collateral_amount(required, price, 18), None);
    // Keep the historical wrapper stable for replay/redemption callers;
    // financially sensitive live paths use the checked API above.
    assert_eq!(icusd_to_collateral_amount(required, price, 18), 0);
}

#[test]
fn checked_icusd_collateral_conversion_preserves_representable_amounts() {
    assert_eq!(
        try_icusd_to_collateral_amount(ICUSD::new(100_000_000), dec!(5.0), 8),
        Some(20_000_000)
    );
    assert_eq!(
        try_icusd_to_collateral_amount(ICUSD::new(100_000_000), Decimal::ZERO, 8),
        None
    );
}

#[test]
fn checked_icusd_collateral_conversion_rejects_unrepresentable_decimal_scale() {
    // A malformed or unexpected token metadata value must fail closed rather
    // than overflow the integer power used to scale whole tokens to raw units.
    assert_eq!(
        try_icusd_to_collateral_amount(ICUSD::new(100_000_000), dec!(1), 20),
        None
    );
    assert_eq!(
        try_icusd_to_collateral_amount(ICUSD::new(100_000_000), dec!(1), u8::MAX),
        None
    );
}
