# Dynamic Borrowing Fee Multiplier — Design

**Date:** 2026-03-08
**Status:** Approved

## Problem

The borrowing fee is currently a flat rate per collateral type (e.g., 0.2% for ICP). This means a borrower who takes their vault from 250% CR down to 155% CR (just above the system minimum) pays the same fee as one who stays at 250%. Since low-CR borrows drag down the system-wide TCR, they should cost more as a soft deterrent.

## Solution

A configurable piecewise-linear fee multiplier curve that scales the base borrowing fee based on the borrower's **projected vault CR** (CR after the borrow) relative to the system-wide TCR.

Default configuration:
- **1x** at or above system TCR — no penalty
- **1.75x** at the midpoint between system minimum CR and TCR
- **3x** at system minimum CR + 5% buffer (e.g., 155% if minimum is 150%)
- **Capped at 3x** below that floor

All multiplier values, CR anchor points, and the number of nodes are admin-configurable.

## Design

### 1. Unified CrAnchor Type

Replace the current split between concrete `cr_level: Ratio` (in `RateMarker`) and `SystemThreshold` enum (in `RecoveryRateMarker`) with a single `CrAnchor` enum:

```rust
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum CrAnchor {
    /// Concrete CR value (e.g., 1.5 = 150%).
    Fixed(Ratio),

    /// Per-asset threshold, resolved from CollateralConfig at runtime.
    AssetThreshold(AssetThreshold),

    /// System-wide threshold, resolved from debt-weighted averages at runtime.
    SystemThreshold(SystemThreshold),

    /// Midpoint of two anchors: (A + B) / 2.
    Midpoint(Box<CrAnchor>, Box<CrAnchor>),

    /// Offset from another anchor: A + delta (delta can be negative).
    Offset(Box<CrAnchor>, Ratio),
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum AssetThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
}
```

`SystemThreshold` gains a new variant:

```rust
pub enum SystemThreshold {
    LiquidationRatio,       // debt-weighted avg liquidation ratio
    BorrowThreshold,        // debt-weighted avg borrow threshold
    WarningCr,              // debt-weighted avg warning CR
    HealthyCr,              // debt-weighted avg healthy CR
    TotalCollateralRatio,   // NEW: actual system-wide CR
}
```

`RateMarker` changes from `cr_level: Ratio` to `cr_anchor: CrAnchor`. `RecoveryRateMarker` is deleted — the recovery curve becomes a standard `RateCurve` with `SystemThreshold` anchors.

### 2. Resolution

A single `resolve_anchor()` method replaces both `resolve_layer1_markers` and `resolve_layer2_markers`:

```rust
fn resolve_anchor(
    &self,
    anchor: &CrAnchor,
    asset_context: Option<&CollateralType>,
) -> Ratio {
    match anchor {
        CrAnchor::Fixed(r) => *r,
        CrAnchor::AssetThreshold(t) => {
            let ct = asset_context.expect("AssetThreshold needs asset context");
            match t {
                AssetThreshold::LiquidationRatio => self.get_liquidation_ratio_for(ct),
                AssetThreshold::BorrowThreshold => self.get_borrow_threshold_for(ct),
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

fn resolve_curve(
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

`interpolate_multiplier()` is unchanged — it already operates on `Vec<(Ratio, Ratio)>`.

### 3. Borrowing Fee Curve

New `Option<RateCurve>` field on `ProtocolState`:

```rust
pub borrowing_fee_curve: Option<RateCurve>,
```

Default configuration:

```rust
borrowing_fee_curve: Some(RateCurve {
    markers: vec![
        RateMarker {
            cr_anchor: CrAnchor::Offset(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                Ratio::new(dec!(0.05)),
            ),
            multiplier: Ratio::new(dec!(3.0)),
        },
        RateMarker {
            cr_anchor: CrAnchor::Midpoint(
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::BorrowThreshold)),
                Box::new(CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio)),
            ),
            multiplier: Ratio::new(dec!(1.75)),
        },
        RateMarker {
            cr_anchor: CrAnchor::SystemThreshold(SystemThreshold::TotalCollateralRatio),
            multiplier: Ratio::new(dec!(1.0)),
        },
    ],
    method: InterpolationMethod::Linear,
}),
```

### 4. Fee Calculation (vault.rs)

In `borrow_from_vault_impl()`, replace the flat fee lookup:

```rust
// Compute projected vault CR after this borrow
let new_total_debt = vault.borrowed_icusd_amount + amount;
let projected_cr = if new_total_debt.to_u64() == 0 {
    Ratio::new(dec!(999))
} else {
    Ratio::from(
        Decimal::from_u64(collateral_value.to_u64()).unwrap_or(Decimal::ZERO)
        / Decimal::from_u64(new_total_debt.to_u64()).unwrap_or(Decimal::ONE)
    )
};

// Get base fee and dynamic multiplier
let (base_fee, multiplier) = read_state(|s| {
    let base = s.get_borrowing_fee_for(&vault.collateral_type);
    let mult = match &s.borrowing_fee_curve {
        Some(curve) => {
            let resolved = s.resolve_curve(curve, None);
            ProtocolState::interpolate_multiplier(&resolved, projected_cr)
        }
        None => Ratio::new(dec!(1.0)),
    };
    (base, mult)
});

let fee: ICUSD = amount * base_fee * multiplier;
```

### 5. Migration Strategy

**Borrowing fee curve:** Entirely new `Option` field — `None` on deserialization of old state means flat fee, fully backward compatible.

**Existing interest rate curves:** Use v1/v2 approach for safety:
- Keep `global_rate_curve` and `recovery_rate_curve` (v1) with `#[serde(default)]`
- Add `global_rate_curve_v2: Option<RateCurve>` and `recovery_rate_curve_v2: Option<RateCurve>` using new `CrAnchor` markers
- Rate calculation checks v2 first, falls back to v1
- Migrate v1 → v2 via admin call or `post_upgrade` when ready
- Eventually deprecate v1

### 6. Frontend Preview

**Backend:** Add to `ProtocolStatus`:

```rust
pub borrowing_fee_curve_resolved: Vec<(f64, f64)>,  // [(cr_level, multiplier), ...]
```

Pre-resolved with current system state so the frontend doesn't need to know about `CrAnchor`.

**Frontend:** Simple piecewise linear interpolation in TypeScript:

```typescript
function interpolateMultiplier(curve: [number, number][], cr: number): number {
    if (curve.length === 0) return 1;
    if (cr <= curve[0][0]) return curve[0][1];
    if (cr >= curve[curve.length - 1][0]) return curve[curve.length - 1][1];
    for (let i = 0; i < curve.length - 1; i++) {
        if (cr >= curve[i][0] && cr <= curve[i + 1][0]) {
            const t = (cr - curve[i][0]) / (curve[i + 1][0] - curve[i][0]);
            return curve[i][1] + t * (curve[i + 1][1] - curve[i][1]);
        }
    }
    return 1;
}
```

**Borrow form UI:** As user types borrow amount, compute projected CR, look up multiplier, display:
- Base fee: 0.20%
- Multiplier: 1.75x (projected CR: 162%)
- Effective fee: 0.35%
- Fee amount: X.XXXX icUSD
- You receive: X.XXXX icUSD

### 7. Admin Interface

```rust
pub fn set_borrowing_fee_curve(curve: Option<RateCurve>) -> Result<(), ProtocolError>
```

Accepts `None` (disable, revert to flat) or any `RateCurve`. Admin can add/remove nodes freely. Validation: at least 1 marker, all multipliers > 0.

## What This Does NOT Change

- `formatNumber()`, `formatTokenBalance()`, collateral display formatting
- Base `borrowing_fee` on `CollateralConfig` — still the foundation; multiplier scales it
- `get_borrowing_fee_for()` — still returns the base fee; multiplier is applied separately in `vault.rs`
- Recovery mode borrowing fee override — still takes precedence when in Recovery mode
- Interest rate calculation — continues using v1 curves until explicit migration
