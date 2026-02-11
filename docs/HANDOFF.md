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

## Git Branch Status (Updated Feb 10)

| Branch | Status | Action |
|--------|--------|--------|
| `main` | ✅ Contains staging merge + LICENSE + UI rebrand + XRC optimization | Production branch — backend deployed Feb 10 |
| `feature/ui-updates` | ✅ Merged into main (PR #4, Feb 10) | Can delete |
| `feature/xrc-cost-optimization` | ✅ Merged into main (PR #7, Feb 10) | Can delete |
| `feature/ckusdt-ckusdc-repayment` | ⏳ Agnes's PR #1 open → main | Awaiting review |
| `feature/ckusdt-ckusdc-repayment-fixes` | ⏳ Rob's PR #6 open → ckusdt branch | Merge after PR #1 |
| `feature/ii-wallet-send-receive` | ✅ Deployed to mainnet | Merge to main when stable |
| `feature/plug-wallet-reconnect` | ✅ Merged via PR #2 | Can delete |
| `staging` | ✅ Merged into main | Can delete |
| `main-backup-feb4` | Backup of main before staging merge | Keep for safety |
| `test/oisy-icrc2-repayment` | Test branch for Oisy icUSD ICRC-2 | DO NOT MERGE — test only |

---

## Deployment Log: February 10, 2026

### PRs Merged

1. **PR #4 — UI Rebrand + Page Reworks** (`feature/ui-updates` → `main`)
   - 34 commits, frontend-only
   - Had one merge conflict in HANDOFF.md (git branch status table) — resolved manually
   - Merged locally and pushed to main

2. **PR #7 — XRC Cost Optimization** (`feature/xrc-cost-optimization` → `main`)
   - 2 commits on top of UI branch (was branched from tip of `feature/ui-updates`)
   - Merged cleanly after PR #4 was in main
   - Backend changes: `main.rs` (async validate_call), `xrc.rs` (300s interval + on-demand fetch)

### Backend Build Fixes Applied

The backend had 33 pre-existing build errors from the staging merge. Fixed in this session:

1. **guard.rs ↔ state.rs mismatch** — `guard.rs` referenced `operation_guards`, `operation_guard_timestamps`, `operation_details` fields that didn't exist in `State`. Added the missing fields to `state.rs` with the correct types (`BTreeSet<String>`, `BTreeMap<String, u64>`, `BTreeMap<String, (Principal, String)>`).

2. **ckBTC multi-collateral stubs** — `main.rs` had references to `CollateralType`, `UsdCkBtc`, `fetch_ckbtc_rate()`, `ckbtc_margin_amount` etc. from the staging merge. These are future multi-collateral features with no underlying implementation. Commented out all ckBTC-specific code paths.

3. **Duplicate `#[update]` exports** — `partial_repay_to_vault` and `partial_liquidate_vault` had `#[update]` macros in both `main.rs` and `vault.rs`, causing linker duplicate symbol errors. Removed the `#[update]` from `vault.rs` (main.rs is the proper entry point with `validate_call()`).

### Backend Deployment

- **Canister:** `tfesu-vyaaa-aaaap-qrd7a-cai`
- **Method:** `dfx canister install --mode upgrade` (preserves stable memory)
- **Upgrade argument:** `(variant { Upgrade = record { mode = null } })`

#### Event Schema Migration Issue

The upgrade initially failed with: `missing field "liquidator_payment"` during event replay.

**Root cause:** The `PartialLiquidateVault` event schema changed between the previously deployed version and the current code:

| Field | Old Schema (deployed) | New Schema (current) |
|-------|----------------------|---------------------|
| Debt amount | `liquidated_debt: ICUSD` | `liquidator_payment: ICUSD` |
| Collateral | `collateral_seized: ICP` | `icp_to_liquidator: ICP` |
| Liquidator | `liquidator: Option<Principal>` | `liquidator: Principal` (required) |
| Price | `icp_rate: UsdIcp` | *(not present)* |

**Fix applied** (`f4466dc`):
- Added `#[serde(alias = "liquidated_debt")]` on `liquidator_payment`
- Added `#[serde(alias = "collateral_seized")]` on `icp_to_liquidator`
- Changed `liquidator` to `Option<Principal>` with `#[serde(default)]`
- Added `icp_rate: Option<UsdIcp>` with `#[serde(default)]`
- Updated `vault.rs` to pass `Some(caller)` and `Some(icp_rate)` when recording new events

This ensures old CBOR-encoded events in stable memory deserialize correctly with the new code.

Also added `#[serde(default)]` to `LiquidateVault.liquidator` for the same backward-compatibility reason (oldest events didn't have the `liquidator` field at all).

### Post-Deployment Verification

- `get_protocol_status` returns successfully
- `last_icp_rate = 0.0` immediately after upgrade (expected — first 300s timer hadn't fired yet)
- Protocol shows correct state: `total_icusd_borrowed = 409_380_000`, `total_icp_margin = 270_880_000`

### What Was NOT Deployed

- **Frontend** — UI rebrand was already deployed to production from the feature branch prior to this session. The merge to main was for codebase hygiene, not a new deploy.
- **PRs #1 and #6** (ckUSDT/ckUSDC repayment) — left for Agnes to review. PR #6 contains critical security fixes (100x decimal overcharge, missing ICRC-2 approval flow, missing validate_call on partial liquidation).

### ⚠️ Note for Future Upgrades

When modifying Event enum variants, always maintain backward compatibility with serde aliases/defaults. The canister replays ALL events from stable memory on every upgrade. If deserialization fails for any event, the entire upgrade traps and rolls back. Test event schema changes against the production event log before deploying.

---

## ✅ RESOLVED: Backend Build Errors (February 10, 2026)

Fixed as part of the Feb 10 deployment. See "Deployment Log: February 10, 2026" above for full details. Summary: added missing State fields for guard.rs, commented out ckBTC stubs, removed duplicate `#[update]` exports.

---

## ✅ DEPLOYED: XRC Cost Reduction (February 10, 2026)

### Previous Cost
- ICP/USD fetched every **60 seconds** via XRC
- 1,440 calls/day × 1B cycles/call = ~1.44T cycles/day ≈ **$1.95/day ($58.50/month)**

### Current (Deployed Feb 10)
- Background polling reduced to **300 seconds** + on-demand freshness for operations
- ~288 calls/day ≈ **$0.39/day ($11.70/month)** — **80% savings**
- See "XRC Price Oracle Cost Optimization" section below for full technical details

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
- **Minimum Collateral Ratio**: 133% (`dec!(1.33)` in code)
- **Recovery Mode**: Triggers when system-wide CR < 150% (liquidation threshold rises to 150%)
- **Read-Only Mode**: Triggers when system-wide CR < 100% or oracle < $0.01
- **Borrowing Fee**: 0.5% one-time (0% in Recovery mode)
- **Liquidation Bonus**: 10%
- **Price Oracle**: XRC canister, 300s background polling + 30s on-demand freshness for operations

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
| **Oisy** | 🔴 Greyed out ("Coming Soon") | ICRC-2 incompatible with ICP ledger; icUSD untested. Disabled in wallet selector. |

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

### ~~3. Left Nav Active Highlight Doesn't Track Page~~ ✅ RESOLVED
**Fix**: Replaced manual `window.location` check with SvelteKit `$page` store
**Branch**: `feature/ui-updates`

---

## UI Rebrand & Page Reworks (February 6–7, 2026)

**Branch:** `feature/ui-updates` (local only — NOT deployed to production)

**Goal:** Elevate the UI from template-feeling to a sleek, modern DeFi product. Make it feel like crypto people built it.

### Design System — `/docs/DESIGN_SYSTEM.md`
A formal design constitution was established and governs all UI decisions:
- **Three-color system**: indigo base (#080b16), purple/pink identity (#d176e8), emerald action (#34d399)
- **Typography**: Circular Std headings, Inter body/numbers
- **Primary Brand** (transactional pages): no gradients, serious infrastructure aesthetic
- **Secondary Brand** (marketing/educational): gradients allowed
- **Card hover**: purple inner glow on interactive card grids only
- **Button text**: dark on emerald fills
- **Risk colors**: amber (caution), red (danger). No green "safe" states.
- **Noise grain**: SVG feTurbulence at 3% opacity over body — felt, not seen
- **Depth cues**: Inset top-edge highlight + purple-tinted shadows on all cards

### Global CSS / Design Foundation
| Change | Details |
|--------|---------|
| **Background surfaces** | Indigo/blue-purple family: #080b16 (page), #0e1222 (surface1), #141a2e (surface2), #1a2139 (surface3) |
| **Noise grain** | `body::after` SVG fractalNoise at 3% opacity, fixed, pointer-events none |
| **Depth cues** | Inset highlight + 2-layer shadow on `.glass-panel`, `.glass-card`, `.icp-card`, `.price-card` |
| **Purple inner-glow hover** | Cards get faint purple glow on hover via inset box-shadow |
| **Ambient glow** | `body::before` radial gradient, indigo-tinted, centered top |
| **Color calibration** | Emerald (#34d399) for action, teal (#2DD4BF) for subtle accents, #d176e8 for identity/orientation |
| **Typography scale** | Logo 2rem, nav 0.9375rem, page titles 2rem bold purple-accent, key numbers Inter 700 tabular |
| **Debug toggle** | Debug panels hidden by default, Ctrl+D to toggle (dev mode only) |

### Header Redesign
- CSS Grid layout with true viewport-centered nav
- Green underline active state on nav items
- Single amber "Beta" chip, left of social icons, tooltip on hover

### Page Reworks

#### Borrow (Home) + Stability Pool
- **Action-first layout**: left column = action card, right column = protocol stats
- Stability Pool page stripped of pink gradients, matches Primary Brand
- Step numbers use muted text (not teal), headlines solid off-white

#### Learn → Docs
Replaced old "Learn" page with structured documentation (5 sub-pages sourced from actual Rust code):
- Before You Borrow, Liquidation Mechanics, What Can Go Wrong, Protocol Parameters, Beta Disclaimer
- **Important correction**: Old Learn page said 130% MCR. Actual code is 133% (`dec!(1.33)`).

#### Redeem + Treasury
- Removed old pink-to-purple gradient headlines → `.page-title` class
- Removed gradient buttons → `.btn-primary` class

#### Vaults — Vault Management Spec Compliance (Feb 7)
Dense, expandable inline vault list with full risk-forward UX:

| Feature | Implementation |
|---------|---------------|
| **CR-ascending sort** | Riskiest vaults always at top, vault ID tiebreaker |
| **Single active intent** | Add/Borrow/Repay mutually exclusive — others clear when one is populated |
| **Add Collateral Max** | Shows wallet ICP balance, neutral color, inline clickable text |
| **Borrow Max** | Amount that results in CR = 150% |
| **Repay Max** | min(wallet icUSD balance, outstanding debt) |
| **Max styling** | All three identical: `--rumi-text-muted`, no action color, subtle hover underline |
| **Input behavior** | User types freely — no clamping, no value substitution. Over-max inputs grey out the button. |
| **Over-max disable** | Buttons disabled + handler hard-guarded when input exceeds max (Add, Borrow, Repay all guarded) |
| **Single expanded vault** | Only one vault can be expanded at a time; opening another closes the previous and resets inputs |
| **Projected CR** | Shown inline next to action button, live color: neutral ≥150%, amber 140-149%, red <140% |
| **Action disable** | Buttons disabled when projected CR is below minimum |
| **Risk left-border** | Danger vaults get 2px red left edge, warning vaults get amber |
| **Stable ordering** | Expanding/collapsing a vault does NOT reorder the list |
| **No sort controls** | No dropdowns, toggles, or configuration for MVP |

#### Liquidations — Row-Card Redesign (Feb 7, v3)
Complete structural redesign of the liquidation experience, iterated through multiple passes:

**Layout: Three-zone card**
| Zone | Content |
|------|---------|
| **Left** | Risk stats: CR badge (semantic color + warning icon), Debt, Collateral |
| **Center** | "You receive" outcome: ICP amount (bold) + USD value (muted). Appears when user types input. |
| **Right** | Execution: "Amount to liquidate" input + "Liquidate" button |

**Interaction model:**
| Feature | Details |
|---------|---------|
| **Unified flow** | ONE liquidation path. User inputs icUSD amount, protocol handles full vs partial internally (≥99.9% of debt = full) |
| **No mode switching** | Removed "Partial / Full" distinction entirely |
| **Input freedom** | User types freely — no clamping, no value substitution |
| **Over-max behavior** | Input text + button grey out. Button unclickable. No error message on separate line. |
| **Max utility text** | Neutral color, hover underline. NOT a button, NOT action-colored. Shows "Max: ····" pulse placeholder while wallet balance loads. |
| **Max cap logic** | Calculates minimum icUSD needed to restore vault CR to ~150%, capped to min(wallet balance, vault debt, restoration amount) |
| **Liquidate button** | Emerald green (action color). Disabled until valid input > 0 and ≤ max. Hard-guarded in handler too. |
| **CR coloring** | Red <130%, amber 130-150%. Warning icon on danger. ONLY colored element in card. |
| **No hover expansion** | Subtle purple border glow only. No layout shifts. |
| **Sort** | CR ascending (riskiest first), vault ID tiebreaker |

**Copy (locked):**
- Input label: "Amount to liquidate" (not "Repay")
- Outcome: "You receive" / "0.4472 ICP $1.11" (no parentheses, no "Est.", no abbreviations)

**⚠️ KNOWN BUG: "You receive" reactivity**
The center-column seizure calculation does NOT update live when the user types. Currently requires clicking "Refresh" to recalculate. The root cause is Svelte reactivity — `liquidationAmounts` is a plain object and property mutations don't trigger re-renders. A self-assignment trick (`liquidationAmounts = liquidationAmounts`) was attempted but did not resolve the issue in production. This needs a proper fix, likely by:
- Converting `liquidationAmounts` to a Svelte store, OR
- Using a reactive `$:` block that watches a serialized version of the amounts, OR
- Moving to per-vault component state (like VaultCard does)

**Commits (feature/ui-updates):**
```
39c3608 fix: live-reactive seizure calculation + layout tweak (attempted, not working)
7851053 feat: three-zone liquidation card — outcome in center column
7f40df5 fix: show 'Max: ····' placeholder while wallet balance loads
5a85ddf fix: stop clamping liquidation input — grey out + disable instead
67427fb fix: cap liquidation max to restore CR ~150%, not full debt
fed0950 feat: liquidations row-card redesign — unified flow, no hover expansion
1dfaa33 feat: rework Liquidations page — profit-forward table layout
```

### Git Log (feature/ui-updates, key commits)
```
39c3608 fix: live-reactive seizure calculation + layout tweak
7851053 feat: three-zone liquidation card — outcome in center column
7f40df5 fix: show 'Max: ····' placeholder while wallet balance loads
5a85ddf fix: stop clamping liquidation input — grey out + disable instead
67427fb fix: cap liquidation max to restore CR ~150%, not full debt
fed0950 feat: liquidations row-card redesign — unified flow, no hover expansion
20ce879 fix: stop auto-clamping over-max inputs + hard-guard handlers
0ef8c3a fix: disable buttons when input exceeds max + single expanded vault
16c138a feat: grey out Oisy wallet with 'Coming Soon' in connect dialog
706fa4d docs: update HANDOFF.md with all UI rebrand + page rework details
187cfde fix: remove old pink gradients from Redeem and Treasury pages
41f7687 feat: complete vault management spec compliance
1dfaa33 feat: rework Liquidations page — profit-forward table layout
4deb63a feat: vault list CR-ascending sort + spec compliance fixes
f1f9662 feat: VaultCard rewrite + vault list cleanup + doc archive purge
760bf98 docs: add AVAI security review to beta disclaimer and risks page
f19b6a9 fix: remove gradient from Docs title — Primary Brand
4467e2c feat: replace Learn page with Docs section
82febf4 feat: single beta chip in header, redesign Learn + Stability pages
58cf25b feat: lock emerald (#34d399) as action color, teal (#2DD4BF) as subtle accent
3594a48 feat: action-first layout for Borrow and Liquidate pages
b25584e feat: top nav rail + purple inner-glow card hover
79674cb feat: shift background family from red-purple to indigo/blue-purple
407ec3f fix: hide debug panels by default, toggle with Ctrl+D
aceda3d feat: add noise grain + depth cues for living surface feel
a76e90d feat: implement design constitution compliance
1c7ceb4 fix: bring purple back into design system
6e1f7c6 feat: UI rebrand - dark precision theme with teal accent
```

### Deferred
- Vault close navigation bug (bigger task, saved for last)
- ckToken support in Send modal (post-rebrand)
- Deploying `feature/ui-updates` to production (needs review first)

---

## Protocol Constants Centralization (February 9, 2026)

**Branch:** `feature/ui-updates` — Commit `6bff0d1`

### Problem
Protocol-critical numbers (minimum CR, liquidation threshold) were hardcoded in multiple places across the frontend with inconsistent values. `VaultCard.svelte` had `MINT_MINIMUM = 1.5` and `E8S = 100_000_000` as local constants, and `getRiskLevel()` used hardcoded thresholds (`1.5`, `1.4`) that didn't match actual protocol parameters. The `config.ts` settings object also had wrong values (`minCollateralRatio: 130`, `liquidationThreshold: 125`). If protocol parameters ever change, tracking down every hardcoded instance would be a nightmare.

### Solution: `src/vault_frontend/src/lib/protocol.ts`
Created a single source of truth for protocol parameters on the frontend:

```typescript
export const MINIMUM_CR = 1.5;        // 150% — min to open/borrow (backend: RECOVERY_COLLATERAL_RATIO)
export const LIQUIDATION_CR = 1.33;   // 133% — liquidation threshold (backend: MINIMUM_COLLATERAL_RATIO)
export const E8S = 100_000_000;
```

**Backend source:** `src/rumi_protocol_backend/src/lib.rs` lines 56–57:
```rust
pub const RECOVERY_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.5));   // 150%
pub const MINIMUM_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.33));   // 133%
```

**Naming note:** The backend names are confusing — `MINIMUM_COLLATERAL_RATIO` is actually the *liquidation* threshold, while `RECOVERY_COLLATERAL_RATIO` is the minimum to open/borrow. The frontend names (`MINIMUM_CR`, `LIQUIDATION_CR`) are more intuitive.

### Risk Level Thresholds (corrected)
| CR Range | Risk Level | Color | Icon |
|----------|-----------|-------|------|
| ≥ 150% (`MINIMUM_CR`) | `normal` | Neutral white | None |
| 133%–150% (`LIQUIDATION_CR` to `MINIMUM_CR`) | `warning` | Amber | ⚠ |
| ≤ 133% (`LIQUIDATION_CR`) | `danger` | Red | ⚠ |

**Exception:** Projected CR preview (when user types into Add Collateral / Borrow / Repay fields) uses green for `normal` to signal "this action improves your position."

### Migration Checklist
`VaultCard.svelte` now imports from `$lib/protocol` instead of local constants. Other files that may still have hardcoded values should be migrated:
- [ ] `config.ts` — `CONFIG.settings` object has wrong values (130%, 125%) — remove or update
- [ ] Liquidation page components — check for hardcoded ratio thresholds
- [ ] Borrow page — check max borrowable calculations
- [ ] Any future components that reference protocol ratios

---

## ✅ DEPLOYED: XRC Price Oracle Cost Optimization (February 9–10, 2026)

**Branch:** `feature/xrc-cost-optimization` — merged to main and **deployed to mainnet Feb 10**

### Problem
The XRC (Exchange Rate Canister) was being polled every 60 seconds for the ICP/USD price, costing ~$58/month in cycles. This is the single largest operational cost for the backend canister. Most of these calls are wasted — the price only matters when a user actually performs a vault operation.

### Solution: Lazy Polling + On-Demand Freshness

**Two-layer approach:**

1. **Background timer polls lazily every 300s (5 min)** — just keeps a reasonably fresh price in state for display/queries on the frontend. Reduces cycle cost by ~80% to ~$12/month.

2. **On-demand fetch for price-sensitive operations** — when any price-sensitive function is called, `validate_call()` (now async) checks if the cached price is older than 30 seconds. If so, it triggers an immediate XRC fetch before proceeding. The user's operation always uses a fresh price.

### Code Changes

| File | Change |
|------|--------|
| `xrc.rs` | `FETCHING_ICP_RATE_INTERVAL`: 60s → 300s |
| `xrc.rs` | Added `PRICE_FRESHNESS_THRESHOLD_NANOS` (30s) |
| `xrc.rs` | Added `ensure_fresh_price()` — checks cache age, fetches on-demand if stale |
| `main.rs` | `validate_call()` changed from sync → async, now calls `ensure_fresh_price().await` |
| `main.rs` | All 14 callers updated: `validate_call()?` → `validate_call().await?` |

### Price-Sensitive Functions (14 total)

All go through `validate_call()` → `ensure_fresh_price()`:

**Vault operations (user-initiated, all need fresh price):**
- `open_vault` — calculates collateral ratio from price
- `borrow_from_vault` — calculates how much can be minted at current CR
- `repay_to_vault` — updates CR after debt reduction
- `partial_repay_to_vault` — same as repay
- `add_margin_to_vault` — updates CR after collateral addition
- `close_vault` — needs accurate CR for safety checks
- `withdraw_collateral` — needs accurate CR to prevent undercollateralization
- `withdraw_and_close_vault` — same as close

**Liquidations (user-initiated, critical — must have fresh price):**
- `liquidate_vault` — determines if vault is actually undercollateralized
- `partial_liquidate_vault` — same

**Redemptions (user-initiated, needs fresh price):**
- `redeem_icp` — calculates ICP to return for icUSD redeemed

**Stability pool (user-initiated, arguably don't need fresh price):**
- `provide_liquidity` — deposits icUSD into stability pool (no CR calculation)
- `withdraw_liquidity` — withdraws icUSD from stability pool (no CR calculation)
- `claim_liquidity_returns` — claims accumulated returns (no CR calculation)

> **Note:** The three stability pool functions don't perform collateral ratio calculations and arguably don't need a fresh price. We left them with the price check for now (conservative approach), but they could be split out to skip the on-demand fetch if we want to save users the occasional 1-2s delay.

> **Note:** `redeem_icp` is named for the current ICP-only collateral. When we add ckBTC, ckETH, or other collateral types, the redemption function will need to be generalized (e.g., `redeem_collateral` with a collateral type parameter) and each collateral's price will need its own freshness guarantee. This is a future architecture concern to revisit when multi-collateral is implemented.

### Concurrency Safety

The existing `FetchXrcGuard` prevents concurrent XRC calls. If two users trigger actions simultaneously when the price is stale, only one XRC call fires. The second user's `ensure_fresh_price()` calls `fetch_icp_rate()` which silently returns (guard blocks it), but by then the first fetch has already updated the price in state, so the subsequent `check_price_not_too_old()` passes.

### Future Work: Admin-Configurable Interval

Discussed but not yet implemented: a controller-only `set_price_interval(secs: u64)` function that stores the interval in stable memory. Would allow changing the polling interval without redeploying. Currently the canister has zero admin-only functions — this would be the first, establishing the controller-check pattern. See [chat log](https://claude.ai/chat/c89c7960-62cd-40fc-8c69-63dd762bb743) for full discussion.

### Future Work: Multi-Collateral Price Feeds

When additional collateral types (ckBTC, ckETH) are added, `ensure_fresh_price()` will need to become collateral-aware — checking and refreshing the price for the specific collateral involved in the operation, not just ICP. The `FETCHING_CKBTC_RATE_INTERVAL` and `fetch_ckbtc_rate()` references already exist in `setup_timers()` in `main.rs` (from Agnes's staging merge) but the corresponding functions in `xrc.rs` haven't been implemented yet.

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

*Last updated: February 7, 2026*
