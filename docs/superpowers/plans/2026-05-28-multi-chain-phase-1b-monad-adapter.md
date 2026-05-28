# Phase 1b: Monad Adapter + Happy Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the first real chain (Monad testnet) into the Phase 1a multi-chain scaffolding so a Monad-collateral vault can open, mint icUSD on Monad, repay (burn), and close, end-to-end, with every cross-chain transaction signed and submitted by the staging canister via tECDSA. The production canister (`tfesu-vyaaa-aaaap-qrd7a-cai`) is never touched.

**Architecture:** A new `chains/monad/` subtree implements the `ChainAdapter` trait from Phase 1a. tECDSA (`ecdsa_public_key` + `sign_with_ecdsa` on the management canister) derives per-user collateral custody addresses and a per-chain settlement (minter) address. EVM reads and the final `eth_sendRawTransaction` go through the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`) using `RpcServices::Custom` (Monad is not a built-in EVM-RPC chain). Two new timers run the async loops: an inbound **observer** (deposit watch + `Burn` event watch) and an outbound **settlement worker (Timer D)** that drains `multi_chain.settlement_queues[Monad]`. Supply accounting follows **Design B (confirmed-supply)**: `chain_supplies` and vault debt only move when an on-chain mint/burn is OBSERVED at finality, so `apply_supply_delta` never needs a compensating reversal. Monad vaults live in a new `chain_vaults` map inside a versioned `MultiChainStateV2` (the core ICP-native `Vault` struct is untouched). The icUSD token on Monad is an OpenZeppelin ERC-20 (`IcUSD.sol`, 8 decimals to match e8s 1:1) where the canister's settlement address holds `MINTER_ROLE`.

**Tech Stack:**
- Rust 2021 (workspace resolver "2"), ic-cdk 0.12, candid 0.10, ic-stable-structures 0.6, rust_decimal 1.32, async-trait 0.1
- New backend crates: `k256` (secp256k1 pubkey recovery/decompression), `sha3` (keccak256 for EVM addresses + tx hashing), `alloy-rlp` or `rlp` (EIP-1559 tx encoding), `evm_rpc_client` (typed EVM RPC canister client). Versions verified in Task 1.
- tECDSA via the management canister (`aaaaa-aa`): `ecdsa_public_key`, `sign_with_ecdsa`. Key name `test_key_1` on staging (mainnet test key), `key_1` for the eventual production rollout (Phase 2).
- EVM RPC canister `7hfb6-caaaa-aaaar-qadga-cai` for Monad RPC (custom JSON-RPC services).
- Foundry (`forge`) for the Solidity build/test/deploy. OpenZeppelin Contracts (ERC20 + AccessControl).
- pocket-ic 6.0 for the chain-agnostic integration test; a new `monad_rpc_mock` canister mocks the EVM RPC surface (precedent: `src/xrc_demo/xrc_mock`).
- icp-cli + `icp canister install --environment mainnet-staging` for the staging deploy (per `docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md`).

**Reference docs (read before starting):**
- Spec: `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md` (Sections 1, 2 flows 1-9, 3 hardening, "Sub-phase 1b")
- Phase 1a plan (structural template): `docs/superpowers/plans/2026-05-27-multi-chain-phase-1a-backend-scaffolding.md`
- Deploy pattern (locked, do not deviate): `docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md`
- Skills to load at session start: `evm-rpc`, `https-outcalls`, `vetkd` (tECDSA patterns), `canister-security`, `icp-cli`, `cycles-management`
- AMM state-wipe incident (the cautionary tale): MEMORY.md -> `project_amm_state_wipe_2026_05_18.md`
- Phase 1a staging deploy notes (tone/detail reference): MEMORY.md -> `project_multi_chain_phase_1a_staging_deployed.md`

**Branch:** Work on `feat/multi-chain-phase-1b` (create from main after this plan PR merges). The current branch (`feat/multi-chain-phase-1b-plan`) carries this plan only.

**World state at start (Phase 1a complete, PR #209 merged at `83ea1bed`):**
- Staging canister `kvg63-wiaaa-aaaao-bbabq-cai`: STOPPED, Phase 1a wasm hash `0xe60c8c13a146d0ca272a269e97bdf6c44e021f604272c8f6814e6c90a7fa649d`, ~3.37T cycles, controller rumi_identity (`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`). Phase 1b's first deploy step starts it.
- Production canister `tfesu-vyaaa-aaaap-qrd7a-cai`: hash `0xcc9bb33f389ca10b8859ecd59d9149e753f4aa4c1bc596827406a9e71c34304c`. MUST stay unchanged. Phase 1b never deploys to `mainnet-live`.
- Smoke-test debris: chain id `999` ("Phase1aSmokeTest") is registered-but-disabled on staging. Task 14 ships `delete_chain`; Task 22 uses it to clean the entry.

**What Phase 1b is NOT (deferred to 1c / 1d / 2 / 3+):**
- No liquidation routing, no keeper market, no SP backstop, no DEX swap (all Phase 1c)
- No `LiquidationRouter.sol` (Phase 1c; Phase 1b ships `IcUSD.sol` only)
- No user-facing cross-chain bridge primitive (Phase 1c per spec Section 4)
- No frontend route, MetaMask connector, or SIWE login (all Phase 1d)
- No production deploy (Phase 2)
- No Solana / Ethereum / L2 (Phase 3+)

---

## Decision Gates Summary

Four gates require Rob's explicit "go":

1. **After Task 1** (architecture lock): confirm the five load-bearing decisions before any real code flows: (a) Design B confirmed-supply timing, (b) parallel `chain_vaults` map vs unifying the core `Vault` struct, (c) `onlyRole(MINTER_ROLE)` canister-as-sender mint model vs ecrecover meta-tx, (d) native MON as Phase 1b collateral with a developer-set manual MON/USD price, (e) `test_key_1` tECDSA key name on staging. Each has a recommendation in Task 1.
2. **After Task 3** (state migration): confirm `MultiChainStateV1 -> MultiChainStateV2` round-trips through a PocketIC upgrade before any new field is used. This is the AMM-state-wipe fence, mirroring Phase 1a's Task 5 gate.
3. **After Task 22** (staging configured): confirm the deployed `IcUSD.sol` address, the granted `MINTER_ROLE`, `register_chain(10143)` success, and the canister's MON hot-wallet funding before running the first real mint.
4. **After Task 23** (manual integration): confirm the full deposit -> mint -> burn -> withdraw cycle succeeded on staging and the observed cycle/MON burn is acceptable before opening the merge PR.

Every other task uses local-only verification and routine commits.

---

## File Structure

New files this phase creates (all under `src/rumi_protocol_backend/` unless noted):

| File | Responsibility |
|---|---|
| `src/chains/monad/mod.rs` | Module entry, re-exports, `CONTRACTS` pointer |
| `src/chains/monad/config.rs` | Monad defaults (chain id 10143, finality depth, gas strategy, RPC endpoints, key name) |
| `src/chains/monad/tecdsa.rs` | Custody + settlement address derivation (pubkey -> EVM address), mgmt-canister calls |
| `src/chains/monad/evm_rpc.rs` | Thin typed wrapper over the EVM RPC canister |
| `src/chains/monad/tx.rs` | EIP-1559 tx build, RLP encode, keccak hash, tECDSA sign, assemble signed tx |
| `src/chains/monad/adapter.rs` | `ChainAdapter` impl for Monad (the six methods) |
| `src/chains/monad/deposit_watch.rs` | Inbound observer: collateral deposits + `Burn` events |
| `src/chains/monad/settlement.rs` | Outbound Timer-D worker: drain queue, sign, submit, confirm |
| `src/chains/monad/chain_vault.rs` | `ChainVaultV1` (Monad vault record) + pure CR/health helpers |
| `foundry/` | Foundry project root (forge config, OZ deps) |
| `foundry/src/IcUSD.sol` | Canister-minted ERC-20 (8 decimals) |
| `foundry/test/IcUSD.t.sol` | Foundry test suite |
| `foundry/script/DeployIcUSD.s.sol` | Deploy script for Monad testnet |
| `src/monad_rpc_mock/` | Mock EVM RPC canister for PocketIC (precedent: `src/xrc_demo/xrc_mock`) |
| `tests/phase1b_monad_happy_path_pic.rs` | PocketIC happy-path integration test |

Files modified: `src/chains/mod.rs`, `src/chains/multi_chain_state.rs`, `src/chains/supply.rs` (migration), `src/event.rs`, `src/main.rs`, `src/state.rs` (only if a replay arm needs extending), `Cargo.toml`, `rumi_protocol_backend.did`, `declarations/rumi_protocol_backend/`, `icp.yaml`, root `Cargo.toml` (workspace members for `monad_rpc_mock`).

---

## Task 1: Branch, Monad Module Scaffold, and Cargo Dependencies

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/monad/mod.rs`
- Create stubs: `src/rumi_protocol_backend/src/chains/monad/{config,tecdsa,evm_rpc,tx,adapter,deposit_watch,settlement,chain_vault}.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs`
- Modify: `src/rumi_protocol_backend/Cargo.toml`
- Modify: root `Cargo.lock`

This task only adds the module tree and pins dependencies. No behaviour change beyond the new (unused) deps. The wasm must still build for `wasm32-unknown-unknown`.

- [ ] **Step 1: Branch off main**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
git fetch origin
git checkout main
git pull origin main
git checkout -b feat/multi-chain-phase-1b
git branch --show-current
```

Expected: `feat/multi-chain-phase-1b`. Verify with `git branch --show-current` (a failed checkout silently leaves you on main).

- [ ] **Step 2: Load the supporting skills**

Invoke the `evm-rpc`, `https-outcalls`, and `vetkd` skills now and skim them. They carry the canonical `evm_rpc_client` usage, the HTTPS-outcall cycle-cost model, and the tECDSA call shapes. Confirm the current `evm_rpc_client` crate name + version and the management-canister tECDSA method signatures (ic-cdk 0.12 exposes them under `ic_cdk::api::management_canister::ecdsa`). Record the verified crate versions; you will paste them in Step 5.

- [ ] **Step 3: Create the Monad module entry**

Write `src/rumi_protocol_backend/src/chains/monad/mod.rs`:

```rust
//! Monad adapter (Phase 1b). First real chain integration.
//!
//! Implements the Phase 1a `ChainAdapter` trait against Monad testnet
//! (chain id 10143) using tECDSA for signing and the EVM RPC canister for
//! reads and `eth_sendRawTransaction`. Supply accounting is confirmed-supply
//! (Design B): `chain_supplies` and vault debt move only when an on-chain
//! mint/burn is observed at finality.

pub mod adapter;
pub mod chain_vault;
pub mod config;
pub mod deposit_watch;
pub mod evm_rpc;
pub mod settlement;
pub mod tecdsa;
pub mod tx;

pub use adapter::MonadAdapter;
pub use chain_vault::{ChainVaultStatus, ChainVaultV1};
pub use config::{monad_default_config, MonadContracts, CONTRACTS, MONAD_CHAIN_ID};
```

- [ ] **Step 4: Write the eight stub files**

So the build passes before later tasks fill them in, write minimal stubs. Each is a doc comment plus the smallest item needed by `mod.rs`.

`config.rs`:
```rust
//! Monad config defaults. Real impl in Task 2.
use super::super::config::ChainId;

pub const MONAD_CHAIN_ID: ChainId = ChainId(10143);

#[derive(Clone, Debug, Default)]
pub struct MonadContracts {
    pub icusd: Option<String>,
}

pub static CONTRACTS: MonadContracts = MonadContracts { icusd: None };

pub fn monad_default_config() {}
```

`chain_vault.rs`:
```rust
//! Monad vault record. Real impl in Task 3.
#[derive(Clone, Debug)]
pub enum ChainVaultStatus { MintPending, Open, Closing, Closed }

#[derive(Clone, Debug)]
pub struct ChainVaultV1 {
    pub vault_id: u64,
}
```

`tecdsa.rs`, `evm_rpc.rs`, `tx.rs`, `adapter.rs`, `deposit_watch.rs`, `settlement.rs`: each a single doc comment line, e.g.:
```rust
//! Placeholder. Real impl in a later Task.
```

For `adapter.rs`, add an empty struct so `mod.rs`'s `pub use adapter::MonadAdapter;` resolves:
```rust
//! MonadAdapter placeholder. Real impl in Task 8.
pub struct MonadAdapter;
```

- [ ] **Step 5: Wire the monad module + add dependencies**

Edit `src/rumi_protocol_backend/src/chains/mod.rs`. Add after the existing `pub mod supply;` line (keep the existing ordering, monad after the shared modules):

```rust
pub mod monad;
```

Edit `src/rumi_protocol_backend/Cargo.toml`. Under `[dependencies]`, add (use the versions verified in Step 2; the values below are the expected current majors):

```toml
k256 = { version = "0.13", default-features = false, features = ["ecdsa", "arithmetic"] }
sha3 = "0.10"
alloy-rlp = "0.3"
evm_rpc_client = "1"
```

Note: do NOT add any `@rollup/rollup-darwin-*` style platform-pinned dep anywhere; that rule is for npm, not cargo, but the spirit (no platform-pinned native deps) holds. If `evm_rpc_client` is only published as a git dependency at execution time, mirror the existing IC git-dep pattern (`{ git = "https://github.com/Rumi-Protocol/ic", rev = "..." }` or the upstream DFINITY repo) and record the rev.

- [ ] **Step 6: Build for wasm to confirm deps resolve**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release 2>&1 | tail -20
```

Expected: build succeeds. If `k256` or `sha3` pull in a non-wasm-compatible default feature, disable default features (k256 already has `default-features = false` above). Resolve any `getrandom` wasm error by ensuring no crate enables `getrandom/js` or `std` rng (canisters have no RNG; we only use these crates for hashing + pubkey math, not key generation).

- [ ] **Step 7: Run the existing suite to confirm zero regression**

```bash
cargo test --package rumi_protocol_backend --lib 2>&1 | tail -15
```

Expected: every existing test passes.

- [ ] **Step 8: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/ src/rumi_protocol_backend/Cargo.toml Cargo.lock
git commit -m "feat(monad): scaffold chains/monad module tree + EVM/tECDSA deps

Phase 1b Task 1. Module entry + eight stub files; k256/sha3/alloy-rlp/
evm_rpc_client added. No behaviour change."
```

**Decision gate (Gate 1 - architecture lock).** Pause here. Walk Rob through the five decisions and get explicit "go" before Task 2:

1. **Supply timing = Design B (confirmed-supply).** `chain_supplies` + vault debt move only when the on-chain mint/burn is observed at finality, never at enqueue. Rationale: the spec Section 3 error table says "debt and supply not modified until tx confirms," and the scope says "observed mint = supply increase." This makes the supply invariant trivially true at all times and removes the need for compensating reversals on a failed/ reverted tx. Recommended.
2. **Monad vaults in a parallel `chain_vaults` map (in `MultiChainStateV2`), not by adding `collateral_chain` to the core `Vault` struct.** Rationale: the core `Vault` struct is persisted and heavily used; mutating its shape mid-stream risks the AMM state-wipe failure mode on the eventual production upgrade. Keeping Monad vaults inside the already-versioned `multi_chain` state confines the blast radius. Unifying into one `Vault` model (lock #1's long-term goal) becomes a deliberate Phase 2 task. Recommended for 1b.
3. **Mint authorization = `onlyRole(MINTER_ROLE)` with the canister settlement address as `msg.sender`.** The canister already signs and submits the mint tx, so it is already the authenticated sender; `onlyRole` is the standard ck-asset minter pattern and avoids in-contract nonce/replay management. The ecrecover meta-tx alternative (anyone submits, contract verifies a canister signature) is documented as the fallback but adds complexity for no 1b benefit. Recommended: `onlyRole`.
4. **Collateral = native MON with a developer-set manual MON/USD price for 1b.** Pyth-on-Monad-testnet feeds may not exist; a gated manual price unblocks the happy path. Pyth integration is a 1c+ follow-up. Recommended.
5. **tECDSA key = `test_key_1` on staging.** It is the mainnet test key, available on the staging subnet. Production (Phase 2) uses `key_1`. The derived addresses differ per key, so `IcUSD.sol`'s minter address must be derived with the same key used at runtime (captured in Task 22's ordering). Recommended.

---

## Task 2: Monad Config Defaults

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/config.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_config.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs` (wire the test module)

Define the Monad-specific defaults: chain id 10143, finality depth (start at 1, the spec's Monad guess; tunable per registration), gas strategy `EvmEip1559`, candidate RPC endpoints, the tECDSA key name, and the `CONTRACTS.icusd` pointer (filled at deploy time in Task 22). The defaults feed `register_chain` in Task 22.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_config.rs`:

```rust
use super::config::{
    monad_default_register_arg, monad_ecdsa_key_name, MONAD_CHAIN_ID,
    MONAD_ICUSD_DECIMALS,
};
use crate::chains::config::{ChainId, GasStrategy};

#[test]
fn chain_id_is_monad_testnet() {
    assert_eq!(MONAD_CHAIN_ID, ChainId(10143));
}

#[test]
fn default_register_arg_uses_eip1559_and_nonempty_rpc() {
    let arg = monad_default_register_arg();
    assert_eq!(arg.chain_id, ChainId(10143));
    assert!(!arg.rpc_endpoints.is_empty());
    assert!(matches!(arg.gas_strategy, GasStrategy::EvmEip1559 { .. }));
    assert_eq!(arg.chain_native_decimals, 18); // MON has 18 decimals
    assert!(arg.finality_depth >= 1);
}

#[test]
fn icusd_decimals_match_e8s() {
    // IcUSD.sol uses 8 decimals so on-chain amount == e8s 1:1.
    assert_eq!(MONAD_ICUSD_DECIMALS, 8);
}

#[test]
fn key_name_is_test_key_on_staging_default() {
    assert_eq!(monad_ecdsa_key_name(), "test_key_1");
}
```

Wire in `src/rumi_protocol_backend/src/chains/monad/mod.rs`:
```rust
#[cfg(test)]
mod tests_config;
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_config 2>&1 | tail -10
```

Expected: compile error on missing items.

- [ ] **Step 3: Implement config.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/config.rs` with:

```rust
//! Monad testnet configuration defaults.
//!
//! These feed `register_chain` (Task 22). `CONTRACTS.icusd` is None until
//! the Foundry deploy (Task 21/22) writes the deployed address via
//! `set_chain_config` -> stored in the chain's `ChainConfigV1` (the contract
//! address is carried in the config's `rpc_endpoints`-adjacent fields is wrong;
//! we store it in chain_vault-side state, see note below).

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Monad testnet chain id (verify current value at execution; 10143 is the
/// published Monad testnet id as of 2026-05).
pub const MONAD_CHAIN_ID: ChainId = ChainId(10143);

/// IcUSD.sol uses 8 decimals so 1 on-chain base unit == 1 e8s.
pub const MONAD_ICUSD_DECIMALS: u8 = 8;

/// MON native gas asset decimals.
pub const MON_NATIVE_DECIMALS: u8 = 18;

/// Candidate Monad testnet RPC endpoints. The EVM RPC canister fans out to
/// these via `RpcServices::Custom`. VERIFY these URLs are live at execution
/// time and pick 2-3 with adequate rate limits (spec calls for multi-provider).
pub fn monad_rpc_endpoints() -> Vec<String> {
    vec![
        "https://testnet-rpc.monad.xyz".to_string(),
        // Add 1-2 third-party Monad-testnet endpoints (e.g. an Ankr/dRPC
        // endpoint) once confirmed available. Multi-provider = consensus.
    ]
}

/// tECDSA key name. `test_key_1` on staging (mainnet test key); switch to
/// `key_1` for the Phase 2 production rollout. The derived addresses differ
/// per key, so the IcUSD.sol minter must be derived with this exact key.
pub fn monad_ecdsa_key_name() -> String {
    "test_key_1".to_string()
}

/// Default registration payload for Monad testnet.
pub fn monad_default_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: MONAD_CHAIN_ID,
        display_name: "MonadTestnet".to_string(),
        rpc_endpoints: monad_rpc_endpoints(),
        // Spec open question: Monad single-slot finality likely means depth 1.
        // Start at 1, verify on testnet (Task 23), bump via set_chain_config if
        // reorgs are observed.
        finality_depth: 1,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 2,
            max_fee_gwei_ceiling: 500,
        },
        chain_native_decimals: MON_NATIVE_DECIMALS,
    }
}
```

Update `mod.rs` re-exports to match the real names (remove the stub `MonadContracts`/`CONTRACTS`; the deployed icUSD address is carried in `ChainVaultV1`-side state per Task 3, not a static):

```rust
pub use config::{monad_default_register_arg, monad_ecdsa_key_name, MONAD_CHAIN_ID, MONAD_ICUSD_DECIMALS};
```

Remove the now-stale `pub use config::{... MonadContracts, CONTRACTS ...}` line and the `MonadContracts`/`CONTRACTS` stub from `config.rs`. The IcUSD contract address is stored at runtime in `MultiChainStateV2.chain_contracts` (Task 3) so it survives upgrades; a `static` could not.

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_config
```

Expected: all four tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/monad/
git commit -m "feat(monad): config defaults (chain id 10143, EIP-1559, test_key_1)

Phase 1b Task 2. Default register arg + key name + decimals constants.
icUSD decimals = 8 to match e8s 1:1."
```

---

## Task 3: MultiChainStateV2 Migration (chain_vaults, contracts, runtime caches)

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs`
- Modify: `src/rumi_protocol_backend/src/chains/supply.rs` (add `migrate_multi_chain_state`)
- Modify: `src/rumi_protocol_backend/src/main.rs` (call migration in `post_upgrade`)
- Create: `src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_chain_vault.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs` (wire test modules)

This is the load-bearing upgrade-safety task. Per the AMM state-wipe lesson, adding fields to the persisted `multi_chain` root requires a NEW version (`MultiChainStateV2`) with a `From<MultiChainStateV1>` migration, NOT an in-place edit of V1. V2 adds: `chain_vaults` (Monad vault records), `chain_contracts` (deployed icUSD address per chain), `manual_prices` (developer-set collateral prices), `last_observed_block` (per-chain observer cursor), and `hot_wallet_balance_e18` (cached MON gas balance per chain).

- [ ] **Step 1: Write the failing tests FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_chain_vault.rs`:

```rust
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use candid::{Decode, Encode};

#[test]
fn chain_vault_round_trips_via_candid() {
    let v = ChainVaultV1 {
        vault_id: 42,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xabc0000000000000000000000000000000000001".into(),
        collateral_amount_e18: 5_000_000_000_000_000_000, // 5 MON
        debt_e8s: 0,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 10_000_000_000, // 100 icUSD pending
        status: ChainVaultStatus::MintPending,
        opened_at_ns: 1_700_000_000_000_000_000,
    };
    let bytes = Encode!(&v).expect("encode");
    let back: ChainVaultV1 = Decode!(&bytes, ChainVaultV1).expect("decode");
    assert_eq!(back.vault_id, 42);
    assert!(matches!(back.status, ChainVaultStatus::MintPending));
}

#[test]
fn chain_vault_round_trips_via_cbor() {
    // The whole State persists via ciborium CBOR; a field that survives Candid
    // but trips CBOR would still wipe state on upgrade.
    let v = ChainVaultV1 {
        vault_id: 1,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xa".into(),
        collateral_amount_e18: 1,
        debt_e8s: 2,
        mint_recipient: "0xb".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
    };
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v, &mut buf).expect("cbor encode");
    let back: ChainVaultV1 = ciborium::de::from_reader(buf.as_slice()).expect("cbor decode");
    assert_eq!(back.debt_e8s, 2);
}
```

Create `src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs`:

```rust
use super::multi_chain_state::{MultiChainState, MultiChainStateV1, MultiChainStateV2};
use super::config::ChainId;
use super::supply::migrate_multi_chain_state;

#[test]
fn v2_default_is_empty() {
    let s = MultiChainStateV2::default();
    assert!(s.chain_configs.is_empty());
    assert!(s.chain_supplies.is_empty());
    assert!(s.chain_vaults.is_empty());
    assert!(s.chain_contracts.is_empty());
    assert!(s.manual_prices.is_empty());
    assert!(s.last_observed_block.is_empty());
    assert!(s.hot_wallet_balance_e18.is_empty());
    assert_eq!(s.total_supply_all_chains_e8s(), 0u128);
}

#[test]
fn migration_preserves_v1_fields_and_defaults_new_ones() {
    let mut v1 = MultiChainStateV1::default();
    v1.chain_supplies.insert(ChainId(10143), 12345);
    v1.invariant_halted = true;
    let v2 = migrate_multi_chain_state(v1);
    // Preserved:
    assert_eq!(v2.chain_supplies.get(&ChainId(10143)), Some(&12345u128));
    assert!(v2.invariant_halted);
    // New fields default to empty:
    assert!(v2.chain_vaults.is_empty());
    assert!(v2.chain_contracts.is_empty());
}

#[test]
fn active_alias_points_at_v2() {
    fn _check(x: MultiChainState) -> MultiChainStateV2 { x }
}
```

Wire both in `src/rumi_protocol_backend/src/chains/mod.rs`:
```rust
#[cfg(test)]
mod tests_multi_chain_state_v2;
```
and in `src/rumi_protocol_backend/src/chains/monad/mod.rs`:
```rust
#[cfg(test)]
mod tests_chain_vault;
```

- [ ] **Step 2: Run to confirm compile failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_multi_chain_state_v2 2>&1 | tail -10
```

- [ ] **Step 3: Implement ChainVaultV1**

Replace `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs` with:

```rust
//! Monad (and future foreign-chain) vault record.
//!
//! Lives in `MultiChainStateV2.chain_vaults`, keyed by the globally-unique
//! u64 vault_id. The core ICP-native `Vault` struct is untouched in Phase 1b;
//! unifying the two models is a deliberate Phase 2 task (see lock #1).
//!
//! Design B (confirmed-supply): `debt_e8s` is the CONFIRMED debt. While a mint
//! is in flight, the intended amount lives in `pending_mint_e8s` and does NOT
//! count toward `total_debt` or `chain_supplies` until the on-chain mint is
//! observed at finality (settlement worker, Task 10).

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ChainVaultStatus {
    /// Vault created; mint enqueued but not yet confirmed on-chain.
    MintPending,
    /// Mint confirmed; debt counted; vault live.
    Open,
    /// Withdrawal/close enqueued; awaiting on-chain settlement.
    Closing,
    /// Fully repaid + collateral returned.
    Closed,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct ChainVaultV1 {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_chain: ChainId,
    /// tECDSA-derived custody address holding this vault's collateral.
    pub custody_address: String,
    /// Confirmed collateral balance, in the chain-native unit (MON e18).
    pub collateral_amount_e18: u128,
    /// CONFIRMED debt (e8s). Does not include `pending_mint_e8s`.
    pub debt_e8s: u128,
    /// Address that receives the minted icUSD on Monad.
    pub mint_recipient: String,
    /// Mint amount in flight, not yet confirmed (e8s). Moves into debt on
    /// confirmation; cleared on permanent mint failure.
    pub pending_mint_e8s: u128,
    pub status: ChainVaultStatus,
    pub opened_at_ns: u64,
}
```

- [ ] **Step 4: Add MultiChainStateV2 + migration**

Edit `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`. Keep `MultiChainStateV1` EXACTLY as it is (do not touch its fields). Append `MultiChainStateV2`, repoint the alias, and add the `total_supply_all_chains_e8s` helper to V2:

```rust
use super::monad::chain_vault::ChainVaultV1;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default)]
pub struct MultiChainStateV2 {
    // --- carried verbatim from V1 ---
    pub chain_configs: BTreeMap<ChainId, ChainConfigV1>,
    pub chain_supplies: BTreeMap<ChainId, u128>,
    pub settlement_queues: BTreeMap<ChainId, SettlementQueueV1>,
    pub invariant_halted: bool,
    // --- new in V2 ---
    /// Foreign-chain vault records, keyed by global vault_id.
    pub chain_vaults: BTreeMap<u64, ChainVaultV1>,
    /// Deployed token contract address per chain (icUSD ERC-20). Filled at
    /// deploy time via set_chain_contract (Task 14).
    pub chain_contracts: BTreeMap<ChainId, String>,
    /// Developer-set manual prices (USD e8s) keyed by (chain, asset symbol).
    /// Phase 1b uses this for MON/USD until Pyth lands (1c+).
    pub manual_prices: BTreeMap<(ChainId, String), u64>,
    /// Per-chain observer cursor: last block fully scanned for events.
    pub last_observed_block: BTreeMap<ChainId, u64>,
    /// Cached settlement-address MON balance (e18) per chain. Refreshed by the
    /// observer; read by the settlement worker's gas gate.
    pub hot_wallet_balance_e18: BTreeMap<ChainId, u128>,
}

impl MultiChainStateV2 {
    pub fn total_supply_all_chains_e8s(&self) -> u128 {
        self.chain_supplies.values().copied().sum()
    }
}

/// Active alias. Was V1 in Phase 1a; now V2. Future field additions go via V3.
pub type MultiChainState = MultiChainStateV2;
```

Remove the old `pub type MultiChainState = MultiChainStateV1;` line. Keep the `impl MultiChainStateV1 { total_supply_all_chains_e8s }` so the migration test and any V1-typed code still compile.

- [ ] **Step 5: Add the migration function**

Edit `src/rumi_protocol_backend/src/chains/supply.rs`. Add:

```rust
use super::multi_chain_state::{MultiChainStateV1, MultiChainStateV2};

/// Migrate the Phase 1a `MultiChainStateV1` snapshot to `MultiChainStateV2`.
/// Carries every V1 field verbatim; defaults the V2 additions to empty. Called
/// from `post_upgrade` after the State is decoded.
pub fn migrate_multi_chain_state(v1: MultiChainStateV1) -> MultiChainStateV2 {
    MultiChainStateV2 {
        chain_configs: v1.chain_configs,
        chain_supplies: v1.chain_supplies,
        settlement_queues: v1.settlement_queues,
        invariant_halted: v1.invariant_halted,
        chain_vaults: Default::default(),
        chain_contracts: Default::default(),
        manual_prices: Default::default(),
        last_observed_block: Default::default(),
        hot_wallet_balance_e18: Default::default(),
    }
}
```

Note on how the migration actually runs across the upgrade: because `State::multi_chain` now has type `MultiChainState = MultiChainStateV2` and carries `#[serde(default)]`, a Phase 1a snapshot (which serialized a `MultiChainStateV1`-shaped value) decodes into `MultiChainStateV2` field-by-field. The four V1 fields share names + types with V2, so ciborium maps them straight across; the five new fields hit `#[serde(default)]` and come up empty. This means the in-place decode IS the migration for the V1->V2 step (the explicit `migrate_multi_chain_state` is exercised by the unit test and is the template for the next version bump where types diverge). Verify this with the PocketIC round-trip in Step 7. If the decode ever loses the V1 `chain_supplies` content, that is the AMM-state-wipe signature: STOP.

- [ ] **Step 6: Confirm post_upgrade is migration-ready**

Read `src/rumi_protocol_backend/src/main.rs` `post_upgrade`. Confirm the State decode path keeps `#[serde(default)]` on `multi_chain` (it does, from Phase 1a Task 5). No code change is required for the V1->V2 in-place decode. Add a one-line log in `post_upgrade` after state restore so the upgrade leaves a breadcrumb:

```rust
log!(INFO, "[post_upgrade] multi_chain: {} chains, {} chain_vaults",
    read_state(|s| s.multi_chain.chain_configs.len()),
    read_state(|s| s.multi_chain.chain_vaults.len()));
```

- [ ] **Step 7: Build wasm + run the V2 + chain_vault tests**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::tests_multi_chain_state_v2
cargo test --package rumi_protocol_backend --lib chains::monad::tests_chain_vault
cargo test --package rumi_protocol_backend --lib
```

Expected: all green. Then run the Phase 1a PocketIC upgrade round-trip, which now exercises a real V1->V2 decode (the staging canister carries V1 state shape; this test rehearses that exact upgrade):

```bash
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1a_scaffolding_pic
```

Expected: `phase1a_state_survives_upgrade` passes. If it fails, the V1->V2 decode lost state: do not proceed.

- [ ] **Step 8: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/ src/rumi_protocol_backend/src/main.rs
git commit -m "feat(monad): MultiChainStateV2 (chain_vaults + contracts + caches)

Phase 1b Task 3. V1->V2 via #[serde(default)] in-place decode + explicit
migrate_multi_chain_state template. PocketIC upgrade round-trip green."
```

**Decision gate (Gate 2 - state migration).** Pause. Walk Rob through the V1->V2 shape, confirm the PocketIC upgrade round-trip is green, and confirm the `#[serde(default)]` story before any task writes to the new fields. This is the AMM-state-wipe fence.

---

## Task 4: Monad Event Variants

**Files:**
- Modify: `src/rumi_protocol_backend/src/event.rs`
- Modify: `src/rumi_protocol_backend/src/tests.rs` (round-trip test)

Define every event the later tasks emit, all upfront, append-only (so historical CBOR decodes unchanged). Per the spec's "no silent drop" rule, every cross-chain state transition emits an event for the on-chain audit log.

- [ ] **Step 1: Write the failing test FIRST**

Append to `src/rumi_protocol_backend/src/tests.rs`:

```rust
#[test]
fn monad_event_variants_round_trip_via_candid() {
    use candid::{Decode, Encode};
    use crate::event::Event;
    use crate::chains::config::ChainId;

    let events = vec![
        Event::DepositObserved {
            chain_id: ChainId(10143), vault_id: 1,
            custody_address: "0xa".into(), amount_e18: 5, tx_hash: "0xh".into(),
            block_number: 100, timestamp: 1,
        },
        Event::ChainMintSubmitted {
            chain_id: ChainId(10143), vault_id: 1, op_id: 0,
            recipient: "0xr".into(), amount_e8s: 10, tx_hash: "0xs".into(), timestamp: 2,
        },
        Event::ChainMintConfirmed {
            chain_id: ChainId(10143), vault_id: 1, op_id: 0,
            amount_e8s: 10, tx_hash: "0xs".into(), block_number: 102, timestamp: 3,
        },
        Event::ChainBurnObserved {
            chain_id: ChainId(10143), vault_id: 1,
            amount_e8s: 4, tx_hash: "0xb".into(), block_number: 110, timestamp: 4,
        },
        Event::WithdrawalSigned {
            chain_id: ChainId(10143), vault_id: 1, op_id: 1,
            recipient: "0xw".into(), amount_e18: 5, tx_hash: "0xt".into(), timestamp: 5,
        },
        Event::ChainSettlementFailed {
            chain_id: ChainId(10143), op_id: 1, reason: "reverted".into(), timestamp: 6,
        },
        Event::ChainReorgDetected {
            chain_id: ChainId(10143), observed_block: 100, reorg_depth: 5, timestamp: 7,
        },
        Event::ChainHotWalletLow {
            chain_id: ChainId(10143), balance_e18: 1, threshold_e18: 100, timestamp: 8,
        },
    ];
    for e in events {
        let bytes = Encode!(&e).expect("encode");
        let _: Event = Decode!(&bytes, Event).expect("decode");
    }
}
```

```bash
cargo test --package rumi_protocol_backend --lib monad_event_variants_round_trip 2>&1 | tail -10
```

Expected: compile failure on missing variants.

- [ ] **Step 2: Append the event variants**

Edit `src/rumi_protocol_backend/src/event.rs`. After the Phase 1a `SupplyInvariantSelfCheckFailed` variant (keep append-only ordering), add:

```rust
    // Phase 1b: Monad (and future foreign-chain) audit trail.
    #[serde(rename = "deposit_observed")]
    DepositObserved {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        custody_address: String,
        amount_e18: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_mint_submitted")]
    ChainMintSubmitted {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        recipient: String,
        amount_e8s: u128,
        tx_hash: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_mint_confirmed")]
    ChainMintConfirmed {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        amount_e8s: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_burn_observed")]
    ChainBurnObserved {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        amount_e8s: u128,
        tx_hash: String,
        block_number: u64,
        timestamp: u64,
    },
    #[serde(rename = "withdrawal_signed")]
    WithdrawalSigned {
        chain_id: crate::chains::config::ChainId,
        vault_id: u64,
        op_id: u64,
        recipient: String,
        amount_e18: u128,
        tx_hash: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_settlement_failed")]
    ChainSettlementFailed {
        chain_id: crate::chains::config::ChainId,
        op_id: u64,
        reason: String,
        timestamp: u64,
    },
    #[serde(rename = "chain_reorg_detected")]
    ChainReorgDetected {
        chain_id: crate::chains::config::ChainId,
        observed_block: u64,
        reorg_depth: u64,
        timestamp: u64,
    },
    #[serde(rename = "chain_hot_wallet_low")]
    ChainHotWalletLow {
        chain_id: crate::chains::config::ChainId,
        balance_e18: u128,
        threshold_e18: u128,
        timestamp: u64,
    },
```

- [ ] **Step 3: Add no-op replay arms**

Build and let the compiler point at every non-exhaustive `match` on `Event` (there are two in `event.rs` around the Phase 1a arms: `is_vault_related`-style and the replay sink). Add the eight new variants to each as no-ops, mirroring the Phase 1a chain-admin arms. The `DepositObserved`/`ChainMintConfirmed`/`ChainBurnObserved`/`WithdrawalSigned` variants carry a `vault_id`; if `is_vault_related` should surface them in vault history, return `vault_id == filter_vault_id` for those four (so the explorer can show foreign-chain vault activity). For the others return `false`.

```bash
cargo build --package rumi_protocol_backend 2>&1 | tail -20
```

Add arms until the build is clean.

- [ ] **Step 4: Run the test**

```bash
cargo test --package rumi_protocol_backend --lib monad_event_variants_round_trip
```

Expected: passes.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/tests.rs
git commit -m "feat(monad): eight Phase 1b event variants (append-only)

Phase 1b Task 4. DepositObserved, ChainMint{Submitted,Confirmed},
ChainBurnObserved, WithdrawalSigned, ChainSettlementFailed,
ChainReorgDetected, ChainHotWalletLow. Replay arms wired."
```

---

## Task 5: tECDSA Address Derivation

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/tecdsa.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_tecdsa.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

Derive deterministic EVM addresses: per-user collateral custody addresses (path `[chain_id_le, principal_bytes, nonce_le]`) and one per-chain settlement (minter) address (path `[chain_id_le, b"settlement"]`). The pure step is pubkey -> EVM address (keccak256 of the uncompressed secp256k1 pubkey, last 20 bytes). The async step calls `ecdsa_public_key` on the management canister. Both are unit-testable: the pure conversion against a known pubkey vector, and the path construction.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_tecdsa.rs`:

```rust
use super::tecdsa::{custody_derivation_path, evm_address_from_pubkey, settlement_derivation_path};
use crate::chains::config::ChainId;
use candid::Principal;

#[test]
fn evm_address_from_known_uncompressed_pubkey() {
    // Well-known test vector: the secp256k1 generator point's address is
    // 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf for private key = 1.
    // Uncompressed pubkey for k=1 (65 bytes, 0x04 || X || Y):
    let pubkey_hex = "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";
    let pubkey = hex_to_bytes(pubkey_hex);
    let addr = evm_address_from_pubkey(&pubkey).expect("address");
    assert_eq!(addr.to_lowercase(), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
}

#[test]
fn evm_address_accepts_compressed_pubkey() {
    // 33-byte compressed pubkey for k=1: 0x02 || X.
    let compressed = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let addr = evm_address_from_pubkey(&hex_to_bytes(compressed)).expect("address");
    assert_eq!(addr.to_lowercase(), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
}

#[test]
fn custody_path_is_deterministic_and_distinct_per_user() {
    let p1 = Principal::from_slice(&[1, 2, 3]);
    let p2 = Principal::from_slice(&[4, 5, 6]);
    let a = custody_derivation_path(ChainId(10143), p1, 0);
    let b = custody_derivation_path(ChainId(10143), p1, 0);
    let c = custody_derivation_path(ChainId(10143), p2, 0);
    let d = custody_derivation_path(ChainId(10143), p1, 1);
    assert_eq!(a, b);          // deterministic
    assert_ne!(a, c);          // distinct per principal
    assert_ne!(a, d);          // distinct per nonce
}

#[test]
fn settlement_path_differs_from_any_custody_path() {
    let s = settlement_derivation_path(ChainId(10143));
    let cust = custody_derivation_path(ChainId(10143), Principal::anonymous(), 0);
    assert_ne!(s, cust);
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_tecdsa;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_tecdsa 2>&1 | tail -10
```

- [ ] **Step 3: Implement tecdsa.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/tecdsa.rs` with:

```rust
//! tECDSA address derivation for Monad (secp256k1).
//!
//! Pure helpers (pubkey -> address, derivation paths) are unit-tested. The
//! async `ecdsa_public_key` call hits the management canister and is covered
//! by the PocketIC integration test (Task 17) and manual staging (Task 23).

use crate::chains::config::ChainId;
use candid::Principal;
use ic_cdk::api::management_canister::ecdsa::{
    ecdsa_public_key, EcdsaCurve, EcdsaKeyId, EcdsaPublicKeyArgument,
};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::PublicKey;
use sha3::{Digest, Keccak256};

use super::config::monad_ecdsa_key_name;

/// Derivation path for a per-user collateral custody address.
/// `[chain_id (LE u32), principal bytes, nonce (LE u64)]`.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Derivation path for the per-chain settlement (minter) address.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
}

fn key_id() -> EcdsaKeyId {
    EcdsaKeyId { curve: EcdsaCurve::Secp256k1, name: monad_ecdsa_key_name() }
}

/// Convert a secp256k1 public key (33-byte compressed or 65-byte uncompressed)
/// to a checksummed-lowercase 0x EVM address: keccak256(uncompressed[1..])[12..].
pub fn evm_address_from_pubkey(pubkey: &[u8]) -> Result<String, String> {
    let pk = PublicKey::from_sec1_bytes(pubkey).map_err(|e| format!("bad pubkey: {e}"))?;
    let uncompressed = pk.to_encoded_point(false); // 0x04 || X(32) || Y(32)
    let bytes = uncompressed.as_bytes();
    if bytes.len() != 65 {
        return Err(format!("expected 65-byte uncompressed pubkey, got {}", bytes.len()));
    }
    let hash = Keccak256::digest(&bytes[1..]); // drop the 0x04 prefix
    let addr = &hash[12..]; // last 20 bytes
    Ok(format!("0x{}", hex::encode(addr)))
}

/// Async: fetch the derived public key from the management canister and return
/// both the raw pubkey and the EVM address. Used by deposit-address queries and
/// by the settlement worker to know its minter address.
pub async fn derive_evm_address(derivation_path: Vec<Vec<u8>>) -> Result<(Vec<u8>, String), String> {
    let arg = EcdsaPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: key_id(),
    };
    let (res,) = ecdsa_public_key(arg).await.map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    let addr = evm_address_from_pubkey(&res.public_key)?;
    Ok((res.public_key, addr))
}
```

Add `hex = "0.4"` to `src/rumi_protocol_backend/Cargo.toml` `[dependencies]` (used for `hex::encode`/decode). If `hex` is already a transitive dep, still declare it directly.

- [ ] **Step 4: Build wasm + run the tests**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_tecdsa
```

Expected: all four tests pass. The k=1 vector (`0x7E5F...Bdf`) is the canonical Ethereum "address of private key 1" check; if it fails, the keccak/pubkey-decompression wiring is wrong.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/monad/ src/rumi_protocol_backend/Cargo.toml Cargo.lock
git commit -m "feat(monad): tECDSA address derivation (custody + settlement)

Phase 1b Task 5. Pure pubkey->EVM-address (keccak) verified against the
k=1 vector; deterministic per-user + settlement derivation paths;
async ecdsa_public_key wrapper."
```

---

## Task 6: EVM RPC Wrapper

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/evm_rpc.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_evm_rpc.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

A thin typed wrapper over the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`) using `RpcServices::Custom` (Monad is not built in). The wrapper exposes the calls the adapter needs: latest + finalized block (`eth_getBlockByNumber`), `eth_getBalance`, `eth_getTransactionCount` (nonce), `eth_getLogs` (deposits + Burn events), `eth_getTransactionReceipt`, `eth_sendRawTransaction`, and a fee read (`eth_feeHistory` or `eth_gasPrice`). Pure response-parsing helpers (hex-quantity -> u128/u64, log-topic decode) are unit-tested; the inter-canister calls are covered by the PocketIC mock (Task 17).

- [ ] **Step 1: Write the failing test FIRST (pure parsers)**

Create `src/rumi_protocol_backend/src/chains/monad/tests_evm_rpc.rs`:

```rust
use super::evm_rpc::{parse_hex_quantity, decode_mint_log, decode_burn_log, MintLog, BurnLog};

#[test]
fn parses_hex_quantity() {
    assert_eq!(parse_hex_quantity("0x0").unwrap(), 0u128);
    assert_eq!(parse_hex_quantity("0x10").unwrap(), 16u128);
    assert_eq!(parse_hex_quantity("0x2540be400").unwrap(), 10_000_000_000u128); // 100 icUSD @ 8dp
    assert!(parse_hex_quantity("not-hex").is_err());
}

#[test]
fn decodes_burn_log() {
    // Burn(uint256 vault_id, address burner, uint256 amount)
    // topic0 = keccak("Burn(uint256,address,uint256)"). For this test we assert
    // the decoder pulls vault_id + amount out of a well-formed log's data/topics.
    // Construct a log where vault_id is indexed (topic1) and amount in data.
    let topic0 = super::evm_rpc::BURN_EVENT_TOPIC0.to_string();
    let vault_id_topic = format!("0x{:064x}", 7u64);
    let burner_topic = format!("0x{:064x}", 0u8); // placeholder address
    let amount_data = format!("0x{:064x}", 10_000_000_000u128);
    let log = BurnLog::from_raw(
        &[topic0, vault_id_topic, burner_topic],
        &amount_data,
        "0xtxhash",
        110,
    ).expect("decode burn");
    assert_eq!(log.vault_id, 7);
    assert_eq!(log.amount_e8s, 10_000_000_000);
    assert_eq!(log.block_number, 110);
}

#[test]
fn rejects_log_with_wrong_topic0() {
    let res = BurnLog::from_raw(
        &["0xdeadbeef".into(), format!("0x{:064x}", 1u64), format!("0x{:064x}", 0u8)],
        &format!("0x{:064x}", 1u128),
        "0xtx",
        1,
    );
    assert!(res.is_err());
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_evm_rpc;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_evm_rpc 2>&1 | tail -10
```

- [ ] **Step 3: Implement evm_rpc.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/evm_rpc.rs` with the wrapper. The exact `evm_rpc_client` API surface MUST be confirmed against the version pinned in Task 1; the shape below targets the typed-client pattern from the `evm-rpc` skill. Where the live API differs, adapt the call site but keep the public function signatures stable (the adapter in Task 8 depends on them).

```rust
//! Thin EVM RPC wrapper for Monad. Calls the EVM RPC canister with
//! `RpcServices::Custom` (Monad is not a built-in chain). Consensus across the
//! configured providers is handled by the EVM RPC canister, so no transform
//! function is needed here (unlike a raw https-outcall).

use crate::chains::config::ChainId;
use crate::state::read_state;
use candid::Principal;

/// EVM RPC canister (mainnet). Local/PocketIC overrides this with the mock
/// canister id via state (Task 17 wires the override).
pub const EVM_RPC_CANISTER: &str = "7hfb6-caaaa-aaaar-qadga-cai";

/// keccak256("Burn(uint256,address,uint256)") and
/// keccak256("Mint(uint256,address,uint256)"). Compute these once with the
/// canonical signatures and paste the 0x-prefixed 32-byte topic here. (A unit
/// test in Task 8 recomputes them via Keccak256 to guard against typos.)
pub const BURN_EVENT_TOPIC0: &str =
    "0x<keccak256 of Burn(uint256,address,uint256) - compute in Task 8>";
pub const MINT_EVENT_TOPIC0: &str =
    "0x<keccak256 of Mint(uint256,address,uint256) - compute in Task 8>";

#[derive(Clone, Debug)]
pub struct BurnLog {
    pub vault_id: u64,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

#[derive(Clone, Debug)]
pub struct MintLog {
    pub vault_id: u64,
    pub recipient: String,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

/// Parse an EVM hex quantity ("0x..") to u128. Rejects malformed input.
pub fn parse_hex_quantity(s: &str) -> Result<u128, String> {
    let stripped = s.strip_prefix("0x").ok_or_else(|| format!("missing 0x: {s}"))?;
    u128::from_str_radix(stripped, 16).map_err(|e| format!("bad hex {s}: {e}"))
}

impl BurnLog {
    /// Decode from raw topics + data. Expects:
    ///   topics[0] == BURN_EVENT_TOPIC0
    ///   topics[1] == vault_id (indexed uint256)
    ///   topics[2] == burner (indexed address)
    ///   data      == amount (uint256)
    pub fn from_raw(topics: &[String], data: &str, tx_hash: &str, block_number: u64) -> Result<Self, String> {
        let t0 = topics.first().ok_or("no topic0")?;
        if !t0.eq_ignore_ascii_case(BURN_EVENT_TOPIC0) {
            return Err(format!("wrong topic0: {t0}"));
        }
        let vault_id = parse_hex_quantity(topics.get(1).ok_or("no vault_id topic")?)? as u64;
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(Self { vault_id, amount_e8s, tx_hash: tx_hash.to_string(), block_number })
    }
}

impl MintLog {
    pub fn from_raw(topics: &[String], data: &str, tx_hash: &str, block_number: u64) -> Result<Self, String> {
        let t0 = topics.first().ok_or("no topic0")?;
        if !t0.eq_ignore_ascii_case(MINT_EVENT_TOPIC0) {
            return Err(format!("wrong topic0: {t0}"));
        }
        let vault_id = parse_hex_quantity(topics.get(1).ok_or("no vault_id topic")?)? as u64;
        let recipient = topics.get(2).cloned().unwrap_or_default();
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(Self { vault_id, recipient, amount_e8s, tx_hash: tx_hash.to_string(), block_number })
    }
}

// Free functions exposed for tests that don't construct a struct first.
pub fn decode_burn_log(topics: &[String], data: &str, tx_hash: &str, block: u64) -> Result<BurnLog, String> {
    BurnLog::from_raw(topics, data, tx_hash, block)
}
pub fn decode_mint_log(topics: &[String], data: &str, tx_hash: &str, block: u64) -> Result<MintLog, String> {
    MintLog::from_raw(topics, data, tx_hash, block)
}

fn evm_rpc_principal() -> Principal {
    // Allow a state override so PocketIC can point at the mock canister.
    read_state(|s| s.evm_rpc_override())
        .unwrap_or_else(|| Principal::from_text(EVM_RPC_CANISTER).expect("evm rpc principal"))
}

// --- async inter-canister calls (covered by the PocketIC mock, Task 17) ---
// Each returns a typed value or an error string. Implement against the pinned
// evm_rpc_client API. Signatures (stable; the adapter depends on them):

/// latest + finalized block numbers.
pub async fn fetch_block_numbers(chain: ChainId) -> Result<(u64, u64), String> { /* impl */ unimplemented!() }
/// MON balance (e18) of `address`.
pub async fn get_balance(chain: ChainId, address: &str) -> Result<u128, String> { unimplemented!() }
/// transaction count (nonce) of `address` at latest.
pub async fn get_transaction_count(chain: ChainId, address: &str) -> Result<u64, String> { unimplemented!() }
/// logs for `contract` between blocks for a given topic0.
pub async fn get_logs(chain: ChainId, contract: &str, topic0: &str, from_block: u64, to_block: u64) -> Result<Vec<(Vec<String>, String, String, u64)>, String> { unimplemented!() }
/// transaction receipt: Some((status_ok, block_number)) if mined, None if pending.
pub async fn get_transaction_receipt(chain: ChainId, tx_hash: &str) -> Result<Option<(bool, u64)>, String> { unimplemented!() }
/// submit a raw signed tx, returns tx hash.
pub async fn send_raw_transaction(chain: ChainId, raw_tx_hex: &str) -> Result<String, String> { unimplemented!() }
/// current base fee + suggested priority fee (gwei).
pub async fn fetch_fees(chain: ChainId) -> Result<(u128, u128), String> { unimplemented!() }
```

Note the `unimplemented!()` bodies are filled in this same task using the pinned `evm_rpc_client` API. Build the `RpcServices::Custom { chain_id, services }` from `read_state(|s| s.multi_chain.chain_configs[&chain].rpc_endpoints)`. The `unimplemented!()` markers above are call-shape placeholders for THIS plan only; the executor replaces each with the real client call before committing (do not commit `unimplemented!()`). The pure parsers below them are what the Step 1 tests cover.

Add an `evm_rpc_override()` helper on `State` (returns `Option<Principal>`, default None; a developer-gated setter `set_evm_rpc_principal` lands in Task 14 so PocketIC + staging can point at the mock or the real canister). Add a `#[serde(default)]` `evm_rpc_principal_override: Option<Principal>` field to `State` directly (it is a single scalar, not multi-chain state; `#[serde(default)]` is sufficient and it round-trips through the existing State decode).

- [ ] **Step 4: Build + run the parser tests**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_evm_rpc
```

Expected: the three parser tests pass. (The async fns are not exercised here.)

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/monad/ src/rumi_protocol_backend/src/state.rs
git commit -m "feat(monad): EVM RPC wrapper (custom services, log decoders)

Phase 1b Task 6. Typed evm_rpc_client wrapper over chain 10143 custom
providers; pure hex-quantity + Burn/Mint log decoders unit-tested.
evm_rpc principal override on State for PocketIC/staging."
```

---

## Task 7: EIP-1559 Transaction Build + Sign

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/tx.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_tx.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

Build an EIP-1559 (type-2) transaction, RLP-encode the signing payload, keccak256-hash it, sign via `sign_with_ecdsa`, recover the `v` parity, and RLP-encode the final signed tx for `eth_sendRawTransaction`. The pure pieces (RLP encoding, ABI-encoding the `mint`/`transfer` calldata, signed-tx assembly given a fixed signature) are unit-tested against known vectors. The `sign_with_ecdsa` call is covered by PocketIC (Task 17).

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_tx.rs`:

```rust
use super::tx::{encode_mint_calldata, encode_transfer_calldata, Eip1559Fields, assemble_signed_tx};

#[test]
fn mint_calldata_has_correct_selector() {
    // mint(address,uint256,uint64) selector = first 4 bytes of
    // keccak256("mint(address,uint256,uint64)").
    let calldata = encode_mint_calldata("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf", 10_000_000_000, 42);
    // 4-byte selector + 3 * 32-byte args = 100 bytes.
    assert_eq!(calldata.len(), 4 + 32 * 3);
    // Selector recomputed in Task 8's topic test; here assert non-zero + length.
    assert_ne!(&calldata[0..4], &[0u8; 4]);
}

#[test]
fn transfer_calldata_encodes_address_and_amount() {
    let calldata = encode_transfer_calldata("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf", 5_000_000_000_000_000_000);
    assert_eq!(calldata.len(), 4 + 32 * 2);
}

#[test]
fn signed_tx_assembly_is_rlp_type2() {
    // Given fixed fields + a fixed 64-byte signature + parity, assemble the
    // signed type-2 tx and assert it starts with the 0x02 type byte.
    let fields = Eip1559Fields {
        chain_id: 10143,
        nonce: 0,
        max_priority_fee_per_gas: 2_000_000_000,
        max_fee_per_gas: 50_000_000_000,
        gas_limit: 120_000,
        to: "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into(),
        value: 0,
        data: vec![0xde, 0xad, 0xbe, 0xef],
    };
    let r = [0x11u8; 32];
    let s = [0x22u8; 32];
    let signed = assemble_signed_tx(&fields, &r, &s, 0).expect("assemble");
    assert_eq!(signed[0], 0x02); // EIP-2718 type byte for EIP-1559
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_tx;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_tx 2>&1 | tail -10
```

- [ ] **Step 3: Implement tx.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/tx.rs` with the builder. Use `alloy-rlp` for RLP. ABI-encode calldata by hand (selector + left-padded 32-byte words) to avoid pulling a heavy ABI crate.

```rust
//! EIP-1559 transaction construction + tECDSA signing for Monad.

use ic_cdk::api::management_canister::ecdsa::{
    sign_with_ecdsa, EcdsaCurve, EcdsaKeyId, SignWithEcdsaArgument,
};
use sha3::{Digest, Keccak256};

use super::config::monad_ecdsa_key_name;

#[derive(Clone, Debug)]
pub struct Eip1559Fields {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: String,   // 0x-address
    pub value: u128,  // wei
    pub data: Vec<u8>,
}

fn selector(sig: &str) -> [u8; 4] {
    let h = Keccak256::digest(sig.as_bytes());
    [h[0], h[1], h[2], h[3]]
}

fn word_from_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

fn word_from_address(addr: &str) -> [u8; 32] {
    let mut w = [0u8; 32];
    let bytes = hex::decode(addr.trim_start_matches("0x")).unwrap_or_default();
    if bytes.len() == 20 {
        w[12..].copy_from_slice(&bytes);
    }
    w
}

/// mint(address to, uint256 amount, uint64 vault_id)
pub fn encode_mint_calldata(to: &str, amount_e8s: u128, vault_id: u64) -> Vec<u8> {
    let mut out = selector("mint(address,uint256,uint64)").to_vec();
    out.extend_from_slice(&word_from_address(to));
    out.extend_from_slice(&word_from_u128(amount_e8s));
    out.extend_from_slice(&word_from_u128(vault_id as u128));
    out
}

/// transfer(address to, uint256 amount) - used for MON-collateral return is
/// native, so this is for ERC-20 collateral if added later. Native MON sends
/// use Eip1559Fields.value with empty data instead.
pub fn encode_transfer_calldata(to: &str, amount: u128) -> Vec<u8> {
    let mut out = selector("transfer(address,uint256)").to_vec();
    out.extend_from_slice(&word_from_address(to));
    out.extend_from_slice(&word_from_u128(amount));
    out
}

/// RLP-encode the type-2 signing payload (no signature) per EIP-1559:
/// 0x02 || rlp([chain_id, nonce, max_priority, max_fee, gas, to, value, data, access_list]).
fn encode_unsigned(fields: &Eip1559Fields) -> Vec<u8> {
    // Build with alloy-rlp. The access list is an empty list.
    // (Exact alloy-rlp call shape verified at execution; the byte layout is
    // the EIP-1559 canonical encoding.)
    let mut payload = rlp_encode_eip1559(fields, None);
    let mut out = vec![0x02u8];
    out.append(&mut payload);
    out
}

/// Keccak256 of the unsigned type-2 payload = the digest tECDSA signs.
pub fn signing_hash(fields: &Eip1559Fields) -> [u8; 32] {
    let unsigned = encode_unsigned(fields);
    let h = Keccak256::digest(&unsigned);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h);
    out
}

/// Assemble the final signed type-2 tx from fields + (r, s, y_parity).
pub fn assemble_signed_tx(fields: &Eip1559Fields, r: &[u8; 32], s: &[u8; 32], y_parity: u8) -> Result<Vec<u8>, String> {
    let mut payload = rlp_encode_eip1559(fields, Some((r, s, y_parity)));
    let mut out = vec![0x02u8];
    out.append(&mut payload);
    Ok(out)
}

/// RLP-encode the EIP-1559 field list, optionally including the signature.
/// Implement with alloy-rlp's list encoder. Splitting it out keeps both the
/// unsigned (signing) and signed encodings consistent.
fn rlp_encode_eip1559(fields: &Eip1559Fields, sig: Option<(&[u8; 32], &[u8; 32], u8)>) -> Vec<u8> {
    // IMPLEMENT with alloy-rlp. Field order:
    //   chain_id, nonce, max_priority_fee_per_gas, max_fee_per_gas, gas_limit,
    //   to (20 bytes), value, data (bytes), access_list (empty list),
    //   then if signed: y_parity, r (trimmed), s (trimmed).
    // Encode integers as big-endian minimal byte strings (RLP convention).
    unimplemented!("alloy-rlp list encoding")
}

/// y-parity from the signature + message + expected signer address. tECDSA
/// returns a 64-byte (r||s) signature with no recovery id; recover the parity
/// by trying both (0/1) and matching the recovered address to `expected_addr`.
pub fn recover_y_parity(hash: &[u8; 32], r: &[u8; 32], s: &[u8; 32], expected_addr: &str) -> Result<u8, String> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
    let sig = Signature::from_scalars(*r, *s).map_err(|e| format!("sig: {e}"))?;
    for parity in [0u8, 1u8] {
        let rid = RecoveryId::from_byte(parity).ok_or("bad rid")?;
        if let Ok(vk) = VerifyingKey::recover_from_prehash(hash, &sig, rid) {
            let pubkey = vk.to_encoded_point(false);
            if let Ok(addr) = super::tecdsa::evm_address_from_pubkey(pubkey.as_bytes()) {
                if addr.eq_ignore_ascii_case(expected_addr) {
                    return Ok(parity);
                }
            }
        }
    }
    Err("could not recover parity".into())
}

fn key_id() -> EcdsaKeyId {
    EcdsaKeyId { curve: EcdsaCurve::Secp256k1, name: monad_ecdsa_key_name() }
}

/// Async: sign + assemble a ready-to-broadcast raw tx hex for the given fields,
/// signing under `derivation_path` whose address is `signer_addr`.
pub async fn sign_eip1559(
    fields: &Eip1559Fields,
    derivation_path: Vec<Vec<u8>>,
    signer_addr: &str,
) -> Result<String, String> {
    let hash = signing_hash(fields);
    let arg = SignWithEcdsaArgument {
        message_hash: hash.to_vec(),
        derivation_path,
        key_id: key_id(),
    };
    let (res,) = sign_with_ecdsa(arg).await.map_err(|(c, m)| format!("{c:?}: {m}"))?;
    if res.signature.len() != 64 {
        return Err(format!("expected 64-byte sig, got {}", res.signature.len()));
    }
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&res.signature[..32]);
    s.copy_from_slice(&res.signature[32..]);
    let parity = recover_y_parity(&hash, &r, &s, signer_addr)?;
    let signed = assemble_signed_tx(fields, &r, &s, parity)?;
    Ok(format!("0x{}", hex::encode(signed)))
}
```

Replace both `unimplemented!()` bodies (`rlp_encode_eip1559`) with the real alloy-rlp encoding before committing. Add an integration-grade RLP vector test (Step 4) so the encoding is pinned.

- [ ] **Step 4: Add an RLP vector test, then run all tx tests**

Add to `tests_tx.rs` one canonical EIP-1559 RLP vector (take a known signed type-2 tx from the EIP-1559 spec or a Foundry-generated fixture, hard-code its fields + expected raw hex, assert `assemble_signed_tx` reproduces it byte-for-byte). This pins the RLP layout. Then:

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_tx
```

Expected: all tx tests pass, including the RLP vector.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/monad/ src/rumi_protocol_backend/Cargo.toml Cargo.lock
git commit -m "feat(monad): EIP-1559 tx build + tECDSA sign

Phase 1b Task 7. RLP type-2 encoding pinned against a known vector; mint/
transfer calldata; y-parity recovery via k256; sign_with_ecdsa wrapper."
```

---

## Task 8: ChainAdapter Implementation for Monad

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/adapter.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_adapter.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

Implement `ChainAdapter` for `MonadAdapter` using Tasks 5/6/7. The adapter is the glue: `chain_id`, `verify_deposit`, `sign_withdrawal`, `sign_mint`, `sign_burn`, `fetch_finality`, `observe_event`. It carries the chain id and reads config from state. This task also pins the `Burn`/`Mint` topic0 constants by recomputing them with keccak (guarding the typed constants in `evm_rpc.rs`).

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_adapter.rs`:

```rust
use super::adapter::MonadAdapter;
use super::evm_rpc::{BURN_EVENT_TOPIC0, MINT_EVENT_TOPIC0};
use crate::chains::adapter::ChainAdapter;
use crate::chains::config::ChainId;
use sha3::{Digest, Keccak256};

#[test]
fn adapter_reports_monad_chain_id() {
    let a = MonadAdapter::new(ChainId(10143));
    assert_eq!(a.chain_id(), ChainId(10143));
}

#[test]
fn adapter_is_trait_object_safe() {
    let a: Box<dyn ChainAdapter> = Box::new(MonadAdapter::new(ChainId(10143)));
    assert_eq!(a.chain_id(), ChainId(10143));
}

#[test]
fn burn_topic0_matches_canonical_signature() {
    let expected = format!("0x{}", hex::encode(Keccak256::digest(b"Burn(uint256,address,uint256)")));
    assert_eq!(BURN_EVENT_TOPIC0.to_lowercase(), expected);
}

#[test]
fn mint_topic0_matches_canonical_signature() {
    let expected = format!("0x{}", hex::encode(Keccak256::digest(b"Mint(uint256,address,uint256)")));
    assert_eq!(MINT_EVENT_TOPIC0.to_lowercase(), expected);
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_adapter;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_adapter 2>&1 | tail -10
```

The topic tests fail first (the constants in Task 6 are placeholders). Compute the real values and paste them into `evm_rpc.rs`'s `BURN_EVENT_TOPIC0` / `MINT_EVENT_TOPIC0`:

```bash
# One-off to print the canonical topic0 values:
cat > /tmp/topic.rs <<'EOF'
fn main() {
    use sha3::{Digest, Keccak256};
    for sig in ["Burn(uint256,address,uint256)", "Mint(uint256,address,uint256)"] {
        println!("{sig} => 0x{}", hex::encode(Keccak256::digest(sig.as_bytes())));
    }
}
EOF
# Or just let the failing test's assert message print the expected value.
```

- [ ] **Step 3: Implement adapter.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/adapter.rs` with:

```rust
//! MonadAdapter: ChainAdapter impl for Monad testnet.

use crate::chains::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot, MintInstruction,
    SignedBurn, SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use crate::chains::config::ChainId;
use crate::state::read_state;
use async_trait::async_trait;
use candid::Principal;

use super::config::MONAD_ICUSD_DECIMALS;
use super::evm_rpc;
use super::tecdsa;
use super::tx::{encode_mint_calldata, sign_eip1559, Eip1559Fields};

pub struct MonadAdapter {
    chain_id: ChainId,
}

impl MonadAdapter {
    pub fn new(chain_id: ChainId) -> Self {
        Self { chain_id }
    }

    fn icusd_contract(&self) -> Result<String, ChainAdapterError> {
        read_state(|s| s.multi_chain.chain_contracts.get(&self.chain_id).cloned())
            .ok_or_else(|| ChainAdapterError::InvalidPayload("icUSD contract not set".into()))
    }

    async fn build_fees(&self) -> Result<(u128, u128), ChainAdapterError> {
        evm_rpc::fetch_fees(self.chain_id)
            .await
            .map_err(|m| ChainAdapterError::RpcError { provider: "evm_rpc".into(), message: m })
    }
}

#[async_trait(?Send)]
impl ChainAdapter for MonadAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    async fn verify_deposit(&self, tx_hash: &str) -> Result<DepositRecord, ChainAdapterError> {
        // Receipt lookup; the observer (Task 9) is the primary deposit path.
        match evm_rpc::get_transaction_receipt(self.chain_id, tx_hash).await {
            Ok(Some((true, block))) => Ok(DepositRecord {
                depositor: String::new(),
                amount_e8s: 0, // populated by the observer from the tx, not here
                block_number: block,
                tx_hash: tx_hash.to_string(),
            }),
            Ok(_) => Err(ChainAdapterError::InvalidPayload("deposit not mined".into())),
            Err(m) => Err(ChainAdapterError::RpcError { provider: "evm_rpc".into(), message: m }),
        }
    }

    async fn sign_withdrawal(&self, req: WithdrawalRequest) -> Result<SignedWithdrawal, ChainAdapterError> {
        // Native MON return: value = amount, empty data, to = recipient.
        let settlement_path = tecdsa::settlement_derivation_path(self.chain_id);
        let (_, settlement_addr) = tecdsa::derive_evm_address(settlement_path.clone())
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        let nonce = evm_rpc::get_transaction_count(self.chain_id, &settlement_addr)
            .await
            .map_err(|m| ChainAdapterError::RpcError { provider: "evm_rpc".into(), message: m })?;
        let (base_fee, prio) = self.build_fees().await?;
        let fields = Eip1559Fields {
            chain_id: self.chain_id.0 as u64,
            nonce,
            max_priority_fee_per_gas: prio,
            max_fee_per_gas: base_fee.saturating_mul(2).saturating_add(prio),
            gas_limit: 21_000,
            to: req.recipient.clone(),
            value: req.amount_e8s, // collateral is MON e18; caller passes e18 here
            data: vec![],
        };
        let raw = sign_eip1559(&fields, settlement_path, &settlement_addr)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        Ok(SignedWithdrawal { raw_tx: raw.into_bytes(), tx_hash: String::new() })
    }

    async fn sign_mint(&self, instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        let _ = MONAD_ICUSD_DECIMALS; // amount_e8s maps 1:1 to on-chain units
        let contract = self.icusd_contract()?;
        let settlement_path = tecdsa::settlement_derivation_path(self.chain_id);
        let (_, settlement_addr) = tecdsa::derive_evm_address(settlement_path.clone())
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        let nonce = evm_rpc::get_transaction_count(self.chain_id, &settlement_addr)
            .await
            .map_err(|m| ChainAdapterError::RpcError { provider: "evm_rpc".into(), message: m })?;
        let (base_fee, prio) = self.build_fees().await?;
        let data = encode_mint_calldata(&instr.recipient, instr.amount_e8s, instr.vault_id);
        let fields = Eip1559Fields {
            chain_id: self.chain_id.0 as u64,
            nonce,
            max_priority_fee_per_gas: prio,
            max_fee_per_gas: base_fee.saturating_mul(2).saturating_add(prio),
            gas_limit: 120_000,
            to: contract,
            value: 0,
            data,
        };
        let raw = sign_eip1559(&fields, settlement_path, &settlement_addr)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;
        Ok(SignedMint { raw_tx: raw.into_bytes(), tx_hash: String::new() })
    }

    async fn sign_burn(&self, _amount_e8s: u128, _burner: Principal) -> Result<SignedBurn, ChainAdapterError> {
        // Phase 1b: burns are user-initiated on-chain (the user calls
        // IcUSD.burn from their own wallet). The canister never signs a burn.
        // Kept as Err so a future canister-initiated burn (e.g. SP backstop,
        // Phase 1c) has the slot.
        Err(ChainAdapterError::NotImplemented)
    }

    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        let (latest, finalized) = evm_rpc::fetch_block_numbers(self.chain_id)
            .await
            .map_err(|m| ChainAdapterError::RpcError { provider: "evm_rpc".into(), message: m })?;
        Ok(FinalitySnapshot { latest_block: latest, finalized_block: finalized })
    }

    async fn observe_event(&self, from_block: u64) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        // Returns nothing here; deposit/burn observation is done by the
        // dedicated observer (Task 9) which decodes typed logs. observe_event
        // satisfies the trait and is used for a generic finality-bounded scan.
        let _ = from_block;
        Ok(vec![])
    }
}
```

- [ ] **Step 4: Build + run adapter tests**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_adapter
```

Expected: all four pass (topic0 constants now match keccak).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/monad/
git commit -m "feat(monad): ChainAdapter impl (mint/withdraw sign, finality)

Phase 1b Task 8. sign_mint builds mint() calldata + EIP-1559 tx;
sign_withdrawal does native MON return; Burn/Mint topic0 pinned to keccak."
```

---

## Task 9: Inbound Observer (Deposit Watch + Burn Watch)

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/deposit_watch.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_deposit_watch.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

The observer is the inbound async loop (registered on a timer in Task 15). It does two jobs at finality depth: (1) credit collateral deposits to custody addresses, marking the relevant `ChainVault` pending->confirmed, and (2) watch the icUSD `Burn` event and decrement `chain_supplies` + vault debt via `apply_supply_delta`. The pure state-transition helpers are unit-tested; the RPC fetch is covered by the PocketIC mock (Task 17).

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_deposit_watch.rs`:

```rust
use super::deposit_watch::{apply_burn_to_state, credit_deposit_to_state};
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::monad::evm_rpc::BurnLog;
use candid::Principal;

fn seeded() -> MultiChainStateV2 {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1,
        owner: Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(),
        collateral_amount_e18: 0,
        debt_e8s: 0,
        mint_recipient: "0xr".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
    });
    s
}

#[test]
fn credit_deposit_increments_collateral() {
    let mut s = seeded();
    credit_deposit_to_state(&mut s, 1, 5_000_000_000_000_000_000).expect("credit");
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 5_000_000_000_000_000_000);
}

#[test]
fn burn_decrements_supply_and_debt_preserving_invariant() {
    let mut s = seeded();
    // Vault has 100 icUSD debt; chain_supply matches; total_debt = 100e8.
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 10_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 10_000_000_000);
    let total_debt = 10_000_000_000u128;
    let burn = BurnLog { vault_id: 1, amount_e8s: 4_000_000_000, tx_hash: "0xb".into(), block_number: 110 };
    apply_burn_to_state(&mut s, &burn, total_debt).expect("burn");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 6_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 6_000_000_000);
}

#[test]
fn burn_exceeding_debt_is_rejected_without_mutation() {
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 1_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 1_000_000_000);
    let burn = BurnLog { vault_id: 1, amount_e8s: 9_999_999_999, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 1_000_000_000);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].debt_e8s, 1_000_000_000); // unchanged
}

#[test]
fn burn_for_unknown_vault_is_rejected() {
    let mut s = seeded();
    let burn = BurnLog { vault_id: 999, amount_e8s: 1, tx_hash: "0xb".into(), block_number: 1 };
    assert!(apply_burn_to_state(&mut s, &burn, 0).is_err());
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_deposit_watch;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_deposit_watch 2>&1 | tail -10
```

- [ ] **Step 3: Implement deposit_watch.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/deposit_watch.rs` with:

```rust
//! Inbound observer for Monad: collateral deposits + icUSD Burn events.
//!
//! Pure state helpers (`credit_deposit_to_state`, `apply_burn_to_state`) are
//! unit-tested. The async `run_observer` ties them to the RPC + finality and is
//! covered by the PocketIC mock (Task 17). Registered on a timer in Task 15.

use crate::chains::config::ChainId;
use crate::chains::monad::evm_rpc::{self, BurnLog};
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};

/// Credit a confirmed collateral deposit (MON e18) to a vault.
pub fn credit_deposit_to_state(state: &mut MultiChainStateV2, vault_id: u64, amount_e18: u128) -> Result<(), String> {
    let v = state.chain_vaults.get_mut(&vault_id).ok_or("unknown vault")?;
    v.collateral_amount_e18 = v.collateral_amount_e18.saturating_add(amount_e18);
    Ok(())
}

/// Apply a confirmed Burn: decrement vault debt + chain supply together,
/// preserving sum(chain_supplies) == total_debt. `new_total_debt` is the
/// authoritative total AFTER this burn (caller computes it).
pub fn apply_burn_to_state(
    state: &mut MultiChainStateV2,
    burn: &BurnLog,
    new_total_debt: u128,
) -> Result<(), String> {
    let chain = {
        let v = state.chain_vaults.get(&burn.vault_id).ok_or("unknown vault")?;
        if burn.amount_e8s > v.debt_e8s {
            return Err(format!("burn {} exceeds debt {}", burn.amount_e8s, v.debt_e8s));
        }
        v.collateral_chain
    };
    // Supply delta first (it validates + can reject); only then touch debt.
    apply_supply_delta(state, chain, SupplyDelta::Decrease(burn.amount_e8s), new_total_debt)
        .map_err(|e: SupplyInvariantError| format!("supply: {e:?}"))?;
    let v = state.chain_vaults.get_mut(&burn.vault_id).unwrap();
    v.debt_e8s -= burn.amount_e8s;
    Ok(())
}
```

Note: `apply_supply_delta` takes `&mut MultiChainStateV1` in Phase 1a. Update its signature to `&mut MultiChainStateV2` (the active alias is now V2) as part of this task; the Phase 1a unit tests in `tests_supply.rs` already construct `MultiChainStateV1` directly, so either (a) change them to `MultiChainStateV2` or (b) keep `apply_supply_delta` generic is not possible (it accesses `chain_supplies`/`invariant_halted` which both V1 and V2 have). Simplest: repoint `apply_supply_delta` + `check_invariant` to `MultiChainStateV2` and update the four Phase 1a test files' `MultiChainStateV1` references to `MultiChainStateV2`. Run the full lib suite to confirm.

Add the async `run_observer(chain: ChainId)` that: reads `last_observed_block`, fetches finalized block via `fetch_block_numbers`, scans `get_logs` for the Burn topic over `(last+1 ..= finalized)`, decodes each via `BurnLog::from_raw`, computes `new_total_debt = current_total_debt - amount`, calls `apply_burn_to_state`, records `Event::ChainBurnObserved`, and advances `last_observed_block`. Also scans custody-address balances / deposit txs to credit collateral and emits `Event::DepositObserved`. The exact `current_total_debt` source: `read_state(|s| s.multi_chain.total_supply_all_chains_e8s())` (the confirmed total). Guard the whole loop on `mode != ReadOnly` and `!invariant_halted`.

- [ ] **Step 4: Build + run the observer tests + full lib suite**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_deposit_watch
cargo test --package rumi_protocol_backend --lib
```

Expected: observer tests pass; the supply/self-check tests still pass after the V1->V2 signature repoint.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(monad): inbound observer (deposit credit + Burn -> debt decrement)

Phase 1b Task 9. apply_burn_to_state decrements supply + debt together via
apply_supply_delta (repointed to MultiChainStateV2). Burn exceeding debt or
unknown vault rejected with no mutation."
```

---

## Task 10: Outbound Settlement Worker (Timer D)

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/settlement.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_settlement.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

Timer D drains `settlement_queues[Monad]`. For each `Queued` op it (gas-gates, then) signs via the adapter and submits via `send_raw_transaction`, marking `Inflight` with the tx hash. For each `Inflight` op it checks the receipt at finality; a confirmed mint applies `apply_supply_delta(+)` and moves the vault `pending_mint_e8s -> debt_e8s` (Design B: this is where debt starts counting), marks `Succeeded`, and emits `ChainMintConfirmed`. The pure drain state-machine (which op to act on, status transitions, one-in-flight-per-address) is unit-tested with a mock; the real signing/submission is covered by PocketIC (Task 17).

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_settlement.rs`:

```rust
use super::settlement::{confirm_mint_in_state, select_next_op, OpAction};
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind, SettlementOpStatus};
use candid::Principal;

fn vault_pending(s: &mut MultiChainStateV2, vault_id: u64, pending: u128) {
    s.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_e18: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: pending,
        status: ChainVaultStatus::MintPending, opened_at_ns: 0,
    });
}

#[test]
fn select_next_op_prefers_queued_then_inflight() {
    let mut q = crate::chains::settlement_queue::SettlementQueueV1::default();
    let mut op = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xr".into(), amount_e8s: 10, vault_id: 1 },
        "k0".into(), 0);
    let id = q.enqueue(op.clone()).unwrap();
    op.op_id = id;
    // Queued -> action Submit.
    match select_next_op(&q) {
        Some((oid, OpAction::Submit)) => assert_eq!(oid, id),
        other => panic!("expected Submit, got {other:?}"),
    }
}

#[test]
fn confirm_mint_moves_pending_to_debt_and_increments_supply() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000); // 100 icUSD pending
    // Confirming the mint: total_debt goes 0 -> 100e8.
    confirm_mint_in_state(&mut s, ChainId(10143), 1, 10_000_000_000, 10_000_000_000).expect("confirm");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 10_000_000_000);
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Open));
    assert_eq!(s.chain_supplies[&ChainId(10143)], 10_000_000_000);
}

#[test]
fn confirm_mint_rejects_amount_mismatch() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    vault_pending(&mut s, 1, 10_000_000_000);
    // Observed amount differs from pending: reject, do not mutate.
    let res = confirm_mint_in_state(&mut s, ChainId(10143), 1, 9_999_999_999, 9_999_999_999);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].pending_mint_e8s, 10_000_000_000);
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_settlement;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_settlement 2>&1 | tail -10
```

- [ ] **Step 3: Implement settlement.rs**

Replace `src/rumi_protocol_backend/src/chains/monad/settlement.rs` with the worker. Pure helpers first, then the async `run_settlement(chain)`:

```rust
//! Outbound settlement worker (Timer D) for Monad.

use crate::chains::config::ChainId;
use crate::chains::monad::chain_vault::ChainVaultStatus;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOpStatus, SettlementQueueV1};
use crate::chains::supply::{apply_supply_delta, SupplyDelta};

#[derive(Debug, PartialEq, Eq)]
pub enum OpAction {
    /// Queued op: sign + submit.
    Submit,
    /// Inflight op: check receipt / finality.
    Confirm,
}

/// Pick the next op to act on, draining head-first. One in-flight at a time per
/// queue (per-derivation-path serialization): if any op is Inflight, only that
/// op (Confirm) is actionable; otherwise the head Queued op (Submit).
pub fn select_next_op(q: &SettlementQueueV1) -> Option<(u64, OpAction)> {
    // If something is in flight, confirm it before submitting anything new.
    for (&id, op) in q.pending.iter() {
        if matches!(op.status, SettlementOpStatus::Inflight { .. }) {
            return Some((id, OpAction::Confirm));
        }
    }
    for (&id, op) in q.pending.iter() {
        if matches!(op.status, SettlementOpStatus::Queued) {
            return Some((id, OpAction::Submit));
        }
    }
    None
}

/// On confirmed mint: move pending_mint_e8s into debt_e8s, flip vault to Open,
/// and increment chain supply. `new_total_debt` is the authoritative total
/// AFTER this confirmation. `observed_e8s` must equal the vault's pending mint.
pub fn confirm_mint_in_state(
    state: &mut MultiChainStateV2,
    chain: ChainId,
    vault_id: u64,
    observed_e8s: u128,
    new_total_debt: u128,
) -> Result<(), String> {
    {
        let v = state.chain_vaults.get(&vault_id).ok_or("unknown vault")?;
        if v.pending_mint_e8s != observed_e8s {
            return Err(format!("observed {} != pending {}", observed_e8s, v.pending_mint_e8s));
        }
    }
    apply_supply_delta(state, chain, SupplyDelta::Increase(observed_e8s), new_total_debt)
        .map_err(|e| format!("supply: {e:?}"))?;
    let v = state.chain_vaults.get_mut(&vault_id).unwrap();
    v.debt_e8s = v.debt_e8s.saturating_add(observed_e8s);
    v.pending_mint_e8s = 0;
    v.status = ChainVaultStatus::Open;
    Ok(())
}
```

Add the async `run_settlement(chain: ChainId)`:
- Guard on `mode != ReadOnly` and `!invariant_halted`.
- `select_next_op`. If `Submit`: gas-gate (Task 11's `hot_wallet_ok`), build a `MonadAdapter`, call `sign_mint`/`sign_withdrawal` from the op kind, `send_raw_transaction`, mark `Inflight` (`mark_inflight` + store tx hash; extend `SettlementOpStatus::Inflight` or carry the hash on the op - simplest: add an `Option<String> last_tx_hash` field to `SettlementOp` via the versioned queue; since `SettlementQueueV1` is inside `MultiChainStateV2` and `#[serde(default)]`-friendly, add the field with `#[serde(default)]`), emit `ChainMintSubmitted`/`WithdrawalSigned`.
- If `Confirm`: `get_transaction_receipt`. If mined+ok at finality: for a mint, fetch the Mint log to read the confirmed amount, `confirm_mint_in_state`, mark `Succeeded`, emit `ChainMintConfirmed`; for a withdrawal, mark `Succeeded`, set vault `Closing->Closed` if fully repaid. If reverted: mark `Failed`, clear `pending_mint_e8s` (Design B: no debt was counted, so no supply reversal needed), emit `ChainSettlementFailed`. If still pending past the stuck threshold: hand to Task 11's stuck-tx path.

Adding the `last_tx_hash` field to `SettlementOp`: append `#[serde(default)] pub last_tx_hash: Option<String>,` to the struct in `settlement_queue.rs`. Update `SettlementOp::new` to set it `None`. The Phase 1a `tests_settlement_queue.rs` constructs ops via `SettlementOp::new`, so they keep compiling.

- [ ] **Step 4: Build + run settlement tests + full lib suite**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_settlement
cargo test --package rumi_protocol_backend --lib
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(monad): settlement worker (Timer D drain + mint confirm)

Phase 1b Task 10. select_next_op enforces one-in-flight-per-queue;
confirm_mint_in_state moves pending->debt + increments supply on observed
mint (Design B). last_tx_hash added to SettlementOp (serde default)."
```

---

## Task 11: Operational Hardening (Stuck-Tx, Reorg, Hot-Wallet Gas)

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/monad/hardening.rs`
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_hardening.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/{mod.rs, settlement.rs, deposit_watch.rs}`

Three hardening behaviours from spec Section 3, all unit-testable as pure predicates:
1. **Stuck-tx detection:** an `Inflight` op whose tx has not confirmed within `finality_depth x 2` (measured in elapsed time using the chain's block cadence, or simply N ticks) gets its gas bumped and is resubmitted with the same nonce.
2. **Reorg handling:** if the observer ever sees the finalized block number go BACKWARDS by more than `finality_depth`, halt new ops on that chain (set a per-chain `reorg_halted` flag) and emit `ChainReorgDetected`. Shallower regressions are within finality tolerance and ignored.
3. **Hot-wallet gas gate:** before submitting any outbound tx, check the cached settlement-address MON balance against a per-chain threshold; below it, refuse new outbound ops (reads still work) and emit `ChainHotWalletLow`.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_hardening.rs`:

```rust
use super::hardening::{is_stuck, is_reorg, hot_wallet_ok, bump_gas, HOT_WALLET_MIN_E18};

#[test]
fn detects_stuck_tx_after_threshold() {
    // finality_depth = 1 -> threshold 2 ticks. tries >= 2 + still inflight.
    assert!(!is_stuck(1, 1));   // 1 try, not yet
    assert!(is_stuck(2, 1));    // 2 tries at depth 1 -> stuck
    assert!(is_stuck(10, 5));   // 10 tries at depth 5 -> stuck (>= 10)
    assert!(!is_stuck(9, 5));   // 9 < 2*5
}

#[test]
fn detects_reorg_only_beyond_finality_depth() {
    // observed previously 100, now 98 with finality_depth 1 -> regression 2 > 1 -> reorg.
    assert!(is_reorg(100, 98, 1));
    // regression 1 == finality_depth -> within tolerance, not a reorg.
    assert!(!is_reorg(100, 99, 1));
    // forward progress -> never a reorg.
    assert!(!is_reorg(100, 105, 1));
}

#[test]
fn hot_wallet_gate_blocks_below_threshold() {
    assert!(hot_wallet_ok(HOT_WALLET_MIN_E18));
    assert!(hot_wallet_ok(HOT_WALLET_MIN_E18 + 1));
    assert!(!hot_wallet_ok(HOT_WALLET_MIN_E18 - 1));
}

#[test]
fn bump_gas_increases_fees_by_at_least_125_percent() {
    let (new_prio, new_max) = bump_gas(2_000_000_000, 50_000_000_000);
    assert!(new_prio >= 2_000_000_000 * 125 / 100);
    assert!(new_max >= 50_000_000_000 * 125 / 100);
}
```

Wire in `mod.rs`:
```rust
pub mod hardening;
#[cfg(test)]
mod tests_hardening;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_hardening 2>&1 | tail -10
```

- [ ] **Step 3: Implement hardening.rs**

```rust
//! Operational hardening predicates (spec Section 3). Pure + unit-tested.

/// Minimum settlement-address MON balance (e18) to allow new outbound ops.
/// Below this, refuse new ops on the chain (reads still work). Tunable via a
/// developer setter if needed; 0.1 MON default.
pub const HOT_WALLET_MIN_E18: u128 = 100_000_000_000_000_000; // 0.1 MON

/// An inflight op is stuck once tries >= finality_depth * 2 (each tick that
/// re-confirms increments tries). EVM bump-and-resubmit on the same nonce.
pub fn is_stuck(tries: u32, finality_depth: u32) -> bool {
    tries as u64 >= (finality_depth as u64).saturating_mul(2).max(2)
}

/// A reorg deeper than finality is when the newly-observed finalized block is
/// LOWER than the previously-observed one by more than finality_depth.
pub fn is_reorg(prev_observed: u64, now_observed: u64, finality_depth: u32) -> bool {
    now_observed < prev_observed && (prev_observed - now_observed) > finality_depth as u64
}

/// Gas gate: settlement address has enough MON.
pub fn hot_wallet_ok(balance_e18: u128) -> bool {
    balance_e18 >= HOT_WALLET_MIN_E18
}

/// Bump EIP-1559 fees by 25% (EVM replace-by-fee floor is +10%; 25% is safe).
pub fn bump_gas(prio: u128, max_fee: u128) -> (u128, u128) {
    (prio.saturating_mul(125) / 100, max_fee.saturating_mul(125) / 100)
}
```

Wire the predicates into `settlement.rs` (gas gate before Submit; `is_stuck` + `bump_gas` on the Confirm path when still pending) and `deposit_watch.rs` (`is_reorg` check after fetching the finalized block; on reorg set a per-chain `reorg_halted` flag in state and emit `ChainReorgDetected`, then skip the rest of the loop for that chain). Add a `#[serde(default)] reorg_halted: BTreeMap<ChainId, bool>` to `MultiChainStateV2` (it is a new field on V2, which is fine since V2 is new this phase; no V3 needed). Add a developer-gated `clear_reorg_halt(chain)` in Task 14.

- [ ] **Step 4: Build + run hardening tests + full lib suite**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_hardening
cargo test --package rumi_protocol_backend --lib
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/
git commit -m "feat(monad): hardening (stuck-tx bump, reorg halt, hot-wallet gate)

Phase 1b Task 11. Pure predicates wired into settlement + observer.
reorg_halted per-chain flag on MultiChainStateV2; gas bump +25%."
```

---

## Task 12: Mint Flow (Open Monad Vault)

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (new `open_chain_vault` update)
- Modify: `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs` (pure CR + open helper)
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_open_vault.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

`open_chain_vault(collateral_chain, collateral_already_deposited, debt_e8s, mint_recipient)` creates a `ChainVault` (Design B: status `MintPending`, debt NOT yet counted), validates the collateralization ratio using the manual MON price, allocates a vault_id, and enqueues a `Mint` op on the chain's settlement queue. Timer D (Task 10) signs + submits; the observed mint confirms it. The pure CR check + open-state transition are unit-tested.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_open_vault.rs`:

```rust
use super::chain_vault::{collateral_ratio_e4, open_chain_vault_in_state, OpenVaultError};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use candid::Principal;

#[test]
fn cr_computed_from_collateral_price_and_debt() {
    // 5 MON at $2.00 = $10 collateral. Debt 100 icUSD ($100). CR = 10% -> 1000 (e4).
    // 5 MON at $2 with debt of $4 -> CR 250% -> 25000 (e4).
    let cr = collateral_ratio_e4(
        5_000_000_000_000_000_000, // 5 MON e18
        2_0000_0000,               // $2.00 e8
        4_00000000,                // $4.00 debt e8s
    );
    assert_eq!(cr, 25000); // 250.00%
}

#[test]
fn open_rejects_below_min_cr() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.manual_prices.insert((ChainId(10143), "MON".into()), 2_0000_0000); // $2
    // 1 MON ($2) collateral, want 100 icUSD ($100) debt -> CR 2% -> reject.
    let res = open_chain_vault_in_state(
        &mut s, ChainId(10143), Principal::anonymous(),
        "0xcustody".into(), 1_000_000_000_000_000_000, 10_000_000_000,
        "0xrecipient".into(), /*min_cr_e4=*/ 13000, /*now=*/ 0, /*vault_id=*/ 1,
    );
    assert!(matches!(res, Err(OpenVaultError::BelowMinCr { .. })));
    assert!(s.chain_vaults.is_empty());
    assert!(s.settlement_queues.get(&ChainId(10143)).map(|q| q.pending.is_empty()).unwrap_or(true));
}

#[test]
fn open_creates_pending_vault_and_enqueues_mint() {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.settlement_queues.insert(ChainId(10143), Default::default());
    s.manual_prices.insert((ChainId(10143), "MON".into()), 100_0000_0000); // $100/MON
    // 100 MON ($10,000) collateral, 100 icUSD ($100) debt -> CR huge -> ok.
    open_chain_vault_in_state(
        &mut s, ChainId(10143), Principal::anonymous(),
        "0xcustody".into(), 100_000_000_000_000_000_000, 10_000_000_000,
        "0xrecipient".into(), 13000, 0, 7,
    ).expect("open");
    let v = &s.chain_vaults[&7];
    assert_eq!(v.pending_mint_e8s, 10_000_000_000);
    assert_eq!(v.debt_e8s, 0); // Design B: debt not counted until confirmed
    assert!(matches!(v.status, super::chain_vault::ChainVaultStatus::MintPending));
    assert_eq!(s.settlement_queues[&ChainId(10143)].pending.len(), 1);
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_open_vault;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_open_vault 2>&1 | tail -10
```

- [ ] **Step 3: Add the pure helpers to chain_vault.rs**

Append to `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs`:

```rust
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind};

#[derive(Debug, PartialEq, Eq)]
pub enum OpenVaultError {
    UnknownChain,
    NoPrice,
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    QueueError(String),
}

/// CR in basis-points-of-percent (e4): 25000 == 250.00%. Returns u64::MAX when
/// debt is zero. collateral_e18 in MON e18, price_e8 in USD e8, debt_e8s in e8s.
pub fn collateral_ratio_e4(collateral_e18: u128, price_e8: u64, debt_e8s: u128) -> u64 {
    if debt_e8s == 0 {
        return u64::MAX;
    }
    // collateral_usd_e8 = collateral_e18 * price_e8 / 1e18
    let collateral_usd_e8 = collateral_e18.saturating_mul(price_e8 as u128) / 1_000_000_000_000_000_000u128;
    // cr_e4 = collateral_usd_e8 / debt_e8s * 10000
    ((collateral_usd_e8.saturating_mul(10_000)) / debt_e8s) as u64
}

/// Create a MintPending vault + enqueue the mint op. Design B: debt is NOT
/// counted yet (pending_mint_e8s holds it until the on-chain mint confirms).
#[allow(clippy::too_many_arguments)]
pub fn open_chain_vault_in_state(
    state: &mut MultiChainStateV2,
    chain: ChainId,
    owner: candid::Principal,
    custody_address: String,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
    min_cr_e4: u64,
    now_ns: u64,
    vault_id: u64,
) -> Result<(), OpenVaultError> {
    if !state.chain_configs.contains_key(&chain) {
        return Err(OpenVaultError::UnknownChain);
    }
    let price = *state
        .manual_prices
        .get(&(chain, "MON".to_string()))
        .ok_or(OpenVaultError::NoPrice)?;
    let cr = collateral_ratio_e4(collateral_e18, price, debt_e8s);
    if cr < min_cr_e4 {
        return Err(OpenVaultError::BelowMinCr { cr_e4: cr, min_e4: min_cr_e4 });
    }
    let idempotency_key = format!("mint-{}-{}", chain.0, vault_id);
    let op = SettlementOp::new(
        SettlementOpKind::Mint { recipient: mint_recipient.clone(), amount_e8s: debt_e8s, vault_id },
        idempotency_key,
        now_ns,
    );
    let queue = state.settlement_queues.entry(chain).or_default();
    queue.enqueue(op).map_err(|e| OpenVaultError::QueueError(format!("{e:?}")))?;
    state.chain_vaults.insert(vault_id, ChainVaultV1 {
        vault_id,
        owner,
        collateral_chain: chain,
        custody_address,
        collateral_amount_e18: collateral_e18,
        debt_e8s: 0,
        mint_recipient,
        pending_mint_e8s: debt_e8s,
        status: ChainVaultStatus::MintPending,
        opened_at_ns: now_ns,
    });
    Ok(())
}
```

- [ ] **Step 4: Add the update endpoint in main.rs**

Add `open_chain_vault` (developer-gated for Phase 1b, since there is no SIWE-derived caller yet; the manual integration in Task 23 calls it as rumi_identity). It derives the custody address, allocates a vault_id (reuse the existing vault-id counter or a dedicated `chain_vault_id_counter` on State; simplest: a `#[serde(default)] chain_vault_id_counter: u64` on State, incremented here), reads `min_liquidation_ratio` for `min_cr_e4`, calls `open_chain_vault_in_state`, and records `Event::ChainMintSubmitted` is premature here (submit happens in Timer D); instead just log the open. The mint op is enqueued; Timer D handles submission.

```rust
#[candid_method(update)]
#[update]
async fn open_chain_vault(
    collateral_chain: rumi_protocol_backend::chains::config::ChainId,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // Derive custody address for (caller, chain, nonce=vault_id).
    let vault_id = mutate_state(|s| { s.chain_vault_id_counter += 1; s.chain_vault_id_counter });
    let path = rumi_protocol_backend::chains::monad::tecdsa::custody_derivation_path(collateral_chain, caller, vault_id);
    let (_, custody) = rumi_protocol_backend::chains::monad::tecdsa::derive_evm_address(path).await
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))?;
    let min_cr_e4 = read_state(|s| (s.min_liquidation_ratio() * 10_000.0) as u64);
    let now = ic_cdk::api::time();
    let res = mutate_state(|s| rumi_protocol_backend::chains::monad::chain_vault::open_chain_vault_in_state(
        &mut s.multi_chain, collateral_chain, caller, custody, collateral_e18, debt_e8s, mint_recipient, min_cr_e4, now, vault_id,
    ));
    res.map(|()| vault_id).map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
}
```

Verify `min_liquidation_ratio()` exists on State (it governs the existing vault CR floor); if the accessor differs, use the actual accessor. Add `chain_vault_id_counter` to State with `#[serde(default)]`.

- [ ] **Step 5: Build + run + commit**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_open_vault
cargo test --package rumi_protocol_backend --lib
git add src/rumi_protocol_backend/src/
git commit -m "feat(monad): open_chain_vault mint flow (Design B pending mint)

Phase 1b Task 12. Creates MintPending vault, CR-checks against manual MON
price, enqueues mint. Debt counted only on confirmation (Task 10)."
```

---

## Task 13: Repay, Withdraw, and Close Flows

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs` (withdraw + close helpers)
- Modify: `src/rumi_protocol_backend/src/main.rs` (`withdraw_chain_collateral`, `close_chain_vault` updates)
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_withdraw.rs`
- Modify: `src/rumi_protocol_backend/src/chains/monad/mod.rs`

Repay needs no new endpoint: the user burns icUSD on Monad with their own wallet (`IcUSD.burn(amount, vault_id)`), and the observer (Task 9) decrements debt. Withdraw verifies the vault is healthy (or fully repaid) and enqueues a native-MON transfer-out op; Timer D signs + submits. Close = repaid to zero + full collateral withdrawn -> status `Closed`. Pure health + enqueue helpers are unit-tested.

- [ ] **Step 1: Write the failing test FIRST**

Create `src/rumi_protocol_backend/src/chains/monad/tests_withdraw.rs`:

```rust
use super::chain_vault::{withdraw_collateral_in_state, WithdrawError, ChainVaultStatus};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use candid::Principal;

fn vault_open(s: &mut MultiChainStateV2, debt: u128, collateral_e18: u128) {
    s.chain_supplies.insert(ChainId(10143), debt);
    s.settlement_queues.insert(ChainId(10143), Default::default());
    s.manual_prices.insert((ChainId(10143), "MON".into()), 100_0000_0000); // $100/MON
    s.chain_vaults.insert(1, super::chain_vault::ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xc".into(), collateral_amount_e18: collateral_e18, debt_e8s: debt,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
    });
}

#[test]
fn full_withdraw_when_fully_repaid_closes_vault() {
    let mut s = MultiChainStateV2::default();
    vault_open(&mut s, 0, 5_000_000_000_000_000_000); // debt 0, 5 MON
    withdraw_collateral_in_state(&mut s, 1, 5_000_000_000_000_000_000, "0xdest".into(), 13000, 0)
        .expect("withdraw all");
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 0);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Closing));
    assert_eq!(s.settlement_queues[&ChainId(10143)].pending.len(), 1);
}

#[test]
fn partial_withdraw_keeping_cr_above_min_is_allowed() {
    let mut s = MultiChainStateV2::default();
    // debt 100 icUSD ($100); 5 MON ($500) collateral; withdraw 1 MON -> 4 MON ($400) still CR 400%.
    vault_open(&mut s, 10_000_000_000, 5_000_000_000_000_000_000);
    withdraw_collateral_in_state(&mut s, 1, 1_000_000_000_000_000_000, "0xdest".into(), 13000, 0)
        .expect("partial");
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 4_000_000_000_000_000_000);
    assert!(matches!(s.chain_vaults[&1].status, ChainVaultStatus::Open));
}

#[test]
fn withdraw_breaking_min_cr_is_rejected() {
    let mut s = MultiChainStateV2::default();
    // debt 100 icUSD ($100); 5 MON ($500); withdraw 4.9 MON -> $10 left -> CR 10% -> reject.
    vault_open(&mut s, 10_000_000_000, 5_000_000_000_000_000_000);
    let res = withdraw_collateral_in_state(&mut s, 1, 4_900_000_000_000_000_000, "0xdest".into(), 13000, 0);
    assert!(matches!(res, Err(WithdrawError::BelowMinCr { .. })));
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 5_000_000_000_000_000_000); // unchanged
}

#[test]
fn withdraw_exceeding_balance_is_rejected() {
    let mut s = MultiChainStateV2::default();
    vault_open(&mut s, 0, 1_000_000_000_000_000_000);
    let res = withdraw_collateral_in_state(&mut s, 1, 2_000_000_000_000_000_000, "0xd".into(), 13000, 0);
    assert!(matches!(res, Err(WithdrawError::InsufficientCollateral)));
}
```

Wire in `mod.rs`:
```rust
#[cfg(test)]
mod tests_withdraw;
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::monad::tests_withdraw 2>&1 | tail -10
```

- [ ] **Step 3: Add withdraw/close helpers to chain_vault.rs**

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum WithdrawError {
    UnknownVault,
    NoPrice,
    InsufficientCollateral,
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    QueueError(String),
}

/// Verify health post-withdraw, decrement collateral, enqueue a MON transfer-out
/// op, and flip status to Closing iff the vault is now empty + debt-free.
pub fn withdraw_collateral_in_state(
    state: &mut MultiChainStateV2,
    vault_id: u64,
    amount_e18: u128,
    dest_address: String,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), WithdrawError> {
    let (chain, debt, remaining, price) = {
        let v = state.chain_vaults.get(&vault_id).ok_or(WithdrawError::UnknownVault)?;
        if amount_e18 > v.collateral_amount_e18 {
            return Err(WithdrawError::InsufficientCollateral);
        }
        let price = *state.manual_prices.get(&(v.collateral_chain, "MON".to_string()))
            .ok_or(WithdrawError::NoPrice)?;
        (v.collateral_chain, v.debt_e8s, v.collateral_amount_e18 - amount_e18, price)
    };
    // Health check on the REMAINING collateral (skip when debt-free).
    if debt > 0 {
        let cr = collateral_ratio_e4(remaining, price, debt);
        if cr < min_cr_e4 {
            return Err(WithdrawError::BelowMinCr { cr_e4: cr, min_e4: min_cr_e4 });
        }
    }
    let idempotency_key = format!("withdraw-{}-{}-{}", chain.0, vault_id, now_ns);
    let op = SettlementOp::new(
        SettlementOpKind::Withdrawal { recipient: dest_address, amount_e8s: amount_e18 },
        idempotency_key, now_ns,
    );
    state.settlement_queues.entry(chain).or_default()
        .enqueue(op).map_err(|e| WithdrawError::QueueError(format!("{e:?}")))?;
    let v = state.chain_vaults.get_mut(&vault_id).unwrap();
    v.collateral_amount_e18 = remaining;
    if remaining == 0 && v.debt_e8s == 0 {
        v.status = ChainVaultStatus::Closing;
    }
    Ok(())
}
```

Note: `SettlementOpKind::Withdrawal.amount_e8s` is reused to carry the MON e18 amount for native transfers (the field name says e8s but for a Withdrawal op it is the native-unit amount; document this in a comment on the enum, or add a dedicated `WithdrawalNative { recipient, amount_e18 }` variant. Cleaner: add the variant. Since `SettlementOpKind` lives in the Phase 1a `settlement_queue.rs` and is inside `#[serde(default)]`-friendly V2 state, appending a variant is safe; do that and update `select_next_op`/Timer D to handle it).

- [ ] **Step 4: Add withdraw_chain_collateral + close_chain_vault endpoints in main.rs**

Both developer-gated for Phase 1b. `withdraw_chain_collateral(vault_id, amount_e18, dest_address)` calls the helper. `close_chain_vault(vault_id, dest_address)` requires `debt_e8s == 0` then withdraws the full remaining collateral. Mirror the `open_chain_vault` gating + error mapping.

- [ ] **Step 5: Build + run + commit**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::monad::tests_withdraw
cargo test --package rumi_protocol_backend --lib
git add src/rumi_protocol_backend/src/
git commit -m "feat(monad): withdraw + close flows (health-gated transfer-out)

Phase 1b Task 13. Repay is observer-driven (user burns); withdraw verifies
remaining-CR, enqueues native MON transfer-out, closes when empty+debt-free."
```

---

## Task 14: Queries + Admin Endpoints (deposit address, contract, price, delete_chain, halts)

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/src/chains/admin.rs` (delete_chain pure helper)
- Create: `src/rumi_protocol_backend/src/chains/monad/tests_queries.rs` (pure parts) / extend `tests_admin.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs`

The surface the frontend (Phase 1d) and manual testing (Task 23) need:
- `get_user_deposit_address(chain, user) -> String` (query-style, but tECDSA derivation is async, so it is an `update` that derives + returns; cache nothing)
- `get_chain_settlement_address(chain) -> String` (async update; the minter address to grant MINTER_ROLE)
- `get_chain_vault(vault_id) -> Option<ChainVault>` (query)
- `list_chain_vaults(chain) -> Vec<ChainVault>` (query, bounded)
- `set_chain_contract(chain, address)` (developer-gated; writes `chain_contracts`)
- `set_manual_collateral_price(chain, symbol, price_e8)` (developer-gated)
- `set_evm_rpc_principal(principal)` (developer-gated; the PocketIC/staging override from Task 6)
- `clear_invariant_halt()` + `clear_reorg_halt(chain)` (developer-gated recovery)
- `delete_chain(chain)` (developer-gated; cleans the chain id 999 Phase 1a debris in Task 22). Only allowed when the chain has zero supply and no open vaults.

- [ ] **Step 1: Write the failing test FIRST (delete_chain pure helper)**

Add to `src/rumi_protocol_backend/src/chains/tests_admin.rs`:

```rust
#[test]
fn delete_chain_removes_zero_supply_chain() {
    use crate::chains::admin::{delete_chain_in_state, register_chain_in_state};
    use crate::chains::config::ChainId;
    use crate::chains::multi_chain_state::MultiChainStateV2;
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, super::config_arg_999(), 0).expect("register");
    delete_chain_in_state(&mut s, ChainId(999)).expect("delete");
    assert!(!s.chain_configs.contains_key(&ChainId(999)));
    assert!(!s.chain_supplies.contains_key(&ChainId(999)));
    assert!(!s.settlement_queues.contains_key(&ChainId(999)));
}

#[test]
fn delete_chain_refuses_when_supply_nonzero() {
    use crate::chains::admin::{delete_chain_in_state, register_chain_in_state, ChainAdminError};
    use crate::chains::config::ChainId;
    use crate::chains::multi_chain_state::MultiChainStateV2;
    let mut s = MultiChainStateV2::default();
    register_chain_in_state(&mut s, super::config_arg_999(), 0).expect("register");
    s.chain_supplies.insert(ChainId(999), 1);
    let err = delete_chain_in_state(&mut s, ChainId(999)).expect_err("must refuse");
    assert!(matches!(err, ChainAdminError::InvalidConfig(_)));
}
```

Add a `config_arg_999()` test helper near the top of `tests_admin.rs` returning a `RegisterChainArg` for chain 999 with one rpc endpoint. (If `tests_admin.rs` already has an `arg()` helper, generalize it.) Note: the Phase 1a `register_chain_in_state` signature takes `&mut MultiChainStateV1`; after Task 9's repoint it takes `&mut MultiChainStateV2` (the alias). Update the existing `tests_admin.rs` `MultiChainStateV1` references to `MultiChainStateV2`.

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test --package rumi_protocol_backend --lib chains::tests_admin 2>&1 | tail -10
```

- [ ] **Step 3: Implement delete_chain_in_state**

Add to `src/rumi_protocol_backend/src/chains/admin.rs`:

```rust
/// Remove a chain entirely. Only permitted when the chain carries zero supply
/// and no chain_vaults reference it (so deletion cannot orphan debt).
pub fn delete_chain_in_state(
    state: &mut MultiChainStateV2,
    chain_id: ChainId,
) -> Result<(), ChainAdminError> {
    if !state.chain_configs.contains_key(&chain_id) {
        return Err(ChainAdminError::ChainNotRegistered(chain_id));
    }
    let supply = state.chain_supplies.get(&chain_id).copied().unwrap_or(0);
    if supply != 0 {
        return Err(ChainAdminError::InvalidConfig(format!("chain {} has nonzero supply {}", chain_id.0, supply)));
    }
    if state.chain_vaults.values().any(|v| v.collateral_chain == chain_id) {
        return Err(ChainAdminError::InvalidConfig(format!("chain {} still has vaults", chain_id.0)));
    }
    state.chain_configs.remove(&chain_id);
    state.chain_supplies.remove(&chain_id);
    state.settlement_queues.remove(&chain_id);
    state.chain_contracts.remove(&chain_id);
    state.last_observed_block.remove(&chain_id);
    state.hot_wallet_balance_e18.remove(&chain_id);
    state.reorg_halted.remove(&chain_id);
    Ok(())
}
```

Change the `admin.rs` imports from `MultiChainStateV1` to `MultiChainStateV2`.

- [ ] **Step 4: Implement the endpoints in main.rs**

Add the queries + developer-gated updates listed above. Examples:

```rust
#[candid_method(update)]
#[update]
async fn get_user_deposit_address(
    chain: rumi_protocol_backend::chains::config::ChainId,
    user: candid::Principal,
) -> Result<String, ProtocolError> {
    // Phase 1b uses nonce = 0 for the user's primary custody address. (Multiple
    // custody addresses per user is a later refinement; the vault's custody
    // address is derived with nonce = vault_id in open_chain_vault.)
    let path = rumi_protocol_backend::chains::monad::tecdsa::custody_derivation_path(chain, user, 0);
    rumi_protocol_backend::chains::monad::tecdsa::derive_evm_address(path).await
        .map(|(_, addr)| addr)
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))
}

#[candid_method(update)]
#[update]
async fn get_chain_settlement_address(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<String, ProtocolError> {
    let path = rumi_protocol_backend::chains::monad::tecdsa::settlement_derivation_path(chain);
    rumi_protocol_backend::chains::monad::tecdsa::derive_evm_address(path).await
        .map(|(_, addr)| addr)
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))
}

#[candid_method(update)]
#[update]
fn set_chain_contract(
    chain: rumi_protocol_backend::chains::config::ChainId,
    address: String,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| { s.multi_chain.chain_contracts.insert(chain, address); });
    Ok(())
}
```

Add `set_manual_collateral_price`, `set_evm_rpc_principal`, `clear_invariant_halt`, `clear_reorg_halt`, `delete_chain`, `get_chain_vault`, `list_chain_vaults` (clamp to e.g. 500 entries per the Wave-9a DOS pagination convention). Each developer-gated update mirrors the caller check + event/log pattern. `delete_chain` emits `Event::ChainDisabled` is wrong (it deletes, not disables); just `log!` it, or reuse the existing chain-admin event surface with a clear message.

- [ ] **Step 5: Build + run + commit**

```bash
cargo build --package rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo test --package rumi_protocol_backend --lib chains::tests_admin
cargo test --package rumi_protocol_backend --lib
git add src/rumi_protocol_backend/src/
git commit -m "feat(monad): query + admin surface (deposit addr, contract, price, delete_chain)

Phase 1b Task 14. get_user_deposit_address / get_chain_settlement_address
(async tECDSA), set_chain_contract, set_manual_collateral_price,
set_evm_rpc_principal, clear_invariant_halt/clear_reorg_halt, delete_chain
(zero-supply + no-vaults guard)."
```

---

## Task 15: Timer Registration (Observer + Settlement)

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs` (`setup_timers`, two new register fns, two interval setters)
- Modify: `src/rumi_protocol_backend/src/chains/monad/{settlement.rs, deposit_watch.rs}` (the timer entry async fns)
- Create: `src/rumi_protocol_backend/tests/phase1b_timers_pic.rs` (PocketIC: timers registered, no panic on empty chains)

Register the observer + settlement timers in `setup_timers()`, mirroring the Phase 1a Wave-14b timer pattern (clear-and-re-register with a tracked `TimerId`, tunable interval via a developer setter). Both loops are no-ops when no chain is registered, so they are safe to register on the staging canister even before Monad is configured.

- [ ] **Step 1: Add the timer entry fns**

In `settlement.rs`, add `pub async fn settlement_tick()` that iterates registered+enabled chains and calls `run_settlement(chain)` for each (guarded on `mode != ReadOnly`, `!invariant_halted`, `!reorg_halted[chain]`). In `deposit_watch.rs`, add `pub async fn observer_tick()` that iterates chains and calls `run_observer(chain)`.

- [ ] **Step 2: Register the timers**

In `main.rs`, mirror `register_interest_treasury_timer`:

```rust
thread_local! {
    static SETTLEMENT_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> = const { std::cell::Cell::new(None) };
    static OBSERVER_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> = const { std::cell::Cell::new(None) };
}

fn register_settlement_timer() {
    let secs = read_state(|s| s.settlement_tick_interval_secs);
    SETTLEMENT_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() { ic_cdk_timers::clear_timer(old); }
        let id = ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(rumi_protocol_backend::chains::monad::settlement::settlement_tick()));
        cell.set(Some(id));
    });
}

fn register_observer_timer() {
    let secs = read_state(|s| s.observer_tick_interval_secs);
    OBSERVER_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() { ic_cdk_timers::clear_timer(old); }
        let id = ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(rumi_protocol_backend::chains::monad::deposit_watch::observer_tick()));
        cell.set(Some(id));
    });
}
```

Call both at the end of `setup_timers()`. Add `#[serde(default)] settlement_tick_interval_secs: u64` and `observer_tick_interval_secs: u64` to State with sensible defaults (e.g. 30s each) via the `Default`/init path. Add developer-gated `set_settlement_tick_interval_secs` / `set_observer_tick_interval_secs` mirroring `set_interest_treasury_tick_interval_secs` (clear + re-register in place).

Default-interval note: a `#[serde(default)]` u64 defaults to 0, which would make `set_timer_interval(0)` fire continuously (the heartbeat-cost regression the MEMORY.md warns about). Guard: in `register_*_timer`, treat `secs == 0` as the default 30 (`let secs = read_state(...).max(1); let secs = if secs == 0 { 30 } else { secs };` -> simplest: store the default in the init/post_upgrade path explicitly so it is never 0). Verify both intervals are >= a floor so the timers never busy-loop.

- [ ] **Step 3: Write a PocketIC smoke test**

Create `src/rumi_protocol_backend/tests/phase1b_timers_pic.rs`: boot the backend (no chains), advance time a few ticks via `pic.advance_time` + `pic.tick()`, and assert the canister stays alive (a query still answers) and `get_supply_audit` is still empty. This proves the timers are safe with no chain configured. Mirror the locally-mirrored-types pattern from `phase1a_scaffolding_pic.rs`.

```bash
cargo build --target wasm32-unknown-unknown --release --package rumi_protocol_backend
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1b_timers_pic
```

Expected: passes.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/ src/rumi_protocol_backend/tests/phase1b_timers_pic.rs
git commit -m "feat(monad): register observer + settlement timers

Phase 1b Task 15. Timer D (settlement) + observer registered in setup_timers
with tracked ids + tunable intervals; no-op when no chain configured.
Interval floor guards against the 0s busy-loop regression."
```

---

## Task 16: Candid .did + TypeScript Bindings

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `declarations/rumi_protocol_backend/`
- Verify: `vault_frontend/` build

Mirror every new type + endpoint into Candid and regenerate the TS bindings (the frontend uses them in Phase 1d, but the bindings must exist + the frontend must still build now).

- [ ] **Step 1: Add the Candid types**

Edit `src/rumi_protocol_backend/rumi_protocol_backend.did`. Add (near the Phase 1a chain types):

```candid
type ChainVaultStatus = variant { MintPending; Open; Closing; Closed };

type ChainVault = record {
  vault_id : nat64;
  owner : principal;
  collateral_chain : ChainId;
  custody_address : text;
  collateral_amount_e18 : nat;
  debt_e8s : nat;
  mint_recipient : text;
  pending_mint_e8s : nat;
  status : ChainVaultStatus;
  opened_at_ns : nat64;
};
```

- [ ] **Step 2: Add the service methods**

In the `service` block:

```candid
  open_chain_vault : (ChainId, nat, nat, text) -> (variant { Ok : nat64; Err : ProtocolError });
  withdraw_chain_collateral : (nat64, nat, text) -> (variant { Ok; Err : ProtocolError });
  close_chain_vault : (nat64, text) -> (variant { Ok; Err : ProtocolError });
  get_user_deposit_address : (ChainId, principal) -> (variant { Ok : text; Err : ProtocolError });
  get_chain_settlement_address : (ChainId) -> (variant { Ok : text; Err : ProtocolError });
  get_chain_vault : (nat64) -> (opt ChainVault) query;
  list_chain_vaults : (ChainId) -> (vec ChainVault) query;
  set_chain_contract : (ChainId, text) -> (variant { Ok; Err : ProtocolError });
  set_manual_collateral_price : (ChainId, text, nat64) -> (variant { Ok; Err : ProtocolError });
  set_evm_rpc_principal : (principal) -> (variant { Ok; Err : ProtocolError });
  clear_invariant_halt : () -> (variant { Ok; Err : ProtocolError });
  clear_reorg_halt : (ChainId) -> (variant { Ok; Err : ProtocolError });
  delete_chain : (ChainId) -> (variant { Ok; Err : ProtocolError });
  set_settlement_tick_interval_secs : (nat64) -> (variant { Ok; Err : ProtocolError });
  set_observer_tick_interval_secs : (nat64) -> (variant { Ok; Err : ProtocolError });
```

- [ ] **Step 3: Verify the .did matches `__export_did_tmp__` (or candid_method export)**

If the project uses `candid::export_service!`/`__get_candid_interface_tmp_hack`, run the candid-export check the project uses (search for an existing `test` that compares the generated candid to the .did file, or a `make candid` script). Reconcile any drift so the .did matches the Rust surface exactly.

- [ ] **Step 4: Regenerate declarations + build the frontend**

```bash
npm run regenerate-declarations
grep -E "open_chain_vault|get_user_deposit_address|delete_chain|get_chain_vault" declarations/rumi_protocol_backend/rumi_protocol_backend.did.d.ts
cd vault_frontend && npm run build 2>&1 | tail -20 ; cd ..
```

Expected: every grep returns a line; frontend build succeeds (no frontend code added this phase, just confirming the new bindings do not break existing imports).

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/rumi_protocol_backend.did declarations/rumi_protocol_backend/
git commit -m "feat(monad): Candid .did + regenerated TS bindings

Phase 1b Task 16. ChainVault type + 16 new methods mirrored; frontend
build green (bindings only, no UI yet)."
```

---

## Task 17: PocketIC Happy-Path Integration Test + Mock EVM RPC Canister

**Files:**
- Create: `src/monad_rpc_mock/Cargo.toml`
- Create: `src/monad_rpc_mock/src/lib.rs`
- Modify: root `Cargo.toml` (add `src/monad_rpc_mock` to workspace `members`)
- Create: `src/rumi_protocol_backend/tests/phase1b_monad_happy_path_pic.rs`

A mock EVM RPC canister (precedent: `src/xrc_demo/xrc_mock`) lets PocketIC exercise the full chain-agnostic backend path (`register_chain` -> `open_chain_vault` -> Timer-D submit -> observed mint -> burn -> debt decrement -> withdraw) without a real Monad node. The mock implements the EVM RPC canister's method surface the wrapper calls (Task 6) and is scripted to return canned blocks, receipts, and logs. Per spec Section 3 testing tiers, tECDSA + real HTTPS outcalls are explicitly out of PocketIC scope; the mock stands in for the RPC, and the backend's `set_evm_rpc_principal` points at it.

Note on tECDSA in PocketIC: PocketIC supports the management-canister ECDSA API on a test subnet. If the installed PocketIC version's ECDSA support is unavailable or flaky, gate the signing-dependent assertions behind a `cfg`/env check and assert the queue/state transitions up to the submit boundary instead (the pure transitions are already unit-tested; the mock test's value is the end-to-end wiring). Document whichever path is taken in the test's module comment.

- [ ] **Step 1: Build the mock canister**

Create `src/monad_rpc_mock/Cargo.toml` (mirror `src/xrc_demo/xrc_mock/Cargo.toml`):

```toml
[package]
name = "monad_rpc_mock"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]
path = "src/lib.rs"

[dependencies]
candid = "0.10.6"
ic-cdk = "0.12.0"
ic-cdk-macros = "0.8.3"
serde = "1.0"
```

Create `src/monad_rpc_mock/src/lib.rs`. It must expose whatever method names + arg/return shapes the `evm_rpc_client` wrapper calls (confirm from Task 6's final implementation). Model it on the real EVM RPC canister's request/result variant shape so the wrapper's decode path is exercised. At minimum:

```rust
//! Mock EVM RPC canister for PocketIC. Scripted responses for the Monad
//! happy-path test. Mirrors the subset of the EVM RPC canister surface the
//! backend wrapper calls (Task 6). NOT a faithful EVM RPC clone; just enough
//! to drive register -> open -> mint -> burn -> withdraw.

use candid::{CandidType, Deserialize};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(CandidType, Deserialize, Clone, Default)]
struct Script {
    latest_block: u64,
    finalized_block: u64,
    // tx_hash -> (status_ok, block) once "mined"
    receipts: HashMap<String, (bool, u64)>,
    // canned logs returned by the next get_logs call: (topics, data, tx_hash, block)
    logs: Vec<(Vec<String>, String, String, u64)>,
    balances: HashMap<String, u128>,
    nonces: HashMap<String, u64>,
    next_send_hash: String,
}

thread_local! { static SCRIPT: RefCell<Script> = RefCell::new(Script::default()); }

// --- test-control endpoints (called by the PocketIC test to script behaviour) ---
#[ic_cdk_macros::update] fn set_blocks(latest: u64, finalized: u64) { SCRIPT.with(|s| { let mut s = s.borrow_mut(); s.latest_block = latest; s.finalized_block = finalized; }); }
#[ic_cdk_macros::update] fn push_log(topics: Vec<String>, data: String, tx_hash: String, block: u64) { SCRIPT.with(|s| s.borrow_mut().logs.push((topics, data, tx_hash, block))); }
#[ic_cdk_macros::update] fn set_receipt(tx_hash: String, ok: bool, block: u64) { SCRIPT.with(|s| { s.borrow_mut().receipts.insert(tx_hash, (ok, block)); }); }
#[ic_cdk_macros::update] fn set_balance(addr: String, bal: u128) { SCRIPT.with(|s| { s.borrow_mut().balances.insert(addr, bal); }); }
#[ic_cdk_macros::update] fn set_next_send_hash(h: String) { SCRIPT.with(|s| s.borrow_mut().next_send_hash = h); }

// --- EVM RPC surface the backend wrapper calls ---
// Implement these to MATCH the method names + candid shapes the wrapper uses.
// (Names below are illustrative; align with the evm_rpc_client request shape.)
#[ic_cdk_macros::update] fn eth_get_block_numbers() -> (u64, u64) { SCRIPT.with(|s| { let s = s.borrow(); (s.latest_block, s.finalized_block) }) }
#[ic_cdk_macros::update] fn eth_get_balance(addr: String) -> u128 { SCRIPT.with(|s| s.borrow().balances.get(&addr).copied().unwrap_or(0)) }
#[ic_cdk_macros::update] fn eth_get_transaction_count(addr: String) -> u64 { SCRIPT.with(|s| s.borrow().nonces.get(&addr).copied().unwrap_or(0)) }
#[ic_cdk_macros::update] fn eth_get_logs(_contract: String, _topic0: String, _from: u64, _to: u64) -> Vec<(Vec<String>, String, String, u64)> { SCRIPT.with(|s| s.borrow().logs.clone()) }
#[ic_cdk_macros::update] fn eth_get_transaction_receipt(tx_hash: String) -> Option<(bool, u64)> { SCRIPT.with(|s| s.borrow().receipts.get(&tx_hash).copied()) }
#[ic_cdk_macros::update] fn eth_send_raw_transaction(_raw: String) -> String { SCRIPT.with(|s| { let mut s = s.borrow_mut(); let h = s.next_send_hash.clone(); /* auto-mine */ s.receipts.insert(h.clone(), (true, s.finalized_block)); h }) }
#[ic_cdk_macros::update] fn eth_fee_history() -> (u128, u128) { (1_000_000_000, 2_000_000_000) }
```

IMPORTANT reconciliation: Task 6's wrapper either calls the REAL EVM RPC canister methods (e.g. `eth_getLogs`, `request`) with the real candid shapes, or these simplified mock methods. To keep the mock simple, add a thin indirection in the wrapper: behind the `evm_rpc_override()` principal, call these mock method names; against the real canister, call the real surface. Cleanest: define a small internal trait/enum in `evm_rpc.rs` with two backends (real vs mock) selected by whether the override is set. Document this in `evm_rpc.rs`. This keeps the mock trivial and the test meaningful for the state machine, accepting that the real-canister candid path is exercised only on staging (Task 23), consistent with the spec's PocketIC-scope caveat.

Add to root `Cargo.toml` workspace `members`: `"src/monad_rpc_mock"`.

- [ ] **Step 2: Write the happy-path test**

Create `src/rumi_protocol_backend/tests/phase1b_monad_happy_path_pic.rs`. Boot backend + mock (mirror `phase1a_scaffolding_pic.rs` local-type-mirroring), then:

1. Install both canisters; `set_evm_rpc_principal(mock_id)` on the backend (as rumi_identity, which is the `developer_principal` in the test init).
2. `register_chain(10143, ...)`; `set_chain_contract(10143, "0xIcUSD")`; `set_manual_collateral_price(10143, "MON", $2e8)`.
3. Script the mock: blocks (latest=finalized=100), settlement-address balance above the gas floor, `set_next_send_hash("0xmint1")`.
4. `open_chain_vault(10143, collateral_e18 = 100 MON, debt_e8s = 100 icUSD, "0xrecipient")` -> returns vault_id; assert vault is `MintPending`, `get_global_icusd_supply()` still 0 (Design B).
5. Tick the settlement timer (`pic.advance_time` + `pic.tick()` enough rounds). The mock auto-mines `0xmint1`. Then `push_log` a Mint log for vault_id at finalized block, tick the observer/settlement confirm path.
6. Assert `get_chain_vault(vault_id).status == Open`, `debt_e8s == 100e8`, `get_global_icusd_supply() == 100e8`, `get_supply_audit()` shows Monad supply 100e8.
7. `push_log` a Burn log (vault_id, amount 40 icUSD) at a new finalized block; tick observer. Assert `debt_e8s == 60e8`, `get_global_icusd_supply() == 60e8`.
8. Burn the remaining 60 (push Burn log), tick. Assert debt 0, supply 0.
9. `withdraw_chain_collateral(vault_id, full collateral, "0xdest")`; `set_next_send_hash("0xwd1")`; tick settlement. Assert vault `Closing`/`Closed`, supply still 0.
10. Final invariant assertion: `get_global_icusd_supply() == 0` and `get_supply_audit().total == 0`.

- [ ] **Step 3: Build the wasms + run**

```bash
cargo build --target wasm32-unknown-unknown --release --package rumi_protocol_backend
cargo build --target wasm32-unknown-unknown --release --package monad_rpc_mock
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1b_monad_happy_path_pic 2>&1 | tail -30
```

Expected: the full deposit->mint->burn->withdraw cycle passes with the supply invariant holding at every assertion. If ECDSA is unavailable in the installed PocketIC, the test asserts up to the submit boundary per the Step 1 note.

- [ ] **Step 4: Commit**

```bash
git add src/monad_rpc_mock/ Cargo.toml src/rumi_protocol_backend/tests/phase1b_monad_happy_path_pic.rs
git commit -m "test(monad): PocketIC happy-path + mock EVM RPC canister

Phase 1b Task 17. monad_rpc_mock drives register->open->mint->burn->withdraw;
supply invariant asserted at every step. tECDSA/real-RPC out of PocketIC
scope per spec (staging covers them, Task 23)."
```

---

## Task 18: Property Tests for Real-Flow Supply Invariant

**Files:**
- Modify: `src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs`

Extend the Phase 1a proptest harness so the random op sequence drives the REAL state helpers (`open_chain_vault_in_state`, `confirm_mint_in_state`, `apply_burn_to_state`) against actual `chain_vaults` + `settlement_queues`, not just bare `apply_supply_delta`. This proves the invariant survives the full mint(open+confirm) -> burn lifecycle backed by settlement-queue state, covering the spec's "Mint -> Bridge -> Burn" property requirement (bridge = burn-on-X + mint-on-Y; modeled as a burn followed by an open+confirm on another chain).

- [ ] **Step 1: Add the real-flow property test**

Append a second `proptest!` block to `src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs`:

```rust
use rumi_protocol_backend::chains::monad::chain_vault::{open_chain_vault_in_state, ChainVaultStatus};
use rumi_protocol_backend::chains::monad::settlement::confirm_mint_in_state;
use rumi_protocol_backend::chains::monad::deposit_watch::apply_burn_to_state;
use rumi_protocol_backend::chains::monad::evm_rpc::BurnLog;

#[derive(Clone, Debug)]
enum RealOp {
    OpenAndConfirm { chain: u32, amount: u64 },
    BurnPartial { vault_id: u64, frac_pct: u8 },
}

proptest! {
    #[test]
    fn invariant_holds_across_open_confirm_burn(ops in proptest::collection::vec(arb_real_op(), 0..30)) {
        let mut state = seeded_state(); // reuse the Phase 1a helper; chains 1..=5 registered, supplies 0
        // ensure settlement queues + manual prices exist for each chain
        for id in 1u32..=5u32 {
            state.settlement_queues.insert(rumi_protocol_backend::chains::config::ChainId(id), Default::default());
            state.manual_prices.insert((rumi_protocol_backend::chains::config::ChainId(id), "MON".into()), 100_0000_0000);
        }
        let mut total_debt: u128 = 0;
        let mut next_vault_id: u64 = 0;
        let mut open_vaults: Vec<u64> = vec![];

        for op in ops {
            match op {
                RealOp::OpenAndConfirm { chain, amount } => {
                    next_vault_id += 1;
                    let cid = rumi_protocol_backend::chains::config::ChainId(chain);
                    // huge collateral so CR never binds in this invariant test
                    let collateral = 1_000_000_000_000_000_000_000_000u128;
                    let amt = amount as u128;
                    if open_chain_vault_in_state(&mut state, cid, candid::Principal::anonymous(),
                        format!("0x{next_vault_id}"), collateral, amt, "0xr".into(), 0, 0, next_vault_id).is_ok() {
                        let new_total = total_debt + amt;
                        if confirm_mint_in_state(&mut state, cid, next_vault_id, amt, new_total).is_ok() {
                            total_debt = new_total;
                            open_vaults.push(next_vault_id);
                        }
                    }
                }
                RealOp::BurnPartial { vault_id, frac_pct } => {
                    if open_vaults.is_empty() { continue; }
                    let vid = open_vaults[(vault_id as usize) % open_vaults.len()];
                    let debt = state.chain_vaults[&vid].debt_e8s;
                    if debt == 0 { continue; }
                    let amount = (debt * (frac_pct.min(100) as u128)) / 100;
                    if amount == 0 { continue; }
                    let chain = state.chain_vaults[&vid].collateral_chain;
                    let new_total = total_debt - amount;
                    let burn = BurnLog { vault_id: vid, amount_e8s: amount, tx_hash: "0xb".into(), block_number: 1 };
                    if apply_burn_to_state(&mut state, &burn, new_total).is_ok() {
                        total_debt = new_total;
                    }
                }
            }
            let sum: u128 = state.chain_supplies.values().copied().sum();
            prop_assert_eq!(sum, total_debt);
        }
    }
}

fn arb_real_op() -> impl Strategy<Value = RealOp> {
    prop_oneof![
        (1u32..=5u32, 1u64..=1_000_000u64).prop_map(|(c, a)| RealOp::OpenAndConfirm { chain: c, amount: a }),
        (0u64..50, 0u8..=100u8).prop_map(|(v, f)| RealOp::BurnPartial { vault_id: v, frac_pct: f }),
    ]
}
```

Confirm `seeded_state()` from the Phase 1a harness now returns a `MultiChainStateV2` (the alias changed in Task 3); if it explicitly names `MultiChainStateV1`, update it. The harness's `mod` visibility may need `pub` on the chain_vault/settlement/deposit_watch helper fns (they are already `pub`).

- [ ] **Step 2: Run**

```bash
cargo test --package rumi_protocol_backend --test multi_chain_supply_invariant 2>&1 | tail -15
```

Expected: both the Phase 1a `apply_supply_delta` property test and the new real-flow property test pass (256 cases each).

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/tests/multi_chain_supply_invariant.rs
git commit -m "test(monad): supply invariant across open->confirm->burn real flow

Phase 1b Task 18. Property test drives the real chain_vault + settlement
helpers; sum(chain_supplies) == total_debt after every op."
```

---

## Task 19: Foundry Scaffold + IcUSD.sol

**Files:**
- Create: `foundry/foundry.toml`
- Create: `foundry/.gitignore`
- Create: `foundry/src/IcUSD.sol`
- Create: `foundry/remappings.txt` (or rely on `foundry.toml`)
- Add OpenZeppelin via `forge install`

Solidity work lives under `foundry/` at the project root (NOT under `src/`, per conventions). `IcUSD.sol` is an OpenZeppelin ERC-20 + AccessControl, 8 decimals (1 base unit == 1 e8s), with the canister settlement address as sole `MINTER_ROLE` holder. `burn` is public (any holder can repay).

- [ ] **Step 1: Scaffold the Foundry project**

```bash
cd /Users/robertripley/coding/rumi-protocol-v2
mkdir -p foundry
cd foundry
forge init --no-git --no-commit .
forge install OpenZeppelin/openzeppelin-contracts --no-git
cd ..
```

If `forge init` refuses on a non-empty dir, init in a temp dir and move `foundry.toml`/`lib`/`script`/`test` into `foundry/`. Confirm `forge --version` works (install Foundry via `foundryup` if absent).

Write `foundry/foundry.toml`:

```toml
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
solc_version = "0.8.24"
optimizer = true
optimizer_runs = 200

[rpc_endpoints]
monad_testnet = "${MONAD_TESTNET_RPC}"
```

Write `foundry/remappings.txt`:

```
@openzeppelin/=lib/openzeppelin-contracts/
```

Add to `foundry/.gitignore`:

```
out/
cache/
broadcast/
.env
```

Note: do NOT gitignore `lib/` if you want the OZ source vendored; alternatively add `lib/` to `.gitignore` and document that `forge install` must run before building. Recommended for this repo: gitignore `lib/` and document the install step (keeps the diff small; OZ is a well-known dep).

- [ ] **Step 2: Write IcUSD.sol**

Create `foundry/src/IcUSD.sol`:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {AccessControl} from "@openzeppelin/contracts/access/AccessControl.sol";

/// @title IcUSD - Rumi icUSD on Monad
/// @notice Canister-minted ERC-20. The Rumi backend canister's tECDSA-derived
/// settlement address holds MINTER_ROLE and is the sole minter. Any holder may
/// burn to repay their vault. Uses 8 decimals so 1 base unit == 1 e8s, keeping
/// the ICP-side supply accounting 1:1 with on-chain amounts.
///
/// totalSupply() reflects Monad-side circulation only. The canonical all-chains
/// total is get_global_icusd_supply() on the Rumi backend (ICP).
contract IcUSD is ERC20, AccessControl {
    bytes32 public constant MINTER_ROLE = keccak256("MINTER_ROLE");

    /// @param vault_id the Rumi vault this mint backs
    event Mint(uint256 vault_id, address recipient, uint256 amount);
    /// @param vault_id the Rumi vault this burn repays
    event Burn(uint256 vault_id, address burner, uint256 amount);

    /// @param admin receives DEFAULT_ADMIN_ROLE (the canister settlement address
    ///        for Phase 1b; rotate to an SNS-controlled admin later)
    /// @param minter receives MINTER_ROLE (the canister settlement address)
    constructor(address admin, address minter) ERC20("Rumi icUSD", "icUSD") {
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(MINTER_ROLE, minter);
    }

    function decimals() public pure override returns (uint8) {
        return 8;
    }

    /// @notice Mint icUSD. Only the canister settlement address (MINTER_ROLE).
    function mint(address to, uint256 amount, uint64 vault_id) external onlyRole(MINTER_ROLE) {
        _mint(to, amount);
        emit Mint(uint256(vault_id), to, amount);
    }

    /// @notice Burn icUSD to repay vault `target_vault_id`. Callable by anyone
    /// holding the tokens. The Rumi backend observes the Burn event and
    /// decrements the vault's debt + the Monad chain supply.
    function burn(uint256 amount, uint64 target_vault_id) external {
        _burn(msg.sender, amount);
        emit Burn(uint256(target_vault_id), msg.sender, amount);
    }
}
```

- [ ] **Step 3: Build**

```bash
cd foundry && forge build 2>&1 | tail -20 ; cd ..
```

Expected: compiles clean on solc 0.8.24.

- [ ] **Step 4: Commit (force-add; foundry/ may be under a gitignored path? verify)**

`foundry/` is under the project root, not `docs/`, so normal `git add` works (only `docs/` and `.claude/` are gitignored). Confirm with `git status foundry/`.

```bash
git add foundry/foundry.toml foundry/remappings.txt foundry/.gitignore foundry/src/IcUSD.sol
git commit -m "feat(monad): IcUSD.sol (OZ ERC-20, 8 decimals, MINTER_ROLE)

Phase 1b Task 19. Canister settlement address is sole minter; public burn
emits Burn(vault_id, burner, amount). 8 decimals = e8s 1:1."
```

---

## Task 20: Foundry Test Suite for IcUSD.sol

**Files:**
- Create: `foundry/test/IcUSD.t.sol`

Cover mint/burn invariants, access control, total-supply consistency, decimals, and event emission.

- [ ] **Step 1: Write the test suite**

Create `foundry/test/IcUSD.t.sol`:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {IcUSD} from "../src/IcUSD.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";

contract IcUSDTest is Test {
    IcUSD icusd;
    address admin = address(0xA11CE);
    address minter = address(0xB0B);   // stands in for the canister settlement address
    address alice = address(0xCAFE);
    address bob = address(0xBEEF);

    event Mint(uint256 vault_id, address recipient, uint256 amount);
    event Burn(uint256 vault_id, address burner, uint256 amount);

    function setUp() public {
        icusd = new IcUSD(admin, minter);
    }

    function test_decimals_is_8() public view {
        assertEq(icusd.decimals(), 8);
    }

    function test_minter_can_mint_and_emits_event() public {
        vm.expectEmit(true, true, true, true);
        emit Mint(42, alice, 10_000_000_000); // 100 icUSD
        vm.prank(minter);
        icusd.mint(alice, 10_000_000_000, 42);
        assertEq(icusd.balanceOf(alice), 10_000_000_000);
        assertEq(icusd.totalSupply(), 10_000_000_000);
    }

    function test_non_minter_cannot_mint() public {
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, alice, icusd.MINTER_ROLE())
        );
        icusd.mint(alice, 1, 1);
    }

    function test_anyone_can_burn_their_balance_and_emits_event() public {
        vm.prank(minter);
        icusd.mint(alice, 10_000_000_000, 7);
        vm.expectEmit(true, true, true, true);
        emit Burn(7, alice, 4_000_000_000);
        vm.prank(alice);
        icusd.burn(4_000_000_000, 7);
        assertEq(icusd.balanceOf(alice), 6_000_000_000);
        assertEq(icusd.totalSupply(), 6_000_000_000);
    }

    function test_burn_exceeding_balance_reverts() public {
        vm.prank(minter);
        icusd.mint(alice, 100, 1);
        vm.prank(alice);
        vm.expectRevert(); // ERC20InsufficientBalance
        icusd.burn(101, 1);
    }

    function test_total_supply_tracks_mint_minus_burn() public {
        vm.startPrank(minter);
        icusd.mint(alice, 1_000, 1);
        icusd.mint(bob, 2_000, 2);
        vm.stopPrank();
        assertEq(icusd.totalSupply(), 3_000);
        vm.prank(bob);
        icusd.burn(500, 2);
        assertEq(icusd.totalSupply(), 2_500);
    }

    function testFuzz_mint_then_full_burn_nets_zero(uint96 amount, uint64 vaultId) public {
        vm.assume(amount > 0);
        vm.prank(minter);
        icusd.mint(alice, amount, vaultId);
        vm.prank(alice);
        icusd.burn(amount, vaultId);
        assertEq(icusd.balanceOf(alice), 0);
        assertEq(icusd.totalSupply(), 0);
    }

    function test_standard_erc20_transfer_approve() public {
        vm.prank(minter);
        icusd.mint(alice, 1_000, 1);
        vm.prank(alice);
        icusd.transfer(bob, 400);
        assertEq(icusd.balanceOf(bob), 400);
        vm.prank(bob);
        icusd.approve(alice, 100);
        assertEq(icusd.allowance(bob, alice), 100);
    }
}
```

- [ ] **Step 2: Run the Foundry tests**

```bash
cd foundry && forge test -vv 2>&1 | tail -30 ; cd ..
```

Expected: all tests + the fuzz test pass. If the OZ `AccessControlUnauthorizedAccount` selector differs (OZ v4 used a string revert, v5 uses the custom error), adjust the `expectRevert` to match the installed OZ major version.

- [ ] **Step 3: Commit**

```bash
git add foundry/test/IcUSD.t.sol
git commit -m "test(monad): Foundry suite for IcUSD.sol

Phase 1b Task 20. mint/burn invariants, access control, total-supply
consistency, decimals, events, fuzz mint->burn nets zero."
```

---

## Task 21: Foundry Deploy Script

**Files:**
- Create: `foundry/script/DeployIcUSD.s.sol`
- Create: `foundry/.env.example`
- Create: `foundry/DEPLOY.md` (runbook)

The deploy script publishes `IcUSD.sol` to Monad testnet with the canister settlement address as both admin and minter, then prints the deployed address. The address gets written into backend state via `set_chain_contract` in Task 22 (NOT a Rust constant; the runtime store survives upgrades).

- [ ] **Step 1: Write the deploy script**

Create `foundry/script/DeployIcUSD.s.sol`:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console2} from "forge-std/Script.sol";
import {IcUSD} from "../src/IcUSD.sol";

/// Deploy IcUSD to Monad testnet. The canister settlement address (derived via
/// get_chain_settlement_address on the backend, Task 22) is BOTH admin + minter
/// for Phase 1b. Set env vars before running:
///   MONAD_TESTNET_RPC  - Monad testnet RPC URL
///   DEPLOYER_PK        - a funded testnet deployer private key (pays gas)
///   CANISTER_SETTLEMENT_ADDR - the backend's settlement address (0x..)
contract DeployIcUSD is Script {
    function run() external {
        address settlement = vm.envAddress("CANISTER_SETTLEMENT_ADDR");
        uint256 deployerPk = vm.envUint("DEPLOYER_PK");
        vm.startBroadcast(deployerPk);
        IcUSD icusd = new IcUSD(settlement, settlement);
        vm.stopBroadcast();
        console2.log("IcUSD deployed at:", address(icusd));
        console2.log("admin + minter:", settlement);
    }
}
```

Create `foundry/.env.example`:

```
MONAD_TESTNET_RPC=https://testnet-rpc.monad.xyz
DEPLOYER_PK=0xyour_funded_testnet_deployer_key
CANISTER_SETTLEMENT_ADDR=0xfilled_from_get_chain_settlement_address
```

- [ ] **Step 2: Write the deploy runbook**

Create `foundry/DEPLOY.md` documenting the exact ordering (this is executed in Task 22, captured here so the steps live with the contract):

```markdown
# IcUSD Monad-testnet deploy

PREREQUISITE: the backend's settlement address must be derived FIRST (it is the
minter), so the contract is owned by the canister from block zero.

1. On the backend (staging), derive the settlement address:
   dfx canister call --network ic <STAGING_ID> get_chain_settlement_address '(record { 0 = 10143 : nat32 })' --identity rumi_identity
   (returns 0x.. ; this is CANISTER_SETTLEMENT_ADDR)
2. Fund a testnet deployer key with Monad testnet MON (faucet).
3. cp .env.example .env  and fill all three vars.
4. forge script script/DeployIcUSD.s.sol:DeployIcUSD \
     --rpc-url monad_testnet --broadcast -vvvv
5. Record the "IcUSD deployed at:" address.
6. Fund the CANISTER_SETTLEMENT_ADDR with testnet MON (it pays gas for mints +
   withdrawals; this is the hot wallet from Task 11).
7. On the backend: set_chain_contract(10143, "<deployed address>").
```

- [ ] **Step 3: Confirm the script compiles**

```bash
cd foundry && forge build 2>&1 | tail -10 ; cd ..
```

Expected: compiles (no broadcast in CI; just the build).

- [ ] **Step 4: Commit**

```bash
git add foundry/script/DeployIcUSD.s.sol foundry/.env.example foundry/DEPLOY.md
git commit -m "feat(monad): Foundry deploy script + runbook for IcUSD

Phase 1b Task 21. Deploys with canister settlement address as admin+minter;
runbook pins the derive-first ordering so the canister owns the token from
block zero."
```

---

## Task 22: Staging Deploy + Monad Configuration

**Files:**
- Modify: `icp.yaml` (staging upgrade-arg description; no shape change)
- No repo code changes beyond the upgrade arg.

Deploy the Phase 1b wasm to the staging canister (`kvg63-wiaaa-aaaao-bbabq-cai`), clean the chain-999 debris, deploy `IcUSD.sol`, and wire the chain config. The production canister is NOT touched. Use the locked icp-cli deploy pattern.

- [ ] **Step 1: Pre-flight - resolve dfx, confirm production untouched, confirm cycles**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
STAGING_ID=$(jq -r '.rumi_protocol_backend["ic-staging"]' canister_ids.json)
echo "staging: $STAGING_ID"   # expect kvg63-wiaaa-aaaao-bbabq-cai
# Production hash must be unchanged from cc9bb33f... (we never deploy to it):
"$DFX" canister info --network ic tfesu-vyaaa-aaaap-qrd7a-cai | grep "Module hash"
# Staging cycle balance (Phase 1b adds tECDSA + RPC outcalls; budget below):
"$DFX" canister status --network ic $STAGING_ID --identity rumi_identity 2>&1 | grep -iE "cycles|status"
```

Expected: production hash `0xcc9bb33f...`; staging currently Stopped at `0xe60c8c13...` with ~3.37T cycles.

- [ ] **Step 2: Cycles budget pass + top-up**

Estimate Phase 1b's marginal burn (this is the cycles-budget pass the scope calls for):
- tECDSA `sign_with_ecdsa`: ~26B cycles per signature (mainnet `test_key_1`). Each mint = 1 sig; each withdrawal = 1 sig. A handful of manual-test ops = a few hundred B cycles.
- `ecdsa_public_key`: cheaper, but every `get_user_deposit_address`/`get_chain_settlement_address` call costs an outcall-style fee.
- EVM RPC canister calls: the EVM RPC canister charges per request (HTTPS-outcall cost passthrough, ~few hundred M to low-B cycles per call depending on response size). The observer + settlement timers poll continuously: at 30s intervals, ~2,880 ticks/day/timer. Each tick with one registered chain makes 1-3 RPC calls. Rough: ~10-50B cycles/day of RPC polling once Monad is registered. Document the real number in Task 23.
- Idle storage: ~0.07T/day (the Phase 1a runway figure).

Top up to a comfortable margin before enabling Monad (the continuous polling is the new sustained cost). Convert ICP -> cycles and deposit:

```bash
# Top up to ~10T to cover the first weeks of polling + signing during testing.
"$DFX" ledger top-up $STAGING_ID --network ic --amount 1.0 --identity rumi_identity
"$DFX" canister status --network ic $STAGING_ID --identity rumi_identity 2>&1 | grep -i cycles
```

(Adjust the ICP amount to reach ~10T; 1 ICP ~= 1.7T at the time of writing. Use the `cycles-management` skill for the current conversion + the CMC flow.)

- [ ] **Step 3: Build + install the Phase 1b wasm on staging (upgrade mode)**

This is an UPGRADE (the canister already carries the Phase 1a wasm). Per the deploy-pattern note + the MEMORY.md rule, NEVER reinstall (that wipes state); always upgrade. Update the `icp.yaml` staging upgrade description first:

```yaml
      # (in the mainnet-staging environment block, switch to the Upgrade variant
      #  for this and all subsequent staging deploys)
      rumi_protocol_backend: |
        (variant { Upgrade = record {
          mode = null;
          description = opt "Phase 1b: Monad adapter (deposit/mint/burn/withdraw) + chain_vaults V2"
        } })
```

Then build + install:

```bash
icp build rumi_protocol_backend
icp canister install rumi_protocol_backend \
  --environment mainnet-staging \
  --identity rumi_identity \
  --mode upgrade \
  --args '(variant { Upgrade = record { mode = null; description = opt "Phase 1b: Monad adapter + chain_vaults V2" } })'
```

The pre-deploy hook fires + runs the test suite. The canister is currently Stopped; if `icp canister install` requires it Running, start it first:

```bash
"$DFX" canister start --network ic $STAGING_ID --identity rumi_identity
```

- [ ] **Step 4: Verify the upgrade preserved state + bumped the hash**

```bash
"$DFX" canister info --network ic $STAGING_ID | grep "Module hash"   # expect a NEW hash, not e60c8c13
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply # expect (0 : nat)
"$DFX" canister call --network ic $STAGING_ID get_supply_audit        # chain 999 still present (disabled), supply 0
```

If `get_supply_audit` lost the chain-999 entry or trapped, the V1->V2 upgrade wiped state: STOP (this is the AMM-state-wipe signature; the PocketIC test in Task 3 should have caught it).

- [ ] **Step 5: Clean the chain-999 Phase 1a debris**

```bash
"$DFX" canister call --network ic $STAGING_ID delete_chain '(record { 0 = 999 : nat32 })' --identity rumi_identity
"$DFX" canister call --network ic $STAGING_ID get_supply_audit   # chain 999 gone
```

Expected: `delete_chain` returns Ok (chain 999 has zero supply + no vaults); the audit no longer lists it.

- [ ] **Step 6: Derive the settlement address + deploy IcUSD.sol**

```bash
"$DFX" canister call --network ic $STAGING_ID get_chain_settlement_address '(record { 0 = 10143 : nat32 })' --identity rumi_identity
# -> 0x.. ; this is CANISTER_SETTLEMENT_ADDR
```

Follow `foundry/DEPLOY.md`: fund a deployer key, set `.env`, `forge script ... --broadcast`, record the deployed IcUSD address, and fund the settlement address with testnet MON (the hot wallet that pays for mints/withdrawals).

- [ ] **Step 7: Register Monad + wire config + set MON price**

```bash
# register_chain with the Monad testnet config (verify finality depth on testnet first; start at 1):
"$DFX" canister call --network ic $STAGING_ID register_chain '(record {
  chain_id = record { 0 = 10143 : nat32 };
  display_name = "MonadTestnet";
  rpc_endpoints = vec { "https://testnet-rpc.monad.xyz" };
  finality_depth = 1 : nat32;
  gas_strategy = variant { EvmEip1559 = record { max_priority_fee_gwei = 2 : nat64; max_fee_gwei_ceiling = 500 : nat64 } };
  chain_native_decimals = 18 : nat8;
})' --identity rumi_identity

# point at the deployed IcUSD:
"$DFX" canister call --network ic $STAGING_ID set_chain_contract '(record { 0 = 10143 : nat32 }, "0x<deployed IcUSD>")' --identity rumi_identity

# set the manual MON/USD price (e8s); use the current MON testnet reference price:
"$DFX" canister call --network ic $STAGING_ID set_manual_collateral_price '(record { 0 = 10143 : nat32 }, "MON", 2_0000_0000 : nat64)' --identity rumi_identity

# confirm the EVM RPC principal points at the real canister (default), not a mock:
"$DFX" canister call --network ic $STAGING_ID set_evm_rpc_principal '(principal "7hfb6-caaaa-aaaar-qadga-cai")' --identity rumi_identity
```

- [ ] **Step 8: Confirm timers are live + Monad observed**

Wait ~2 minutes (a few observer ticks), then:

```bash
"$DFX" canister call --network ic $STAGING_ID get_supply_audit   # Monad present, supply 0
"$DFX" canister logs --network ic $STAGING_ID 2>&1 | tail -30    # observer/settlement ticks, last_observed_block advancing
```

**Decision gate (Gate 3 - staging configured).** Pause. Confirm with Rob: the new module hash, state preserved (supply 0, chain 999 deleted, Monad registered), the deployed IcUSD address + its MINTER_ROLE = settlement address, the settlement address funded with MON, and the observer advancing `last_observed_block` without errors. Get explicit "go" before the first real mint.

- [ ] **Step 9: Commit the icp.yaml description change**

```bash
git add icp.yaml
git commit -m "chore(monad): staging upgrade-arg description for Phase 1b deploy

Phase 1b Task 22. mainnet-staging switches to Upgrade variant. Production
(mainnet-live) untouched."
```

---

## Task 23: Manual End-to-End Integration on Staging

**Files:**
- None (on-chain + off-chain manual flow). Captures observations for the PR + memory.

Run the full happy path against the live staging canister + Monad testnet + Rob's MetaMask, and record the cycle/MON burn.

- [ ] **Step 1: Get Rob's MetaMask address + a custody deposit address**

```bash
ROB_EVM="0x<Rob's MetaMask address>"
# Rob's principal for the call (rumi_identity is the developer; open_chain_vault
# is developer-gated in Phase 1b, so call as rumi_identity and pass mint_recipient = ROB_EVM):
"$DFX" canister call --network ic $STAGING_ID get_user_deposit_address '(record { 0 = 10143 : nat32 }, principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae")' --identity rumi_identity
# -> 0xCUSTODY ; send MON collateral here from MetaMask
```

- [ ] **Step 2: Deposit collateral**

From MetaMask, send testnet MON (e.g. 5 MON) to `0xCUSTODY`. Wait for finality, then confirm the observer credited it (the vault does not exist yet; for Phase 1b the open call carries the collateral amount explicitly, and the deposit watch verifies the custody address actually holds it before the mint is allowed to confirm). Confirm via logs that `DepositObserved` fired.

- [ ] **Step 3: Open the vault (enqueues the mint)**

```bash
# 5 MON collateral at $2 = $10; mint 5 icUSD ($5) -> CR 200% (above the floor).
"$DFX" canister call --network ic $STAGING_ID open_chain_vault '(record { 0 = 10143 : nat32 }, 5_000_000_000_000_000_000 : nat, 500_000_000 : nat, "0x<ROB_EVM>")' --identity rumi_identity
# -> (variant { Ok = <vault_id> })
VAULT_ID=<returned>
"$DFX" canister call --network ic $STAGING_ID get_chain_vault "($VAULT_ID : nat64)"  # status MintPending, debt 0, pending 5e8
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply                  # still 0 (Design B)
```

- [ ] **Step 4: Watch Timer D sign + submit + confirm the mint**

Wait for the settlement + observer ticks (1-2 minutes). Watch logs for `ChainMintSubmitted` then `ChainMintConfirmed`. Then:

```bash
"$DFX" canister call --network ic $STAGING_ID get_chain_vault "($VAULT_ID : nat64)"   # status Open, debt 5e8, pending 0
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply                   # 5e8
"$DFX" canister call --network ic $STAGING_ID get_supply_audit                          # Monad supply 5e8
```

In MetaMask (add the IcUSD token by its address), confirm `0x<ROB_EVM>` shows a 5 icUSD balance. On a Monad testnet explorer, confirm the Mint tx + event.

- [ ] **Step 5: Repay (burn) from MetaMask**

Call `IcUSD.burn(200000000, VAULT_ID)` from MetaMask (2 icUSD = 200000000 at 8 decimals). Wait for finality, watch logs for `ChainBurnObserved`, then:

```bash
"$DFX" canister call --network ic $STAGING_ID get_chain_vault "($VAULT_ID : nat64)"   # debt 3e8
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply                   # 3e8
```

- [ ] **Step 6: Burn the rest, then withdraw collateral + close**

Burn the remaining 3 icUSD from MetaMask. Confirm debt 0, supply 0. Then:

```bash
"$DFX" canister call --network ic $STAGING_ID withdraw_chain_collateral "($VAULT_ID : nat64, 5_000_000_000_000_000_000 : nat, \"0x<ROB_EVM>\")" --identity rumi_identity
# settlement signs the MON transfer-out; wait for confirmation, watch for WithdrawalSigned
"$DFX" canister call --network ic $STAGING_ID get_chain_vault "($VAULT_ID : nat64)"   # status Closing/Closed, collateral 0
```

Confirm in MetaMask that the 5 MON returned to `0x<ROB_EVM>`.

- [ ] **Step 7: Document the cycle + MON burn**

```bash
# Cycle balance before vs after the full cycle (capture at start of Task 23 and now):
"$DFX" canister status --network ic $STAGING_ID --identity rumi_identity 2>&1 | grep -i cycles
```

Record: cycles consumed by the full deposit->mint->burn->withdraw cycle, the steady-state polling burn rate (cycles/hour with Monad registered), and the MON gas spent by the settlement address. These numbers go in the PR + the memory note.

**Decision gate (Gate 4 - integration confirmed).** Pause. Confirm with Rob the full cycle succeeded, the supply invariant held throughout (`get_global_icusd_supply` tracked debt at every step), and the observed cycle/MON burn is acceptable. Get explicit "go" before opening the merge PR. If anything failed mid-cycle, debug on staging (the canister can be left Running; no rollback needed since production is untouched) before proceeding.

---

## Task 24: End-of-Phase Verification, Memory Update, and PR

**Files:**
- Modify: MEMORY.md + a new topic file (memory dir)

- [ ] **Step 1: Confirm the end-of-phase invariants**

```bash
DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"
STAGING_ID=$(jq -r '.rumi_protocol_backend["ic-staging"]' canister_ids.json)

# 1. Production UNCHANGED (the critical check):
"$DFX" canister info --network ic tfesu-vyaaa-aaaap-qrd7a-cai | grep "Module hash"   # cc9bb33f...

# 2. Staging carries the Phase 1b wasm (new hash), Monad registered, supply 0 after the test cycle closed:
"$DFX" canister info --network ic $STAGING_ID | grep "Module hash"
"$DFX" canister call --network ic $STAGING_ID get_global_icusd_supply                 # 0 (vault closed)
"$DFX" canister call --network ic $STAGING_ID get_supply_audit                        # Monad present, supply 0

# 3. The supply invariant self-check has not halted the protocol:
"$DFX" canister call --network ic $STAGING_ID get_protocol_status 2>&1 | grep -iE "mode|read_only" || true
```

- [ ] **Step 2: Run the full local test suite one last time**

```bash
cargo test --package rumi_protocol_backend --lib
cargo test --package rumi_protocol_backend --test multi_chain_supply_invariant
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1a_scaffolding_pic
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1b_timers_pic
POCKET_IC_BIN=./pocket-ic cargo test --package rumi_protocol_backend --test phase1b_monad_happy_path_pic
cd foundry && forge test ; cd ..
```

Expected: all green.

- [ ] **Step 3: Update MEMORY.md**

Append under "## Follow-ups" in `/Users/robertripley/.claude/projects/-Users-robertripley-coding-rumi-protocol-v2/memory/MEMORY.md` (one line, under ~200 chars):

```markdown
- [Phase 1b Monad adapter deployed to staging](project_multi_chain_phase_1b_monad_staging.md) - YYYY-MM-DD. Staging kvg63 carries Monad adapter; deposit->mint->burn->withdraw verified end-to-end on Monad testnet. Production untouched. Next: Phase 1c liquidations + bridge.
```

Create the topic file `project_multi_chain_phase_1b_monad_staging.md` in the memory dir with: date, staging principal + new wasm hash, deployed IcUSD address, settlement address, the cycle/MON burn numbers from Task 23, the Design B / chain_vaults / onlyRole decisions locked at Gate 1, and the chain-999 cleanup. Use the Phase 1a staging note as the tone/detail template. If the cross-directory write is blocked, hand the content to Rob to apply.

- [ ] **Step 4: Push + open the PR**

The plan file itself was committed on `feat/multi-chain-phase-1b-plan` (this planning branch). The IMPLEMENTATION branch is `feat/multi-chain-phase-1b`. Push it and open the PR:

```bash
git push -u origin feat/multi-chain-phase-1b
gh pr create --title "Phase 1b: Monad adapter + happy path" --body "$(cat <<'EOF'
## Summary

Wires Monad testnet into the Phase 1a multi-chain scaffolding per `docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md` (Sub-phase 1b). First real chain integration: a Monad-collateral vault opens, mints icUSD on Monad, repays (burn), and closes, end-to-end, with every cross-chain tx signed by the staging canister via tECDSA. Production (`tfesu-vyaaa-aaaap-qrd7a-cai`) is untouched.

## What ships
- `chains/monad/` adapter: config, tECDSA derivation, EVM RPC wrapper, EIP-1559 tx build+sign, ChainAdapter impl, inbound observer (deposit + Burn watch), outbound settlement worker (Timer D), operational hardening (stuck-tx bump, reorg halt, hot-wallet gas gate)
- `MultiChainStateV2` (chain_vaults, chain_contracts, manual_prices, observer cursors, hot-wallet cache, reorg flags) via versioned migration from V1
- Vault flows: open_chain_vault (Design B pending-mint), withdraw/close, repay via burn observer
- Query + admin surface: get_user_deposit_address, get_chain_settlement_address, set_chain_contract, set_manual_collateral_price, set_evm_rpc_principal, clear_invariant_halt/clear_reorg_halt, delete_chain
- `IcUSD.sol` (OZ ERC-20, 8 decimals, canister settlement address as sole MINTER_ROLE) + Foundry test suite + deploy script
- PocketIC happy-path test (monad_rpc_mock canister) + real-flow supply-invariant property test

## Staging deploy
- Canister `kvg63-wiaaa-aaaao-bbabq-cai`, new wasm hash `<fill>`
- Monad testnet (chain id 10143) registered; IcUSD at `<address>`; settlement address `<address>`
- Full deposit->mint->burn->withdraw cycle verified on Monad testnet (Task 23)
- Cycle burn: `<fill>` for the test cycle; `<fill>` cycles/hour steady-state polling. MON gas: `<fill>`
- Production `tfesu-...` module hash UNCHANGED (`0xcc9bb33f...`)

## Decisions locked (Gate 1)
- Design B confirmed-supply: chain_supplies + debt move only on observed mint/burn at finality
- Monad vaults in a parallel `chain_vaults` map (core Vault struct untouched; unification deferred to Phase 2)
- Mint = onlyRole(MINTER_ROLE), canister settlement address as msg.sender
- Native MON collateral + developer-set manual MON/USD price for 1b (Pyth deferred to 1c+)
- tECDSA key `test_key_1` on staging (`key_1` for Phase 2 production)

## Non-goals (Phase 1c/1d/2/3+)
- Liquidations, keeper market, SP backstop, DEX swap, LiquidationRouter.sol (1c)
- User-facing cross-chain bridge (1c)
- Frontend route / MetaMask connector / SIWE (1d)
- Production deploy (2); Solana/Ethereum/L2 (3+)

## Test plan
- [x] Unit tests: tECDSA address derivation (k=1 vector), EIP-1559 RLP vector, log decoders, supply/CR helpers, drain state machine, hardening predicates
- [x] proptest: supply invariant across open->confirm->burn real flow
- [x] PocketIC: timers safe with no chain; happy-path deposit->mint->burn->withdraw via mock RPC
- [x] Foundry: mint/burn/access-control/total-supply + fuzz
- [x] Staging manual integration on Monad testnet (Task 23)
- [x] Production module hash unchanged

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr view --json url -q .url
```

- [ ] **Step 5: Surface the PR URL to Rob. Do NOT merge.**

**Phase 1b is complete when this PR merges.** Phase 1c (liquidations + bridge) writes its own plan against the same staging canister.

---

## Self-Review (run by the plan author before opening the plan PR)

**Spec coverage:** Every Sub-phase 1b bullet maps to tasks: Monad adapter (deposit watch T9, settlement T10, admin T14) -> T2-T14; `IcUSD.sol` to Monad testnet via Foundry -> T19-T22; deposit->borrow->repay->close end-to-end on staging -> T12, T13, T23; production untouched -> asserted in T22 Step 1, T24 Step 1. Spec Section 3 hardening: error categories (T6 RPC, T7/T10 signing retry, T11 stuck-tx + reorg + gas-out, T3 state-wipe versioning, T10 nonce serialization) all covered. Supply invariant enforcement (T9/T10 apply_supply_delta, T18 property tests). Oracle strategy: manual MON price for 1b (T14), Pyth deferred (noted). Testing tiers 1-5 all present.

**Placeholder scan:** The `unimplemented!()` markers in T6/T7 are explicitly flagged as call-shape placeholders the executor replaces before committing (the surrounding pure logic is fully specified + tested). The `<keccak ...>` topic constants in T6 are computed + pinned by the T8 test. The `<fill>` markers in the T24 PR body are runtime values (hashes, addresses, cycle numbers) captured during execution. No silent TODOs.

**Type consistency:** `apply_supply_delta`/`check_invariant` repoint from `MultiChainStateV1` to `MultiChainStateV2` in T9 (called out, with the Phase 1a test-file updates). `SettlementOpKind` gains a native-withdrawal variant in T13; `SettlementOp` gains `last_tx_hash` in T10 (both serde-default-safe). `ChainVaultV1`/`ChainVaultStatus` field names are consistent across T3/T9/T10/T12/T13. `MultiChainStateV2` field set is fixed in T3 and only `reorg_halted` is added (T11, same phase, no V3). Event variant names match between T4 (definition) and the emitting tasks.

**Decision gates:** Four gates (T1 architecture, T3 state migration, T22 staging configured, T23 integration) each have a recommendation and an explicit "go" requirement, consistent with the memory rule to never deploy/merge without authorization.

---

## Execution Handoff

This plan is for review BEFORE execution. After Rob approves + merges the plan PR, a separate session executes it. At that point, offer the execution choice:

**Two execution options:**
1. **Subagent-Driven (recommended):** dispatch a fresh subagent per task (superpowers:subagent-driven-development), review between tasks, fast iteration. Well-suited here: most tasks are self-contained TDD units. The four decision gates become hard checkpoints where the orchestrator stops for Rob.
2. **Inline Execution:** execute tasks in-session (superpowers:executing-plans), batch with checkpoints at the four gates.

Either way: the four decision gates are mandatory stops, the staging deploy (T22) never touches `mainnet-live`, and every deploy uses `--identity rumi_identity` + the locked icp-cli pattern.







