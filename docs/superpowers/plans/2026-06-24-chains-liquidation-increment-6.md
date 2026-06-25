# Chains Liquidation Increment 6 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Inc 5's manual reconciliation proof strings with finalized on-chain proof verification for pending-chain-burn and reserve-burn settlement, while keeping the existing manual endpoints as developer fallback.

**Architecture:** Add an EVM settlement-proof module that reuses the existing `eth_getTransactionReceipt` and finality helpers, verifies exact receipt log identities, and returns compact verified proof metadata. Add separate proof-id dedupe maps to `MultiChainStateV6` so pending-burn and reserve-burn settlements cannot collide with user burn proofs. The async endpoints fetch and verify receipts before entering a synchronous `mutate_state` section that dedupes, settles accounting, stores proof metadata, and records the existing Inc 5 events atomically.

**Tech Stack:** Rust 2021, `ic-cdk` update methods, existing hand-rolled EVM RPC wrapper on the repo's `ic-cdk` 0.12 pin, Candid, CBOR/serde stable snapshots, Cargo unit tests and targeted PocketIC tests.

## Verification Results

- PASS: `cargo test -p rumi_protocol_backend settlement_proof --lib -- --nocapture`
- PASS: `cargo test -p rumi_protocol_backend proof_backed_settlement --lib -- --nocapture`
- PASS: `cargo test -p rumi_protocol_backend pending_chain_burn_aging --lib -- --nocapture`
- PASS: `cargo test -p rumi_protocol_backend settlement_proof --bin rumi_protocol_backend -- --nocapture`
- PASS: `cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture`
- PASS: `cargo test -p rumi_protocol_backend --lib` (650 passed, 1 ignored)
- PASS: `cargo test -p rumi_protocol_backend --bin rumi_protocol_backend` (16 passed)
- PASS: final post-review rerun `cargo test -p rumi_protocol_backend --bin rumi_protocol_backend` (16 passed)
- PASS: `git diff --check`
- CAVEAT: the broad targeted `rustfmt --edition 2021 --check ... src/lib.rs` command fails on pre-existing formatting/trailing-whitespace outside this increment because rustfmt follows the full module tree from `lib.rs` (for example `test_helpers.rs` and `icrc21.rs`). No mass-formatting was applied.

## Global Constraints

- TDD is mandatory: write failing tests first, run them red, then implement the minimum code to turn them green.
- Do not replace or remove the Inc 5 manual endpoints `settle_pending_chain_burn` and `settle_reserve_burn`; Inc 6 adds proof-backed endpoints beside them.
- Do not settle accounting on caller-supplied amount alone; the amount must come from the verified receipt log.
- Do not hold a state borrow across an `.await`; fetch and verify receipts first, then mutate state synchronously.
- Every rejection path before mutation must leave `chain_supplies`, `pending_chain_burn_e8s`, `reserve_backing_e8s`, and proof-id maps unchanged.
- Proof storage must be compact: store canonical proof id plus minimal metadata, never full receipts.
- Proof domains are separate: user burn proof ids, pending-chain-burn settlement ids, and reserve-burn settlement ids must not share a dedupe map.
- Use `icp`, not `dfx`, for build/deploy instructions.
- Do not add `@rollup/rollup-darwin-*` to any `package.json`.
- Stop at code, tests, and PR unless the user explicitly authorizes merge/deploy for Inc 6.

---

### Task 1: Receipt-Log Proof Parsing and Verification

**Files:**
- Create: `src/rumi_protocol_backend/src/chains/evm/settlement_proof.rs`
- Modify: `src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs`
- Modify: `src/rumi_protocol_backend/src/chains/evm/mod.rs`
- Test: `src/rumi_protocol_backend/src/chains/evm/tests_settlement_proof.rs`
- Modify: `src/rumi_protocol_backend/src/chains/evm/tests_evm_rpc.rs`

**Interfaces:**
- Consumes:
  - `TxReceiptWithLogs { success, block_number, logs }`
  - `BURN_EVENT_TOPIC0`
  - `TRANSFER_EVENT_TOPIC0`
  - `parse_hex_quantity`
  - `is_block_final(chain, block_number, finality_depth)`
- Produces:
  - `pub struct BurnSettlementProofArg { pub tx_hash: String, pub log_index: u64, pub expected_burner: Option<String> }`
  - `pub struct ReserveSettlementProofArg { pub burn_tx_hash: String, pub burn_log_index: u64, pub reserve_tx_hash: String, pub reserve_transfer_log_index: u64, pub expected_burner: Option<String> }`
  - `pub struct VerifiedBurnSettlementProof { pub proof_id: String, pub tx_hash: String, pub log_index: u64, pub block_number: u64, pub burner: String, pub amount_e8s: u128 }`
  - `pub struct VerifiedReserveSettlementProof { pub proof_id: String, pub burn_tx_hash: String, pub burn_log_index: u64, pub burn_block_number: u64, pub burner: String, pub amount_e8s: u128, pub reserve_tx_hash: String, pub reserve_transfer_log_index: u64, pub reserve_block_number: u64, pub reserve_transfer_amount_native: u128 }`
  - `pub enum SettlementProofError`
  - `pub fn decode_burn_log_with_burner(topics: &[String], data: &str, tx_hash: &str, block_number: u64) -> Result<BurnLogWithBurner, String>`
  - `pub fn verify_pending_burn_receipt(contract: &str, proof: &BurnSettlementProofArg, receipt: &TxReceiptWithLogs) -> Result<VerifiedBurnSettlementProof, SettlementProofError>`
  - `pub fn verify_reserve_burn_receipts(icusd_contract: &str, settle_stable_token: &str, reserve_address: &str, proof: &ReserveSettlementProofArg, burn_receipt: &TxReceiptWithLogs, reserve_receipt: &TxReceiptWithLogs) -> Result<VerifiedReserveSettlementProof, SettlementProofError>`

- [ ] **Step 1: Write failing burner decode tests**

Add to `src/rumi_protocol_backend/src/chains/evm/tests_evm_rpc.rs`:

```rust
#[test]
fn burn_log_with_burner_decodes_indexed_burner_address() {
    let topics = vec![
        super::evm_rpc::BURN_EVENT_TOPIC0.to_string(),
        format!("0x{:064x}", 7u64),
        "0x0000000000000000000000001234567890abcdef1234567890abcdef12345678".to_string(),
    ];
    let burn = super::evm_rpc::decode_burn_log_with_burner(
        &topics,
        &format!("0x{:064x}", 40_000_000u128),
        "0xabc",
        99,
    )
    .expect("decode burn with burner");
    assert_eq!(burn.vault_id, 7);
    assert_eq!(burn.amount_e8s, 40_000_000);
    assert_eq!(burn.burner, "0x1234567890abcdef1234567890abcdef12345678");
}
```

- [ ] **Step 2: Run burner decode test to verify it fails**

Run: `cargo test -p rumi_protocol_backend burn_log_with_burner --lib -- --nocapture`

Expected: FAIL with an unresolved `decode_burn_log_with_burner` item.

- [ ] **Step 3: Implement burner decode**

In `evm_rpc.rs`, add:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnLogWithBurner {
    pub vault_id: u64,
    pub burner: String,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

pub fn decode_burn_log_with_burner(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<BurnLogWithBurner, String> {
    let base = BurnLog::from_raw(topics, data, tx_hash, block_number)?;
    let raw = topics
        .get(2)
        .ok_or_else(|| "BurnLogWithBurner: missing burner topic".to_string())?;
    let hex = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")).unwrap_or(raw);
    if hex.len() < 40 {
        return Err(format!("BurnLogWithBurner: burner topic too short: {raw}"));
    }
    let burner = format!("0x{}", hex[hex.len() - 40..].to_ascii_lowercase());
    Ok(BurnLogWithBurner {
        vault_id: base.vault_id,
        burner,
        amount_e8s: base.amount_e8s,
        tx_hash: base.tx_hash,
        block_number: base.block_number,
    })
}
```

- [ ] **Step 4: Run burner decode test to verify it passes**

Run: `cargo test -p rumi_protocol_backend burn_log_with_burner --lib -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Write failing pure settlement-proof tests**

Create `src/rumi_protocol_backend/src/chains/evm/tests_settlement_proof.rs` with tests for:

```rust
#[test]
fn pending_burn_proof_accepts_exact_contract_log_index_and_amount_from_log() {
    let contract = "0x000000000000000000000000000000000000cafe";
    let proof = BurnSettlementProofArg {
        tx_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        log_index: 7,
        expected_burner: Some("0x000000000000000000000000000000000000beef".to_string()),
    };
    let receipt = receipt(
        true,
        55,
        vec![
            transfer_log("0x000000000000000000000000000000000000feed", "0x000000000000000000000000000000000000aaaa", 1, 3),
            burn_log(contract, 99, "0x000000000000000000000000000000000000beef", 25_000_000, 7),
        ],
    );

    let verified = verify_pending_burn_receipt(contract, &proof, &receipt).expect("verified proof");
    assert_eq!(verified.amount_e8s, 25_000_000);
    assert_eq!(verified.block_number, 55);
    assert_eq!(verified.log_index, 7);
    assert_eq!(verified.burner, "0x000000000000000000000000000000000000beef");
    assert_eq!(
        verified.proof_id,
        "pending:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:7"
    );
}

#[test]
fn pending_burn_proof_rejects_wrong_contract_wrong_log_index_and_wrong_burner() {
    let contract = "0x000000000000000000000000000000000000cafe";
    let proof = BurnSettlementProofArg {
        tx_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        log_index: 2,
        expected_burner: Some("0x000000000000000000000000000000000000beef".to_string()),
    };
    let wrong_contract = receipt(true, 55, vec![burn_log("0x000000000000000000000000000000000000dead", 99, "0x000000000000000000000000000000000000beef", 25_000_000, 2)]);
    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &wrong_contract),
        Err(SettlementProofError::NoMatchingBurnLog)
    ));

    let wrong_index = receipt(true, 55, vec![burn_log(contract, 99, "0x000000000000000000000000000000000000beef", 25_000_000, 3)]);
    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &wrong_index),
        Err(SettlementProofError::NoMatchingBurnLog)
    ));

    let wrong_burner = receipt(true, 55, vec![burn_log(contract, 99, "0x000000000000000000000000000000000000badd", 25_000_000, 2)]);
    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &wrong_burner),
        Err(SettlementProofError::UnexpectedBurner { .. })
    ));
}

#[test]
fn reserve_burn_proof_requires_icusd_burn_and_settle_stable_transfer_to_reserve() {
    let icusd = "0x000000000000000000000000000000000000cafe";
    let usdc = "0x0000000000000000000000000000000000001000";
    let reserve = "0x0000000000000000000000000000000000002222";
    let proof = ReserveSettlementProofArg {
        burn_tx_hash: "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
        burn_log_index: 4,
        reserve_tx_hash: "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
        reserve_transfer_log_index: 8,
        expected_burner: Some("0x000000000000000000000000000000000000beef".to_string()),
    };
    let burn_receipt = receipt(true, 70, vec![burn_log(icusd, 0, "0x000000000000000000000000000000000000beef", 25_000_000, 4)]);
    let reserve_receipt = receipt(true, 71, vec![transfer_log(usdc, reserve, 25_000_000, 8)]);

    let verified = verify_reserve_burn_receipts(icusd, usdc, reserve, &proof, &burn_receipt, &reserve_receipt)
        .expect("reserve proof verified");
    assert_eq!(verified.amount_e8s, 25_000_000);
    assert_eq!(verified.reserve_transfer_amount_native, 25_000_000);
    assert_eq!(
        verified.proof_id,
        "reserve:0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc:4:0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd:8"
    );
}

#[test]
fn reserve_burn_proof_rejects_mismatched_transfer_recipient_or_too_small_transfer() {
    let icusd = "0x000000000000000000000000000000000000cafe";
    let usdc = "0x0000000000000000000000000000000000001000";
    let reserve = "0x0000000000000000000000000000000000002222";
    let proof = ReserveSettlementProofArg {
        burn_tx_hash: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string(),
        burn_log_index: 4,
        reserve_tx_hash: "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
        reserve_transfer_log_index: 8,
        expected_burner: None,
    };
    let burn_receipt = receipt(true, 70, vec![burn_log(icusd, 0, "0x000000000000000000000000000000000000beef", 25_000_000, 4)]);

    let wrong_recipient = receipt(true, 71, vec![transfer_log(usdc, "0x0000000000000000000000000000000000003333", 25_000_000, 8)]);
    assert!(matches!(
        verify_reserve_burn_receipts(icusd, usdc, reserve, &proof, &burn_receipt, &wrong_recipient),
        Err(SettlementProofError::ReserveTransferMissing)
    ));

    let too_small = receipt(true, 71, vec![transfer_log(usdc, reserve, 24_999_999, 8)]);
    assert!(matches!(
        verify_reserve_burn_receipts(icusd, usdc, reserve, &proof, &burn_receipt, &too_small),
        Err(SettlementProofError::ReserveTransferTooSmall { .. })
    ));
}
```

Use helper constructors:

```rust
fn word(v: u128) -> String { format!("0x{v:064x}") }
fn word_addr(addr: &str) -> String {
    let h = addr.trim_start_matches("0x");
    format!("0x{h:0>64}")
}
```

- [ ] **Step 6: Run settlement-proof tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend settlement_proof --lib -- --nocapture`

Expected: FAIL because `settlement_proof` module and proof types do not exist.

- [ ] **Step 7: Implement pure proof module**

Create `settlement_proof.rs`. The verification functions must:

```rust
pub fn canonical_tx_hash(tx_hash: &str) -> Result<String, SettlementProofError> {
    let tx = tx_hash.trim().to_ascii_lowercase();
    if !tx.starts_with("0x") || tx.len() != 66 {
        return Err(SettlementProofError::InvalidTxHash);
    }
    Ok(tx)
}
```

For pending burn:
- reject `receipt.success == false`
- find exactly `proof.log_index`
- require `log.address == contract`
- require `topics[0] == BURN_EVENT_TOPIC0`
- decode amount and burner with `decode_burn_log_with_burner`
- if `expected_burner` is `Some`, require exact lowercase match
- return `proof_id = format!("pending:{}:{}", tx_hash, log_index)`

For reserve burn:
- verify burn receipt as above against `icusd_contract`
- verify reserve transfer receipt at `reserve_transfer_log_index`
- require transfer log address equals `settle_stable_token`
- require decoded `Transfer(_, reserve_address, amount)` recipient equals `reserve_address`
- require `transfer.amount >= burn.amount_e8s`
- return `proof_id = format!("reserve:{}:{}:{}:{}", burn_tx_hash, burn_log_index, reserve_tx_hash, reserve_transfer_log_index)`

- [ ] **Step 8: Run task tests**

Run: `cargo test -p rumi_protocol_backend settlement_proof burn_log_with_burner --lib -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
git add src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs \
  src/rumi_protocol_backend/src/chains/evm/settlement_proof.rs \
  src/rumi_protocol_backend/src/chains/evm/mod.rs \
  src/rumi_protocol_backend/src/chains/evm/tests_evm_rpc.rs \
  src/rumi_protocol_backend/src/chains/evm/tests_settlement_proof.rs
git commit -m "feat(chains): Inc6 settlement proof parsing"
```

Expected: commit succeeds.

### Task 2: Proof Dedupe and Atomic Settlement State Helpers

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`
- Modify: `src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs`
- Modify: `src/rumi_protocol_backend/src/chains/supply.rs`
- Modify: `src/rumi_protocol_backend/src/chains/tests_supply.rs`

**Interfaces:**
- Consumes:
  - Task 1 verified proof structs.
  - Inc 5 `settle_pending_chain_burn` and `settle_reserve_burn` helpers.
- Produces:
  - `pub struct SettlementProofRecord { pub proof_id: String, pub tx_hash: String, pub log_index: u64, pub amount_e8s: u128, pub block_number: u64, pub recorded_at_ns: u64 }`
  - `MultiChainStateV6.settled_pending_burn_proofs: BTreeMap<String, SettlementProofRecord>`
  - `MultiChainStateV6.settled_reserve_burn_proofs: BTreeMap<String, SettlementProofRecord>`
  - `pub enum ProofBackedSettlementError`
  - `pub fn settle_pending_chain_burn_with_verified_proof(state: &mut MultiChainState, chain: ChainId, proof: VerifiedBurnSettlementProof, now_ns: u64) -> Result<(), ProofBackedSettlementError>`
  - `pub fn settle_reserve_burn_with_verified_proof(state: &mut MultiChainState, chain: ChainId, proof: VerifiedReserveSettlementProof, now_ns: u64) -> Result<(), ProofBackedSettlementError>`

- [ ] **Step 1: Write failing state migration/dedupe tests**

Add tests proving:
- a pre-Inc6 V6 snapshot decodes with both proof maps empty,
- a duplicate pending proof id rejects with no accounting mutation,
- a duplicate reserve proof id rejects with no accounting mutation,
- proof records store only ids and compact metadata.

- [ ] **Step 2: Run state tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend settlement_proof_state --lib -- --nocapture`

Expected: FAIL because proof maps and records do not exist.

- [ ] **Step 3: Add state fields and record type**

In `multi_chain_state.rs`, add:

```rust
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementProofRecord {
    pub proof_id: String,
    pub tx_hash: String,
    pub log_index: u64,
    pub amount_e8s: u128,
    pub block_number: u64,
    pub recorded_at_ns: u64,
}
```

Add to `MultiChainStateV6`:

```rust
#[serde(default)]
pub settled_pending_burn_proofs: BTreeMap<String, SettlementProofRecord>,
#[serde(default)]
pub settled_reserve_burn_proofs: BTreeMap<String, SettlementProofRecord>,
```

- [ ] **Step 4: Run state tests to verify they pass**

Run: `cargo test -p rumi_protocol_backend settlement_proof_state --lib -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Write failing atomic settlement tests**

Add tests in `tests_supply.rs` proving:
- pending proof settlement reduces `pending_chain_burn_e8s` and `chain_supplies`, inserts proof id, and preserves invariant;
- reserve proof settlement reduces `reserve_backing_e8s` and `chain_supplies`, inserts proof id, and preserves invariant;
- underflow/duplicate/unknown-chain rejection leaves all accounting and proof maps unchanged.

- [ ] **Step 6: Run settlement tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend proof_backed_settlement --lib -- --nocapture`

Expected: FAIL because proof-backed helper functions do not exist.

- [ ] **Step 7: Implement proof-backed settlement helpers**

Validation order:
1. check proof id absent in the domain map;
2. call the Inc 5 backing settlement helper on a cloned `MultiChainState`;
3. insert compact proof record into the cloned state;
4. assign cloned state back to `*state`.

This clone-then-commit pattern guarantees no partial mutation if settlement or proof insertion rejects.

- [ ] **Step 8: Run task tests**

Run: `cargo test -p rumi_protocol_backend proof_backed_settlement settlement_proof_state --lib -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
git add src/rumi_protocol_backend/src/chains/multi_chain_state.rs \
  src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs \
  src/rumi_protocol_backend/src/chains/supply.rs \
  src/rumi_protocol_backend/src/chains/tests_supply.rs
git commit -m "feat(chains): Inc6 proof dedupe settlement state"
```

Expected: commit succeeds.

### Task 3: Async Proof-Backed Operator Endpoints

**Files:**
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Modify: `src/rumi_protocol_backend/src/event.rs`
- Modify: `src/rumi_protocol_backend/src/lib.rs`
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`

**Interfaces:**
- Consumes:
  - Task 1 proof verification functions.
  - Task 2 proof-backed state helpers.
- Produces:
  - `settle_pending_chain_burn_with_proof(chain: ChainId, proof: BurnSettlementProofArg) -> Result<(), ProtocolError>`
  - `settle_reserve_burn_with_proof(chain: ChainId, proof: ReserveSettlementProofArg) -> Result<(), ProtocolError>`
  - `get_settlement_proof_ids(chain: opt ChainId) -> SettlementProofIds`

- [ ] **Step 1: Write failing endpoint helper tests**

Add tests for a small pure function in `main.rs`:

```rust
fn chain_finality_depth_or_default(s: &State, chain: ChainId) -> u64
fn settlement_proof_context(s: &State, chain: ChainId) -> Result<SettlementProofContext, ProtocolError>
```

The tests prove missing chain, missing icUSD contract, missing liquidation config, and missing reserve token/address reject before any RPC call.

- [ ] **Step 2: Run endpoint helper tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend settlement_proof_context --bin rumi_protocol_backend -- --nocapture`

Expected: FAIL because context helpers do not exist.

- [ ] **Step 3: Implement endpoint context helpers**

`SettlementProofContext` contains:

```rust
struct SettlementProofContext {
    contract: String,
    finality_depth: u64,
    settle_stable_token: Option<String>,
    reserve_address: Option<String>,
}
```

For pending proofs, require only registered chain and `chain_contracts[chain]`. For reserve proofs, also require `chain_liquidation_configs[chain].settle_stable_token` and derived `reserve_address`.

- [ ] **Step 4: Write failing async endpoint tests**

Add a PocketIC test file only if the existing mock can set receipts with logs by tx hash; otherwise add unit tests around the async-free commit helpers and leave live RPC verification to staging. The test should prove the endpoint path rejects duplicate proof id before second accounting mutation.

- [ ] **Step 5: Run endpoint tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend settlement_proof_context --bin rumi_protocol_backend -- --nocapture`

Expected: FAIL until endpoint wiring exists.

- [ ] **Step 6: Implement proof-backed endpoints**

Endpoint flow:
1. developer-gate the call;
2. read context without awaiting;
3. fetch receipt(s) through `get_transaction_receipt_with_logs`;
4. require receipt present, `success == true`, and finality via `is_block_final`;
5. run pure verifier from Task 1;
6. call Task 2 helper inside `mutate_state`;
7. record existing Inc 5 event with `proof = verified.proof_id`;
8. log proof id and amount.

Keep the Inc 5 manual endpoints unchanged.

- [ ] **Step 7: Add proof-id query**

Expose compact operator visibility:

```rust
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SettlementProofIds {
    pub pending: Vec<String>,
    pub reserve: Vec<String>,
}
```

`get_settlement_proof_ids(chain)` may ignore `chain` in v1 if proof ids are globally unique, but keep the argument for future chain-specific filtering. Sort ids lexicographically.

- [ ] **Step 8: Run task tests**

Run: `cargo test -p rumi_protocol_backend settlement_proof_context --bin rumi_protocol_backend -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
git add src/rumi_protocol_backend/src/main.rs \
  src/rumi_protocol_backend/src/event.rs \
  src/rumi_protocol_backend/src/lib.rs \
  src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat(chains): Inc6 proof-backed reconciliation endpoints"
```

Expected: commit succeeds.

### Task 4: Aged Pending-Burn Monitor

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/multi_chain_state.rs`
- Modify: `src/rumi_protocol_backend/src/main.rs`
- Test: `src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs`

**Interfaces:**
- Consumes:
  - `pending_chain_burn_e8s`
  - existing event log and settlement proof maps
- Produces:
  - `pub struct PendingChainBurnAging { pub chain_id: ChainId, pub pending_chain_burn_e8s: u128, pub oldest_reference_ns: Option<u64>, pub age_ns: Option<u64>, pub proof_count: u64 }`
  - `get_pending_chain_burn_aging() -> Vec<PendingChainBurnAging>`

- [ ] **Step 1: Write failing aging tests**

Add tests proving chains with nonzero `pending_chain_burn_e8s` are surfaced, zero-pending chains are omitted, and proof counts are reported without exposing receipt payloads.

- [ ] **Step 2: Run aging tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend pending_chain_burn_aging --lib -- --nocapture`

Expected: FAIL because aging report types and helpers do not exist.

- [ ] **Step 3: Implement aging report helper**

Use the oldest matching `ChainPendingBurnSettled` proof record when available. If no timestamp exists for a pending chain, return `oldest_reference_ns = None` and `age_ns = None` rather than manufacturing a timestamp.

- [ ] **Step 4: Expose query endpoint**

Add `get_pending_chain_burn_aging` as a query. It is operator visibility only and does not mutate state.

- [ ] **Step 5: Run task tests**

Run: `cargo test -p rumi_protocol_backend pending_chain_burn_aging --lib -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add src/rumi_protocol_backend/src/chains/multi_chain_state.rs \
  src/rumi_protocol_backend/src/chains/tests_multi_chain_state_v2.rs \
  src/rumi_protocol_backend/src/main.rs
git commit -m "feat(chains): Inc6 pending burn aging monitor"
```

Expected: commit succeeds.

### Task 5: Candid, Verification, and PR

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`
- Modify: `docs/superpowers/plans/2026-06-24-chains-liquidation-increment-6.md`

**Interfaces:**
- Consumes: Tasks 1-4 public API changes.
- Produces: regenerated Candid and a reviewed PR branch.

- [ ] **Step 1: Regenerate Candid**

Run: `RUMI_REGEN_DID=1 cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture`

Expected: PASS and `src/rumi_protocol_backend/rumi_protocol_backend.did` updated.

- [ ] **Step 2: Run focused backend tests**

Run:

```bash
cargo test -p rumi_protocol_backend settlement_proof proof_backed_settlement pending_chain_burn_aging --lib --bin rumi_protocol_backend -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run full backend lib/bin checks**

Run:

```bash
cargo test -p rumi_protocol_backend --lib
cargo test -p rumi_protocol_backend --bin rumi_protocol_backend
```

Expected: PASS.

- [ ] **Step 4: Run targeted formatting**

Run:

```bash
rustfmt --edition 2021 --check \
  src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs \
  src/rumi_protocol_backend/src/chains/evm/settlement_proof.rs \
  src/rumi_protocol_backend/src/chains/evm/tests_settlement_proof.rs \
  src/rumi_protocol_backend/src/chains/multi_chain_state.rs \
  src/rumi_protocol_backend/src/chains/supply.rs \
  src/rumi_protocol_backend/src/main.rs \
  src/rumi_protocol_backend/src/lib.rs
```

Expected: PASS.

- [ ] **Step 5: Commit final verification metadata**

Run:

```bash
git add docs/superpowers/plans/2026-06-24-chains-liquidation-increment-6.md \
  src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "docs(chains): Inc6 proof verification plan"
```

Expected: commit succeeds if those files changed after prior commits; otherwise skip this commit with `git status --short` evidence.

- [ ] **Step 6: Push and open PR**

Run:

```bash
git push -u origin codex/chains-liquidation-inc6
gh pr create --title "feat(chains): Inc6 proof-backed reconciliation" --body-file /tmp/inc6-pr-body.md --base main --head codex/chains-liquidation-inc6
```

Expected: PR opens. Do not merge or deploy without explicit user authorization.

## Self-Review

**Spec coverage:** This plan implements the Inc 5 follow-up goal: verified finalized receipts/logs for pending-chain-burn settlement and reserve-burn settlement, proof dedupe, compact proof metadata, and aged pending visibility. It deliberately excludes automatic SP escalation to keep Inc 6 focused.

**Placeholder scan:** No task uses TBD/TODO/fill-in language. Every task names concrete files, interfaces, commands, and expected outcomes.

**Type consistency:** `BurnSettlementProofArg`, `ReserveSettlementProofArg`, `VerifiedBurnSettlementProof`, `VerifiedReserveSettlementProof`, and proof id map names are consistent across parsing, state, endpoint, and Candid tasks.
