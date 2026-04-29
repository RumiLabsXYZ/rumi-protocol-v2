//! Wave-8c/8d LIQ-004: ICRC-3 burn / transfer proof verification for SP-triggered writedowns.
//!
//! The stability pool entry points (`stability_pool_liquidate_debt_burned`,
//! `stability_pool_liquidate_with_reserves`) call into
//! `vault::liquidate_vault_debt_already_burned`, which intentionally bypasses
//! the `ratio < min_liq_ratio` check. Without this module the only access
//! control would be `caller == stability_pool_canister`. If the SP were
//! buggy, upgraded to buggy code, or its principal rotated to a malicious
//! one, the backend would write down debt on healthy vaults with no sanity
//! check.
//!
//! This module adds defense-in-depth: every writedown carries a
//! `SpWritedownProof` pointing at a real ICRC-3 block on the relevant
//! ledger, and the backend verifies the block matches expected accounts,
//! amount, and (for the legacy burn path) memo before accepting the
//! writedown.
//!
//! Vault binding has two flavours, one per ledger kind:
//!
//!   * `IcusdBurn` (legacy `_debt_burned` path) — the SP autonomously chose
//!     which vault to burn for and encoded the vault id in the burn block's
//!     memo. The verifier decodes that memo and asserts it matches the call
//!     site's vault id, so a burn proof for vault A cannot be replayed
//!     against vault B.
//!   * `ThreePoolTransfer` (reserves `_with_reserves` path) — the proof is
//!     produced by the backend itself after `transfer_3usd_to_reserves`
//!     succeeds, so vault binding is enforced by code construction at
//!     proof-build time. The on-chain block has no memo to check (the
//!     `rumi_3pool` ledger's `Icrc3Transaction::Transfer` variant does not
//!     persist memos into ICRC-3 blocks; it only consumes them for ICRC-1
//!     dedup). Verification on this kind asserts op / amount / from / to,
//!     and the consumed-proof set still blocks block-index replay.
//!
//! Replay within a single vault is blocked by the consumed-proof set on
//! `State` (see `State::consumed_writedown_proofs`).
//!
//! Wave-8d Phase-2 rollout: `proof: SpWritedownProof` is required on the
//! legacy `_debt_burned` entry point. The reserves entry point
//! (`_with_reserves`) builds its proof internally so it has no proof
//! parameter on its public surface. The Wave-8c migration WARN-log path
//! (`proof: None` with a per-call WARN) has been retired.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::{ICRC3Value, ICRC3Map};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

/// Which ledger the proof is against. Drives both the canister to query and
/// the expected operation kind.
///
/// Derives `Ord`/`PartialOrd`/`Eq`/`PartialEq` so it can serve as a key in
/// `State::consumed_writedown_proofs` (a `BTreeSet<(SpProofLedger, u64)>`).
#[derive(
    CandidType, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub enum SpProofLedger {
    /// icUSD ledger — expect a `burn` block (legacy 3pool atomic-burn path).
    IcusdBurn,
    /// 3USD / 3pool ledger — expect a transfer to the protocol's reserves
    /// subaccount (reserves path).
    ThreePoolTransfer,
}

/// Typed proof argument the SP passes alongside a writedown call.
///
/// `vault_id_memo` MUST equal the vault id the call is operating on; the
/// verifier rejects mismatches so a proof for vault A cannot be replayed
/// against vault B.
#[derive(CandidType, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpWritedownProof {
    pub block_index: u64,
    pub ledger_kind: SpProofLedger,
    pub vault_id_memo: u64,
}

/// Memo prefix that binds an ICRC-3 block to a Wave-8c writedown. Combined
/// with the vault id (8 bytes big-endian) the full memo is 21 bytes — well
/// under the standard ICRC-1 ledger's 32-byte memo cap.
pub const WRITEDOWN_MEMO_PREFIX: &[u8] = b"RUMI-LIQ-004:";

/// Build the canonical memo bytes for a SP writedown of `vault_id`.
pub fn encode_writedown_memo(vault_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(WRITEDOWN_MEMO_PREFIX.len() + 8);
    out.extend_from_slice(WRITEDOWN_MEMO_PREFIX);
    out.extend_from_slice(&vault_id.to_be_bytes());
    out
}

/// Reverse of `encode_writedown_memo`. Returns the vault id if `memo` matches
/// the Wave-8c shape, else `Err` with a description.
pub fn decode_writedown_memo(memo: &[u8]) -> Result<u64, String> {
    if memo.len() != WRITEDOWN_MEMO_PREFIX.len() + 8 {
        return Err(format!(
            "memo length {} not equal to expected {}",
            memo.len(),
            WRITEDOWN_MEMO_PREFIX.len() + 8
        ));
    }
    if !memo.starts_with(WRITEDOWN_MEMO_PREFIX) {
        return Err("memo prefix does not match RUMI-LIQ-004:".to_string());
    }
    let mut id_bytes = [0u8; 8];
    id_bytes.copy_from_slice(&memo[WRITEDOWN_MEMO_PREFIX.len()..]);
    Ok(u64::from_be_bytes(id_bytes))
}

/// Decoded ICRC-3 block fields that the verifier inspects. Sourced from a
/// generic `ICRC3Value` returned by `icrc3_get_blocks`. Both the standard
/// ic-icrc1-ledger (top-level `btype`) and the in-tree `rumi_3pool` ledger
/// (`tx.op`) are accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedBlock {
    pub op: String,
    pub from: Option<Account>,
    pub to: Option<Account>,
    pub spender: Option<Account>,
    pub amount: u128,
    pub memo: Option<Vec<u8>>,
}

/// Expectations the verifier asserts against a decoded block. Constructed
/// from the call-site state (ledger principals, SP principal, vault id,
/// expected amount).
#[derive(Clone, Debug)]
pub struct ProofExpectations {
    pub ledger_kind: SpProofLedger,
    pub expected_amount_e8s: u64,
    /// SP principal — must match `from` on both burn and transfer paths.
    pub sp_principal: Principal,
    /// Reserves account — must match `to` on the transfer path. Ignored on
    /// the burn path (burns have no `to`).
    pub reserves_account: Account,
    /// Vault id encoded into the memo. Verifier rejects if the decoded
    /// memo's vault id does not equal this.
    pub vault_id_memo: u64,
}

/// Pure-logic decoder. Walks an `ICRC3Value` and pulls out the fields we
/// validate. Handles both block formats:
///
///   * standard ic-icrc1-ledger: `btype` at top level (e.g., `"1burn"`,
///     `"1xfer"`); operation fields under `tx`.
///   * `rumi_3pool` ledger: no top-level `btype`; `op` lives under `tx`
///     (e.g., `"burn"`, `"xfer"`); operation fields under `tx`.
///
/// Returns `Err` if the value is not a Map, lacks a recognizable op, or the
/// expected fields cannot be decoded.
pub fn decode_block(value: &ICRC3Value) -> Result<DecodedBlock, String> {
    let block_map = match value {
        ICRC3Value::Map(m) => m,
        _ => return Err("block is not a Map".to_string()),
    };

    let tx_map = block_map
        .get("tx")
        .ok_or_else(|| "block missing 'tx' field".to_string())
        .and_then(|v| match v {
            ICRC3Value::Map(m) => Ok(m),
            _ => Err("'tx' is not a Map".to_string()),
        })?;

    let op = if let Some(btype) = block_map.get("btype").and_then(text_value) {
        normalize_op(&btype)
    } else if let Some(op) = tx_map.get("op").and_then(text_value) {
        normalize_op(&op)
    } else {
        return Err("block has neither top-level 'btype' nor tx.'op'".to_string());
    };

    let from = tx_map.get("from").map(account_from_value).transpose()?;
    let to = tx_map.get("to").map(account_from_value).transpose()?;
    let spender = tx_map.get("spender").map(account_from_value).transpose()?;

    let amount = tx_map
        .get("amt")
        .ok_or_else(|| "tx missing 'amt'".to_string())
        .and_then(nat_to_u128)?;

    let memo = match tx_map.get("memo") {
        Some(ICRC3Value::Blob(b)) => Some(b.to_vec()),
        Some(_) => return Err("tx 'memo' is not a Blob".to_string()),
        None => None,
    };

    Ok(DecodedBlock {
        op,
        from,
        to,
        spender,
        amount,
        memo,
    })
}

/// Pure-logic validator. Asserts `block` matches `expected` for the given
/// `ledger_kind`. Returns the validated vault id on success.
///
/// Rules common to both kinds:
///   * `amount` must equal `expected.expected_amount_e8s`.
///   * `from.owner` must equal `expected.sp_principal`.
///
/// `IcusdBurn`-specific rules:
///   * `op == "burn"`.
///   * Memo must be present and must decode to `expected.vault_id_memo` via
///     `decode_writedown_memo`. The SP autonomously chose the vault for a
///     burn, so memo binding is the cross-vault replay guard.
///
/// `ThreePoolTransfer`-specific rules:
///   * `op == "xfer"` (or `"transfer"`).
///   * `to` must equal `expected.reserves_account`.
///   * Memo is NOT checked. The `rumi_3pool` ledger does not persist memos
///     into its ICRC-3 block log (only consumes them for ICRC-1 dedup), and
///     the proof on this path is constructed by the backend itself rather
///     than supplied by the SP, so cross-vault replay is prevented by the
///     backend's code-time construction (`vault_id_memo` is set to the
///     call's `vault_id`) plus the consumed-proof set's per-block-index
///     replay defense.
pub fn validate_block(
    block: &DecodedBlock,
    expected: &ProofExpectations,
) -> Result<u64, String> {
    match expected.ledger_kind {
        SpProofLedger::IcusdBurn => {
            if block.op != "burn" {
                return Err(format!(
                    "expected burn block on icUSD ledger, got op={}",
                    block.op
                ));
            }
        }
        SpProofLedger::ThreePoolTransfer => {
            if block.op != "xfer" && block.op != "transfer" {
                return Err(format!(
                    "expected transfer block on 3USD ledger, got op={}",
                    block.op
                ));
            }
        }
    }

    let amount_u64 = u64::try_from(block.amount)
        .map_err(|_| format!("block amount {} does not fit in u64", block.amount))?;
    if amount_u64 != expected.expected_amount_e8s {
        return Err(format!(
            "block amount {} does not equal expected {}",
            amount_u64, expected.expected_amount_e8s
        ));
    }

    let from = block
        .from
        .as_ref()
        .ok_or_else(|| "block missing 'from' field".to_string())?;
    if from.owner != expected.sp_principal {
        return Err(format!(
            "block 'from' owner {} does not equal expected SP {}",
            from.owner, expected.sp_principal
        ));
    }

    match expected.ledger_kind {
        SpProofLedger::ThreePoolTransfer => {
            let to = block
                .to
                .as_ref()
                .ok_or_else(|| "transfer block missing 'to' field".to_string())?;
            // The 3pool ledger's ICRC-3 block log records `to: Principal` only,
            // dropping the subaccount (see `Icrc3Transaction::Transfer` in
            // `rumi_3pool/src/types.rs` and `account_to_value` in
            // `rumi_3pool/src/icrc3.rs`). The actual ICRC-2 transfer still
            // credits the correct (owner, subaccount) pair on-ledger, but the
            // block has no subaccount to compare against. Verify the owner
            // matches and accept either `None` or the expected subaccount —
            // the security invariant ("transfer ended up at the protocol
            // canister's principal") still holds, since the destination is
            // hardcoded in `transfer_3usd_to_reserves` and the SP cannot
            // redirect it.
            if to.owner != expected.reserves_account.owner {
                return Err(format!(
                    "block 'to' does not equal expected reserves account (owner {} sub {:?})",
                    expected.reserves_account.owner, expected.reserves_account.subaccount
                ));
            }
            if to.subaccount.is_some()
                && to.subaccount != expected.reserves_account.subaccount
            {
                return Err(format!(
                    "block 'to' subaccount {:?} does not equal expected {:?}",
                    to.subaccount, expected.reserves_account.subaccount
                ));
            }
            // Memo is NOT checked on the 3pool transfer path — see fn doc.
            // Vault binding comes from the backend's code-time construction
            // of `vault_id_memo`, which is asserted against the call's
            // `vault_id` at the call site in `vault.rs`.
            Ok(expected.vault_id_memo)
        }
        SpProofLedger::IcusdBurn => {
            let memo = block
                .memo
                .as_ref()
                .ok_or_else(|| "block missing 'memo' field".to_string())?;
            let decoded_vault = decode_writedown_memo(memo)?;
            if decoded_vault != expected.vault_id_memo {
                return Err(format!(
                    "memo vault id {} does not equal expected {}",
                    decoded_vault, expected.vault_id_memo
                ));
            }
            Ok(decoded_vault)
        }
    }
}

/// I/O wrapper: query `icrc3_get_blocks` on `ledger_principal`, decode the
/// returned block, validate against `expected`, and return the vault id on
/// success. Caller is responsible for the consumed-proof bookkeeping (this
/// helper is read-only) and for translating the returned `Err(String)` into
/// the appropriate `ProtocolError` variant.
pub async fn fetch_and_validate_block(
    ledger_principal: Principal,
    block_index: u64,
    expected: &ProofExpectations,
) -> Result<u64, String> {
    let request = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger_principal, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|(code, msg)| {
        format!(
            "icrc3_get_blocks call to {} failed: {:?} {}",
            ledger_principal, code, msg
        )
    })?;

    let block_with_id = response
        .blocks
        .into_iter()
        .find(|b| nat_to_u64_opt(&b.id) == Some(block_index))
        .ok_or_else(|| {
            format!(
                "ledger {} returned no block at index {}",
                ledger_principal, block_index
            )
        })?;

    let decoded = decode_block(&block_with_id.block)?;
    validate_block(&decoded, expected)
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn text_value(v: &ICRC3Value) -> Option<String> {
    match v {
        ICRC3Value::Text(t) => Some(t.clone()),
        _ => None,
    }
}

/// Strip ICRC-3 schema prefix from `btype` (e.g., `"1burn"` → `"burn"`,
/// `"2approve"` → `"approve"`). Lowercases for case-insensitive matching.
fn normalize_op(raw: &str) -> String {
    let trimmed = raw.trim_start_matches(|c: char| c.is_ascii_digit());
    trimmed.to_ascii_lowercase()
}

fn account_from_value(v: &ICRC3Value) -> Result<Account, String> {
    let arr = match v {
        ICRC3Value::Array(a) => a,
        _ => return Err("account is not an Array".to_string()),
    };
    if arr.is_empty() || arr.len() > 2 {
        return Err(format!(
            "account array must have 1 or 2 elements, got {}",
            arr.len()
        ));
    }
    let owner_blob = match &arr[0] {
        ICRC3Value::Blob(b) => b,
        _ => return Err("account owner is not a Blob".to_string()),
    };
    let owner = Principal::try_from_slice(owner_blob.as_ref())
        .map_err(|e| format!("could not decode principal: {}", e))?;
    let subaccount = if arr.len() == 2 {
        match &arr[1] {
            ICRC3Value::Blob(b) => {
                let bytes: [u8; 32] = b
                    .as_ref()
                    .try_into()
                    .map_err(|_| "subaccount is not 32 bytes".to_string())?;
                Some(bytes)
            }
            _ => return Err("account subaccount is not a Blob".to_string()),
        }
    } else {
        None
    };
    Ok(Account { owner, subaccount })
}

fn nat_to_u128(v: &ICRC3Value) -> Result<u128, String> {
    match v {
        ICRC3Value::Nat(n) => n
            .0
            .to_u128()
            .ok_or_else(|| format!("Nat {} does not fit in u128", n)),
        _ => Err("expected Nat value".to_string()),
    }
}

fn nat_to_u64_opt(n: &Nat) -> Option<u64> {
    n.0.to_u64()
}

// ─── Test helpers (not gated on cfg(test) so audit_pocs files can use them) ─

/// Build an `ICRC3Value` shaped like a standard ICRC-3 burn block. Used by
/// audit_pocs unit tests to feed the verifier without spinning up a ledger.
/// `op` is placed under `tx` (3pool style); pass `with_btype = true` to also
/// emit the top-level `btype` (standard ledger style).
pub fn make_test_burn_block(
    from: Account,
    amount_e8s: u64,
    memo: &[u8],
    with_btype: bool,
) -> ICRC3Value {
    make_test_block("burn", Some(from), None, amount_e8s, Some(memo), with_btype)
}

/// Build an `ICRC3Value` shaped like a standard ICRC-3 transfer block. See
/// `make_test_burn_block` for the `with_btype` knob.
pub fn make_test_transfer_block(
    from: Account,
    to: Account,
    amount_e8s: u64,
    memo: &[u8],
    with_btype: bool,
) -> ICRC3Value {
    make_test_block("xfer", Some(from), Some(to), amount_e8s, Some(memo), with_btype)
}

fn make_test_block(
    op: &str,
    from: Option<Account>,
    to: Option<Account>,
    amount_e8s: u64,
    memo: Option<&[u8]>,
    with_btype: bool,
) -> ICRC3Value {
    let mut tx: ICRC3Map = std::collections::BTreeMap::new();
    tx.insert("op".to_string(), ICRC3Value::Text(op.to_string()));
    if let Some(f) = from {
        tx.insert("from".to_string(), account_to_value(f));
    }
    if let Some(t) = to {
        tx.insert("to".to_string(), account_to_value(t));
    }
    tx.insert("amt".to_string(), ICRC3Value::Nat(Nat::from(amount_e8s)));
    if let Some(m) = memo {
        tx.insert(
            "memo".to_string(),
            ICRC3Value::Blob(ByteBuf::from(m.to_vec())),
        );
    }
    let mut block: ICRC3Map = std::collections::BTreeMap::new();
    if with_btype {
        block.insert(
            "btype".to_string(),
            ICRC3Value::Text(format!("1{}", op)),
        );
    }
    block.insert("ts".to_string(), ICRC3Value::Nat(Nat::from(0u64)));
    block.insert("tx".to_string(), ICRC3Value::Map(tx));
    ICRC3Value::Map(block)
}

fn account_to_value(account: Account) -> ICRC3Value {
    let mut parts = vec![ICRC3Value::Blob(ByteBuf::from(account.owner.as_slice().to_vec()))];
    if let Some(sub) = account.subaccount {
        parts.push(ICRC3Value::Blob(ByteBuf::from(sub.to_vec())));
    }
    ICRC3Value::Array(parts)
}

/// Public helper exported for audit_pocs use: builds a memoless burn block.
/// Tests use this to confirm `validate_block` rejects burn blocks without
/// the LIQ-004 memo.
pub fn make_test_block_without_memo(
    op: &str,
    from: Account,
    to: Option<Account>,
    amount_e8s: u64,
) -> ICRC3Value {
    make_test_block(op, Some(from), to, amount_e8s, None, false)
}
