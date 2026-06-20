# Chains Liquidation — Increment 1: MultiChainStateV6 + Unified Supply Invariant

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`. This increment touches the protocol's most safety-critical code (state migration + the supply invariant). The first task de-risks the state-wipe; do NOT proceed to the invariant rewrite until the V5→V6 round-trip test is green.

**Goal:** Bump the persisted multi-chain root to `MultiChainStateV6` (reserve + pending-burn + liquidation-config + sp-attempted state, plus a `pending_liquidation` vault marker) and generalize the supply invariant to `sum(chain_supplies) == debt + reserve_backing + pending_chain_burn` — purely the accounting + config scaffolding, NO liquidation execution. Ships behind the existing dev-gating; the new terms are all 0 until Increment 2+ uses them, so it is behavior-preserving.

**Architecture:** Follow the documented additive-reshape recipe (`multi_chain_state.rs:8-30`): keep `MultiChainStateV5` byte-verbatim, add `MultiChainStateV6` with the V5 fields + new `#[serde(default)]` fields, move the impl to V6, rebind `pub type MultiChainState = MultiChainStateV6`. `State.multi_chain` already uses the alias, so the only signature churn is the ~40 functions that hard-code `&mut MultiChainStateV5` → rename to the alias `MultiChainState` so this is the LAST time a bump needs a rename.

**Tech stack:** Rust, ciborium/CBOR persisted state, the chains module, `cargo test` (unit + the V5→V6 CBOR round-trip + the conflux PocketIC regression).

**HIGH review findings folded in (from the spec Appendix A):**
- **#1 (interest conservation):** the invariant counts REALIZED supply vs `debt_e8s`. `pending_interest_mint_e8s` is NOT yet in `chain_supplies` (it mints on confirm), so it must NOT appear on the invariant RHS. The reserve/burn terms only ever move ALREADY-realized debt. A unit test must assert the invariant holds for a vault carrying non-zero `pending_interest_mint_e8s`.
- **#2 (apply_supply_delta false-halt):** the current strict-equality self-check is `sum(supplies) == sum(debt)`. Generalizing to a 3-term RHS, the reserve-shift helper must move debt→reserve in ONE atomic mutation so the equality holds at every checkpoint; `apply_supply_delta` keeps rejecting a delta that would break the (generalized) equality. Add the reserve/burn terms to BOTH the periodic self-check and `apply_supply_delta`'s check in the SAME commit so neither sees a half-updated RHS.

---

### Task 1: MultiChainStateV6 bump + V5→V6 round-trip safety test (DE-RISK THE WIPE FIRST)

**Files:** `chains/multi_chain_state.rs`, `chains/tests_multi_chain_state_v2.rs`, and a per-file alias rename across the ~25 files listed by `grep -rln MultiChainStateV5 src`.

- [ ] **Step 1: Add the V6 struct.** In `multi_chain_state.rs`, after the V5 struct, add `MultiChainStateV6` = every V5 field verbatim (the 4 base fields undecorated; the 11 existing `#[serde(default)]` fields) PLUS, each `#[serde(default)]`:
```rust
/// Bot-path (PSM) reserve backing per chain (e8s): icUSD value backed by
/// bot-held stable, NOT by an open vault. Part of the unified invariant RHS.
pub reserve_backing_e8s: BTreeMap<ChainId, u128>,
/// Bot-held settle-stable (USDC) per chain in native base units (18-dec on
/// eSpace), pending the manual bridge to cUSDT reserves. Informational/audit.
pub reserve_usdc_native: BTreeMap<ChainId, u128>,
/// SP-path debt mid-burn per chain (e8s): the SP has absorbed it (icUSD will
/// burn on-chain) but the burn is not yet confirmed. Part of the invariant RHS.
pub pending_chain_burn_e8s: BTreeMap<ChainId, u128>,
/// Vaults whose bot liquidation failed and were escalated to the SP exactly
/// once (the no-retry guard; once present, never re-attempted by the SP).
pub sp_attempted_chain_vaults: BTreeSet<u64>,
```
(The `chain_liquidation_configs` map + its `ChainLiquidationConfigV1` type are added to V6 in Task 4 — still pre-deploy, so the same V6 struct, no V7.)

- [ ] **Step 2: Move the impl + rebind the alias.** Change `impl MultiChainStateV5` → `impl MultiChainStateV6` (the helper methods now live on V6), and `pub type MultiChainState = MultiChainStateV5;` → `= MultiChainStateV6;`. Update the in-file `manual_price_tests` to construct `MultiChainStateV6::default()`.

- [ ] **Step 3: Alias-rename the function signatures.** For every NON-definition file that references `MultiChainStateV5` (all of `grep -rln MultiChainStateV5 src` except `multi_chain_state.rs` and the version-specific round-trip tests in `tests_multi_chain_state_v2.rs`), replace `MultiChainStateV5` → `MultiChainState`. In `supply.rs`, the historical `migrate_multi_chain_state(v1: MultiChainStateV1) -> MultiChainStateV2` template stays version-explicit; only the live function signatures (`&mut MultiChainStateV5` → `&mut MultiChainState`) change.

- [ ] **Step 4: Write the V5→V6 round-trip test.** In `tests_multi_chain_state_v2.rs`, mirroring `v3_cbor_snapshot_decodes_into_v4_without_wiping_state`:
```rust
#[test]
fn v5_cbor_snapshot_decodes_into_v6_without_wiping_state() {
    // Build a populated V5, encode to CBOR, decode as V6: every carried field
    // survives and the four new V6 fields come up empty (NOT a wipe).
    let mut v5 = MultiChainStateV5::default();
    v5.chain_supplies.insert(ChainId(1030), 100 * E8);
    // ... seed a vault, a contract, a manual price, a nonce ...
    let mut buf = Vec::new();
    ciborium::into_writer(&v5, &mut buf).unwrap();
    let v6: MultiChainStateV6 = ciborium::from_reader(&buf[..]).unwrap();
    assert_eq!(v6.chain_supplies, v5.chain_supplies, "carried field survived");
    assert!(v6.reserve_backing_e8s.is_empty(), "new field defaulted, not wiped");
    assert!(v6.pending_chain_burn_e8s.is_empty());
    assert!(v6.sp_attempted_chain_vaults.is_empty());
}
```

- [ ] **Step 5: Build + test.** `cargo build -p rumi_protocol_backend` (compiler verifies every renamed signature), then `cargo test -p rumi_protocol_backend --lib chains::tests_multi_chain_state` — the round-trip test MUST pass before continuing. Commit: `feat(chains): MultiChainStateV6 (reserve + pending-burn + sp-attempted)`.

---

### Task 2: The `pending_liquidation` vault marker (no new status)

**Files:** `chains/monad/chain_vault.rs` (the `ChainVaultV1` struct), `chains/vault.rs` (the owner-write guards).

- [ ] **Step 1:** Add to `ChainVaultV1`, `#[serde(default)]`: `pub pending_liquidation: Option<PendingLiquidationV1>,` and define `PendingLiquidationV1 { op_id: u64, debt_to_clear_e8s: u128, collateral_reserved_native: u128, tier: LiquidationTier, started_at_ns: u64 }` + `enum LiquidationTier { Bot, StabilityPool }`. (Resolved spec §3.1: a marker, NOT a new `ChainVaultStatus` variant, so no `match` exhaustiveness churn and the existing pending-marker write-guards apply.)
- [ ] **Step 2:** Add `LiquidationInFlight` rejection arms to `WithdrawError`/`BorrowError`/`OpenVaultError` as appropriate, and reject owner write-ops while `pending_liquidation.is_some()` (mirrors the `MintInFlight` guard). Tests: a vault with a `pending_liquidation` marker rejects borrow/withdraw/close. Update the `ChainVaultV1` literal-construction sites in tests (the compiler lists them) to add `pending_liquidation: None`.
- [ ] **Step 3:** Build + test + commit `feat(chains): pending_liquidation vault marker + write guards`.

---

### Task 3: The unified supply invariant + apply_supply_delta (HIGH #1, #2)

**Files:** `chains/supply.rs` (`reconcile_chain_supply` / the self-check / `apply_supply_delta`).

- [ ] **Step 1: Generalize the invariant helper.** Add `pub fn chain_backing_rhs_e8s(state, chain) -> u128` = `sum(vault.debt_e8s for chain) + reserve_backing_e8s[chain] + pending_chain_burn_e8s[chain]`. The invariant is `chain_supplies[chain] == chain_backing_rhs_e8s(chain)`. With all-zero reserve/burn it reduces to the existing `supply == debt`, so it is behavior-preserving.
- [ ] **Step 2: Write the failing tests FIRST:** (a) invariant holds with non-zero `reserve_backing_e8s` (supply unchanged, debt down by X, reserve up by X); (b) invariant holds with non-zero `pending_chain_burn_e8s` (supply still == old, debt down, pending-burn up — pre-confirm); (c) **HIGH #1:** invariant holds for a vault carrying non-zero `pending_interest_mint_e8s` (it is NOT on the RHS — only realized `debt_e8s` is); (d) **HIGH #2:** `apply_supply_delta` rejects a delta that would break the GENERALIZED equality, and accepts one that preserves it.
- [ ] **Step 3:** Update `reconcile_chain_supply`, the periodic self-check, AND `apply_supply_delta`'s equality check to the 3-term RHS in the SAME commit (so no checkpoint sees a half-updated RHS — HIGH #2). Run; make green. Commit `feat(chains): unified supply invariant (debt + reserve + pending-burn)`.

---

### Task 4: `apply_debt_to_reserve_shift` + the liquidation config (getter/setter)

**Files:** `chains/supply.rs`, NEW `chains/liquidation_config.rs`, `chains/multi_chain_state.rs` (add `chain_liquidation_configs` to V6), `main.rs` (admin endpoints).

- [ ] **Step 1:** `pub fn apply_debt_to_reserve_shift(state, chain, vault_id, cleared_e8s, stable_native)` — atomically: `vault.debt_e8s -= cleared`, `reserve_backing_e8s[chain] += cleared`, `reserve_usdc_native[chain] += stable_native`, leaving `chain_supplies` UNCHANGED (no burn). Asserts the invariant holds after. NO liquidation caller yet — exercised by a unit test proving a reserve shift keeps the invariant balanced (the Increment-2 bot path will call it).
- [ ] **Step 2:** `chains/liquidation_config.rs`: `ChainLiquidationConfigV1 { dex: DexKind, router, factory, pair, collateral_token, settle_stable_token, slippage_cap_bps, restore_target_cr_e4, enabled: bool }` + `enum DexKind { UniswapV2 }`. Add `chain_liquidation_configs: BTreeMap<ChainId, ChainLiquidationConfigV1>` to `MultiChainStateV6` (`#[serde(default)]`). Dev-gated `set_chain_liquidation_config` / `get_chain_liquidation_config` in `main.rs`. Tests + the V6 round-trip test extended to seed + assert the config survives.
- [ ] **Step 3:** Build + test + commit.

---

### Task 5: Event variants + verify + PR

- [ ] **Step 1:** Add the forward-looking event variants (`ChainVaultLiquidated`, `ChainReserveCredited`, `ChainCfxClaimSettled`, `ChainLiquidationDeferred`) to the chains `Event` enum (unused until Increment 2; additive, like the config was in Increment 0). Confirm the candid round-trip test for the event enum still passes.
- [ ] **Step 2: Full verify.** `cargo test -p rumi_protocol_backend --lib` (incl. the V5→V6 round-trip) + `--bin` + the conflux + interest PocketIC suites (`POCKET_IC_BIN=…`). Rebuild the wasm first (PocketIC `include_bytes!`s it).
- [ ] **Step 3: PR** against `main`: "Increment 1 — MultiChainStateV6 + unified supply invariant (accounting scaffolding, no execution)". Note the V5→V6 round-trip proof, the all-zero behavior-preservation, and that the deploy is a state-preserving upgrade (the new fields default empty on the live kvg63 snapshot).

---

## Self-review
- **Spec coverage:** implements spec §3 (state + versioning), §5 (unified invariant), the §3.1 marker resolution, and the §8 config seam (getter/setter only). Execution (sizing, the swap, the SP) is Increments 2-4, untouched here.
- **Wipe safety:** the V5→V6 round-trip test (Task 1 Step 4) is the gate; nothing proceeds until it passes. Every new persisted field/struct carries `#[serde(default)]`; the four base fields stay undecorated.
- **Behavior preservation:** all new RHS terms are 0 until Increment 2, so the generalized invariant equals the old `supply == debt` on the live snapshot — the staging upgrade changes no behavior.
- **HIGH findings:** #1 (pending_interest excluded from RHS) and #2 (3-term RHS updated atomically in self-check + apply_supply_delta together) are pinned by Task 3 Step 2 tests (c) and (d).
