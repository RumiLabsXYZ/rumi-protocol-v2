# Immutable Vault Terms — Design Sketch

## Overview

When a vault is opened, a snapshot of the current protocol parameters is stored with the vault. These "vault terms" act as a **user-favorable floor/ceiling** — the protocol always applies whichever value (global or vault-specific) is more beneficial to the vault owner.

This protects users from adverse parameter changes (even via SNS governance) while still allowing the protocol to loosen terms globally and have all vaults benefit automatically.

---

## Current State of the Codebase

**Current `Vault` struct** (`vault.rs:41-46`):
```rust
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    pub icp_margin_amount: ICP,
    pub vault_id: u64,
}
```

**Current parameter sources** (all global, no per-vault terms):
- **MCR / Liquidation Ratio**: Hardcoded constants `MINIMUM_COLLATERAL_RATIO` (133%) and `RECOVERY_COLLATERAL_RATIO` (150%), selected by `Mode` enum (`state.rs:67-73`)
- **Borrowing Fee**: `state.fee` field (a `Ratio`), set at init from `fee_e8s`. Zero in Recovery mode (`state.rs:337-343`)
- **Liquidation Bonus**: Hardcoded `1.1` (10%) in `liquidate_vault` and `1.111...` in `partial_liquidate_vault` (`vault.rs`)
- **Redemption Fee**: Dynamic — computed from `current_base_rate`, elapsed time, and redemption volume (`state.rs:325-335`)

---

## Proposed New Structs

### `VaultTerms` — Stored per vault at creation time

```rust
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct VaultTerms {
    /// Timestamp when vault was opened
    pub created_at: u64,
    
    /// Minimum collateral ratio at vault creation (e.g., 1.33 = 133%)
    /// Effective MCR can only be <= this value (decreases benefit the user)
    pub mcr_at_creation: Ratio,
    
    /// Liquidation ratio at vault creation (same as MCR in normal mode,
    /// but captures Recovery mode ratio if opened during recovery)
    /// Effective LR can only be <= this value
    pub lr_at_creation: Ratio,
    
    /// Liquidation penalty at vault creation (e.g., 0.10 = 10%)
    /// Effective penalty can only be <= this value
    pub liquidation_penalty_at_creation: Ratio,
    
    /// Borrowing fee at vault creation
    pub borrowing_fee_at_creation: Ratio,
    
    /// Maximum borrowing fee this vault can ever be charged
    /// Set to 2x borrowing_fee_at_creation (configurable multiplier)
    pub borrowing_fee_cap: Ratio,
    
    /// Protocol version at time of vault creation
    /// Useful for migration logic if terms structure changes
    pub terms_version: u8,
}
```

### Updated `Vault` struct

```rust
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    pub icp_margin_amount: ICP,
    pub vault_id: u64,
    /// Immutable terms snapshot from vault creation. 
    /// None for vaults created before this feature (legacy vaults).
    pub terms: Option<VaultTerms>,
}
```

### Updated `CandidVault` (for frontend/Candid interface)

```rust
#[derive(CandidType, Serialize, Deserialize, Debug)]
pub struct CandidVault {
    pub owner: Principal,
    pub borrowed_icusd_amount: u64,
    pub icp_margin_amount: u64,
    pub vault_id: u64,
    pub terms: Option<VaultTerms>,
}
```

---

## Effective Parameter Resolution

The core logic: **always apply whichever value is more favorable to the user.**

```rust
impl Vault {
    /// Get the effective MCR for this vault.
    /// Uses the LOWER of global MCR and vault's locked MCR (lower = more lenient).
    pub fn effective_mcr(&self, global_mcr: Ratio) -> Ratio {
        match &self.terms {
            Some(terms) => global_mcr.min(terms.mcr_at_creation),
            None => global_mcr, // Legacy vaults use global params
        }
    }
    
    /// Get the effective liquidation ratio for this vault.
    /// Uses the LOWER of global LR and vault's locked LR.
    pub fn effective_lr(&self, global_lr: Ratio) -> Ratio {
        match &self.terms {
            Some(terms) => global_lr.min(terms.lr_at_creation),
            None => global_lr,
        }
    }
    
    /// Get the effective liquidation penalty for this vault.
    /// Uses the LOWER of global penalty and vault's locked penalty.
    pub fn effective_liquidation_penalty(&self, global_penalty: Ratio) -> Ratio {
        match &self.terms {
            Some(terms) => global_penalty.min(terms.liquidation_penalty_at_creation),
            None => global_penalty,
        }
    }
    
    /// Get the effective borrowing fee for this vault.
    /// Uses the LOWER of global fee and vault's fee cap.
    pub fn effective_borrowing_fee(&self, global_fee: Ratio) -> Ratio {
        match &self.terms {
            Some(terms) => global_fee.min(terms.borrowing_fee_cap),
            None => global_fee,
        }
    }
}
```

---

## Terms Snapshot at Vault Creation

In `open_vault()`, capture current parameters when creating the vault:

```rust
// Inside open_vault(), after successful ICP transfer:
let vault_id = mutate_state(|s| {
    let vault_id = s.increment_vault_id();
    
    // Snapshot current terms
    let terms = VaultTerms {
        created_at: ic_cdk::api::time(),
        mcr_at_creation: s.mode.get_minimum_liquidation_collateral_ratio(),
        lr_at_creation: s.mode.get_minimum_liquidation_collateral_ratio(),
        liquidation_penalty_at_creation: Ratio::new(dec!(0.10)), // Current hardcoded 10%
        borrowing_fee_at_creation: s.get_borrowing_fee(),
        borrowing_fee_cap: s.get_borrowing_fee() * Ratio::new(dec!(2.0)), // 2x cap
        terms_version: 1,
    };
    
    record_open_vault(
        s,
        Vault {
            owner: caller,
            borrowed_icusd_amount: 0.into(),
            icp_margin_amount,
            vault_id,
            terms: Some(terms),
        },
        block_index,
    );
    vault_id
});
```

---

## Where Effective Terms Must Be Applied

These are the functions that currently read global parameters and need to be updated to use per-vault effective terms:

### 1. `borrow_from_vault()` — vault.rs ~line 233
**Currently:**
```rust
let max_borrowable_amount = vault.icp_margin_amount * icp_rate
    / read_state(|s| s.mode.get_minimum_liquidation_collateral_ratio());
```
**Should become:**
```rust
let (global_mcr, global_fee) = read_state(|s| (
    s.mode.get_minimum_liquidation_collateral_ratio(),
    s.get_borrowing_fee(),
));
let effective_mcr = vault.effective_mcr(global_mcr);
let max_borrowable_amount = vault.icp_margin_amount * icp_rate / effective_mcr;
// ...
let fee: ICUSD = amount * vault.effective_borrowing_fee(global_fee);
```

### 2. `check_vaults()` / liquidation eligibility — lib.rs ~line 166
**Currently:**
```rust
if compute_collateral_ratio(vault, last_icp_rate)
    < s.mode.get_minimum_liquidation_collateral_ratio()
```
**Should become:**
```rust
let global_lr = s.mode.get_minimum_liquidation_collateral_ratio();
let effective_lr = vault.effective_lr(global_lr);
if compute_collateral_ratio(vault, last_icp_rate) < effective_lr
```

### 3. `liquidate_vault()` — vault.rs ~line 568
**Currently:**
```rust
let liquidation_bonus = Ratio::new(dec!(1.1)); // 110% (10% bonus)
```
**Should become:**
```rust
let global_penalty = Ratio::new(dec!(0.10)); // Will become a state field
let effective_penalty = vault.effective_liquidation_penalty(global_penalty);
let liquidation_bonus = Ratio::new(Decimal::ONE + effective_penalty.to_decimal());
```

### 4. `partial_liquidate_vault()` — vault.rs ~line 1063
Same pattern as full liquidation.

### 5. Redemption logic in `redeem_icp()` — vault.rs ~line 68
Redemptions affect the lowest-CR vaults. The effective LR per vault determines which vaults are eligible and in what order. This is the most complex integration point — the `record_redemption_on_vaults` function in event.rs would need to consider per-vault terms when selecting targets.

---

## Global Parameters That Should Become State Fields

Currently some parameters are hardcoded constants. For the vault terms system to work properly, these need to become mutable state fields so governance can change them (with vault terms providing the user-favorable floor):

| Parameter | Current Location | Proposed State Field |
|---|---|---|
| MCR (normal) | `MINIMUM_COLLATERAL_RATIO` const (133%) | `s.minimum_collateral_ratio` |
| MCR (recovery) | `RECOVERY_COLLATERAL_RATIO` const (150%) | `s.recovery_collateral_ratio` |
| Liquidation penalty | Hardcoded `dec!(1.1)` in vault.rs | `s.liquidation_penalty` |
| Borrowing fee | `s.fee` (already a state field) ✓ | Already exists |

---

## Stable Memory / Upgrade Considerations

### Backward Compatibility
The `terms: Option<VaultTerms>` field handles migration cleanly:
- Existing vaults deserialized from stable memory will have `terms: None`
- All effective_* methods fall back to global params when `terms` is `None`
- No data migration required — legacy vaults just don't get the protection

### Serde Compatibility
Adding an `Option<T>` field to a struct that's serialized with serde is backward-compatible as long as you use `#[serde(default)]`:
```rust
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    pub icp_margin_amount: ICP,
    pub vault_id: u64,
    #[serde(default)]
    pub terms: Option<VaultTerms>,
}
```
This ensures vaults stored before this feature was added deserialize with `terms: None`.

---

## Voluntary Re-Lock Mechanism (Future)

Allow users to opt into current global terms if they're more favorable overall:

```rust
pub async fn relock_vault_terms(vault_id: u64) -> Result<VaultTerms, ProtocolError> {
    // Verify caller is vault owner
    // Snapshot current global params as new terms
    // Only allow if ALL new terms are at least as favorable as existing terms
    // (prevents users from gaming by selectively re-locking)
    // Update vault.terms with new snapshot
}
```

---

## What NOT to Lock

These should remain fully flexible (no per-vault terms):

- **Oracle parameters** — price feed sources, staleness thresholds
- **Debt ceilings** — systemic risk controls must remain responsive  
- **Recovery mode triggers** — protocol-wide safety mechanism
- **Minimum amounts** — `MIN_ICP_AMOUNT`, `MIN_ICUSD_AMOUNT` etc.
- **Redemption fee formula** — this is dynamic and algorithmic, locking it per-vault would break the peg mechanism. The redemption fee protects the *system*, not individual vaults.

---

## Open Questions

1. **Fee cap multiplier**: Is 2x the right cap for borrowing fee? Could also be a governance-settable multiplier stored in State.

2. **MCR vs LR distinction**: Currently these are the same value (selected by Mode). If you plan to separate them in the future (e.g., MCR for borrowing at 150%, LR for liquidation at 133%), the VaultTerms struct already captures both.

3. **Recovery mode interaction**: If a vault was opened during Recovery mode (150% MCR), should its locked MCR be 150% or the normal 133%? The current sketch captures whatever was active at creation time, meaning Recovery-mode vaults would have a *higher* locked MCR. This seems correct — they opened under stricter terms and the snapshot reflects that. When the protocol exits Recovery, the global MCR drops to 133% and their effective MCR becomes 133% (the min of 150% and 133%).

4. **Incremental mints**: When a user borrows more from an existing vault, should the additional debt use the original terms or current terms? Options:
   - **Option A**: Original terms apply to all debt in the vault (simpler, current sketch assumes this)
   - **Option B**: Track debt tranches with different terms (complex, probably not worth it)
   - **Option C**: Re-snapshot terms on each borrow, but only if new terms are less favorable (middle ground)

   Recommendation: **Option A** — keep it simple. The terms were the deal when they opened the vault.

5. **Event/audit trail**: Should term snapshots be recorded as events for on-chain auditability? Probably yes — add a `VaultTermsLocked` event type.
