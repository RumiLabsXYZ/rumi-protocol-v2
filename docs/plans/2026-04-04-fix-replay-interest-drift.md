# Fix Replay Interest Drift — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Prevent canister upgrades from inflating vault debts via non-deterministic interest re-accrual, and correct the current drift.

**Architecture:** Replace pure event-sourced state reconstruction with stable memory serialization. During `pre_upgrade`, serialize the full `State` to a dedicated stable memory region. During `post_upgrade`, deserialize it directly instead of replaying ~11k events. Keep event replay as a fallback for the very first upgrade after this change (when no serialized state exists yet). Add an admin endpoint to correct currently-drifted vault debts.

**Tech Stack:** Rust, `ic_stable_structures` (MemoryManager, VirtualMemory), `ciborium` (CBOR serialization — already used for events), `serde` (already used for most types).

---

## Why This Happened

The canister has no `pre_upgrade` hook. All state lives on the heap and is rebuilt from scratch by replaying the entire event log in `post_upgrade`. The `AccrueInterest` events only store a timestamp — the actual interest rate is recomputed during replay using the current rate curves and `last_price`. Since prices change over time, replay produces different interest amounts than original execution, inflating vault debts by ~5-7%.

## Why Stable Memory Serialization Fixes It

By serializing the live `State` to stable memory before upgrade and restoring it after, we bypass event replay entirely. The state is preserved exactly as it was — no recomputation, no drift. Event replay becomes a fallback for disaster recovery only.

---

## Part A: Add `Serialize`/`Deserialize` to `State`

Most component types (`Vault`, `CollateralConfig`, `Mode`, `Ratio`, `ICUSD`, `ICP`, etc.) already derive `Serialize`/`Deserialize`. The `State` struct itself does not.

### Task A1: Add serde derives to `State`

**File:** `src/rumi_protocol_backend/src/state.rs`

Add `#[derive(serde::Serialize, serde::Deserialize)]` to `pub struct State`. Add `#[serde(default)]` to fields that may not exist in older serialized state (future-proofing for when we add new fields).

Check that all field types implement `Serialize`/`Deserialize`:
- `BTreeMap<K, V>` — yes, if K and V do
- `BTreeSet<T>` — yes, if T does
- `Option<T>` — yes
- `Principal` — yes (via `candid`)
- `Ratio`, `ICUSD`, `ICP`, `UsdIcp` — check, add if missing
- `Mode`, `OperationState` — check, add if missing
- `CollateralConfig` — already has it
- `Vault` — already has it (line 54)
- `RateCurve`, `RateCurveV2`, `RecoveryRateMarker` — check, add if missing
- `InterestRecipient` — check, add if missing
- `BotClaim`, `PendingMarginTransfer` — check, add if missing
- `Vec<(u64, Principal)>` — yes

Run `cargo check` after adding the derive. Fix any missing trait impls.

### Task A2: Verify it compiles

```bash
cd src/rumi_protocol_backend && cargo check 2>&1
```

Fix any compilation errors from missing `Serialize`/`Deserialize` impls on nested types.

---

## Part B: Add Stable Memory State Storage

### Task B1: Add a new MemoryId for state serialization

**File:** `src/rumi_protocol_backend/src/storage.rs`

```rust
const STATE_MEMORY_ID: MemoryId = MemoryId::new(4);
```

Add two new public functions:

```rust
/// Serializes the full State to stable memory (called in pre_upgrade).
pub fn save_state_to_stable(state: &crate::state::State) {
    let bytes = {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(state, &mut buf)
            .expect("failed to serialize State to CBOR");
        buf
    };
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_MEMORY_ID);
        let len = bytes.len() as u64;
        // Write length prefix (8 bytes) + data
        let len_bytes = len.to_le_bytes();
        ic_stable_structures::writer::Writer::new(&mut mem, 0)
            .write_all(&len_bytes)
            .expect("failed to write state length");
        ic_stable_structures::writer::Writer::new(&mut mem, 8)
            .write_all(&bytes)
            .expect("failed to write state data");
    });
}

/// Attempts to restore State from stable memory. Returns None if no state was saved.
pub fn load_state_from_stable() -> Option<crate::state::State> {
    MEMORY_MANAGER.with(|m| {
        let mem = m.borrow().get(STATE_MEMORY_ID);
        // Read length prefix
        let mut len_bytes = [0u8; 8];
        // ... read len_bytes from memory offset 0 ...
        let len = u64::from_le_bytes(len_bytes);
        if len == 0 {
            return None; // No state saved yet
        }
        let mut buf = vec![0u8; len as usize];
        // ... read buf from memory offset 8 ...
        match ciborium::de::from_reader::<crate::state::State, _>(buf.as_slice()) {
            Ok(state) => Some(state),
            Err(e) => {
                ic_cdk::println!("Failed to deserialize state from stable memory: {:?}", e);
                None // Fall back to event replay
            }
        }
    })
}
```

**Note:** The exact stable memory read/write API depends on `ic_stable_structures` version. Use `StableWriter`/`StableReader` or raw memory operations as appropriate. Check the crate docs during implementation.

### Task B2: Add `pre_upgrade` to `main.rs`

**File:** `src/rumi_protocol_backend/src/main.rs`

Add before `post_upgrade`:

```rust
#[pre_upgrade]
fn pre_upgrade() {
    use rumi_protocol_backend::storage::save_state_to_stable;

    read_state(|state| {
        save_state_to_stable(state);
    });

    log!(INFO, "[pre_upgrade]: state serialized to stable memory");
}
```

### Task B3: Modify `post_upgrade` to try stable memory first

**File:** `src/rumi_protocol_backend/src/main.rs`

Change the state restoration logic in `post_upgrade`:

```rust
// Try to restore from stable memory (fast path, no drift)
let state = match rumi_protocol_backend::storage::load_state_from_stable() {
    Some(mut state) => {
        log!(INFO, "[upgrade]: restored state from stable memory (skipped event replay)");

        // Still need to process the Upgrade event we just recorded
        // Apply any upgrade-specific config changes here if needed
        state
    }
    None => {
        // Fallback: replay events (first upgrade after this change, or recovery)
        log!(INFO, "[upgrade]: no stable state found, replaying {} events", count_events());
        replay(events()).unwrap_or_else(|e| {
            ic_cdk::trap(&format!("[upgrade]: failed to replay: {:?}", e))
        })
    }
};
```

Keep all the post-replay migrations (last_accrual_time, bot_allowed_collateral_types, empty vault cleanup, etc.) — they should still run regardless of restoration path.

### Task B4: Verify compilation

```bash
cd src/rumi_protocol_backend && cargo check 2>&1
```

---

## Part C: One-Time Debt Correction

### Task C1: Add admin endpoint `admin_correct_vault_debts`

**File:** `src/rumi_protocol_backend/src/main.rs`

```rust
#[derive(CandidType, Deserialize)]
struct VaultDebtCorrection {
    vault_id: u64,
    correct_borrowed_e8s: u64,
    correct_accrued_interest_e8s: u64,
}

#[update]
#[candid_method(update)]
fn admin_correct_vault_debts(corrections: Vec<VaultDebtCorrection>) -> Result<String, ProtocolError> {
    let caller = ic_cdk::api::caller();
    read_state(|s| {
        if caller != s.developer_principal {
            return Err(ProtocolError::NotController);
        }
        Ok(())
    })?;

    let mut results = Vec::new();
    mutate_state(|s| {
        for c in &corrections {
            if let Some(vault) = s.vault_id_to_vaults.get_mut(&c.vault_id) {
                let old_borrowed = vault.borrowed_icusd_amount.0;
                let old_accrued = vault.accrued_interest.0;
                vault.borrowed_icusd_amount = ICUSD::new(c.correct_borrowed_e8s);
                vault.accrued_interest = ICUSD::new(c.correct_accrued_interest_e8s);
                results.push(format!(
                    "vault#{}: borrowed {}→{}, accrued {}→{}",
                    c.vault_id, old_borrowed, c.correct_borrowed_e8s,
                    old_accrued, c.correct_accrued_interest_e8s
                ));
            } else {
                results.push(format!("vault#{}: NOT FOUND", c.vault_id));
            }
        }
    });

    Ok(results.join("\n"))
}
```

### Task C2: Compute correct debt values

Use `dfx` to query current vault states and compute corrections:

1. Get all vaults: `dfx canister call rumi_protocol_backend get_vaults '(null)' --network ic`
2. For each vault with debt, compute expected debt:
   - Walk event log for that vault: sum borrows, subtract repays/liquidations/redemptions = **net principal**
   - Add legitimate interest: `principal × rate × time_since_last_correct_accrual / NANOS_PER_YEAR`
   - The "last correct accrual" is the timestamp of the last AccrueInterest event BEFORE the upgrade (event ~11153)
3. Exclude vault 44 (already liquidated, dust only — leave as-is)
4. Call `admin_correct_vault_debts` with the corrections

### Task C3: Record correction event

**File:** `src/rumi_protocol_backend/src/event.rs`

Add a new event variant:

```rust
#[serde(rename = "admin_debt_correction")]
AdminDebtCorrection {
    vault_id: u64,
    old_borrowed: u64,
    new_borrowed: u64,
    old_accrued: u64,
    new_accrued: u64,
    timestamp: Option<u64>,
}
```

Record this event in the admin endpoint so the correction is in the event log for auditability. Add a replay handler that applies the correction.

---

## Part D: Testing

### Task D1: Unit test — State serialization round-trip

**File:** `src/rumi_protocol_backend/tests/tests.rs` or a new test file

```rust
#[test]
fn test_state_serialization_roundtrip() {
    let state = create_test_state_with_vaults();
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).unwrap();
    let restored: State = ciborium::de::from_reader(buf.as_slice()).unwrap();
    assert_eq!(restored.vault_id_to_vaults.len(), state.vault_id_to_vaults.len());
    // Compare key vault fields
    for (id, vault) in &state.vault_id_to_vaults {
        let rv = &restored.vault_id_to_vaults[id];
        assert_eq!(vault.borrowed_icusd_amount, rv.borrowed_icusd_amount);
        assert_eq!(vault.accrued_interest, rv.accrued_interest);
        assert_eq!(vault.collateral_amount, rv.collateral_amount);
    }
}
```

### Task D2: PocketIC test — upgrade preserves vault state exactly

**File:** `src/rumi_protocol_backend/tests/pocket_ic_tests.rs`

```rust
#[test]
fn test_upgrade_preserves_vault_state() {
    // 1. Deploy canister, open vault, borrow, let interest accrue
    // 2. Record vault state (borrowed_icusd_amount, accrued_interest)
    // 3. Upgrade canister
    // 4. Query vault state again
    // 5. Assert borrowed_icusd_amount and accrued_interest are IDENTICAL
}
```

This is the key regression test — it proves that upgrades no longer cause drift.

### Task D3: Test admin correction endpoint

```rust
#[test]
fn test_admin_correct_vault_debts() {
    // 1. Deploy, open vault, borrow
    // 2. Call admin_correct_vault_debts with new values
    // 3. Query vault, verify corrected values
    // 4. Verify non-admin caller is rejected
}
```

---

## Part E: Deploy & Correct

### Task E1: Build and deploy

```bash
dfx build rumi_protocol_backend --network ic
dfx canister install rumi_protocol_backend --mode upgrade --network ic \
  --argument '(variant { Upgrade = record {
    mode = null;
    description = opt "Add pre_upgrade state serialization to prevent replay interest drift. Add admin debt correction endpoint."
  } })'
```

**IMPORTANT:** This is the LAST deploy that will use event replay. After this, all future upgrades restore from stable memory.

### Task E2: Compute and apply corrections

1. Query all vaults and their current debt values
2. Compute correct values (script or manual calculation)
3. Call `admin_correct_vault_debts` with corrections
4. Verify corrected CRs match expected (~150% for ICP vaults)

### Task E3: Verify

- Check all vault CRs on the liquidations page
- Confirm ICP vaults are back near 150% (or wherever they should be given current ICP price + legitimate interest)
- Check that vault 44 was excluded / left as-is

---

## Execution Order

1. **A1-A2**: Make State serializable
2. **B1-B4**: Add stable memory state storage + pre/post upgrade
3. **C1, C3**: Add admin correction endpoint + event
4. **D1-D3**: Tests
5. **E1-E3**: Deploy and correct

## Excluded from scope (vault 44)

Vault 44 has already been liquidated (`collateral_amount = 0`, `borrowed_icusd_amount = 1` dust). It will be excluded from corrections. The existing empty-vault cleanup in `post_upgrade` will remove it. Rob (the vault owner) has confirmed no action needed — the funds went to protocol reserves.
