# Chains Liquidation — Increment 0: Debt Ceiling & Min-Debt Enforcement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce the already-declared-but-inert `min_vault_debt_e8s` and `debt_ceiling_e8s` config on the chain-vault open and borrow paths, and raise the Conflux open gate to 150% — closing the "debt ceiling not enforced" half of the audit's prod blocker, independently of the liquidation engine.

**Architecture:** The `ChainCollateralConfig` (chains/collateral_config.rs) already carries `min_vault_debt_e8s` and `debt_ceiling_e8s: Option<u128>`, but `open_chain_vault_in_state` / `borrow_chain_vault_in_state` (chains/vault.rs) consume only `min_cr_e4`. We thread the two extra config values into both functions, reject opens/borrows that fall below the min-debt floor or push the chain's total outstanding+in-flight debt over the ceiling, and set Conflux's config to 150% min CR + a depth-bound ceiling. No state-shape change, so NO `MultiChainStateV<N>` bump.

**Tech Stack:** Rust, ic-cdk, the existing chains module + its `tests_vault.rs` unit suite (pure `MultiChainStateV5` in-memory tests, no PocketIC needed).

**Scope note:** This is the first of six increments from `docs/superpowers/specs/2026-06-19-chains-liquidation-design.md` (§12). Increments 1-5 (the V6 state bump + unified invariant, bot detection/sizing, the tECDSA Swappi swap, the SP cross-chain fallback, follow-ups) each get their own plan as they are reached, because their TDD detail depends on the V6 accounting firming up as this lands.

---

### Task 1: Min-debt + debt-ceiling rejection variants on the open path

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs` (the `OpenVaultError` enum, and `open_chain_vault_in_state` ~line 228-304)
- Test: `src/rumi_protocol_backend/src/chains/tests_vault.rs`

- [ ] **Step 1: Add a `total_chain_debt_including_pending_e8s` helper (if absent)**

In `vault.rs`, near `collateral_ratio_e4`, add (grep first; if a `total_chain_vault_debt_e8s` already exists, reuse it and skip):

```rust
/// Sum of confirmed debt + intended/in-flight mints for all vaults on `chain`.
/// Used to enforce a per-chain debt ceiling: an open/borrow must not push this
/// over the configured cap. Counts `pending_mint_e8s` and `pending_interest_mint_e8s`
/// so concurrent in-flight opens cannot collectively breach the ceiling.
pub fn total_chain_debt_including_pending_e8s(state: &MultiChainStateV5, chain: ChainId) -> u128 {
    state
        .chain_vaults
        .values()
        .filter(|v| v.collateral_chain == chain && v.status != ChainVaultStatus::Closed)
        .map(|v| v.debt_e8s.saturating_add(v.pending_mint_e8s).saturating_add(v.pending_interest_mint_e8s))
        .fold(0u128, |acc, d| acc.saturating_add(d))
}
```

- [ ] **Step 2: Add the two `OpenVaultError` variants**

```rust
/// Resulting vault debt is below the chain's `min_vault_debt_e8s` floor.
BelowMinDebt { debt_e8s: u128, min_e8s: u128 },
/// The open would push the chain's total outstanding+in-flight debt over `debt_ceiling_e8s`.
DebtCeilingExceeded { would_be_e8s: u128, ceiling_e8s: u128 },
```

- [ ] **Step 3: Write the failing tests**

In `tests_vault.rs`, mirroring the existing open tests (find an existing `open_*` test for the setup helper):

```rust
#[test]
fn open_rejects_debt_below_min_vault_debt() {
    let mut s = setup_registered_chain(); // existing helper: chain config + price set
    let r = open_chain_vault_in_state(
        &mut s, TEST_CHAIN, principal(), custody(), 1_000 * E18,
        5_000_000, // 0.05 icUSD < 10_000_000 floor
        recip(), is_valid_evm_address, "MON",
        13_300, /*min_vault_debt_e8s*/ 10_000_000, /*debt_ceiling*/ None, 0, 1,
    );
    assert!(matches!(r, Err(OpenVaultError::BelowMinDebt { min_e8s: 10_000_000, .. })));
    assert!(s.chain_vaults.is_empty()); // no mutation on rejection
}

#[test]
fn open_rejects_when_over_debt_ceiling() {
    let mut s = setup_registered_chain();
    // seed an existing vault carrying 900 icUSD of pending debt on the chain
    seed_open_vault(&mut s, /*pending_mint_e8s*/ 900 * E8);
    let r = open_chain_vault_in_state(
        &mut s, TEST_CHAIN, principal(), custody(), 100_000 * E18,
        200 * E8, // 200 icUSD -> 1100 total > 1000 ceiling
        recip(), is_valid_evm_address, "MON",
        13_300, 10_000_000, /*debt_ceiling*/ Some(1_000 * E8), 0, 2,
    );
    assert!(matches!(r, Err(OpenVaultError::DebtCeilingExceeded { ceiling_e8s, .. }) if ceiling_e8s == 1_000 * E8));
}

#[test]
fn open_allows_at_ceiling_boundary() {
    let mut s = setup_registered_chain();
    seed_open_vault(&mut s, 800 * E8);
    let r = open_chain_vault_in_state(
        &mut s, TEST_CHAIN, principal(), custody(), 100_000 * E18,
        200 * E8, // exactly 1000 == ceiling -> allowed
        recip(), is_valid_evm_address, "MON",
        13_300, 10_000_000, Some(1_000 * E8), 0, 2,
    );
    assert!(r.is_ok());
}
```

(Define `E8 = 100_000_000u128` and `E18 = 1_000_000_000_000_000_000u128` test consts if not already present; add `seed_open_vault` if no equivalent helper exists.)

- [ ] **Step 4: Run the tests, verify they FAIL to compile** (the new params/variants don't exist yet)

Run: `cargo test -p rumi_protocol_backend --lib chains::tests_vault 2>&1 | tail -20`
Expected: compile error (unexpected args / unknown variant).

- [ ] **Step 5: Extend `open_chain_vault_in_state` signature + checks**

Add two params after `min_cr_e4: u64`:
```rust
    min_cr_e4: u64,
    min_vault_debt_e8s: u128,
    debt_ceiling_e8s: Option<u128>,
```
After the existing `BelowMinCr` check (line ~277), before the `state.chain_vaults.insert`:
```rust
    if debt_e8s < min_vault_debt_e8s {
        return Err(OpenVaultError::BelowMinDebt { debt_e8s, min_e8s: min_vault_debt_e8s });
    }
    if let Some(ceiling) = debt_ceiling_e8s {
        let would_be = total_chain_debt_including_pending_e8s(state, chain).saturating_add(debt_e8s);
        if would_be > ceiling {
            return Err(OpenVaultError::DebtCeilingExceeded { would_be_e8s: would_be, ceiling_e8s: ceiling });
        }
    }
```

- [ ] **Step 6: Run the tests, verify they PASS**

Run: `cargo test -p rumi_protocol_backend --lib chains::tests_vault 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/vault.rs src/rumi_protocol_backend/src/chains/tests_vault.rs
git commit -m "feat(chains): enforce min-debt + debt-ceiling on open path"
```

---

### Task 2: Same enforcement on the borrow path

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs` (`BorrowError` enum, `borrow_chain_vault_in_state` ~line 661-721)
- Test: `src/rumi_protocol_backend/src/chains/tests_vault.rs`

- [ ] **Step 1: Add the two `BorrowError` variants** (mirror Task 1 Step 2: `BelowMinDebt`, `DebtCeilingExceeded`).

- [ ] **Step 2: Write the failing tests** — mirror Task 1 Step 3 but via `borrow_chain_vault_in_state` on an existing Open vault: one asserting a borrow that would leave `new_debt < min_vault_debt_e8s` is rejected (only reachable if min-debt raised above existing debt; otherwise assert the ceiling case), and one asserting a borrow pushing chain total over the ceiling returns `BorrowError::DebtCeilingExceeded`. For the ceiling test, the borrow's contribution to the chain total must use `new_debt` (post-borrow) so the vault's own existing `debt_e8s`/`pending` is not double-counted.

- [ ] **Step 3: Run, verify FAIL.** `cargo test -p rumi_protocol_backend --lib chains::tests_vault 2>&1 | tail -20`

- [ ] **Step 4: Extend `borrow_chain_vault_in_state`** — add `min_vault_debt_e8s: u128, debt_ceiling_e8s: Option<u128>` params after `min_cr_e4`. After the existing `BelowMinCr` check (~line 707):

```rust
    if new_debt < min_vault_debt_e8s {
        return Err(BorrowError::BelowMinDebt { debt_e8s: new_debt, min_e8s: min_vault_debt_e8s });
    }
    if let Some(ceiling) = debt_ceiling_e8s {
        // Replace THIS vault's current contribution with its post-borrow debt to
        // avoid double-counting: total_excluding_this + new_debt.
        let total_excluding_this = total_chain_debt_including_pending_e8s(state, chain)
            .saturating_sub(existing_contribution); // existing_contribution = v.debt_e8s + v.pending_mint_e8s + v.pending_interest_mint_e8s
        let would_be = total_excluding_this.saturating_add(new_debt);
        if would_be > ceiling {
            return Err(BorrowError::DebtCeilingExceeded { would_be_e8s: would_be, ceiling_e8s: ceiling });
        }
    }
```
(Capture `existing_contribution` from the vault read block near line 679 before mutation.)

- [ ] **Step 5: Run, verify PASS.**

- [ ] **Step 6: Commit** — `feat(chains): enforce min-debt + debt-ceiling on borrow path`

---

### Task 3: Wire the config values through the main.rs callers

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (the `open_chain_vault*` / `borrow_chain_vault*` endpoints that call the `*_in_state` fns and currently pass only `cfg.min_cr_e4`)

- [ ] **Step 1: Find every caller.** `grep -rn "open_chain_vault_in_state\|borrow_chain_vault_in_state" src/rumi_protocol_backend/src` — each call site reads the chain's `ChainCollateralConfig`. Pass `cfg.min_vault_debt_e8s` and `cfg.debt_ceiling_e8s` as the two new args. (The EVM self-serve `_evm` variants and any II variants all route through the same `*_in_state` fns — update each.)

- [ ] **Step 2: Build the whole backend.** `cargo build -p rumi_protocol_backend 2>&1 | tail -20` — fix any arity mismatches the compiler flags. Expected: clean build.

- [ ] **Step 3: Run the full backend lib suite.** `cargo test -p rumi_protocol_backend --lib 2>&1 | tail -15` — Expected: all green (no regressions in the existing open/borrow/PocketIC-adjacent unit tests).

- [ ] **Step 4: Commit** — `feat(chains): thread min-debt + ceiling config into open/borrow callers`

---

### Task 4: Set the Conflux config — 150% open gate + depth-bound ceiling

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/collateral_config.rs` (the Conflux/eSpace default, and/or the registration default)
- Test: `src/rumi_protocol_backend/src/chains/tests_config.rs`

- [ ] **Step 1: Update the failing config test.** In `tests_config.rs`, change the Conflux assertion to expect `min_cr_e4 == 15_000` (150%, mirroring ICP per Rob's decision) and a `debt_ceiling_e8s == Some(<cap>)`. Choose the depth-bound cap: the liquidation swap clears only ~$1-3k/swap at <=3% slippage (spec §9), so seed a conservative total ceiling of **500 icUSD** = `Some(500 * E8)` for the gated launch (operator-tunable later). Assert it.

- [ ] **Step 2: Run, verify FAIL.** `cargo test -p rumi_protocol_backend --lib chains::tests_config 2>&1 | tail -10`

- [ ] **Step 3: Update the Conflux config default** — set `min_cr_e4: 15_000` and `debt_ceiling_e8s: Some(500 * 100_000_000)` (500 icUSD) in the Conflux/eSpace `ChainCollateralConfig`. Leave the generic default (`min_cr_e4: 13_300`) for other chains unless they should also move; document the Conflux-specific 150% in a comment referencing the spec decision.

- [ ] **Step 4: Run, verify PASS.** Then run the full lib suite again (`cargo test -p rumi_protocol_backend --lib 2>&1 | tail -15`) to confirm no other test asserted the old 133% Conflux gate.

- [ ] **Step 5: Commit** — `feat(chains): Conflux open gate 150% + 500-icUSD depth-bound ceiling`

---

### Task 5: Verify + PR

- [ ] **Step 1: Full backend lib + the chains PocketIC suites** (the open/borrow flows are exercised end-to-end there):
```bash
cargo test -p rumi_protocol_backend --lib 2>&1 | tail -8
POCKET_IC_BIN=./pocket-ic cargo test -p rumi_protocol_backend --test pocket_ic_tests 2>&1 | tail -12
```
Expected: green. (Rebuild canister wasms first if a rebase happened — PocketIC `include_bytes!`s the prebuilt wasm.)

- [ ] **Step 2: Confirm no candid change.** The `_in_state` signatures changed but the candid-facing endpoints did not (same args in/out). `grep` the .did for the open/borrow methods to confirm no interface drift; if a returned error variant is surfaced in candid, add the two new `OpenVaultError`/`BorrowError` arms to the .did.

- [ ] **Step 3: Open the PR** against `main` summarizing: closes the "debt ceiling not enforced" half of the audit prod blocker; Conflux now 150/133/155 like ICP; no state-shape change (no V6 bump); the depth-bound 500-icUSD gated ceiling. Note this is Increment 0 of the liquidation engine (spec linked); the engine itself follows in increments 1-5.

---

## Self-review

- **Spec coverage:** Implements spec §9 (depth-bound debt cap) + §12 Increment 0 + resolved-fork #1 (150% open gate). Does NOT touch the V6 state, the swap, the SP, or the invariant — those are increments 1-5 by design.
- **Type consistency:** `total_chain_debt_including_pending_e8s` used identically in Task 1 and Task 2; the two new params (`min_vault_debt_e8s: u128`, `debt_ceiling_e8s: Option<u128>`) and the two error variants (`BelowMinDebt`, `DebtCeilingExceeded`) are named identically across open and borrow.
- **No state-shape change:** confirmed — only function signatures + config values + error enums change; no persisted struct gains a field, so no `MultiChainStateV<N>` bump and no upgrade/wipe risk.
- **Boundary:** `open_allows_at_ceiling_boundary` pins `==ceiling` as allowed (only `> ceiling` rejects).
