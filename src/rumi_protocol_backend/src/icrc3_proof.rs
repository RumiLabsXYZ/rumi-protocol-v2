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
use icrc_ledger_types::icrc3::blocks::{ArchivedBlocks, BlockWithId, GetBlocksRequest, GetBlocksResult};
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
    /// 3USD transferFrom to the backend's default account, used only by the
    /// receipt-backed V2 reserve-ingress journal.
    ThreePoolTransferDefault,
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
    pub btype: Option<String>,
    pub op: String,
    pub from: Option<Account>,
    pub to: Option<Account>,
    pub spender: Option<Account>,
    pub amount: u128,
    pub transaction_fee: Option<u128>,
    pub fee: Option<u128>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
    pub expected_allowance: Option<u128>,
    pub expires_at: Option<u64>,
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

    let btype = block_map.get("btype").map(|value| {
        text_value(value).ok_or_else(|| "block 'btype' is not Text".to_string())
    }).transpose()?;
    let op = if let Some(btype) = btype.as_ref() {
        normalize_op(btype)
    } else if let Some(value) = tx_map.get("op") {
        normalize_op(&text_value(value).ok_or_else(|| "tx 'op' is not Text".to_string())?)
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

    let transaction_fee = tx_map.get("fee").map(nat_to_u128).transpose()?;
    let block_fee = block_map.get("fee").map(nat_to_u128).transpose()?;
    let fee = match (transaction_fee, block_fee) {
        (Some(tx_fee), Some(actual_fee)) if tx_fee != actual_fee => {
            return Err("block 'fee' and tx 'fee' conflict".to_string());
        }
        (Some(tx_fee), _) | (_, Some(tx_fee)) => Some(tx_fee),
        (None, None) => None,
    };
    let created_at_time = decode_tx_timestamp(tx_map)?;
    let expected_allowance = tx_map.get("expected_allowance").map(nat_to_u128).transpose()?;
    let expires_at = tx_map.get("expires_at").map(|value| {
        nat_to_u128(value)?.try_into().map_err(|_| "tx 'expires_at' does not fit in u64".to_string())
    }).transpose()?;

    let memo = match tx_map.get("memo") {
        Some(ICRC3Value::Blob(b)) => Some(b.to_vec()),
        Some(_) => return Err("tx 'memo' is not a Blob".to_string()),
        None => None,
    };

    Ok(DecodedBlock {
        btype,
        op,
        from,
        to,
        spender,
        amount,
        transaction_fee,
        fee,
        memo,
        created_at_time,
        expected_allowance,
        expires_at,
    })
}

fn decode_tx_timestamp(tx_map: &ICRC3Map) -> Result<Option<u64>, String> {
    let decode = |field: &str| -> Result<Option<u64>, String> {
        tx_map.get(field).map(|value| {
            nat_to_u128(value)?.try_into()
                .map_err(|_| format!("tx '{}' does not fit in u64", field))
        }).transpose()
    };
    let canonical = decode("ts")?;
    let alias = decode("created_at_time")?;
    match (canonical, alias) {
        (Some(canonical), Some(alias)) if canonical != alias => {
            Err("tx 'ts' and 'created_at_time' conflict".to_string())
        }
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
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
        SpProofLedger::ThreePoolTransferDefault => {
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
            if to.owner != expected.reserves_account.owner
                || to.subaccount != expected.reserves_account.subaccount
            {
                return Err(format!(
                    "block 'to' does not equal expected reserves account (owner {} sub {:?})",
                    expected.reserves_account.owner, expected.reserves_account.subaccount
                ));
            }
            // Memo is NOT checked on the 3pool transfer path — see fn doc.
            // Vault binding comes from the backend's code-time construction
            // of `vault_id_memo`, which is asserted against the call's
            // `vault_id` at the call site in `vault.rs`.
            Ok(expected.vault_id_memo)
        }
        SpProofLedger::ThreePoolTransferDefault => {
            let to = block
                .to
                .as_ref()
                .ok_or_else(|| "transfer block missing 'to' field".to_string())?;
            if to.owner != expected.reserves_account.owner || to.subaccount.is_some() {
                return Err("V2 reserve transfer destination is not the backend default account".into());
            }
            let spender = block.spender.as_ref()
                .ok_or_else(|| "V2 reserve transfer block missing ICRC-2 spender".to_string())?;
            if spender.owner != expected.reserves_account.owner || spender.subaccount.is_some() {
                return Err("V2 reserve transfer spender is not the backend default account".into());
            }
            if from.subaccount.is_some() {
                return Err("V2 reserve transfer source must be the Stability Pool default account".into());
            }
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

pub fn validate_three_usd_reserve_ingress_block(
    block: &DecodedBlock,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<(), String> {
    if !matches!(block.btype.as_deref(), Some("2xfer") | None)
        || (block.btype.is_none() && block.spender.is_none())
        || (block.op != "xfer" && block.op != "transfer")
    {
        return Err("reserve ingress receipt is not an ICRC-2 transferFrom block".into());
    }
    let source = tuple.source.clone();
    let spender = Account { owner: tuple.spender_owner, subaccount: tuple.spender_subaccount };
    if !block.from.as_ref().is_some_and(|actual| accounts_match(actual, &source))
        || !block.to.as_ref().is_some_and(|actual| accounts_match(actual, &tuple.destination))
        || !block.spender.as_ref().is_some_and(|actual| accounts_match(actual, &spender))
        || block.amount != u128::from(tuple.amount_e8s)
        || block.transaction_fee != tuple.fee_e8s.map(u128::from)
        || block.fee != Some(0)
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err("reserve ingress receipt does not match its exact persisted ICRC-2 tuple".into());
    }
    Ok(())
}

/// Verify the exact transferFrom record committed by the ingress journal.
/// The ledger index is part of the durable identity; an equal-looking record
/// at another index cannot authorize a second write-down.
pub async fn verify_three_usd_reserve_ingress_block(
    ledger: Principal,
    block_index: u64,
    journal_block_index: u64,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<(), String> {
    if block_index != journal_block_index {
        return Err("3USD receipt block index does not match the persisted ingress journal".into());
    }
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_three_usd_reserve_ingress_block(&block, tuple)
}

pub fn validate_three_usd_default_source_refund_block(
    block: &DecodedBlock,
    tuple: &crate::state::ThreeUsdRefundTransferTuple,
) -> Result<(), String> {
    let source = Account { owner: tuple.source_owner, subaccount: tuple.source_subaccount };
    if (block.op != "xfer" && block.op != "transfer")
        || !block.from.as_ref().is_some_and(|actual| accounts_match(actual, &source))
        || !block.to.as_ref().is_some_and(|actual| accounts_match(actual, &tuple.destination))
        || block.spender.is_some()
        || block.amount != u128::from(tuple.amount_e8s)
        || block.transaction_fee != Some(u128::from(tuple.fee_e8s))
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
    {
        return Err("3USD refund receipt does not match its exact persisted ICRC-1 tuple".into());
    }
    Ok(())
}

pub async fn verify_icrc3_transfer_block(
    ledger: Principal,
    block_index: u64,
    from: Option<Account>,
    to: Account,
    amount_e8s: u64,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
) -> Result<(), String> {
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_icrc3_transfer_block(&block, from, to, amount_e8s, memo, created_at_time)
}

/// Verify an exact ICRC-1 transfer including the fee paid by its source
/// account. Used where the fee payer is part of the accounting invariant.
pub async fn verify_icrc3_transfer_block_with_fee(
    ledger: Principal,
    block_index: u64,
    from: Account,
    to: Account,
    amount_e8s: u64,
    expected_fee: u64,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
) -> Result<(), String> {
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_icrc3_transfer_block(&block, Some(from), to, amount_e8s, memo, created_at_time)?;
    if block.fee != Some(expected_fee as u128) {
        return Err("block fee does not match expected source-paid fee".into());
    }
    Ok(())
}

/// Verify a direct ICRC-1 transfer (not an ICRC-2 `transfer_from`). This
/// distinction matters for protocol fee reserves:
/// an `icrc2_transfer_from` deposit block must not be reused as reserve funding.
pub async fn verify_icrc3_direct_transfer_block(
    ledger: Principal,
    block_index: u64,
    from: Account,
    to: Account,
    amount_e8s: u64,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
) -> Result<(), String> {
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_icrc3_direct_transfer_block(
        &block,
        from,
        to,
        amount_e8s,
        memo,
        created_at_time,
    )
}

pub fn validate_icrc3_direct_transfer_block(
    block: &DecodedBlock,
    expected_from: Account,
    expected_to: Account,
    expected_amount_e8s: u64,
    expected_memo: Option<&[u8]>,
    expected_created_at_time: Option<u64>,
) -> Result<(), String> {
    if block.btype.as_deref() != Some("1xfer") {
        return Err("block does not prove an ICRC-1 1xfer; direct funding remains unverified".into());
    }
    validate_icrc3_transfer_block(
        block,
        Some(expected_from),
        expected_to,
        expected_amount_e8s,
        expected_memo,
        expected_created_at_time,
    )?;
    if block.spender.is_some() {
        return Err("block records an ICRC-2 spender; direct ICRC-1 transfer required".into());
    }
    Ok(())
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



// Exact receipt and archive helpers used by bot claim reconciliation.
pub fn accounts_match(actual: &Account, expected: &Account) -> bool {
    actual.owner == expected.owner
        && match (&actual.subaccount, &expected.subaccount) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual == expected,
            (Some(value), None) | (None, Some(value)) => *value == [0; 32],
        }
}


pub fn validate_icrc3_transfer_block(
    block: &DecodedBlock,
    expected_from: Option<Account>,
    expected_to: Account,
    expected_amount_e8s: u64,
    expected_memo: Option<&[u8]>,
    expected_created_at_time: Option<u64>,
) -> Result<(), String> {
    let expected_op = if expected_from.is_some() {
        "transfer"
    } else {
        "mint"
    };
    // A normalized operation name is not enough to prove an ICRC-1 transfer
    // or mint: when present, btype is authoritative and must use its exact
    // ICRC-1 version. Legacy untyped blocks retain their exact tx.op fallback.
    let expected_btype = if expected_from.is_some() { "1xfer" } else { "1mint" };
    if let Some(btype) = block.btype.as_deref() {
        if btype != expected_btype {
            return Err(format!("block type does not identify an ICRC-1 {}", expected_op));
        }
    }
    if block.op != expected_op && !(expected_op == "transfer" && block.op == "xfer") {
        return Err(format!("expected {} block, got {}", expected_op, block.op));
    }
    if block.amount != expected_amount_e8s as u128 {
        return Err(format!(
            "block amount {} does not equal expected {}",
            block.amount, expected_amount_e8s
        ));
    }
    match (&block.from, &expected_from) {
        (None, None) => {}
        (Some(actual), Some(expected)) if accounts_match(actual, expected) => {}
        _ => return Err("block 'from' account does not match expected account".to_string()),
    }
    if !block
        .to
        .as_ref()
        .is_some_and(|actual| accounts_match(actual, &expected_to))
    {
        return Err("block 'to' account does not match expected account".to_string());
    }
    if let Some(expected_memo) = expected_memo {
        if block.memo.as_deref() != Some(expected_memo) {
            return Err("block memo does not match expected memo".to_string());
        }
    }
    if let Some(expected_time) = expected_created_at_time {
        if block.created_at_time != Some(expected_time) {
            return Err("block created_at_time does not match expected time".to_string());
        }
    }
    Ok(())
}


pub fn validate_icrc3_transfer_block_with_fee(
    block: &DecodedBlock,
    expected_from: Account,
    expected_to: Account,
    expected_amount_e8s: u64,
    expected_fee_raw: u64,
    expected_memo: &[u8],
    expected_created_at_time: u64,
) -> Result<(), String> {
    validate_icrc3_transfer_block(
        block,
        Some(expected_from),
        expected_to,
        expected_amount_e8s,
        Some(expected_memo),
        Some(expected_created_at_time),
    )?;
    if block.transaction_fee != Some(expected_fee_raw as u128) {
        return Err("block tx.fee does not match the persisted explicit fee".to_string());
    }
    Ok(())
}


pub async fn fetch_icrc3_block(
    ledger_principal: Principal,
    block_index: u64,
) -> Result<DecodedBlock, String> {
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
    let block_with_id = resolve_block_with_archive(
        ledger_principal,
        block_index,
        response,
        |archive_id, method, request| async move {
            let result: Result<(GetBlocksResult,), _> =
                ic_cdk::call(archive_id, &method, (request,)).await;
            result.map(|(response,)| response).map_err(|(code, msg)| {
                format!(
                    "ICRC-3 archive call to {} failed: {:?} {}",
                    archive_id, code, msg
                )
            })
        },
    )
    .await?;
    decode_block(&block_with_id.block)
}


pub async fn icrc3_log_length(ledger_principal: Principal) -> Result<u64, String> {
    let request = vec![GetBlocksRequest { start: Nat::from(0u64), length: Nat::from(1u64) }];
    let result: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger_principal, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|(code, msg)| format!(
        "icrc3_get_blocks log-length call to {} failed: {:?} {}", ledger_principal, code, msg
    ))?;
    response.log_length.0.to_u64()
        .ok_or_else(|| "ICRC-3 log length exceeds u64".to_string())
}


async fn resolve_block_with_archive<F, Fut>(
    ledger_principal: Principal,
    block_index: u64,
    response: GetBlocksResult,
    fetch_archive: F,
) -> Result<BlockWithId, String>
where
    F: FnOnce(Principal, String, Vec<GetBlocksRequest>) -> Fut,
    Fut: std::future::Future<Output = Result<GetBlocksResult, String>>,
{
    const MAX_ARCHIVE_DESCRIPTORS: usize = 32;
    const MAX_ARCHIVE_RANGES: usize = 32;
    if response.archived_blocks.len() > MAX_ARCHIVE_DESCRIPTORS
        || response
            .archived_blocks
            .iter()
            .any(|archive| archive.args.len() > MAX_ARCHIVE_RANGES)
    {
        return Err(format!(
            "ledger {} returned excessive archive metadata for index {}",
            ledger_principal, block_index
        ));
    }
    if response.blocks.len() > 1
        || response
            .blocks
            .iter()
            .any(|block| nat_to_u64_opt(&block.id) != Some(block_index))
    {
        return Err(format!(
            "ledger {} returned malformed direct blocks for requested index {}",
            ledger_principal, block_index
        ));
    }
    if let Some(block) = response.blocks.first() {
        if response.archived_blocks.iter().any(|archive| {
            archive
                .args
                .iter()
                .any(|request| request_covers_index(request, block_index))
        }) {
            return Err(format!(
                "ledger {} returned overlapping direct and archived evidence for index {}",
                ledger_principal, block_index
            ));
        }
        return Ok(block.clone());
    }

    let mut covering = response.archived_blocks.iter().filter(|archive| {
        archive
            .args
            .iter()
            .any(|request| request_covers_index(request, block_index))
    });
    let callback = covering.next().ok_or_else(|| {
        format!(
            "ledger {} returned no block or archive descriptor at index {}",
            ledger_principal, block_index
        )
    })?;
    if covering.next().is_some() {
        return Err(format!(
            "ledger {} returned multiple archive descriptors for index {}",
            ledger_principal, block_index
        ));
    }

    let archive_id = callback.callback.canister_id;
    let method = callback.callback.method.clone();
    let request = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let archive_response = fetch_archive(archive_id, method, request).await?;
    if !archive_response.archived_blocks.is_empty()
        || archive_response.blocks.len() != 1
        || nat_to_u64_opt(&archive_response.blocks[0].id) != Some(block_index)
    {
        return Err(format!(
            "archive {} returned malformed block response for index {}",
            archive_id, block_index
        ));
    }
    Ok(archive_response.blocks.into_iter().next().expect("length checked"))
}


fn request_covers_index(request: &GetBlocksRequest, block_index: u64) -> bool {
    let Some(start) = request.start.0.to_u64() else {
        return false;
    };
    if start > block_index || request.length.0 == Nat::from(0u64).0 {
        return false;
    }
    let Some(length) = request.length.0.to_u64() else {
        // Any length above u64::MAX covers every representable block index
        // at or after `start`.
        return true;
    };
    start
        .checked_add(length)
        .map_or(true, |end| block_index < end)
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

#[cfg(test)]
mod three_usd_ingress_receipt_tests {
    use super::*;

    fn tuple() -> crate::state::ThreeUsdReserveIngressTuple {
        crate::state::ThreeUsdReserveIngressTuple {
            spender_owner: Principal::from_slice(&[0x31]),
            spender_subaccount: None,
            source: Account { owner: Principal::from_slice(&[0x32]), subaccount: None },
            destination: Account { owner: Principal::from_slice(&[0x33]), subaccount: None },
            amount_e8s: 123_456,
            fee_e8s: None,
            memo: [0x34; 16],
            created_at_time_ns: 99,
            op_nonce: 100,
            parent_absorb_id: Some(7),
        }
    }

    fn exact_block(tuple: &crate::state::ThreeUsdReserveIngressTuple) -> DecodedBlock {
        DecodedBlock {
            btype: Some("2xfer".into()),
            op: "xfer".into(),
            from: Some(tuple.source),
            to: Some(tuple.destination),
            spender: Some(Account { owner: tuple.spender_owner, subaccount: None }),
            amount: u128::from(tuple.amount_e8s),
            transaction_fee: None,
            fee: Some(0),
            memo: Some(tuple.memo.to_vec()),
            created_at_time: Some(tuple.created_at_time_ns),
            expected_allowance: None,
            expires_at: None,
        }
    }

    #[test]
    fn v2_ingress_receipt_requires_exact_transferfrom_tuple_and_zero_block_fee() {
        let tuple = tuple();
        let exact = exact_block(&tuple);
        assert!(validate_three_usd_reserve_ingress_block(&exact, &tuple).is_ok());

        let mut wrong = exact.clone();
        wrong.amount += 1;
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact.clone();
        wrong.spender = None;
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact;
        wrong.fee = Some(1);
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
    }

    #[test]
    fn archived_receipt_lookup_is_bounded_to_one_advertised_exact_block() {
        let archive = ArchivedBlocks {
            args: vec![GetBlocksRequest { start: Nat::from(40u64), length: Nat::from(4u64) }],
            callback: icrc_ledger_types::icrc3::archive::QueryArchiveFn {
                canister_id: Principal::from_slice(&[0x44]),
                method: "get_blocks".to_string(),
                _marker: std::marker::PhantomData,
            },
        };
        assert!(archive_covers_index(&archive, 43));
        assert!(!archive_covers_index(&archive, 44));

        let exact = GetBlocksResult {
            log_length: Nat::from(100u64),
            blocks: vec![icrc_ledger_types::icrc3::blocks::BlockWithId {
                id: Nat::from(43u64),
                block: make_test_transfer_block(
                    Account { owner: Principal::from_slice(&[0x45]), subaccount: None },
                    Account { owner: Principal::from_slice(&[0x46]), subaccount: None },
                    7,
                    b"receipt",
                    false,
                ),
            }],
            archived_blocks: vec![],
        };
        assert!(extract_exact_archive_block(exact.clone(), 43).is_ok());

        let wrong_id = GetBlocksResult {
            blocks: vec![icrc_ledger_types::icrc3::blocks::BlockWithId { id: Nat::from(44u64), block: exact.blocks[0].block.clone() }],
            ..exact
        };
        assert!(extract_exact_archive_block(wrong_id, 43).is_err());
    }
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

#[cfg(test)]
mod direct_transfer_tests {
    use super::*;

    fn account(byte: u8) -> Account {
        Account {
            owner: Principal::from_slice(&[byte]),
            subaccount: None,
        }
    }

    #[test]
    fn admin_deposit_transfer_from_block_is_rejected_but_direct_funding_is_accepted() {
        let from = account(1);
        let to = account(2);
        let mut block = DecodedBlock {
            btype: Some("1xfer".into()),
            op: "transfer".into(),
            from: Some(from.clone()),
            to: Some(to.clone()),
            spender: None,
            amount: 100,
            transaction_fee: None,
            fee: None,
            memo: Some(b"reserve".to_vec()),
            created_at_time: Some(7),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(validate_icrc3_direct_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            100,
            Some(b"reserve"),
            Some(7),
        )
        .is_ok());

        block.spender = Some(account(3));
        assert!(validate_icrc3_direct_transfer_block(
            &block,
            from,
            to,
            100,
            Some(b"reserve"),
            Some(7),
        )
        .unwrap_err()
        .contains("ICRC-2 spender"));

        // Some ledgers may omit `spender` from a transfer_from transaction;
        // the ICRC-3 btype remains the authoritative operation discriminator.
        block.spender = None;
        block.btype = Some("2xfer".into());
        assert!(validate_icrc3_direct_transfer_block(
            &block,
            from.clone(),
            to.clone(),
            100,
            Some(b"reserve"),
            Some(7),
        )
        .unwrap_err()
        .contains("1xfer"));

        // Unknown/btype-less transfer variants also cannot fund the reserve.
        block.btype = None;
        assert!(validate_icrc3_direct_transfer_block(
            &block,
            from,
            to,
            100,
            Some(b"reserve"),
            Some(7),
        )
        .unwrap_err()
        .contains("1xfer"));
    }
}
