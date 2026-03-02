# Design: Dynamic Interest Rates

## Problem

Interest rates are currently flat per collateral asset regardless of vault health or system state. A vault at 500% CR pays the same rate as one at 140% CR. During Recovery mode, the protocol has no mechanism to dynamically pressure undercollateralized positions through rates — only static overrides from the Phase 1 recovery fix.

## Solution

Two-layer dynamic interest rate system:

- **Layer 1 (always active):** Per-asset rate curve that multiplies the base APR based on individual vault CR. Healthy vaults pay base rate; riskier vaults pay progressively more.
- **Layer 2 (Recovery mode only):** System-wide multiplier based on TCR position relative to weighted-average thresholds. Stacks on top of Layer 1 to create additional urgency during system-wide crises.

Static overrides from Phase 1 (`recovery_borrowing_fee`, `recovery_interest_rate_apr`) remain as admin escape valves.

## Data Model

### New Types

```rust
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub enum InterpolationMethod {
    Linear,
    // Future: Exponential, Polynomial(u8), etc.
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct RateMarker {
    pub cr_level: Ratio,
    pub multiplier: Ratio,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct RateCurve {
    pub markers: Vec<RateMarker>,  // sorted by cr_level ascending
    pub method: InterpolationMethod,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub enum SystemThreshold {
    LiquidationRatio,
    BorrowThreshold,
    WarningCr,
    HealthyCr,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct RecoveryRateMarker {
    pub threshold: SystemThreshold,
    pub multiplier: Ratio,
}
```

### New Fields on CollateralConfig

```rust
/// Admin-configurable "healthy" CR. Default: 1.5 * borrow_threshold_ratio.
#[serde(default)]
pub healthy_cr: Option<Ratio>,

/// Per-asset rate curve markers. None = use global defaults.
#[serde(default)]
pub rate_curve: Option<RateCurve>,
```

### New Fields on State

```rust
/// Global default rate curve for Layer 1 (used when asset has no per-asset curve).
pub global_rate_curve: RateCurve,

/// Recovery mode rate curve for Layer 2 (system-wide, uses named thresholds).
pub recovery_rate_curve: Vec<RecoveryRateMarker>,

/// Cached debt-weighted averages (updated on each price tick):
pub weighted_avg_recovery_cr: Ratio,
pub weighted_avg_warning_cr: Ratio,
pub weighted_avg_healthy_cr: Ratio,
```

### Derived Values (computed, not stored)

Per-asset:
- `recovery_cr` = `borrow_threshold_ratio + recovery_liquidation_buffer`
- `warning_cr` = `2 * recovery_cr - borrow_threshold_ratio`
- `healthy_cr` = admin override if set, else `1.5 * borrow_threshold_ratio`

System-wide (cached on each price tick in `update_total_collateral_ratio_and_mode()`):
- `weighted_avg_recovery_cr` = debt-weighted average of per-asset `recovery_cr`
- `weighted_avg_warning_cr` = debt-weighted average of per-asset `warning_cr`
- `weighted_avg_healthy_cr` = debt-weighted average of per-asset `healthy_cr`

### Default Rate Curve Markers (Layer 1)

Built from per-asset thresholds. Example with ICP (liq=133%, borrow=150%, buffer=5%):

| Marker Source        | CR Level | Default Multiplier |
|----------------------|----------|--------------------|
| `liquidation_ratio`  | 133%     | 5.0x               |
| `borrow_threshold`   | 150%     | 2.5x               |
| `warning_cr`         | 160%     | 1.75x              |
| `healthy_cr`         | 225%     | 1.0x               |

Assets without a per-asset `rate_curve` use `global_rate_curve` from State. The global default is initialized with the multipliers above; the CR levels are resolved per-asset from that asset's thresholds.

### Default Recovery Rate Curve (Layer 2)

Uses named `SystemThreshold` values resolved to weighted averages at runtime:

| Threshold            | Multiplier |
|----------------------|------------|
| `HealthyCr`          | 1.0x       |
| `WarningCr`          | 1.15x      |
| `BorrowThreshold`    | 1.33x      |
| `LiquidationRatio`   | 2.0x       |

## Rate Computation Logic

### Layer 1 — Per-asset vault CR rate (always active)

Given a vault with collateral ratio `vault_cr` and base rate `interest_rate_apr`:

1. Look up the asset's rate curve (per-asset `rate_curve` if set, else `global_rate_curve`)
2. Resolve marker CR levels from the asset's own thresholds (liquidation_ratio, borrow_threshold_ratio, derived warning_cr, derived healthy_cr)
3. If `vault_cr >= healthy_cr`: multiplier = 1.0x
4. If `vault_cr <= liquidation_ratio`: multiplier = max marker multiplier (5.0x)
5. Otherwise: find the two surrounding markers, linearly interpolate the multiplier
6. **Effective rate** = `interest_rate_apr * multiplier`

Example: ICP vault at 155% CR (between borrow_threshold 150% and warning_cr 160%):
- Markers: (150%, 2.5x) and (160%, 1.75x)
- Position: (155 - 150) / (160 - 150) = 0.5
- Multiplier: 2.5 - 0.5 * (2.5 - 1.75) = 2.125x
- If base APR is 2%: effective = 4.25%

### Layer 2 — System-wide recovery multiplier (Recovery mode only)

When `mode == Recovery`:

1. Resolve `recovery_rate_curve` named thresholds to their current weighted averages
2. If `TCR >= weighted_avg_healthy_cr`: recovery multiplier = 1.0x
3. If `TCR <= weighted_avg_liquidation_ratio`: recovery multiplier = max marker (2.0x)
4. Between: linearly interpolate
5. **Final rate** = `Layer 1 rate * recovery multiplier`

Example: System in deep recovery, TCR at weighted liquidation ratio. ICP vault at 155% CR:
- Layer 1: 4.25% (from above)
- Layer 2: 2.0x
- Final: 8.5% APR

### Static Override Escape Valve

If `recovery_interest_rate_apr` is set on a CollateralConfig and the system is in Recovery mode, that static value **replaces** the entire dynamic calculation. This gives admins a kill switch.

## Admin Functions

### `set_rate_curve_markers`

```
set_rate_curve_markers(
    collateral_type: Option<Principal>,  // None = global default
    markers: Vec<(f64, f64)>,            // (cr_level, multiplier) pairs
) -> Result<(), ProtocolError>
```

- `None` collateral_type updates `global_rate_curve` on State
- `Some(principal)` updates that asset's `rate_curve` on CollateralConfig
- Validates: caller is developer, markers have at least 2 entries, sorted by cr_level ascending, multipliers > 0
- Hardcodes `InterpolationMethod::Linear`

### `set_recovery_rate_curve`

```
set_recovery_rate_curve(
    markers: Vec<(SystemThreshold, f64)>,  // (named threshold, multiplier) pairs
) -> Result<(), ProtocolError>
```

- Updates `recovery_rate_curve` on State
- Validates: caller is developer, multipliers > 0, at least 2 markers

### `set_healthy_cr`

```
set_healthy_cr(
    collateral_type: Principal,
    healthy_cr: Option<f64>,  // None = reset to default (1.5x borrow threshold)
) -> Result<(), ProtocolError>
```

- Validates: caller is developer, collateral exists, value > borrow_threshold_ratio if set

### Events

Three new event variants for on-chain auditability:

```rust
SetRateCurveMarkers {
    collateral_type: Option<String>,  // None for global
    markers: String,                  // JSON-serialized marker pairs
}

SetRecoveryRateCurve {
    markers: String,  // JSON-serialized (threshold, multiplier) pairs
}

SetHealthyCr {
    collateral_type: String,
    healthy_cr: Option<String>,
}
```

Each with a recording helper and replay case following existing event patterns.

## Integration Points

### Price tick — cache weighted averages

In `update_total_collateral_ratio_and_mode()`, extend the existing `compute_dynamic_recovery_threshold()` loop to also compute:
- `weighted_avg_recovery_cr` (weighted avg of `borrow_threshold_ratio + recovery_liquidation_buffer`)
- `weighted_avg_warning_cr` (weighted avg of `2 * recovery_cr - borrow_threshold_ratio`)
- `weighted_avg_healthy_cr` (weighted avg of per-asset `healthy_cr` or `1.5 * borrow_threshold_ratio`)

Same loop, same debt weights. Four weighted sums instead of one.

### New rate getter

```rust
pub fn get_dynamic_interest_rate_for(&self, ct: &CollateralType, vault_cr: Ratio) -> Ratio
```

1. Check static override escape valve
2. Get base rate from CollateralConfig
3. Layer 1: resolve curve, interpolate multiplier from vault_cr, apply
4. Layer 2 (Recovery only): resolve named thresholds, interpolate from TCR, multiply
5. Return final rate

### Query endpoint

```
get_vault_interest_rate(vault_id: u64) -> Result<f64, ProtocolError>
```

Returns current dynamic rate for a specific vault. Frontend can call this instead of replicating interpolation logic.

### Backward compatibility

The Phase 1 method `get_interest_rate_for(ct)` remains unchanged for callers without vault CR context. `get_dynamic_interest_rate_for(ct, vault_cr)` is the richer version.

### `.did` file updates

- New types: `InterpolationMethod`, `RateMarker`, `RateCurve`, `SystemThreshold`, `RecoveryRateMarker`
- Updated `CollateralConfig` with `healthy_cr` and `rate_curve`
- New service methods: `set_rate_curve_markers`, `set_recovery_rate_curve`, `set_healthy_cr`, `get_vault_interest_rate`
- New event variants

## Performance

Rate computation is cheap:
- **Per-vault rate:** O(k) where k = number of markers (~4). One interpolation per vault operation.
- **Weighted averages:** Computed per collateral type (5-20 items), not per vault. Piggybacks on existing `compute_dynamic_recovery_threshold()` loop.
- **No additional vault iteration** beyond what already happens on each price tick.

## Future-Proofing

The `InterpolationMethod` enum allows adding `Exponential`, `Polynomial(u8)`, or other curve types via a backend upgrade (code change + `dfx deploy --upgrade`). No data migration needed — existing `Linear` configs deserialize unchanged. The admin function for setting markers would gain an optional `method` parameter at that point.

## Note

Interest rate accrual is **not yet implemented**. This design defines how rates are *computed* — the accrual mechanism that applies these rates to vault debt over time is a separate feature. The `get_vault_interest_rate` query and `get_dynamic_interest_rate_for()` method will return correct rates immediately; they just won't affect debt until accrual logic is built.
