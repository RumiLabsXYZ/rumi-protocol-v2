# Rumi Protocol - Project Handoff Document

## Quick Reference

| Item | Value |
|------|-------|
| **GitHub** | https://github.com/RumiLabsXYZ/rumi-protocol-v2 |
| **Live Site** | https://rumiprotocol.io or https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io |
| **Local Path** | `/Users/robertripley/coding/rumi-protocol-v2` |
| **Company** | Rumi Labs LLC (Wyoming, EIN: 33-2759974) |

---

## Team & Controllers

### Principals (Add as canister controllers)
| Name | GitHub | Principal |
|------|--------|-----------|
| Rob (Lead) | RobRipley | `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` |
| Agnes | agneskoinange | `wrppb-amng2-jzskb-wcmam-mwrmi-ci52r-bkkre-tzu35-hjfpb-dnl4p-6qe` |
| Gurleen | Gurleenkdhaliwal | `bsu7v-jz2ty-tyonm-dmkdj-nir27-num7e-dtlff-4vmjj-gagxl-xiljg-lqe` |
| CycleOps | - | `cpbhu-5iaaa-aaaad-aalta-cai` |

---

## ✅ COMPLETED: Staging Branch Merge (February 4, 2026)

### Merge Summary

The long-stranded `staging` branch (57 commits) has been successfully merged into `main`. This recovers all of Agnes's work from Fall 2025 that was blocked due to a workflow disruption.

**Merge Commit:** `1cb0034` - "Merge staging into main: Treasury, Stability Pool, partial vault operations, II improvements"

**Additional Commit:** `477a89d` - Cherry-picked LICENSE and ACKNOWLEDGMENTS files from `feature/liquidation-price-check`

### What Was Merged

| Feature | Description | Author |
|---------|-------------|--------|
| **Treasury Canister** | Protocol fee collection and management | Agnes |
| **Stability Pool Canister** | Liquidation pool for protocol stability | Agnes |
| **Partial Vault Operations** | `partial_repay_to_vault`, `partial_liquidate_vault` endpoints | Agnes |
| **Internet Identity Improvements** | Better II integration and state persistence | Agnes |
| **WalletSelector Component** | New wallet selection UI component | Agnes |
| **Stability Pool UI** | Frontend components for stability pool | Agnes |
| **Various Bug Fixes** | Multiple fixes across frontend and backend | Agnes |

### Conflict Resolution Details

8 files had merge conflicts. Here's how each was resolved:

| File | Resolution | Rationale |
|------|------------|-----------|
| `canister_ids.json` | Kept main's treasury ID (`tlg74-oiaaa-aaaap-qrd6a-cai`) | Main has the currently deployed production canister IDs |
| `dfx.json` | Merged main's clean structure + added `internet_identity` and `xrc` canisters from staging | Combined best of both: main's simple canister configs without embedded init_args, plus staging's new canister definitions |
| `rumi_protocol_backend.did` | Combined partial operations from staging + ICRC-21/28/10 standards from main | Main had critical Oisy wallet standards; staging had new partial operations |
| `config.ts` | Kept main's canister IDs | Main has production-deployed canister IDs |
| `auth.ts` | Kept main's version | Main has comprehensive Oisy wallet integration and session restoration logic |
| `apiClient.ts` | Kept main's version | Main has complete Treasury service implementation and better imports |
| `+layout.svelte` | Kept main's version | Main has enhanced sidebar layout with better navigation |
| `LoadingSpinner.svelte` | Kept main's version | Main's version is parameterized (size, color props) |

### Backup Branch

A backup was created before merging: `main-backup-feb4`

If anything goes wrong, you can restore with:
```bash
git checkout main
git reset --hard main-backup-feb4
git push origin main --force
```

### Deferred Work

The `feature/liquidation-price-check` branch has one remaining commit that was NOT merged due to conflicts:
- **Commit:** `4169380` - "fix: add price freshness validation to liquidation endpoints"
- **Files affected:** `src/rumi_protocol_backend/src/main.rs`, `src/rumi_protocol_backend/src/xrc.rs`
- **What it does:** Adds `validate_price_for_liquidation()` helper to enforce price staleness checks on liquidation endpoints
- **Action needed:** Merge this separately after testing the current merge

---

## Git Branch Status (Updated)

| Branch | Status | Action |
|--------|--------|--------|
| `main` | ✅ **Up to date** - Contains all staging work + LICENSE/ACKNOWLEDGMENTS | Production branch |
| `staging` | ✅ **Merged** | Can be deleted or kept for reference |
| `main-backup-feb4` | Backup of main before merge | Keep for safety, delete after confirming stability |
| `feature/liquidation-price-check` | Has 1 unmerged commit (price validation) | Merge separately |
| `test/oisy-icrc2-repayment` | Test branch (currently deployed to mainnet) | DO NOT MERGE - test only |

### Next Steps After Merge

1. **Build and test locally:**
   ```bash
   cd /Users/robertripley/coding/rumi-protocol-v2
   npm install
   npm run build
   ```

2. **Deploy to mainnet** (when ready):
   ```bash
   dfx deploy vault_frontend --network ic
   dfx deploy rumi_protocol_backend --network ic
   ```

3. **Merge price validation fix** (in separate session):
   ```bash
   git checkout main
   git cherry-pick 4169380
   # Resolve conflicts in main.rs
   git push origin main
   ```

4. **Clean up branches** (optional):
   ```bash
   git push origin --delete staging  # If no longer needed
   git branch -d main-backup-feb4    # After confirming stability
   ```

---

## Canister IDs (Mainnet)

```
rumi_protocol_backend: tfesu-vyaaa-aaaap-qrd7a-cai
vault_frontend:        tcfua-yaaaa-aaaap-qrd7q-cai
icusd_ledger:          t6bor-paaaa-aaaap-qrd5q-cai
icusd_index:           6niqu-siaaa-aaaap-qrjeq-cai
rumi_treasury:         tlg74-oiaaa-aaaap-qrd6a-cai
rumi_stability_pool:   tmhzi-dqaaa-aaaap-qrd6q-cai
```

**⚠️ CRITICAL**: Previous canisters were blackholed due to controller misconfiguration. This is a fresh deployment from cloned repo.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    vault_frontend (Svelte)                   │
│                   tcfua-yaaaa-aaaap-qrd7q-cai                │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│ rumi_protocol_   │  │   icusd_ledger   │  │  rumi_treasury   │
│    backend       │  │ t6bor-paaaa-...  │  │ tlg74-oiaaa-...  │
│ tfesu-vyaaa-...  │  │  (ICRC-1/2)      │  │  (Fee Collection)│
│                  │  └──────────────────┘  └──────────────────┘
│ - Vault Mgmt     │           │
│ - Liquidation    │           ▼
│ - Price Oracle   │  ┌──────────────────┐  ┌──────────────────┐
│ - Stability Pool │  │   icusd_index    │  │ rumi_stability_  │
└──────────────────┘  │ 6niqu-siaaa-...  │  │      pool        │
                      │  (Tx Index)      │  │ tmhzi-dqaaa-...  │
                      └──────────────────┘  └──────────────────┘
```

---

## Core Protocol Mechanics

### Vault Operations
- **Collateral**: ICP only for MVP (ckBTC, ckETH planned)
- **Liquidation Ratio**: 150% (collateral must be ≥1.5x debt)
- **Max LTV**: 66.67% (can borrow up to 2/3 of collateral value)

### Key Backend Functions (from .did file)
```candid
// Vault Management
open_vault : (amount_e8s: nat64) -> (Result)
borrow_from_vault : (vault_id: nat64, amount_e8s: nat64) -> (Result)
repay_to_vault : (vault_id: nat64, amount_e8s: nat64) -> (Result)
add_margin_to_vault : (vault_id: nat64, amount_e8s: nat64) -> (Result)
withdraw_collateral : (vault_id: nat64, amount_e8s: nat64) -> (Result)
withdraw_collateral_and_close_vault : (vault_id: nat64) -> (Result)

// Partial Operations (NEW from staging merge)
partial_repay_to_vault : (VaultArg) -> (Result)
partial_liquidate_vault : (VaultArg) -> (Result)

// ICRC Standards (for Oisy wallet)
icrc21_canister_call_consent_message : (Request) -> (Result)
icrc28_trusted_origins : () -> (Response) query
icrc10_supported_standards : () -> (vec StandardRecord) query

// Queries
get_vault : (vault_id: nat64) -> (opt Vault) query
get_vaults_by_owner : (owner: principal) -> (vec Vault) query
get_protocol_status : () -> (ProtocolStatus) query
get_icp_price : () -> (nat64) query
```

---

## Wallet Integration Status

### Supported Wallets
| Wallet | Status | Notes |
|--------|--------|-------|
| **Plug** | ✅ Working | Primary testing wallet |
| **Internet Identity** | ⚠️ Needs Testing | II 2.0 uses `https://id.ai` |
| **Oisy** | 🔴 Partially Blocked | ICP ICRC-2 fails; icUSD under test |

### Oisy Wallet - Current Status

**Known:** Oisy CANNOT do ICRC-2 operations on the **ICP ledger** (vault creation collateral).
- Solution implemented: Push-deposit pattern for ICP collateral

**Unknown (Testing):** Can Oisy do ICRC-2 on the **icUSD ledger**?
- Test branch deployed: `test/oisy-icrc2-repayment`
- Test URL: https://rumiprotocol.io
- Look for `[TEST-ICRC2]` logs in browser console
- See `/docs/archive/OISY_ICRC2_TEST_SESSION_HANDOFF.md` for full details

**If icUSD ICRC-2 fails:** Will need push-style repayment implementation.

---

## Known Bugs

### 1. Vault Close Navigation Bug
**File**: `src/vault_frontend/src/lib/components/vault/VaultDetails.svelte`
**Problem**: After closing vault, page stays on `/vaults/[id]` instead of redirecting to `/vaults`
**Tried**: Event dispatch, direct `goto()`, even `window.location.href` - none worked
**Status**: Needs investigation - deployed code may not match source

### 2. Plug Wallet Auto-Reconnect
**File**: `src/vault_frontend/src/lib/services/auth.ts`
**Problem**: Plug sessions don't persist across page refresh
**Status**: Added `waitForPlug()` polling but still failing silently

---

## Tech Stack

### Backend (Rust)
- **Framework**: IC CDK
- **State**: Stable memory with `ic-stable-structures`
- **Price Oracle**: Exchange Rate Canister (XRC)
- **Key Files**:
  - `src/rumi_protocol_backend/src/lib.rs` - Main entry
  - `src/rumi_protocol_backend/src/state.rs` - State management
  - `src/rumi_protocol_backend/src/vault.rs` - Vault logic
  - `src/stability_pool/` - Stability pool canister (NEW)
  - `src/rumi_treasury/` - Treasury canister

### Frontend (Svelte + TypeScript)
- **Build**: Vite
- **Styling**: Tailwind CSS
- **Wallet Libraries**: 
  - `@dfinity/auth-client` (II)
  - `window.ic.plug` (Plug)
  - `@dfinity/oisy-wallet-signer` (Oisy)
- **Key Files**:
  - `src/vault_frontend/src/lib/services/auth.ts` - Wallet integration
  - `src/vault_frontend/src/lib/services/protocol/` - Backend API calls
  - `src/vault_frontend/src/lib/services/stabilityPool.ts` - Stability pool service (NEW)
  - `src/vault_frontend/src/lib/components/stability/` - Stability pool UI (NEW)

---

## Development Commands

```bash
# Start local replica
dfx start --clean --background

# Deploy locally
dfx deploy

# Deploy to mainnet
dfx deploy --network ic

# Generate declarations after .did changes
dfx generate

# Build frontend only
npm run build

# Deploy frontend to mainnet
dfx deploy vault_frontend --network ic
```

---

## MVP Priorities

1. ~~**Merge Staging**~~: ✅ COMPLETED - Treasury, Stability Pool, and other features recovered
2. **Deploy Merged Code**: Build and deploy the merged main branch to mainnet
3. **Core Stability**: Vault creation, minting, repayments, liquidations
4. **Oisy Integration**: Complete icUSD testing, implement push-repayment if needed
5. **Bug Fixes**: Navigation after vault close, Plug auto-reconnect
6. **UI Polish**: See task doc for header/nav/wallet UX improvements

### Post-MVP
- Fees implementation (route to treasury canister)
- Stability Pool automation
- Redemption process
- Alternative stablecoin repayments (ckUSDT/ckUSDC)
- Additional collateral types (ckBTC, ckETH)

---

## Important Notes

- **No fees currently** - Will implement after beta testing
- **Manual liquidations** - Browse liquidations page for undercollateralized vaults
- **Fresh deployment** - Old repo at github.com/Rumi-Protocol/Rumi-protocol (blackholed)
- **Staging merge complete** - All 57 commits now in main

---

## Related Documentation

| Document | Purpose |
|----------|---------|
| `/docs/archive/OISY_ICRC2_TEST_SESSION_HANDOFF.md` | Oisy icUSD test details |
| `/docs/OISY_IMPLEMENTATION_COMPLETE.md` | Original Oisy integration work |
| `/ACKNOWLEDGMENTS.md` | Contributor credits |
| `/LICENSE` | MIT License |

---

*Last updated: February 4, 2026*
