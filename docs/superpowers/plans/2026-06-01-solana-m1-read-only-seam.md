# Solana M1 (Read-Only Seam) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove, from the `rumi_protocol_backend` canister on Solana devnet, that we can derive a threshold-Ed25519 Solana address and read balances + the icUSD SPL mint account via the SOL RPC canister, all on the project's ic-cdk 0.12 pin, building to wasm32.

**Architecture:** A new `chains/solana/` module mirroring `chains/monad/`, hand-rolling the SOL RPC seam (raw `call_with_payment128` to the SOL RPC canister with candid types mirrored from the live .did) and threshold Ed25519 (raw management-canister `schnorr_public_key` call). No signing of transactions, no vaults, no timers (those are M2+). Dev-gated read endpoints exercise the seam on devnet.

**Tech Stack:** Rust, ic-cdk 0.12, candid 0.10, `bs58` for base58, `serde_json` (already a dep), PocketIC 6.0 for integration tests. The SOL RPC canister speaks the 3-arg consensus-wrapped pattern `(RpcSources, opt RpcConfig, Params) -> Multi*Result`.

**Reference (read before starting):**
- `docs/superpowers/specs/2026-06-01-solana-integration-design.md` (the design)
- `docs/icp-solana-integration-playbook.md` (the seam field guide; #1-#4 apply since we hand-roll)
- `src/rumi_protocol_backend/src/chains/monad/tecdsa.rs` (template for `ted25519.rs`)
- `src/rumi_protocol_backend/src/chains/monad/evm_rpc.rs` (template for `sol_rpc.rs`, esp. `call_evm_rpc` at lines 532-580 and the candid-types block at 116-170)
- `src/rumi_protocol_backend/src/chains/monad/config.rs` (template for `solana/config.rs`)

**Scope note vs the spec:** the spec's M1 bullet mentions "bootstrap durable nonce." That requires signing a transaction, so it moves to the FIRST task of M2 (it needs `tx.rs` + `sign_with_schnorr`). M1 is derivation + reads only.

**Build/test commands used throughout:**
- Unit tests: `cargo test -p rumi_protocol_backend solana`
- Native build: `cargo build -p rumi_protocol_backend`
- Wasm build (the deploy target): `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`

---

## File Structure

- Create: `src/rumi_protocol_backend/src/chains/solana/mod.rs` — module decls + re-exports
- Create: `src/rumi_protocol_backend/src/chains/solana/config.rs` — chain id, decimals, key name, default registration arg
- Create: `src/rumi_protocol_backend/src/chains/solana/ted25519.rs` — Ed25519 address derivation + schnorr candid structs
- Create: `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs` — hand-rolled SOL RPC wrapper (reads)
- Create: `src/rumi_protocol_backend/src/chains/solana/tests_config.rs`
- Create: `src/rumi_protocol_backend/src/chains/solana/tests_ted25519.rs`
- Create: `src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs` — add `pub mod solana;`
- Modify: `src/rumi_protocol_backend/Cargo.toml` — add `bs58`
- Modify: `src/rumi_protocol_backend/src/main.rs` — dev-gated read endpoints
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` — declare the new endpoints
- Create: `src/rumi_protocol_backend/tests/solana_m1_seam_pic.rs` — PocketIC smoke test

---

## Task 1: Solana module scaffold, config, and wasm32 baseline

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/solana/mod.rs`
- Create: `src/rumi_protocol_backend/src/chains/solana/config.rs`
- Create: `src/rumi_protocol_backend/src/chains/solana/tests_config.rs`
- Modify: `src/rumi_protocol_backend/src/chains/mod.rs`
- Modify: `src/rumi_protocol_backend/Cargo.toml`

- [ ] **Step 1: Add the `bs58` dependency**

In `src/rumi_protocol_backend/Cargo.toml`, under `[dependencies]`, after the `hex = "0.4"` line, add:

```toml
bs58 = "0.5"
```

- [ ] **Step 2: Write `config.rs`**

Create `src/rumi_protocol_backend/src/chains/solana/config.rs`:

```rust
//! Solana devnet configuration defaults.
//!
//! The deployed icUSD SPL mint address is NOT a static here; it lives in
//! runtime state (`MultiChainState.chain_contracts`) so it survives upgrades,
//! exactly like the Monad icUSD contract.

use crate::chains::config::{ChainId, GasStrategy, RegisterChainArg};

/// Internal multi-chain key for Solana. Solana has no EVM-style numeric chain
/// id, so we use its SLIP-44 coin type (501) as a stable, mnemonic internal key.
/// The actual network (devnet/mainnet) is selected by the RPC cluster in
/// `sol_rpc`, not by this number.
pub const SOLANA_CHAIN_ID: ChainId = ChainId(501);

/// icUSD is 8 decimals on every chain (1 base unit == 1 e8s).
pub const SOLANA_ICUSD_DECIMALS: u8 = 8;

/// Native SOL is 9 decimals (lamports).
pub const SOL_NATIVE_DECIMALS: u8 = 9;

/// Threshold-Ed25519 key name. `test_key_1` is the mainnet test key (Ed25519 has
/// NO local dfx key); switch to `key_1` for the production rollout. Derived
/// addresses differ per key, so the SPL mint authority must be derived with this
/// exact key.
pub fn solana_schnorr_key_name() -> String {
    "test_key_1".to_string()
}

/// Default registration payload for Solana devnet. `rpc_endpoints` is left empty
/// because the SOL RPC canister addresses devnet via `RpcSources::Default(Devnet)`
/// (built-in providers), not per-URL like the Monad EVM path.
pub fn solana_default_register_arg() -> RegisterChainArg {
    RegisterChainArg {
        chain_id: SOLANA_CHAIN_ID,
        display_name: "SolanaDevnet".to_string(),
        rpc_endpoints: vec![],
        // Solana finality is a commitment level (`finalized`), not block depth.
        // We keep depth 0 and read at `finalized`; see sol_rpc.rs.
        finality_depth: 0,
        gas_strategy: GasStrategy::SolanaPriorityFee {
            lamports_per_cu_ceiling: 10_000,
        },
        chain_native_decimals: SOL_NATIVE_DECIMALS,
    }
}
```

- [ ] **Step 3: Write `mod.rs`**

Create `src/rumi_protocol_backend/src/chains/solana/mod.rs`:

```rust
//! Solana integration (mirrors `chains::monad`). M1 ships config, Ed25519
//! address derivation, and read-only SOL RPC access. Signing, vaults, and
//! timers land in M2+.

pub mod config;
pub mod sol_rpc;
pub mod ted25519;

#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_sol_rpc;
#[cfg(test)]
mod tests_ted25519;
```

(`sol_rpc` and `ted25519` are created in later tasks; add their `mod` lines now so the tree compiles once they exist. To keep this task compiling on its own, temporarily comment the `sol_rpc` and `ted25519` lines, then uncomment them in Tasks 2 and 4. Simpler: create empty stub files now.)

Create stub `src/rumi_protocol_backend/src/chains/solana/ted25519.rs` with `// filled in Task 2` and stub `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs` with `// filled in Task 4`, plus empty stub test files, so the module tree compiles.

- [ ] **Step 4: Wire the module into `chains/mod.rs`**

In `src/rumi_protocol_backend/src/chains/mod.rs`, after the `pub mod monad;` line (line 20), add:

```rust
pub mod solana;
```

- [ ] **Step 5: Write the failing config test**

Create `src/rumi_protocol_backend/src/chains/solana/tests_config.rs`:

```rust
use super::config::*;
use crate::chains::config::ChainId;

#[test]
fn solana_chain_id_is_slip44() {
    assert_eq!(SOLANA_CHAIN_ID, ChainId(501));
}

#[test]
fn solana_decimals_are_correct() {
    assert_eq!(SOL_NATIVE_DECIMALS, 9);
    assert_eq!(SOLANA_ICUSD_DECIMALS, 8);
}

#[test]
fn default_register_arg_matches_constants() {
    let arg = solana_default_register_arg();
    assert_eq!(arg.chain_id, SOLANA_CHAIN_ID);
    assert_eq!(arg.chain_native_decimals, SOL_NATIVE_DECIMALS);
    assert!(arg.rpc_endpoints.is_empty());
}

#[test]
fn key_name_is_test_key_1() {
    assert_eq!(solana_schnorr_key_name(), "test_key_1");
}
```

- [ ] **Step 6: Run the tests, verify they pass**

Run: `cargo test -p rumi_protocol_backend solana::config`
Expected: 4 tests pass.

- [ ] **Step 7: Verify the wasm32 baseline builds**

Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
Expected: builds clean (this is the baseline before adding any Solana primitive crates in Task 3).

- [ ] **Step 8: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/solana/ src/rumi_protocol_backend/src/chains/mod.rs src/rumi_protocol_backend/Cargo.toml
git commit -m "feat(solana): scaffold chains/solana module + devnet config

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Ed25519 address derivation (pure helpers + schnorr candid structs)

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/solana/ted25519.rs`
- Modify: `src/rumi_protocol_backend/src/chains/solana/tests_ted25519.rs`

- [ ] **Step 1: Write the failing tests**

Replace `src/rumi_protocol_backend/src/chains/solana/tests_ted25519.rs` with:

```rust
use super::ted25519::*;
use candid::Principal;

#[test]
fn all_zero_pubkey_is_system_program_address() {
    // 32 zero bytes base58-encode to the 32-char System Program address.
    let addr = solana_address_from_pubkey(&[0u8; 32]).unwrap();
    assert_eq!(addr, "11111111111111111111111111111111");
}

#[test]
fn address_roundtrips_through_base58() {
    let pk = [7u8; 32];
    let addr = solana_address_from_pubkey(&pk).unwrap();
    let decoded = bs58::decode(&addr).into_vec().unwrap();
    assert_eq!(decoded, pk.to_vec());
}

#[test]
fn wrong_length_pubkey_rejected() {
    assert!(solana_address_from_pubkey(&[0u8; 31]).is_err());
    assert!(solana_address_from_pubkey(&[0u8; 33]).is_err());
}

#[test]
fn is_valid_solana_address_accepts_32_byte_base58() {
    assert!(is_valid_solana_address("11111111111111111111111111111111"));
    let good = solana_address_from_pubkey(&[42u8; 32]).unwrap();
    assert!(is_valid_solana_address(&good));
}

#[test]
fn is_valid_solana_address_rejects_evm_and_junk() {
    assert!(!is_valid_solana_address("0x0000000000000000000000000000000000000000"));
    assert!(!is_valid_solana_address("not base58 !!!"));
    // base58 of 31 bytes -> wrong length
    let short = bs58::encode([1u8; 31]).into_string();
    assert!(!is_valid_solana_address(&short));
}

#[test]
fn derivation_paths_are_distinct_and_structured() {
    let chain = crate::chains::config::ChainId(501);
    let settle = settlement_derivation_path(chain);
    let custody = custody_derivation_path(chain, Principal::anonymous(), 0);
    assert_ne!(settle, custody);
    assert_eq!(settle[0], 501u32.to_le_bytes().to_vec());
}
```

- [ ] **Step 2: Run, verify it fails to compile**

Run: `cargo test -p rumi_protocol_backend solana::ted25519`
Expected: FAIL (functions not defined).

- [ ] **Step 3: Implement `ted25519.rs`**

Replace `src/rumi_protocol_backend/src/chains/solana/ted25519.rs` with:

```rust
//! Threshold Ed25519 (Schnorr) address derivation for Solana.
//!
//! ic-cdk 0.12 has no `management_canister::schnorr` module, so the management
//! canister is called directly via `call_with_payment128` with the candid
//! structs hand-mirrored below (verified against the management canister .did).
//! Mirrors `chains::monad::tecdsa` (which uses the built-in `ecdsa` module).

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};

use super::config::solana_schnorr_key_name;

// ─── Derivation paths (mirror tecdsa) ───────────────────────────────────────

/// Per-user collateral custody address path: `[chain_id LE, principal, nonce LE]`.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Per-chain settlement (mint-authority) address path.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
}

// ─── Pure encoding helpers ──────────────────────────────────────────────────

/// A Solana address is the base58 of the 32-byte Ed25519 public key (no hashing).
pub fn solana_address_from_pubkey(pubkey: &[u8]) -> Result<String, String> {
    if pubkey.len() != 32 {
        return Err(format!("expected 32-byte Ed25519 pubkey, got {}", pubkey.len()));
    }
    Ok(bs58::encode(pubkey).into_string())
}

/// True iff `s` base58-decodes to exactly 32 bytes.
pub fn is_valid_solana_address(s: &str) -> bool {
    match bs58::decode(s).into_vec() {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

// ─── Management-canister Schnorr candid structs (hand-mirrored) ──────────────
// Source: management canister .did. ic-cdk 0.12 lacks the typed `schnorr`
// module, so we define the minimal surface and call by name.

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum SchnorrAlgorithm {
    #[serde(rename = "ed25519")]
    Ed25519,
    #[serde(rename = "bip340secp256k1")]
    Bip340Secp256k1,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrKeyId {
    pub algorithm: SchnorrAlgorithm,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrPublicKeyArgument {
    pub canister_id: Option<Principal>,
    pub derivation_path: Vec<Vec<u8>>,
    pub key_id: SchnorrKeyId,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SchnorrPublicKeyResponse {
    pub public_key: Vec<u8>,
    pub chain_code: Vec<u8>,
}

fn key_id() -> SchnorrKeyId {
    SchnorrKeyId { algorithm: SchnorrAlgorithm::Ed25519, name: solana_schnorr_key_name() }
}

/// Cycles attached to a `schnorr_public_key` call. Public-key derivation is far
/// cheaper than signing; this is generous headroom (unused cycles are refunded).
const SCHNORR_PUBKEY_CYCLES: u128 = 100_000_000;

/// Async: derive the Ed25519 public key from the management canister and return
/// both the raw 32-byte pubkey and its base58 Solana address.
pub async fn derive_solana_address(
    derivation_path: Vec<Vec<u8>>,
) -> Result<(Vec<u8>, String), String> {
    let arg = SchnorrPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: key_id(),
    };
    let (res,): (SchnorrPublicKeyResponse,) = ic_cdk::api::call::call_with_payment128(
        Principal::management_canister(),
        "schnorr_public_key",
        (arg,),
        SCHNORR_PUBKEY_CYCLES,
    )
    .await
    .map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    let addr = solana_address_from_pubkey(&res.public_key)?;
    Ok((res.public_key, addr))
}
```

- [ ] **Step 4: Run the tests, verify they pass**

Run: `cargo test -p rumi_protocol_backend solana::ted25519`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/solana/ted25519.rs src/rumi_protocol_backend/src/chains/solana/tests_ted25519.rs
git commit -m "feat(solana): Ed25519 address derivation + schnorr candid structs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: De-risk spike — do the lightweight `solana-*` crates compile to wasm32?

This decides `tx.rs`'s strategy in M2 (use real Solana instruction crates vs hand-encode). Do it now while M1 is cheap to change.

**Files:**
- Modify (temporarily): `src/rumi_protocol_backend/Cargo.toml`

- [ ] **Step 1: Add the probe dependencies**

In `Cargo.toml` `[dependencies]`, temporarily add:

```toml
solana-pubkey = { version = "4", default-features = false }
solana-instruction = { version = "4", default-features = false }
```

- [ ] **Step 2: Attempt the wasm32 build**

Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`

- [ ] **Step 3: Record the outcome and decide**

- If it builds: keep these crates; M2 `tx.rs` uses `solana_pubkey::Pubkey` and `solana_instruction::Instruction` for message/instruction building. Add a note to the spec Section 6.3.
- If it fails (e.g. `getrandom`/`std`/`curve25519` feature pulls a non-wasm dep): remove both lines, and record in the spec Section 6.3 that M2 `tx.rs` hand-encodes instructions (SPL MintTo, ATA, System Transfer, AdvanceNonce) using `bs58` + manual byte layout. Capture the exact error in the commit message.

Note the cycle/size impact: these crates add to the wasm size; confirm the gzipped wasm still fits the install limit (the project already `ic-wasm shrink` + `gzip` for analytics; check the backend wasm size after).

- [ ] **Step 4: Commit the decision**

```bash
git add src/rumi_protocol_backend/Cargo.toml
git commit -m "chore(solana): M1 spike - solana-* wasm32 compat outcome

<paste: builds clean | fails with <error>; tx.rs strategy decided>

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: SOL RPC wrapper — candid types + `get_balance`

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs`
- Modify: `src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs`

- [ ] **Step 1: Confirm the live candid**

Fetch the live SOL RPC .did and confirm the inner result types before hand-typing (playbook #1):

Run: `gh api repos/dfinity/sol-rpc-canister/contents/canister/sol_rpc_canister.did --jq '.download_url' | xargs curl -s | sed -n '1,200p'`

Confirm/adjust the exact shapes of `GetBalanceResult`, `RpcError`, `Pubkey`, `Lamports`, `Slot` against the structs below. The 3-arg pattern `(RpcSources, opt RpcConfig, GetBalanceParams) -> MultiGetBalanceResult` and the `Consistent`/`Inconsistent` wrapper are already verified.

- [ ] **Step 2: Write the failing tests (pure consensus + parse helpers)**

Replace `src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs` with:

```rust
use super::sol_rpc::*;

#[test]
fn consistent_ok_yields_value() {
    let r = MultiGetBalanceResult::Consistent(GetBalanceResult::Ok(1_000_000_000));
    assert_eq!(lamports_from_balance_result(r).unwrap(), 1_000_000_000);
}

#[test]
fn consistent_err_is_error() {
    let r = MultiGetBalanceResult::Consistent(GetBalanceResult::Err(RpcError::Text(
        "boom".to_string(),
    )));
    assert!(lamports_from_balance_result(r).is_err());
}

#[test]
fn inconsistent_is_rejected_for_reads() {
    // Reads demand agreement (playbook #4): Inconsistent => Err, even if an arm is Ok.
    let r = MultiGetBalanceResult::Inconsistent(vec![]);
    assert!(lamports_from_balance_result(r).is_err());
}
```

- [ ] **Step 3: Run, verify it fails**

Run: `cargo test -p rumi_protocol_backend solana::sol_rpc`
Expected: FAIL (types not defined).

- [ ] **Step 4: Implement `sol_rpc.rs`**

Replace `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs` with (adjust inner types per Step 1 if the live .did differs):

```rust
//! Hand-rolled SOL RPC canister wrapper (reads only in M1). Mirrors
//! `chains::monad::evm_rpc`: raw `call_with_payment128` with candid types
//! mirrored from the live .did. We avoid `sol_rpc_client`/`sol_rpc_types`
//! (they require ic-cdk 0.20; this project is pinned to 0.12).
//!
//! Consensus: reads demand agreement (Consistent only). `Inconsistent` => Err
//! (playbook #4). Reads use commitment `finalized`.

use candid::{CandidType, Deserialize, Principal};
use crate::state::read_state;

/// Production SOL RPC canister principal (fiduciary subnet). VERIFY against the
/// live repo before mainnet; a developer-gated state override points at a mock
/// in PocketIC / staging.
const SOL_RPC_PRINCIPAL: &str = "tghme-zyaaa-aaaar-qarca-cai";

/// Cycles attached per SOL RPC call. The docs suggest ~1-5B per request; 10B
/// gives headroom and unused cycles are refunded.
pub const SOL_RPC_CALL_CYCLES: u128 = 10_000_000_000;

// ─── Candid types mirrored from sol_rpc_canister.did ────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum SolanaCluster { Mainnet, Devnet, Testnet }

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcSources {
    Default(SolanaCluster),
    // `Custom(Vec<RpcSource>)` exists in the .did but is unused in M1.
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold { total: Option<u8>, min: u8 },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcConfig {
    pub responseSizeEstimate: Option<u64>,
    pub responseConsensus: Option<ConsensusStrategy>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum CommitmentLevel {
    #[serde(rename = "processed")] Processed,
    #[serde(rename = "confirmed")] Confirmed,
    #[serde(rename = "finalized")] Finalized,
}

/// Pubkey is candid `text` (base58) in the .did.
pub type Pubkey = String;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBalanceParams {
    pub pubkey: Pubkey,
    pub commitment: Option<CommitmentLevel>,
    pub minContextSlot: Option<u64>,
}

/// RpcError — minimal surface (confirm full shape against the live .did).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    Text(String),
}

/// `GetBalanceResult = variant { Ok : nat64; Err : RpcError }` (lamports).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum GetBalanceResult {
    Ok(u64),
    Err(RpcError),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MultiGetBalanceResult {
    Consistent(GetBalanceResult),
    Inconsistent(Vec<(super::sol_rpc::RpcSourceTag, GetBalanceResult)>),
}

/// Placeholder for the `RpcSource` tag in `Inconsistent` arms; we never read it
/// (reads reject any Inconsistent), so a permissive decode is fine.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RpcSourceTag {}

// ─── Pure consensus extraction (unit-tested) ────────────────────────────────

/// Extract lamports from a balance result, demanding provider agreement.
pub fn lamports_from_balance_result(r: MultiGetBalanceResult) -> Result<u64, String> {
    match r {
        MultiGetBalanceResult::Consistent(GetBalanceResult::Ok(v)) => Ok(v),
        MultiGetBalanceResult::Consistent(GetBalanceResult::Err(e)) => {
            Err(format!("rpc error: {e:?}"))
        }
        MultiGetBalanceResult::Inconsistent(_) => {
            Err("providers disagree (Inconsistent) on getBalance".to_string())
        }
    }
}

// ─── Async network call ─────────────────────────────────────────────────────

fn sol_rpc_principal() -> Principal {
    read_state(|s| s.sol_rpc_override())
        .unwrap_or_else(|| Principal::from_text(SOL_RPC_PRINCIPAL).expect("valid SOL RPC principal"))
}

/// Read a SOL balance (lamports) at `finalized`, demanding provider agreement.
pub async fn get_balance(pubkey: &str) -> Result<u64, String> {
    let sources = RpcSources::Default(SolanaCluster::Devnet);
    let config: Option<RpcConfig> = Some(RpcConfig {
        responseSizeEstimate: None,
        responseConsensus: Some(ConsensusStrategy::Equality),
    });
    let params = GetBalanceParams {
        pubkey: pubkey.to_string(),
        commitment: Some(CommitmentLevel::Finalized),
        minContextSlot: None,
    };
    let (res,): (MultiGetBalanceResult,) = ic_cdk::api::call::call_with_payment128(
        sol_rpc_principal(),
        "getBalance",
        (sources, config, params),
        SOL_RPC_CALL_CYCLES,
    )
    .await
    .map_err(|(code, msg)| format!("getBalance call error {code:?}: {msg}"))?;
    lamports_from_balance_result(res)
}
```

- [ ] **Step 5: Add the `sol_rpc_override` state accessor**

The Monad path has `evm_rpc_override()` on `State`. Add the Solana analogue. Find the existing `evm_rpc_override` (grep: `grep -rn "fn evm_rpc_override" src/rumi_protocol_backend/src/`) and add right beside it a parallel field + accessor:

In `state.rs`, beside the EVM override field, add `pub sol_rpc_principal_override: Option<Principal>` (with `#[serde(default)]` if `State` is a versioned snapshot, matching how `evm_rpc_principal_override` is declared), and:

```rust
pub fn sol_rpc_override(&self) -> Option<Principal> {
    self.sol_rpc_principal_override
}
```

(Match the exact pattern of `evm_rpc_principal_override` / `evm_rpc_override` already in the file, including any state-version handling.)

- [ ] **Step 6: Run tests + builds**

Run: `cargo test -p rumi_protocol_backend solana::sol_rpc`
Expected: 3 tests pass.
Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
Expected: builds clean.

- [ ] **Step 7: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs src/rumi_protocol_backend/src/state.rs
git commit -m "feat(solana): hand-rolled SOL RPC wrapper + getBalance (read seam)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: SOL RPC `get_account_info` + SPL mint supply decode

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs`
- Modify: `src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs`

- [ ] **Step 1: Write the failing test for the SPL mint supply parser**

Append to `src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs`:

```rust
#[test]
fn parse_mint_supply_reads_offset_36_le() {
    // Classic SPL Token Mint is 82 bytes; supply is u64 LE at offset 36.
    let mut buf = vec![0u8; 82];
    let supply: u64 = 123_456_789;
    buf[36..44].copy_from_slice(&supply.to_le_bytes());
    assert_eq!(parse_mint_supply(&buf).unwrap(), supply);
}

#[test]
fn parse_mint_supply_rejects_short_buffer() {
    assert!(parse_mint_supply(&[0u8; 10]).is_err());
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p rumi_protocol_backend solana::sol_rpc::parse_mint_supply`
Expected: FAIL (function not defined).

- [ ] **Step 3: Implement `parse_mint_supply` + `get_account_info`**

Append to `src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs`:

```rust
/// Decode the `supply` field (u64 LE at offset 36) of a classic SPL Token Mint
/// account's raw data. The mint layout: [0..4] mint_authority COption tag,
/// [4..36] mint_authority, [36..44] supply, [44] decimals, ...
pub fn parse_mint_supply(account_data: &[u8]) -> Result<u64, String> {
    if account_data.len() < 44 {
        return Err(format!("mint account too short: {} bytes", account_data.len()));
    }
    let mut le = [0u8; 8];
    le.copy_from_slice(&account_data[36..44]);
    Ok(u64::from_le_bytes(le))
}
```

For `get_account_info`: mirror `get_balance`, calling `getAccountInfo` with `GetAccountInfoParams { pubkey, commitment: Finalized, encoding: Some(Base64), dataSlice: None, minContextSlot: None }`. Define `GetAccountInfoParams`, `GetAccountInfoEncoding` (with a `base64` variant), `AccountInfo` (the fields you need: `data` as `(String, GetAccountInfoEncoding)` or a base64 string per the live .did), `GetAccountInfoResult = variant { Ok: opt AccountInfo; Err: RpcError }`, and `MultiGetAccountInfoResult`, all confirmed against the live .did (Step 1 of Task 4). Then:

```rust
/// Read the icUSD SPL mint's on-chain `supply` (e8s) at `finalized`.
pub async fn get_mint_supply(mint_pubkey: &str) -> Result<u64, String> {
    let info = get_account_info(mint_pubkey).await?
        .ok_or_else(|| "mint account not found".to_string())?;
    let raw = base64_decode_account_data(&info)?; // base64 -> bytes per the .did encoding
    parse_mint_supply(&raw)
}
```

(Implement `get_account_info` and `base64_decode_account_data` mirroring `get_balance`'s consensus handling. Use `serde_json`'s base64 or add the `base64` crate only if `base64` is not already transitively available; prefer decoding via the candid-typed data field.)

- [ ] **Step 4: Run tests + wasm build**

Run: `cargo test -p rumi_protocol_backend solana::sol_rpc`
Expected: all pass (5 total now).
Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/rumi_protocol_backend/src/chains/solana/sol_rpc.rs src/rumi_protocol_backend/src/chains/solana/tests_sol_rpc.rs
git commit -m "feat(solana): getAccountInfo + SPL mint supply decode

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Dev-gated read endpoints + chain registration + candid

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

- [ ] **Step 1: Add the endpoints to `main.rs`**

Near the other chain endpoints (after `set_observer_tick_interval_secs`, ~line 4644), add (the dev-gate mirrors that function exactly):

```rust
/// M1 read-seam probe: derive and return the Solana settlement (mint-authority)
/// address. Developer-gated. Exercises threshold Ed25519 on devnet/staging.
#[candid_method(update)]
#[update]
async fn solana_settlement_address() -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can derive the Solana settlement address".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, ted25519};
    let path = ted25519::settlement_derivation_path(SOLANA_CHAIN_ID);
    let (_pk, addr) = ted25519::derive_solana_address(path)
        .await
        .map_err(ProtocolError::GenericError)?;
    Ok(addr)
}

/// M1 read-seam probe: read a SOL balance (lamports) via the SOL RPC canister.
#[candid_method(update)]
#[update]
async fn solana_get_balance(pubkey: String) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can call solana_get_balance".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{sol_rpc, ted25519};
    if !ted25519::is_valid_solana_address(&pubkey) {
        return Err(ProtocolError::GenericError(format!("invalid Solana address: {pubkey}")));
    }
    sol_rpc::get_balance(&pubkey).await.map_err(ProtocolError::GenericError)
}

/// M1 read-seam probe: read the registered icUSD SPL mint's on-chain supply.
#[candid_method(update)]
#[update]
async fn solana_get_mint_supply() -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can call solana_get_mint_supply".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, sol_rpc};
    let mint = read_state(|s| s.multi_chain.chain_contracts.get(&SOLANA_CHAIN_ID).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Solana icUSD mint not set".to_string()))?;
    sol_rpc::get_mint_supply(&mint).await.map_err(ProtocolError::GenericError)
}

/// Developer-gated: set the SOL RPC canister principal override (mock/staging).
#[candid_method(update)]
#[update]
async fn set_sol_rpc_principal(p: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the SOL RPC principal".to_string(),
        ));
    }
    mutate_state(|s| s.sol_rpc_principal_override = Some(p));
    Ok(())
}
```

(Solana chain registration reuses the existing `register_chain` endpoint with `chains::solana::config::solana_default_register_arg()`; no new registration endpoint is needed. Setting the SPL mint reuses the existing `set_chain_contract`.)

- [ ] **Step 2: Update the candid interface**

In `src/rumi_protocol_backend/rumi_protocol_backend.did`, in the `service` block, add:

```candid
  solana_settlement_address : () -> (Result_text);
  solana_get_balance : (text) -> (Result_nat64);
  solana_get_mint_supply : () -> (Result_nat64);
  set_sol_rpc_principal : (principal) -> (Result);
```

Reuse existing `Result_*` type aliases if present; otherwise define `Result_text = variant { Ok : text; Err : ProtocolError }` and `Result_nat64 = variant { Ok : nat64; Err : ProtocolError }` near the other Result aliases. Verify with `grep -n "Result_text\|Result_nat64\|type Result " src/rumi_protocol_backend/rumi_protocol_backend.did`.

- [ ] **Step 3: Build (native + wasm) and verify candid matches**

Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
Expected: clean.
If the project has a candid-consistency check (grep the test suite for `candid::export_service` / a `.did` diff test), run it. Otherwise manually confirm the four new methods appear in the generated service.

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat(solana): dev-gated M1 read endpoints + SOL RPC override setter

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: PocketIC smoke test (install + dev-gate + Ed25519 derivation)

The async SOL RPC reads are verified manually on devnet (Task 8); PocketIC has no real network. This test covers what PocketIC can: the canister installs, dev-gating rejects non-developers, and (if PocketIC provisions an Ed25519 key) derivation returns a valid base58 address. The SOL RPC mock + read-path integration test lands in M2.

**Files:**
- Create: `src/rumi_protocol_backend/tests/solana_m1_seam_pic.rs`

- [ ] **Step 1: Write the test**

Create `src/rumi_protocol_backend/tests/solana_m1_seam_pic.rs`, mirroring the install/setup boilerplate in `tests/phase1b_supply_gate_pic.rs` (reuse its helper for building the backend wasm path and creating the PocketIC instance). Core assertions:

```rust
// 1. Install the backend with a known developer principal.
// 2. As a NON-developer, call solana_get_balance("11111111111111111111111111111111")
//    -> expect Err (dev-gated). Asserts the gate, no network needed.
// 3. As the developer, call solana_settlement_address():
//    - If PocketIC provisions a test Ed25519 key: expect Ok(addr) where
//      ted25519-style validation passes (base58 decodes to 32 bytes).
//    - If it returns a key-provisioning error: log and skip the assertion
//      (graceful degrade, mirroring phase1b_supply_gate_pic's ECDSA degrade).
```

Use the same PocketIC version and wasm-loading approach as `phase1b_supply_gate_pic.rs` (it `include_bytes!`/loads the prebuilt wasm — REBUILD the backend wasm before running, per MEMORY: "Rebuild canister wasms after any rebase").

- [ ] **Step 2: Build the backend wasm, then run the test**

Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
Then: `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_protocol_backend --test solana_m1_seam_pic`
Expected: PASS (dev-gate asserted; derivation asserted or gracefully skipped).

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/tests/solana_m1_seam_pic.rs
git commit -m "test(solana): PocketIC M1 smoke test (install + dev-gate + derivation)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: Devnet verification runbook (operator-gated, not agent-executed)

This is the M1 acceptance gate: prove the seam against real Solana devnet. Operator-run (it deploys to staging and touches devnet). Invoke the cycles-management skill before deploying.

- [ ] **Step 1: Pre-deploy cycle check** — invoke the `cycles-management` skill; confirm the staging backend has headroom. M1 adds NO timers, so idle burn is unchanged; the read endpoints burn only when called.

- [ ] **Step 2: Deploy the backend to staging** — upgrade only (never reinstall), using the dev identity, per MEMORY deployment rules and `.claude/hooks/pre-deploy-test.sh`.

- [ ] **Step 3: Register the Solana chain** — call `register_chain(solana_default_register_arg())` (chain id 501, devnet).

- [ ] **Step 4: Deploy an icUSD SPL mint on devnet** — using the Solana CLI, create an SPL Token mint with the address from `solana_settlement_address()` as the mint authority and 8 decimals. Record the mint address. Call `set_chain_contract(501, <mint_address>)`.

- [ ] **Step 5: Exercise the read seam** — as the developer principal:
  - `solana_settlement_address()` -> a valid base58 address (fund it with devnet SOL via faucet).
  - `solana_get_balance(<settlement_address>)` -> matches the faucet amount in lamports.
  - `solana_get_mint_supply()` -> 0 (freshly created mint).
  Record outputs. If all three return correct live-devnet values, M1 is DONE: the SOL RPC + Ed25519 seam works end to end on ic-cdk 0.12.

- [ ] **Step 6: Update tracking docs** — mark M1 complete in the spec; note the verified SOL RPC principal, the devnet mint address, and the solana-* wasm32 outcome (Task 3) for M2.

---

## Self-Review

**Spec coverage (M1 portion):** derive settlement/custody address (Tasks 2, 6) ✓; read balances + mint account via SOL RPC (Tasks 4, 5, 6) ✓; wasm32 build proof (Tasks 1, 3) ✓; hand-rolled on ic-cdk 0.12 (Tasks 2, 4) ✓; dev-gated endpoints + staging deploy with no timers (Tasks 6, 8) ✓; cycle-burn mindfulness (Task 8 step 1) ✓. Durable-nonce bootstrap correctly deferred to M2 (needs signing) and called out in the intro. SPL token program choice (classic SPL Token, 8 decimals) fixed in Task 8 step 4.

**Placeholder scan:** the only deliberately-deferred specifics are the SOL RPC inner result types (Task 4 Step 1 pulls the live .did to confirm them) and the `get_account_info`/base64 decode body (Task 5 Step 3 gives the shape + the consensus pattern to mirror from `get_balance`). These are verification-against-live-interface steps (playbook #1), not hand-waves; every pure helper and the call mechanics are fully specified.

**Type consistency:** `solana_address_from_pubkey`, `is_valid_solana_address`, `settlement_derivation_path`, `custody_derivation_path`, `derive_solana_address` (ted25519); `get_balance`, `lamports_from_balance_result`, `parse_mint_supply`, `get_account_info`, `get_mint_supply`, `sol_rpc_override`/`sol_rpc_principal_override` (sol_rpc/state); `SOLANA_CHAIN_ID`, `solana_schnorr_key_name`, `solana_default_register_arg` (config) — names are used consistently across tasks and endpoints.

---

## Execution Handoff

See the bottom of this session for the chosen execution mode.
