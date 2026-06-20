//! Pure CR + partial-liquidation sizing math for the chains rail (spec §4.2,
//! §4.6, §4.7). All integer, all saturating, no I/O. Composed by
//! `chains::vault::begin_liquidation_in_state` and the detection tick.

/// `bonus_e4 = 10_000 + liquidation_penalty_bps` (spec §4.1.2). A repaid icUSD
/// releases `bonus_e4 / 10_000`x its value in collateral. NEVER hardcode 1.12.
pub fn bonus_e4_from_penalty_bps(liquidation_penalty_bps: u64) -> u64 {
    10_000u64.saturating_add(liquidation_penalty_bps)
}

/// USD value (e8s) of `collateral_native` at `price_e8`. Byte-identical to the
/// numerator math inside `chains::vault::collateral_ratio_e4` so the CR check and
/// the sizing agree.
pub fn collateral_value_e8s(collateral_native: u128, native_decimals: u8, price_e8: u64) -> u128 {
    let native_scale = 10u128.saturating_pow(native_decimals as u32);
    collateral_native.saturating_mul(price_e8 as u128) / native_scale
}

/// Interest-aware debt (spec §4.2): realized debt + reserved interest + freshly
/// accrued interest. The accrual window MUST be byte-identical to
/// `harvest_chain_interest_in_state` (`now_ns - last_interest_accrual_ns`), which
/// the CALLER computes and passes as `elapsed_ns`. Accrual is on `debt_e8s` only
/// (matching harvest), NOT on the pending term.
pub fn effective_debt_e8s(
    debt_e8s: u128,
    pending_interest_mint_e8s: u128,
    apr_bps: u64,
    elapsed_ns: u64,
) -> u128 {
    debt_e8s
        .saturating_add(pending_interest_mint_e8s)
        .saturating_add(crate::chains::interest::accrued_chain_interest_e8s(
            debt_e8s, apr_bps, elapsed_ns,
        ))
}

/// Partial-liquidation repay amount (e8s) that restores CR to AT LEAST
/// `target_cr_e4` (spec §4.6, MANDATORY property `restored_cr_e4 >=
/// target_cr_e4`). Port of ICP's `compute_partial_liquidation_cap`, all
/// saturating.
///
/// Rounding: the final division rounds repay UP (ceil). In this regime
/// `dCR/drepay > 0` (collateral is seized at the `bonus_e4` rate, 1.12x, while
/// debt clears 1:1, so MORE repay raises the restored CR), so rounding repay UP
/// is what guarantees the restored CR lands AT OR ABOVE target. (The spec's
/// prose said "round down → lands above target", which has the CR direction
/// backwards; the ceil is required to satisfy the spec's own mandatory property,
/// proven by `sized_repay_partial_restores_to_at_least_target_property`.) The
/// resulting over-clear is < 1 e8 unit (10^-8 icUSD), capped at `eff_debt_e8s` —
/// economically zero, NOT the material over-seizure findings #11/#19 guard.
pub fn sized_repay_e8s(
    eff_debt_e8s: u128,
    collateral_value_e8s: u128,
    target_cr_e4: u64,
    bonus_e4: u64,
) -> u128 {
    let numerator = eff_debt_e8s.saturating_mul(target_cr_e4 as u128) / 10_000;
    if numerator <= collateral_value_e8s {
        return 0; // already at/above target
    }
    if target_cr_e4 <= bonus_e4 {
        return eff_debt_e8s; // denominator <= 0 -> full close
    }
    let deficit = numerator - collateral_value_e8s;
    let denom_e4 = (target_cr_e4 - bonus_e4) as u128; // >= 1 (target > bonus checked above)
    // Ceil division: (deficit*10_000 + denom-1) / denom, capped at full debt.
    let scaled = deficit
        .saturating_mul(10_000)
        .saturating_add(denom_e4 - 1);
    (scaled / denom_e4).min(eff_debt_e8s)
}

/// DEX-depth cap (spec §4.7): the max repay (e8s) coverable by selling at most
/// `max_swap_value_e8s` of collateral at the bonus. ADVISORY ONLY (finding #3):
/// it is a static USD ceiling, NOT a fraction of live reserves. The real
/// guarantee is the submit-time live-reserves min-out gate (Increment 3). An
/// operator MUST re-tune `max_swap_value_e8s` as pool depth moves.
pub fn value_cap_to_repay(max_swap_value_e8s: u128, bonus_e4: u64) -> u128 {
    if bonus_e4 == 0 {
        return 0;
    }
    max_swap_value_e8s.saturating_mul(10_000) / (bonus_e4 as u128)
}

/// Collateral (native base units) to seize for `repay_e8s`, grossed up by the
/// bonus so the swap output covers the debt (spec §4.6). The CALLER caps this at
/// the vault's `collateral_amount_native` (cap-binds -> residual debt falls to
/// SP/manual in a later increment).
pub fn collateral_in_native_for_repay(
    repay_e8s: u128,
    bonus_e4: u64,
    native_decimals: u8,
    price_e8: u64,
) -> u128 {
    if price_e8 == 0 {
        return 0;
    }
    let native_scale = 10u128.saturating_pow(native_decimals as u32);
    let grossed_value_e8s = repay_e8s.saturating_mul(bonus_e4 as u128) / 10_000;
    grossed_value_e8s.saturating_mul(native_scale) / (price_e8 as u128)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: u64 = 15_500; // 155%
    const BONUS: u64 = 11_200; // 10_000 + 1_200 penalty bps = 1.12x

    #[test]
    fn collateral_value_matches_collateral_ratio_math() {
        // 1400 CFX (1400e18 wei) @ $0.15 (15_000_000 e8) = $210 = 210e8.
        let v = collateral_value_e8s(1_400u128 * 10u128.pow(18), 18, 15_000_000);
        assert_eq!(v, 210u128 * 100_000_000);
    }

    #[test]
    fn effective_debt_adds_pending_interest_and_accrual() {
        // No apr/elapsed -> effective == debt + pending_interest only.
        assert_eq!(effective_debt_e8s(100, 7, 0, 0), 107);
        // With apr+elapsed the accrued term is added on top of debt (not pending).
        let e = effective_debt_e8s(100_000_000, 0, 200, 31_557_600_000_000_000); // ~1yr @ 2%
        assert!(e > 100_000_000, "accrual added");
    }

    #[test]
    fn sized_repay_zero_when_at_or_above_target() {
        // collateral_value already >= debt*target/10000 -> nothing to repay.
        assert_eq!(sized_repay_e8s(100, 1_000, TARGET, BONUS), 0);
    }

    #[test]
    fn sized_repay_full_when_denominator_nonpositive() {
        // target <= bonus -> denom <= 0 -> repay capped at full debt.
        assert_eq!(sized_repay_e8s(100, 0, 11_000, 11_200), 100);
    }

    #[test]
    fn sized_repay_partial_restores_to_at_least_target_property() {
        // MANDATORY (spec §4.6): across a fuzzed grid, a partial repay restores
        // CR to >= target. Round-down sizing lands the vault AT OR ABOVE target.
        let mut checked = 0u64;
        for debt_units in 1u128..=60 {
            for cr_pct in 100u128..=154 {
                let eff_debt = debt_units * 100_000_000; // e8s
                                                         // collateral_value = debt * cr_pct/100
                let coll_val = eff_debt * cr_pct / 100;
                let repay = sized_repay_e8s(eff_debt, coll_val, TARGET, BONUS);
                if repay == 0 || repay >= eff_debt {
                    continue; // only the strict-partial case has a restore target
                }
                let coll_sold = repay.saturating_mul(BONUS as u128) / 10_000;
                if coll_sold > coll_val {
                    continue; // cap-binds case (handled by collateral clamp, not here)
                }
                let new_debt = eff_debt - repay;
                let new_coll = coll_val - coll_sold;
                let restored_cr_e4 = (new_coll.saturating_mul(10_000) / new_debt) as u64;
                assert!(
                    restored_cr_e4 >= TARGET,
                    "restored {restored_cr_e4} < target {TARGET} (debt={eff_debt} cv={coll_val} repay={repay})"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "property exercised on a meaningful grid ({checked})");
    }

    #[test]
    fn depth_cap_limits_repay_to_swappable_value() {
        // max_swap_value_e8s = $2000 (2000e8), bonus 1.12 -> max repay ~ 1785.7e8.
        let cap = value_cap_to_repay(2_000u128 * 100_000_000, BONUS);
        assert_eq!(cap, 2_000u128 * 100_000_000 * 10_000 / 11_200);
        // effective_repay = min(sized, cap)
        assert_eq!(cap.min(5_000u128 * 100_000_000), cap);
    }

    #[test]
    fn collateral_in_native_grosses_up_by_bonus() {
        // repay 100e8 @ $0.15, bonus 1.12, 18-dec: collateral worth $112 of CFX.
        let native = collateral_in_native_for_repay(100u128 * 100_000_000, BONUS, 18, 15_000_000);
        // $112 / $0.15 = 746.666.. CFX (round down)
        let expected = (112u128 * 100_000_000) // grossed value e8s
            .saturating_mul(10u128.pow(18))
            / 15_000_000;
        assert_eq!(native, expected);
    }

    #[test]
    fn bonus_from_penalty() {
        assert_eq!(bonus_e4_from_penalty_bps(1_200), 11_200);
    }
}
