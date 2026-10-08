use super::{
    checked_icusd_add, checked_proportional_amount, collateral_usd_value,
    collateral_with_bonus_capped, icusd_to_collateral_amount,
    icusd_to_collateral_amount_for_burned_debt, try_icusd_to_collateral_amount,
    try_icusd_to_collateral_amount_with_bonus, Ratio, ICUSD,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

#[test]
fn debt_addition_rejects_u64_overflow_before_mint() {
    assert_eq!(
        checked_icusd_add(ICUSD::new(u64::MAX - 1), ICUSD::new(1)),
        Some(ICUSD::new(u64::MAX))
    );
    assert_eq!(checked_icusd_add(ICUSD::new(u64::MAX), ICUSD::new(1)), None);
}

#[test]
fn proportional_amount_preserves_legacy_rounding_and_avoids_overflow() {
    let legacy = (rust_decimal::Decimal::from(7u64) * rust_decimal::Decimal::from(13u64)
        / rust_decimal::Decimal::from(19u64))
    .to_u64()
    .unwrap();
    assert_eq!(checked_proportional_amount(7, 19, 13), Some(legacy));

    let large = checked_proportional_amount(u64::MAX / 2, u64::MAX, u64::MAX);
    assert_eq!(large, Some(u64::MAX / 2));
    assert_eq!(checked_proportional_amount(1, 0, 10), None);
}

#[test]
fn collateral_usd_value_saturates_above_u64_instead_of_zeroing() {
    assert_eq!(
        collateral_usd_value(1_000_000_000, dec!(10), 8),
        ICUSD::new(10_000_000_000)
    );
    assert_eq!(
        collateral_usd_value(u64::MAX, dec!(10_000_000_000), 8),
        ICUSD::new(u64::MAX)
    );
    assert_eq!(
        collateral_usd_value(u64::MAX, dec!(1), 20),
        ICUSD::new(18_446_744)
    );
    assert_eq!(collateral_usd_value(u64::MAX, dec!(0), 8), ICUSD::new(0));
}

#[test]
fn collateral_domain_bonus_avoids_icusd_intermediate_overflow_and_overseizure() {
    let debt = ICUSD::new(u64::MAX);
    let bonus = Ratio::new(dec!(1.15));

    // The debt is near u64::MAX, so narrowing its bonus-adjusted ICUSD e8
    // intermediate would overflow even though the final collateral fits.
    assert_eq!(
        try_icusd_to_collateral_amount_with_bonus(debt, dec!(10_000_000_000), 8, bonus),
        Some(2_121_375_568)
    );
    // Preflight and committed post-pull accounting use identical direct
    // Decimal math for fractional raw units (staged rounding would yield 6).
    assert_eq!(
        try_icusd_to_collateral_amount_with_bonus(
            ICUSD::new(610_000_000),
            dec!(100_000_000),
            8,
            bonus,
        ),
        Some(7)
    );

    // When the final raw amount itself is unrepresentable, post-payment
    // settlement clamps to physical vault collateral rather than trapping.
    let available_collateral = 10_000;
    let seized = try_icusd_to_collateral_amount_with_bonus(debt, dec!(1), 18, bonus)
        .unwrap_or(u64::MAX)
        .min(available_collateral);
    assert_eq!(seized, available_collateral);
}

#[test]
fn collateral_conversion_checks_u64_boundary() {
    let fits = try_icusd_to_collateral_amount(ICUSD::new(1_844_674_407), dec!(1), 18);
    assert_eq!(fits, Some(18_446_744_070_000_000_000));

    let exceeds = ICUSD::new(1_844_674_408);
    assert_eq!(
        try_icusd_to_collateral_amount(exceeds, dec!(1), 18),
        None,
        "the first amount above u64::MAX must be reported as unrepresentable"
    );
    let trapped = std::panic::catch_unwind(|| icusd_to_collateral_amount(exceeds, dec!(1), 18));
    assert!(trapped.is_err(), "normal conversion must fail closed");
}

#[test]
fn finalized_burn_overflow_clamps_to_available_collateral() {
    let required = icusd_to_collateral_amount_for_burned_debt(
        ICUSD::new(2_000_000_000), // $20 at $1, 18 decimals exceeds u64::MAX
        dec!(1),
        18,
    );
    assert_eq!(required, u64::MAX);

    let vault_collateral = 7_000_000_000_000_000_000;
    assert_eq!(
        collateral_with_bonus_capped(required, Ratio::new(dec!(1.1)), vault_collateral),
        vault_collateral,
        "the production bonus sizing must cap before narrowing to a token"
    );

    let invalid_decimals = ICUSD::new(100_000_000);
    assert_eq!(
        try_icusd_to_collateral_amount(invalid_decimals, dec!(1), 20),
        None,
        "a decimal scale that cannot fit in u64 must be reported as unrepresentable"
    );
    assert_eq!(
        icusd_to_collateral_amount_for_burned_debt(invalid_decimals, dec!(1), 20),
        u64::MAX,
        "finalized burns must remain settleable for an invalid scale"
    );

    let decimal_overflow = ICUSD::new(100_000_000);
    let tiny_price = dec!(0.0000000000000000000000000001);
    assert_eq!(
        try_icusd_to_collateral_amount(decimal_overflow, tiny_price, 18),
        None,
        "checked Decimal multiplication must report an unrepresentable amount"
    );
    assert_eq!(
        icusd_to_collateral_amount_for_burned_debt(decimal_overflow, tiny_price, 18),
        u64::MAX,
        "the finalized-burn path must handle Decimal overflow without trapping"
    );
}
