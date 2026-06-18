# M2 EVM-native Self-Serve Auth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:test-driven-development for each task. Steps use checkbox (`- [ ]`) syntax. Design doc: `docs/superpowers/plans/2026-06-18-conflux-evm-self-serve-auth-design.md`.

**Goal:** Add EIP-712-signed, anonymous-callable `open/borrow/withdraw/close` vault methods to the Conflux/Monad native-collateral CDP rail, with vaults owned by a synthetic principal derived from the EVM signer.

**Architecture:** A new pure `chains/evm/eip712.rs` (domain/struct hashing, k256 ecrecover, synthetic principal) feeds four `_evm` methods in `main.rs` that drive the existing in-state vault helpers owned by the synthetic principal. Borrow adds a per-op `IcUSD.sol` mint-idempotency change. Replay is blocked by a per-owner nonce; state grows by two additive `#[serde(default)]` fields (ciborium-safe). All testnet + dev-gated to enable.

**Tech Stack:** Rust (ic-cdk 0.12, k256 0.13 ecdsa+arithmetic, sha3 0.10, ciborium), Solidity 0.8.24 (Foundry), PocketIC 6.0.0.

**Worktree:** `/Users/robertripley/coding/rumi-protocol-v2/.worktrees/conflux-evm-self-serve-auth` (branch `feat/conflux-evm-self-serve-auth`).

**Test commands:**
- Unit/lib: `cargo test -p rumi_protocol_backend --lib <filter>`
- Build wasm (before PocketIC): `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release && cargo build -p monad_rpc_mock --target wasm32-unknown-unknown --release`
- PocketIC: `POCKET_IC_BIN=$(pwd)/pocket-ic cargo test -p rumi_protocol_backend --test conflux_evm_self_serve_pic`
- Foundry: `cd foundry && export PATH="$HOME/.foundry/bin:$PATH" && forge test`

---

## File Structure

- **New:** `src/rumi_protocol_backend/src/chains/evm/eip712.rs` — EIP-712 domain/struct hashing, `recover_evm_address`, `synthetic_owner`, `VaultIntent`, `IntentAction`. One responsibility: turn `(intent, sig)` into a verified `(signer_addr, synthetic_principal)` + digest.
- **New:** `src/rumi_protocol_backend/src/chains/evm/tests_eip712.rs` — unit tests.
- **New:** `src/rumi_protocol_backend/tests/conflux_evm_self_serve_pic.rs` — PocketIC e2e.
- **Modify:** `chains/vault.rs` (owner_evm field, `borrow_chain_vault_in_state`, per-owner cap + nonce helpers), `chains/multi_chain_state.rs` (`evm_owner_nonces`), `chains/evm/tx.rs` (mint selector + op_id), `chains/evm/settlement.rs` (thread op_id), `chains/evm/mod.rs` (`pub mod eip712;`), `main.rs` (4 `_evm` methods, verify helper, inspect_message, GC timer), `lib.rs` (`ProtocolError::EvmAuth`), `rumi_protocol_backend.did`.
- **Modify (Solidity):** `foundry/src/IcUSD.sol`, `foundry/test/IcUSD.t.sol`, `foundry/README.md`, `foundry/DEPLOY.md`, `chains/evm/adapter.rs` (doc prose).

---

## Task 1: EIP-712 module — types, hashing, recovery, synthetic principal

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/evm/eip712.rs`
- Create: `src/rumi_protocol_backend/src/chains/evm/tests_eip712.rs`
- Modify: `src/rumi_protocol_backend/src/chains/evm/mod.rs` (add `pub mod eip712;` and `#[cfg(test)] mod tests_eip712;`)

- [ ] **Step 1.1: Write the module with types + pure functions.**

```rust
//! EIP-712 typed-data intents for EVM-native self-serve vault auth (M2).
//!
//! The canister is the verifier: a user signs a `VaultIntent` in their EVM
//! wallet, and the canister recomputes the digest, recovers the signer, and
//! acts. There is NO on-chain verifying contract; the IcUSD contract address
//! merely binds the EIP-712 domain to a specific chain + deployment so a
//! signature cannot be replayed across chains (71 vs 10143) or deployments
//! (staging kvg63 vs mainnet IcUSD addresses differ).

use crate::chains::config::ChainId;
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;
use sha3::{Digest, Keccak256};

/// Domain name + version, frozen into the EIP-712 domain separator.
pub const DOMAIN_NAME: &str = "Rumi icUSD CDP";
pub const DOMAIN_VERSION: &str = "1";

/// The four vault operations a `VaultIntent` can authorize. The numeric value
/// is the on-wire `uint8 action` field hashed into the struct (do NOT renumber).
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAction {
    Open,               // 0
    Borrow,             // 1
    WithdrawCollateral, // 2
    Close,              // 3
}

impl IntentAction {
    pub fn as_u8(self) -> u8 {
        match self {
            IntentAction::Open => 0,
            IntentAction::Borrow => 1,
            IntentAction::WithdrawCollateral => 2,
            IntentAction::Close => 3,
        }
    }
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(IntentAction::Open),
            1 => Some(IntentAction::Borrow),
            2 => Some(IntentAction::WithdrawCollateral),
            3 => Some(IntentAction::Close),
            _ => None,
        }
    }
}

/// The signed intent. Candid mirror: `action` is a `nat8`, addresses are
/// lowercase `0x` `text`, amounts are `nat`, nonce/deadline are `nat64`.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct VaultIntent {
    pub action: u8,
    pub chain_id: u64,
    /// Claimed EVM owner; MUST equal the recovered signer.
    pub owner: String,
    pub vault_id: u64,
    /// Open: declared collateral (wei). Withdraw: amount to release (wei). Else 0.
    pub collateral_wei: u128,
    /// Open: initial debt (e8s). Borrow: additional debt (e8s). Else 0.
    pub debt_e8s: u128,
    /// Mint recipient (open/borrow) or collateral destination (withdraw/close).
    /// Enforced `== owner` in M2.
    pub recipient: String,
    pub nonce: u64,
    pub deadline_secs: u64,
}

/// `keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")`
fn domain_typehash() -> [u8; 32] {
    Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    )
    .into()
}

/// `keccak256("VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)")`
fn intent_typehash() -> [u8; 32] {
    Keccak256::digest(
        b"VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)",
    )
    .into()
}

/// 32-byte big-endian word for a u128 (left-padded).
fn word_u128(n: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&n.to_be_bytes());
    w
}
/// 32-byte big-endian word for a u64.
fn word_u64(n: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&n.to_be_bytes());
    w
}
/// 32-byte word for a 20-byte address (right-aligned). Returns Err on bad hex/len.
fn word_address(addr: &str) -> Result<[u8; 32], String> {
    let bytes = parse_addr_20(addr)?;
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(&bytes);
    Ok(w)
}

/// Parse a `0x`-prefixed 20-byte EVM address into raw bytes (canonical key input).
pub fn parse_addr_20(addr: &str) -> Result<[u8; 20], String> {
    let h = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    let v = hex::decode(h).map_err(|e| format!("bad address hex: {e}"))?;
    v.try_into().map_err(|v: Vec<u8>| format!("address is {} bytes, expected 20", v.len()))
}

/// `domainSeparator = keccak256(abi.encode(DOMAIN_TYPEHASH, keccak(name), keccak(version), chainId, verifyingContract))`
pub fn domain_separator(chain_id: u64, verifying_contract: &str) -> Result<[u8; 32], String> {
    let mut buf = Vec::with_capacity(32 * 5);
    buf.extend_from_slice(&domain_typehash());
    buf.extend_from_slice(&<[u8; 32]>::from(Keccak256::digest(DOMAIN_NAME.as_bytes())));
    buf.extend_from_slice(&<[u8; 32]>::from(Keccak256::digest(DOMAIN_VERSION.as_bytes())));
    buf.extend_from_slice(&word_u64(chain_id));
    buf.extend_from_slice(&word_address(verifying_contract)?);
    Ok(Keccak256::digest(&buf).into())
}

/// `hashStruct(intent) = keccak256(abi.encode(INTENT_TYPEHASH, ...fields...))`.
pub fn intent_struct_hash(intent: &VaultIntent) -> Result<[u8; 32], String> {
    let mut buf = Vec::with_capacity(32 * 10);
    buf.extend_from_slice(&intent_typehash());
    buf.extend_from_slice(&word_u64(intent.action as u64));
    buf.extend_from_slice(&word_u64(intent.chain_id));
    buf.extend_from_slice(&word_address(&intent.owner)?);
    buf.extend_from_slice(&word_u64(intent.vault_id));
    buf.extend_from_slice(&word_u128(intent.collateral_wei));
    buf.extend_from_slice(&word_u128(intent.debt_e8s));
    buf.extend_from_slice(&word_address(&intent.recipient)?);
    buf.extend_from_slice(&word_u64(intent.nonce));
    buf.extend_from_slice(&word_u64(intent.deadline_secs));
    Ok(Keccak256::digest(&buf).into())
}

/// `digest = keccak256(0x19 ‖ 0x01 ‖ domainSeparator ‖ hashStruct)`.
pub fn intent_digest(domain_sep: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(2 + 64);
    buf.push(0x19);
    buf.push(0x01);
    buf.extend_from_slice(domain_sep);
    buf.extend_from_slice(struct_hash);
    Keccak256::digest(&buf).into()
}

/// Recover the lowercase `0x` EVM signer address from a 65-byte signature over a
/// 32-byte prehash digest. Accepts `v ∈ {0,1,27,28}`.
pub fn recover_evm_address(digest: &[u8; 32], sig65: &[u8]) -> Result<String, String> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use super::tecdsa::evm_address_from_pubkey;

    if sig65.len() != 65 {
        return Err(format!("signature must be 65 bytes, got {}", sig65.len()));
    }
    let r: [u8; 32] = sig65[0..32].try_into().unwrap();
    let s: [u8; 32] = sig65[32..64].try_into().unwrap();
    let parity = match sig65[64] {
        0 | 27 => 0u8,
        1 | 28 => 1u8,
        v => return Err(format!("bad recovery byte v={v}")),
    };
    let sig = Signature::from_scalars(r, s).map_err(|e| format!("bad (r,s): {e}"))?;
    let rid = RecoveryId::new(parity == 1, false);
    let vk = VerifyingKey::recover_from_prehash(digest, &sig, rid)
        .map_err(|e| format!("ecrecover failed: {e}"))?;
    let pk = vk.to_encoded_point(false).as_bytes().to_vec();
    evm_address_from_pubkey(&pk)
}

/// Deterministic opaque-class synthetic owner principal for an EVM address on a
/// chain. `keccak256("rumi.evm.owner.v1:" ‖ chain_le ‖ addr20)[0..28] ‖ 0x01`.
/// The trailing `0x01` is the opaque type tag, so this can never equal a
/// self-authenticating (trailing `0x02`) user principal. Internal owner key only.
pub fn synthetic_owner(chain: ChainId, evm_addr: &str) -> Result<Principal, String> {
    let addr20 = parse_addr_20(evm_addr)?;
    let mut hasher = Keccak256::new();
    hasher.update(b"rumi.evm.owner.v1:");
    hasher.update(chain.0.to_le_bytes());
    hasher.update(addr20);
    let h: [u8; 32] = hasher.finalize().into();
    let mut bytes = Vec::with_capacity(29);
    bytes.extend_from_slice(&h[0..28]);
    bytes.push(0x01);
    Ok(Principal::from_slice(&bytes))
}
```

- [ ] **Step 1.2: Wire the module** — in `chains/evm/mod.rs` add `pub mod eip712;` near the other `pub mod` lines and `#[cfg(test)] mod tests_eip712;` near the other test modules.

- [ ] **Step 1.3: Write `tests_eip712.rs`** (golden vectors using the fixed-key pattern copied from `chains/evm/tests_tx.rs:336`):

```rust
use super::eip712::*;
use crate::chains::config::ChainId;
use candid::Principal;

/// Fixed secp256k1 key with scalar = 1 → canonical address.
fn fixed_key_addr() -> (k256::ecdsa::SigningKey, String) {
    use k256::ecdsa::{SigningKey, VerifyingKey};
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let vk = VerifyingKey::from(&sk);
    let pk = vk.to_encoded_point(false).as_bytes().to_vec();
    let addr = crate::chains::evm::tecdsa::evm_address_from_pubkey(&pk).unwrap();
    (sk, addr)
}

fn sign_intent(sk: &k256::ecdsa::SigningKey, digest: &[u8; 32]) -> Vec<u8> {
    use k256::ecdsa::{signature::hazmat::PrehashSigner, Signature, RecoveryId};
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(digest).unwrap();
    let mut out = sig.to_bytes().to_vec(); // 64 bytes r||s
    out.push(27 + u8::from(rid)); // canonical EVM v
    out
}

fn sample_intent(owner: &str) -> VaultIntent {
    VaultIntent {
        action: IntentAction::Open.as_u8(),
        chain_id: 71,
        owner: owner.to_string(),
        vault_id: 0,
        collateral_wei: 1_400_000_000_000_000_000_000, // 1400 CFX
        debt_e8s: 10_000_000_000,                      // 100 icUSD
        recipient: owner.to_string(),
        nonce: 0,
        deadline_secs: 9_999_999_999,
    }
}

#[test]
fn recovers_known_signer() {
    let (sk, addr) = fixed_key_addr();
    assert_eq!(addr, "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf");
    let contract = "0x00000000000000000000000000000000cf1c0de5";
    let intent = sample_intent(&addr);
    let dsep = domain_separator(71, contract).unwrap();
    let sh = intent_struct_hash(&intent).unwrap();
    let digest = intent_digest(&dsep, &sh);
    let sig = sign_intent(&sk, &digest);
    let recovered = recover_evm_address(&digest, &sig).unwrap();
    assert_eq!(recovered, addr);
}

#[test]
fn wrong_contract_changes_digest() {
    let (_sk, addr) = fixed_key_addr();
    let intent = sample_intent(&addr);
    let a = intent_digest(&domain_separator(71, "0x00000000000000000000000000000000cf1c0de5").unwrap(), &intent_struct_hash(&intent).unwrap());
    let b = intent_digest(&domain_separator(71, "0x00000000000000000000000000000000deadbeef").unwrap(), &intent_struct_hash(&intent).unwrap());
    assert_ne!(a, b);
}

#[test]
fn wrong_chain_changes_digest() {
    let (_sk, addr) = fixed_key_addr();
    let intent = sample_intent(&addr);
    let sh = intent_struct_hash(&intent).unwrap();
    let a = intent_digest(&domain_separator(71, "0x00000000000000000000000000000000cf1c0de5").unwrap(), &sh);
    let b = intent_digest(&domain_separator(10143, "0x00000000000000000000000000000000cf1c0de5").unwrap(), &sh);
    assert_ne!(a, b);
}

#[test]
fn tampered_amount_breaks_recovery_match() {
    let (sk, addr) = fixed_key_addr();
    let contract = "0x00000000000000000000000000000000cf1c0de5";
    let intent = sample_intent(&addr);
    let digest = intent_digest(&domain_separator(71, contract).unwrap(), &intent_struct_hash(&intent).unwrap());
    let sig = sign_intent(&sk, &digest);
    // Recover against a DIFFERENT digest (tampered debt) → different/!=owner addr.
    let mut tampered = intent.clone();
    tampered.debt_e8s += 1;
    let d2 = intent_digest(&domain_separator(71, contract).unwrap(), &intent_struct_hash(&tampered).unwrap());
    let recovered = recover_evm_address(&d2, &sig).unwrap();
    assert_ne!(recovered, addr);
}

#[test]
fn synthetic_owner_is_opaque_and_deterministic() {
    let p1 = synthetic_owner(ChainId(71), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();
    let p2 = synthetic_owner(ChainId(71), "0x7E5F4552091a69125d5DFcb7b8C2659029395Bdf").unwrap(); // case-insensitive
    assert_eq!(p1, p2, "address case must not matter");
    let bytes = p1.as_slice();
    assert_eq!(bytes.len(), 29);
    assert_eq!(bytes[28], 0x01, "opaque class tag");
    // Distinct per chain.
    let p3 = synthetic_owner(ChainId(10143), "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();
    assert_ne!(p1, p3);
    // Never equals a self-authenticating principal (those end in 0x02, len 29).
    assert_ne!(bytes[28], 0x02);
}
```

- [ ] **Step 1.4: Run** `cargo test -p rumi_protocol_backend --lib eip712` → expect all pass.
- [ ] **Step 1.5: Commit** `feat(chains/evm): EIP-712 intent hashing, ecrecover, synthetic principal`.

---

## Task 2: Additive state fields + ciborium upgrade test

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs` (`ChainVaultV1.owner_evm`)
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs` (`MultiChainStateV4.evm_owner_nonces`)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` (`ChainVaultV1.owner_evm : opt text`)
- Test: in `chains/tests_multi_chain_state.rs` (add an upgrade round-trip)

- [ ] **Step 2.1: Add `owner_evm` to `ChainVaultV1`** (chains/vault.rs, after `opened_at_ns`):

```rust
    pub opened_at_ns: u64,
    /// EVM owner address (lowercase 0x) for vaults opened via the M2 self-serve
    /// `_evm` path; `None` for developer-opened / Monad / Solana vaults.
    /// `#[serde(default)]` keeps pre-M2 ciborium snapshots decoding cleanly
    /// (State is ciborium-encoded — see storage.rs).
    #[serde(default)]
    pub owner_evm: Option<String>,
```

Then fix every `ChainVaultV1 { ... }` literal that does NOT use `..Default` — there is one in `open_chain_vault_in_state` (chains/vault.rs:247). Add `owner_evm: None,` there (the `_evm` open path will set it after insert, or pass it through — see Task 5). Search the crate for `ChainVaultV1 {` to find all literals (tests included) and add the field.

- [ ] **Step 2.2: Add `evm_owner_nonces` to `MultiChainStateV4`** (multi_chain_state.rs, after `processed_burn_keys`):

```rust
    #[serde(default)]
    pub processed_burn_keys: BTreeMap<u64, BTreeSet<String>>,
    /// M2: per-synthetic-owner monotonic nonce for EIP-712 intent replay
    /// protection. Keyed by `eip712::synthetic_owner(chain, evm_addr)` (which
    /// embeds the chain → nonces are per-(owner,chain)). `#[serde(default)]`:
    /// additive, ciborium-safe (no version bump — coordinated with the
    /// concurrent interest-accrual branch).
    #[serde(default)]
    pub evm_owner_nonces: BTreeMap<Principal, u64>,
```

`Principal` is already imported via `use candid::{CandidType, Deserialize};` — add `Principal` to that import if missing (it currently imports `ChainId` etc.; confirm `candid::Principal` is in scope, else add `use candid::Principal;`).

- [ ] **Step 2.3: Candid** — in `rumi_protocol_backend.did`, add to `ChainVaultV1`:

```candid
  owner_evm : opt text;
```

(Do NOT add `evm_owner_nonces` to candid — it is internal state, not exposed.)

- [ ] **Step 2.4: Write the upgrade round-trip test** in `chains/tests_multi_chain_state.rs`:

```rust
#[test]
fn pre_m2_snapshot_decodes_with_defaulted_evm_fields() {
    use crate::chains::multi_chain_state::{MultiChainStateV4};
    use crate::chains::monad::chain_vault::{ChainVaultV1, ChainVaultStatus};
    use crate::chains::config::ChainId;
    use candid::Principal;

    // Build a V4 with one funded vault + a chain supply, encode via ciborium,
    // then decode into the (post-M2-field) V4 and assert the data survived and
    // the new fields default.
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(ChainId(71), 100_000_000);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1,
        owner: Principal::from_slice(&[1, 2, 3]),
        collateral_chain: ChainId(71),
        custody_address: "0xabc".to_string(),
        collateral_amount_native: 1_000,
        debt_e8s: 100_000_000,
        mint_recipient: "0xdef".to_string(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 42,
        owner_evm: None,
    });
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&s, &mut buf).unwrap();
    let back: MultiChainStateV4 = ciborium::de::from_reader(buf.as_slice()).unwrap();
    assert_eq!(back.chain_supplies.get(&ChainId(71)).copied(), Some(100_000_000));
    assert_eq!(back.chain_vaults.get(&1).unwrap().debt_e8s, 100_000_000);
    assert_eq!(back.chain_vaults.get(&1).unwrap().owner_evm, None);
    assert!(back.evm_owner_nonces.is_empty());
}
```

> Note: the design's stronger claim — that a snapshot written BEFORE the new keys existed still decodes — is already proven generically by storage.rs's tests; this test pins the V4-specific shape. To additionally prove the "missing key" path, encode a `ciborium::Value::Map`, strip the `owner_evm`/`evm_owner_nonces` keys, re-encode, and decode (mirroring `state.rs:6504`). Add that as a second test if time permits.

- [ ] **Step 2.5: Run** `cargo test -p rumi_protocol_backend --lib multi_chain_state` → pass.
- [ ] **Step 2.6: Commit** `feat(chains): additive owner_evm + evm_owner_nonces (serde-default)`.

---

## Task 3: borrow helper + nonce + per-owner cap

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs` (`borrow_chain_vault_in_state`, `count_owner_active_vaults`)
- Modify: `src/rumi_protocol_backend/src/chains/monad/chain_vault.rs` (Monad wrapper for borrow, mirroring open/withdraw/close)
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs` (nonce helper methods)
- Test: `chains/tests_vault.rs`

- [ ] **Step 3.1: Add nonce helpers to `MultiChainStateV4`** (multi_chain_state.rs `impl MultiChainStateV4`):

```rust
    /// Expected next nonce for a synthetic owner (0 if never seen).
    pub fn expected_evm_nonce(&self, owner: &Principal) -> u64 {
        self.evm_owner_nonces.get(owner).copied().unwrap_or(0)
    }
    /// Consume `nonce` for `owner`: succeeds and bumps the counter iff
    /// `nonce == expected`; else returns the expected value as Err.
    pub fn consume_evm_nonce(&mut self, owner: &Principal, nonce: u64) -> Result<(), u64> {
        let expected = self.expected_evm_nonce(owner);
        if nonce != expected {
            return Err(expected);
        }
        self.evm_owner_nonces.insert(*owner, expected.saturating_add(1));
        Ok(())
    }
    /// Count non-terminal vaults (AwaitingDeposit/MintPending/Open/Closing) owned
    /// by `owner` (anti-spam per-owner cap).
    pub fn count_owner_active_vaults(&self, owner: &Principal) -> usize {
        self.chain_vaults
            .values()
            .filter(|v| &v.owner == owner)
            .filter(|v| !matches!(v.status, ChainVaultStatus::Closed))
            .count()
    }
```

(Add `use crate::chains::vault::ChainVaultStatus;` / `use candid::Principal;` to multi_chain_state.rs imports if not present.)

- [ ] **Step 3.2: Add `MAX_VAULTS_PER_OWNER` const + `borrow_chain_vault_in_state`** to chains/vault.rs:

```rust
/// Anti-spam: max non-terminal vaults a single synthetic owner may hold.
pub const MAX_VAULTS_PER_OWNER: usize = 25;

/// Reasons `borrow_chain_vault_in_state` can reject.
#[derive(Debug, PartialEq, Eq)]
pub enum BorrowError {
    UnknownVault,
    /// Vault is not `Open` (only an Open vault has confirmed collateral + debt).
    WrongStatus { status: ChainVaultStatus },
    /// A mint is already in flight for this vault (`pending_mint_e8s != 0`).
    MintInFlight,
    ZeroDebt,
    NoPrice,
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    QueueError(String),
    InvalidAddress(String),
}

/// Borrow additional icUSD against an existing Open vault (a SECOND on-chain
/// mint, gated by the per-op IcUSD idempotency from Task 4). Sets
/// `pending_mint_e8s = additional_e8s` and enqueues a `Mint` op; the settlement
/// confirm moves pending→debt + supply at finality (Design B). The off-chain
/// idempotency key embeds `now_ns` so it never collides with the genesis open
/// mint key (`mint-{chain}-{vault}`); the on-chain op_id (assigned at enqueue)
/// is what the per-op IcUSD guard discriminates on.
#[allow(clippy::too_many_arguments)]
pub fn borrow_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    additional_e8s: u128,
    recipient: String,
    address_validator: fn(&str) -> bool,
    price_symbol: &str,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), BorrowError> {
    if additional_e8s == 0 {
        return Err(BorrowError::ZeroDebt);
    }
    if !address_validator(&recipient) {
        return Err(BorrowError::InvalidAddress(recipient));
    }
    // Step 1: read-only validation (no mutation on any reject).
    let (chain, collateral, new_debt) = {
        let v = state.chain_vaults.get(&vault_id).ok_or(BorrowError::UnknownVault)?;
        if v.status != ChainVaultStatus::Open {
            return Err(BorrowError::WrongStatus { status: v.status.clone() });
        }
        if v.pending_mint_e8s != 0 {
            return Err(BorrowError::MintInFlight);
        }
        (v.collateral_chain, v.collateral_amount_native, v.debt_e8s.saturating_add(additional_e8s))
    };
    let price_e8 = *state
        .manual_prices
        .get(&(chain, price_symbol.to_string()))
        .ok_or(BorrowError::NoPrice)?;
    let native_decimals = state
        .chain_configs.get(&chain).map(|c| c.chain_native_decimals).unwrap_or(18);
    let cr_e4 = collateral_ratio_e4(collateral, native_decimals, price_e8, new_debt);
    if cr_e4 < min_cr_e4 {
        return Err(BorrowError::BelowMinCr { cr_e4, min_e4: min_cr_e4 });
    }
    // Step 2: enqueue FIRST (can fail on a dup key). Distinct from the genesis
    // open key by the now_ns suffix.
    let op = SettlementOp::new(
        SettlementOpKind::Mint { recipient, amount_e8s: additional_e8s, vault_id },
        format!("mint-{}-{}-{}", chain.0, vault_id, now_ns),
        now_ns,
    );
    state.settlement_queues.entry(chain).or_default().enqueue(op)
        .map_err(|e| BorrowError::QueueError(format!("{e:?}")))?;
    // Step 3: only after a successful enqueue — reserve the borrow as pending.
    let v = state.chain_vaults.get_mut(&vault_id).expect("vault present: checked above");
    v.pending_mint_e8s = additional_e8s;
    Ok(())
}
```

- [ ] **Step 3.3: Add the Monad wrapper** in `chains/monad/chain_vault.rs` (mirroring the open/withdraw/close wrappers, baking `is_valid_evm_address` + `"MON"`):

```rust
pub fn borrow_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    additional_e8s: u128,
    recipient: String,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), crate::chains::vault::BorrowError> {
    crate::chains::vault::borrow_chain_vault_in_state(
        state, vault_id, additional_e8s, recipient,
        crate::chains::monad::tecdsa::is_valid_evm_address, "MON", min_cr_e4, now_ns,
    )
}
```

Also re-export `BorrowError` in the `pub use crate::chains::vault::{...}` line at the top of `chains/monad/chain_vault.rs`.

- [ ] **Step 3.4: Tests** in `chains/tests_vault.rs` — borrow CR boundary (pass at exactly min CR, fail just below), `pending != 0` → `MintInFlight`, non-Open → `WrongStatus`, the supply invariant via `confirm_mint_in_state` after a borrow (mint confirm moves pending→debt and bumps supply by exactly `additional_e8s`), nonce monotonic accept/replay-reject, per-owner cap counts non-terminal only. Use the existing test helpers in that file (registered chain + manual price). Example for the cap:

```rust
#[test]
fn per_owner_cap_counts_non_terminal_only() {
    use crate::chains::multi_chain_state::MultiChainStateV4;
    use crate::chains::monad::chain_vault::{ChainVaultV1, ChainVaultStatus};
    use crate::chains::config::ChainId;
    use candid::Principal;
    let mut s = MultiChainStateV4::default();
    let owner = Principal::from_slice(&[9, 9, 9]);
    let mk = |id: u64, st: ChainVaultStatus| ChainVaultV1 {
        vault_id: id, owner, collateral_chain: ChainId(71), custody_address: "0x".into(),
        collateral_amount_native: 0, debt_e8s: 0, mint_recipient: "0x".into(),
        pending_mint_e8s: 0, status: st, opened_at_ns: 0, owner_evm: None,
    };
    s.chain_vaults.insert(1, mk(1, ChainVaultStatus::Open));
    s.chain_vaults.insert(2, mk(2, ChainVaultStatus::AwaitingDeposit));
    s.chain_vaults.insert(3, mk(3, ChainVaultStatus::Closed)); // not counted
    assert_eq!(s.count_owner_active_vaults(&owner), 2);
}
```

- [ ] **Step 3.5: Run** `cargo test -p rumi_protocol_backend --lib vault` and `--lib multi_chain_state` → pass.
- [ ] **Step 3.6: Commit** `feat(chains): borrow_chain_vault_in_state + per-owner nonce/cap helpers`.

---

## Task 4: IcUSD.sol per-op idempotency + calldata + settlement op_id

**Files:**
- Modify: `foundry/src/IcUSD.sol`, `foundry/test/IcUSD.t.sol`, `foundry/README.md`, `foundry/DEPLOY.md`
- Modify: `src/rumi_protocol_backend/src/chains/evm/tx.rs` (`encode_mint_calldata`, `MonadTxKind::Mint`, `build_eip1559_fields`)
- Modify: `src/rumi_protocol_backend/src/chains/evm/settlement.rs` (`build_tx_plan` + 2 call sites thread op_id)
- Modify: `src/rumi_protocol_backend/src/chains/evm/tests_tx.rs` (calldata length 4+32*3 → 4+32*4)
- Modify: `src/rumi_protocol_backend/src/chains/evm/adapter.rs` (doc prose `mint(address,uint256,uint64)` → `mint(address,uint256,uint64,uint64)`)

- [ ] **Step 4.1: Compute the new selector** (record it for the Foundry pin):

Run: `cd foundry && export PATH="$HOME/.foundry/bin:$PATH" && cast sig "mint(address,uint256,uint64,uint64)"`
Record the 4-byte selector hex; it replaces `0x8b3d35ae` in the Foundry ABI-pin test. (If `cast` is unavailable, a Rust scratch test printing `keccak_selector("mint(address,uint256,uint64,uint64)")` works.)

- [ ] **Step 4.2: Edit `foundry/src/IcUSD.sol`** — re-key idempotency to op_id (the Mint EVENT is unchanged):

```solidity
    /// @dev One mint per settlement op_id (idempotency guard): a canister
    /// resubmit-after-transient-RPC-error must not double-mint on-chain. Per-OP
    /// (not per-vault) so a vault can be minted to more than once (borrow).
    mapping(uint64 => bool) public mintedOps;

    // ...

    /// @notice Mint icUSD. Only the canister settlement address (MINTER_ROLE).
    /// `op_id` is the backend settlement queue's unique-per-chain op id; reverts
    /// if it was already minted (idempotency). `vault_id` stays the debt key for
    /// the Mint event + burn(repay).
    function mint(address to, uint256 amount, uint64 vault_id, uint64 op_id) external onlyRole(MINTER_ROLE) {
        require(!mintedOps[op_id], "op already minted");
        mintedOps[op_id] = true;
        _mint(to, amount);
        emit Mint(uint256(vault_id), to, amount);
    }
```

- [ ] **Step 4.3: Update `foundry/test/IcUSD.t.sol`** — add the 4th arg to every `icusd.mint(...)` call; replace `test_mint_same_vault_id_twice_reverts` with per-op semantics + a new "same vault, different op_id succeeds" test; fix the selector literal:

```solidity
    // Same op_id reverts (idempotency); different op_id (even same vault) succeeds.
    function test_mint_same_op_id_twice_reverts() public {
        vm.startPrank(minter);
        icusd.mint(alice, 100, 7, 1000);
        assertTrue(icusd.mintedOps(1000));
        vm.expectRevert(bytes("op already minted"));
        icusd.mint(alice, 50, 7, 1000); // same op_id -> revert
        vm.stopPrank();
        assertEq(icusd.totalSupply(), 100);
    }

    function test_borrow_same_vault_new_op_id_succeeds() public {
        vm.startPrank(minter);
        icusd.mint(alice, 100, 7, 1000); // genesis
        icusd.mint(alice, 50, 7, 1001);  // borrow: same vault, new op_id
        vm.stopPrank();
        assertEq(icusd.totalSupply(), 150);
        assertEq(icusd.balanceOf(alice), 150);
    }
```

In `test_abi_pinned_to_backend_constants`, change line 100 to the new signature + recomputed selector from Step 4.1:

```solidity
        assertEq(bytes4(keccak256("mint(address,uint256,uint64,uint64)")), bytes4(0x<NEW_SELECTOR>), "mint selector drift");
```

The Mint/Burn topic0 pins stay (event unchanged). Update the other mint call sites (lines ~32, 49, 54, 65, 73, 74, 117, 127, 142, 151, 173) to pass a 4th arg (any distinct op id; reuse the vault_id or a literal). Delete/replace the old `test_mint_same_vault_id_twice_reverts`.

- [ ] **Step 4.4: Run Foundry** `cd foundry && export PATH="$HOME/.foundry/bin:$PATH" && forge test` → pass (install deps first if `lib/` missing per README).

- [ ] **Step 4.5: Edit `tx.rs`** — `MonadTxKind::Mint` gains `op_id`, `encode_mint_calldata` gains `op_id` + a 4th word + new selector string, `build_eip1559_fields` threads it:

```rust
    Mint { contract: &'a str, recipient: &'a str, amount_e8s: u128, vault_id: u64, op_id: u64 },
```
```rust
pub fn encode_mint_calldata(to: &str, amount_e8s: u128, vault_id: u64, op_id: u64) -> Result<Vec<u8>, String> {
    let selector = keccak_selector("mint(address,uint256,uint64,uint64)");
    let mut out = Vec::with_capacity(4 + 128);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&abi_word_address(to)?);
    out.extend_from_slice(&abi_word_u128(amount_e8s));
    out.extend_from_slice(&abi_word_u128(vault_id as u128));
    out.extend_from_slice(&abi_word_u128(op_id as u128));
    Ok(out)
}
```
In `build_eip1559_fields`, the `MonadTxKind::Mint` arm destructures `op_id` and passes it: `encode_mint_calldata(recipient, amount_e8s, vault_id, op_id)`.

- [ ] **Step 4.6: Edit `settlement.rs`** — `build_tx_plan` gains an `op_id: u64` param; the `Mint` arm sets `MonadTxKind::Mint { ..., op_id }`; both call sites (`submit_op` ~line 587 and the resubmit path ~line 1075) pass the `op_id` they already hold. (The `op_id` is the function param in `submit_op(chain, op_id, op)` and in the resubmit fn.)

- [ ] **Step 4.7: Fix `tests_tx.rs`** — `mint_calldata_has_correct_selector`: `encode_mint_calldata(addr, amount, vault_id, op_id)` and assert `calldata.len() == 4 + 32 * 4`. Add a positive assertion that the 4th word equals the op_id.

- [ ] **Step 4.8: Update doc prose** in `adapter.rs:12,165`, `foundry/README.md:12-13,19-20`, `foundry/DEPLOY.md` (mint signature + per-op idempotency wording).

- [ ] **Step 4.9: Run** `cargo test -p rumi_protocol_backend --lib tx` and `--lib settlement` → pass.
- [ ] **Step 4.10: Commit** `feat(chains/evm,foundry): per-op mint idempotency (op_id) for borrow`.

---

## Task 5: the 4 `_evm` methods + verify helper + inspect_message + error

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs` (`ProtocolError::EvmAuth(String)`)
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` (`EvmAuth : text`, `VaultIntent` record, 4 methods)
- Modify: `src/rumi_protocol_backend/src/main.rs` (verify helper, 4 methods, inspect_message)

- [ ] **Step 5.1: Add the error variant** (lib.rs, append AFTER `ChainAdmin(String)`):

```rust
    /// M2 EVM-native self-serve auth failure (bad signature, signer != owner,
    /// nonce replay, expired deadline, recipient != owner, per-owner cap, etc.).
    EvmAuth(String),
```
And the candid `variant` (append): `EvmAuth : text;`.

- [ ] **Step 5.2: Add the shared verify helper + the 4 methods** to main.rs (near the dev-gated chain endpoints, ~line 1015). Use the verbatim integration points:
  - `evm_vault_params(chain)` → `(symbol, min_cr)`.
  - `chain_contracts` read for the domain's verifyingContract.
  - `eip712::{VaultIntent, IntentAction, domain_separator, intent_struct_hash, intent_digest, recover_evm_address, synthetic_owner}`.
  - `chain_vault_id_counter`, `custody_derivation_path`, `derive_evm_address`, the in-state `open/borrow/withdraw/close` helpers.

```rust
/// Verified, authenticated intent context shared by all four `_evm` methods.
struct VerifiedIntent {
    chain: rumi_protocol_backend::chains::config::ChainId,
    owner_evm: String,            // lowercase recovered signer
    synthetic: candid::Principal, // vault owner key
    symbol: &'static str,
    min_cr: u64,
}

/// Recompute the EIP-712 digest, recover the signer, enforce signer==owner,
/// chain match, deadline, and recipient==owner. Does NOT touch the nonce
/// (each method consumes it atomically with its state change). Pure (no await).
fn verify_intent(
    intent: &rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    sig: &[u8],
    expected_action: rumi_protocol_backend::chains::evm::eip712::IntentAction,
) -> Result<VerifiedIntent, ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712 as e712;
    let chain = rumi_protocol_backend::chains::config::ChainId(intent.chain_id as u32);
    // Action must match the endpoint.
    if e712::IntentAction::from_u8(intent.action) != Some(expected_action) {
        return Err(ProtocolError::EvmAuth("intent action mismatch".into()));
    }
    let (symbol, min_cr) = evm_vault_params(chain)?;
    // Domain bound to the deployed IcUSD contract for this chain.
    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned())
        .ok_or_else(|| ProtocolError::EvmAuth(format!("no contract for chain {}", chain.0)))?;
    // Deadline (seconds).
    let now_secs = ic_cdk::api::time() / 1_000_000_000;
    if now_secs > intent.deadline_secs {
        return Err(ProtocolError::EvmAuth("intent expired".into()));
    }
    // Recompute digest + recover signer.
    let dsep = e712::domain_separator(intent.chain_id, &contract)
        .map_err(|e| ProtocolError::EvmAuth(format!("domain: {e}")))?;
    let sh = e712::intent_struct_hash(intent)
        .map_err(|e| ProtocolError::EvmAuth(format!("struct hash: {e}")))?;
    let digest = e712::intent_digest(&dsep, &sh);
    let signer = e712::recover_evm_address(&digest, sig)
        .map_err(|e| ProtocolError::EvmAuth(format!("recover: {e}")))?;
    if !signer.eq_ignore_ascii_case(&intent.owner) {
        return Err(ProtocolError::EvmAuth("signer != owner".into()));
    }
    // M2: recipient forced == owner for every action.
    if !intent.recipient.eq_ignore_ascii_case(&intent.owner) {
        return Err(ProtocolError::EvmAuth("recipient must equal owner".into()));
    }
    let synthetic = e712::synthetic_owner(chain, &signer)
        .map_err(|e| ProtocolError::EvmAuth(format!("synthetic: {e}")))?;
    Ok(VerifiedIntent { chain, owner_evm: signer.to_lowercase(), synthetic, symbol, min_cr })
}
```

`open_chain_vault_evm` (async; nonce + cap + vault_id reserved PRE-await, derive, then insert):

```rust
#[candid_method(update)]
#[update]
async fn open_chain_vault_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<u64, ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent(&intent, &signature, IntentAction::Open)?;
    // Pre-await atomic: consume nonce, enforce cap, reserve vault_id. (Spend on
    // attempt — race-safe around the tECDSA derive; a same-nonce double-submit
    // is rejected here so the loser never pays for a derive.)
    let vault_id = mutate_state(|s| {
        s.multi_chain.consume_evm_nonce(&v.synthetic, intent.nonce)
            .map_err(|exp| ProtocolError::EvmAuth(format!("bad nonce: got {}, expected {}", intent.nonce, exp)))?;
        if s.multi_chain.count_owner_active_vaults(&v.synthetic)
            >= rumi_protocol_backend::chains::vault::MAX_VAULTS_PER_OWNER {
            return Err(ProtocolError::EvmAuth("per-owner vault cap reached".into()));
        }
        s.chain_vault_id_counter += 1;
        Ok(s.chain_vault_id_counter)
    })?;
    let path = rumi_protocol_backend::chains::evm::tecdsa::custody_derivation_path(v.chain, v.synthetic, vault_id);
    let (_pk, custody) = rumi_protocol_backend::chains::evm::tecdsa::derive_evm_address(path)
        .await
        .map_err(|e| ProtocolError::EvmAuth(format!("derive: {e}")))?;
    let now = ic_cdk::api::time();
    let owner_evm = v.owner_evm.clone();
    mutate_state(|s| {
        rumi_protocol_backend::chains::vault::open_chain_vault_in_state(
            &mut s.multi_chain, v.chain, v.synthetic, custody,
            intent.collateral_wei, intent.debt_e8s, v.owner_evm.clone(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol, v.min_cr, now, vault_id,
        ).map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        // Stamp the EVM owner on the freshly-inserted vault.
        if let Some(vault) = s.multi_chain.chain_vaults.get_mut(&vault_id) {
            vault.owner_evm = Some(owner_evm.clone());
        }
        Ok(vault_id)
    })
}
```

> Decision: rather than thread `owner_evm` through `open_chain_vault_in_state`'s signature (which Monad/Solana also call), the `_evm` method stamps `owner_evm` on the inserted vault immediately after. Keeps the shared helper's signature stable. (`mint_recipient` is passed as `owner_evm` since recipient==owner.)

`borrow_chain_vault_evm` / `withdraw_chain_collateral_evm` / `close_chain_vault_evm` (sync; spend-on-success). Each loads the vault, asserts `vault.owner == synthetic` AND `vault.owner_evm == Some(owner_evm)`, then in ONE `mutate_state`: do the op, and only on Ok consume the nonce:

```rust
#[candid_method(update)]
#[update]
fn borrow_chain_vault_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent(&intent, &signature, IntentAction::Borrow)?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        // Authorize: vault owned by this synthetic + EVM owner matches.
        let ok = s.multi_chain.chain_vaults.get(&intent.vault_id)
            .map(|vault| vault.owner == v.synthetic
                && vault.owner_evm.as_deref().map(|a| a.eq_ignore_ascii_case(&v.owner_evm)).unwrap_or(false))
            .unwrap_or(false);
        if !ok { return Err(ProtocolError::EvmAuth("not vault owner".into())); }
        // Nonce check FIRST (read-only), perform op, then bump nonce on success.
        let expected = s.multi_chain.expected_evm_nonce(&v.synthetic);
        if intent.nonce != expected {
            return Err(ProtocolError::EvmAuth(format!("bad nonce: got {}, expected {}", intent.nonce, expected)));
        }
        rumi_protocol_backend::chains::vault::borrow_chain_vault_in_state(
            &mut s.multi_chain, intent.vault_id, intent.debt_e8s, v.owner_evm.clone(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol, v.min_cr, now,
        ).map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        s.multi_chain.evm_owner_nonces.insert(v.synthetic, expected.saturating_add(1));
        Ok(())
    })
}
```

Withdraw and close are identical in shape, calling `withdraw_collateral_in_state(vault_id, intent.collateral_wei, dest = owner_evm, ...)` and `close_chain_vault_in_state(vault_id, dest = owner_evm, ...)` respectively, with the same owner-auth + nonce-on-success pattern and `IntentAction::WithdrawCollateral` / `IntentAction::Close`.

- [ ] **Step 5.3: inspect_message accept-list** — add the 4 method names so anonymous ingress reaches them (main.rs:180):

```rust
        "icrc21_canister_call_consent_message" | "icrc10_supported_standards"
        | "open_chain_vault_evm" | "borrow_chain_vault_evm"
        | "withdraw_chain_collateral_evm" | "close_chain_vault_evm" => {
            ic_cdk::api::call::accept_message();
        }
```

- [ ] **Step 5.4: Candid** — add to `rumi_protocol_backend.did`: the `VaultIntent` record (snake_case mirror), the `EvmAuth : text` variant in `ProtocolError`, and the 4 methods:

```candid
type VaultIntent = record {
  action : nat8;
  chain_id : nat64;
  owner : text;
  vault_id : nat64;
  collateral_wei : nat;
  debt_e8s : nat;
  recipient : text;
  nonce : nat64;
  deadline_secs : nat64;
};
```
```candid
  open_chain_vault_evm : (VaultIntent, blob) -> (Result_1);
  borrow_chain_vault_evm : (VaultIntent, blob) -> (Result);
  withdraw_chain_collateral_evm : (VaultIntent, blob) -> (Result);
  close_chain_vault_evm : (VaultIntent, blob) -> (Result);
```

(`blob` is candid for `Vec<u8>`. `Result_1` = `Ok : nat64`, `Result` = `Ok` unit, both already defined.)

- [ ] **Step 5.5: Run** `cargo build -p rumi_protocol_backend` and `cargo test -p rumi_protocol_backend --lib` → compile + lib green.
- [ ] **Step 5.6: Commit** `feat(backend): EVM-signed open/borrow/withdraw/close + inspect_message + EvmAuth`.

---

## Task 6: TTL-GC timer for stale AwaitingDeposit vaults

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/vault.rs` (pure `prune_stale_awaiting_deposit` + const)
- Modify: `src/rumi_protocol_backend/src/main.rs` (`register_chain_vault_gc_timer` + hook into `setup_timers`)
- Test: `chains/tests_vault.rs`

- [ ] **Step 6.1: Add the pure prune fn + TTL const** to chains/vault.rs:

```rust
/// TTL for an unfunded vault before the GC reaps it (24h in ns).
pub const AWAITING_DEPOSIT_TTL_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

/// Remove `AwaitingDeposit` vaults whose `opened_at_ns` is older than the TTL.
/// Returns the number pruned. Pure (in-state) — safe: an `AwaitingDeposit`
/// vault has no confirmed debt, no enqueued mint, and contributes nothing to
/// `chain_supplies`, so removal cannot break the supply invariant.
pub fn prune_stale_awaiting_deposit(state: &mut MultiChainStateV4, now_ns: u64, ttl_ns: u64) -> usize {
    let stale: Vec<u64> = state.chain_vaults.iter()
        .filter(|(_, v)| v.status == ChainVaultStatus::AwaitingDeposit
            && now_ns.saturating_sub(v.opened_at_ns) > ttl_ns)
        .map(|(&id, _)| id)
        .collect();
    for id in &stale {
        state.chain_vaults.remove(id);
    }
    stale.len()
}
```

- [ ] **Step 6.2: Add the timer** in main.rs (model on `register_observer_timer`), ticking every 1h:

```rust
fn register_chain_vault_gc_timer() {
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(3600), || {
        let now = ic_cdk::api::time();
        let pruned = mutate_state(|s| {
            rumi_protocol_backend::chains::vault::prune_stale_awaiting_deposit(
                &mut s.multi_chain, now,
                rumi_protocol_backend::chains::vault::AWAITING_DEPOSIT_TTL_NS,
            )
        });
        if pruned > 0 {
            log!(INFO, "[chain_vault_gc] pruned {} stale AwaitingDeposit vaults", pruned);
        }
    });
}
```

Call `register_chain_vault_gc_timer();` at the end of `setup_timers()` (main.rs ~line 439), next to `register_observer_timer()`.

- [ ] **Step 6.3: Test** in `chains/tests_vault.rs`: a vault older than TTL in AwaitingDeposit is pruned; a young one stays; an Open vault older than TTL is NOT pruned.
- [ ] **Step 6.4: Run** `cargo test -p rumi_protocol_backend --lib vault` → pass.
- [ ] **Step 6.5: Commit** `feat(chains): TTL-GC for stale AwaitingDeposit chain vaults`.

---

## Task 7: PocketIC e2e — anonymous EVM-signed flow

**Files:**
- Create: `src/rumi_protocol_backend/tests/conflux_evm_self_serve_pic.rs`

This mirrors `conflux_espace_happy_path_pic.rs` but drives the `_evm` methods as an **anonymous** caller with in-test-signed intents. Reuse that file's harness verbatim (copy `boot`, `update_dev`, `query_unit`, `get_vault`, `advance_and_tick`, `assert_supply`, `word_u128`, `word_addr`, `push_mint_log`, `push_burn_log`, the mock-control calls, `ProtocolInitArg`, the topic0 consts).

- [ ] **Step 7.1: Add EIP-712 signing helpers in the test** (fixed key, sign digest, build sig65), recomputing the digest exactly as the canister does:

```rust
fn signer() -> (k256::ecdsa::SigningKey, String) { /* scalar=1 → 0x7e5f...95bdf, as Task 1 */ }

fn eip712_digest(chain_id: u64, contract: &str, intent: &VaultIntentWire) -> [u8;32] {
    // Re-implement domain_separator + intent_struct_hash + intent_digest with
    // keccak (sha3) over the SAME field order as chains/evm/eip712.rs, OR import
    // the lib functions directly (the backend crate is a dep of the test).
}
```

> Simplest: the test depends on `rumi_protocol_backend` as a lib, so call `rumi_protocol_backend::chains::evm::eip712::{domain_separator, intent_struct_hash, intent_digest, VaultIntent}` directly — no re-implementation. Build the `VaultIntent`, compute the digest with the lib, sign with k256, append `27 + v`.

- [ ] **Step 7.2: Drive the full flow** as `Principal::anonymous()`:
  1. `boot()` + register chain (dev) + `set_chain_contract(71, CONTRACT)` + manual price + seed cursor + `set_burn_watch_poll_enabled` (all dev), exactly as the happy-path test.
  2. Probe `get_chain_settlement_address(71)`; if tECDSA unavailable on this PocketIC build, assert the gated subset and return (mirror the happy-path guard).
  3. Sign an **Open** intent (nonce 0, collateral 1400 CFX, debt 100e8, owner=recipient=signer); `update_call(backend, anonymous, "open_chain_vault_evm", Encode!(&intent, &sig))` → `Ok(vault_id)`. Assert the vault's `owner` is the synthetic principal (`eip712::synthetic_owner(ChainId(71), &signer_addr)`) and `owner_evm == Some(signer_lower)`.
  4. Deposit (`set_balance(custody, collateral)`) + ticks → MintPending; relocate receipt + `push_mint_log` → Open, supply 100e8. The on-chain mint log carries the vault's `mint_recipient` (== signer); the settlement op carries op_id (the mock doesn't check the calldata, so no contract change is exercised here — the per-op idempotency is exercised by Foundry).
  5. Sign a **Borrow** intent (nonce 1, debt 50e8); submit anonymous → Ok. Ticks + relocate a SECOND mint receipt (`0xcfxmint2`) + `push_mint_log(vault, recipient, 50e8, "0xcfxmint2", cursor)` → debt 150e8, supply 150e8.
  6. **Repay** by `push_burn_log` 150e8 → debt 0, supply 0.
  7. Sign a **Withdraw**/**Close** intent (nonce 2) dest=owner; submit anonymous → Ok; ticks + receipt → Closed.
  8. **Replay rejection:** re-submit the Open intent (nonce 0) → `Err(EvmAuth("bad nonce..."))`. **Wrong-signer rejection:** flip a byte in the signature → `Err(EvmAuth("signer != owner"))` (or recover error). **Anonymous-reaches-method:** the fact that the anonymous calls above returned `Ok`/typed `Err` (not a transport reject) proves the inspect_message accept-list works.

- [ ] **Step 7.3: Build wasms + run:**
```
cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release
cargo build -p monad_rpc_mock --target wasm32-unknown-unknown --release
POCKET_IC_BIN=$(pwd)/pocket-ic cargo test -p rumi_protocol_backend --test conflux_evm_self_serve_pic -- --nocapture
```
Expect: full flow passes; replay + wrong-signer rejected.

- [ ] **Step 7.4: Commit** `test(chains/evm): PocketIC EVM-signed self-serve e2e + replay rejection`.

---

## Task 8: Regression sweep, review, verification

- [ ] **Step 8.1: Full lib suite** `cargo test -p rumi_protocol_backend --lib` → all green (incl. the prior 361 + new).
- [ ] **Step 8.2: Existing chain PocketIC tests stay green** — rebuild wasms, then run the conflux + monad happy-path + burn-idempotency + supply-gate suites with the absolute `POCKET_IC_BIN`. These exercise the genesis mint through the NEW per-op calldata path (the mock ignores calldata, so they validate no regression in the queue/confirm threading).
- [ ] **Step 8.3: Foundry** `cd foundry && forge test` → green.
- [ ] **Step 8.4: Candid sync** — regenerate/inspect the `.did` matches the new methods (the build emits candid via `candid_method`; confirm no drift).
- [ ] **Step 8.5: Adversarial review** (see plan §Review) — parallel canister-security / silent-failure / audit pass on the diff; fix findings.
- [ ] **Step 8.6: Commit** any review fixes; push the branch; open the PR to `main` (surface the diff). **Do NOT merge or deploy without Rob's explicit authorization.**

---

## Spec coverage check

- EIP-712 domain binding (chain + contract) → Task 1 (`domain_separator`) + Task 5 (`verify_intent` reads `chain_contracts`). ✓
- Signer recovery reusing existing machinery → Task 1 (`recover_evm_address`). ✓
- Synthetic principal (opaque, collision-resistant, ≠ II) → Task 1 + tests. ✓
- Per-owner monotonic nonce (per-(owner,chain)) → Task 3 helpers + Task 5 (async spend-on-attempt, sync spend-on-success). ✓
- Synthetic ownership of the op via existing in-state helpers → Task 5. ✓
- `owner_evm` stored → Task 2 + Task 5 stamp. ✓
- Anti-spam: per-owner cap + deposit-gated mint + TTL-GC + cheap-before-derive → Task 3 (cap), Task 6 (GC), Task 5 (ordering). ✓
- Borrow + per-op IcUSD idempotency → Task 3 + Task 4. ✓
- `recipient == owner` enforced → Task 5 (`verify_intent`). ✓
- inspect_message accept-list → Task 5.3. ✓
- Additive serde-default migration + upgrade test → Task 2. ✓
- Unit vectors (recovery + EIP-712) + PocketIC e2e + replay rejection → Task 1, Task 7. ✓
- Existing tests stay green → Task 8. ✓
