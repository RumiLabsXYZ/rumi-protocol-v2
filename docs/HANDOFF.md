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
| `main` | ✅ Contains staging merge + LICENSE | Production branch |
| `feature/ii-wallet-send-receive` | ✅ **Active** — deployed to mainnet | Merge to main when stable |
| `staging` | ✅ Merged into main | Can delete |
| `main-backup-feb4` | Backup of main before staging merge | Keep for safety |
| `feature/liquidation-price-check` | Has 1 unmerged commit (price validation) | Merge separately |
| `test/oisy-icrc2-repayment` | Test branch for Oisy icUSD ICRC-2 | DO NOT MERGE — test only |

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
- **Minimum Collateral Ratio**: 133% (`dec!(1.33)` in code)
- **Recovery Mode**: Triggers when system-wide CR < 150% (liquidation threshold rises to 150%)
- **Read-Only Mode**: Triggers when system-wide CR < 100% or oracle < $0.01
- **Borrowing Fee**: 0.5% one-time (0% in Recovery mode)
- **Liquidation Bonus**: 10%
- **Price Oracle**: XRC canister, 60-second fetch interval

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

### ~~2. Plug Wallet Auto-Reconnect~~ ✅ RESOLVED

### 3. Left Nav Active Highlight Doesn't Track Page
**Problem**: The highlight/active indicator in the left sidebar navigation doesn't move when the user navigates to a different page
**Status**: Being addressed in `feature/ui-updates` branch

---

## UI Rebrand (February 6, 2026)

**Branch:** `feature/ui-updates`

**Goal:** Elevate the UI from template-feeling to a sleek, modern DeFi product ready for public launch.

### Design System — `/docs/DESIGN_SYSTEM.md`
A formal design constitution was established and governs all UI decisions:
- **Three-color system**: indigo base (#080b16), purple/pink identity (#d176e8), emerald action (#34d399)
- **Typography**: Circular Std headings, Inter body
- **Primary Brand** (transactional pages): no gradients, serious infrastructure aesthetic
- **Secondary Brand** (marketing/educational): gradients allowed
- **Card hover**: purple inner glow on interactive card grids only
- **Button text**: dark on emerald fills

### Completed Work
| Change | Details |
|--------|---------|
| **Header redesign** | CSS Grid layout, viewport-centered nav, green underline active state |
| **Action-first layout** | Borrow + Liquidate pages: left = action card, right = protocol stats |
| **Stability Pool rewrite** | Stripped pink gradients, now matches Primary Brand |
| **Learn → Docs** | 5 sub-pages sourced from actual Rust code (not assumptions) |
| **Beta chip** | Single amber pill in header, left of social icons, tooltip on hover |
| **Color calibration** | Emerald (#34d399) for action, teal (#2DD4BF) for subtle accents |
| **Typography scale** | Logo 2rem, nav 0.9375rem, page titles 2rem bold |

### Docs Section (`/docs` route)
Replaced the old "Learn" page with structured documentation:
- **Before You Borrow** — fees, minimums, closing process
- **Liquidation Mechanics** — triggers, 10% bonus, worked example, protocol modes
- **What Can Go Wrong** — price volatility, oracle failure, smart contract risk, cascades
- **Protocol Parameters** — all constants from `lib.rs` / `state.rs`
- **Beta Disclaimer** — no warranty, no audit, canister control, not financial advice

**Important correction**: Old Learn page said 130% MCR. Actual code is 133% (`dec!(1.33)`).

### Deferred
- Vault close navigation bug (bigger task)
- ckToken support in Send modal (post-rebrand)
- Vault list/detail view polish

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
- **Styling**: Custom CSS with design system variables (see `/docs/DESIGN_SYSTEM.md`), Tailwind being phased out
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
| `/docs/Oisy_integration_handoff.md` | Oisy wallet integration details + ICRC-21 root cause analysis |
| `/docs/archive/OISY_ICRC2_TEST_SESSION_HANDOFF.md` | Oisy icUSD test details |
| `/docs/DESIGN_SYSTEM.md` | UI design constitution — colors, typography, component rules |
| `/docs/OISY_IMPLEMENTATION_COMPLETE.md` | Original Oisy integration work |
| `/ACKNOWLEDGMENTS.md` | Contributor credits |
| `/LICENSE` | MIT License |

---

*Last updated: February 6, 2026*
