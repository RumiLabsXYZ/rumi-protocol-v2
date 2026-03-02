# Dynamic Interest Rates Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a two-layer dynamic interest rate system where rates scale based on individual vault CR (Layer 1) and system-wide TCR during Recovery mode (Layer 2).

**Architecture:** Per-asset rate curves with admin-configurable markers define Layer 1 multipliers. Named-threshold recovery markers define Layer 2 multipliers. Both layers use piecewise linear interpolation with an `InterpolationMethod` enum for future curve types. Weighted averages of derived CR thresholds are cached on each price tick.

**Tech Stack:** Rust (IC canister), Candid IDL, rust_decimal for precise arithmetic, serde for serialization.

**Design doc:** `docs/plans/2026-03-01-dynamic-interest-rates-design.md`

---

### Task 1: Add new types to state.rs

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:1-55` (constants/imports area)
- Modify: `src/rumi_protocol_backend/src/state.rs:140-206` (after PriceSource, before/in CollateralConfig)

**Step 1: Add the new type definitions**

After the `PriceSource` enum (line ~150) and before `CollateralConfig` (line ~153), add:

```rust
/// How to interpolate between rate curve markers.
/// Linear for now; enum allows adding Exponential, Polynomial, etc. via upgrade.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum InterpolationMethod {
    Linear,
}

impl Default for InterpolationMethod {
    fn default() -> Self {
        InterpolationMethod::Linear
    }
}

/// A point on a rate curve: at this CR level, apply this multiplier to the base rate.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateMarker {
    pub cr_level: Ratio,
    pub multiplier: Ratio,
}

/// A per-asset rate curve: ordered markers + interpolation method.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateCurve {
    pub markers: Vec<RateMarker>,  // sorted by cr_level ascending
    pub method: InterpolationMethod,
}

/// Named system-wide thresholds for the recovery rate curve (Layer 2).
/// These resolve to debt-weighted averages at runtime.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum SystemThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
}

/// A recovery rate marker: at this named threshold, apply this multiplier.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RecoveryRateMarker {
    pub threshold: SystemThreshold,
    pub multiplier: Ratio,
}
```

**Step 2: Add default constants**

Near the existing constants (line ~41-52), add:

```rust
/// Default Layer 1 multipliers at each CR marker
pub const DEFAULT_RATE_MULTIPLIER_HEALTHY: Ratio = Ratio::new(dec!(1.0));
pub const DEFAULT_RATE_MULTIPLIER_WARNING: Ratio = Ratio::new(dec!(1.75));
pub const DEFAULT_RATE_MULTIPLIER_BORROW_THRESHOLD: Ratio = Ratio::new(dec!(2.5));
pub const DEFAULT_RATE_MULTIPLIER_LIQUIDATION: Ratio = Ratio::new(dec!(5.0));

/// Default Layer 2 (recovery) multipliers
pub const DEFAULT_RECOVERY_MULTIPLIER_HEALTHY: Ratio = Ratio::new(dec!(1.0));
pub const DEFAULT_RECOVERY_MULTIPLIER_WARNING: Ratio = Ratio::new(dec!(1.15));
pub const DEFAULT_RECOVERY_MULTIPLIER_BORROW_THRESHOLD: Ratio = Ratio::new(dec!(1.33));
pub const DEFAULT_RECOVERY_MULTIPLIER_LIQUIDATION: Ratio = Ratio::new(dec!(2.0));

/// Default healthy CR multiplier (healthy_cr = this * borrow_threshold_ratio)
pub const DEFAULT_HEALTHY_CR_MULTIPLIER: Ratio = Ratio::new(dec!(1.5));
```

**Step 3: Run cargo build to verify types compile**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Should compile (types aren't used anywhere yet)

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: add dynamic rate curve types and constants"
```

---

### Task 2: Add new fields to CollateralConfig and State

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:197-234` (CollateralConfig fields + PartialEq)
- Modify: `src/rumi_protocol_backend/src/state.rs:330-372` (State struct fields)
- Modify: `src/rumi_protocol_backend/src/state.rs:377-474` (State::from(InitArg) initializer)

**Step 1: Add fields to CollateralConfig**

After `recovery_interest_rate_apr` (line ~202) and before `display_color` (line ~204), add:

```rust
    /// Admin-configurable "healthy" CR. None = default (1.5 * borrow_threshold_ratio).
    /// Above this, the rate multiplier is 1.0x (base rate).
    #[serde(default)]
    pub healthy_cr: Option<Ratio>,
    /// Per-asset rate curve markers. None = use global defaults.
    #[serde(default)]
    pub rate_curve: Option<RateCurve>,
```

**Step 2: Update PartialEq for CollateralConfig**

In the `PartialEq` impl (line ~230-232), before the `display_color` comparison, add:

```rust
            && self.healthy_cr == other.healthy_cr
            && self.rate_curve == other.rate_curve
```

**Step 3: Add fields to State struct**

After `recovery_mode_threshold` (line ~360) and before `reserve_redemptions_enabled` (line ~363), add:

```rust
    /// Global default rate curve for Layer 1 (used when asset has no per-asset rate_curve).
    #[serde(default = "default_global_rate_curve")]
    pub global_rate_curve: RateCurve,
    /// Recovery mode rate curve for Layer 2 (system-wide, uses named thresholds).
    #[serde(default = "default_recovery_rate_curve")]
    pub recovery_rate_curve: Vec<RecoveryRateMarker>,
    /// Cached debt-weighted average of per-asset recovery CRs.
    #[serde(default)]
    pub weighted_avg_recovery_cr: Ratio,
    /// Cached debt-weighted average of per-asset warning CRs.
    #[serde(default)]
    pub weighted_avg_warning_cr: Ratio,
    /// Cached debt-weighted average of per-asset healthy CRs.
    #[serde(default)]
    pub weighted_avg_healthy_cr: Ratio,
```

**Step 4: Add serde default helper functions**

Near the other helper functions or just before the State struct, add:

```rust
fn default_global_rate_curve() -> RateCurve {
    RateCurve {
        markers: vec![
            RateMarker { cr_level: Ratio::new(dec!(0.0)), multiplier: DEFAULT_RATE_MULTIPLIER_LIQUIDATION },
            RateMarker { cr_level: Ratio::new(dec!(0.0)), multiplier: DEFAULT_RATE_MULTIPLIER_BORROW_THRESHOLD },
            RateMarker { cr_level: Ratio::new(dec!(0.0)), multiplier: DEFAULT_RATE_MULTIPLIER_WARNING },
            RateMarker { cr_level: Ratio::new(dec!(0.0)), multiplier: DEFAULT_RATE_MULTIPLIER_HEALTHY },
        ],
        method: InterpolationMethod::Linear,
    }
}

fn default_recovery_rate_curve() -> Vec<RecoveryRateMarker> {
    vec![
        RecoveryRateMarker { threshold: SystemThreshold::LiquidationRatio, multiplier: DEFAULT_RECOVERY_MULTIPLIER_LIQUIDATION },
        RecoveryRateMarker { threshold: SystemThreshold::BorrowThreshold, multiplier: DEFAULT_RECOVERY_MULTIPLIER_BORROW_THRESHOLD },
        RecoveryRateMarker { threshold: SystemThreshold::WarningCr, multiplier: DEFAULT_RECOVERY_MULTIPLIER_WARNING },
        RecoveryRateMarker { threshold: SystemThreshold::HealthyCr, multiplier: DEFAULT_RECOVERY_MULTIPLIER_HEALTHY },
    ]
}
```

Note: The global rate curve markers use placeholder `cr_level: 0.0` because the actual CR levels are derived per-asset from that asset's thresholds (liquidation_ratio, borrow_threshold_ratio, etc.). The multipliers are what matter in the global curve; CR levels are resolved at interpolation time. Alternatively, store only the multipliers in the global curve and resolve CR levels per-asset. Either approach works — pick whichever is cleaner during implementation.

**Step 5: Update State::from(InitArg) initializer**

In the `State::from(InitArg)` initializer (line ~377-474), after `recovery_mode_threshold: RECOVERY_COLLATERAL_RATIO,` (line ~428), add:

```rust
            global_rate_curve: default_global_rate_curve(),
            recovery_rate_curve: default_recovery_rate_curve(),
            weighted_avg_recovery_cr: RECOVERY_COLLATERAL_RATIO,
            weighted_avg_warning_cr: RECOVERY_COLLATERAL_RATIO,
            weighted_avg_healthy_cr: RECOVERY_COLLATERAL_RATIO,
```

And in the ICP CollateralConfig initializer (line ~440-469), add before `display_color`:

```rust
                    healthy_cr: None,
                    rate_curve: None,
```

**Step 6: Update all other CollateralConfig initializers**

In `src/rumi_protocol_backend/src/main.rs`, the `add_collateral_token` function (line ~1622-1646), add before `display_color`:

```rust
        healthy_cr: None,
        rate_curve: None,
```

In `src/rumi_protocol_backend/tests/tests.rs`, the `cketh_config()` function (line ~927-958), add before `display_color`:

```rust
            healthy_cr: None,
            rate_curve: None,
```

**Step 7: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile

**Step 8: Run cargo test**

Run: `cargo test --bin rumi_protocol_backend 2>&1 | tail -10`
Expected: All tests pass (Candid check will fail — that's expected; we'll fix the .did file in Task 6)

Note: The Candid interface check test will fail because we added new fields. This is expected and will be fixed in Task 6 when we update the .did file. Focus on compilation success here.

**Step 9: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/tests/tests.rs
git commit -m "feat: add rate curve fields to CollateralConfig and State"
```

---

### Task 3: Add derived value computation methods

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:814-818` (near get_recovery_target_cr_for)

**Step 1: Add per-asset derived value getters**

After `get_recovery_target_cr_for` (line ~818), add these methods to the `impl State` block:

```rust
    /// Compute the per-asset recovery CR: borrow_threshold_ratio + recovery_liquidation_buffer.
    /// This is the CR users need to retreat above during Recovery mode.
    pub fn get_recovery_cr_for(&self, ct: &CollateralType) -> Ratio {
        let borrow_threshold = self.get_min_collateral_ratio_for(ct);
        Ratio::from(borrow_threshold.0 + self.recovery_liquidation_buffer.0)
    }

    /// Compute the per-asset warning CR: 2 * recovery_cr - borrow_threshold.
    /// This is the derived midpoint where rates begin to increase.
    pub fn get_warning_cr_for(&self, ct: &CollateralType) -> Ratio {
        let borrow_threshold = self.get_min_collateral_ratio_for(ct);
        let recovery_cr = self.get_recovery_cr_for(ct);
        // warning_cr = 2 * recovery_cr - borrow_threshold
        Ratio::from(recovery_cr.0 * dec!(2) - borrow_threshold.0)
    }

    /// Get the healthy CR for an asset. Uses admin override if set,
    /// otherwise defaults to DEFAULT_HEALTHY_CR_MULTIPLIER * borrow_threshold_ratio.
    pub fn get_healthy_cr_for(&self, ct: &CollateralType) -> Ratio {
        if let Some(config) = self.collateral_configs.get(ct) {
            if let Some(healthy) = config.healthy_cr {
                return healthy;
            }
        }
        let borrow_threshold = self.get_min_collateral_ratio_for(ct);
        Ratio::from(borrow_threshold.0 * DEFAULT_HEALTHY_CR_MULTIPLIER.0)
    }
```

**Step 2: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -10`
Expected: Clean compile

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: add per-asset derived CR getters (recovery_cr, warning_cr, healthy_cr)"
```

---

### Task 4: Cache weighted averages on price tick

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:652-676` (compute_dynamic_recovery_threshold)
- Modify: `src/rumi_protocol_backend/src/state.rs:934-960` (update_total_collateral_ratio_and_mode)

**Step 1: Add compute_weighted_cr_averages method**

After `compute_dynamic_recovery_threshold` (line ~676), add:

```rust
    /// Compute debt-weighted averages of recovery_cr, warning_cr, and healthy_cr
    /// across all collateral types. Used by Layer 2 recovery rate curve.
    /// Returns (weighted_recovery_cr, weighted_warning_cr, weighted_healthy_cr).
    pub fn compute_weighted_cr_averages(&self) -> (Ratio, Ratio, Ratio) {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            // Fallback: use ICP defaults when no debt exists
            let recovery_cr = Ratio::from(RECOVERY_COLLATERAL_RATIO.0 + self.recovery_liquidation_buffer.0);
            let warning_cr = Ratio::from(recovery_cr.0 * dec!(2) - RECOVERY_COLLATERAL_RATIO.0);
            let healthy_cr = Ratio::from(RECOVERY_COLLATERAL_RATIO.0 * DEFAULT_HEALTHY_CR_MULTIPLIER.0);
            return (recovery_cr, warning_cr, healthy_cr);
        }
        let total_debt_dec = Decimal::from_u64(total_debt.to_u64())
            .unwrap_or(Decimal::ZERO);

        let mut recovery_sum = Decimal::ZERO;
        let mut warning_sum = Decimal::ZERO;
        let mut healthy_sum = Decimal::ZERO;

        for (ct, _config) in &self.collateral_configs {
            let debt_i = self.total_debt_for_collateral(ct);
            if debt_i == ICUSD::new(0) {
                continue;
            }
            let weight = Decimal::from_u64(debt_i.to_u64())
                .unwrap_or(Decimal::ZERO) / total_debt_dec;

            let recovery_cr = self.get_recovery_cr_for(ct);
            let warning_cr = self.get_warning_cr_for(ct);
            let healthy_cr = self.get_healthy_cr_for(ct);

            recovery_sum += weight * recovery_cr.0;
            warning_sum += weight * warning_cr.0;
            healthy_sum += weight * healthy_cr.0;
        }

        (
            Ratio::from(recovery_sum),
            Ratio::from(warning_sum),
            Ratio::from(healthy_sum),
        )
    }
```

**Step 2: Update update_total_collateral_ratio_and_mode**

In `update_total_collateral_ratio_and_mode` (line ~934), after `self.recovery_mode_threshold = dynamic_threshold;` (line ~941), add:

```rust
        // Cache weighted averages of derived CR thresholds for Layer 2 rate computation
        let (avg_recovery, avg_warning, avg_healthy) = self.compute_weighted_cr_averages();
        self.weighted_avg_recovery_cr = avg_recovery;
        self.weighted_avg_warning_cr = avg_warning;
        self.weighted_avg_healthy_cr = avg_healthy;
```

**Step 3: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -10`
Expected: Clean compile

**Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: cache weighted CR averages on price tick"
```

---

### Task 5: Implement rate interpolation and get_dynamic_interest_rate_for

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:755-765` (near get_interest_rate_for)

**Step 1: Add the linear interpolation helper**

Add as a free function near the top of state.rs, or as an associated function on RateCurve:

```rust
/// Linearly interpolate a multiplier from a sorted list of (cr_level, multiplier) pairs.
/// - If vault_cr >= highest marker's cr_level: return that marker's multiplier (e.g., 1.0x at healthy).
/// - If vault_cr <= lowest marker's cr_level: return that marker's multiplier (e.g., 5.0x at liquidation).
/// - Otherwise: find the two surrounding markers and interpolate.
/// Markers MUST be sorted by cr_level ascending.
fn interpolate_multiplier(markers: &[(Ratio, Ratio)], vault_cr: Ratio) -> Ratio {
    if markers.is_empty() {
        return Ratio::new(dec!(1.0));
    }
    if markers.len() == 1 {
        return markers[0].1;
    }
    // Below lowest marker
    if vault_cr.0 <= markers[0].0 {
        return markers[0].1;
    }
    // Above highest marker
    if vault_cr.0 >= markers[markers.len() - 1].0 {
        return markers[markers.len() - 1].1;
    }
    // Find surrounding markers
    for i in 0..markers.len() - 1 {
        let (cr_lo, mult_lo) = markers[i];
        let (cr_hi, mult_hi) = markers[i + 1];
        if vault_cr.0 >= cr_lo.0 && vault_cr.0 <= cr_hi.0 {
            let range = cr_hi.0 - cr_lo.0;
            if range == Decimal::ZERO {
                return mult_lo;
            }
            let position = (vault_cr.0 - cr_lo.0) / range;
            let multiplier = mult_lo.0 + position * (mult_hi.0 - mult_lo.0);
            return Ratio::from(multiplier);
        }
    }
    // Fallback (shouldn't reach here with valid sorted markers)
    Ratio::new(dec!(1.0))
}
```

**Step 2: Add resolve_rate_curve_markers helper on State**

This resolves a rate curve's markers to concrete (cr_level, multiplier) pairs for a given collateral type. If the asset has a per-asset curve with explicit CR levels, use those. Otherwise, build markers from the asset's derived thresholds using the global curve's multipliers.

```rust
    /// Resolve Layer 1 rate curve markers for a collateral type.
    /// Per-asset curve uses its own explicit markers.
    /// Global curve maps multipliers to this asset's derived thresholds.
    fn resolve_layer1_markers(&self, ct: &CollateralType) -> Vec<(Ratio, Ratio)> {
        // Check for per-asset curve first
        if let Some(config) = self.collateral_configs.get(ct) {
            if let Some(ref curve) = config.rate_curve {
                return curve.markers.iter()
                    .map(|m| (m.cr_level, m.multiplier))
                    .collect();
            }
        }
        // Use global curve: map default multipliers to this asset's thresholds
        let liq = self.get_liquidation_ratio_for(ct);
        let borrow = self.get_min_collateral_ratio_for(ct);
        let warning = self.get_warning_cr_for(ct);
        let healthy = self.get_healthy_cr_for(ct);

        vec![
            (liq, self.global_rate_curve.markers.get(0).map(|m| m.multiplier).unwrap_or(DEFAULT_RATE_MULTIPLIER_LIQUIDATION)),
            (borrow, self.global_rate_curve.markers.get(1).map(|m| m.multiplier).unwrap_or(DEFAULT_RATE_MULTIPLIER_BORROW_THRESHOLD)),
            (warning, self.global_rate_curve.markers.get(2).map(|m| m.multiplier).unwrap_or(DEFAULT_RATE_MULTIPLIER_WARNING)),
            (healthy, self.global_rate_curve.markers.get(3).map(|m| m.multiplier).unwrap_or(DEFAULT_RATE_MULTIPLIER_HEALTHY)),
        ]
    }

    /// Resolve Layer 2 recovery rate curve markers to concrete CR values
    /// using cached weighted averages.
    fn resolve_layer2_markers(&self) -> Vec<(Ratio, Ratio)> {
        let mut markers: Vec<(Ratio, Ratio)> = self.recovery_rate_curve.iter().map(|m| {
            let cr = match m.threshold {
                SystemThreshold::LiquidationRatio => {
                    // Use weighted avg of liquidation ratios (approximate: use recovery threshold - buffer - typical gap)
                    // Simpler: iterate collateral types for weighted avg liquidation ratio
                    self.compute_weighted_liquidation_ratio()
                }
                SystemThreshold::BorrowThreshold => self.recovery_mode_threshold,
                SystemThreshold::WarningCr => self.weighted_avg_warning_cr,
                SystemThreshold::HealthyCr => self.weighted_avg_healthy_cr,
            };
            (cr, m.multiplier)
        }).collect();
        markers.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        markers
    }
```

**Step 3: Add compute_weighted_liquidation_ratio helper**

```rust
    /// Compute debt-weighted average of per-asset liquidation ratios.
    fn compute_weighted_liquidation_ratio(&self) -> Ratio {
        let total_debt = self.total_borrowed_icusd_amount();
        if total_debt == ICUSD::new(0) {
            return MINIMUM_COLLATERAL_RATIO;
        }
        let total_debt_dec = Decimal::from_u64(total_debt.to_u64())
            .unwrap_or(Decimal::ZERO);
        let mut weighted_sum = Decimal::ZERO;
        for (ct, config) in &self.collateral_configs {
            let debt_i = self.total_debt_for_collateral(ct);
            if debt_i == ICUSD::new(0) {
                continue;
            }
            let weight = Decimal::from_u64(debt_i.to_u64())
                .unwrap_or(Decimal::ZERO) / total_debt_dec;
            weighted_sum += weight * config.liquidation_ratio.0;
        }
        if weighted_sum == Decimal::ZERO {
            return MINIMUM_COLLATERAL_RATIO;
        }
        Ratio::from(weighted_sum)
    }
```

**Step 4: Add get_dynamic_interest_rate_for**

After the existing `get_interest_rate_for` method (line ~765), add:

```rust
    /// Get the dynamic interest rate for a vault based on its CR.
    /// Layer 1: per-asset rate curve multiplier based on vault_cr.
    /// Layer 2 (Recovery only): system-wide multiplier based on TCR.
    /// Static override escape valve: if recovery_interest_rate_apr is set
    /// and we're in Recovery, return it directly.
    pub fn get_dynamic_interest_rate_for(&self, ct: &CollateralType, vault_cr: Ratio) -> Ratio {
        let config = self.collateral_configs.get(ct);

        // Escape valve: static recovery override takes precedence
        if self.mode == Mode::Recovery {
            if let Some(override_rate) = config.and_then(|c| c.recovery_interest_rate_apr) {
                return override_rate;
            }
        }

        // Base rate
        let base_rate = config
            .map(|c| c.interest_rate_apr)
            .unwrap_or(DEFAULT_INTEREST_RATE_APR);

        // Layer 1: per-asset vault CR multiplier
        let layer1_markers = self.resolve_layer1_markers(ct);
        let layer1_multiplier = interpolate_multiplier(&layer1_markers, vault_cr);
        let layer1_rate = Ratio::from(base_rate.0 * layer1_multiplier.0);

        // Layer 2: system-wide recovery multiplier (only in Recovery mode)
        if self.mode == Mode::Recovery {
            let layer2_markers = self.resolve_layer2_markers();
            let layer2_multiplier = interpolate_multiplier(&layer2_markers, self.total_collateral_ratio);
            return Ratio::from(layer1_rate.0 * layer2_multiplier.0);
        }

        layer1_rate
    }
```

**Step 5: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: implement rate interpolation and get_dynamic_interest_rate_for"
```

---

### Task 6: Add unit tests for interpolation and dynamic rates

**Files:**
- Modify: `src/rumi_protocol_backend/tests/tests.rs`

**Step 1: Add interpolation unit tests**

Add tests to the existing test module. These test the core math:

```rust
#[test]
fn test_interpolate_at_boundaries() {
    // Test: vault at exactly healthy_cr gets 1.0x
    // Test: vault at exactly liquidation_ratio gets 5.0x
    // Test: vault below liquidation_ratio gets clamped to 5.0x
    // Test: vault above healthy_cr gets clamped to 1.0x
}

#[test]
fn test_interpolate_midpoint() {
    // ICP: liq=133%, borrow=150%, warning=160%, healthy=225%
    // Vault at 155% (midpoint of borrow-warning): multiplier = 2.5 - 0.5*(2.5-1.75) = 2.125
}

#[test]
fn test_dynamic_rate_normal_mode() {
    // Set up state with ICP, base APR = 2%
    // Vault at 225% (healthy) → rate = 2% * 1.0 = 2%
    // Vault at 155% → rate = 2% * 2.125 = 4.25%
    // Vault at 133% (liq) → rate = 2% * 5.0 = 10%
}

#[test]
fn test_dynamic_rate_recovery_mode() {
    // Set up state in Recovery mode
    // Layer 1 + Layer 2 should compound
}

#[test]
fn test_static_override_takes_precedence() {
    // Set recovery_interest_rate_apr on config
    // In Recovery mode, should return the override, not dynamic
}
```

Note: Exact test implementations depend on test helper availability. Use existing patterns from `tests.rs` (e.g., `cketh_config()`, `register_cketh()`) as templates. The interpolation helper function should be made `pub(crate)` or tested through `get_dynamic_interest_rate_for`.

**Step 2: Run tests**

Run: `cargo test --lib -p rumi_protocol_backend 2>&1 | tail -20`
Expected: All new tests pass

**Step 3: Commit**

```bash
git add src/rumi_protocol_backend/tests/tests.rs
git commit -m "test: add unit tests for rate interpolation and dynamic rates"
```

---

### Task 7: Add event variants and recording helpers

**Files:**
- Modify: `src/rumi_protocol_backend/src/event.rs`

**Step 1: Add three new Event variants**

In the `Event` enum, after the `SetRecoveryParameters` variant, add:

```rust
    #[serde(rename = "set_rate_curve_markers")]
    SetRateCurveMarkers {
        /// None for global, Some(principal.to_string()) for per-asset
        collateral_type: Option<String>,
        markers: String,  // JSON-serialized marker pairs
    },

    #[serde(rename = "set_recovery_rate_curve")]
    SetRecoveryRateCurve {
        markers: String,  // JSON-serialized (threshold, multiplier) pairs
    },

    #[serde(rename = "set_healthy_cr")]
    SetHealthyCr {
        collateral_type: String,
        healthy_cr: Option<String>,
    },
```

**Step 2: Add is_vault_related match arms**

In the `is_vault_related` method, add:

```rust
            Event::SetRateCurveMarkers { .. } => false,
            Event::SetRecoveryRateCurve { .. } => false,
            Event::SetHealthyCr { .. } => false,
```

**Step 3: Add replay cases**

In the `replay` function, add match arms for each new variant. Follow the same pattern as `SetRecoveryParameters`:

- `SetRateCurveMarkers`: Parse markers JSON back to `Vec<RateMarker>`, update `global_rate_curve` or per-asset `rate_curve`.
- `SetRecoveryRateCurve`: Parse markers JSON back to `Vec<RecoveryRateMarker>`, update `recovery_rate_curve`.
- `SetHealthyCr`: Parse healthy_cr string back to `Option<Ratio>`, update per-asset config.

**Step 4: Add recording helpers**

```rust
pub fn record_set_rate_curve_markers(
    state: &mut State,
    collateral_type: Option<CollateralType>,
    markers: Vec<RateMarker>,
) {
    let markers_json = serde_json::to_string(&markers).unwrap_or_default();
    record_event(&Event::SetRateCurveMarkers {
        collateral_type: collateral_type.map(|ct| ct.to_string()),
        markers: markers_json,
    });
    let curve = RateCurve {
        markers,
        method: InterpolationMethod::Linear,
    };
    match collateral_type {
        None => { state.global_rate_curve = curve; }
        Some(ct) => {
            if let Some(config) = state.collateral_configs.get_mut(&ct) {
                config.rate_curve = Some(curve);
            }
        }
    }
}

pub fn record_set_recovery_rate_curve(
    state: &mut State,
    markers: Vec<RecoveryRateMarker>,
) {
    let markers_json = serde_json::to_string(&markers).unwrap_or_default();
    record_event(&Event::SetRecoveryRateCurve {
        markers: markers_json,
    });
    state.recovery_rate_curve = markers;
}

pub fn record_set_healthy_cr(
    state: &mut State,
    collateral_type: CollateralType,
    healthy_cr: Option<Ratio>,
) {
    record_event(&Event::SetHealthyCr {
        collateral_type: collateral_type.to_string(),
        healthy_cr: healthy_cr.map(|r| r.0.to_string()),
    });
    if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
        config.healthy_cr = healthy_cr;
    }
}
```

Note: You'll need to add imports for the new types at the top of event.rs: `use crate::state::{RateMarker, RateCurve, RecoveryRateMarker, InterpolationMethod, SystemThreshold};`

**Step 5: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs
git commit -m "feat: add event variants and recording helpers for rate curves"
```

---

### Task 8: Add admin functions to main.rs

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`

**Step 1: Add set_rate_curve_markers admin function**

After the `set_recovery_parameters` function (line ~1504), add:

```rust
#[candid_method(update)]
#[update]
async fn set_rate_curve_markers(
    collateral_type: Option<Principal>,
    markers: Vec<(f64, f64)>,  // (cr_level, multiplier) pairs
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set rate curve markers".to_string(),
        ));
    }
    // Validate collateral type if specified
    if let Some(ct) = collateral_type {
        let exists = read_state(|s| s.collateral_configs.contains_key(&ct));
        if !exists {
            return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
        }
    }
    // Validate markers
    if markers.len() < 2 {
        return Err(ProtocolError::GenericError("At least 2 markers required".to_string()));
    }
    for (i, (cr, mult)) in markers.iter().enumerate() {
        if *mult <= 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Marker {} multiplier must be > 0", i),
            ));
        }
        if *cr < 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Marker {} CR level must be >= 0", i),
            ));
        }
        if i > 0 && *cr <= markers[i - 1].0 {
            return Err(ProtocolError::GenericError(
                "Markers must be sorted by cr_level ascending".to_string(),
            ));
        }
    }
    use rumi_protocol_backend::state::RateMarker;
    use rumi_protocol_backend::numeric::Ratio;
    let rate_markers: Vec<RateMarker> = markers.iter().map(|(cr, mult)| {
        RateMarker {
            cr_level: Ratio::from_f64(*cr),
            multiplier: Ratio::from_f64(*mult),
        }
    }).collect();
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_rate_curve_markers(s, collateral_type, rate_markers);
    });
    log!(INFO, "[set_rate_curve_markers] collateral={:?}, {} markers set", collateral_type, markers.len());
    Ok(())
}
```

**Step 2: Add set_recovery_rate_curve admin function**

```rust
#[candid_method(update)]
#[update]
async fn set_recovery_rate_curve(
    markers: Vec<(String, f64)>,  // (threshold_name, multiplier) pairs
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set recovery rate curve".to_string(),
        ));
    }
    if markers.len() < 2 {
        return Err(ProtocolError::GenericError("At least 2 markers required".to_string()));
    }
    use rumi_protocol_backend::state::{SystemThreshold, RecoveryRateMarker};
    use rumi_protocol_backend::numeric::Ratio;
    let mut recovery_markers = Vec::new();
    for (name, mult) in &markers {
        if *mult <= 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Multiplier for {} must be > 0", name),
            ));
        }
        let threshold = match name.as_str() {
            "LiquidationRatio" => SystemThreshold::LiquidationRatio,
            "BorrowThreshold" => SystemThreshold::BorrowThreshold,
            "WarningCr" => SystemThreshold::WarningCr,
            "HealthyCr" => SystemThreshold::HealthyCr,
            _ => return Err(ProtocolError::GenericError(
                format!("Unknown threshold name: {}. Use LiquidationRatio, BorrowThreshold, WarningCr, or HealthyCr", name),
            )),
        };
        recovery_markers.push(RecoveryRateMarker { threshold, multiplier: Ratio::from_f64(*mult) });
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_rate_curve(s, recovery_markers);
    });
    log!(INFO, "[set_recovery_rate_curve] {} markers set", markers.len());
    Ok(())
}
```

**Step 3: Add set_healthy_cr admin function**

```rust
#[candid_method(update)]
#[update]
async fn set_healthy_cr(
    collateral_type: Principal,
    healthy_cr: Option<f64>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set healthy CR".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    if let Some(cr) = healthy_cr {
        let borrow_threshold = read_state(|s| s.get_min_collateral_ratio_for(&collateral_type).to_f64());
        if cr <= borrow_threshold {
            return Err(ProtocolError::GenericError(
                format!("healthy_cr ({}) must be above borrow_threshold_ratio ({})", cr, borrow_threshold),
            ));
        }
    }
    use rumi_protocol_backend::numeric::Ratio;
    let ratio = healthy_cr.map(Ratio::from_f64);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_healthy_cr(s, collateral_type, ratio);
    });
    log!(INFO, "[set_healthy_cr] collateral={}, healthy_cr={:?}", collateral_type, healthy_cr);
    Ok(())
}
```

**Step 4: Add get_vault_interest_rate query**

```rust
#[candid_method(query)]
#[query]
fn get_vault_interest_rate(vault_id: u64) -> Result<f64, ProtocolError> {
    read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or(ProtocolError::VaultNotFound(vault_id))?;
        let config = s.get_collateral_config(&vault.collateral_type)
            .ok_or(ProtocolError::GenericError("Unknown collateral type".to_string()))?;
        let price = config.last_price
            .ok_or(ProtocolError::GenericError("No price available".to_string()))?;
        let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
        let vault_cr = crate::compute_collateral_ratio(
            vault.collateral_amount,
            vault.borrowed_icusd_amount,
            price_dec,
            config.decimals,
        );
        let rate = s.get_dynamic_interest_rate_for(&vault.collateral_type, vault_cr);
        Ok(rate.to_f64())
    })
}
```

Note: You'll need to add `use rust_decimal::prelude::FromPrimitive;` and `use rust_decimal::Decimal;` at the top of main.rs if not already imported.

**Step 5: Run cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | head -20`
Expected: Clean compile

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs
git commit -m "feat: add admin functions for rate curves and vault rate query"
```

---

### Task 9: Update .did file

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Step 1: Add new Candid types**

Add to the .did file:

```candid
type InterpolationMethod = variant { Linear };

type RateMarker = record {
  cr_level : blob;
  multiplier : blob;
};

type RateCurve = record {
  markers : vec RateMarker;
  method : InterpolationMethod;
};

type SystemThreshold = variant {
  LiquidationRatio;
  BorrowThreshold;
  WarningCr;
  HealthyCr;
};

type RecoveryRateMarker = record {
  threshold : SystemThreshold;
  multiplier : blob;
};
```

**Step 2: Update CollateralConfig type**

Add to the CollateralConfig record:

```candid
  healthy_cr : opt blob;
  rate_curve : opt RateCurve;
```

**Step 3: Add new Event variants**

In the Event variant type, add:

```candid
  set_rate_curve_markers : record { collateral_type : opt text; markers : text; };
  set_recovery_rate_curve : record { markers : text; };
  set_healthy_cr : record { collateral_type : text; healthy_cr : opt text; };
```

**Step 4: Add new service methods**

```candid
  set_rate_curve_markers : (opt principal, vec record { float64; float64 }) -> (Result);
  set_recovery_rate_curve : (vec record { text; float64 }) -> (Result);
  set_healthy_cr : (principal, opt float64) -> (Result);
  get_vault_interest_rate : (nat64) -> (Result_2) query;
```

Note: `Result_2` should be the existing `variant { Ok : float64; Err : ProtocolError }` type — check if it already exists in the .did file for similar query patterns, or define it.

**Step 5: Run the Candid interface check**

Run: `cargo test --bin rumi_protocol_backend check_candid 2>&1 | tail -20`
Expected: PASS — the generated interface should match the .did file

**Step 6: Commit**

```bash
git add src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat: update .did file with rate curve types and admin functions"
```

---

### Task 10: Full build and test verification

**Files:** None (verification only)

**Step 1: Full cargo build**

Run: `cargo build -p rumi_protocol_backend 2>&1 | tail -5`
Expected: Clean compile, no warnings

**Step 2: Run all unit tests**

Run: `cargo test --bin rumi_protocol_backend 2>&1 | tail -20`
Expected: All tests pass (including Candid interface check)

**Step 3: Run lib tests**

Run: `cargo test --lib -p rumi_protocol_backend 2>&1 | tail -10`
Expected: All tests pass

**Step 4: Build frontend**

Run: `cd src/vault_frontend && npm run build 2>&1 | tail -5`
Expected: Clean build (no frontend changes in this plan, but verify nothing broke)

**Step 5: Commit any fixes if needed**

If any tests fail, fix and commit with descriptive message.

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Add new types (InterpolationMethod, RateMarker, RateCurve, SystemThreshold, RecoveryRateMarker) + constants | state.rs |
| 2 | Add fields to CollateralConfig and State, update all initializers | state.rs, main.rs, tests.rs |
| 3 | Add derived value getters (recovery_cr, warning_cr, healthy_cr) | state.rs |
| 4 | Cache weighted averages on price tick | state.rs |
| 5 | Implement interpolation + get_dynamic_interest_rate_for | state.rs |
| 6 | Add unit tests for interpolation and dynamic rates | tests.rs |
| 7 | Add event variants and recording helpers | event.rs |
| 8 | Add admin functions (set_rate_curve_markers, set_recovery_rate_curve, set_healthy_cr, get_vault_interest_rate) | main.rs |
| 9 | Update .did file with new types and methods | .did |
| 10 | Full build and test verification | — |
