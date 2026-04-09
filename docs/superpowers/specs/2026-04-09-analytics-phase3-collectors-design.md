# rumi_analytics Phase 3: Current-State Daily Collectors

**Date**: 2026-04-09
**Status**: Design
**Prereqs**: Phase 1 (complete, merged PR #61), Phase 0 audit (complete, all source queries exist)

## Context

Phase 1 deployed the analytics canister skeleton with one collector (daily TVL from backend) and the `/api/supply` HTTP endpoint. Phase 3 extends this with three more daily collectors that pull current-state data from source canisters. None depend on event tailing (that's Phase 4).

The Phase 0 audit confirmed all needed source queries already exist:
- `rumi_protocol_backend.get_all_vaults()` returns `vec CandidVault`
- `rumi_protocol_backend.get_protocol_status()` returns `ProtocolStatus` (already used in Phase 1)
- `rumi_stability_pool.get_pool_status()` returns `StabilityPoolStatus`
- `rumi_3pool.get_pool_status()` returns `PoolStatus`

No source canister changes needed.

## Collectors

### 1. Extend TVL collector (`collectors/tvl.rs`)

The existing daily TVL collector calls `backend.get_protocol_status()` and writes a `DailyTvlRow`. Extend it to also pull stability pool and 3pool data in the same daily tick.

**New fields on DailyTvlRow** (all `Option<T>` to preserve backward compat with existing rows):

| Field | Type | Source |
|-------|------|--------|
| `stability_pool_deposits_e8s` | `Option<u64>` | `stability_pool.get_pool_status().total_deposits_e8s` |
| `three_pool_reserve_0_e8s` | `Option<u128>` | `three_pool.get_pool_status().balances[0]` |
| `three_pool_reserve_1_e8s` | `Option<u128>` | `three_pool.get_pool_status().balances[1]` |
| `three_pool_reserve_2_e8s` | `Option<u128>` | `three_pool.get_pool_status().balances[2]` |
| `three_pool_virtual_price_e18` | `Option<u128>` | `three_pool.get_pool_status().virtual_price` |
| `three_pool_lp_supply_e8s` | `Option<u128>` | `three_pool.get_pool_status().total_lp_tokens` |

The collector makes three async calls (backend, stability pool, 3pool). If stability pool or 3pool calls fail, the row is still written with the backend data populated and the failed fields as `None`. Errors increment the per-source error counter. This is a change from Phase 1 where the whole row was skipped on error. The rationale: partial data is better than no data for a composite snapshot.

### 2. New vault collector (`collectors/vaults.rs`)

Daily snapshot of all open vaults, producing both per-collateral and protocol-wide aggregates.

**DailyVaultSnapshotRow** (one row per day, stored in `DAILY_VAULT_SNAPSHOTS` StableLog):

| Field | Type | Description |
|-------|------|-------------|
| `timestamp_ns` | `u64` | Snapshot time |
| `total_vault_count` | `u32` | Protocol-wide open vault count |
| `total_collateral_usd_e8s` | `u64` | Protocol-wide collateral in USD terms |
| `total_debt_e8s` | `u64` | Protocol-wide icUSD debt |
| `median_cr_bps` | `u32` | Protocol-wide median collateral ratio in bps |
| `collaterals` | `Vec<CollateralStats>` | Per-collateral breakdown |

**CollateralStats**:

| Field | Type | Description |
|-------|------|-------------|
| `collateral_type` | `Principal` | Collateral token principal |
| `vault_count` | `u32` | Vaults using this collateral |
| `total_collateral_e8s` | `u64` | Total collateral deposited |
| `total_debt_e8s` | `u64` | Total icUSD borrowed against this collateral |
| `min_cr_bps` | `u32` | Lowest CR among vaults of this type |
| `max_cr_bps` | `u32` | Highest CR |
| `median_cr_bps` | `u32` | Median CR |

The collector calls `backend.get_all_vaults()`, groups by collateral type, computes stats per group and protocol-wide. CR is computed from each vault's own collateral ratio. The protocol-wide median is the median across all individual vault CRs, not a median of per-collateral medians.

**Serialization**: Since `collaterals` is variable-length, the row is Candid-encoded (same pattern as `SlimState`). The `Storable` impl uses `Bound::Unbounded`.

### 3. New stability pool collector (`collectors/stability.rs`)

Daily snapshot of stability pool state.

**DailyStabilityRow** (stored in `DAILY_STABILITY` StableLog):

| Field | Type | Description |
|-------|------|-------------|
| `timestamp_ns` | `u64` | Snapshot time |
| `total_deposits_e8s` | `u64` | Total stablecoin deposits |
| `total_depositors` | `u64` | Active depositor count |
| `total_liquidations_executed` | `u64` | Cumulative liquidation count |
| `total_interest_received_e8s` | `u64` | Cumulative interest revenue |
| `stablecoin_balances` | `Vec<(Principal, u64)>` | Per-stablecoin deposit breakdown |
| `collateral_gains` | `Vec<(Principal, u64)>` | Per-collateral gain totals |

The collector calls `stability_pool.get_pool_status()` and maps the response directly. Straightforward.

## Source wrappers

Two new files in `sources/`:

**`sources/stability_pool.rs`**: Wrapper for `get_pool_status()`. Returns a `StabilityPoolStatusSubset` with the fields we need (mirrors the pattern in `sources/backend.rs`).

**`sources/three_pool.rs`**: Wrapper for `get_pool_status()`. Returns a `ThreePoolStatusSubset` with balances, virtual_price, total_lp_tokens.

## Storage

**New MemoryIds** (from the reserved range in the design doc):
- `DAILY_VAULT_SNAPSHOTS`: MemoryId 2 (StableLog)
- `DAILY_STABILITY`: MemoryId 3 (StableLog)

MemoryId 1 is already `DAILY_TVL`. No new MemoryIds needed for the TVL extension since the row struct just gets wider.

**DailyTvlRow backward compatibility**: The `Storable` impl uses Candid encoding. Adding `Option` fields to the struct means old rows (which lack these fields) will decode with the new fields as `None`. Candid's record subtyping handles this automatically. No migration needed.

## Query endpoints

Two new query methods, same pagination pattern as `get_tvl_series`:

- `get_vault_series(RangeQuery) -> VaultSeriesResponse`
- `get_stability_series(RangeQuery) -> StabilitySeriesResponse`

Response types follow the same cursor pattern: `{ rows: Vec<Row>, next_from_ts: Option<u64> }`.

## Timer wiring

All three collectors fire on the existing daily (86400s) timer in `timers.rs`. The daily_snapshot function calls them sequentially (tvl, then vaults, then stability). If one fails, the others still run.

## HTTP endpoints

No new HTTP endpoints in Phase 3. The existing `/api/supply` is sufficient. Phase 7 adds the full HTTP layer.

## Testing

PocketIC integration tests extending the existing `pocket_ic_analytics.rs`:

1. **TVL extended fields**: Deploy analytics + mock stability pool + mock 3pool, advance past daily tick, verify new optional fields are populated.
2. **Vault snapshot**: Deploy analytics + mock backend with fixture vaults (multiple collateral types), advance past daily tick, query `get_vault_series`, verify per-collateral stats and protocol-wide aggregates.
3. **Stability snapshot**: Deploy analytics + mock stability pool, advance past daily tick, query `get_stability_series`, verify fields match mock data.
4. **Partial failure resilience**: Deploy analytics where stability pool call fails, verify TVL row still written with SP fields as None.
5. **Upgrade preserves new logs**: Write rows to all three logs, upgrade canister, verify rows survive.

## Candid interface updates

The `.did` file gets the new types (`DailyVaultSnapshotRow`, `CollateralStats`, `DailyStabilityRow`, `VaultSeriesResponse`, `StabilitySeriesResponse`) and two new query methods.

## What this does NOT include

- Event tailing (Phase 4)
- Holder tracking (Phase 4)
- Fast/hourly tiers (Phase 5)
- HTTP series endpoints (Phase 7)
- Any source canister modifications (Phase 0 confirmed none needed)
