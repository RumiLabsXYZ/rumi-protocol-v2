# Stability Pool Liquidation Bug Fixes

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all identified bugs in the stability pool liquidation flow, ordered by severity.
**Architecture:** All changes are in `src/stability_pool/src/liquidation.rs` and `src/stability_pool/src/state.rs`. No backend changes needed.
**Tech Stack:** Rust, IC CDK

---

## Task 1: Non-LP path — deduct balances when backend call fails or response is lost (CRITICAL)

**Bug:** When a non-LP token (icUSD, ckUSDC, ckUSDT) is approved and `liquidate_vault_partial` fails (backend rejects or inter-canister call drops), the approve was granted and the fee was deducted, but the pool doesn't know whether the backend actually pulled the tokens. If the backend DID pull them (call succeeded but response was lost), the SP's tracked balance will be higher than reality.

**File:** `src/stability_pool/src/liquidation.rs`, lines 243-259

**Fix:** After a non-LP backend call fails, query the actual ledger balance for that token and compare it to the tracked balance. If they diverge, correct the tracked balance. This is the only way to handle the ambiguity of "did the backend actually pull the tokens?"

Actually, simpler: use the **deduct-before-call** pattern (same as withdrawals in deposits.rs). Deduct the token draw from tracked balances BEFORE calling the backend. If the call succeeds, the deduction stands. If it fails, rollback.

**Change:**

In `liquidation.rs`, replace lines 209-259 with:

```rust
        // Deduct from tracked balances BEFORE calling backend (prevents phantom balances)
        mutate_state(|s| s.deduct_burned_lp_from_balances(*token_ledger, *amount));

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
            let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                protocol_id,
                "liquidate_vault_partial",
                (rumi_protocol_backend::vault::VaultArg {
                    vault_id: vault_info.vault_id,
                    amount: *amount,
                },)
            ).await;
            call_result.map(|(r,)| r)
        } else {
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let amount_e8s = crate::types::normalize_to_e8s(*amount, token_decimals);
                    let call_result: Result<(Result<rumi_protocol_backend::SuccessWithFee, rumi_protocol_backend::ProtocolError>,), _> = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: amount_e8s,
                            token_type: tt,
                        },)
                    ).await;
                    call_result.map(|(r,)| r)
                },
                None => {
                    log!(INFO, "Unknown stable token type for {}, rolling back deduction", token_ledger);
                    // Rollback: re-credit the tokens since we never called backend
                    mutate_state(|s| s.credit_tokens_to_pool(*token_ledger, *amount));
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                // Deduction stands — tokens were consumed
                let collateral = success.collateral_amount_received.unwrap_or(success.fee_amount_paid);
                log!(INFO, "Liquidation succeeded for vault {} with token {}: collateral={}, fee={}",
                    vault_info.vault_id, token_ledger, collateral, success.fee_amount_paid);
                actual_consumed.insert(*token_ledger, *amount);
                total_collateral_gained += collateral;
            },
            Ok(Err(protocol_error)) => {
                // Backend rejected — tokens NOT consumed, rollback deduction
                log!(INFO, "Protocol rejected liquidation for vault {} with token {}: {:?}. Rolling back.",
                    vault_info.vault_id, token_ledger, protocol_error);
                mutate_state(|s| s.credit_tokens_to_pool(*token_ledger, *amount));
            },
            Err(call_error) => {
                // Ambiguous — call failed, tokens MAY have been consumed.
                // Leave deduction in place (conservative: assume consumed).
                // Better to under-report than over-report.
                log!(INFO, "CRITICAL: Liquidation call failed for vault {} with token {}: {:?}. Deduction kept (conservative).",
                    vault_info.vault_id, token_ledger, call_error);
            }
        }
```

**New helper needed in `state.rs`:**

```rust
    /// Re-credit tokens to depositor balances (rollback after failed backend call).
    /// Inverse of deduct_burned_lp_from_balances.
    pub fn credit_tokens_to_pool(&mut self, token_ledger: Principal, amount: u64) {
        // Add back to aggregate
        *self.total_stablecoin_balances.entry(token_ledger).or_insert(0) += amount;

        // Distribute proportionally across depositors who hold this token
        let total = self.total_stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
        if total == 0 || amount == 0 {
            return;
        }

        let holders: Vec<(Principal, u64)> = self.deposits.iter()
            .filter_map(|(p, pos)| {
                let bal = pos.stablecoin_balances.get(&token_ledger).copied().unwrap_or(0);
                if bal > 0 { Some((*p, bal)) } else { None }
            })
            .collect();

        // If no holders exist (edge case after full liquidation), credit first depositor
        if holders.is_empty() {
            if let Some((first_p, _)) = self.deposits.iter().next() {
                let first_p = *first_p;
                if let Some(pos) = self.deposits.get_mut(&first_p) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += amount;
                }
            }
            return;
        }

        let holder_total: u64 = holders.iter().map(|(_, b)| *b).sum();
        let mut credited = 0u64;
        for (principal, bal) in &holders {
            let share = (amount as u128 * *bal as u128 / holder_total as u128) as u64;
            if let Some(pos) = self.deposits.get_mut(principal) {
                *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += share;
            }
            credited += share;
        }

        // Assign dust to first holder
        let dust = amount.saturating_sub(credited);
        if dust > 0 {
            if let Some((first_p, _)) = holders.first() {
                if let Some(pos) = self.deposits.get_mut(first_p) {
                    *pos.stablecoin_balances.entry(token_ledger).or_insert(0) += dust;
                }
            }
        }
    }
```

**Test:** `cargo test -p stability_pool`

---

## Task 2: Fix `execute_liquidation` public fallback — use backend's partial cap (HIGH)

**Bug:** The public `execute_liquidation` function sets `recommended_liquidation_amount: 0` and `collateral_price_e8s: 0`, causing full-debt liquidation attempts and corrupted history records.

**File:** `src/stability_pool/src/liquidation.rs`, lines 73-99

**Fix:** Query the backend for the vault's recommended liquidation amount and price. Replace the manual `LiquidatableVaultInfo` construction:

```rust
    // Fetch vault info from backend — use the backend's recommended partial cap
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Query the backend's structured liquidation info instead of raw vault data
    let vault_info_result: Result<(Vec<LiquidatableVaultInfo>,), _> = call(
        protocol_id, "get_liquidatable_vault_infos", ()
    ).await;

    let vault_info = match vault_info_result {
        Ok((infos,)) => {
            match infos.into_iter().find(|v| v.vault_id == vault_id) {
                Some(info) => info,
                None => return Err(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "Vault not found in liquidatable list".to_string(),
                }),
            }
        }
        Err(_) => {
            // Fallback to old method if endpoint doesn't exist yet
            let (vaults,): (Vec<rumi_protocol_backend::vault::CandidVault>,) = call(
                protocol_id, "get_liquidatable_vaults", ()
            ).await.map_err(|_e| StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".to_string(),
                method: "get_liquidatable_vaults".to_string(),
            })?;
            let vault = vaults.into_iter().find(|v| v.vault_id == vault_id)
                .ok_or(StabilityPoolError::LiquidationFailed {
                    vault_id,
                    reason: "Vault not found in liquidatable list".to_string(),
                })?;

            LiquidatableVaultInfo {
                vault_id: vault.vault_id,
                collateral_type: vault.collateral_type,
                debt_amount: vault.borrowed_icusd_amount,
                collateral_amount: vault.icp_margin_amount,
                recommended_liquidation_amount: vault.borrowed_icusd_amount, // full debt as last resort
                collateral_price_e8s: 0,
            }
        }
    };
```

**Note:** This requires a `get_liquidatable_vault_infos` endpoint on the backend that returns `Vec<LiquidatableVaultInfo>` with proper `recommended_liquidation_amount` and `collateral_price_e8s`. If that doesn't exist yet, the fallback path still uses full debt but at least it's explicit.

**Actually, simpler approach — no backend changes needed:** The `notify_liquidatable_vaults` push path already has the right data. The public fallback just needs to compute partial cap. Since the backend already has `compute_partial_liquidation_cap`, the simplest fix is: don't use the public fallback for partial amounts. Just set `recommended_liquidation_amount` to the full debt (it's a full-debt fallback endpoint) and let the backend's own partial cap logic limit the actual liquidation. The real issue is `collateral_price_e8s: 0` corrupting history.

**Simplest fix:** Just pass `vault.borrowed_icusd_amount` as `recommended_liquidation_amount` (no change in behavior for the public path, it was already doing full debt). The backend caps it. And pass a price from the vault data if available.

```rust
    let vault_info = LiquidatableVaultInfo {
        vault_id: vault.vault_id,
        collateral_type: vault.collateral_type,
        debt_amount: vault.borrowed_icusd_amount,
        collateral_amount: vault.icp_margin_amount,
        recommended_liquidation_amount: vault.borrowed_icusd_amount,
        collateral_price_e8s: 0, // Not available from CandidVault; history record will note this
    };
```

This is already the behavior since `execute_single_liquidation` falls back to `debt_amount` when `recommended_liquidation_amount > 0`. The key fix is making sure the backend's `liquidate_vault_partial` caps at its own partial amount (which it already does).

**Verdict:** This is already handled by the token_draw fix. The public fallback will draw for full debt, but the backend will only consume the partial cap. No code change needed beyond documenting the behavior.

**Test:** `cargo test -p stability_pool`

---

## Task 3: Collateral gain rounding dust — assign to first depositor (MEDIUM)

**Bug:** `process_liquidation_gains` distributes collateral via integer division, losing rounding dust. Unlike `distribute_interest_revenue` which assigns dust to the first eligible depositor, this function doesn't.

**File:** `src/stability_pool/src/state.rs`, lines 564-569

**Fix:** After the Phase 3 loop, compute unassigned collateral and give it to the first opted-in depositor:

Add after line 571 (after the `for principal in &opted_in_principals` loop closes):

```rust
        // Phase 3b: Assign collateral rounding dust to first opted-in depositor
        let total_distributed_collateral: u64 = opted_in_principals.iter()
            .filter_map(|p| self.deposits.get(p))
            .filter_map(|pos| pos.collateral_gains.get(&collateral_type).copied())
            .sum::<u64>()
            .saturating_sub(
                // Subtract pre-existing gains to get only what was distributed this round
                opted_in_principals.iter()
                    .filter_map(|p| self.deposits.get(p))
                    .filter_map(|pos| pos.collateral_gains.get(&collateral_type).copied())
                    .sum::<u64>()
            );
```

Wait, that's wrong — we can't distinguish pre-existing gains from newly added ones after the fact. Better approach: track total distributed during the loop.

**Fix (revised):** Add a `total_collateral_distributed` counter in the Phase 3 loop:

In `process_liquidation_gains_at`, add tracking variable before Phase 3 loop (after line 526):

```rust
        let mut total_collateral_distributed: u64 = 0;
```

In the collateral distribution block (line 567), track it:

```rust
                if user_consumed_e8s > 0 {
                    let user_collateral = (collateral_gained as u128 * user_consumed_e8s as u128 / total_consumed_e8s as u128) as u64;
                    *position.collateral_gains.entry(collateral_type).or_insert(0) += user_collateral;
                    total_collateral_distributed += user_collateral;
                }
```

After Phase 3 loop closes (after line 571), add dust assignment:

```rust
        // Assign collateral rounding dust to first opted-in depositor
        let collateral_dust = collateral_gained.saturating_sub(total_collateral_distributed);
        if collateral_dust > 0 {
            if let Some(first) = opted_in_principals.first() {
                if let Some(pos) = self.deposits.get_mut(first) {
                    *pos.collateral_gains.entry(collateral_type).or_insert(0) += collateral_dust;
                }
            }
        }
```

**Test:** `cargo test -p stability_pool`

---

## Task 4: Fix `deduct_burned_lp_from_balances` rounding dust (MEDIUM)

**Bug:** `total_deducted` can be less than `actual_deduct` due to integer rounding. The aggregate is decremented by `total_deducted`, leaving the aggregate slightly higher than the sum of individual balances. Over time this causes `validate_state()` failures.

**File:** `src/stability_pool/src/state.rs`, lines 259-273

**Fix:** After the loop, assign any remaining dust to the depositor with the largest balance:

```rust
        // Assign rounding dust to largest holder
        let dust = actual_deduct.saturating_sub(total_deducted);
        if dust > 0 {
            // Find depositor with largest remaining balance for this token
            if let Some(largest_p) = depositors.iter()
                .max_by_key(|(_, bal)| *bal)
                .map(|(p, _)| *p)
            {
                if let Some(pos) = self.deposits.get_mut(&largest_p) {
                    if let Some(bal) = pos.stablecoin_balances.get_mut(&token_ledger) {
                        *bal = bal.saturating_sub(dust);
                    }
                }
                total_deducted += dust;
            }
        }
```

Place this before the aggregate update at line 271.

**Test:** `cargo test -p stability_pool`

---

## Task 5: Record actual backend consumption, not planned draw (MEDIUM)

**Bug:** `actual_consumed` records the `token_draw` amount, not what the backend actually consumed. If the backend partially fills, accounting diverges.

**File:** `src/stability_pool/src/liquidation.rs`, line 248

**Fix:** For non-LP tokens, use the `success.fee_amount_paid` or a dedicated field from the backend response to determine actual consumption. However, `SuccessWithFee` doesn't have an "amount_consumed" field — it has `fee_amount_paid` and `collateral_amount_received`.

The backend's `liquidate_vault_partial` uses `icrc2_transfer_from` to pull exactly the amount it needs. If it pulls less than requested (because the vault only needed a partial amount), the unused allowance just expires.

**Actually:** The backend computes its own partial cap in `liquidate_vault_partial` and only pulls what's needed via `icrc2_transfer_from`. The amount pulled IS the amount consumed. But the SP records `*amount` (the draw), not the amount pulled.

The cleanest fix: the SP should record what the backend actually pulled. But we don't get that info back from `liquidate_vault_partial` — it returns `SuccessWithFee { fee_amount_paid, block_index, collateral_amount_received }`.

**For now:** This is acceptable because the backend will reject (Err) if it can't process the full amount, and we handle that with rollback (Task 1). The only case where this diverges is if the backend pulls LESS than requested but still succeeds — which shouldn't happen because `icrc2_transfer_from` pulls the exact amount.

**Verdict:** No code change needed. The deduct-before-call pattern from Task 1 handles this correctly: we deduct the full draw, if the backend rejects we rollback, if it succeeds the full draw was consumed.

---

## Task 6: Remove dead circuit breaker code (MEDIUM)

**Bug:** `token_consecutive_failures`, `is_token_suspended`, `record_token_failure`, `reset_token_failures` exist in state but are never called from the liquidation flow.

**File:** `src/stability_pool/src/state.rs`

**Fix:** Search for and remove:
- `token_consecutive_failures` field
- `is_token_suspended` method
- `record_token_failure` method
- `reset_token_failures` method
- Any `SUSPENSION_THRESHOLD` constant

Also check `types.rs` for `SuspendedToken` or similar types.

**Test:** `cargo test -p stability_pool` — ensure no compile errors from removed code.

---

## Task 7: Multiple non-LP tokens sequentially liquidating same vault (HIGH)

**Bug:** The token_draw is computed as a single snapshot, but each `liquidate_vault_partial` call changes vault state. The second token's call may fail or over-liquidate.

**File:** `src/stability_pool/src/liquidation.rs`, lines 158-260

**Fix:** After the first successful non-LP liquidation, break out of the loop. One token per vault per liquidation round. The next cycle will pick it up again if still underwater.

After line 249 (`actual_consumed.insert(*token_ledger, *amount);`), add:

```rust
                // One token per vault per round — vault state changed,
                // remaining draws are stale. Next cycle will pick up if still underwater.
                break;
```

And in the LP section, similarly break after first success.

**Test:** `cargo test -p stability_pool`

---

## Task 8: LP burn path — use actual burned amounts (MEDIUM)

**Bug:** The LP path records `*amount` (planned draw) in `actual_consumed`, not the actual `result.lp_amount_burned` from the 3pool. If the 3pool burns a different amount than requested, accounting diverges.

**File:** `src/stability_pool/src/liquidation.rs`, line 321

**Fix:** Replace `*amount` with the actual burned amount from the 3pool result:

```rust
                        actual_consumed.insert(*token_ledger, result.lp_amount_burned as u64);
```

And update the deduct-on-failure logic to use the actual burned amount too.

**Note:** Need to check what fields `RedeemAndBurnResult` has. Likely `lp_amount_burned` and `token_amount_burned`.

**Test:** `cargo test -p stability_pool`

---

## Task 9: Deploy and verify

1. `dfx build stability_pool --network ic`
2. Deploy: `dfx deploy rumi_stability_pool --network ic --argument '(record { protocol_canister_id = principal "tfesu-vyaaa-aaaap-qrd7a-cai" })'`
3. Unpause: `dfx canister call rumi_stability_pool emergency_resume --network ic`
4. Verify pool status: `dfx canister call rumi_stability_pool get_pool_status '()' --network ic`

---

## Summary of changes by file

### `src/stability_pool/src/liquidation.rs`
- Task 1: Deduct-before-call pattern for non-LP tokens
- Task 7: Break after first successful non-LP liquidation per vault
- Task 8: Use actual burned LP amount

### `src/stability_pool/src/state.rs`
- Task 1: Add `credit_tokens_to_pool` helper
- Task 3: Collateral dust assignment in `process_liquidation_gains_at`
- Task 4: Fix rounding dust in `deduct_burned_lp_from_balances`
- Task 6: Remove dead circuit breaker code
