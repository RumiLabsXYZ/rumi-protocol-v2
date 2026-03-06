# Reserve Redemption System — Implementation Plan

## Overview

Build a two-tier redemption system:
1. **Tier 1 (Reserves)**: Users burn icUSD and receive ckStable tokens (ckUSDT/ckUSDC) from the protocol's reserves at a flat 0.3% fee
2. **Tier 2 (Vaults)**: When reserves are exhausted, remaining redemption spills over to vault redemptions using a water-filling algorithm that equalizes CR across affected vaults

Admin-toggleable. Fee from reserve redemptions goes to treasury. Lower global redemption fee floor from 0.5% to 0.3%.

---

## Phase 1: Backend — State & Configuration

**File: `src/rumi_protocol_backend/src/state.rs`**

1. Add new state fields:
   ```rust
   pub reserve_redemptions_enabled: bool,          // admin toggle, default false
   pub reserve_redemption_fee: Ratio,              // flat fee for reserve redemptions (0.003 = 0.3%)
   ```

2. Add new constant:
   ```rust
   pub const DEFAULT_RESERVE_REDEMPTION_FEE: Ratio = Ratio::new(dec!(0.003)); // 0.3%
   ```

3. Initialize in `From<InitArg>`:
   ```rust
   reserve_redemptions_enabled: false,
   reserve_redemption_fee: DEFAULT_RESERVE_REDEMPTION_FEE,
   ```

4. Lower `DEFAULT_REDEMPTION_FEE_FLOOR` from `0.005` (0.5%) to `0.003` (0.3%)

5. Add helper to get available reserve tokens and balances — we do NOT need a new registry struct. The canister already knows its ckUSDT and ckUSDC ledger principals (`ckusdt_ledger_principal`, `ckusdc_ledger_principal`). For "any future token" support, we'll use the existing `collateral_configs` map — any collateral config's ledger is a potential reserve token. But for now, ckStable reserves are identified by the two `Option<Principal>` fields already on State. We'll add a lightweight `ReserveTokenConfig` if/when a third reserve token is needed.

6. Add `check_semantically_eq` entries for new fields.

---

## Phase 2: Backend — Events

**File: `src/rumi_protocol_backend/src/event.rs`**

1. Add new event variants:
   ```rust
   #[serde(rename = "set_reserve_redemptions_enabled")]
   SetReserveRedemptionsEnabled { enabled: bool },

   #[serde(rename = "set_reserve_redemption_fee")]
   SetReserveRedemptionFee { fee: String },

   #[serde(rename = "reserve_redemption")]
   ReserveRedemption {
       owner: Principal,
       icusd_amount: ICUSD,
       fee_amount: ICUSD,             // fee in icUSD terms
       stable_token_ledger: Principal, // which ckStable was sent
       stable_amount_sent: u64,        // e6s actually sent to user
       fee_stable_amount: u64,         // e6s sent to treasury as fee
       icusd_block_index: u64,         // block where icUSD was pulled from user
   },
   ```

2. Add replay handlers for each new event variant.

3. Add `record_*` functions:
   - `record_set_reserve_redemptions_enabled(state, enabled)`
   - `record_set_reserve_redemption_fee(state, fee)`
   - `record_reserve_redemption(state, ...)` — this one just records the event (actual transfers happen async)

4. Mark all new admin events as non-vault-affecting (`=> false` in the relation method).

---

## Phase 3: Backend — Reserve Redemption Logic

**File: `src/rumi_protocol_backend/src/vault.rs`** (add new function)

New function `redeem_reserves(icusd_amount: u64, preferred_token: Option<Principal>)`:

### Flow:
1. **Guard**: `GuardPrincipal` to prevent reentrancy
2. **Check**: `reserve_redemptions_enabled == true`, else error
3. **Pull icUSD** from caller via `transfer_icusd_from` (this effectively burns it — the protocol canister IS the minting account)
4. **Calculate fee**: `fee_icusd = icusd_amount × reserve_redemption_fee` (flat 0.3%)
5. **Net amount**: `net_icusd = icusd_amount - fee_icusd`
6. **Convert to e6s**: `net_e6s = net_icusd_e8s / 100` (icUSD is e8s, ckStables are e6s)
7. **Choose reserve token**:
   - If `preferred_token` specified and has sufficient balance → use it
   - Otherwise try ckUSDT first, then ckUSDC (or whichever has balance)
   - If neither has enough, use what's available from reserves and **spill remainder to vault redemption** (Tier 2)
8. **Transfer ckStable to user**: `management::transfer_collateral(net_e6s, caller, stable_ledger)`
9. **Transfer fee to treasury**: `management::transfer_collateral(fee_e6s, treasury_principal, stable_ledger)`
   - If no treasury configured, fee stays in reserves (protocol keeps it)
10. **Record event**: `record_reserve_redemption(...)`
11. **If spillover**: call existing vault redemption path for the remaining amount

### Return type:
```rust
pub struct ReserveRedemptionResult {
    pub icusd_block_index: u64,
    pub stable_amount_sent: u64,        // e6s to user
    pub fee_amount: u64,                // fee in icUSD e8s
    pub stable_token_used: Principal,   // which ledger
    pub vault_spillover_amount: u64,    // if any went to vault redemption (e8s)
}
```

### Important edge case — partial reserves:
If reserves only cover part of the redemption:
- Send available reserves to user (still at flat 0.3% fee on the reserve portion)
- Remaining icUSD goes through vault redemption at DYNAMIC fee rate
- Both legs reported in the result

### Balance queries:
We need to query ckStable ledger balances from within the canister. This is an inter-canister call:
```rust
async fn get_reserve_balance(ledger: Principal) -> u64 {
    // ICRC-1 balance_of call for ic_cdk::id()
}
```
This already exists indirectly — `transfer_collateral` will fail if balance is insufficient. But we need the balance upfront to determine token selection and spillover. Add a helper in `management.rs`.

---

## Phase 4: Backend — Water-Filling Vault Redemption

**File: `src/rumi_protocol_backend/src/state.rs`** (modify `redeem_on_vaults`)

Replace the current simple "drain lowest-CR vault first" with the water-filling algorithm:

### Algorithm:
```
1. Sort vaults by CR ascending (already done via BTreeSet)
2. Group into bands by CR level
3. For the lowest band:
   a. Calculate icUSD needed to raise all band vaults to next band's CR
      Formula per vault: x_i = D_i × (CR_next - CR_current) / (CR_next - 1)
   b. If remaining_redemption >= total_needed:
      - Deduct proportionally from each vault
      - Merge band with next band
      - Repeat
   c. Else (partial fill within band):
      - Calculate achievable CR: CR_max = (S × CR_current - R) / (S - R)
        where S = sum of debts in band, R = remaining redemption
      - Distribute proportionally by debt size
      - Done
```

### Key formula:
To redeem `x` icUSD from a vault to raise it from `CR_current` to `CR_target`:
```
x = debt × (CR_target - CR_current) / (CR_target - 1)
```

### Edge cases:
- Vaults with zero debt: skip (CR = infinity)
- Very small amounts: ensure we don't leave dust (below ledger fee)
- Rounding: work in integer arithmetic where possible, handle remainder on last vault

### The fee stays with vault owners:
Currently: `state.provide_liquidity(fee_amount, state.developer_principal)` — sends fee to developer's stability pool position.
Change to: simply don't redeem the fee portion from vaults. The fee is already deducted before `redeem_on_vaults` is called (`icusd_amount - fee_amount`), so vault owners "keep" the fee as unredeemed debt. But the fee icUSD was already pulled from the user and sits in the protocol (burned). So effectively: fee = free debt reduction for vault owners. **This is actually already how it works** — `provide_liquidity` is a separate concern (stability pool). We should remove the `provide_liquidity` call since the stability pool is legacy. The fee is already effectively burned.

---

## Phase 5: Backend — Admin Functions & Queries

**File: `src/rumi_protocol_backend/src/main.rs`**

1. `set_reserve_redemptions_enabled(enabled: bool)` — developer only
2. `get_reserve_redemptions_enabled() -> bool` — query
3. `set_reserve_redemption_fee(fee: f64)` — developer only, range 0.0–0.10
4. `get_reserve_redemption_fee() -> f64` — query
5. `redeem_reserves(amount: u64, preferred_token: opt principal)` — update endpoint
6. `get_reserve_balances() -> vec { ledger: principal, balance: u64, symbol: text }` — query

Update `get_protocol_status()` to include:
- `reserve_redemptions_enabled: bool`
- `reserve_redemption_fee: f64`

---

## Phase 6: Backend — Candid Interface

**File: `src/rumi_protocol_backend/rumi_protocol_backend.did`**

Add new types and endpoints:
- `ReserveRedemptionResult` record type
- `ReserveBalance` record type
- All new admin/query/update endpoints
- New event variants in the Event type
- Updated ProtocolStatus with reserve fields

---

## Phase 7: Frontend — Types & API

**File: `src/vault_frontend/src/lib/services/types.ts`**

Add:
```typescript
interface ReserveRedemptionResult {
  icusdBlockIndex: number;
  stableAmountSent: number;     // e6s
  feeAmount: number;            // e8s
  stableTokenUsed: string;      // principal
  vaultSpilloverAmount: number;  // e8s, 0 if fully covered by reserves
}

interface ReserveBalance {
  ledger: string;
  balance: number;  // human-readable
  symbol: string;
}
```

Add `reserveRedemptionsEnabled` and `reserveRedemptionFee` to `ProtocolStatusDTO`.

**File: `src/vault_frontend/src/lib/services/protocol/apiClient.ts`**

Add `redeemReserves(amount: number, preferredToken?: string)` method.
Add `getReserveBalances()` query method.

**File: `src/vault_frontend/src/lib/services/protocol/queryOperations.ts`**

Map new ProtocolStatus fields.

---

## Phase 8: Frontend — Redesigned Redeem Page

**File: `src/vault_frontend/src/routes/redeem/+page.svelte`**

Complete redesign:

1. **Title**: "Redeem icUSD" (not "Redeem ICP")
2. **Show reserve balances**: display available ckUSDT and ckUSDC in reserves
3. **Token selector**: dropdown/tabs to choose which token to receive (ckUSDT, ckUSDC, or ICP)
4. **Amount input**: icUSD amount to redeem
5. **Fee display**:
   - If redeeming from reserves: "Reserve fee: 0.3% (flat)"
   - If reserves insufficient: "Reserve fee: 0.3% on $X + Dynamic fee: Y% on $Z (vault spillover)"
   - If ICP selected: "Dynamic fee: Y%"
6. **Output display**: "You will receive: X ckUSDT" (or ckUSDC/ICP)
7. **Reserve status indicator**: green badge "Reserves available" or yellow "Reserves depleted — vault redemption"
8. **Disabled state**: if reserve redemptions are disabled by admin and no ICP redemption desired
9. **"How it works" section**: updated to explain two-tier system

---

## Phase 9: Lower Fee Floor

After deployment, call `set_redemption_fee_floor(0.003)` via dfx to lower from 0.5% to 0.3%. This is a simple admin call, no code change needed (the constant DEFAULT only affects fresh deploys).

Actually — we should change `DEFAULT_REDEMPTION_FEE_FLOOR` in state.rs from 0.005 to 0.003 so future deploys pick up the new default. The live instance gets updated via the admin call.

---

## Execution Order

1. **Phase 1–2**: State fields + events (foundation)
2. **Phase 3**: Reserve redemption logic + management helper for balance queries
3. **Phase 4**: Water-filling algorithm upgrade to `redeem_on_vaults`
4. **Phase 5–6**: Admin endpoints + Candid interface
5. **Build & test backend** (`cargo build`, `cargo test`)
6. **Phase 7**: Frontend types & API
7. **Phase 8**: Frontend redeem page redesign
8. **Build frontend**
9. **Deploy backend** then **frontend**
10. **Phase 9**: Call `set_redemption_fee_floor(0.003)` and `set_reserve_redemptions_enabled(true)` via dfx

---

## Files Modified

### Backend (Rust):
- `src/rumi_protocol_backend/src/state.rs` — new fields, water-filling algorithm, constants
- `src/rumi_protocol_backend/src/event.rs` — 3 new event variants + replay + record functions
- `src/rumi_protocol_backend/src/vault.rs` — new `redeem_reserves()` function
- `src/rumi_protocol_backend/src/main.rs` — new admin/query/update endpoints, updated ProtocolStatus
- `src/rumi_protocol_backend/src/management.rs` — `get_token_balance()` helper
- `src/rumi_protocol_backend/src/lib.rs` — updated ProtocolStatus struct, ReserveRedemptionResult type
- `src/rumi_protocol_backend/rumi_protocol_backend.did` — new types + endpoints

### Frontend (Svelte/TS):
- `src/vault_frontend/src/lib/services/types.ts` — new interfaces
- `src/vault_frontend/src/lib/services/protocol/apiClient.ts` — new API methods
- `src/vault_frontend/src/lib/services/protocol/queryOperations.ts` — status mapping
- `src/vault_frontend/src/routes/redeem/+page.svelte` — complete redesign

### No new files created — all modifications to existing files.
