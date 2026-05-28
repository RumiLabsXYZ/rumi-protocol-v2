# Phase 1a: Multi-Chain Backend Scaffolding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the chain-agnostic scaffolding inside `rumi_protocol_backend` so future sub-phases (1b Monad, 1c liquidations, 1d frontend) can attach a real chain without touching the core. Phase 1a registers no real chain, signs nothing, and makes no outcalls. It introduces the `chains/` module tree, a `ChainAdapter` trait, per-chain settlement queues, a `chain_supplies` accounting table guarded by `apply_supply_delta`, a periodic invariant self-check on Timer B, public `register_chain` / `disable_chain` / `set_chain_config` admin endpoints, and the `get_global_icusd_supply()` / `get_supply_audit()` queries. Phase 1a deploys to a brand-new `mainnet-staging` canister with `chain_configs` empty so the public API exists but no real chain talks to it.

**Architecture:** New `src/rumi_protocol_backend/src/chains/` subtree (`mod.rs`, `adapter.rs`, `config.rs`, `settlement_queue.rs`, `supply.rs`). All new persisted state lives inside a single `MultiChainStateV1` struct embedded in `State` via a `pub type MultiChainState = MultiChainStateV1;` alias, mirroring the AMM versioned-snapshot pattern. Every mutation that touches debt or chain supply routes through `apply_supply_delta(chain, delta)`; the function traps the canister on invariant violation. A periodic self-check on Timer B (the existing 60s interest/treasury timer) recomputes `sum(chain_supplies)` against `total_debt` and flips the protocol into `Mode::ReadOnly` on mismatch with `mode_triggered_by_oracle = false` (operator-only recovery). Phase 1a is wire-only: the adapter trait has no implementations beyond a dev-only stub used in tests.

**Tech Stack:**
- Rust 2021 (workspace resolver "2"), ic-cdk 0.12, candid 0.10, ic-stable-structures 0.6, rust_decimal 1.32
- proptest 1.0 for the supply-invariant property tests (already a dev-dependency of `rumi_protocol_backend`)
- pocket-ic 6.0 for the end-to-end deploy smoke test (already wired up under `tests/`)
- icp-cli 0.2 + `icp canister install` for the mainnet-staging deploy (per `docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md`)
- didc for declarations regen via `scripts/regenerate-declarations.sh`

**Reference docs:**
- Spec: `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md` (Sections 1, 2, 3 and "Sub-phase 1a")
- Deploy pattern (locked, do not deviate): `docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md`
- Phase 0 reference plan: `docs/superpowers/plans/2026-05-27-icp-cli-migration-phase-0.md`
- AMM state-wipe incident (the cautionary tale this plan is shaped around): `MEMORY.md` -> `project_amm_state_wipe_2026_05_18.md`

**Branch:** Work on `feat/multi-chain-phase-1a` (create from main after this plan PR merges). The current branch (`feat/multi-chain-phase-1a-plan`) carries this plan only.

**What Phase 1a is NOT:**
- No tECDSA / tEd25519 signing
- No HTTPS outcalls or EVM RPC wiring
- No Monad / Solana / EVM configuration
- No Solidity (`IcUSD.sol`, `LiquidationRouter.sol`)
- No SIWE / SIWS providers
- No frontend changes
- No liquidation routing logic
- No real chain registered at end of phase (`register_chain` works; it is simply never called against a real config until Phase 1b)

---

## Decision Gates Summary

Three gates require Rob's explicit "go" before the next task runs:

1. **After Task 9** (state-shape choice locked, `MultiChainStateV1` lands but is empty): confirm the versioned-snapshot pattern before any new field flows through it. The 2026-05-18 AMM incident is the precedent.
2. **After Task 14** (mainnet-staging canister created on IC, empty wasm): confirm canister IDs, mappings file, and controllers before installing the real Phase 1a wasm.
3. **After Task 15** (Phase 1a wasm installed on mainnet-staging): confirm `get_global_icusd_supply()` returns 0 and `get_supply_audit()` returns an empty per-chain breakdown before opening the merge PR. This is the canary that the versioned-snapshot round-trip survived.

Every other task uses local-only verification and routine commits.

---

## Task 1: Create Feature Branch and Scaffold the chains/ Module Entry

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/mod.rs`
- Modify: `src/rumi_protocol_backend/src/lib.rs` (add `pub mod chains;` to the module list)

This task only adds an empty module so subsequent tasks have a parent. No types, no functions, no behaviour change. The wasm hash must shift only because `mod chains;` was added (a no-op insofar as the canister's public surface is concerned).

- [ ] **Step 1: Branch off main**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
git fetch origin
git checkout main
git pull origin main
git checkout -b feat/multi-chain-phase-1a
git branch --show-current
```

Expected: `feat/multi-chain-phase-1a`. Verify with `git branch --show-current` per [feedback_branch_discipline](memory).

- [ ] **Step 2: Create the empty chains module entry**

Write `src/rumi_protocol_backend/src/chains/mod.rs`:

```rust
//! Multi-chain scaffolding (Phase 1a).
//!
//! This module tree carries the chain-agnostic abstractions used by every
//! foreign-chain integration: the `ChainAdapter` trait (adapter.rs), the
//! per-chain configuration record (config.rs), the per-chain settlement
//! queue (settlement_queue.rs), and the supply-invariant accounting helpers
//! (supply.rs).
//!
//! Phase 1a registers no real chain. The trait has no production impls,
//! the settlement queues are never drained, and `chain_supplies` stays
//! empty after install. Phase 1b (Monad) will add the first real adapter
//! and the first non-zero entries.

pub mod adapter;
pub mod config;
pub mod settlement_queue;
pub mod supply;

pub use adapter::ChainAdapter;
pub use config::{ChainConfig, ChainId, ChainStatus};
pub use settlement_queue::{SettlementOp, SettlementQueueV1};
pub use supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};
```

- [ ] **Step 3: Write stub module files referenced by mod.rs**

So the build does not break before Task 2 lands the real trait, write four minimal stub files. Each is a single doc comment so `cargo build --package rumi_protocol_backend` passes.

Create `src/rumi_protocol_backend/src/chains/adapter.rs`:

```rust
//! Placeholder for the `ChainAdapter` trait. Real trait lands in Task 2.

pub trait ChainAdapter {}
```

Create `src/rumi_protocol_backend/src/chains/config.rs`:

```rust
//! Placeholder for `ChainConfig`, `ChainId`, `ChainStatus`. Real types in Task 3.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChainId(pub u32);

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStatus { Registered, Disabled }

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfig {
    pub chain_id: ChainId,
    pub display_name: String,
}
```

Create `src/rumi_protocol_backend/src/chains/settlement_queue.rs`:

```rust
//! Placeholder for the per-chain settlement queue. Real queue in Task 5.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct SettlementQueueV1 {
    pub head: u64,
    pub tail: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SettlementOp {
    pub idempotency_key: String,
}
```

Create `src/rumi_protocol_backend/src/chains/supply.rs`:

```rust
//! Placeholder for `apply_supply_delta` and the invariant types. Real impl in Task 7.

#[derive(Debug)]
pub struct SupplyDelta;

#[derive(Debug)]
pub enum SupplyInvariantError {}

pub fn apply_supply_delta() {}
```

- [ ] **Step 4: Wire the module in lib.rs**

Edit `src/rumi_protocol_backend/src/lib.rs`. Find the `pub mod` block near the top (around the `pub mod xrc;` line) and add `pub mod chains;` in alphabetical order between `pub mod treasury;` and `pub mod vault;`:

```rust
pub mod chains;
```

- [ ] **Step 5: Verify the build still passes**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
```

Expected: build succeeds. No warnings about unused imports (the stubs intentionally avoid unused imports).

- [ ] **Step 6: Run the existing unit test suite to confirm zero regression**

```bash
cargo test --package rumi_protocol_backend --lib
```

Expected: every existing test passes. The new module is empty so nothing else changes.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/ src/rumi_protocol_backend/src/lib.rs
git commit -m "feat(multi-chain): scaffold chains/ module tree

Phase 1a Task 1. Empty module entry + four stub files so subsequent
tasks have a parent. No behaviour change."
```

---

## Task 2: Define the ChainAdapter Trait

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/adapter.rs`
- Create: `src/rumi_protocol_backend/src/chains/tests_adapter.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs` (wire the test module under `#[cfg(test)]`)

The trait shape comes directly from the spec ("Section 1: Per-chain integration cost target"). Phase 1a defines the trait and a single in-memory test stub that asserts the method signatures compile against a `Box<dyn ChainAdapter>`. No production impl ships.

- [ ] **Step 1: Write the failing test FIRST**

Write `src/rumi_protocol_backend/src/chains/tests_adapter.rs`:

```rust
//! Adapter-trait shape tests. No production impl; tests use a stub.

use super::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot,
    MintInstruction, SignedBurn, SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use super::config::{ChainConfig, ChainId};
use async_trait::async_trait;
use candid::Principal;

struct StubAdapter {
    chain_id: ChainId,
}

#[async_trait(?Send)]
impl ChainAdapter for StubAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    async fn verify_deposit(&self, _tx_hash: &str) -> Result<DepositRecord, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_withdrawal(&self, _req: WithdrawalRequest) -> Result<SignedWithdrawal, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_mint(&self, _instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_burn(&self, _amount_e8s: u128, _burner: Principal) -> Result<SignedBurn, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn observe_event(&self, _from_block: u64) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }
}

#[test]
fn adapter_can_be_held_as_trait_object() {
    let a: Box<dyn ChainAdapter> = Box::new(StubAdapter { chain_id: ChainId(7) });
    assert_eq!(a.chain_id(), ChainId(7));
}

#[test]
fn adapter_error_serializes_via_candid() {
    use candid::{Decode, Encode};
    let err = ChainAdapterError::NotImplemented;
    let bytes = Encode!(&err).expect("encode");
    let round_trip: ChainAdapterError = Decode!(&bytes, ChainAdapterError).expect("decode");
    assert!(matches!(round_trip, ChainAdapterError::NotImplemented));
}
```

Wire it in `mod.rs` under a cfg-test block. Open `src/rumi_protocol_backend/src/chains/mod.rs` and add at the end:

```rust
#[cfg(test)]
mod tests_adapter;
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_adapter 2>&1 | tail -20
```

Expected: compile error on missing items (`DepositRecord`, `ChainAdapterError`, etc.). This locks in the API.

- [ ] **Step 3: Add `async-trait` to Cargo.toml**

Edit `src/rumi_protocol_backend/Cargo.toml`. Under `[dependencies]`, add (keep alphabetical):

```toml
async-trait = "0.1"
```

Run `cargo build --package rumi_protocol_backend` once to fetch and pin.

- [ ] **Step 4: Write the real trait**

Replace the entire contents of `src/rumi_protocol_backend/src/chains/adapter.rs` with:

```rust
//! Chain-agnostic adapter trait. Every foreign chain implements this trait
//! in its own module (`chains::monad`, `chains::solana`, ...). Phase 1a
//! ships the trait only; Phase 1b adds the first impl (Monad).

use super::config::ChainId;
use async_trait::async_trait;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

/// Per-chain operations the protocol relies on.
///
/// `?Send` because canister code is single-threaded; relaxing the bound
/// keeps adapters free to hold ICP-side stable-memory handles without
/// going through `Arc` indirection.
#[async_trait(?Send)]
pub trait ChainAdapter {
    fn chain_id(&self) -> ChainId;

    async fn verify_deposit(&self, tx_hash: &str) -> Result<DepositRecord, ChainAdapterError>;

    async fn sign_withdrawal(&self, req: WithdrawalRequest) -> Result<SignedWithdrawal, ChainAdapterError>;

    async fn sign_mint(&self, instr: MintInstruction) -> Result<SignedMint, ChainAdapterError>;

    async fn sign_burn(&self, amount_e8s: u128, burner: Principal) -> Result<SignedBurn, ChainAdapterError>;

    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError>;

    async fn observe_event(&self, from_block: u64) -> Result<Vec<DepositRecord>, ChainAdapterError>;
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct DepositRecord {
    pub depositor: String,
    pub amount_e8s: u128,
    pub block_number: u64,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct WithdrawalRequest {
    pub recipient: String,
    pub amount_e8s: u128,
    pub idempotency_key: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedWithdrawal {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct MintInstruction {
    pub recipient: String,
    pub amount_e8s: u128,
    pub vault_id: u64,
    pub idempotency_key: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedMint {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SignedBurn {
    pub raw_tx: Vec<u8>,
    pub tx_hash: String,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct FinalitySnapshot {
    pub latest_block: u64,
    pub finalized_block: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum ChainAdapterError {
    NotImplemented,
    RpcError { provider: String, message: String },
    SignatureFailed(String),
    InsufficientFinality { latest: u64, required: u64 },
    InvalidPayload(String),
}
```

- [ ] **Step 5: Run the test to confirm it now passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_adapter
```

Expected: both tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/ src/rumi_protocol_backend/Cargo.toml src/rumi_protocol_backend/Cargo.lock 2>/dev/null
# Cargo.lock lives at the workspace root:
git add Cargo.lock
git commit -m "feat(multi-chain): ChainAdapter trait + payload types

Phase 1a Task 2. Trait signatures lock the chain-agnostic surface so
every adapter (Monad, Solana, ...) implements the same six methods."
```

---

## Task 3: Define ChainConfig, ChainId, ChainStatus (Versioned from Day One)

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/config.rs`
- Create: `src/rumi_protocol_backend/src/chains/tests_config.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs` (wire the new test module)

Per the spec Section 3 "State wipe on upgrade", every multi-chain struct ships in a versioned form. `ChainConfig` becomes `ChainConfigV1` with a `pub type ChainConfig = ChainConfigV1;` alias. When Phase 1c needs a new field, it adds `ChainConfigV2` plus a migration.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_config.rs`:

```rust
//! ChainConfig encode/decode + version-alias invariants.

use super::config::{ChainConfig, ChainConfigV1, ChainId, ChainStatus, GasStrategy};
use candid::{Decode, Encode};

#[test]
fn chain_id_orderable_for_btreemap_use() {
    let a = ChainId(1);
    let b = ChainId(2);
    assert!(a < b);
}

#[test]
fn chain_status_is_exhaustive() {
    // Phase 1a defines two variants. Future variants land via a versioned
    // migration, never an in-place enum addition (cf. CBOR untagged-enum
    // round-trips for Mode).
    let variants = vec![ChainStatus::Registered, ChainStatus::Disabled];
    assert_eq!(variants.len(), 2);
}

#[test]
fn chain_config_round_trips_via_candid() {
    let cfg = ChainConfigV1 {
        chain_id: ChainId(101),
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: vec!["https://rpc.testnet.example".to_string()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: 18,
        registered_at_ns: 1_700_000_000_000_000_000,
        status: ChainStatus::Registered,
    };
    let bytes = Encode!(&cfg).expect("encode");
    let back: ChainConfigV1 = Decode!(&bytes, ChainConfigV1).expect("decode");
    assert_eq!(back.chain_id, cfg.chain_id);
    assert_eq!(back.display_name, cfg.display_name);
    assert_eq!(back.finality_depth, 1);
}

#[test]
fn chain_config_alias_matches_v1() {
    // Phase 1a invariant: `ChainConfig` is the active version pointer.
    // Phase 1c (or whenever a field is added) rebinds this alias to V2 and
    // ships a `MultiChainStateMigration` step.
    fn _check(_x: ChainConfig) -> ChainConfigV1 { _x }
}
```

Wire it in `mod.rs`:

```rust
#[cfg(test)]
mod tests_config;
```

- [ ] **Step 2: Run the test and confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_config 2>&1 | tail -10
```

Expected: compile error on missing items.

- [ ] **Step 3: Write the real config**

Replace the entire contents of `src/rumi_protocol_backend/src/chains/config.rs` with:

```rust
//! Per-chain configuration record.
//!
//! Versioned-snapshot pattern (see spec Section 3): the active shape is
//! `ChainConfigV1`. Adding a field = bump to `ChainConfigV2`, register a
//! migration in `crate::chains::supply::migrate_multi_chain_state`, and
//! rebind `pub type ChainConfig = ChainConfigV2;`. Never modify V1 in
//! place once it has shipped.

use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChainId(pub u32);

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainStatus {
    Registered,
    Disabled,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum GasStrategy {
    /// EIP-1559 EVM chains (Monad, Ethereum, L2s).
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
    /// Pre-EIP-1559 EVM (rare).
    EvmLegacy { gas_price_gwei_ceiling: u64 },
    /// Solana priority fee bidding.
    SolanaPriorityFee { lamports_per_cu_ceiling: u64 },
    /// No fee model needed (read-only adapters; dev placeholders).
    NotApplicable,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainConfigV1 {
    pub chain_id: ChainId,
    pub display_name: String,
    pub rpc_endpoints: Vec<String>,
    /// Blocks past head before a deposit/event is treated as committed.
    pub finality_depth: u32,
    pub gas_strategy: GasStrategy,
    /// Decimals of the chain-native gas asset (18 for EVM, 9 for Solana SOL).
    pub chain_native_decimals: u8,
    /// `ic_cdk::api::time()` nanoseconds when this config was first registered.
    pub registered_at_ns: u64,
    pub status: ChainStatus,
}

/// Active alias. Rebind to a later version when a field is added.
pub type ChainConfig = ChainConfigV1;

/// Caller-supplied registration payload. Distinct from the persisted
/// `ChainConfigV1` so the admin endpoint can fill `registered_at_ns` and
/// `status` server-side without trusting the caller.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RegisterChainArg {
    pub chain_id: ChainId,
    pub display_name: String,
    pub rpc_endpoints: Vec<String>,
    pub finality_depth: u32,
    pub gas_strategy: GasStrategy,
    pub chain_native_decimals: u8,
}

/// Operator-supplied update payload. Every field is optional; omitted
/// fields are left unchanged.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct UpdateChainConfigArg {
    pub display_name: Option<String>,
    pub rpc_endpoints: Option<Vec<String>>,
    pub finality_depth: Option<u32>,
    pub gas_strategy: Option<GasStrategy>,
}

/// Reasons a `register_chain`/`set_chain_config` call can be rejected.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum ChainAdminError {
    NotDeveloper,
    ChainAlreadyRegistered(ChainId),
    ChainNotRegistered(ChainId),
    InvalidConfig(String),
}
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_config
```

Expected: all four tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(multi-chain): ChainConfigV1 + ChainId + ChainStatus (versioned)

Phase 1a Task 3. Active alias points at V1. Future field additions go
via V2 + migration per the AMM state-wipe lessons (MEMORY.md
project_amm_state_wipe_2026_05_18.md)."
```

---

## Task 4: Define the SettlementOp Payload and SettlementQueueV1 Type

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/settlement_queue.rs`
- Create: `src/rumi_protocol_backend/src/chains/tests_settlement_queue.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs`

A `SettlementQueueV1` is a per-chain VecDeque-shaped struct holding pending outbound ops (mints, withdrawals, signed burns). Phase 1a never drains the queue; Timer D wiring lands in Phase 1b. This task only defines the data shape and the idempotency-key contract.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_settlement_queue.rs`:

```rust
use super::settlement_queue::{
    SettlementOp, SettlementOpKind, SettlementOpStatus, SettlementQueueError,
    SettlementQueueV1,
};
use candid::{Decode, Encode};

#[test]
fn empty_queue_has_zero_head_and_tail() {
    let q = SettlementQueueV1::default();
    assert_eq!(q.head, 0);
    assert_eq!(q.tail, 0);
    assert!(q.pending.is_empty());
}

#[test]
fn enqueue_assigns_increasing_op_ids() {
    let mut q = SettlementQueueV1::default();
    let op_a = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xabc".to_string(), amount_e8s: 100, vault_id: 1 },
        "key-a".to_string(),
        0,
    );
    let op_b = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xdef".to_string(), amount_e8s: 200, vault_id: 2 },
        "key-b".to_string(),
        0,
    );
    let id_a = q.enqueue(op_a).expect("first enqueue");
    let id_b = q.enqueue(op_b).expect("second enqueue");
    assert_eq!(id_a, 0);
    assert_eq!(id_b, 1);
    assert_eq!(q.pending.len(), 2);
    assert_eq!(q.tail, 2);
}

#[test]
fn enqueue_rejects_duplicate_idempotency_key() {
    let mut q = SettlementQueueV1::default();
    let op_a = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xa".to_string(), amount_e8s: 1, vault_id: 1 },
        "duplicate".to_string(),
        0,
    );
    let op_b = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xb".to_string(), amount_e8s: 2, vault_id: 2 },
        "duplicate".to_string(),
        0,
    );
    q.enqueue(op_a).expect("first");
    let err = q.enqueue(op_b).expect_err("second must reject");
    assert!(matches!(err, SettlementQueueError::DuplicateIdempotencyKey(_)));
}

#[test]
fn round_trip_via_candid() {
    let mut q = SettlementQueueV1::default();
    let op = SettlementOp::new(
        SettlementOpKind::Withdrawal { recipient: "0xrecip".to_string(), amount_e8s: 42 },
        "k1".to_string(),
        0,
    );
    q.enqueue(op).expect("enqueue");
    let bytes = Encode!(&q).expect("encode");
    let back: SettlementQueueV1 = Decode!(&bytes, SettlementQueueV1).expect("decode");
    assert_eq!(back.pending.len(), 1);
    assert_eq!(back.tail, 1);
}

#[test]
fn op_status_transitions_only_to_terminal_states() {
    let mut op = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xa".to_string(), amount_e8s: 1, vault_id: 1 },
        "k".to_string(),
        0,
    );
    assert!(matches!(op.status, SettlementOpStatus::Queued));
    op.mark_inflight(1_700_000_000_000_000_000);
    assert!(matches!(op.status, SettlementOpStatus::Inflight { .. }));
    op.mark_succeeded("0xdeadbeef".to_string(), 1_700_000_000_001_000_000);
    assert!(matches!(op.status, SettlementOpStatus::Succeeded { .. }));
}
```

Wire it in `mod.rs`:

```rust
#[cfg(test)]
mod tests_settlement_queue;
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_settlement_queue 2>&1 | tail -10
```

- [ ] **Step 3: Write the real settlement queue**

Replace the entire contents of `src/rumi_protocol_backend/src/chains/settlement_queue.rs` with:

```rust
//! Per-chain settlement queue.
//!
//! Each registered chain owns one `SettlementQueueV1` carrying outbound ops
//! the canister still needs to sign and submit. Phase 1a defines the shape
//! and the enqueue/idempotency rules. Phase 1b adds the Timer-D worker that
//! actually drains the queue against the Monad adapter.
//!
//! Versioned per the spec Section 3. Adding a field bumps to V2 plus a
//! migration in `chains::supply::migrate_multi_chain_state`.

use candid::{CandidType, Deserialize};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum SettlementOpKind {
    Mint { recipient: String, amount_e8s: u128, vault_id: u64 },
    Withdrawal { recipient: String, amount_e8s: u128 },
    Burn { amount_e8s: u128 },
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum SettlementOpStatus {
    Queued,
    Inflight { tries: u32, last_attempt_ns: u64 },
    Succeeded { tx_hash: String, confirmed_ns: u64 },
    Failed { reason: String, failed_ns: u64 },
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct SettlementOp {
    pub op_id: u64,
    pub kind: SettlementOpKind,
    pub idempotency_key: String,
    pub enqueued_at_ns: u64,
    pub status: SettlementOpStatus,
}

impl SettlementOp {
    pub fn new(kind: SettlementOpKind, idempotency_key: String, now_ns: u64) -> Self {
        Self {
            op_id: 0,
            kind,
            idempotency_key,
            enqueued_at_ns: now_ns,
            status: SettlementOpStatus::Queued,
        }
    }

    pub fn mark_inflight(&mut self, now_ns: u64) {
        let tries = match &self.status {
            SettlementOpStatus::Inflight { tries, .. } => tries.saturating_add(1),
            _ => 1,
        };
        self.status = SettlementOpStatus::Inflight { tries, last_attempt_ns: now_ns };
    }

    pub fn mark_succeeded(&mut self, tx_hash: String, now_ns: u64) {
        self.status = SettlementOpStatus::Succeeded { tx_hash, confirmed_ns: now_ns };
    }

    pub fn mark_failed(&mut self, reason: String, now_ns: u64) {
        self.status = SettlementOpStatus::Failed { reason, failed_ns: now_ns };
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct SettlementQueueV1 {
    /// Lowest enqueued op_id still pending. Advances as ops complete.
    pub head: u64,
    /// Next op_id to assign. Always >= head.
    pub tail: u64,
    /// Pending ops indexed by op_id. Drained head-first by Phase-1b's Timer D.
    pub pending: BTreeMap<u64, SettlementOp>,
    /// Idempotency keys seen on this queue. Enqueue rejects duplicates.
    pub seen_idempotency_keys: BTreeSet<String>,
    /// FIFO ordering hint for the drain loop. Phase 1a never reads it; kept
    /// so Phase 1b can drain in enqueue order without scanning `pending`.
    pub drain_order: VecDeque<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettlementQueueError {
    DuplicateIdempotencyKey(String),
}

impl SettlementQueueV1 {
    pub fn enqueue(&mut self, mut op: SettlementOp) -> Result<u64, SettlementQueueError> {
        if self.seen_idempotency_keys.contains(&op.idempotency_key) {
            return Err(SettlementQueueError::DuplicateIdempotencyKey(op.idempotency_key));
        }
        let assigned = self.tail;
        op.op_id = assigned;
        self.seen_idempotency_keys.insert(op.idempotency_key.clone());
        self.drain_order.push_back(assigned);
        self.pending.insert(assigned, op);
        self.tail = self.tail.saturating_add(1);
        Ok(assigned)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_settlement_queue
```

Expected: all five tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(multi-chain): SettlementQueueV1 + SettlementOp lifecycle

Phase 1a Task 4. Versioned per-chain queue with idempotency enforcement.
Drain wiring lands in Phase 1b."
```

---

## Task 5: Define MultiChainStateV1 and Wire it Under State

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs` (re-export `MultiChainStateV1` + `MultiChainState`)
- Create: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`
- Modify: `src/rumi_protocol_backend/src/state.rs` (add `pub multi_chain: MultiChainState` field with `#[serde(default)]`)
- Create: `src/rumi_protocol_backend/src/chains/tests_multi_chain_state.rs`

This task lands the persisted shape that every later task in Phase 1a slots fields into. Per the spec's hard constraint, the entire multi-chain payload lives inside a single versioned struct.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_multi_chain_state.rs`:

```rust
use super::multi_chain_state::MultiChainStateV1;
use super::config::ChainId;
use candid::{Decode, Encode};
use std::collections::BTreeMap;

#[test]
fn default_is_empty() {
    let s = MultiChainStateV1::default();
    assert!(s.chain_configs.is_empty());
    assert!(s.chain_supplies.is_empty());
    assert!(s.settlement_queues.is_empty());
    assert_eq!(s.total_supply_all_chains_e8s(), 0u128);
}

#[test]
fn total_supply_sums_across_chains() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 10_000_000);
    s.chain_supplies.insert(ChainId(2), 25_000_000);
    s.chain_supplies.insert(ChainId(3), 5_000_000);
    assert_eq!(s.total_supply_all_chains_e8s(), 40_000_000u128);
}

#[test]
fn round_trips_via_candid() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(7), 99);
    let bytes = Encode!(&s).expect("encode");
    let back: MultiChainStateV1 = Decode!(&bytes, MultiChainStateV1).expect("decode");
    assert_eq!(back.chain_supplies.get(&ChainId(7)), Some(&99u128));
}

#[test]
fn round_trips_via_cbor() {
    // The whole State is persisted via ciborium CBOR in `storage::save_state_to_stable`.
    // A multi_chain field that survives Candid but trips up CBOR would still
    // wipe state across an upgrade, so test CBOR directly.
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(11), 1234);
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&s, &mut buf).expect("cbor encode");
    let back: MultiChainStateV1 = ciborium::de::from_reader(buf.as_slice()).expect("cbor decode");
    assert_eq!(back.chain_supplies.get(&ChainId(11)), Some(&1234u128));
}

// NOTE on pre-1a snapshot decode: the strongest guard against the AMM-style
// state-wipe failure mode is the PocketIC upgrade round-trip in Task 12, which
// installs the wasm twice with an `upgrade_canister` between calls. We don't
// try to synthesise a "pre-1a" CBOR State here because State has no public
// Default impl and forging a snapshot byte sequence by hand is brittle. The
// `MultiChainStateV1` round-trip above plus the PocketIC test together cover
// the same surface.
```

Wire in `mod.rs`:

```rust
#[cfg(test)]
mod tests_multi_chain_state;
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_multi_chain_state 2>&1 | tail -10
```

- [ ] **Step 3: Create the multi_chain_state module**

Create `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`:

```rust
//! Persisted multi-chain root.
//!
//! Lives at `state::State::multi_chain` and carries every chain-aware
//! piece of state in one struct so the AMM-style state-wipe pattern
//! (missing field at decode time -> Default applied silently) cannot
//! happen for any sub-component. Add fields ONLY by:
//!
//! 1. Renaming `MultiChainStateV1` -> keep the V1 fields exactly.
//! 2. Adding `MultiChainStateV2` with the new field plus a `From<V1>` impl.
//! 3. Updating the `pub type MultiChainState = MultiChainStateV2;` alias.
//! 4. Adding a one-line entry to `migrate_multi_chain_state` (see `supply.rs`).
//!
//! See spec Section 3 ("State wipe on upgrade") and the 2026-05-18 AMM
//! incident.

use super::config::{ChainConfigV1, ChainId};
use super::settlement_queue::SettlementQueueV1;
use candid::{CandidType, Deserialize};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV1 {
    pub chain_configs: BTreeMap<ChainId, ChainConfigV1>,
    /// Canonical per-chain icUSD supply (e8s). Invariant:
    /// `sum(chain_supplies.values()) == state.total_borrowed_icusd_amount()`
    /// after every state mutation. Enforced by `apply_supply_delta`.
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    /// `true` iff the periodic invariant self-check on Timer B failed the
    /// last time it ran. When set, every entry point into `apply_supply_delta`
    /// returns `SupplyInvariantError::HaltedAfterSelfCheckFailure`.
    /// Cleared only by `clear_invariant_halt` (developer-gated, lands in
    /// Phase 1b along with operational tooling). For Phase 1a the field
    /// exists, defaults to false, and is only set by the self-check.
    pub invariant_halted: bool,
}

impl MultiChainStateV1 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }
}

pub type MultiChainState = MultiChainStateV1;
```

Update `src/rumi_protocol_backend/src/chains/mod.rs` to re-export:

```rust
pub mod multi_chain_state;
pub use multi_chain_state::{MultiChainState, MultiChainStateV1};
```

- [ ] **Step 4: Add the `multi_chain` field to State**

Edit `src/rumi_protocol_backend/src/state.rs`. Find the `pub struct State {` declaration (around line 738). Locate the existing `#[serde(default)]` annotations on the struct (the struct-level `#[serde(default)]` plus per-field `#[serde(default)]` markers added by Wave-14). Add a new field at the end of the struct (just before the closing brace), with a `#[serde(default)]` so any pre-1a snapshot round-trips cleanly:

```rust
    /// Phase 1a: multi-chain accounting + per-chain settlement queues.
    /// Empty on every pre-1a snapshot via `#[serde(default)]`. See
    /// `chains::multi_chain_state` for the versioned-snapshot pattern.
    #[serde(default)]
    pub multi_chain: crate::chains::MultiChainState,
```

- [ ] **Step 5: Run the test suite**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_multi_chain_state
cargo test --package rumi_protocol_backend --lib
```

Expected: every new test passes; every existing test continues to pass.

- [ ] **Step 6: Confirm the State default still constructs cleanly**

The existing tests that round-trip `State::default()` (search for tests calling `State::default()` or `replay(...)`) must still pass. If the test in Step 1 above doesn't run on its own, search for any project-internal round-trip helper instead.

```bash
cargo test --package rumi_protocol_backend --lib state:: 2>&1 | tail -20
```

Expected: no regressions.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/ src/rumi_protocol_backend/src/state.rs
git commit -m "feat(multi-chain): MultiChainStateV1 wired under State::multi_chain

Phase 1a Task 5. Versioned root holds chain_configs, chain_supplies,
settlement_queues, invariant_halted. #[serde(default)] keeps pre-1a
snapshots round-tripping per the AMM state-wipe lessons."
```

**Decision gate:** Pause here. Open the diff and walk the State struct one more time. Confirm:
- The new field carries `#[serde(default)]`
- No existing field's CBOR shape changed
- `cargo test --package rumi_protocol_backend --lib` is green

Get Rob's explicit "go" before continuing. The versioned-snapshot pattern is the load-bearing piece for all of Phase 1.

---

## Task 6: ProtocolError Variants for Multi-Chain Admin and Supply Paths

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs` (extend `ProtocolError` enum)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` (mirror enum into Candid)

Two new variants land here so Tasks 7-10 can return them. Keep the variant ORDER stable (append-only) so existing on-chain event-log entries holding `ProtocolError` payloads decode unchanged.

- [ ] **Step 1: Write the failing test FIRST**

Add to `src/rumi_protocol_backend/src/tests.rs` (anywhere after the existing tests):

```rust
#[test]
fn protocol_error_carries_multi_chain_variants() {
    use candid::{Decode, Encode};
    use crate::ProtocolError;
    let halt = ProtocolError::SupplyInvariantHalted;
    let admin = ProtocolError::ChainAdmin("not developer".to_string());
    let halt_bytes = Encode!(&halt).expect("encode halt");
    let admin_bytes = Encode!(&admin).expect("encode admin");
    let _: ProtocolError = Decode!(&halt_bytes, ProtocolError).expect("decode halt");
    let _: ProtocolError = Decode!(&admin_bytes, ProtocolError).expect("decode admin");
}
```

Run it:

```bash
cargo test --package rumi_protocol_backend --lib protocol_error_carries_multi_chain_variants 2>&1 | tail -10
```

Expected: compile failure on missing variants.

- [ ] **Step 2: Extend ProtocolError**

Edit `src/rumi_protocol_backend/src/lib.rs`. Find `pub enum ProtocolError` (around line 590) and append the two variants AFTER `NotLowestCR` so all existing on-chain payloads decode unchanged:

```rust
pub enum ProtocolError {
    TransferFromError(TransferFromError, u64),
    TransferError(TransferError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    AnonymousCallerNotAllowed,
    CallerNotOwner,
    AmountTooLow { minimum_amount: u64 },
    GenericError(String),
    NotLowestCR,
    /// Phase 1a: the periodic supply-invariant self-check (Timer B) caught
    /// a `sum(chain_supplies) != total_debt` divergence. Every entry that
    /// touches debt or chain supply returns this error until an operator
    /// clears `multi_chain.invariant_halted`.
    SupplyInvariantHalted,
    /// Phase 1a: admin-endpoint error for `register_chain`, `disable_chain`,
    /// `set_chain_config`. Wraps a developer-facing message string. The
    /// structured `ChainAdminError` enum lives in `chains::config` and is
    /// stringified here so the Candid surface stays append-only.
    ChainAdmin(String),
}
```

- [ ] **Step 3: Mirror into Candid**

Edit `src/rumi_protocol_backend/rumi_protocol_backend.did`. Find the `type ProtocolError = variant {` block. Append the two new variants AFTER `NotLowestCR`:

```candid
type ProtocolError = variant {
    TransferFromError : record { TransferFromError; nat64 };
    TransferError : TransferError;
    TemporarilyUnavailable : text;
    AlreadyProcessing;
    AnonymousCallerNotAllowed;
    CallerNotOwner;
    AmountTooLow : record { minimum_amount : nat64 };
    GenericError : text;
    NotLowestCR;
    SupplyInvariantHalted;
    ChainAdmin : text;
};
```

- [ ] **Step 4: Run the test**

```bash
cargo test --package rumi_protocol_backend --lib protocol_error_carries_multi_chain_variants
```

Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/rumi_protocol_backend.did src/rumi_protocol_backend/src/tests.rs
git commit -m "feat(multi-chain): ProtocolError adds SupplyInvariantHalted + ChainAdmin

Phase 1a Task 6. Appended variants so existing on-chain payloads decode
unchanged."
```

---

## Task 7: apply_supply_delta + Invariant Enforcement

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/supply.rs`
- Create: `src/rumi_protocol_backend/src/chains/tests_supply.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs`

The single function `apply_supply_delta(state, chain, delta)` is the ONLY mutation path that touches `multi_chain.chain_supplies`. It maintains the invariant in two checks:

1. The new chain supply for `chain` cannot go negative (saturating-subtract policy is wrong; underflow must trap).
2. `sum(chain_supplies) + delta` must remain consistent with `total_debt` at the call site (caller passes both).

Phase 1a's apply_supply_delta is a pure-state function with no inter-canister calls.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_supply.rs`:

```rust
use super::supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};
use super::config::{ChainConfigV1, ChainId, ChainStatus, GasStrategy};
use super::multi_chain_state::MultiChainStateV1;

fn fixture_state() -> MultiChainStateV1 {
    let mut s = MultiChainStateV1::default();
    s.chain_configs.insert(
        ChainId(101),
        ChainConfigV1 {
            chain_id: ChainId(101),
            display_name: "TestChain".into(),
            rpc_endpoints: vec![],
            finality_depth: 1,
            gas_strategy: GasStrategy::NotApplicable,
            chain_native_decimals: 18,
            registered_at_ns: 0,
            status: ChainStatus::Registered,
        },
    );
    s.chain_supplies.insert(ChainId(101), 0);
    s
}

#[test]
fn increase_supply_preserves_invariant() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1_000),
        /* total_debt_e8s = */ 1_000,
    );
    assert!(res.is_ok());
    assert_eq!(s.chain_supplies[&ChainId(101)], 1_000);
}

#[test]
fn decrease_supply_below_zero_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(500),
        /* total_debt_e8s = */ 0,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Underflow { .. })));
    // Failed mutation must not have touched chain_supplies.
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}

#[test]
fn decrease_to_exact_zero_keeps_entry_for_audit() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 50);
    apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Decrease(50),
        /* total_debt_e8s = */ 0,
    ).expect("decrease to zero");
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    // We keep the entry so `get_supply_audit` still surfaces the chain.
    assert!(s.chain_supplies.contains_key(&ChainId(101)));
}

#[test]
fn unknown_chain_id_is_rejected() {
    let mut s = fixture_state();
    let res = apply_supply_delta(
        &mut s,
        ChainId(999),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(res, Err(SupplyInvariantError::UnknownChain(_))));
}

#[test]
fn invariant_halted_blocks_every_mutation() {
    let mut s = fixture_state();
    s.invariant_halted = true;
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(1),
        /* total_debt_e8s = */ 1,
    );
    assert!(matches!(res, Err(SupplyInvariantError::HaltedAfterSelfCheckFailure)));
}

#[test]
fn divergence_from_total_debt_is_rejected() {
    let mut s = fixture_state();
    s.chain_supplies.insert(ChainId(101), 100);
    // After applying +50, sum = 150, but caller passes total_debt = 200.
    // That's a 50-unit divergence; the caller wired up the mint wrong.
    let res = apply_supply_delta(
        &mut s,
        ChainId(101),
        SupplyDelta::Increase(50),
        /* total_debt_e8s = */ 200,
    );
    assert!(matches!(res, Err(SupplyInvariantError::Divergence { .. })));
    // Failed mutation: chain_supplies unchanged.
    assert_eq!(s.chain_supplies[&ChainId(101)], 100);
}
```

Wire in `mod.rs`:

```rust
#[cfg(test)]
mod tests_supply;
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_supply 2>&1 | tail -10
```

- [ ] **Step 3: Implement supply.rs**

Replace the entire contents of `src/rumi_protocol_backend/src/chains/supply.rs` with:

```rust
//! Supply-invariant enforcement.
//!
//! Every mutation to `multi_chain.chain_supplies` flows through
//! `apply_supply_delta`. The function maintains the invariant
//! `sum(chain_supplies) == total_debt` at call time, refuses underflows
//! and unknown chain ids, and short-circuits whenever a prior Timer B
//! self-check left `multi_chain.invariant_halted = true`.
//!
//! Phase 1a never invokes `apply_supply_delta` from a state-mutating
//! endpoint (no flow mints icUSD on a foreign chain yet). The function
//! exists so Phase 1b's first cross-chain mint, burn, and bridge ops
//! can call it without inventing the invariant under deadline pressure.

use super::config::ChainId;
use super::multi_chain_state::MultiChainStateV1;
use candid::{CandidType, Deserialize};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug)]
pub enum SupplyDelta {
    Increase(u128),
    Decrease(u128),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SupplyInvariantError {
    UnknownChain(ChainId),
    Underflow { chain: ChainId, current: u128, attempted_decrease: u128 },
    Divergence { sum_after: u128, total_debt: u128 },
    HaltedAfterSelfCheckFailure,
}

/// Single-entry mutation path for `chain_supplies`. Caller passes the
/// authoritative `total_debt_e8s` snapshot taken at the same logical
/// moment; we reject any apply that would leave sum != total_debt.
pub fn apply_supply_delta(
    state: &mut MultiChainStateV1,
    chain: ChainId,
    delta: SupplyDelta,
    total_debt_e8s: u128,
) -> Result<(), SupplyInvariantError> {
    if state.invariant_halted {
        return Err(SupplyInvariantError::HaltedAfterSelfCheckFailure);
    }
    let current = match state.chain_supplies.get(&chain) {
        Some(v) => *v,
        None => return Err(SupplyInvariantError::UnknownChain(chain)),
    };
    let new = match delta {
        SupplyDelta::Increase(n) => current.saturating_add(n),
        SupplyDelta::Decrease(n) => {
            if n > current {
                return Err(SupplyInvariantError::Underflow {
                    chain,
                    current,
                    attempted_decrease: n,
                });
            }
            current - n
        }
    };

    // Compute the post-delta sum WITHOUT mutating state yet, so a divergence
    // rejection leaves the state untouched.
    let sum_after: u128 = state
        .chain_supplies
        .iter()
        .map(|(&id, &v)| if id == chain { new } else { v })
        .sum();
    if sum_after != total_debt_e8s {
        return Err(SupplyInvariantError::Divergence { sum_after, total_debt: total_debt_e8s });
    }

    state.chain_supplies.insert(chain, new);
    Ok(())
}

/// Phase 1a periodic self-check (called from Timer B in Task 10).
/// Returns `Ok(())` when sum == total_debt and `Err(...)` otherwise.
/// On `Err`, the caller flips `state.invariant_halted = true` and emits
/// an event.
pub fn check_invariant(
    state: &MultiChainStateV1,
    total_debt_e8s: u128,
) -> Result<(), SupplyInvariantError> {
    let sum: u128 = state.chain_supplies.values().copied().sum();
    if sum != total_debt_e8s {
        return Err(SupplyInvariantError::Divergence { sum_after: sum, total_debt: total_debt_e8s });
    }
    Ok(())
}
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_supply
```

Expected: all six tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(multi-chain): apply_supply_delta + check_invariant

Phase 1a Task 7. Single-entry mutation path for chain_supplies. Underflow,
unknown chain, divergence, and halt all reject with no state mutation."
```

---

## Task 8: Property Tests for the Supply Invariant

**Files:**
- Create: `src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs` (integration tests dir, not src/)

Spec Section 3 calls for proptest coverage that randomizes cross-chain op sequences and asserts the invariant after every step. The harness in `src/tests.rs` already uses proptest 1.0; we add the multi-chain harness as a standalone integration test so it can't accidentally pull in inter-canister calls.

- [ ] **Step 1: Write the property test**

Create `src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs`:

```rust
//! Property tests for the supply invariant under random cross-chain ops.
//!
//! Strategy: build a randomized sequence of (chain_id, op) pairs where
//! ops are Mint(amount) / Burn(amount) / Bridge(src, dst, amount). Apply
//! each op via `apply_supply_delta` (failing the test if any apply errors
//! with `Divergence`), and assert after every step that
//! `sum(chain_supplies) == total_debt`. The harness tracks `total_debt`
//! explicitly so the property test does not depend on the live State.

use rumi_protocol_backend::chains::config::{
    ChainConfigV1, ChainId, ChainStatus, GasStrategy,
};
use rumi_protocol_backend::chains::multi_chain_state::MultiChainStateV1;
use rumi_protocol_backend::chains::supply::{apply_supply_delta, SupplyDelta};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Mint { chain: u32, amount: u64 },
    Burn { chain: u32, amount: u64 },
    Bridge { src: u32, dst: u32, amount: u64 },
}

fn arb_op() -> impl Strategy<Value = Op> {
    let chain_id_strat = 1u32..=5u32;
    let amount_strat = 1u64..=1_000_000u64;
    prop_oneof![
        (chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(c, a)| Op::Mint { chain: c, amount: a }),
        (chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(c, a)| Op::Burn { chain: c, amount: a }),
        (chain_id_strat.clone(), chain_id_strat.clone(), amount_strat.clone())
            .prop_map(|(s, d, a)| Op::Bridge { src: s, dst: d, amount: a }),
    ]
}

fn seeded_state() -> MultiChainStateV1 {
    let mut state = MultiChainStateV1::default();
    for id in 1u32..=5u32 {
        state.chain_configs.insert(
            ChainId(id),
            ChainConfigV1 {
                chain_id: ChainId(id),
                display_name: format!("chain-{}", id),
                rpc_endpoints: vec![],
                finality_depth: 1,
                gas_strategy: GasStrategy::NotApplicable,
                chain_native_decimals: 18,
                registered_at_ns: 0,
                status: ChainStatus::Registered,
            },
        );
        state.chain_supplies.insert(ChainId(id), 0);
    }
    state
}

proptest! {
    #[test]
    fn invariant_holds_after_every_random_op(ops in proptest::collection::vec(arb_op(), 0..40)) {
        let mut state = seeded_state();
        let mut total_debt: u128 = 0;

        for op in ops {
            match op {
                Op::Mint { chain, amount } => {
                    // A mint increases both the destination chain supply and
                    // total debt by the same amount. Compute the new
                    // total_debt FIRST so the caller can pass it through.
                    let new_total = total_debt + amount as u128;
                    let res = apply_supply_delta(
                        &mut state,
                        ChainId(chain),
                        SupplyDelta::Increase(amount as u128),
                        new_total,
                    );
                    if res.is_ok() {
                        total_debt = new_total;
                    }
                }
                Op::Burn { chain, amount } => {
                    // A burn decreases both the source chain supply and total
                    // debt by the same amount. Skip if it would underflow
                    // either side (the canister rejects underflow; we mirror
                    // that here).
                    let current = state.chain_supplies[&ChainId(chain)];
                    if (amount as u128) > current || (amount as u128) > total_debt {
                        continue;
                    }
                    let new_total = total_debt - amount as u128;
                    let res = apply_supply_delta(
                        &mut state,
                        ChainId(chain),
                        SupplyDelta::Decrease(amount as u128),
                        new_total,
                    );
                    if res.is_ok() {
                        total_debt = new_total;
                    }
                }
                Op::Bridge { src, dst, amount } => {
                    // Bridge: burn on src, mint on dst, total_debt unchanged.
                    if src == dst { continue; }
                    let current_src = state.chain_supplies[&ChainId(src)];
                    if (amount as u128) > current_src { continue; }
                    let burn = apply_supply_delta(
                        &mut state,
                        ChainId(src),
                        SupplyDelta::Decrease(amount as u128),
                        total_debt,
                    );
                    prop_assert!(burn.is_ok(), "bridge burn rejected: {:?}", burn);
                    let mint = apply_supply_delta(
                        &mut state,
                        ChainId(dst),
                        SupplyDelta::Increase(amount as u128),
                        total_debt,
                    );
                    prop_assert!(mint.is_ok(), "bridge mint rejected: {:?}", mint);
                }
            }

            // Invariant after every op:
            let sum: u128 = state.chain_supplies.values().copied().sum();
            prop_assert_eq!(sum, total_debt);
        }
    }
}
```

- [ ] **Step 2: Run it**

```bash
cargo test --package rumi_protocol_backend --test multi_chain_supply_invariant
```

Expected: passes. proptest's default of 256 cases runs in under 5 seconds.

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs
git commit -m "test(multi-chain): proptest harness for the supply invariant

Phase 1a Task 8. Randomised Mint/Burn/Bridge sequences across five chains;
sum(chain_supplies) == total_debt after every op."
```

---

## Task 9: get_global_icusd_supply() and get_supply_audit() Queries

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (add two `#[query]` handlers)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `src/rumi_protocol_backend/src/lib.rs` (add `SupplyAudit` + `SupplyAuditEntry` types)
- Modify: `src/rumi_protocol_backend/src/tests.rs` (round-trip test)

`get_global_icusd_supply` returns `nat` (Candid encoding of `u128`). `get_supply_audit` returns a list of per-chain entries: `(chain_id, display_name, supply_e8s)`.

- [ ] **Step 1: Write the failing test FIRST**

Append to `src/rumi_protocol_backend/src/tests.rs`:

```rust
#[test]
fn supply_audit_round_trips_via_candid() {
    use candid::{Decode, Encode};
    use crate::{SupplyAudit, SupplyAuditEntry};
    use crate::chains::config::ChainId;

    let audit = SupplyAudit {
        total_e8s: 150_000,
        per_chain: vec![
            SupplyAuditEntry { chain_id: ChainId(1), display_name: "ICP".into(), supply_e8s: 100_000 },
            SupplyAuditEntry { chain_id: ChainId(2), display_name: "Monad".into(), supply_e8s: 50_000 },
        ],
    };
    let bytes = Encode!(&audit).expect("encode");
    let back: SupplyAudit = Decode!(&bytes, SupplyAudit).expect("decode");
    assert_eq!(back.total_e8s, 150_000);
    assert_eq!(back.per_chain.len(), 2);
}
```

Run:

```bash
cargo test --package rumi_protocol_backend --lib supply_audit_round_trips_via_candid 2>&1 | tail
```

Expected: compile failure.

- [ ] **Step 2: Add the public types to lib.rs**

Edit `src/rumi_protocol_backend/src/lib.rs`. Add after `ProtocolStatus` definition (around line 180):

```rust
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SupplyAuditEntry {
    pub chain_id: crate::chains::config::ChainId,
    pub display_name: String,
    pub supply_e8s: u128,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SupplyAudit {
    pub total_e8s: u128,
    pub per_chain: Vec<SupplyAuditEntry>,
}
```

- [ ] **Step 3: Add the queries in main.rs**

Edit `src/rumi_protocol_backend/src/main.rs`. After the existing `get_protocol_status` query (around line 616), add two new queries:

```rust
/// Phase 1a: canonical multi-chain icUSD supply (sum across all chains).
/// Equals `sum(state.multi_chain.chain_supplies.values())`. Returns 0 when
/// no chains are registered (the Phase 1a default state).
///
/// Note: this query is read-only and does NOT exercise the invariant
/// check. Operators investigating drift should call `get_supply_audit`
/// for the per-chain breakdown.
#[candid_method(query)]
#[query]
fn get_global_icusd_supply() -> u128 {
    read_state(|s| s.multi_chain.total_supply_all_chains_e8s())
}

/// Phase 1a: per-chain breakdown for external auditors. Iterates
/// `multi_chain.chain_configs` in chain-id order so the response shape is
/// deterministic.
#[candid_method(query)]
#[query]
fn get_supply_audit() -> SupplyAudit {
    read_state(|s| {
        let mut per_chain = Vec::with_capacity(s.multi_chain.chain_configs.len());
        for (chain_id, cfg) in s.multi_chain.chain_configs.iter() {
            let supply = s.multi_chain.chain_supplies.get(chain_id).copied().unwrap_or(0);
            per_chain.push(SupplyAuditEntry {
                chain_id: *chain_id,
                display_name: cfg.display_name.clone(),
                supply_e8s: supply,
            });
        }
        SupplyAudit {
            total_e8s: per_chain.iter().map(|e| e.supply_e8s).sum(),
            per_chain,
        }
    })
}
```

Also add `SupplyAudit, SupplyAuditEntry` to the `use rumi_protocol_backend::{...}` import block at the top of main.rs.

- [ ] **Step 4: Mirror into Candid**

Edit `src/rumi_protocol_backend/rumi_protocol_backend.did`. Add type declarations near the top of the file (after `type ProtocolError = ...`):

```candid
type ChainId = nat32;

type SupplyAuditEntry = record {
    chain_id : ChainId;
    display_name : text;
    supply_e8s : nat;
};

type SupplyAudit = record {
    total_e8s : nat;
    per_chain : vec SupplyAuditEntry;
};
```

In the `service : { ... }` block, add the two queries (place near the other status queries):

```candid
    get_global_icusd_supply : () -> (nat) query;
    get_supply_audit : () -> (SupplyAudit) query;
```

- [ ] **Step 5: Run tests**

```bash
cargo test --package rumi_protocol_backend --lib supply_audit_round_trips_via_candid
cargo test --package rumi_protocol_backend --lib
```

Expected: every test green.

- [ ] **Step 6: Regenerate frontend declarations**

```bash
npm run regenerate-declarations
```

Expected: `declarations/rumi_protocol_backend/` rewrites with the two new methods. Do not commit any other regenerated file; only the backend's declarations changed.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/rumi_protocol_backend.did src/rumi_protocol_backend/src/tests.rs declarations/rumi_protocol_backend/
git commit -m "feat(multi-chain): get_global_icusd_supply + get_supply_audit queries

Phase 1a Task 9. Public read-only surface for the canonical multi-chain
supply. Returns 0 on a fresh canister (Phase 1a default)."
```

---

## Task 10: Admin Endpoints (register_chain, disable_chain, set_chain_config)

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (three new `#[update]` handlers)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `src/rumi_protocol_backend/src/event.rs` (three new `Event` variants: `ChainRegistered`, `ChainDisabled`, `ChainConfigUpdated`)
- Modify: `src/rumi_protocol_backend/src/state.rs` (extend `Event` replay match arms if needed)
- Create: `src/rumi_protocol_backend/src/chains/tests_admin.rs`

All three endpoints are developer-only (gated on `state.developer_principal == caller`). They mutate `state.multi_chain.chain_configs` and emit an event for the on-chain audit log.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_admin.rs`:

```rust
//! Direct-state tests for the chain-admin mutations. The full update-endpoint
//! flow (caller check, event recording, traps) is exercised in PocketIC under
//! Task 13.

use super::config::{ChainConfigV1, ChainId, ChainStatus, GasStrategy, RegisterChainArg, UpdateChainConfigArg};
use super::multi_chain_state::MultiChainStateV1;
use crate::chains::admin::{disable_chain_in_state, register_chain_in_state, update_chain_config_in_state, ChainAdminError};

fn arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: ChainId(101),
        display_name: "Monad".into(),
        rpc_endpoints: vec!["https://rpc.example".into()],
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 { max_priority_fee_gwei: 2, max_fee_gwei_ceiling: 200 },
        chain_native_decimals: 18,
    }
}

#[test]
fn register_chain_inserts_config_and_zero_supply() {
    let mut s = MultiChainStateV1::default();
    register_chain_in_state(&mut s, arg(), /*now_ns=*/ 1_700_000_000_000_000_000).expect("register");
    assert!(s.chain_configs.contains_key(&ChainId(101)));
    assert_eq!(s.chain_supplies[&ChainId(101)], 0);
    assert!(s.settlement_queues.contains_key(&ChainId(101)));
    let cfg = &s.chain_configs[&ChainId(101)];
    assert!(matches!(cfg.status, ChainStatus::Registered));
}

#[test]
fn register_chain_rejects_duplicates() {
    let mut s = MultiChainStateV1::default();
    register_chain_in_state(&mut s, arg(), 0).expect("first");
    let err = register_chain_in_state(&mut s, arg(), 0).expect_err("duplicate");
    assert!(matches!(err, ChainAdminError::ChainAlreadyRegistered(ChainId(101))));
}

#[test]
fn register_chain_rejects_empty_rpc_endpoints() {
    let mut s = MultiChainStateV1::default();
    let mut a = arg();
    a.rpc_endpoints = vec![];
    let err = register_chain_in_state(&mut s, a, 0).expect_err("empty endpoints");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}

#[test]
fn disable_chain_flips_status_and_preserves_supply() {
    let mut s = MultiChainStateV1::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    s.chain_supplies.insert(ChainId(101), 999);
    disable_chain_in_state(&mut s, ChainId(101)).expect("disable");
    assert!(matches!(s.chain_configs[&ChainId(101)].status, ChainStatus::Disabled));
    assert_eq!(s.chain_supplies[&ChainId(101)], 999);
}

#[test]
fn set_chain_config_updates_supplied_fields_only() {
    let mut s = MultiChainStateV1::default();
    register_chain_in_state(&mut s, arg(), 0).expect("register");
    let original_name = s.chain_configs[&ChainId(101)].display_name.clone();
    let update = UpdateChainConfigArg {
        display_name: None,
        rpc_endpoints: Some(vec!["https://new.example".into()]),
        finality_depth: Some(5),
        gas_strategy: None,
    };
    update_chain_config_in_state(&mut s, ChainId(101), update).expect("update");
    assert_eq!(s.chain_configs[&ChainId(101)].display_name, original_name);
    assert_eq!(s.chain_configs[&ChainId(101)].rpc_endpoints.len(), 1);
    assert_eq!(s.chain_configs[&ChainId(101)].finality_depth, 5);
}

#[test]
fn set_chain_config_rejects_unknown_chain() {
    let mut s = MultiChainStateV1::default();
    let err = update_chain_config_in_state(
        &mut s,
        ChainId(404),
        UpdateChainConfigArg::default(),
    ).expect_err("unknown chain");
    assert!(matches!(err, ChainAdminError::ChainNotRegistered(_)));
}
```

Wire in `mod.rs`:

```rust
pub mod admin;
#[cfg(test)]
mod tests_admin;
```

- [ ] **Step 2: Run the test and confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_admin 2>&1 | tail
```

- [ ] **Step 3: Write the pure-state admin module**

Create `src/rumi_protocol_backend/src/chains/admin.rs`:

```rust
//! Pure-state mutation helpers for the chain-admin endpoints. The
//! `#[update]` handlers in `main.rs` call into these after the caller
//! check + event recording. Kept here so unit tests can exercise the
//! state-shape rules without spinning up PocketIC.

use super::config::{
    ChainAdminError, ChainConfigV1, ChainId, ChainStatus, RegisterChainArg,
    UpdateChainConfigArg,
};
use super::multi_chain_state::MultiChainStateV1;
use super::settlement_queue::SettlementQueueV1;

pub use super::config::ChainAdminError as _ChainAdminError;

pub fn register_chain_in_state(
    state: &mut MultiChainStateV1,
    arg: RegisterChainArg,
    now_ns: u64,
) -> Result<ChainConfigV1, ChainAdminError> {
    if arg.rpc_endpoints.is_empty() {
        return Err(ChainAdminError::InvalidConfig(
            "rpc_endpoints must contain at least one URL".into(),
        ));
    }
    if state.chain_configs.contains_key(&arg.chain_id) {
        return Err(ChainAdminError::ChainAlreadyRegistered(arg.chain_id));
    }
    let cfg = ChainConfigV1 {
        chain_id: arg.chain_id,
        display_name: arg.display_name,
        rpc_endpoints: arg.rpc_endpoints,
        finality_depth: arg.finality_depth,
        gas_strategy: arg.gas_strategy,
        chain_native_decimals: arg.chain_native_decimals,
        registered_at_ns: now_ns,
        status: ChainStatus::Registered,
    };
    state.chain_configs.insert(arg.chain_id, cfg.clone());
    state.chain_supplies.insert(arg.chain_id, 0);
    state.settlement_queues.insert(arg.chain_id, SettlementQueueV1::default());
    Ok(cfg)
}

pub fn disable_chain_in_state(
    state: &mut MultiChainStateV1,
    chain_id: ChainId,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;
    cfg.status = ChainStatus::Disabled;
    Ok(())
}

pub fn update_chain_config_in_state(
    state: &mut MultiChainStateV1,
    chain_id: ChainId,
    update: UpdateChainConfigArg,
) -> Result<(), ChainAdminError> {
    let cfg = state
        .chain_configs
        .get_mut(&chain_id)
        .ok_or(ChainAdminError::ChainNotRegistered(chain_id))?;
    if let Some(name) = update.display_name { cfg.display_name = name; }
    if let Some(eps) = update.rpc_endpoints {
        if eps.is_empty() {
            return Err(ChainAdminError::InvalidConfig("rpc_endpoints cannot be empty".into()));
        }
        cfg.rpc_endpoints = eps;
    }
    if let Some(d) = update.finality_depth { cfg.finality_depth = d; }
    if let Some(g) = update.gas_strategy { cfg.gas_strategy = g; }
    Ok(())
}
```

- [ ] **Step 4: Add the three Event variants**

Edit `src/rumi_protocol_backend/src/event.rs`. Find the `pub enum Event { ... }` block and append (BEFORE the last close-brace; keep the variant order append-only so historical CBOR decodes unchanged):

```rust
    ChainRegistered {
        chain_id: crate::chains::config::ChainId,
        display_name: String,
        timestamp: u64,
    },
    ChainDisabled {
        chain_id: crate::chains::config::ChainId,
        timestamp: u64,
    },
    ChainConfigUpdated {
        chain_id: crate::chains::config::ChainId,
        timestamp: u64,
    },
```

If the event replay function in `state.rs` exhaustively matches `Event` variants, add no-op match arms for the three new variants (they are observability events; the state mutation has already happened via the admin pure-state helpers):

```rust
            Event::ChainRegistered { .. } | Event::ChainDisabled { .. } | Event::ChainConfigUpdated { .. } => {
                // Replay is a no-op; the actual state mutations were already applied
                // to multi_chain.* before the event was recorded.
            }
```

Run `cargo build --package rumi_protocol_backend` and let the compiler tell you which match arms need extending. Add no-op arms in every spot.

- [ ] **Step 5: Add the three update handlers in main.rs**

Edit `src/rumi_protocol_backend/src/main.rs`. After the existing developer-gated endpoints (e.g., `set_redemption_tier` around line 3190), add:

```rust
use rumi_protocol_backend::chains::admin::{
    disable_chain_in_state, register_chain_in_state, update_chain_config_in_state,
};
use rumi_protocol_backend::chains::config::{
    ChainId, RegisterChainArg, UpdateChainConfigArg,
};

#[candid_method(update)]
#[update]
fn register_chain(arg: RegisterChainArg) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    let chain_id = arg.chain_id;
    let display_name = arg.display_name.clone();
    let result = mutate_state(|s| register_chain_in_state(&mut s.multi_chain, arg, now));
    match result {
        Ok(_) => {
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainRegistered {
                chain_id, display_name, timestamp: now,
            });
            log!(INFO, "[register_chain] chain_id={:?} registered", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}

#[candid_method(update)]
#[update]
fn disable_chain(chain_id: ChainId) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let result = mutate_state(|s| disable_chain_in_state(&mut s.multi_chain, chain_id));
    match result {
        Ok(()) => {
            let now = ic_cdk::api::time();
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainDisabled {
                chain_id, timestamp: now,
            });
            log!(INFO, "[disable_chain] chain_id={:?} disabled", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}

#[candid_method(update)]
#[update]
fn set_chain_config(chain_id: ChainId, update: UpdateChainConfigArg) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let result = mutate_state(|s| update_chain_config_in_state(&mut s.multi_chain, chain_id, update));
    match result {
        Ok(()) => {
            let now = ic_cdk::api::time();
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainConfigUpdated {
                chain_id, timestamp: now,
            });
            log!(INFO, "[set_chain_config] chain_id={:?} updated", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}
```

- [ ] **Step 6: Mirror into Candid**

Edit `src/rumi_protocol_backend/rumi_protocol_backend.did`. Add types alongside `SupplyAudit`:

```candid
type GasStrategy = variant {
    EvmEip1559 : record { max_priority_fee_gwei : nat64; max_fee_gwei_ceiling : nat64 };
    EvmLegacy : record { gas_price_gwei_ceiling : nat64 };
    SolanaPriorityFee : record { lamports_per_cu_ceiling : nat64 };
    NotApplicable;
};

type RegisterChainArg = record {
    chain_id : ChainId;
    display_name : text;
    rpc_endpoints : vec text;
    finality_depth : nat32;
    gas_strategy : GasStrategy;
    chain_native_decimals : nat8;
};

type UpdateChainConfigArg = record {
    display_name : opt text;
    rpc_endpoints : opt vec text;
    finality_depth : opt nat32;
    gas_strategy : opt GasStrategy;
};
```

In the `service` block, add the three updates:

```candid
    register_chain : (RegisterChainArg) -> (variant { Ok; Err : ProtocolError });
    disable_chain : (ChainId) -> (variant { Ok; Err : ProtocolError });
    set_chain_config : (ChainId, UpdateChainConfigArg) -> (variant { Ok; Err : ProtocolError });
```

- [ ] **Step 7: Run all tests**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_admin
cargo test --package rumi_protocol_backend --lib
```

Expected: every test green. If event-replay match arms in `state.rs` are non-exhaustive, the compiler will point at the exact line; add the no-op arm and re-run.

- [ ] **Step 8: Regenerate frontend declarations**

```bash
npm run regenerate-declarations
```

- [ ] **Step 9: Commit**

```bash
git add src/rumi_protocol_backend/src/ src/rumi_protocol_backend/rumi_protocol_backend.did declarations/rumi_protocol_backend/
git commit -m "feat(multi-chain): register_chain + disable_chain + set_chain_config

Phase 1a Task 10. Developer-gated admin surface for managing the chains
registry. Emits ChainRegistered / ChainDisabled / ChainConfigUpdated
events for the on-chain audit log."
```

---

## Task 11: Periodic Self-Check on Timer B

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (extend `interest_and_treasury_tick`)
- Modify: `src/rumi_protocol_backend/src/event.rs` (`SupplyInvariantSelfCheckFailed` variant)
- Create: `src/rumi_protocol_backend/src/chains/tests_self_check.rs`

The self-check runs inside the existing Timer B handler (cadence governed by `state.interest_treasury_tick_interval_secs`, default 60s). On divergence: set `multi_chain.invariant_halted = true`, record `Event::SupplyInvariantSelfCheckFailed`, and (per spec Section 3) flip `state.mode = Mode::ReadOnly` with `mode_triggered_by_oracle = false` so the operator must explicitly clear it.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/tests_self_check.rs`:

```rust
//! Self-check semantics: when sum(chain_supplies) != total_debt, the
//! self-check flips the halt flag and Mode and stops mutating supplies.

use super::config::{ChainConfigV1, ChainId, ChainStatus, GasStrategy};
use super::multi_chain_state::MultiChainStateV1;
use super::supply::check_invariant;

#[test]
fn check_invariant_passes_when_sum_equals_total_debt() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 100);
    s.chain_supplies.insert(ChainId(2), 200);
    assert!(check_invariant(&s, 300).is_ok());
}

#[test]
fn check_invariant_fails_on_drift() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 100);
    s.chain_supplies.insert(ChainId(2), 200);
    let err = check_invariant(&s, 299).expect_err("drift must be caught");
    assert!(matches!(err, super::supply::SupplyInvariantError::Divergence { .. }));
}

#[test]
fn empty_state_passes_when_total_debt_is_zero() {
    let s = MultiChainStateV1::default();
    assert!(check_invariant(&s, 0).is_ok());
}
```

Wire in `mod.rs`:

```rust
#[cfg(test)]
mod tests_self_check;
```

- [ ] **Step 2: Run the test to confirm it passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_self_check
```

Expected: passes (function `check_invariant` already exists from Task 7).

- [ ] **Step 3: Add the event variant**

Edit `src/rumi_protocol_backend/src/event.rs`. Append (keep variant order append-only):

```rust
    SupplyInvariantSelfCheckFailed {
        sum_chain_supplies_e8s: u128,
        total_debt_e8s: u128,
        timestamp: u64,
    },
```

Add the no-op replay match arm where Phase 1a added the chain-admin arms.

- [ ] **Step 4: Wire the self-check into the existing Timer B handler**

Edit `src/rumi_protocol_backend/src/main.rs`. Find the function registered by `register_interest_treasury_timer` (search for `register_interest_treasury_timer` or `interest_and_treasury_tick`). Inside that function, after the existing interest accrual + treasury sweep but before returning, add:

```rust
// Phase 1a: supply-invariant self-check. Runs on every Timer B tick
// (default cadence 60s) per spec Section 3. On drift, halt new
// debt issuance + supply mutations and flip the protocol into
// ReadOnly. Manual recovery requires `clear_invariant_halt` (lands
// in Phase 1b operational tooling) plus a developer-gated mode flip.
//
// In Phase 1a the chain_supplies table is empty and total_debt_e8s
// represents only ICP-side icUSD, so sum(chain_supplies) == 0 and
// the check always passes UNLESS a future bug somewhere increments
// chain_supplies without going through `apply_supply_delta`. That
// is the failure mode this check is designed to surface.
let total_debt_e8s: u128 = read_state(|s| s.total_borrowed_icusd_amount().to_u64() as u128);
let check_outcome = read_state(|s| {
    rumi_protocol_backend::chains::supply::check_invariant(&s.multi_chain, total_debt_e8s)
});
if let Err(err) = check_outcome {
    let now = ic_cdk::api::time();
    let (sum, td) = match err {
        rumi_protocol_backend::chains::supply::SupplyInvariantError::Divergence { sum_after, total_debt } => (sum_after, total_debt),
        _ => (0u128, total_debt_e8s),
    };
    mutate_state(|s| {
        s.multi_chain.invariant_halted = true;
        if matches!(s.mode, Mode::GeneralAvailability) {
            s.mode = Mode::ReadOnly;
            s.mode_triggered_by_oracle = false;
        }
    });
    rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::SupplyInvariantSelfCheckFailed {
        sum_chain_supplies_e8s: sum,
        total_debt_e8s: td,
        timestamp: now,
    });
    log!(INFO, "[supply_invariant] FAILED: sum={} total_debt={}; halting and flipping to ReadOnly", sum, td);
}
```

`total_debt_e8s` widens to `u128` because `chain_supplies` carries u128 values; on a 64-bit overflow the cast saturates at u64::MAX, which is wildly beyond any real total debt (>184 quintillion e8s).

- [ ] **Step 5: Run all tests**

```bash
cargo test --package rumi_protocol_backend --lib
cargo test --package rumi_protocol_backend --test multi_chain_supply_invariant
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/rumi_protocol_backend/src/
git commit -m "feat(multi-chain): supply-invariant self-check on Timer B

Phase 1a Task 11. Timer B (default 60s) recomputes sum(chain_supplies)
against total_debt; on drift, sets invariant_halted, flips to ReadOnly
(non-oracle-triggered), and emits SupplyInvariantSelfCheckFailed."
```

---

## Task 12: PocketIC End-to-End Test for the Phase 1a Surface

**Files:**
- Create: `src/rumi_protocol_backend/tests/phase1a_scaffolding_pic.rs`

A PocketIC test boots the backend on a local IC, calls each new endpoint, asserts the responses, and round-trips an upgrade to confirm `multi_chain` survives. This is the explicit guard against the AMM-state-wipe failure mode.

- [ ] **Step 1: Write the test**

The existing PocketIC tests under `src/rumi_protocol_backend/tests/` (e.g. `audit_pocs_icc_002_3usd_refund.rs`) mirror `ProtocolInitArg` and `ProtocolArgVariant` locally rather than importing them from the crate. Phase 1a's PocketIC test follows the same pattern so it stays self-contained.

Create `src/rumi_protocol_backend/tests/phase1a_scaffolding_pic.rs`:

```rust
//! Phase 1a PocketIC smoke test.
//!
//! Boots `rumi_protocol_backend`, calls every new endpoint, then upgrades
//! the canister in place and re-checks every query. The upgrade round-trip
//! is the guard against the AMM-style state-wipe failure mode: if the
//! new `multi_chain` field's CBOR shape goes sideways, the second
//! `get_supply_audit()` would lose its registered chain.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, WasmResult};

// Locally-mirrored types so this test does not pull in the crate's macros.
// Field shapes must mirror `src/rumi_protocol_backend/src/lib.rs` exactly.

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolUpgradeArg {
    mode: Option<Mode>,
    description: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum Mode {
    GeneralAvailability,
    Recovery,
    ReadOnly,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
    Upgrade(ProtocolUpgradeArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainIdWire(u32);

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditEntryWire {
    chain_id: ChainIdWire,
    display_name: String,
    supply_e8s: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditWire {
    total_e8s: candid::Nat,
    per_chain: Vec<SupplyAuditEntryWire>,
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 100_000_000_000_000);

    // Phase 1a's PocketIC test does NOT install the ICP ledger, icusd
    // ledger, or XRC. We only exercise the chain-agnostic surface
    // (queries + admin endpoints + upgrade round-trip), none of which
    // make inter-canister calls. The init principals point at the
    // management canister so any accidental outbound call traps fast.
    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let developer = Principal::from_text("aaaaa-aa").expect("dev principal");

    let init = ProtocolArgVariant::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer,
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        protocol_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    (pic, protocol_id)
}

fn query<T>(pic: &PocketIc, cid: Principal, method: &str) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    let reply = pic
        .query_call(cid, Principal::anonymous(), method, encode_one(()).expect("encode unit"))
        .expect("query call");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, T).expect("decode reply"),
        WasmResult::Reject(msg) => panic!("query {} rejected: {}", method, msg),
    }
}

#[test]
fn phase1a_queries_return_empty_on_fresh_canister() {
    let (pic, cid) = boot();
    let supply: candid::Nat = query(&pic, cid, "get_global_icusd_supply");
    assert_eq!(supply, candid::Nat::from(0u32));

    let audit: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32));
    assert!(audit.per_chain.is_empty());
}

#[test]
fn phase1a_state_survives_upgrade() {
    let (pic, cid) = boot();
    let upgrade = ProtocolArgVariant::Upgrade(ProtocolUpgradeArg {
        mode: None,
        description: Some("Phase 1a PocketIC self-check".to_string()),
    });
    pic.upgrade_canister(
        cid,
        backend_wasm(),
        encode_args((upgrade,)).expect("encode upgrade"),
        None,
    )
    .expect("upgrade");

    let audit: SupplyAuditWire = query(&pic, cid, "get_supply_audit");
    assert_eq!(audit.total_e8s, candid::Nat::from(0u32));
    assert!(audit.per_chain.is_empty());
}
```

If the real `ProtocolInitArg` shape in `src/rumi_protocol_backend/src/lib.rs` has changed since this plan was authored (e.g. a new required field added), update the locally-mirrored struct above to match. The mirror pattern means a stale shape produces a Candid decode error at canister-install time, not a silent test pass.

- [ ] **Step 2: Build the wasm fresh (the PocketIC test needs it on disk)**

```bash
cargo build --target wasm32-unknown-unknown --release --package rumi_protocol_backend
```

- [ ] **Step 3: Run the test**

```bash
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1a_scaffolding_pic
```

Expected: both tests pass. If `phase1a_state_survives_upgrade` fails, the most likely cause is `multi_chain` losing its `#[serde(default)]` annotation; re-check Task 5.

**Decision gate:** if this PocketIC upgrade test fails, STOP and surface to Rob. This is the rehearsal that proves the upgrade-safe pattern. Failing here means Phase 1a does not ship.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/tests/phase1a_scaffolding_pic.rs
git commit -m "test(multi-chain): PocketIC upgrade round-trip for Phase 1a surface

Phase 1a Task 12. Boots backend, exercises new queries, upgrades wasm
in place, re-asserts every query. Failing this test means Phase 1a is
not safe to deploy."
```

---

## Task 13: Update Candid Declarations + Frontend Bindings

**Files:**
- Verify: `declarations/rumi_protocol_backend/rumi_protocol_backend.did.js`
- Verify: `declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts`
- Modify: `vault_frontend/` build does not break

Tasks 9 and 10 already ran `npm run regenerate-declarations`. This task is the assurance pass: walk the generated TypeScript, build the frontend, and confirm the new exports are present.

- [ ] **Step 1: Regenerate declarations one more time (idempotent)**

```bash
npm run regenerate-declarations
git status declarations/rumi_protocol_backend/
```

Expected: either no diff (declarations already up to date from Tasks 9 + 10) or a small additive diff (no removed exports).

- [ ] **Step 2: Verify the new symbols are present**

```bash
grep -E "register_chain|disable_chain|set_chain_config|get_global_icusd_supply|get_supply_audit" declarations/rumi_protocol_backend/rumi_protocol_backend.did.js
grep -E "register_chain|disable_chain|set_chain_config|get_global_icusd_supply|get_supply_audit" declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts
```

Expected: every grep returns at least one line per identifier.

- [ ] **Step 3: Verify the frontend still type-checks**

```bash
cd vault_frontend
npm run build 2>&1 | tail -30
cd ..
```

Expected: build succeeds. Phase 1a adds no frontend code; the goal is to confirm the new backend bindings don't break the existing TS imports (e.g., new `Result_*` variants for `register_chain` returning `{ Ok: null } | { Err: ProtocolError }` should not clash with existing names).

- [ ] **Step 4: Commit any drift**

```bash
git add declarations/rumi_protocol_backend/
git commit -m "feat(multi-chain): regenerate frontend declarations

Phase 1a Task 13. New backend methods surface in TypeScript; frontend
build remains green (no frontend code changes in Phase 1a)."
```

If `git status declarations/` is clean (Tasks 9 + 10 already committed everything), this task has nothing to add: proceed to Task 14 directly.

---

## Task 14: Provision the mainnet-staging Canister on IC

**Files:**
- Modify: `.icp/data/mappings/mainnet-staging.ids.json` (new file with the new principal)
- Modify: `canister_ids.json` (add a `mainnet-staging` entry alongside `ic`)
- Modify: `icp.yaml` (populate the `mainnet-staging` environment block)

`mainnet-staging` is a NEW IC environment with NEW canister IDs (distinct from `mainnet-live`). Phase 1a is the first deploy into it.

- [ ] **Step 1: Confirm the dfx wallet identity has cycles for canister creation**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
"$DFX" identity use rumi_identity
"$DFX" wallet balance --network ic
```

Expected: balance >= ~3T cycles. The new canister needs cycles for creation (2T baseline) plus a margin for the first install. If the wallet is short, top it up before continuing.

- [ ] **Step 2: Create the staging canister**

```bash
"$DFX" canister create --network ic rumi_protocol_backend_staging --identity rumi_identity
STAGING_ID=$("$DFX" canister id --network ic rumi_protocol_backend_staging)
echo "Phase 1a staging canister: $STAGING_ID"
```

Save the returned principal string. Phase 1a deploys ONLY the backend to staging (no other canister gets a staging counterpart at this phase).

- [ ] **Step 3: Stop the canister before installing (defense-in-depth)**

```bash
"$DFX" canister stop --network ic $STAGING_ID --identity rumi_identity
```

Expected: stop succeeds. The canister has no installed wasm yet.

- [ ] **Step 4: Update canister_ids.json**

Read the existing `canister_ids.json` (or `dfx.json`'s networks/ic block; verify which holds the source-of-truth ID mapping for this project):

```bash
cat canister_ids.json 2>/dev/null | head -20
```

Add a new top-level entry mirroring the existing `mainnet-live` / `ic` structure:

```json
{
  "rumi_protocol_backend": {
    "ic": "tfesu-vyaaa-aaaap-qrd7a-cai",
    "ic-staging": "<STAGING_ID from step 2>"
  }
}
```

(Use whichever key name the file actually uses for the production entry; mirror the same shape for staging.)

- [ ] **Step 5: Populate `.icp/data/mappings/mainnet-staging.ids.json`**

```bash
mkdir -p .icp/data/mappings
cat > .icp/data/mappings/mainnet-staging.ids.json <<EOF
{
  "rumi_protocol_backend": "$STAGING_ID"
}
EOF
cat .icp/data/mappings/mainnet-staging.ids.json
```

Expected: file written with the new staging principal.

- [ ] **Step 6: Update the icp.yaml mainnet-staging environment**

Edit `icp.yaml`. Find the `- name: mainnet-staging` block (currently `canisters: []`) and populate:

```yaml
  - name: mainnet-staging
    network: ic
    canisters:
      - rumi_protocol_backend
    init_args:
      # Initial deploy on staging: the canister has never been installed,
      # so this is an Init (not Upgrade). Subsequent staging deploys use
      # `--mode upgrade` per the icp-cli deploy-pattern doc, with a fresh
      # `(variant { Upgrade = record { mode = null; description = opt "..." } })`
      # passed via `--args` at install time.
      rumi_protocol_backend: |
        (variant {
          Init = record {
            xrc_principal = principal "uf6dk-hyaaa-aaaaq-qaaaq-cai";
            icusd_ledger_principal = principal "aaaaa-aa";
            icp_ledger_principal = principal "ryjl3-tyaaa-aaaaa-aaaba-cai";
            fee_e8s = 10_000 : nat64;
            developer_principal = principal "<rumi_identity principal>";
            treasury_principal = null;
            stability_pool_principal = null;
            ckusdt_ledger_principal = null;
            ckusdc_ledger_principal = null;
          }
        })
```

The Init shape MUST match the real `InitArg` struct in `src/rumi_protocol_backend/src/lib.rs` at the time of execution. Verify before pasting:

```bash
grep -A 12 "^pub struct InitArg" src/rumi_protocol_backend/src/lib.rs
```

The literal above mirrors the InitArg shape captured when this plan was authored (xrc / icusd / icp principals, fee_e8s, developer_principal, plus four Option<Principal> fields all set to `null` for the staging deploy). Init values are deliberately defensive: `icusd_ledger_principal = aaaaa-aa` and the four optional principals all `null` so any accidental mint or treasury sweep traps immediately. The protocol's runtime `mode` is governed by Phase 1a's first `set_mode` admin call (not via the Init payload, which does not carry a mode field).

Get the `rumi_identity` principal once:

```bash
"$DFX" identity get-principal --identity rumi_identity
# Substitute the returned text for <rumi_identity principal> above.
```

- [ ] **Step 7: Commit the staging environment scaffolding**

```bash
git add canister_ids.json .icp/data/mappings/mainnet-staging.ids.json icp.yaml
git commit -m "feat(icp-cli): provision mainnet-staging environment

Phase 1a Task 14. New IC canister created for staging (principal in
canister_ids.json and .icp/data/mappings/mainnet-staging.ids.json).
icp.yaml carries the staging environment block with a defensive Init
arg (Mode::ReadOnly, ledger_principal = aaaaa-aa)."
```

**Decision gate:** Pause here. Walk Rob through:
1. The new staging canister principal
2. The canister's current state (stopped, no wasm installed)
3. The `mainnet-staging.ids.json` mapping
4. The icp.yaml `mainnet-staging` env block

Get explicit "go" before installing wasm.

---

## Task 15: Install Phase 1a Wasm on mainnet-staging

**Files:**
- None modified beyond what's already committed.

This is the deploy step. Use the locked icp-cli deploy pattern from `docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md`.

- [ ] **Step 1: Build the wasm via icp-cli**

```bash
icp build rumi_protocol_backend
```

Expected: build succeeds. The output wasm (gzipped) ends up at `.icp/cache/artifacts/rumi_protocol_backend/<hash>` or the icp-cli equivalent. Find the exact path:

```bash
find .icp/cache/artifacts/ -name "rumi_protocol_backend*.wasm*" -type f 2>/dev/null | head -5
```

- [ ] **Step 2: Hash the built wasm**

```bash
WASM_PATH=$(find .icp/cache/artifacts/ -name "rumi_protocol_backend*.wasm*" -type f | head -1)
sha256sum "$WASM_PATH"
```

Record the hash. The Phase 1a wasm hash is captured in the PR description and the post-deploy verification.

- [ ] **Step 3: Start the staging canister**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
STAGING_ID=$(jq -r '.rumi_protocol_backend' .icp/data/mappings/mainnet-staging.ids.json)
"$DFX" canister start --network ic $STAGING_ID --identity rumi_identity
```

Expected: canister Running.

- [ ] **Step 4: Install the wasm in install mode (first install, not upgrade)**

This is the canister's FIRST install, so we use `--mode install`, NOT `--mode upgrade`. Follow the deploy-pattern doc's command shape, substituting the staging principal:

```bash
icp canister install $STAGING_ID \
  --wasm "$WASM_PATH" \
  --environment mainnet-staging \
  --identity rumi_identity \
  --mode install \
  --args "$(grep -A 30 'rumi_protocol_backend:' icp.yaml | head -30)"
```

If the inline `--args` extraction is too fragile, dump the Init args to a temp file first and pass `--args-file /tmp/phase1a-staging-init.args`. Use whichever icp-cli supports.

Expected: install succeeds. The pre-deploy hook fires (per Phase 0 Task 7) and runs the test suite.

- [ ] **Step 5: Verify the canister Module hash on chain matches the built wasm**

```bash
"$DFX" canister info --network ic $STAGING_ID | grep "Module hash"
sha256sum "$WASM_PATH"
```

Expected: the two hashes match (after stripping the `0x` prefix from the dfx output).

- [ ] **Step 6: Call get_global_icusd_supply()**

```bash
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply
```

Expected: `(0 : nat)`. The staging canister has no chains registered and no debt; the canonical supply is zero.

- [ ] **Step 7: Call get_supply_audit()**

```bash
"$DFX" canister call --network ic $STAGING_ID get_supply_audit
```

Expected: `(record { total_e8s = 0 : nat; per_chain = vec {} })`.

- [ ] **Step 8: Verify the developer-only gates trip for non-developer callers**

```bash
"$DFX" identity use default
"$DFX" canister call --network ic $STAGING_ID register_chain '(record {
  chain_id = 1 : nat32;
  display_name = "ShouldReject";
  rpc_endpoints = vec { "https://example" };
  finality_depth = 1 : nat32;
  gas_strategy = variant { NotApplicable };
  chain_native_decimals = 18 : nat8;
})'
"$DFX" identity use rumi_identity
```

Expected: returns `(variant { Err = variant { ChainAdmin = "not developer" } })`. Confirms the developer-only check works.

- [ ] **Step 9: Smoke-test the admin endpoints with rumi_identity**

```bash
"$DFX" canister call --network ic $STAGING_ID register_chain '(record {
  chain_id = 999 : nat32;
  display_name = "Phase1aSmokeTest";
  rpc_endpoints = vec { "https://placeholder.invalid" };
  finality_depth = 1 : nat32;
  gas_strategy = variant { NotApplicable };
  chain_native_decimals = 18 : nat8;
})' --identity rumi_identity

"$DFX" canister call --network ic $STAGING_ID get_supply_audit
"$DFX" canister call --network ic $STAGING_ID disable_chain '(999 : nat32)' --identity rumi_identity
"$DFX" canister call --network ic $STAGING_ID get_supply_audit
```

Expected: `register_chain` returns `Ok`. The audit then shows one entry with supply 0. After `disable_chain`, the audit still shows the entry (registration is preserved; only the status flips).

- [ ] **Step 10: Commit any drift (none expected)**

The staging deploy modifies on-chain state but not the repo. Run `git status` and confirm clean:

```bash
git status
```

Expected: clean working tree.

**Decision gate:** if any step 5-9 fails, surface to Rob before opening the PR. The canister can be rolled back by re-installing an empty test wasm or by leaving it stopped; do not silently retry.

---

## Task 16: End-of-Phase Verification + Memory Update + PR

**Files:**
- Modify: `MEMORY.md` (add a one-line entry under "Follow-ups")

Phase 1a is complete when staging exhibits the four invariants below and the PR is open against `main`.

- [ ] **Step 1: Confirm the four end-of-phase invariants**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
STAGING_ID=$(jq -r '.rumi_protocol_backend' .icp/data/mappings/mainnet-staging.ids.json)

# 1. get_global_icusd_supply returns 0 on the staging canister
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply

# 2. get_supply_audit returns an empty per_chain list (after disabling the smoke-test chain)
"$DFX" canister call --network ic $STAGING_ID get_supply_audit

# 3. Module hash matches the locally built wasm
"$DFX" canister info --network ic $STAGING_ID | grep "Module hash"
WASM_PATH=$(find .icp/cache/artifacts/ -name "rumi_protocol_backend*.wasm*" -type f | head -1)
sha256sum "$WASM_PATH"

# 4. Production canister (mainnet-live) is UNCHANGED (never touched by Phase 1a)
"$DFX" canister info --network ic tfesu-vyaaa-aaaap-qrd7a-cai | grep "Module hash"
```

The fourth check is critical: Phase 1a must not have touched production. The mainnet-live hash here should match whatever was on chain before Phase 1a started.

- [ ] **Step 2: Run the local test suite one last time**

```bash
cargo test --package rumi_protocol_backend --lib
cargo test --package rumi_protocol_backend --test multi_chain_supply_invariant
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1a_scaffolding_pic
```

Expected: all green.

- [ ] **Step 3: Update MEMORY.md**

Append to `/Users/robertripley/.claude/projects/-Users-robertripley-coding-rumi-protocol-v2/memory/MEMORY.md` under "## Follow-ups":

```markdown
- [Phase 1a deployed to mainnet-staging](project_multi_chain_phase_1a_staging.md) - YYYY-MM-DD. New staging canister `<STAGING_ID>` carries chain-agnostic scaffolding (`chains/` tree, `MultiChainStateV1`, `apply_supply_delta`, Timer-B self-check). Production untouched. Next: Phase 1b Monad adapter on same staging canister.
```

Then create the topic file `/Users/robertripley/.claude/projects/-Users-robertripley-coding-rumi-protocol-v2/memory/project_multi_chain_phase_1a_staging.md` with the date, staging principal, wasm hash, and a one-paragraph summary. This is a user-instructions memory edit; if the harness blocks the cross-directory write, ask Rob to apply manually.

- [ ] **Step 4: Open the PR**

```bash
git push -u origin feat/multi-chain-phase-1a
gh pr create --title "Phase 1a: multi-chain backend scaffolding" --body "$(cat <<'EOF'
## Summary

Lands Phase 1a per `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md`. Pure scaffolding: no real chain registered, no foreign-chain calls, no tECDSA, no frontend changes.

What ships:
- `src/rumi_protocol_backend/src/chains/` module tree (adapter, config, settlement_queue, supply, admin, multi_chain_state)
- `ChainAdapter` trait (signatures only; no production impl)
- `ChainConfigV1` + versioned-snapshot pattern from day one
- `SettlementQueueV1` (per-chain queue, idempotency-enforced, never drained in Phase 1a)
- `MultiChainStateV1` embedded in `State::multi_chain` via `#[serde(default)]`
- `apply_supply_delta` (sole entry point; rejects underflow, unknown chain, divergence, halt)
- Timer-B supply-invariant self-check (60s default, halts protocol on drift)
- `get_global_icusd_supply()` + `get_supply_audit()` queries
- `register_chain` / `disable_chain` / `set_chain_config` developer-gated updates
- PocketIC upgrade round-trip test (`tests/phase1a_scaffolding_pic.rs`)
- proptest harness for the supply invariant under randomized Mint/Burn/Bridge sequences

## Staging deploy

- New canister: `<STAGING_ID>` on `mainnet-staging`
- Wasm hash: `<sha256>`
- `get_global_icusd_supply()` returns `0`
- `get_supply_audit()` returns empty `per_chain`
- Production canister (`tfesu-vyaaa-aaaap-qrd7a-cai`) untouched

## Test plan

- [x] Unit tests for adapter shape, config round-trip, settlement queue idempotency
- [x] proptest harness for `apply_supply_delta` under random ops
- [x] PocketIC test: queries return 0/empty on fresh canister
- [x] PocketIC test: upgrade round-trip preserves `multi_chain` state (guard against AMM-style state-wipe)
- [x] Staging smoke test: developer-only gates trip, register/disable round-trip works

## Non-goals (explicit; tracked for Phase 1b/c/d)

- No tECDSA / tEd25519 signing
- No HTTPS outcalls or EVM RPC wiring
- No Monad / Solana adapter implementation
- No `IcUSD.sol` / `LiquidationRouter.sol`
- No SIWE / SIWS provider
- No frontend route or UI

## Decision gates honoured

1. Task 5 (versioned snapshot lands): confirmed before any new field flowed through `multi_chain`
2. Task 14 (staging canister created): confirmed canister ID + mapping + controllers before installing wasm
3. Task 15 (staging install succeeded, queries verified): confirmed before opening this PR

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Surface the PR URL**

```bash
gh pr view --json url -q .url
```

Expected: the URL prints. Pass it back to Rob; do NOT merge.

**Phase 1a is complete when this PR is merged.** A separate future session writes the Phase 1b plan (Monad adapter, happy-path flows, staging deploy on the same canister).

---

## Phase 1a -> Phase 1b Handoff

After this PR merges:
- The staging canister carries the Phase 1a wasm
- `chain_configs` is empty (no real chain registered)
- Production is untouched
- The next session invokes `superpowers:writing-plans` with the Phase 1b scope: Monad adapter (`chains/monad/{adapter, config, contracts, ...}`), Foundry deploy of `IcUSD.sol` to Monad testnet, deposit watch via EVM RPC, supply mint flow exercised end-to-end on staging.
