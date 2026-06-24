# Chains Liquidation Increment 5 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the chains-liquidation reconciliation increment: manually settle pending foreign burns and reserve burns, expose reserve reconciliation, persist chain debt risk overrides, and apply the manual-price staleness gate outside liquidation.

**Architecture:** Keep all supply-affecting state transitions inside `chains/supply.rs` so `chain_supplies`, `pending_chain_burn_e8s`, and `reserve_backing_e8s` move atomically with no mutation on rejection. Add developer-gated operator endpoints in `main.rs` that call the pure helpers, cap proof text, record events, and surface book-vs-on-chain reserve facts. Preserve current behavior by making chain debt config overrides optional and only enforcing price-age checks for chains with a configured non-zero `max_price_age_ns`.

**Tech Stack:** Rust 2021, `ic-cdk` canister updates/queries, Candid, existing EVM RPC wrapper, CBOR/serde stable state snapshots, Cargo unit tests.

## Global Constraints

- Full Inc 5 scope is in: pending-chain-burn reconciliation, `settle_reserve_burn`, bridge/reserve getter, bad-debt/circuit-breaker visibility, Tier-B debt config persistence, and staleness-gate factoring.
- Settlement proof model for this increment is developer-gated manual proof text recorded in logs/events; full on-chain proof verification is a follow-up goal.
- Partial per-chain settlements are allowed, capped by available pending/reserve amount.
- Tier-1 reserve retirement reduces both `chain_supplies` and `reserve_backing_e8s`; `reserve_usdc_native` remains as protocol reserve/surplus bookkeeping.
- Reserve getter returns book values and attempts on-chain USDC `balanceOf` only through the existing low-risk EVM RPC helper.
- Default debt config behavior must match current compile-time `chain_collateral_config` unless an override is explicitly set.
- Stop at code, tests, and commit/PR. Do not merge or deploy without separate explicit authorization.
- TDD is mandatory: write failing tests first, watch them fail, then implement.
- Do not add `@rollup/rollup-darwin-*` to any `package.json`.

---

### Task 1: Atomic Burn/Reserve Settlement Helpers

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/supply.rs`
- Test: `src/rumi_protocol_backend/src/chains/tests_supply.rs`

**Interfaces:**
- Consumes: `MultiChainState`, `ChainId`, existing `chain_backing_rhs_e8s`.
- Produces:
  - `pub enum BackingSettlementError`
  - `pub fn settle_pending_chain_burn(state: &mut MultiChainState, chain: ChainId, amount_e8s: u128) -> Result<(), BackingSettlementError>`
  - `pub fn settle_reserve_burn(state: &mut MultiChainState, chain: ChainId, amount_e8s: u128) -> Result<(), BackingSettlementError>`

- [ ] **Step 1: Write failing pending-burn tests**

Add tests proving a partial pending burn reduces `pending_chain_burn_e8s` and `chain_supplies` by the same amount, preserves the unified invariant, and rejects over-settlement with both fields unchanged.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend pending_chain_burn --lib -- --nocapture`
Expected: FAIL because `settle_pending_chain_burn` does not exist.

- [ ] **Step 3: Implement pending-burn helper**

Implement read-only validation first: halted, unknown chain, zero amount, supply underflow, pending underflow, post-move invariant. Only after validation mutate `chain_supplies` and `pending_chain_burn_e8s`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rumi_protocol_backend pending_chain_burn --lib -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Write failing reserve-burn tests**

Add tests proving reserve retirement reduces `reserve_backing_e8s` and `chain_supplies`, leaves `reserve_usdc_native` untouched, preserves the invariant, and rejects over-settlement with no mutation.

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend reserve_burn --lib -- --nocapture`
Expected: FAIL because `settle_reserve_burn` does not exist.

- [ ] **Step 7: Implement reserve-burn helper**

Use the same shared internal settlement routine with the reserve term selected. Do not touch vault debt or `reserve_usdc_native`.

- [ ] **Step 8: Run task tests**

Run: `cargo test -p rumi_protocol_backend tests_supply --lib -- --nocapture`
Expected: PASS.

### Task 2: Developer-Gated Settlement Endpoints, Events, and Reserve Getter

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/src/event.rs`
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Interfaces:**
- Consumes: Task 1 helpers.
- Produces:
  - `settle_pending_chain_burn(chain, amount_e8s, proof) -> Result<(), ProtocolError>`
  - `settle_reserve_burn(chain, amount_e8s, proof) -> Result<(), ProtocolError>`
  - `get_chain_reserves(chain) -> Result<ChainReserveReport, ProtocolError>`
  - Events `ChainPendingBurnSettled` and `ChainReserveBurnSettled`

- [ ] **Step 1: Write failing endpoint-adjacent tests**

Add unit tests for a pure proof normalizer in `main.rs` or a small helper module: empty proof rejected, proof longer than 512 bytes rejected, valid proof preserved. This bounds persistent event storage.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend reconciliation_proof --bin rumi_protocol_backend -- --nocapture`
Expected: FAIL because the proof helper does not exist.

- [ ] **Step 3: Implement proof helper and endpoints**

Developer-gate both updates. Each endpoint normalizes proof, calls the Task 1 helper inside `mutate_state`, records the matching event, and logs `chain`, `amount_e8s`, and proof text.

- [ ] **Step 4: Add reserve getter**

`get_chain_reserves` returns book values (`recorded_supply_e8s`, `reserve_backing_e8s`, `reserve_usdc_native`, `pending_chain_burn_e8s`, `bad_debt_e8s`) and attempts `erc20_balance_of(chain, settle_stable_token, reserve_address, finalized_block)` when config/token/cursor/address are available. RPC/derive failures populate an `onchain_usdc_error: opt text` field rather than hiding the book values.

- [ ] **Step 5: Wire event no-op replay/filter behavior**

Add the new variants to event filtering and replay as observability-only events.

- [ ] **Step 6: Run task tests**

Run: `cargo test -p rumi_protocol_backend reconciliation_proof --bin rumi_protocol_backend -- --nocapture`
Expected: PASS.

### Task 3: Persisted Chain Debt Config and Price Staleness Gate Factoring

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/collateral_config.rs`
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs`
- Modify: `src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs`
- Modify: `src/rumi_protocol_backend/src/chains/tests_vault.rs`
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Interfaces:**
- Consumes: existing compile-time `ChainCollateralConfig`.
- Produces:
  - `ChainDebtConfigV1 { min_vault_debt_e8s: u128, debt_ceiling_e8s: Option<u128> }`
  - `MultiChainStateV6.chain_debt_configs: BTreeMap<ChainId, ChainDebtConfigV1>`
  - `set_chain_debt_config(chain, config) -> Result<(), ProtocolError>`
  - `get_chain_debt_config(chain) -> Option<ChainDebtConfigV1>`
  - `get_effective_chain_debt_config(chain) -> Option<ChainDebtConfigV1>`

- [ ] **Step 1: Write failing config/default tests**

Tests should prove absent overrides mirror current Conflux defaults exactly, an override changes effective min/ceiling, and V5/V6 snapshot decode defaults the new map empty.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend chain_debt_config --lib -- --nocapture`
Expected: FAIL because `ChainDebtConfigV1` and the state map do not exist.

- [ ] **Step 3: Implement config/state**

Add the new serde-defaulted map, helper conversion from `ChainCollateralConfig`, and developer-gated set/get endpoints. Do not mutate compile-time defaults.

- [ ] **Step 4: Route open/borrow through effective config**

Change `evm_vault_params` to read the override from state, falling back to compile-time defaults. Existing no-override behavior must remain identical.

- [ ] **Step 5: Write failing staleness tests**

Tests should prove open, borrow, and withdraw reject stale/no-timestamp manual prices when `chain_liquidation_configs[chain].max_price_age_ns > 0`, and preserve legacy raw-price behavior when no max age is configured.

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p rumi_protocol_backend stale_price --lib -- --nocapture`
Expected: FAIL because open/borrow/withdraw still read raw `manual_prices`.

- [ ] **Step 7: Implement shared chain price helper**

In `vault.rs`, add a helper that uses `liquidation::fresh_chain_price_e8` when `max_price_age_ns > 0`, otherwise falls back to the existing raw manual price. Add stale/no-timestamp error variants to pure errors as needed.

- [ ] **Step 8: Run task tests**

Run: `cargo test -p rumi_protocol_backend tests_vault chain_debt_config --lib -- --nocapture`
Expected: PASS.

### Task 4: Candid, Compatibility, and Final Verification

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `docs/superpowers/plans/2026-06-24-chains-liquidation-increment-5.md`

**Interfaces:**
- Consumes: Tasks 1-3 public API changes.
- Produces: regenerated Candid and verified compatibility.

- [ ] **Step 1: Regenerate Candid**

Run: `RUMI_REGEN_DID=1 cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture`
Expected: PASS and `rumi_protocol_backend.did` updated.

- [ ] **Step 2: Run targeted tests**

Run:
`cargo test -p rumi_protocol_backend tests_supply tests_vault tests_multi_chain_state_v2 reconciliation_proof --lib --bin rumi_protocol_backend -- --nocapture`

Expected: PASS.

- [ ] **Step 3: Run candid compatibility**

Run: `cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Run formatting/check**

Run: `cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**

Run:
`git add docs/superpowers/plans/2026-06-24-chains-liquidation-increment-5.md src/rumi_protocol_backend/src/chains/supply.rs src/rumi_protocol_backend/src/chains/tests_supply.rs src/rumi_protocol_backend/src/chains/collateral_config.rs src/rumi_protocol_backend/src/chains/multi_chain_state.rs src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs src/rumi_protocol_backend/src/chains/vault.rs src/rumi_protocol_backend/src/chains/tests_vault.rs src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/rumi_protocol_backend.did`

Run:
`git commit -m "feat(chains): Inc5 reconciliation controls"`

Expected: commit succeeds. Stop after commit/PR; do not merge or deploy.

## Future Goal: Full On-Chain Proof Verification

This increment records a manual proof string because it is the lowest-risk operator bridge between IC accounting and foreign-chain burn/bridge facts. The stronger target should be a proof-backed flow:

- Pending-chain burn proof: accept a chain tx hash/log identity, fetch the finalized receipt through the EVM RPC wrapper, verify the `Burn` event amount and expected burn authority/sender, dedupe by `tx_hash:log_index`, then call `settle_pending_chain_burn`.
- Reserve burn proof: verify both sides of the slow bridge leg: finalized foreign `Burn` event reducing icUSD supply and reserve-address `Transfer`/balance movement of settle-stable. Only then call `settle_reserve_burn`.
- Aged pending monitor: expose pending burn age/op ids and alarm when a pending amount is older than the operator SLO.
- Proof storage: store compact canonical proof ids and event metadata, not full receipts, to avoid unbounded stable-state growth.

## Self-Review

**Spec coverage:** Full Inc 5 approved scope maps to Task 1 (pending/reserve settlement), Task 2 (operator proof events and reserve getter), Task 3 (debt config + staleness gate), and Task 4 (Candid/verification). Full on-chain proof is documented as a follow-up goal per user instruction.

**Placeholder scan:** No task uses TBD/TODO/fill-in language. Each task names concrete files, functions, and commands.

**Type consistency:** `amount_e8s`, `ChainId`, and `ChainDebtConfigV1` names are consistent across helper, endpoint, and Candid tasks.
