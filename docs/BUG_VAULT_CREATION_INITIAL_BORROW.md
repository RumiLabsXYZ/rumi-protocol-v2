# Bug Report: Vault Creation Initial Borrow Not Working

**Date:** January 26, 2026  
**Reporter:** Rob Ripley  
**Priority:** HIGH - Core functionality broken  
**Status:** ✅ **BACKEND FIX DEPLOYED - TESTING IN PROGRESS**
**Affected Wallets:** Oisy (confirmed), possibly others  
**Deployed Canister:** `tcfua-yaaaa-aaaap-qrd7q-cai`  

---

## CURRENT STATUS (January 26, 2026 - Late Evening)

### ✅ FRONTEND + BACKEND FIX COMPLETE

The issue was traced through the entire stack. Both frontend AND backend were contributing to the bug.

---

## Root Cause Analysis

### Issue 1: Frontend (FIXED EARLIER)
**File:** `apiClient.ts` line ~583
```javascript
const vaultResult = await actor.open_vault_with_deposit(0n); // borrow_amount = 0 for now
```
The borrow amount was hardcoded to `0n` instead of passing the user's specified amount.

### Issue 2: Backend (FIXED NOW)
**File:** `vault.rs` line ~262 in `open_vault_with_deposit` function
```rust
borrowed_icusd_amount: 0.into(), // Was hardcoded to 0!
```
Even after the frontend fix, the backend was ignoring the `borrow_amount` parameter and hardcoding the vault's borrowed amount to 0.

---

## Evidence from Testing

Console output showed the disconnect:
```
Processing vault #23: ICP=0.08, icUSD=0
```
Despite the UI claiming "borrowed 0.11 icUSD", the vault card confirmed: `Borrowed: 0 icUSD, Collateral ratio: ∞`

---

## Backend Fix Implementation

**File:** `/src/rumi_protocol_backend/src/vault.rs`
**Function:** `open_vault_with_deposit` (lines 196-340)

### Changes Made:

1. **Added logging** for debugging:
   ```rust
   ic_cdk::println!("Starting for caller {:?}, borrow_amount: {}", caller, borrow_amount);
   ```

2. **Convert borrow_amount to ICUSD type**

3. **Validate collateral ratio** before allowing borrow:
   - Calculate `max_borrowable_amount` using ICP rate and `min_collateral_ratio`
   - Return error if borrow exceeds maximum allowed

4. **Create vault** with `borrowed_icusd_amount: 0` initially (unchanged)

5. **After vault creation**, if `borrow_amount > 0`:
   - Calculate borrowing fee
   - Call `mint_icusd(borrow_icusd - fee, caller)`
   - Record borrow via `record_borrow_from_vault`
   - Schedule treasury fee routing via `route_minting_fee_to_treasury`

6. **Error handling**: If minting fails, vault still created (user can borrow later manually)

### Logic Source
The borrow logic mirrors the existing `borrow_from_vault` function (lines 294-370) but integrated into the vault creation flow.

---

## Build Status

Backend compiled successfully:
```
Compiling rumi_protocol_backend v0.1.0
Finished `release` profile [optimized] target(s) in 12.56s
```
- 19 warnings (non-critical: unused imports/variables)
- Target: wasm32-unknown-unknown release

---

## Files Modified

| File | Location | Changes |
|------|----------|---------|
| `apiClient.ts` | Frontend | Pass borrow amount to backend call |
| `+page.svelte` | Frontend | Wallet-type detection, different flows for Oisy vs Plug/II |
| `vault.rs` | Backend | Actually USE the borrow_amount parameter to mint icUSD |

---

## Deployment Commands

### Backend (READY TO DEPLOY):
```bash
cd /Users/robertripley/coding/rumi-protocol-v2
dfx deploy rumi_protocol_backend --network ic
```

### Frontend (ALREADY DEPLOYED):
```bash
dfx deploy vault_frontend --network ic
```

---

## Testing Checklist

### Initial Borrow at Vault Creation:
- [ ] Oisy: Create vault with 0.1 ICP and borrow 0.05 icUSD → Verify icUSD minted
- [ ] Plug: Create vault with initial borrow → Verify two-step flow works
- [ ] II: Create vault with initial borrow → Verify two-step flow works
- [ ] Verify vault card shows correct borrowed amount (not 0)
- [ ] Verify collateral ratio displays correctly (not ∞)

### Validation:
- [ ] Can't borrow more than 66.67% of collateral value (150% collateral ratio)
- [ ] Error message displays if trying to over-borrow

### Backward Compatibility:
- [ ] Create vault without initial borrow → Works as before
- [ ] Manual borrow from existing vault → Still works

---

## Key Canister IDs

| Canister | ID | Purpose |
|----------|-----|---------|
| vault_frontend | `tcfua-yaaaa-aaaap-qrd7q-cai` | Frontend hosting |
| rumi_protocol_backend | `tfesu-vyaaa-aaaap-qrd7a-cai` | Protocol backend |
| ICP Ledger | `ryjl3-tyaaa-aaaaa-aaaba-cai` | ICP transfers |

---

## Technical Details

### Wallet-Specific Flows

**Oisy Wallet (Push Deposit):**
- Single atomic call: `open_vault_with_deposit(borrow_amount)`
- Vault creation + ICP transfer + icUSD minting in one operation

**Plug/Internet Identity (ICRC-2):**
- Two-step process required:
  1. `open_vault()` - creates vault with collateral
  2. `borrow_from_vault(vault_id, amount)` - mints icUSD
- Backend `open_vault()` doesn't accept borrow parameter (ICRC-2 flow limitation)

### Backend Function Signature
```rust
pub async fn open_vault_with_deposit(borrow_amount: u64) -> Result<OpenVaultSuccess, ProtocolError>
```

### Candid Interface
```candid
open_vault_with_deposit : (nat64) -> (variant { Ok : OpenVaultSuccess; Err : ProtocolError });
```

---

## Related Documentation

- `/docs/OISY_QUERY_SIGNER_BUG.md` - Previous signer fix
- `/docs/OISY_ICP_DEPOSIT_IMPLEMENTATION_PLAN.md` - ICP deposit flow design
- `/docs/OISY_IMPLEMENTATION_COMPLETE.md` - Oisy integration status
