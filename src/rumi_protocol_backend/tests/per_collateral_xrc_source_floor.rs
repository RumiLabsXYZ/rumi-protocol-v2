//! Wave-14a CDP-14 follow-up: per-collateral override for the XRC
//! source-count floor.
//!
//! `CollateralConfig.min_xrc_sources: Option<u32>` lets an operator
//! lower (or kill) the source-count gate for a single asset whose
//! underlying market is genuinely thin on XRC's CEX panel (XAUT, etc.)
//! without weakening the gate for every other collateral. `None` keeps
//! the global default (`State.min_xrc_sources_used`, set to
//! `MIN_XRC_SOURCES = 3` in production); `Some(n)` overrides it.
//!
//! The fences in this file pin the pure resolution behaviour. The
//! actual XRC-call wiring is exercised by the existing CDP-14 audit
//! tests; they continue to pass because a `None` override resolves to
//! the global default and the prior behaviour.

use candid::Principal;
use rumi_protocol_backend::state::{CollateralConfig, CollateralStatus, PriceSource, XrcAssetClass};
use rumi_protocol_backend::numeric::{ICUSD, Ratio};
use rumi_protocol_backend::xrc::{xrc_metadata_meets_source_floor, MIN_XRC_SOURCES};
use rust_decimal_macros::dec;

fn mock_cfg(min_xrc_sources: Option<u32>) -> CollateralConfig {
    // Minimal `CollateralConfig` for testing `effective_min_xrc_sources` —
    // every other field is irrelevant to this method.
    CollateralConfig {
        ledger_canister_id: Principal::anonymous(),
        decimals: 8,
        liquidation_ratio: Ratio::from(dec!(1.33)),
        borrow_threshold_ratio: Ratio::from(dec!(1.5)),
        liquidation_bonus: Ratio::from(dec!(1.1)),
        borrowing_fee: Ratio::from(dec!(0.003)),
        interest_rate_apr: Ratio::from(dec!(0.0)),
        debt_ceiling: u64::MAX,
        min_vault_debt: ICUSD::from(10_000_000),
        ledger_fee: 10_000,
        price_source: PriceSource::Xrc {
            base_asset: "ICP".to_string(),
            base_asset_class: XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: XrcAssetClass::FiatCurrency,
        },
        status: CollateralStatus::Active,
        last_price: None,
        last_price_timestamp: None,
        redemption_fee_floor: Ratio::from(dec!(0.003)),
        redemption_fee_ceiling: Ratio::from(dec!(0.05)),
        current_base_rate: Ratio::from(dec!(0)),
        last_redemption_time: 0,
        recovery_target_cr: Ratio::from(dec!(1.55)),
        min_collateral_deposit: 0,
        recovery_borrowing_fee: None,
        recovery_interest_rate_apr: None,
        display_color: None,
        healthy_cr: None,
        rate_curve: None,
        redemption_tier: 1,
        min_xrc_sources,
    }
}

#[test]
fn none_override_inherits_global_floor() {
    let cfg = mock_cfg(None);
    assert_eq!(cfg.effective_min_xrc_sources(3), 3, "None must inherit global=3");
    assert_eq!(cfg.effective_min_xrc_sources(5), 5, "None must inherit global=5");
    assert_eq!(
        cfg.effective_min_xrc_sources(MIN_XRC_SOURCES),
        MIN_XRC_SOURCES,
        "None must inherit MIN_XRC_SOURCES production default",
    );
}

#[test]
fn some_override_wins_over_global() {
    // The override is "this collateral's market has fewer sources than the
    // generic case" — should override DOWN as the typical use, but the
    // resolver is symmetric and supports raising too.
    let down = mock_cfg(Some(2));
    assert_eq!(down.effective_min_xrc_sources(3), 2, "Some(2) overrides global=3 downward");

    let up = mock_cfg(Some(5));
    assert_eq!(up.effective_min_xrc_sources(3), 5, "Some(5) overrides global=3 upward");
}

#[test]
fn zero_override_disables_per_collateral_gate() {
    // `Some(0)` is the per-collateral kill switch — matches the global
    // semantics. The helper's pre-existing contract is "0 always passes".
    let cfg = mock_cfg(Some(0));
    assert_eq!(cfg.effective_min_xrc_sources(3), 0, "Some(0) resolves to 0 (kill switch)");
    // And once 0, `xrc_metadata_meets_source_floor` short-circuits to true
    // for any sample count, including the degenerate 0-source case.
    let floor = cfg.effective_min_xrc_sources(3);
    assert!(xrc_metadata_meets_source_floor(0, floor));
    assert!(xrc_metadata_meets_source_floor(2, floor));
    assert!(xrc_metadata_meets_source_floor(7, floor));
}

#[test]
fn xaut_use_case_with_floor_two_accepts_two_source_samples() {
    // The reason this field exists: XAUT (Tether Gold) on XRC's CEX panel
    // consistently aggregates only 2 sources because the token trades on
    // only a handful of exchanges. With the strict global floor of 3,
    // ~30% of XRC ticks for XAUT got rejected even when XRC was healthy.
    let xaut_cfg = mock_cfg(Some(2));
    let floor = xaut_cfg.effective_min_xrc_sources(3);
    assert!(
        xrc_metadata_meets_source_floor(2, floor),
        "with per-collateral floor=2, two-source XAUT samples must be accepted",
    );
    // A one-source sample still gets rejected — `Some(2)` is "two or
    // more is OK", not "any sample is OK".
    assert!(
        !xrc_metadata_meets_source_floor(1, floor),
        "with per-collateral floor=2, one-source samples must still be rejected",
    );
}

#[test]
fn other_collaterals_unaffected_when_only_one_is_overridden() {
    // The whole point of per-collateral overrides: lowering XAUT's floor
    // must NOT change ICP's effective floor.
    let icp_cfg = mock_cfg(None);          // no override
    let xaut_cfg = mock_cfg(Some(2));      // overridden to 2

    let global = 3;
    assert_eq!(icp_cfg.effective_min_xrc_sources(global), 3);
    assert_eq!(xaut_cfg.effective_min_xrc_sources(global), 2);
    // A 2-source ICP tick is rejected; a 2-source XAUT tick is accepted.
    assert!(!xrc_metadata_meets_source_floor(2, icp_cfg.effective_min_xrc_sources(global)));
    assert!(xrc_metadata_meets_source_floor(2, xaut_cfg.effective_min_xrc_sources(global)));
}

#[test]
fn override_persists_across_collateral_config_serde_roundtrip() {
    // CollateralConfig is serialized to stable memory on pre_upgrade and
    // restored on post_upgrade. The override field must survive the
    // roundtrip; otherwise a backend upgrade silently reverts every
    // per-collateral floor to None.
    let original = mock_cfg(Some(2));
    let bytes = candid::encode_one(&original).expect("encode failed");
    let decoded: CollateralConfig = candid::decode_one(&bytes).expect("decode failed");
    assert_eq!(decoded.min_xrc_sources, Some(2));
    assert_eq!(decoded.effective_min_xrc_sources(3), 2);
}

#[test]
fn legacy_config_decodes_with_default_none() {
    // Wave-14a follow-up: the field is `#[serde(default)]` so an upgrade
    // from a snapshot that predates this PR must hydrate `min_xrc_sources`
    // as `None` (inherit global) rather than failing to decode.
    let legacy = mock_cfg(None);
    // We can't easily construct a "before-this-field-existed" payload
    // via candid::encode_one (which serializes the current schema), so
    // the canonical fence is the `#[serde(default)]` attribute in the
    // struct definition. This test acts as a behavior anchor: a default
    // `None` resolves to the global floor.
    assert_eq!(legacy.min_xrc_sources, None);
    assert_eq!(legacy.effective_min_xrc_sources(3), 3);
}
