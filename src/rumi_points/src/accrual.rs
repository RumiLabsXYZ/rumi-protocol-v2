//! Pure accrual math (spec Sections 4-6, implementation plan Phase 5). NO IC
//! calls: the snapshot driver (`epoch.rs`) fetches balances/prices and feeds them
//! here as `usd_e8s`, so every multiplier, verification, min(), and repayment-
//! window rule is unit-testable in isolation.
//!
//! `SnapshotWeights` are the per-source "weighted values" (value x multiplier) at
//! one snapshot, in `usd_e8s`. The two intra-epoch snapshots reduce by
//! `min_by_total` (keep the smaller-TOTAL snapshot's WHOLE breakdown, closing
//! cross-position sniping), then `scale_by_period` turns the weighted value into
//! `usd_e8s`-days points. Repayment-window points (Section 6) are computed
//! separately and added OUTSIDE the min() (a past repayment cannot be sniped).

#![allow(dead_code)] // wired by the Phase 5 accrual driver

use serde::{Deserialize, Serialize};

use crate::types::{AssetType, PointSource, RepaymentEvent};
use crate::valuation::{value_lp_at_vp, value_usd_e8s, SnapshotPrices};
use crate::NANOS_PER_DAY;

/// Per-source weighted values (value x multiplier, `usd_e8s`) captured at one
/// snapshot. Also the at-rest value of the MemoryId-11 snapshot buffer (wrapped in
/// a versioned `Stored*` enum in `state.rs`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotWeights {
    /// icUSD vault debt outstanding, 1x.
    pub icusd_debt: u128,
    /// icUSD deposited in the 3pool, 1x (after verification).
    pub icusd_3pool: u128,
    /// Matched ckUSDC+ckUSDT in the 3pool, `2*min(usdc,usdt)` at 5x.
    pub ck_matched: u128,
    /// Unmatched ck-stable in the 3pool, `|usdc-usdt|` at 3x.
    pub ck_unmatched: u128,
    /// icUSD in the stability pool, 1x.
    pub icusd_sp: u128,
    /// 3USD in the stability pool, 2x.
    pub threeusd_sp: u128,
    /// 3USD/ICP AMM LP, 2x.
    pub amm_lp: u128,
}

impl SnapshotWeights {
    /// Sum of all seven weighted values (the principal's snapshot total).
    pub fn total(&self) -> u128 {
        self.icusd_debt
            .saturating_add(self.icusd_3pool)
            .saturating_add(self.ck_matched)
            .saturating_add(self.ck_unmatched)
            .saturating_add(self.icusd_sp)
            .saturating_add(self.threeusd_sp)
            .saturating_add(self.amm_lp)
    }

    /// Non-zero `(source, weight)` pairs, for writing one `PointEntry` per active
    /// activity. Skips zero fields so no empty ledger rows are written.
    pub fn by_source(&self) -> Vec<(PointSource, u128)> {
        [
            (PointSource::IcUsdDebt, self.icusd_debt),
            (PointSource::IcUsd3Pool, self.icusd_3pool),
            (PointSource::CkStable3PoolMatched, self.ck_matched),
            (PointSource::CkStable3PoolUnmatched, self.ck_unmatched),
            (PointSource::IcUsdStabilityPool, self.icusd_sp),
            (PointSource::ThreeUsdStabilityPool, self.threeusd_sp),
            (PointSource::AmmLp, self.amm_lp),
        ]
        .into_iter()
        .filter(|(_, weight)| *weight > 0)
        .collect()
    }
}

/// Reduce the two intra-epoch snapshots: keep the snapshot with the SMALLER total
/// (its WHOLE breakdown, not a field-wise min, so a user cannot hold position X at
/// snapshot A and position Y at snapshot B and collect both). Ties keep `a`.
pub fn min_by_total(a: SnapshotWeights, b: SnapshotWeights) -> SnapshotWeights {
    if a.total() <= b.total() {
        a
    } else {
        b
    }
}

/// Turn weighted values (`usd_e8s`) into `usd_e8s`-days points by holding the
/// snapshot value over `period_ns`: each field `* period_ns / NANOS_PER_DAY`.
pub fn scale_by_period(w: SnapshotWeights, period_ns: u64) -> SnapshotWeights {
    let scale = |v: u128| v.saturating_mul(period_ns as u128) / NANOS_PER_DAY as u128;
    SnapshotWeights {
        icusd_debt: scale(w.icusd_debt),
        icusd_3pool: scale(w.icusd_3pool),
        ck_matched: scale(w.ck_matched),
        ck_unmatched: scale(w.ck_unmatched),
        icusd_sp: scale(w.icusd_sp),
        threeusd_sp: scale(w.threeusd_sp),
        amm_lp: scale(w.amm_lp),
    }
}

/// 0.5% upward tolerance on verified 3USD (spec open-question #3 / Phase 4),
/// absorbing rounding and dust without rewarding fragmentation.
const VERIFICATION_TOLERANCE_NUM: u128 = 1005;
const VERIFICATION_TOLERANCE_DEN: u128 = 1000;

/// One principal's snapshot inputs, all `usd_e8s`, fetched by the driver
/// (`epoch.rs`) so the math here stays pure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotInputs {
    /// Sum of the principal's current vault debt (icUSD face, 1:1 `usd_e8s`).
    pub vault_debt: u128,
    /// Recorded 3pool deposit composition (from `active_deposits`), `usd_e8s`.
    pub recorded_3pool_icusd: u128,
    pub recorded_3pool_usdc: u128,
    pub recorded_3pool_usdt: u128,
    /// Wallet + SP + AMM 3USD valued at `virtual_price`, summed (pre-tolerance).
    pub verified_3usd: u128,
    /// Stability-pool balances.
    pub sp_icusd: u128,
    pub sp_3usd: u128,
    /// AMM LP value (3USD share + ICP share), pre-multiplier.
    pub amm_lp_value: u128,
}

/// Apply the 0.5% upward tolerance to raw verified 3USD.
fn verification_cap(verified_3usd: u128) -> u128 {
    verified_3usd.saturating_mul(VERIFICATION_TOLERANCE_NUM) / VERIFICATION_TOLERANCE_DEN
}

/// Scale the recorded 3pool composition down by the verification factor
/// `min(cap, total) / total` (spec Section 5), applied uniformly so the
/// matched/unmatched split is unaffected by where we scale. Returns the effective
/// (icUSD, ckUSDC, ckUSDT) values in `usd_e8s`.
fn apply_verification(
    recorded_icusd: u128,
    recorded_usdc: u128,
    recorded_usdt: u128,
    cap: u128,
) -> (u128, u128, u128) {
    let total = recorded_icusd
        .saturating_add(recorded_usdc)
        .saturating_add(recorded_usdt);
    if total == 0 {
        return (0, 0, 0);
    }
    let capped = cap.min(total);
    let scale = |r: u128| r.saturating_mul(capped) / total;
    (scale(recorded_icusd), scale(recorded_usdc), scale(recorded_usdt))
}

/// One principal's per-source weighted values at a snapshot (the multiplier table,
/// spec Section 4). The caller skips excluded principals.
pub fn snapshot_weights(inp: &SnapshotInputs) -> SnapshotWeights {
    let (eff_icusd, eff_usdc, eff_usdt) = apply_verification(
        inp.recorded_3pool_icusd,
        inp.recorded_3pool_usdc,
        inp.recorded_3pool_usdt,
        verification_cap(inp.verified_3usd),
    );
    SnapshotWeights {
        icusd_debt: inp.vault_debt,
        icusd_3pool: eff_icusd,
        // matched value 2*min(usdc,usdt) at 5x; unmatched |usdc-usdt| at 3x.
        ck_matched: eff_usdc.min(eff_usdt).saturating_mul(2).saturating_mul(5),
        ck_unmatched: eff_usdc.abs_diff(eff_usdt).saturating_mul(3),
        icusd_sp: inp.sp_icusd,
        threeusd_sp: inp.sp_3usd.saturating_mul(2),
        amm_lp: inp.amm_lp_value.saturating_mul(2),
    }
}

/// Points for one repayment event's overlap with one epoch (spec Section 6),
/// computed OUTSIDE the snapshot min() because a past repayment cannot be sniped:
/// `amount_usd_e8s * 5 * overlap_ns / NANOS_PER_DAY`. `epoch_end_capped` is
/// `min(epoch_end, season_end)`; `window_end` is already capped at season end when
/// the event is recorded. Returns 0 when the window and epoch do not overlap.
pub fn repayment_points(
    amount_usd_e8s: u128,
    repaid_at: u64,
    window_end: u64,
    epoch_start: u64,
    epoch_end_capped: u64,
) -> u128 {
    let overlap_start = repaid_at.max(epoch_start);
    let overlap_end = window_end.min(epoch_end_capped);
    let overlap_ns = overlap_end.saturating_sub(overlap_start);
    amount_usd_e8s
        .saturating_mul(5)
        .saturating_mul(overlap_ns as u128)
        / NANOS_PER_DAY as u128
}

/// Raw per-principal query results at a snapshot, all `usd_e8s` except the AMM LP
/// counts (LP units) and reserves (`usd_e8s` for 3USD, native e8s for ICP). The
/// driver fills this from inter-canister queries; `build_snapshot_inputs` turns it
/// into `SnapshotInputs` (the AMM share math + virtual-price verification).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawSnapshot {
    pub vault_debt: u128,
    pub recorded_icusd: u128,
    pub recorded_usdc: u128,
    pub recorded_usdt: u128,
    /// Wallet 3USD/LP balance (3pool `icrc1_balance_of`).
    pub wallet_3usd: u128,
    pub sp_icusd: u128,
    pub sp_3usd: u128,
    /// AMM 3USD/ICP pool: the principal's LP, the pool's total LP, and reserves.
    pub amm_user_lp: u128,
    pub amm_total_lp: u128,
    pub amm_reserve_3usd: u128,
    pub amm_reserve_icp: u128,
}

/// Assemble `SnapshotInputs` from raw query results: derive the AMM 3USD/ICP shares
/// from the LP ratio, value the LP position at face, and value the verification
/// total (wallet + SP + AMM 3USD) at `virtual_price`.
pub fn build_snapshot_inputs(raw: &RawSnapshot, prices: &SnapshotPrices) -> SnapshotInputs {
    let (amm_3usd_share, amm_icp_share) = if raw.amm_total_lp == 0 {
        (0, 0)
    } else {
        (
            raw.amm_user_lp.saturating_mul(raw.amm_reserve_3usd) / raw.amm_total_lp,
            raw.amm_user_lp.saturating_mul(raw.amm_reserve_icp) / raw.amm_total_lp,
        )
    };
    // AMM LP position at face: 3USD share ($1) + ICP share at the oracle rate.
    let amm_lp_value =
        amm_3usd_share.saturating_add(value_usd_e8s(AssetType::Icp, amm_icp_share, prices));
    // Verification cap at virtual_price across all three 3USD venues.
    let verified_3usd = value_lp_at_vp(
        raw.wallet_3usd
            .saturating_add(raw.sp_3usd)
            .saturating_add(amm_3usd_share),
        prices.virtual_price,
    );
    SnapshotInputs {
        vault_debt: raw.vault_debt,
        recorded_3pool_icusd: raw.recorded_icusd,
        recorded_3pool_usdc: raw.recorded_usdc,
        recorded_3pool_usdt: raw.recorded_usdt,
        verified_3usd,
        sp_icusd: raw.sp_icusd,
        sp_3usd: raw.sp_3usd,
        amm_lp_value,
    }
}

/// Close-time accrual for one principal: scale the min-snapshot weights over the
/// epoch period into per-source points, then add the repayment-window points
/// (OUTSIDE the min). Returns the per-source ledger entries and the total delta.
pub fn accrue_principal(
    min_weights: SnapshotWeights,
    repayments: &[RepaymentEvent],
    epoch_start: u64,
    epoch_end_capped: u64,
) -> (Vec<(PointSource, u128)>, u128) {
    let period_ns = epoch_end_capped.saturating_sub(epoch_start);
    let mut entries = scale_by_period(min_weights, period_ns).by_source();
    let repay_total = repayments.iter().fold(0u128, |acc, r| {
        acc.saturating_add(repayment_points(
            r.amount_usd,
            r.repaid_at,
            r.window_end,
            epoch_start,
            epoch_end_capped,
        ))
    });
    if repay_total > 0 {
        entries.push((PointSource::VaultRepayment, repay_total));
    }
    let total = entries
        .iter()
        .fold(0u128, |acc, (_, p)| acc.saturating_add(*p));
    (entries, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS_PER_WEEK: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

    fn inputs() -> SnapshotInputs {
        SnapshotInputs::default()
    }

    fn w(
        icusd_debt: u128,
        icusd_3pool: u128,
        ck_matched: u128,
        ck_unmatched: u128,
        icusd_sp: u128,
        threeusd_sp: u128,
        amm_lp: u128,
    ) -> SnapshotWeights {
        SnapshotWeights {
            icusd_debt,
            icusd_3pool,
            ck_matched,
            ck_unmatched,
            icusd_sp,
            threeusd_sp,
            amm_lp,
        }
    }

    #[test]
    fn total_sums_all_seven_fields() {
        assert_eq!(w(1, 2, 4, 8, 16, 32, 64).total(), 127);
        assert_eq!(SnapshotWeights::default().total(), 0);
    }

    #[test]
    fn min_by_total_keeps_the_smaller_total_as_a_whole() {
        // a's total (100) < b's total (200), but b dominates a field-wise. The
        // result must equal a entirely, proving it is NOT a field-wise min.
        let a = w(100, 0, 0, 0, 0, 0, 0); // total 100
        let b = w(50, 50, 50, 50, 0, 0, 0); // total 200, larger in 3 fields
        assert_eq!(min_by_total(a, b), a);
        assert_eq!(min_by_total(b, a), a);
    }

    #[test]
    fn min_by_total_ties_keep_first() {
        let a = w(10, 0, 0, 0, 0, 0, 0);
        let b = w(0, 10, 0, 0, 0, 0, 0); // same total, different breakdown
        assert_eq!(min_by_total(a, b), a);
    }

    #[test]
    fn scale_by_period_full_week_multiplies_each_field_by_seven() {
        let scaled = scale_by_period(w(100, 200, 0, 0, 0, 0, 0), NS_PER_WEEK);
        assert_eq!(scaled.icusd_debt, 700);
        assert_eq!(scaled.icusd_3pool, 1_400);
    }

    #[test]
    fn scale_by_period_partial_days() {
        // One full day -> x1; half a day -> halved.
        assert_eq!(scale_by_period(w(100, 0, 0, 0, 0, 0, 0), NANOS_PER_DAY).icusd_debt, 100);
        assert_eq!(
            scale_by_period(w(100, 0, 0, 0, 0, 0, 0), NANOS_PER_DAY / 2).icusd_debt,
            50
        );
    }

    #[test]
    fn by_source_maps_nonzero_fields_only() {
        let got = w(10, 0, 5, 0, 0, 7, 0).by_source();
        assert_eq!(
            got,
            vec![
                (PointSource::IcUsdDebt, 10),
                (PointSource::CkStable3PoolMatched, 5),
                (PointSource::ThreeUsdStabilityPool, 7),
            ]
        );
        assert!(SnapshotWeights::default().by_source().is_empty());
    }

    // ── verification factor (spec Section 5) ──

    #[test]
    fn verification_cap_applies_half_percent_tolerance() {
        assert_eq!(verification_cap(1_000), 1_005);
        assert_eq!(verification_cap(0), 0);
    }

    #[test]
    fn verification_full_holding_does_not_scale() {
        // cap >= total -> factor 1 -> effective == recorded.
        assert_eq!(apply_verification(100, 200, 300, 10_000), (100, 200, 300));
    }

    #[test]
    fn verification_half_holding_scales_proportionally() {
        // recorded total 200, cap 100 -> factor 0.5.
        assert_eq!(apply_verification(0, 100, 100, 100), (0, 50, 50));
    }

    #[test]
    fn verification_zero_holding_zeros_everything() {
        assert_eq!(apply_verification(100, 200, 300, 0), (0, 0, 0));
    }

    #[test]
    fn verification_over_recorded_caps_at_recorded() {
        // Holding more 3USD than recorded never inflates above the recorded value.
        assert_eq!(apply_verification(100, 0, 0, 10_000), (100, 0, 0));
    }

    #[test]
    fn verification_zero_recorded_is_zero_regardless_of_cap() {
        assert_eq!(apply_verification(0, 0, 0, 999), (0, 0, 0));
    }

    // ── multiplier table (spec Section 4) ──

    #[test]
    fn snapshot_weights_apply_each_multiplier() {
        let got = snapshot_weights(&SnapshotInputs {
            vault_debt: 100,
            recorded_3pool_icusd: 100,
            verified_3usd: 1_000_000, // ample -> factor 1
            sp_icusd: 100,
            sp_3usd: 100,
            amm_lp_value: 100,
            ..inputs()
        });
        assert_eq!(got.icusd_debt, 100); // 1x
        assert_eq!(got.icusd_3pool, 100); // 1x
        assert_eq!(got.icusd_sp, 100); // 1x
        assert_eq!(got.threeusd_sp, 200); // 2x
        assert_eq!(got.amm_lp, 200); // 2x
        assert_eq!(got.ck_matched, 0);
        assert_eq!(got.ck_unmatched, 0);
    }

    #[test]
    fn matched_pair_accrues_5x_unmatched_3x() {
        let got = snapshot_weights(&SnapshotInputs {
            recorded_3pool_usdc: 50,
            recorded_3pool_usdt: 30,
            verified_3usd: 1_000_000,
            ..inputs()
        });
        // matched value 2*min(50,30)=60 at 5x -> 300; unmatched |50-30|=20 at 3x -> 60.
        assert_eq!(got.ck_matched, 300);
        assert_eq!(got.ck_unmatched, 60);
    }

    #[test]
    fn equal_pair_is_fully_matched() {
        let got = snapshot_weights(&SnapshotInputs {
            recorded_3pool_usdc: 40,
            recorded_3pool_usdt: 40,
            verified_3usd: 1_000_000,
            ..inputs()
        });
        assert_eq!(got.ck_matched, 2 * 40 * 5); // 400
        assert_eq!(got.ck_unmatched, 0);
    }

    #[test]
    fn dust_pairing_does_not_flip_the_whole_position_to_5x() {
        // The v1 exploit: $1 of ckUSDT next to $50,000 of ckUSDC. Only the matched
        // $2 gets 5x; the rest stays 3x.
        let got = snapshot_weights(&SnapshotInputs {
            recorded_3pool_usdc: 50_000,
            recorded_3pool_usdt: 1,
            verified_3usd: 1_000_000,
            ..inputs()
        });
        assert_eq!(got.ck_matched, 2 * 1 * 5); // 10
        assert_eq!(got.ck_unmatched, (50_000 - 1) * 3); // 149_997
    }

    #[test]
    fn verification_scales_the_3pool_multiplier_split() {
        // Holding only half the recorded 3USD halves the matched points.
        let got = snapshot_weights(&SnapshotInputs {
            recorded_3pool_usdc: 100,
            recorded_3pool_usdt: 100,
            verified_3usd: 100, // recorded total 200, cap ~100 -> factor 0.5
            ..inputs()
        });
        // eff 50/50 -> matched 2*50*5 = 500 (vs 1000 at full holding).
        assert_eq!(got.ck_matched, 500);
        assert_eq!(got.ck_unmatched, 0);
    }

    #[test]
    fn tolerance_grants_full_credit_within_half_percent() {
        // verified 1000, recorded 1003 (within 0.5%): cap 1005 -> capped 1003 -> full.
        let got = snapshot_weights(&SnapshotInputs {
            recorded_3pool_icusd: 1_003,
            verified_3usd: 1_000,
            ..inputs()
        });
        assert_eq!(got.icusd_3pool, 1_003);
    }

    // ── repayment window (spec Section 6, OUTSIDE the min()) ──

    const DOLLAR_1000: u128 = 1_000 * 100_000_000; // $1000 in usd_e8s

    #[test]
    fn repayment_full_epoch_overlap_is_amount_times_days_times_five() {
        // The window spans the whole 7-day epoch.
        let pts = repayment_points(DOLLAR_1000, 0, 100 * NANOS_PER_DAY, 0, NS_PER_WEEK);
        assert_eq!(pts, DOLLAR_1000 * 5 * 7);
    }

    #[test]
    fn repayment_partial_overlap_when_repaid_mid_epoch() {
        // Repaid on day 3 of a [0, 7d] epoch -> 4 days of overlap.
        let pts = repayment_points(DOLLAR_1000, 3 * NANOS_PER_DAY, 100 * NANOS_PER_DAY, 0, NS_PER_WEEK);
        assert_eq!(pts, DOLLAR_1000 * 5 * 4);
    }

    #[test]
    fn repayment_truncated_at_season_end() {
        // epoch_end_capped (2 days) < window_end -> only 2 days count.
        let pts = repayment_points(DOLLAR_1000, 0, 100 * NANOS_PER_DAY, 0, 2 * NANOS_PER_DAY);
        assert_eq!(pts, DOLLAR_1000 * 5 * 2);
    }

    #[test]
    fn repayment_window_end_caps_overlap() {
        // window_end (3 days) < epoch_end_capped (7 days) -> 3 days count.
        let pts = repayment_points(DOLLAR_1000, 0, 3 * NANOS_PER_DAY, 0, NS_PER_WEEK);
        assert_eq!(pts, DOLLAR_1000 * 5 * 3);
    }

    #[test]
    fn repayment_no_overlap_is_zero() {
        // Repaid after the epoch ends.
        assert_eq!(
            repayment_points(DOLLAR_1000, 10 * NANOS_PER_DAY, 100 * NANOS_PER_DAY, 0, NS_PER_WEEK),
            0
        );
        // Window ended before the epoch starts.
        assert_eq!(
            repayment_points(DOLLAR_1000, 0, NANOS_PER_DAY, 2 * NANOS_PER_DAY, NS_PER_WEEK),
            0
        );
    }

    #[test]
    fn repayment_sliced_across_epochs_sums_to_the_whole_window() {
        // Spec example: $1000 over a 47-day window -> 235,000 dollar-days, summed
        // over the 7-day epochs it spans (6 full weeks + a 5-day tail).
        let window_end = 47 * NANOS_PER_DAY;
        let mut total = 0u128;
        let mut start = 0u64;
        while start < window_end {
            let end_capped = (start + NS_PER_WEEK).min(window_end);
            total += repayment_points(DOLLAR_1000, 0, window_end, start, end_capped);
            start += NS_PER_WEEK;
        }
        assert_eq!(total, DOLLAR_1000 * 5 * 47); // 235,000 dollar-days x 1e8
    }

    // ── snapshot-input assembly + close-time accrual (step 8c) ──

    fn prices(icp_rate: f64, virtual_price: u128) -> SnapshotPrices {
        SnapshotPrices { icp_rate, virtual_price }
    }

    const VP1: u128 = 1_000_000_000_000_000_000; // virtual_price 1.0

    #[test]
    fn build_inputs_derives_amm_shares_and_vp_verification() {
        let raw = RawSnapshot {
            vault_debt: 1_000,
            recorded_icusd: 11,
            recorded_usdc: 22,
            recorded_usdt: 33,
            wallet_3usd: 300,
            sp_icusd: 77,
            sp_3usd: 50,
            amm_user_lp: 50,
            amm_total_lp: 100,            // 50% share
            amm_reserve_3usd: 200,        // share -> 100 (3USD, usd_e8s)
            amm_reserve_icp: 200_000_000, // share -> 1 ICP
        };
        let got = build_snapshot_inputs(&raw, &prices(5.0, VP1));
        assert_eq!(got.vault_debt, 1_000);
        assert_eq!(got.recorded_3pool_icusd, 11);
        assert_eq!(got.recorded_3pool_usdc, 22);
        assert_eq!(got.recorded_3pool_usdt, 33);
        assert_eq!(got.sp_icusd, 77);
        assert_eq!(got.sp_3usd, 50);
        // AMM LP value = 3USD share (100 at $1) + ICP share (1 ICP at $5 = 5e8).
        assert_eq!(got.amm_lp_value, 100 + 500_000_000);
        // verified = (wallet 300 + sp 50 + amm share 100) at vp 1.0 = 450.
        assert_eq!(got.verified_3usd, 450);
    }

    /// DESIGN NOTE — INTENTIONAL (Rob confirmed 2026-06-03), NOT a bug. The 3USD/LP
    /// a user receives from a 3pool deposit, when then staked in the stability pool,
    /// counts BOTH toward the 3pool verification cap (preserving full 3pool credit)
    /// AND as a 2x SP position: the same capital is rewarded twice. This is a
    /// DELIBERATE composability reward (it deepens SP liquidity). The verification
    /// cap only checks the LP is "still held somewhere"; it does not subtract SP
    /// usage. Pinned so any FUTURE change to this reward is a conscious choice.
    #[test]
    fn doc_sp_held_3usd_lp_stacks_3pool_and_sp_credit() {
        const D100: u128 = 100 * 100_000_000; // $100 in usd_e8s
        // Deposited $100 ckUSDC + $100 ckUSDT into the 3pool (recorded), and parks
        // the resulting ~$200 of 3USD/LP in the stability pool (sp_3usd).
        let raw = RawSnapshot {
            recorded_usdc: D100,
            recorded_usdt: D100,
            sp_3usd: 2 * D100,
            ..Default::default()
        };
        let inputs = build_snapshot_inputs(&raw, &prices(0.0, VP1));
        // The SP-held LP fully verifies the $200 recorded 3pool composition.
        assert_eq!(inputs.verified_3usd, 2 * D100);
        let w = snapshot_weights(&inputs);
        // Full 3pool matched credit (5x) AND the same capital earns SP 2x on top.
        assert_eq!(w.ck_matched, 2 * D100 * 5, "3pool 5x credit preserved");
        assert_eq!(w.threeusd_sp, 2 * D100 * 2, "SP 2x credit on the same LP");
    }

    #[test]
    fn build_inputs_handles_zero_amm_total_lp() {
        let raw = RawSnapshot {
            wallet_3usd: 10,
            sp_3usd: 5,
            amm_total_lp: 0, // empty pool: no division by zero, no share
            amm_reserve_3usd: 999,
            amm_user_lp: 7,
            ..Default::default()
        };
        let got = build_snapshot_inputs(&raw, &prices(5.0, VP1));
        assert_eq!(got.amm_lp_value, 0);
        assert_eq!(got.verified_3usd, 15); // (10 + 5 + 0) at vp 1.0
    }

    #[test]
    fn accrue_principal_scales_weights_and_adds_repayment() {
        let weights = SnapshotWeights { icusd_debt: 100, threeusd_sp: 50, ..Default::default() };
        let repayments = vec![RepaymentEvent {
            asset: AssetType::CkUsdc,
            amount_usd: 1_000 * 100_000_000, // $1000
            repaid_at: 0,
            window_end: u64::MAX,
        }];
        // Full-week epoch: balance fields x7; the repayment overlaps all 7 days.
        let (entries, total) = accrue_principal(weights, &repayments, 0, NS_PER_WEEK);
        assert!(entries.contains(&(PointSource::IcUsdDebt, 700)));
        assert!(entries.contains(&(PointSource::ThreeUsdStabilityPool, 350)));
        let repay = 1_000 * 100_000_000u128 * 5 * 7;
        assert!(entries.contains(&(PointSource::VaultRepayment, repay)));
        assert_eq!(total, 700 + 350 + repay);
    }

    #[test]
    fn accrue_principal_no_activity_is_empty() {
        let (entries, total) = accrue_principal(SnapshotWeights::default(), &[], 0, NS_PER_WEEK);
        assert!(entries.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn accrue_principal_omits_out_of_window_repayment() {
        let repayments = vec![RepaymentEvent {
            asset: AssetType::CkUsdt,
            amount_usd: 100,
            repaid_at: 10 * NANOS_PER_DAY, // starts after this epoch ends
            window_end: 20 * NANOS_PER_DAY,
        }];
        let (entries, total) =
            accrue_principal(SnapshotWeights::default(), &repayments, 0, NS_PER_WEEK);
        assert!(entries.is_empty());
        assert_eq!(total, 0);
    }
}
