//! Asset valuation (spec Section 7 "Asset valuation", implementation plan Phase 5).
//!
//!   - ckUSDC, ckUSDT, icUSD, 3USD: valued at $1.00 face. No peg-aware cap
//!     (dropped from v1).
//!   - ICP: protocol XRC oracle rate at snapshot time (the same `last_icp_rate`
//!     the backend serves via `get_protocol_status`).
//!   - 3USD/LP for the verification cap (spec Section 5): valued at the 3pool's
//!     `virtual_price`, so a depositor is not under-credited as pool yield pushes
//!     the LP above $1.00 face. POSITION values stay at $1.00 face; only the
//!     verification cap is virtual-price aware.
//!
//! Canonical fixed-point is `usd_e8s` (`1e8` == $1.00). Native ledger decimals
//! (pinned 2026-06-02): icUSD 8, 3USD 8, ckUSDC 6, ckUSDT 6, ICP 8. The async
//! fetchers that pull the three 3USD venues for `verified_3usd` live with the
//! snapshot driver; this module is the pure valuation math it calls.

#![allow(dead_code)] // wired by the Phase 5 accrual driver

use crate::types::AssetType;

/// `1e18`, the scale of the 3pool's `virtual_price` (pinned: `vp = D_18dec * 1e8 /
/// lp_supply_8dec`, so 1 whole LP = `virtual_price / 1e18` USD).
pub const VIRTUAL_PRICE_SCALE: u128 = 1_000_000_000_000_000_000;

/// Snapshot-wide prices fetched ONCE per snapshot (not per principal) and threaded
/// through accrual: the ICP/USD oracle rate and the 3pool LP virtual price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotPrices {
    /// USD per 1 whole ICP (`get_protocol_status().last_icp_rate`).
    pub icp_rate: f64,
    /// 3pool LP virtual price, `1e18`-scaled (`get_pool_status().virtual_price`).
    pub virtual_price: u128,
}

/// Face USD value of `native_amount` (in the asset's NATIVE ledger decimals) in
/// `usd_e8s`. Stables resolve to $1.00 face; ICP uses the oracle rate.
pub fn value_usd_e8s(asset: AssetType, native_amount: u128, prices: &SnapshotPrices) -> u128 {
    match asset {
        // Already 8-decimal; face value is the amount itself.
        AssetType::IcUsd | AssetType::ThreeUsd => native_amount,
        // 6-decimal native; scale up to the 8-decimal usd_e8s space.
        AssetType::CkUsdc | AssetType::CkUsdt => native_amount.saturating_mul(100),
        // 8-decimal native; value at the oracle rate. `usd_e8s = native_e8s *
        // rate` because (native/1e8) ICP * rate USD, re-scaled by 1e8, is
        // native * rate. f64 is deterministic on the IC (the backend serves this
        // rate as f64); the single multiply is the only float, at the boundary.
        // A non-finite or negative rate (corrupt oracle) values to 0 rather than
        // casting Infinity to u128::MAX and dwarfing every other principal.
        AssetType::Icp => {
            if !prices.icp_rate.is_finite() || prices.icp_rate <= 0.0 {
                0
            } else {
                (native_amount as f64 * prices.icp_rate).round() as u128
            }
        }
    }
}

/// USD value (`usd_e8s`) of `lp_e8s` 3USD/LP tokens at `virtual_price`, used only
/// for the 3USD verification cap. `lp_e8s * virtual_price / 1e18`.
pub fn value_lp_at_vp(lp_e8s: u128, virtual_price: u128) -> u128 {
    lp_e8s.saturating_mul(virtual_price) / VIRTUAL_PRICE_SCALE
}

/// Face USD value of a STABLE asset, no oracle needed (the ingestion path has no
/// snapshot prices). The 3pool legs and SP/repayment assets are all stables; ICP
/// is never valued here.
pub fn value_stable_usd_e8s(asset: AssetType, native_amount: u128) -> u128 {
    value_usd_e8s(
        asset,
        native_amount,
        &SnapshotPrices { icp_rate: 0.0, virtual_price: 0 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prices(icp_rate: f64, virtual_price: u128) -> SnapshotPrices {
        SnapshotPrices { icp_rate, virtual_price }
    }

    #[test]
    fn stables_are_face_value_at_eight_decimals() {
        let p = prices(0.0, VIRTUAL_PRICE_SCALE);
        // icUSD / 3USD are already 8-decimal: $1.00 -> 1e8, no scaling.
        assert_eq!(value_usd_e8s(AssetType::IcUsd, 100_000_000, &p), 100_000_000);
        assert_eq!(value_usd_e8s(AssetType::ThreeUsd, 250_000_000, &p), 250_000_000);
    }

    #[test]
    fn ck_stables_scale_six_to_eight_decimals() {
        let p = prices(0.0, VIRTUAL_PRICE_SCALE);
        // ckUSDC / ckUSDT are 6-decimal native: $1.00 == 1_000_000 native -> 1e8.
        assert_eq!(value_usd_e8s(AssetType::CkUsdc, 1_000_000, &p), 100_000_000);
        assert_eq!(value_usd_e8s(AssetType::CkUsdt, 12_500_000, &p), 1_250_000_000);
    }

    #[test]
    fn icp_valued_at_oracle_rate() {
        // 1 ICP (1e8 e8s) at $5.00 -> $5.00.
        assert_eq!(
            value_usd_e8s(AssetType::Icp, 100_000_000, &prices(5.0, 0)),
            500_000_000
        );
        // 2 ICP at $5.50 -> $11.00.
        assert_eq!(
            value_usd_e8s(AssetType::Icp, 200_000_000, &prices(5.5, 0)),
            1_100_000_000
        );
    }

    #[test]
    fn lp_valued_at_virtual_price() {
        // 1 LP at vp 1.0 -> $1.00.
        assert_eq!(value_lp_at_vp(100_000_000, VIRTUAL_PRICE_SCALE), 100_000_000);
        // 1 LP at vp 1.05 -> $1.05 (yield accrued).
        assert_eq!(
            value_lp_at_vp(100_000_000, 1_050_000_000_000_000_000),
            105_000_000
        );
        // 2 LP at vp 1.0 -> $2.00.
        assert_eq!(value_lp_at_vp(200_000_000, VIRTUAL_PRICE_SCALE), 200_000_000);
    }

    #[test]
    fn lp_at_zero_virtual_price_is_zero_not_a_panic() {
        assert_eq!(value_lp_at_vp(100_000_000, 0), 0);
    }

    #[test]
    fn icp_non_finite_or_negative_rate_values_zero() {
        // A corrupt oracle rate must never produce a u128::MAX valuation that would
        // dwarf every other principal's points.
        assert_eq!(value_usd_e8s(AssetType::Icp, 100_000_000, &prices(f64::INFINITY, 0)), 0);
        assert_eq!(value_usd_e8s(AssetType::Icp, 100_000_000, &prices(f64::NAN, 0)), 0);
        assert_eq!(value_usd_e8s(AssetType::Icp, 100_000_000, &prices(-1.0, 0)), 0);
    }

    #[test]
    fn value_stable_helper_matches_face_value_with_decimal_scaling() {
        assert_eq!(value_stable_usd_e8s(AssetType::IcUsd, 100_000_000), 100_000_000);
        assert_eq!(value_stable_usd_e8s(AssetType::ThreeUsd, 5), 5);
        assert_eq!(value_stable_usd_e8s(AssetType::CkUsdc, 1_000_000), 100_000_000);
        assert_eq!(value_stable_usd_e8s(AssetType::CkUsdt, 2_000_000), 200_000_000);
    }
}
