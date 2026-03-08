# Dynamic Borrowing Fee Multiplier Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a piecewise-linear fee multiplier curve that scales the base borrowing fee based on the borrower's projected vault CR relative to the system-wide TCR.

**Architecture:** New `CrAnchor` enum unifies concrete CRs and named dynamic anchors (TCR, system thresholds). A `borrowing_fee_curve` on `ProtocolState` maps projected-CR → multiplier. The multiplier scales the existing per-collateral `borrowing_fee` at borrow time. Frontend gets pre-resolved curve points via `ProtocolStatus` for real-time fee preview.

**Tech Stack:** Rust (IC canister backend), Svelte/TypeScript (frontend), rust_decimal, Candid

**Design doc:** `docs/plans/2026-03-08-dynamic-borrowing-fee-design.md`

---

### Task 1: Add CrAnchor, AssetThreshold enums and TotalCollateralRatio variant

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:189-218`

**Step 1: Add `AssetThreshold` and `CrAnchor` enums**

After the existing `InterpolationMethod` impl block (line 187) and before `RateMarker` (line 189), add:

```rust
/// Named per-asset CR thresholds, resolved from CollateralConfig at runtime.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum AssetThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
}

/// Anchor point for a rate curve marker. Can be a fixed CR or a dynamic reference.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub enum CrAnchor {
    /// Concrete CR value (e.g., 1.5 = 150%).
    Fixed(Ratio),
    /// Per-asset threshold, resolved from CollateralConfig at runtime.
    AssetThreshold(AssetThreshold),
    /// System-wide threshold, resolved from debt-weighted averages at runtime.
    SystemThreshold(SystemThreshold),
    /// Midpoint of two anchors: (A + B) / 2.
    Midpoint(Box<CrAnchor>, Box<CrAnchor>),
    /// Offset from another anchor: base + delta (delta can be negative).
    Offset(Box<CrAnchor>, Ratio),
}
```

**Step 2: Add `TotalCollateralRatio` to `SystemThreshold`**

At `state.rs:206-211`, add the new variant:

```rust
pub enum SystemThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
    TotalCollateralRatio,  // NEW: actual system-wide CR
}
```

**Important:** This changes the `SystemThreshold` enum which is serialized in `recovery_rate_curve`. The new variant will only be used in the new `borrowing_fee_curve`, so existing serialized state won't include it. Deserialization is fine because Candid/serde handle new enum variants gracefully (they'd only fail if old data contained the new variant, which it won't).

**Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend`
Expected: Compiler warnings about the new variant not being matched in `resolve_layer2_markers` and `record_set_recovery_rate_curve`. That's fine — we'll fix those in Task 3.

**Step 4: Fix exhaustive match in `resolve_layer2_markers`**

At `state.rs:1118-1122`, add the new arm:

```rust
let cr = match m.threshold {
    SystemThreshold::LiquidationRatio => self.compute_weighted_liquidation_ratio(),
    SystemThreshold::BorrowThreshold => self.recovery_mode_threshold,
    SystemThreshold::WarningCr => self.weighted_avg_warning_cr,
    SystemThreshold::HealthyCr => self.weighted_avg_healthy_cr,
    SystemThreshold::TotalCollateralRatio => self.total_collateral_ratio,
};
```

**Step 5: Fix exhaustive match in `event.rs`**

At `src/rumi_protocol_backend/src/event.rs:1311-1315`, add the new arm in `record_set_recovery_rate_curve`:

```rust
let thresh_str = match thresh {
    SystemThreshold::LiquidationRatio => "LiquidationRatio",
    SystemThreshold::BorrowThreshold => "BorrowThreshold",
    SystemThreshold::WarningCr => "WarningCr",
    SystemThreshold::HealthyCr => "HealthyCr",
    SystemThreshold::TotalCollateralRatio => "TotalCollateralRatio",
};
```

And in `main.rs:1928-1932` (`set_recovery_rate_curve` parsing), add:

```rust
"TotalCollateralRatio" => SystemThreshold::TotalCollateralRatio,
```

**Step 6: Verify it compiles cleanly**

Run: `cargo check -p rumi_protocol_backend`
Expected: PASS with no errors.

**Step 7: Commit**

```
feat(backend): add CrAnchor enum, AssetThreshold, TotalCollateralRatio variant
```

---

### Task 2: Add `resolve_anchor()` and `resolve_curve()` methods

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (add methods to `impl State`)

**Step 1: Write unit tests for resolve_anchor**

Add to the `#[cfg(test)] mod tests` block at line 2245:

```rust
#[test]
fn test_resolve_anchor_fixed() {
    let state = accrual_test_state();
    let anchor = CrAnchor::Fixed(Ratio::from_f64(1.75));
    let result = state.resolve_anchor(&anchor, None);
    assert!((result.to_f64() - 1.75).abs() < 0.001);
}

#[test]
fn test_resolve_anchor_system_threshold_tcr() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(1.85);
    let anchor = CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio);
    let result = state.resolve_anchor(&anchor, None);
    assert!((result.to_f64() - 1.85).abs() < 0.001);
}

#[test]
fn test_resolve_anchor_midpoint() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(2.0);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);
    let anchor = CrAnchor::Midpoint(
        Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
        Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
    );
    let result = state.resolve_anchor(&anchor, None);
    assert!((result.to_f64() - 1.75).abs() < 0.001,
        "Midpoint of 1.5 and 2.0 should be 1.75, got {}", result.to_f64());
}

#[test]
fn test_resolve_anchor_offset() {
    let mut state = accrual_test_state();
    state.recovery_mode_threshold = Ratio::from_f64(1.5);
    let anchor = CrAnchor::Offset(
        Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
        Ratio::from_f64(0.05),
    );
    let result = state.resolve_anchor(&anchor, None);
    assert!((result.to_f64() - 1.55).abs() < 0.001,
        "1.5 + 0.05 should be 1.55, got {}", result.to_f64());
}

#[test]
fn test_resolve_anchor_asset_threshold() {
    let state = accrual_test_state();
    let icp = state.icp_collateral_type();
    let anchor = CrAnchor::AssetThreshold(AssetThreshold::BorrowThreshold);
    let result = state.resolve_anchor(&anchor, Some(&icp));
    // ICP borrow threshold is 1.5 (RECOVERY_COLLATERAL_RATIO)
    assert!((result.to_f64() - 1.5).abs() < 0.01,
        "ICP borrow threshold should be ~1.5, got {}", result.to_f64());
}

#[test]
fn test_resolve_curve_sorts_by_cr() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(2.0);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);
    let curve = RateCurve {
        markers: vec![
            // Intentionally out of order
            RateMarker {
                cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
                multiplier: Ratio::from_f64(1.0),
            },
            RateMarker {
                cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold),
                multiplier: Ratio::from_f64(3.0),
            },
        ],
        method: InterpolationMethod::Linear,
    };
    let resolved = state.resolve_curve(&curve, None);
    assert!(resolved[0].0.to_f64() < resolved[1].0.to_f64(),
        "Should be sorted ascending: {} < {}", resolved[0].0.to_f64(), resolved[1].0.to_f64());
    assert!((resolved[0].0.to_f64() - 1.5).abs() < 0.01);
    assert!((resolved[1].0.to_f64() - 2.0).abs() < 0.01);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend --lib -- test_resolve_anchor`
Expected: FAIL — `resolve_anchor` and `resolve_curve` don't exist yet.

**Step 3: Implement `resolve_anchor` and `resolve_curve`**

Add to `impl State` block (near the existing `resolve_layer1_markers` at line 1079):

```rust
/// Resolve a CrAnchor to a concrete Ratio.
/// `asset_context` is required for AssetThreshold anchors; pass None for system-wide curves.
pub fn resolve_anchor(
    &self,
    anchor: &CrAnchor,
    asset_context: Option<&CollateralType>,
) -> Ratio {
    match anchor {
        CrAnchor::Fixed(r) => *r,
        CrAnchor::AssetThreshold(t) => {
            let ct = asset_context.expect("AssetThreshold requires asset context");
            match t {
                AssetThreshold::LiquidationRatio => self.get_liquidation_ratio_for(ct),
                AssetThreshold::BorrowThreshold => {
                    self.collateral_configs.get(ct)
                        .map(|c| c.borrow_threshold_ratio)
                        .unwrap_or(RECOVERY_COLLATERAL_RATIO)
                }
                AssetThreshold::WarningCr => self.get_warning_cr_for(ct),
                AssetThreshold::HealthyCr => self.get_healthy_cr_for(ct),
            }
        }
        CrAnchor::SystemThreshold(t) => match t {
            SystemThreshold::LiquidationRatio => self.compute_weighted_liquidation_ratio(),
            SystemThreshold::BorrowThreshold => self.recovery_mode_threshold,
            SystemThreshold::WarningCr => self.weighted_avg_warning_cr,
            SystemThreshold::HealthyCr => self.weighted_avg_healthy_cr,
            SystemThreshold::TotalCollateralRatio => self.total_collateral_ratio,
        },
        CrAnchor::Midpoint(a, b) => {
            let va = self.resolve_anchor(a, asset_context);
            let vb = self.resolve_anchor(b, asset_context);
            Ratio::from((va.0 + vb.0) / dec!(2))
        }
        CrAnchor::Offset(base, delta) => {
            let v = self.resolve_anchor(base, asset_context);
            Ratio::from(v.0 + delta.0)
        }
    }
}

/// Resolve all markers in a RateCurve to concrete (cr_level, multiplier) pairs.
pub fn resolve_curve(
    &self,
    curve: &RateCurve,
    asset_context: Option<&CollateralType>,
) -> Vec<(Ratio, Ratio)> {
    let mut resolved: Vec<(Ratio, Ratio)> = curve.markers.iter()
        .map(|m| (self.resolve_anchor(&m.cr_anchor, asset_context), m.multiplier))
        .collect();
    resolved.sort_by(|a, b| a.0.0.cmp(&b.0.0));
    resolved
}
```

**Note:** This requires `RateMarker` to have `cr_anchor: CrAnchor` instead of `cr_level: Ratio`. But we can't change that yet without breaking existing serialized state. For now, the `RateMarker` struct keeps `cr_level: Ratio` — `resolve_curve` only works with new curves that use `CrAnchor`. We'll handle the RateMarker migration in Task 6 (v2 fields). For this task, add `resolve_curve` as a method that takes a separate new-style curve type.

**Actually, better approach:** Create a parallel marker type for v2 curves:

```rust
/// A rate curve marker using dynamic CrAnchor (v2).
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateMarkerV2 {
    pub cr_anchor: CrAnchor,
    pub multiplier: Ratio,
}

/// A rate curve using dynamic anchors (v2).
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RateCurveV2 {
    pub markers: Vec<RateMarkerV2>,
    pub method: InterpolationMethod,
}
```

Then `resolve_curve` operates on `RateCurveV2`:

```rust
pub fn resolve_curve(
    &self,
    curve: &RateCurveV2,
    asset_context: Option<&CollateralType>,
) -> Vec<(Ratio, Ratio)> {
    let mut resolved: Vec<(Ratio, Ratio)> = curve.markers.iter()
        .map(|m| (self.resolve_anchor(&m.cr_anchor, asset_context), m.multiplier))
        .collect();
    resolved.sort_by(|a, b| a.0.0.cmp(&b.0.0));
    resolved
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p rumi_protocol_backend --lib -- test_resolve_anchor test_resolve_curve`
Expected: All 6 tests PASS.

**Step 5: Commit**

```
feat(backend): add resolve_anchor/resolve_curve with RateCurveV2 types
```

---

### Task 3: Add `borrowing_fee_curve` field to ProtocolState

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:459-469` (State struct)
- Modify: `src/rumi_protocol_backend/src/state.rs:600-615` (Default impl)

**Step 1: Add field to State struct**

After line 469 (`weighted_avg_healthy_cr`), add:

```rust
/// Dynamic borrowing fee multiplier curve (v2).
/// X-axis: projected vault CR after borrow. Y-axis: multiplier on base borrowing_fee.
/// None = flat fee (no dynamic multiplier).
#[serde(default)]
pub borrowing_fee_curve: Option<RateCurveV2>,
```

**Step 2: Add default initialization**

In the `Default` / `From<InitArg>` impl, after the `weighted_avg_healthy_cr` initialization (line 618), add:

```rust
borrowing_fee_curve: Some(RateCurveV2 {
    markers: vec![
        RateMarkerV2 {
            cr_anchor: CrAnchor::Offset(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                Ratio::new(dec!(0.05)),
            ),
            multiplier: Ratio::new(dec!(3.0)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::Midpoint(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
            ),
            multiplier: Ratio::new(dec!(1.75)),
        },
        RateMarkerV2 {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
            multiplier: Ratio::new(dec!(1.0)),
        },
    ],
    method: InterpolationMethod::Linear,
}),
```

**Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend`
Expected: PASS. The `#[serde(default)]` on the field means existing serialized state (which lacks this field) will deserialize as `None`.

**Step 4: Commit**

```
feat(backend): add borrowing_fee_curve field to ProtocolState with default config
```

---

### Task 4: Dynamic fee calculation in `borrow_from_vault_impl`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs:662-669`

**Step 1: Write test for dynamic fee**

Add to `src/rumi_protocol_backend/src/state.rs` tests module:

```rust
#[test]
fn test_borrowing_fee_multiplier_above_tcr() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(1.75);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);

    // Vault CR = 2.0 (above TCR of 1.75) → multiplier should be 1.0
    let curve = state.borrowing_fee_curve.as_ref().unwrap();
    let resolved = state.resolve_curve(curve, None);
    let mult = State::interpolate_multiplier(&resolved, Ratio::from_f64(2.0));
    assert!((mult.to_f64() - 1.0).abs() < 0.01,
        "Above TCR should be 1.0x, got {}", mult.to_f64());
}

#[test]
fn test_borrowing_fee_multiplier_at_midpoint() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(2.0);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);
    // Midpoint = (1.5 + 2.0) / 2 = 1.75

    let curve = state.borrowing_fee_curve.as_ref().unwrap();
    let resolved = state.resolve_curve(curve, None);
    let mult = State::interpolate_multiplier(&resolved, Ratio::from_f64(1.75));
    assert!((mult.to_f64() - 1.75).abs() < 0.01,
        "At midpoint should be 1.75x, got {}", mult.to_f64());
}

#[test]
fn test_borrowing_fee_multiplier_at_floor() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(2.0);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);
    // Floor = BorrowThreshold + 0.05 = 1.55

    let curve = state.borrowing_fee_curve.as_ref().unwrap();
    let resolved = state.resolve_curve(curve, None);
    let mult = State::interpolate_multiplier(&resolved, Ratio::from_f64(1.55));
    assert!((mult.to_f64() - 3.0).abs() < 0.01,
        "At floor should be 3.0x, got {}", mult.to_f64());
}

#[test]
fn test_borrowing_fee_multiplier_below_floor_capped() {
    let mut state = accrual_test_state();
    state.total_collateral_ratio = Ratio::from_f64(2.0);
    state.recovery_mode_threshold = Ratio::from_f64(1.5);

    let curve = state.borrowing_fee_curve.as_ref().unwrap();
    let resolved = state.resolve_curve(curve, None);
    let mult = State::interpolate_multiplier(&resolved, Ratio::from_f64(1.4));
    assert!((mult.to_f64() - 3.0).abs() < 0.01,
        "Below floor should still be 3.0x (capped), got {}", mult.to_f64());
}

#[test]
fn test_borrowing_fee_multiplier_none_curve() {
    let mut state = accrual_test_state();
    state.borrowing_fee_curve = None;
    // No curve = 1.0x multiplier (flat fee)
    let mult = match &state.borrowing_fee_curve {
        Some(curve) => {
            let resolved = state.resolve_curve(curve, None);
            State::interpolate_multiplier(&resolved, Ratio::from_f64(1.4))
        }
        None => Ratio::new(dec!(1.0)),
    };
    assert!((mult.to_f64() - 1.0).abs() < 0.001);
}
```

**Step 2: Run tests to verify they pass with the default curve**

Run: `cargo test -p rumi_protocol_backend --lib -- test_borrowing_fee_multiplier`
Expected: All 5 tests PASS (they just test the curve resolution, not the vault flow).

**Step 3: Add `get_borrowing_fee_multiplier` helper**

Add to `impl State`, near `get_borrowing_fee_for` (line 978):

```rust
/// Get the dynamic borrowing fee multiplier for a projected vault CR.
/// Returns 1.0 if no borrowing_fee_curve is configured.
pub fn get_borrowing_fee_multiplier(&self, projected_vault_cr: Ratio) -> Ratio {
    match &self.borrowing_fee_curve {
        Some(curve) => {
            let resolved = self.resolve_curve(curve, None);
            Self::interpolate_multiplier(&resolved, projected_vault_cr)
        }
        None => Ratio::new(dec!(1.0)),
    }
}
```

**Step 4: Modify fee calculation in vault.rs**

Replace `src/rumi_protocol_backend/src/vault.rs:669`:

```rust
let fee: ICUSD = read_state(|s| amount * s.get_borrowing_fee_for(&vault.collateral_type));
```

With:

```rust
// Compute projected vault CR after this borrow (for dynamic fee multiplier)
let new_total_debt = vault.borrowed_icusd_amount + amount;
let projected_cr = if new_total_debt.to_u64() == 0 {
    Ratio::new(dec!(999))
} else {
    Ratio::from(
        Decimal::from_u64(collateral_value.to_u64()).unwrap_or(Decimal::ZERO)
            / Decimal::from_u64(new_total_debt.to_u64()).unwrap_or(Decimal::ONE)
    )
};

let fee: ICUSD = read_state(|s| {
    let base_fee = s.get_borrowing_fee_for(&vault.collateral_type);
    let multiplier = s.get_borrowing_fee_multiplier(projected_cr);
    amount * base_fee * multiplier
});
```

Note: This requires `use rust_decimal::Decimal` at the top of vault.rs if not already imported. Check existing imports.

**Step 5: Verify it compiles and existing tests pass**

Run: `cargo test -p rumi_protocol_backend`
Expected: All existing tests PASS. The default curve starts with `BorrowThreshold` which defaults to `recovery_mode_threshold` — on a fresh test state with no debt, this defaults to `MINIMUM_COLLATERAL_RATIO` (1.33). The `total_collateral_ratio` starts at 0. So the curve points may collapse. Existing tests that use `create_test_state()` set specific values. Verify no test regression.

**Step 6: Commit**

```
feat(backend): apply dynamic borrowing fee multiplier in borrow_from_vault
```

---

### Task 5: Expose resolved curve in ProtocolStatus

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs:104-121` (ProtocolStatus struct)
- Modify: `src/rumi_protocol_backend/src/main.rs:292-314` (get_protocol_status)

**Step 1: Add field to ProtocolStatus**

In `lib.rs`, add to the `ProtocolStatus` struct (after `weighted_average_interest_rate` at line 120):

```rust
pub borrowing_fee_curve_resolved: Vec<(f64, f64)>,
```

**Step 2: Populate in get_protocol_status**

In `main.rs:293-313`, add before the closing `})`:

```rust
borrowing_fee_curve_resolved: match &s.borrowing_fee_curve {
    Some(curve) => s.resolve_curve(curve, None).iter()
        .map(|(cr, mult)| (cr.to_f64(), mult.to_f64()))
        .collect(),
    None => vec![],
},
```

**Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend`
Expected: PASS.

**Step 4: Commit**

```
feat(backend): expose resolved borrowing fee curve in ProtocolStatus
```

---

### Task 6: Admin endpoint `set_borrowing_fee_curve`

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (add new update function after `set_recovery_rate_curve` ~line 1940)
- Modify: `src/rumi_protocol_backend/src/event.rs` (add event recording)

**Step 1: Add event variant**

In `event.rs`, find the `Event` enum and add:

```rust
SetBorrowingFeeCurve {
    markers: String,  // JSON serialized
},
```

Add recording function:

```rust
pub fn record_set_borrowing_fee_curve(
    state: &mut State,
    curve: Option<RateCurveV2>,
) {
    let markers_json = match &curve {
        Some(c) => serde_json::to_string(&c).unwrap_or_default(),
        None => "null".to_string(),
    };
    record_event(&Event::SetBorrowingFeeCurve {
        markers: markers_json,
    });
    state.borrowing_fee_curve = curve;
}
```

**Step 2: Add admin endpoint in main.rs**

After `set_recovery_rate_curve` function:

```rust
/// Set the dynamic borrowing fee curve.
/// Pass empty vec to disable (revert to flat fee).
/// Each marker is a JSON-encoded CrAnchor + multiplier f64.
/// For simplicity, the API accepts a serialized RateCurveV2.
#[candid_method(update)]
#[update]
async fn set_borrowing_fee_curve(
    curve_json: Option<String>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set borrowing fee curve".to_string(),
        ));
    }
    let curve: Option<RateCurveV2> = match curve_json {
        None => None,
        Some(json) => {
            let parsed: RateCurveV2 = serde_json::from_str(&json)
                .map_err(|e| ProtocolError::GenericError(format!("Invalid curve JSON: {}", e)))?;
            if parsed.markers.is_empty() {
                return Err(ProtocolError::GenericError(
                    "Curve must have at least 1 marker".to_string(),
                ));
            }
            for m in &parsed.markers {
                if m.multiplier.to_f64() <= 0.0 {
                    return Err(ProtocolError::GenericError(
                        "All multipliers must be positive".to_string(),
                    ));
                }
            }
            Some(parsed)
        }
    };
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_borrowing_fee_curve(s, curve);
    });
    log!(INFO, "[set_borrowing_fee_curve] Updated borrowing fee curve");
    Ok(())
}
```

**Step 3: Verify it compiles**

Run: `cargo check -p rumi_protocol_backend`
Expected: PASS.

**Step 4: Commit**

```
feat(backend): add set_borrowing_fee_curve admin endpoint
```

---

### Task 7: Frontend — Add `interpolateMultiplier` utility and update types

**Files:**
- Create: `src/vault_frontend/src/lib/utils/interpolate.ts`
- Modify: `src/vault_frontend/src/lib/services/types.ts:88-103` (ProtocolStatusDTO)
- Modify: `src/vault_frontend/src/lib/services/protocol/queryOperations.ts:27-42` (getProtocolStatus mapping)

**Step 1: Create interpolation utility**

Create `src/vault_frontend/src/lib/utils/interpolate.ts`:

```typescript
/**
 * Piecewise linear interpolation over a sorted curve.
 * curve: array of [crLevel, multiplier] pairs, sorted ascending by crLevel.
 * Returns the interpolated multiplier for the given CR.
 */
export function interpolateMultiplier(curve: [number, number][], cr: number): number {
  if (curve.length === 0) return 1;
  if (cr <= curve[0][0]) return curve[0][1];
  if (cr >= curve[curve.length - 1][0]) return curve[curve.length - 1][1];
  for (let i = 0; i < curve.length - 1; i++) {
    if (cr >= curve[i][0] && cr <= curve[i + 1][0]) {
      const range = curve[i + 1][0] - curve[i][0];
      if (range === 0) return curve[i][1];
      const t = (cr - curve[i][0]) / range;
      return curve[i][1] + t * (curve[i + 1][1] - curve[i][1]);
    }
  }
  return 1;
}
```

**Step 2: Add `borrowingFeeCurveResolved` to ProtocolStatusDTO**

In `src/vault_frontend/src/lib/services/types.ts`, add to `ProtocolStatusDTO` (after `interestPoolShare` at line 102):

```typescript
borrowingFeeCurveResolved: [number, number][];
```

**Step 3: Map the new field in queryOperations.ts**

In `src/vault_frontend/src/lib/services/protocol/queryOperations.ts:27-42`, add to the return object:

```typescript
borrowingFeeCurveResolved: Array.isArray((canisterStatus as any).borrowing_fee_curve_resolved)
  ? (canisterStatus as any).borrowing_fee_curve_resolved.map(
      (p: any) => [Number(p[0]), Number(p[1])] as [number, number]
    )
  : [],
```

**Step 4: Verify frontend compiles**

Run: `cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json 2>&1 | head -30`
Expected: No new errors.

**Step 5: Commit**

```
feat(frontend): add interpolateMultiplier util and borrowingFeeCurveResolved type
```

---

### Task 8: Frontend — Dynamic fee preview in VaultCard borrow form

**Files:**
- Modify: `src/vault_frontend/src/lib/components/vault/VaultCard.svelte:1-15` (imports)
- Modify: `src/vault_frontend/src/lib/components/vault/VaultCard.svelte:370-375` (fee breakdown reactives)
- Modify: `src/vault_frontend/src/lib/components/vault/VaultCard.svelte:789-793` (fee display)

**Step 1: Add import**

At VaultCard.svelte line 1, add to the import:

```typescript
import { interpolateMultiplier } from '../../utils/interpolate';
```

**Step 2: Get protocol status for the fee curve**

The VaultCard already has access to `vaultCollateralInfo`. We need the protocol status for the resolved curve. Add a reactive that gets it:

```typescript
// Fetch protocol status for borrowing fee curve
import { ProtocolService } from '../../services/protocol';

let borrowingFeeCurve: [number, number][] = [];
ProtocolService.getProtocolStatus().then(status => {
  borrowingFeeCurve = status.borrowingFeeCurveResolved ?? [];
});
```

Or better, subscribe to the protocol status if there's a store. Check if there's an existing reactive protocol status. Looking at VaultCard, it doesn't directly subscribe to protocol status — it gets `icpPrice` as a prop. The simplest approach: fetch once on mount and when vault changes.

**Step 3: Update fee breakdown reactives**

Replace the block at lines 371-375:

```typescript
// ── Borrow fee breakdown (dynamic) ──
$: vaultBorrowingFee = vaultCollateralInfo?.borrowingFee ?? 0;
$: parsedBorrowAmount = parseFloat(borrowAmount) || 0;
$: projectedBorrowCr = (() => {
    if (parsedBorrowAmount <= 0 || collateralValueUsd <= 0) return Infinity;
    const newDebt = tickingDebt + parsedBorrowAmount;
    return newDebt > 0 ? collateralValueUsd / newDebt : Infinity;
  })();
$: borrowFeeMultiplier = borrowingFeeCurve.length > 0
    ? interpolateMultiplier(borrowingFeeCurve, projectedBorrowCr)
    : 1;
$: effectiveBorrowFeeRate = vaultBorrowingFee * borrowFeeMultiplier;
$: borrowFeeAmount = parsedBorrowAmount * effectiveBorrowFeeRate;
$: borrowReceiveAmount = parsedBorrowAmount - borrowFeeAmount;
```

**Step 4: Update fee display in template**

Replace lines 789-793:

```svelte
{#if parsedBorrowAmount > 0 && vaultBorrowingFee > 0}
  <div class="fee-breakdown">
    <div class="fee-row">
      <span>Fee ({(effectiveBorrowFeeRate * 100).toFixed(2)}%{borrowFeeMultiplier > 1.01 ? ` · ${borrowFeeMultiplier.toFixed(2)}x` : ''})</span>
      <span>{formatStableTx(borrowFeeAmount)} icUSD</span>
    </div>
    <div class="fee-row"><span>You receive</span><span>{formatStableTx(borrowReceiveAmount)} icUSD</span></div>
  </div>
{/if}
```

This shows "Fee (0.35% · 1.75x)" when the multiplier is above 1, and just "Fee (0.20%)" when at 1x.

**Step 5: Verify frontend compiles and renders**

Start dev server, open a vault, enter a borrow amount. Confirm the fee breakdown shows the multiplier.

**Step 6: Commit**

```
feat(frontend): dynamic borrowing fee preview in VaultCard borrow form
```

---

### Task 9: Frontend — Dynamic fee preview in homepage MintForm

**Files:**
- Modify: `src/vault_frontend/src/routes/+page.svelte` (around lines 41, 260-263)

**Step 1: Import and fetch curve**

The homepage already has fee display at lines 261-262. Add:

```typescript
import { interpolateMultiplier } from '$lib/utils/interpolate';
```

And get the curve from protocol status (the homepage already fetches it for other fields):

```typescript
let borrowingFeeCurve: [number, number][] = [];
```

In the existing `onMount` or protocol status fetch, add:

```typescript
borrowingFeeCurve = status.borrowingFeeCurveResolved ?? [];
```

**Step 2: Add projected CR and multiplier reactives**

Near the existing fee reactives:

```typescript
$: projectedMintCr = (() => {
    if (icusdAmount <= 0 || collateralAmount <= 0 || collateralPrice <= 0) return Infinity;
    const collateralValue = collateralAmount * collateralPrice;
    return collateralValue / icusdAmount;
  })();
$: mintFeeMultiplier = borrowingFeeCurve.length > 0
    ? interpolateMultiplier(borrowingFeeCurve, projectedMintCr)
    : 1;
$: effectiveMintFeeRate = selectedBorrowingFee * mintFeeMultiplier;
$: calculatedBorrowFee = icusdAmount * effectiveMintFeeRate;
$: calculatedIcusdAmount = icusdAmount - calculatedBorrowFee;
```

**Step 3: Update fee display**

Replace lines 261-262:

```svelte
<div class="fee-row">
  <span>Fee ({(effectiveMintFeeRate * 100).toFixed(2)}%{mintFeeMultiplier > 1.01 ? ` · ${mintFeeMultiplier.toFixed(2)}x` : ''})</span>
  <span>{formatStableTx(calculatedBorrowFee)} icUSD</span>
</div>
<div class="fee-row"><span>You receive</span><span>{formatStableTx(calculatedIcusdAmount)} icUSD</span></div>
```

**Step 4: Verify**

Start dev server, go to homepage, enter collateral and borrow amounts. Confirm dynamic fee shows.

**Step 5: Commit**

```
feat(frontend): dynamic borrowing fee preview in homepage create-vault form
```

---

### Task 10: Run full test suite and verify

**Step 1: Run all backend tests**

Run: `cargo test -p rumi_protocol_backend`
Expected: All tests PASS.

**Step 2: Run frontend type check**

Run: `cd src/vault_frontend && npx svelte-check --tsconfig ./tsconfig.json`
Expected: No new errors in modified files.

**Step 3: Visual verification**

Start dev server, test:
1. Homepage: enter 10 ICP collateral at $10, borrow 50 icUSD → projected CR = 200% (should be above TCR, multiplier = 1.0x, fee = 0.20%)
2. Homepage: enter 10 ICP collateral at $10, borrow 60 icUSD → projected CR = 167% (may show multiplier > 1x if TCR is above 167%)
3. Existing vault: borrow additional icUSD → fee breakdown shows multiplier

**Step 4: Final commit if needed**

If any fixes were required during testing.

---

## Notes for Implementer

- **Serde compatibility:** `#[serde(default)]` on `borrowing_fee_curve: Option<RateCurveV2>` means old canister state (lacking this field) deserializes as `None`, which means flat fee. Safe upgrade.
- **`RateMarkerV2` vs `RateMarker`:** We create a parallel v2 type to avoid touching the existing serialized `global_rate_curve` and per-asset `rate_curve`. The v1 types continue to work for interest rates. Migration of interest rates to v2 is a separate future task.
- **`interpolate_multiplier`** is an existing `fn` on `State` (line 1045). It works on `&[(Ratio, Ratio)]` — `resolve_curve` produces exactly this shape.
- **`Decimal` import in vault.rs:** Check if `use rust_decimal::Decimal;` is already imported. If not, add it.
- **Event recording:** Follow the pattern in `record_set_rate_curve_markers` (event.rs:1273) — serialize to JSON string, record event, then mutate state.
- **Test helper `accrual_test_state()`** is defined at `state.rs:2625`. It sets ICP price to $10 and 5% APR. The `borrowing_fee_curve` will be populated from the Default impl since `accrual_test_state()` uses `State::from(InitArg {...})`.
