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
| Name | GitHub | Principal | Notes |
|------|--------|-----------|-------|
| Rob (Lead) | RobRipley | `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` | |
| Agnes (NEW) | agneskoinange | `jtqeo-qixuv-xsygz-jhhre-zht42-iiop6-icktm-f7oeg-horay-dl4ao-dae` | Current identity |
| Agnes (OLD) | agneskoinange | `wrppb-amng2-jzskb-wcmam-mwrmi-ci52r-bkkre-tzu35-hjfpb-dnl4p-6qe` | Lost identity - DO NOT USE |
| Gurleen | Gurleenkdhaliwal | `bsu7v-jz2ty-tyonm-dmkdj-nir27-num7e-dtlff-4vmjj-gagxl-xiljg-lqe` | |
| CycleOps | - | `cpbhu-5iaaa-aaaad-aalta-cai` | Balance checker |

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

## Development Workflow (Agreed Feb 5, 2026)

### Roles
- **Rob**: UI/UX improvements, cleanup, testing. Submits PRs.
- **Agnes**: Feature development, PR review, merge authority.

### Git Flow
1. Work on feature branches
2. Deploy to **staging canister** for validation
3. Submit PR to `main`, Agnes reviews and merges
4. Deploy `main` to **production canister**

### Staging Deployment — PENDING SETUP
Agnes proposed deploying to staging before production. Details TBD:
- Need a separate frontend canister on mainnet for staging
- Unclear if a `staging` git branch is needed or if feature branches deploy directly
- **Rob messaged Agnes to clarify** (Feb 6) — waiting on response
- Until staging is set up, deployments go straight to production (current behavior)

### Remaining Merge Tasks

1. **Merge price validation fix** (in separate session):
   ```bash
   git checkout main
   git cherry-pick 4169380
   # Resolve conflicts in main.rs
   git push origin main
   ```

2. **Clean up branches** (optional):
   ```bash
   git push origin --delete staging  # If no longer needed
   git branch -d main-backup-feb4    # After confirming stability
   ```

---

## ✅ COMPLETED: Send/Receive Feature + UI Polish (February 5, 2026)

### Branch: `feature/ii-wallet-send-receive`

Planned Jan 27, Phase 1 built Jan 27, completed and deployed to mainnet Feb 5.
All changes deployed on vault_frontend canister `tcfua-yaaaa-aaaap-qrd7q-cai`.

### New Files Created (Jan 27 – Feb 5)

| File | Purpose |
|------|---------|
| `components/common/Toast.svelte` | Auto-dismiss notification component |
| `components/common/Modal.svelte` | Reusable modal with DOM portal pattern |
| `components/wallet/ReceiveModal.svelte` | QR code + principal display for receiving |
| `components/wallet/SendModal.svelte` | ICP/icUSD transfer UI for II users |
| `services/transferService.ts` | ICRC-1 transfer logic for ICP and icUSD |
| `utils/principalHelpers.ts` | `truncatePrincipal()` and `copyToClipboard()` |

### 1. Transfer Service Fix

**File:** `transferService.ts`
- Changed `pnp.getActor()` → `walletStore.getActor()` for creating ledger actors
- `walletStore.getActor()` (in `auth.ts`) detects II and uses delegation identity agent
- Fixed "Cannot create signed actor. No wallet provider connected." error
- ICP and icUSD transfers now work for II users via `icrc1_transfer`

### 2. Internet Identity Portal URL

**Files:** `internetIdentity.ts` and `auth.ts`
- Changed II provider from `https://identity.ic0.app` to `https://id.ai`

### 3. Modal Portal Fix

**File:** `Modal.svelte`
- DOM portal pattern: modal `onMount` appends to `document.body`
- Fixes stacking context issues (modals were rendering above viewport)
- Fixes click-outside-to-close (backdrop click checks `event.target === backdropEl`)
- Removed Svelte transitions that conflicted with portal approach

### 4. Header Wallet Pill Redesign

**File:** `WalletConnector.svelte`

| Before | After |
|--------|-------|
| USD values in header | Removed (kept in dropdown only) |
| All balances equal weight | icUSD primary (bright, bold), ICP secondary (muted) |
| Principal same size as balances | Tiny mono metadata text (30% opacity) |
| Controls mixed with data | Vertical divider separates data from controls |

Reading order: Wallet icon → icUSD balance → ICP balance → controls

### 5. Wallet Dropdown Polish

- Truncated principal at top, clickable to copy (hover turns purple)
- Balance rows with token icons, USD values muted
- icUSD always shown (even at 0)
- Action buttons: Receive, Send (II only), Refresh Balance, Disconnect
- Disabled Send/Receive for non-II wallets with tooltip

### 6. Receive Modal with QR Code

- QR code as visual anchor (via `qrcode` npm v1.5.4)
- Principal below in mono block
- Helper text: "Use this address to receive ICP or icUSD."
- Copy Principal button

### 7. Toast Repositioning

- Container: `position: fixed; top: 4.5rem; right: 1rem`
- Below header, never overlaps wallet pill
- `z-index: 8000` (above content, below modals at 9000)
- Multiple toasts stack downward, auto-dismiss 3.5s

### 8. GitHub Icon in Header

**File:** `+layout.svelte`
- GitHub SVG icon added next to email and Twitter social links
- Links to `https://github.com/RumiLabsXYZ/rumi-protocol-v2`

### 9. WalletConnector Click-Outside Fix

- Removed `class="relative"` from `#wallet-container` (was creating stacking context)
- `handleClickOutside()` skips when modals are open

### npm Dependencies Added
- `qrcode` v1.5.4
- `@types/qrcode`

### Deferred / Future Work from Send/Receive Design Sessions (Feb 5)

These items were discussed but NOT yet implemented:

1. **ckToken support in Send modal** — Rob wants to expand beyond ICP/icUSD to include:
   - ckBTC, ckETH, ckXAUT, ckLINK, ckDOGE, ckWSTETH
   - NO stablecoins (ckUSDT, ckUSDC excluded from quick-select)
   - UI: quick-select icons for common tokens + dropdown for full list
   - Requires adding ledger canister IDs to config

2. **Token ledger canister IDs researched** (mainnet):
   - ckBTC: `mxzaz-hqaaa-aaaar-qaada-cai`
   - ckETH: `ss2fx-dyaaa-aaaar-qacoq-cai`
   - ckUSDT: `cngnf-vqaaa-aaaar-qag4q-cai`
   - ckUSDC: `xevnm-gaaaa-aaaar-qafnq-cai`
   - (ckXAUT, ckLINK, ckDOGE, ckWSTETH — IDs need to be looked up)

3. **Testing checklist still outstanding:**
   - [ ] Test ICP transfer with Internet Identity
   - [ ] Test icUSD transfer with Internet Identity
   - [ ] Verify new `id.ai` portal authentication flow
   - [ ] Test Plug/Oisy disabled buttons and tooltip
   - [ ] Test QR code renders correctly in Receive modal

---

## Git Branch Status (Updated Feb 5)

| Branch | Status | Action |
|--------|--------|--------|
| `main` | ⚠️ Backend doesn't build (33 errors) | Fix build errors before next backend deploy |
| `feature/ii-wallet-send-receive` | ✅ **Active** — deployed to mainnet (frontend only) | Merge to main when stable |
| `feature/liquidation-price-check` | Has XRC interval change + price validation | PR #3 open for Agnes — superseded by main already having staleness checks |
| `feature/plug-wallet-reconnect` | ✅ Merged via PR #2 | Can delete |
| `staging` | ✅ Merged into main | Can delete |
| `main-backup-feb4` | Backup of main before staging merge | Keep for safety |
| `test/oisy-icrc2-repayment` | Test branch for Oisy icUSD ICRC-2 | DO NOT MERGE — test only |

---

## 🚨 CRITICAL: Backend Build Errors (33 errors on main)

### Summary

The backend (`rumi_protocol_backend`) does not compile on `main`. This was introduced by the staging merge (`1cb0034`) which brought in Agnes's guard refactoring code that references State fields that were never added. **Production is not affected** — the currently deployed backend was built before the staging merge and is still running fine. But no backend changes can be deployed until this is fixed.

### Error Breakdown

| Error Type | Count | Source File | Description |
|------------|-------|-------------|-------------|
| `E0609: no field operation_guards` | 8 | `guard.rs` | `State` has no `operation_guards: BTreeSet<String>` field |
| `E0609: no field operation_details` | 8 | `guard.rs` | `State` has no `operation_details: BTreeMap<String, (Principal, String)>` field |
| `E0609: no field operation_guard_timestamps` | 7 | `guard.rs` | `State` has no `operation_guard_timestamps: BTreeMap<String, u64>` field |
| `E0308: mismatched types` | 6 | `guard.rs` | Cascade from missing fields (type inference failures) |
| `E0282: type annotations needed` | 4 | `guard.rs`, `lib.rs` | Cascade from missing fields |

### Root Cause

Agnes's staging branch refactored the guard system from **principal-based** guards to **operation-key-based** guards. The old system uses:
- `principal_guards: BTreeSet<Principal>` ✅ exists in State
- `principal_guard_timestamps: BTreeMap<Principal, u64>` ✅ exists in State
- `operation_states: BTreeMap<Principal, OperationState>` ✅ exists in State
- `operation_names: BTreeMap<Principal, String>` ✅ exists in State

The new guard code in `guard.rs` expects:
- `operation_guards: BTreeSet<String>` ❌ MISSING — keyed by `"principal:operation_name"` strings
- `operation_guard_timestamps: BTreeMap<String, u64>` ❌ MISSING — same string keys
- `operation_details: BTreeMap<String, (Principal, String)>` ❌ MISSING — maps key → (principal, operation_name)

The guard.rs was merged from staging but the corresponding State struct updates were lost in conflict resolution (we kept main's state.rs during the merge).

### How guard.rs Works (New System)

The refactored guard creates composite operation keys like `"fd7h3-mgmok...:open_vault"` combining principal + operation name. This allows a single principal to have multiple concurrent operations of different types (e.g., opening a vault while also doing a repayment), which the old principal-only system didn't support.

Key functions in `guard.rs`:
- `create_operation_key(principal, operation_name) → String` — creates the composite key
- `VaultGuard::new(principal, operation_name)` — acquires guard, cleans stale guards (5-min timeout)
- `VaultGuard::complete(self)` — marks operation as completed
- `Drop for VaultGuard` — cleanup on drop, removes guard from all tracking maps
- `MAX_CONCURRENT = 100` — max concurrent operations
- `GUARD_TIMEOUT_NANOS = 5 * 60 * 1_000_000_000` — 5-minute timeout

### Fix Required

Add three fields to `State` struct in `state.rs` (around line 127):

```rust
// Operation-key-based guards (from guard.rs refactor)
pub operation_guards: BTreeSet<String>,
pub operation_guard_timestamps: BTreeMap<String, u64>,
pub operation_details: BTreeMap<String, (Principal, String)>,
```

And initialize them in `impl From<InitArg> for State` (around line 165):

```rust
operation_guards: BTreeSet::new(),
operation_guard_timestamps: BTreeMap::new(),
operation_details: BTreeMap::new(),
```

**Decision needed:** Should the old principal-based guard fields (`principal_guards`, `principal_guard_timestamps`, `operation_states`, `operation_names`) be removed, or kept for backward compatibility? The new guard.rs code doesn't use them, but removing them may affect state deserialization on upgrade.

### What This Blocks

- ❌ Backend deployment to mainnet (any changes)
- ❌ XRC interval reduction (PR #3 — would save ~$46/month in oracle costs)
- ❌ Price freshness validation on liquidation endpoints
- ❌ Any future backend feature work

### What Still Works

- ✅ Frontend-only deploys (`dfx deploy vault_frontend --network ic`)
- ✅ Currently running backend on mainnet (old build, pre-merge)
- ✅ All user-facing functionality (vaults, transfers, wallet integration)

---

## XRC Cost Reduction — Pending Build Fix

### Current Cost
- ICP/USD fetched every **60 seconds** via XRC
- 1,440 calls/day × 1B cycles/call = ~1.44T cycles/day ≈ **$1.95/day ($58.50/month)**

### Planned Change (on `feature/liquidation-price-check`, PR #3)
- Increase `FETCHING_ICP_RATE_INTERVAL_SECS` from 60 → 300 (5 minutes)
- Reduces to 288 calls/day ≈ **$0.39/day ($11.70/month)** — 80% savings
- Safe because `check_price_not_too_old()` in state.rs uses a **10-minute** staleness threshold (`TEN_MINS_NANOS`), giving 2x buffer over the 5-minute fetch interval
- Note: `validate_call()` already calls `check_price_not_too_old()` on ALL endpoints including liquidations, so the separate `validate_price_for_liquidation()` in the PR is redundant but harmless

### Future: USDT/USDC Price Feeds
- For ckUSDT/ckUSDC repayment support, will need USDT/USD and USDC/USD prices
- Recommended approach: **on-demand fetching only** during liquidation/repayment operations, not polling
- Depeg threshold: reject if rate < $0.95 or > $1.05
- This avoids tripling the XRC bill from constant polling

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
| **Internet Identity** | ✅ Working | Send/Receive implemented, uses `https://id.ai` portal |
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

### 3. Left Nav Active Highlight Doesn't Track Page
**Problem**: The highlight/active indicator in the left sidebar navigation doesn't move when the user navigates to a different page
**Status**: Not investigated yet

---

## UI Exploration

- **Font**: `circular-std-medium-500.ttf` is in the repo root — Rob wants to try this font across the UI to see how it looks
- **Vault cards**: Want to explore making vault list/detail views sleeker

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

*Last updated: February 5, 2026*
