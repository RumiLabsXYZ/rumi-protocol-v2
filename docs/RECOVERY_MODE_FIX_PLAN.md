# Plan: Fix Recovery Mode Borrowing Incentives

## Context
Recovery Mode currently sets the borrowing fee to 0%, which incentivizes MORE borrowing — the opposite of what the protocol needs when the system is undercollateralized. A user at 250% CR can borrow down to 151% for free during a crisis, dragging the system TCR lower.

Three fixes:
1. **Stop zeroing the borrowing fee** — use configurable recovery-specific rates instead
2. **Raise the minimum CR for borrowing/withdrawal** during Recovery to the recovery target CR (e.g., 155%)
3. **Add per-collateral recovery overrides** for borrowing fee and interest APR, settable by admin

## Changes

### 1. `state.rs` — Add recovery fields to CollateralConfig

**File:** `src/rumi_protocol_backend/src/state.rs`

Add two `Option` fields to `CollateralConfig` (lines ~192-195, before `display_color`):
```rust
/// Borrowing fee override during Recovery mode. None = use normal borrowing_fee.
#[serde(default)]
pub recovery_borrowing_fee: Option<Ratio>,
/// Interest rate override during Recovery mode. None = use normal interest_rate_apr.
#[serde(default)]
pub recovery_interest_rate_apr: Option<Ratio>,
```

Using `Option` with `#[serde(default)]` ensures backward compatibility with existing serialized state (deserializes as `None`).

Also add these fields to the `PartialEq` impl (~line 198).

### 2. `state.rs` — Fix fee getters

**`get_borrowing_fee()` (line 676):** Remove the `Mode::Recovery => ZERO` branch. Return `self.fee` for all modes.

**`get_borrowing_fee_for()` (line 731):** In Recovery, return `recovery_borrowing_fee` if set, otherwise the normal `borrowing_fee`:
```rust
pub fn get_borrowing_fee_for(&self, ct: &CollateralType) -> Ratio {
    let config = self.collateral_configs.get(ct);
    if self.mode == Mode::Recovery {
        // Use recovery override if set, otherwise normal fee
        return config
            .and_then(|c| c.recovery_borrowing_fee)
            .or_else(|| config.map(|c| c.borrowing_fee))
            .unwrap_or(self.fee);
    }
    config.map(|c| c.borrowing_fee).unwrap_or(self.fee)
}
```

### 3. `state.rs` — Add recovery-aware interest rate getter

Add a new method for future use (interest accrual is not yet implemented, but the frontend displays it):
```rust
pub fn get_interest_rate_for(&self, ct: &CollateralType) -> Ratio {
    let config = self.collateral_configs.get(ct);
    if self.mode == Mode::Recovery {
        return config
            .and_then(|c| c.recovery_interest_rate_apr)
            .or_else(|| config.map(|c| c.interest_rate_apr))
            .unwrap_or(DEFAULT_INTEREST_RATE_APR);
    }
    config.map(|c| c.interest_rate_apr).unwrap_or(DEFAULT_INTEREST_RATE_APR)
}
```

### 4. `vault.rs` — Enforce recovery target CR for borrowing

**`borrow_from_vault_internal()` (line 612):** During Recovery, use the recovery target CR as the minimum ratio instead of the borrow threshold:
```rust
let min_ratio = read_state(|s| {
    let base = s.get_min_collateral_ratio_for(&vault.collateral_type);
    if s.mode == Mode::Recovery {
        let recovery_target = s.get_recovery_target_cr_for(&vault.collateral_type);
        if recovery_target > base { recovery_target } else { base }
    } else {
        base
    }
});
```

**`withdraw_collateral` (line 1392):** Same change — during Recovery, use the higher of borrow threshold or recovery target CR. This prevents withdrawals that would drop the vault below the recovery target.

### 5. `main.rs` — Add `set_recovery_parameters` admin function

New developer-only update function for setting per-collateral recovery overrides:

```rust
#[candid_method(update)]
#[update]
async fn set_recovery_parameters(
    collateral_type: Principal,
    recovery_borrowing_fee: Option<f64>,
    recovery_interest_rate_apr: Option<f64>,
) -> Result<(), ProtocolError> { ... }
```

- Validates caller is developer
- Validates collateral type exists
- Validates fee ranges (0.0–0.10 for borrowing fee, 0.0–1.0 for APR)
- `None` means "clear override, use normal value"
- Records an event for on-chain auditability

### 6. `event.rs` — Add event variant

Add `SetRecoveryParameters` event variant to track admin changes:
```rust
SetRecoveryParameters {
    collateral_type: CollateralType,
    recovery_borrowing_fee: Option<String>,
    recovery_interest_rate_apr: Option<String>,
}
```

Add `record_set_recovery_parameters()` helper that records the event and mutates the config.

### 7. `lib.rs` — Update .did generation

Run `candid::export_service!()` or update the `.did` file to include the new `set_recovery_parameters` function and the updated `CollateralConfig` type with the two new optional fields.

### 8. Frontend docs — Fix Recovery Mode description

**`src/vault_frontend/src/routes/docs/risks/+page.svelte`** (line 62): Remove "but the fee drops to 0% to encourage repayment" — replace with accurate description of Recovery behavior.

**`src/vault_frontend/src/routes/docs/liquidation/+page.svelte`** (line 90): Same — remove the 0% fee claim.

## Files Modified (summary)

| File | Change |
|------|--------|
| `src/rumi_protocol_backend/src/state.rs` | Add 2 fields to CollateralConfig, fix fee getters, add interest rate getter |
| `src/rumi_protocol_backend/src/vault.rs` | Enforce recovery target CR in borrow + withdraw |
| `src/rumi_protocol_backend/src/main.rs` | Add `set_recovery_parameters` admin function |
| `src/rumi_protocol_backend/src/event.rs` | Add event variant + recording function |
| `src/rumi_protocol_backend/src/lib.rs` | Update .did if needed |
| `src/vault_frontend/src/routes/docs/risks/+page.svelte` | Fix Recovery Mode description |
| `src/vault_frontend/src/routes/docs/liquidation/+page.svelte` | Fix Recovery Mode description |

## Verification
1. `cargo build` — backend compiles
2. `cargo test` — existing tests pass
3. `npm run build` in vault_frontend — frontend compiles
4. Deploy backend upgrade (`dfx deploy rumi_protocol_backend --network ic`)
5. Deploy frontend (`dfx deploy vault_frontend --network ic`)
6. Test: call `set_recovery_parameters` via dfx to set a recovery borrowing fee
7. Verify via `get_collateral_config` that the new fields appear in the response

## Note
Interest rate accrual is **not yet implemented** — `interest_rate_apr` is a placeholder field. The `recovery_interest_rate_apr` override can be added now for completeness, but it won't have any runtime effect until accrual logic is built.
