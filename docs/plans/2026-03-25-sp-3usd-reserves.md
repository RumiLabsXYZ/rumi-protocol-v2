# SP 3USD Reserves Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the 3pool redeem-and-burn liquidation path with a simple ICRC-1 transfer of 3USD to the backend canister as protocol reserves.

**Architecture:** The stability pool currently burns 3USD through the 3pool to extract icUSD, then tells the backend the icUSD was burned. Instead, the SP will transfer 3USD directly to a dedicated subaccount on the backend canister, then tell the backend to write down the debt and credit the 3USD to protocol reserves. The backend tracks cumulative `protocol_3usd_reserves` for transparency.

**Tech Stack:** Rust (IC canisters), ICRC-1 ledger calls, Candid

---

## Step 1: Add `protocol_3usd_reserves` field to backend state + event support

**Files:**
- `src/rumi_protocol_backend/src/state.rs`
- `src/rumi_protocol_backend/src/event.rs`

**Why event support is needed:** The backend is event-sourced. State is rebuilt from scratch by replaying events in `post_upgrade`. If we only set `protocol_3usd_reserves` at runtime, it resets to 0 on the next canister upgrade. We must record it in the event log so replay reconstructs it.

**Code — state.rs:**

In `State` struct (after `reserve_redemption_fee` ~line 595):
```rust
/// Cumulative 3USD (LP tokens) received from stability pool liquidations (e8s).
/// These sit in subaccount hash("protocol_3usd_reserves") on the 3USD ledger.
pub protocol_3usd_reserves: u64,
```

In `From<InitArg> for State` (after `reserve_redemption_fee` init):
```rust
protocol_3usd_reserves: 0,
```

**Code — event.rs:**

Add an optional field to the existing `PartialLiquidateVault` event variant (~line 48-64). This follows the same pattern as `protocol_fee_collateral` — old events deserialize as `None`:
```rust
    #[serde(rename = "partial_liquidate_vault")]
    PartialLiquidateVault {
        vault_id: u64,
        #[serde(alias = "liquidated_debt")]
        liquidator_payment: ICUSD,
        #[serde(alias = "collateral_seized")]
        icp_to_liquidator: ICP,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        liquidator: Option<Principal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icp_rate: Option<UsdIcp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol_fee_collateral: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<u64>,
        /// 3USD (LP tokens) credited to protocol reserves during this liquidation.
        /// None for legacy burn-path liquidations; Some(amount_e8s) for reserves-path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        three_usd_reserves_e8s: Option<u64>,
    },
```

In the `replay()` function (~line 610-634), add reserves tracking to the `PartialLiquidateVault` handler. After the existing vault mutation block, add:
```rust
            Event::PartialLiquidateVault {
                vault_id,
                liquidator_payment,
                icp_to_liquidator,
                protocol_fee_collateral,
                three_usd_reserves_e8s,
                ..
            } => {
                // ... existing vault debt/collateral reduction code stays the same ...

                // Track 3USD reserves from stability pool liquidations
                if let Some(reserves_e8s) = three_usd_reserves_e8s {
                    state.protocol_3usd_reserves += reserves_e8s;
                }
            },
```

**Test:** `cargo build -p rumi_protocol_backend` compiles.

**Commit:** `feat(backend): add protocol_3usd_reserves state field + event support`

---

## Step 2: Add helper to compute the 3USD reserves subaccount

**Files:**
- `src/rumi_protocol_backend/src/management.rs` (or wherever subaccount helpers live)

**What:** A function that returns the 32-byte subaccount for protocol 3USD reserves: `sha256("protocol_3usd_reserves")`. This subaccount is used as the ICRC-1 `to.subaccount` when the SP transfers 3USD to the backend.

**Code:**
```rust
/// Deterministic subaccount for protocol-held 3USD reserves.
pub fn protocol_3usd_reserves_subaccount() -> [u8; 32] {
    ic_crypto_sha2::Sha256::hash(b"protocol_3usd_reserves")
}
```

Check what hashing crate is already used in the project — `ic_crypto_sha2`, `sha2`, or `ic_cdk::api::management_canister::main::raw_rand` won't work (that's async). Use whatever the project already depends on.

**Test:** `cargo build -p rumi_protocol_backend` compiles.

**Commit:** `feat(backend): add protocol_3usd_reserves subaccount helper`

---

## Step 3: Modify the backend's `liquidate_vault_debt_already_burned` to support reserves mode

**Files:**
- `src/rumi_protocol_backend/src/vault.rs` (~line 2354)
- `src/rumi_protocol_backend/src/main.rs` (~line 846)

**What:** The existing function `liquidate_vault_debt_already_burned` assumes icUSD was burned (supply reduced). We need it to also support the new flow where 3USD was transferred to reserves (supply NOT reduced, but reserves incremented).

Add a parameter `three_usd_received_e8s: Option<u64>` to distinguish:
- `None` → legacy path (icUSD was burned via 3pool, no reserves tracking)
- `Some(amount)` → new path (3USD transferred to reserves, increment `protocol_3usd_reserves`)

In `vault.rs` `liquidate_vault_debt_already_burned`, inside the `mutate_state` block:

1. Update the event recording (~line 2447) to include the new field:
```rust
let event = crate::event::Event::PartialLiquidateVault {
    vault_id,
    liquidator_payment: max_liquidatable_debt,
    icp_to_liquidator: collateral_to_liquidator,
    liquidator: Some(caller),
    icp_rate: Some(collateral_price_usd),
    protocol_fee_collateral: if protocol_cut > 0 { Some(protocol_cut) } else { None },
    timestamp: Some(ic_cdk::api::time()),
    three_usd_reserves_e8s: three_usd_received_e8s, // None for burn path, Some for reserves path
};
crate::storage::record_event(&event);
```

2. After recording the event, increment the runtime counter:
```rust
if let Some(three_usd_e8s) = three_usd_received_e8s {
    s.protocol_3usd_reserves += three_usd_e8s;
}
```

This ensures the reserves are tracked both at runtime AND persisted in the event log for replay on upgrade.

In `main.rs`, update the endpoint signature and pass the new param. Add a NEW endpoint for the reserves flow:

```rust
/// Called by the stability pool after transferring 3USD to backend reserves.
/// Writes down the vault's debt and releases proportional collateral to the SP.
/// Only callable by the registered stability pool canister.
#[update]
#[candid_method(update)]
async fn stability_pool_liquidate_with_reserves(
    vault_id: u64,
    icusd_debt_covered_e8s: u64,
    three_usd_transferred_e8s: u64,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_price_for_liquidation()?;
    let caller = ic_cdk::api::caller();

    let is_stability_pool = read_state(|s| {
        s.stability_pool_canister.map_or(false, |sp| sp == caller)
    });
    if !is_stability_pool {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }

    rumi_protocol_backend::vault::liquidate_vault_debt_already_burned(
        vault_id, icusd_debt_covered_e8s, caller, Some(three_usd_transferred_e8s)
    ).await
}
```

Update the existing `stability_pool_liquidate_debt_burned` to pass `None` for backwards compat:
```rust
rumi_protocol_backend::vault::liquidate_vault_debt_already_burned(vault_id, icusd_burned_e8s, caller, None).await
```

**Test:** `cargo build -p rumi_protocol_backend` compiles. Both endpoints exist.

**Commit:** `feat(backend): add stability_pool_liquidate_with_reserves endpoint`

---

## Step 4: Add query endpoint to expose protocol 3USD reserves

**Files:**
- `src/rumi_protocol_backend/src/main.rs`

**What:** A simple query so the frontend/dashboards can display protocol reserves.

```rust
#[query]
#[candid_method(query)]
fn get_protocol_3usd_reserves() -> u64 {
    read_state(|s| s.protocol_3usd_reserves)
}
```

**Test:** `cargo build -p rumi_protocol_backend` compiles.

**Commit:** `feat(backend): add get_protocol_3usd_reserves query`

---

## Step 5: Replace 3pool burn with ICRC-1 transfer in the stability pool

**Files:**
- `src/stability_pool/src/liquidation.rs` (lines 274-366 — the LP token section)

**What:** This is the core change. Replace the `authorized_redeem_and_burn` call to the 3pool with an ICRC-1 transfer of 3USD to the backend's reserves subaccount, then call the new `stability_pool_liquidate_with_reserves` endpoint.

Replace the entire LP token block (lines 274-366) with:

```rust
// --- LP tokens (3USD): transfer to backend reserves + debt write-down ---
for (token_ledger, amount) in &token_draw {
    let config = match stablecoin_configs.get(token_ledger) {
        Some(c) if c.is_lp_token.unwrap_or(false) => c,
        _ => continue,
    };

    // Calculate icUSD equivalent using cached virtual price
    let vp = read_state(|s| {
        s.virtual_prices().get(token_ledger).copied().unwrap_or(1_000_000_000_000_000_000)
    });
    let icusd_equiv_e8s = lp_to_usd_e8s(*amount, vp);

    if icusd_equiv_e8s < 10_000_000 {
        log!(INFO, "Skipping LP token {}: icUSD equivalent {} e8s below backend minimum", token_ledger, icusd_equiv_e8s);
        continue;
    }

    // Step A: Transfer 3USD to backend's protocol reserves subaccount
    let backend_reserves_subaccount = ic_crypto_sha2::Sha256::hash(b"protocol_3usd_reserves");

    let transfer_args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: protocol_id,
            subaccount: Some(backend_reserves_subaccount),
        },
        fee: None,
        created_at_time: Some(ic_cdk::api::time()),
        memo: None,
        amount: candid::Nat::from(*amount as u128),
    };

    let transfer_result: Result<(Result<candid::Nat, icrc_ledger_types::icrc1::transfer::TransferError>,), _> = call(
        *token_ledger, "icrc1_transfer", (transfer_args,)
    ).await;

    match transfer_result {
        Ok((Ok(block_index),)) => {
            log!(INFO, "3USD transfer succeeded: {} e8s → backend reserves (block {}), vault {}",
                amount, block_index, vault_info.vault_id);

            // Deduct from tracked balances — transfer succeeded
            mutate_state(|s| s.deduct_burned_lp_from_balances(*token_ledger, *amount));

            // Deduct transfer fee from tracked balances
            if let Some(fee) = config.transfer_fee {
                if fee > 0 {
                    mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, fee));
                }
            }

            // Step B: Tell backend to write down debt and credit reserves
            // Retry up to 3 times — no additional cost, 3USD already transferred
            let mut write_down_succeeded = false;
            for attempt in 0..3 {
                let liq_result: Result<(Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,), _> = call(
                    protocol_id,
                    "stability_pool_liquidate_with_reserves",
                    (vault_info.vault_id, icusd_equiv_e8s, *amount),
                ).await;

                match liq_result {
                    Ok((Ok(success),)) => {
                        actual_consumed.insert(*token_ledger, *amount);
                        total_collateral_gained += success.collateral_received;
                        log!(INFO, "Backend reserves liquidation succeeded for vault {}: {} collateral, {} 3USD transferred (attempt {})",
                            vault_info.vault_id, success.collateral_received, amount, attempt + 1);
                        write_down_succeeded = true;
                        break;
                    }
                    Ok((Err(e),)) => {
                        log!(INFO, "Backend rejected reserves liquidation for vault {} (attempt {}): {:?}",
                            vault_info.vault_id, attempt + 1, e);
                        // Don't retry on explicit rejection — backend said no
                        break;
                    }
                    Err(e) => {
                        log!(INFO, "Backend call failed for reserves liquidation vault {} (attempt {}): {:?}",
                            vault_info.vault_id, attempt + 1, e);
                        // Retry on inter-canister call failure
                    }
                }
            }

            if !write_down_succeeded {
                log!(INFO, "WARN: 3USD transferred to backend reserves but debt write-down failed for vault {}. \
                    3USD is safe in backend subaccount. Will be accounted on next successful call.", vault_info.vault_id);
            }

            // One token per vault per round
            break;
        }
        Ok((Err(e),)) => {
            // Transfer failed — no 3USD left the pool, safe to skip
            log!(INFO, "3USD transfer failed for vault {}: {:?}", vault_info.vault_id, e);
        }
        Err(e) => {
            log!(INFO, "3USD transfer call failed for vault {}: {:?}", vault_info.vault_id, e);
        }
    }
}
```

**Key differences from old code:**
- No `authorized_redeem_and_burn` call to 3pool
- Simple ICRC-1 transfer instead
- Calls new `stability_pool_liquidate_with_reserves` endpoint (not `stability_pool_liquidate_debt_burned`)
- Retry logic (up to 3 attempts) for the write-down call only on inter-canister failures
- No retry on explicit backend rejection
- Transfer fee deducted from pool tracking after successful transfer

**Dependencies:** The SP canister needs `ic_crypto_sha2` (or whatever hash crate) to compute the subaccount. Check if it's already a dependency; if not, add it to `src/stability_pool/Cargo.toml`. Alternatively, hardcode the hash since it's a constant — compute `sha256("protocol_3usd_reserves")` once and use the literal bytes.

**Test:** `cargo build -p stability_pool` compiles.

**Commit:** `feat(stability-pool): replace 3pool burn with ICRC-1 transfer to backend reserves`

---

## Step 6: Verify both canisters compile together

**Command:**
```bash
cargo build
```

Fix any cross-crate type mismatches (e.g., the SP needs the `StabilityPoolLiquidationResult` type from the backend, and the new endpoint name must match).

**Commit:** `fix: resolve cross-crate compilation issues` (if needed)

---

## Step 7: Local replica testing

**What:** Deploy to local replica, create a vault, make it undercollateralized, trigger liquidation via stability pool. Verify:

1. Vault is closed and debt is written off
2. 3USD moves from stability pool to backend canister's reserves subaccount (check 3USD ledger balances)
3. No `authorized_redeem_and_burn` call is made to the 3pool
4. No icUSD burn call is made
5. Stability pool depositor receives seized collateral
6. `get_protocol_3usd_reserves` returns the correct amount
7. 3pool TVL is unchanged after liquidation

**Commit:** None (testing step)

---

## Notes

- The old `stability_pool_liquidate_debt_burned` endpoint stays for backwards compatibility during the rolling upgrade window (SP upgrades before/after backend). Once both are deployed, it becomes dead code but harmless.
- The `protocol_3usd_reserves` field survives upgrades because it's reconstructed during event replay — the `three_usd_reserves_e8s` field on `PartialLiquidateVault` events is summed up during `replay()`. Old events have `None` (serde default), so they contribute 0.
- The conversion uses the same virtual price mechanism already in place (`cached_virtual_prices` fetched every 5 min from 3pool's `get_pool_status`).
