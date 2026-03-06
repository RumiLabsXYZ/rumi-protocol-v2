# Backend Stablecoin Registry Refactor — Scoping Notes

**Date:** 2026-03-05
**Status:** Scoping only — no implementation plan yet

## Goal

Replace the hardcoded `StableTokenType::CKUSDT / CKUSDC` enum and dual liquidation endpoints in the backend with a dynamic stablecoin registry, matching what the stability pool already has.

## Estimated Effort

2–3 focused sessions. Moderate complexity — mostly plumbing and endpoint consolidation, no new algorithms.

## What Needs to Change in the Backend

### 1. Add `BTreeMap<Principal, StablecoinConfig>` registry to backend state
- Similar to what the stability pool already has
- Admin endpoints: `register_stablecoin`, `update_stablecoin_status`

### 2. Unify the two liquidation endpoints
- Currently: `liquidate_vault_partial` (icUSD) vs `liquidate_vault_partial_with_stable` (ckstables)
- Target: one `liquidate_vault_partial` that accepts any registered stablecoin ledger principal
- Medium effort — lots of duplicated code to merge, but it's mostly deletion

### 3. Replace hardcoded transfer functions
- Currently: `transfer_icusd_from` and `transfer_stable_from` are separate functions with hardcoded ledger IDs
- Target: one generic `transfer_stablecoin_from(ledger_id, ...)`

### 4. Update `.did` and `main.rs` endpoint wiring
- Small effort

### 5. Seed registry in `post_upgrade` migration
- On first upgrade, populate the registry from current hardcoded values so existing state survives

### 6. Remove dead code
- `StableTokenType` enum
- `VaultArgWithToken` struct
- Stability pool's `determine_stable_token_type()` shim in `liquidation.rs`

## What's Already Done (Stability Pool Side)

The stability pool is **already fully registry-based** from the refactor on `feat/stability-pool-refactor`. The only bridge to the backend's hardcoded enum is the `determine_stable_token_type()` function in `src/stability_pool/src/liquidation.rs` — it goes away once the backend has a registry.

## Key Files to Touch

| File | Change |
|------|--------|
| `src/rumi_protocol_backend/src/state.rs` | Add `stablecoin_registry` field, registration logic |
| `src/rumi_protocol_backend/src/vault.rs` | Merge dual liquidation endpoints, genericize transfers |
| `src/rumi_protocol_backend/src/management.rs` | Merge `transfer_icusd_from` / `transfer_stable_from` |
| `src/rumi_protocol_backend/src/lib.rs` | Remove `StableTokenType`, `VaultArgWithToken`; add admin endpoints |
| `src/rumi_protocol_backend/src/main.rs` | Wire new endpoints, remove old ones |
| `src/rumi_protocol_backend/rumi_protocol_backend.did` | Update Candid interface |
| `src/stability_pool/src/liquidation.rs` | Remove `determine_stable_token_type()`, call unified endpoint |

## Notes

- The vault math itself doesn't change — it's all plumbing
- The two parallel code paths exist because ckstables were bolted on after icUSD
- This refactor is a prerequisite for adding any future stablecoins without code changes
