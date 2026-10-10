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
//!     proof-build time. This legacy proof kind asserts op / amount / from /
//!     to and the consumed-proof set blocks block-index replay. The P08 V2
//!     ingress path uses the separate exact tuple validator below, including
//!     spender, memo, created_at_time, fee, and both accounts.
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
use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
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
    /// Receipt-backed reserve ingress on the backend's default account.
    ThreePoolTransferDefault,
}

/// Stable output shape for the pre-V2 consumed-proof monitoring query. Keep
/// new V2 proof kinds out of its result; adding an enum arm would break old
/// generated clients that decode the query response.
#[derive(CandidType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacySpProofLedger {
    IcusdBurn,
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
    pub btype: Option<String>,
    pub op: String,
    pub from: Option<Account>,
    pub to: Option<Account>,
    pub spender: Option<Account>,
    pub amount: u128,
    pub fee: Option<u64>,
    pub created_at_time: Option<u64>,
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

    let btype = block_map.get("btype").and_then(text_value);
    let tx_op = tx_map.get("op").and_then(text_value);
    let op = if let Some(btype) = &btype {
        let normalized_btype = normalize_op(btype);
        if tx_op.as_ref().is_some_and(|tx_op| {
            normalize_semantic_op(tx_op) != normalize_semantic_op(&normalized_btype)
        }) {
            return Err("block type and transaction operation disagree".into());
        }
        normalized_btype
    } else if let Some(op) = tx_op {
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

    // Standard ICRC-3 places fee in `tx`; the in-tree 3pool ledger currently
    // emits its (zero) fee at block level. Prefer transaction metadata when
    // present, then read the ledger's block-level representation.
    let fee = tx_map
        .get("fee")
        .or_else(|| block_map.get("fee"))
        .map(nat_to_u128)
        .transpose()?
        .map(|value| u64::try_from(value).map_err(|_| "block fee does not fit in u64".to_string()))
        .transpose()?;

    let created_at_time = tx_map
        .get("ts")
        .or_else(|| tx_map.get("created_at_time"))
        .map(nat_to_u128)
        .transpose()?
        .map(|value| {
            u64::try_from(value).map_err(|_| "block timestamp does not fit in u64".to_string())
        })
        .transpose()?;

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
        fee,
        created_at_time,
        memo,
    })
}

/// Validate the exact ICRC-2 transferFrom tuple retained by a V2 ingress.
/// In particular, a transfer to the legacy hashed subaccount cannot authorize
/// a write-down under the default-account route.
pub fn validate_three_usd_reserve_ingress_block(
    block: &DecodedBlock,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<u64, String> {
    if (block.op != "xfer" && block.op != "transfer")
        || block.btype.as_deref().is_some_and(|kind| kind != "2xfer")
    {
        return Err("reserve ingress receipt is not a transfer block".into());
    }
    let spender = block.spender.as_ref().ok_or("reserve ingress block is missing spender")?;
    if spender.owner != tuple.spender_owner
        || spender.subaccount != tuple.spender_subaccount
        || block.from.as_ref() != Some(&tuple.source)
        || block.to.as_ref() != Some(&tuple.destination)
        || block.amount != u128::from(tuple.amount_e8s)
        || tuple.fee_e8s != Some(tuple.ledger_fee_e8s)
        || block.fee != Some(tuple.ledger_fee_e8s)
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
    {
        return Err("reserve ingress receipt does not match the persisted ICRC-2 tuple".into());
    }
    block
        .fee
        .ok_or_else(|| "reserve ingress block is missing its actual charged fee".into())
}

/// Validate one complete, contiguous ICRC-3 page for an absence scan. A
/// positive exact match returns its global block index. Any missing/extra ID,
/// archive descriptor, short log-length claim, or undecodable block fails
/// closed; callers may advance a durable cursor only after `Ok(None)`.
pub fn validate_three_usd_ingress_scan_page(
    start: u64,
    end: u64,
    fixed_tip: u64,
    response: &GetBlocksResult,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<Option<u64>, String> {
    if end < start || end > fixed_tip
        || response.log_length.0.to_u64().is_none_or(|length| length < fixed_tip) {
        return Err("ICRC-3 page does not cover the pinned ledger prefix".into());
    }
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed prefix pages are unsupported; absence remains unproven".into());
    }
    let count = usize::try_from(end - start).map_err(|_| "page length exceeds address space")?;
    if response.blocks.len() != count {
        return Err("ICRC-3 page is short or has extra blocks".into());
    }
    let mut match_index = None;
    for (offset, block) in response.blocks.iter().enumerate() {
        let expected_id = start.checked_add(offset as u64).ok_or("block index overflow")?;
        if block.id.0.to_u64() != Some(expected_id) {
            return Err("ICRC-3 page has a gap, duplicate, or out-of-order block ID".into());
        }
        let decoded = decode_block(&block.block)?;
        if validate_three_usd_reserve_ingress_block(&decoded, tuple).is_ok() {
            if match_index.replace(expected_id).is_some() {
                return Err("exact ingress tuple appears more than once in scanned prefix".into());
            }
        } else if decoded.op == "transfer" || decoded.op == "xfer" {
            let could_be_tuple = decoded.from.as_ref() == Some(&tuple.source)
                && decoded.to.as_ref() == Some(&tuple.destination)
                // No spender means an ordinary owner transfer. It cannot be
                // accepted as positive ICRC-2 proof, but old/variant ledger
                // schemas could have omitted this discriminator; keep the
                // absence scan held on the matching envelope rather than
                // treating it as proof that the requested transferFrom did
                // not occur.
                && decoded.spender.as_ref().is_none_or(|spender|
                    spender.owner == tuple.spender_owner && spender.subaccount == tuple.spender_subaccount)
                && decoded.amount == u128::from(tuple.amount_e8s);
            if could_be_tuple {
                // Some ledgers omit memo/created_at_time from ICRC-3. Such a
                // block may be this operation, so it cannot be counted as
                // absence or promoted as positive evidence.
                return Err("ledger block matches ingress principals and amount but omits or changes exact tuple metadata".into());
            }
        }
    }
    Ok(match_index)
}

pub async fn verify_three_usd_reserve_ingress_block(
    ledger: Principal,
    block_index: u64,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<u64, String> {
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_three_usd_reserve_ingress_block(&block, tuple)
}

pub fn validate_three_usd_reserve_refund_block(
    block: &DecodedBlock,
    tuple: &crate::state::ThreeUsdReserveRefundTuple,
) -> Result<(), String> {
    if (block.op != "xfer" && block.op != "transfer")
        || block.btype.as_deref().is_some_and(|kind| kind != "1xfer")
        || block.spender.is_some()
        || block.from.as_ref().map_or(true, |from| {
            from.owner != tuple.source_owner || from.subaccount != tuple.source_subaccount
        })
        || block.to.as_ref() != Some(&tuple.destination)
        || block.amount != u128::from(tuple.amount_e8s)
        || block.fee != Some(tuple.charged_fee_e8s)
        || block.memo.as_deref() != Some(tuple.memo.as_slice())
        || block.created_at_time != Some(tuple.created_at_time_ns)
        || tuple.fee_e8s.is_some_and(|fee| fee != tuple.charged_fee_e8s)
    {
        return Err("3USD refund receipt block does not match the exact persisted transfer tuple".into());
    }
    Ok(())
}

#[cfg(test)]
mod three_usd_reserve_refund_tests {
    use super::{validate_three_usd_reserve_refund_block, DecodedBlock};
    use crate::state::ThreeUsdReserveRefundTuple;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn tuple() -> ThreeUsdReserveRefundTuple {
        ThreeUsdReserveRefundTuple {
            source_owner: Principal::from_slice(&[1]),
            source_subaccount: None,
            destination: Account { owner: Principal::from_slice(&[2]), subaccount: None },
            amount_e8s: 99,
            charged_fee_e8s: 1,
            fee_e8s: None,
            memo: [7; 16],
            created_at_time_ns: 42,
        }
    }

    #[test]
    fn refund_proof_binds_the_actual_block_fee_when_transfer_argument_omits_fee() {
        let tuple = tuple();
        let block = DecodedBlock {
            btype: Some("1xfer".into()),
            op: "transfer".into(),
            from: Some(Account { owner: tuple.source_owner, subaccount: tuple.source_subaccount }),
            to: Some(tuple.destination.clone()),
            spender: None,
            amount: tuple.amount_e8s.into(),
            fee: Some(1),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        };
        assert!(validate_three_usd_reserve_refund_block(&block, &tuple).is_ok());

        let mut changed_fee = block.clone();
        changed_fee.fee = Some(2);
        assert!(validate_three_usd_reserve_refund_block(&changed_fee, &tuple).is_err());

        let mut changed_source = block.clone();
        changed_source.from.as_mut().unwrap().owner = Principal::from_slice(&[9]);
        assert!(validate_three_usd_reserve_refund_block(&changed_source, &tuple).is_err());
    }
}

pub async fn verify_three_usd_reserve_refund_block(
    ledger: Principal,
    block_index: u64,
    tuple: &crate::state::ThreeUsdReserveRefundTuple,
) -> Result<(), String> {
    let block = fetch_icrc3_block(ledger, block_index).await?;
    validate_three_usd_reserve_refund_block(&block, tuple)
}

/// Verify an exact ICRC-1 mint/transfer block, resolving a single advertised
/// archive callback when the ledger has archived the requested index.
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

/// Pure exact-tuple validator shared by the ledger fetch path and focused tests.
pub fn validate_icrc3_transfer_block(
    block: &DecodedBlock,
    from: Option<Account>,
    to: Account,
    amount_e8s: u64,
    memo: Option<&[u8]>,
    created_at_time: Option<u64>,
) -> Result<(), String> {
    // The standard ICRC-3 block schema uses `btype = "1xfer"` and
    // `tx.op = "xfer"`; do not normalize that ledger spelling to the
    // ICRC-1 method name (`transfer`).
    let expected_op = if from.is_some() { "xfer" } else { "mint" };
    let expected_btype = if from.is_some() { "1xfer" } else { "1mint" };
    if block.btype.as_deref().is_some_and(|kind| kind != expected_btype) {
        return Err("ICRC-3 block type does not match the expected transfer type".into());
    }
    if block.op != expected_op {
        return Err("ICRC-3 operation does not match the expected transfer operation".into());
    }
    if block.from != from {
        return Err("ICRC-3 sender does not match the expected account".into());
    }
    if block.spender.is_some() {
        return Err("ICRC-3 transfer unexpectedly names a spender".into());
    }
    if !block
        .to
        .as_ref()
        .is_some_and(|actual| accounts_match_default_subaccount(actual, &to))
    {
        return Err("ICRC-3 recipient does not match the expected account".into());
    }
    if block.amount != u128::from(amount_e8s) {
        return Err("ICRC-3 amount does not match the expected transfer amount".into());
    }
    if memo.is_some_and(|expected| block.memo.as_deref() != Some(expected)) {
        return Err("ICRC-3 memo does not match the expected transfer memo".into());
    }
    if created_at_time.is_some_and(|expected| block.created_at_time != Some(expected)) {
        return Err("ICRC-3 timestamp does not match the expected transfer timestamp".into());
    }
    Ok(())
}

/// Validate the stronger evidence required to recover an AMM1 donation after
/// an ambiguous/TooOld mint outcome. Unlike the generic transfer validator,
/// an ICRC-1 mint must explicitly carry the standard `1mint` block type.
pub fn validate_icrc1_mint_block(
    block: &DecodedBlock,
    to: Account,
    amount_e8s: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    if block.btype.as_deref() != Some("1mint") {
        return Err("ICRC-3 block type is not exactly 1mint".into());
    }
    if block.op != "mint" {
        return Err("ICRC-3 transaction operation is not mint".into());
    }
    if block.from.is_some() {
        return Err("ICRC-3 mint unexpectedly has a sender".into());
    }
    if block.spender.is_some() {
        return Err("ICRC-3 mint unexpectedly names a spender".into());
    }
    if block.to.as_ref() != Some(&to) {
        return Err("ICRC-3 mint recipient does not exactly match the pinned account".into());
    }
    if block.amount != u128::from(amount_e8s) {
        return Err("ICRC-3 mint amount does not match the pinned amount".into());
    }
    if block.memo.as_deref() != Some(memo) {
        return Err("ICRC-3 mint memo does not match the pinned nonce".into());
    }
    if block.created_at_time != Some(created_at_time) {
        return Err("ICRC-3 mint timestamp does not match the pinned nonce".into());
    }
    Ok(())
}

/// Validate a positive ICRC-3 receipt for a persisted borrow mint tuple.
/// It accepts the ICRC-3 backward-compatible form without `btype` only when
/// `tx.op = "mint"`; a present block type must be exactly `1mint`.
pub fn validate_icrc3_borrow_mint_block(
    block: &DecodedBlock,
    tuple: &crate::state::BorrowMintTuple,
) -> Result<(), String> {
    if block.op != "mint" || block.btype.as_deref().is_some_and(|kind| kind != "1mint") {
        return Err("ICRC-3 block is not an exact ICRC-1 mint block".into());
    }
    validate_icrc3_transfer_block(
        block,
        None,
        Account {
            owner: tuple.destination,
            subaccount: None,
        },
        tuple.amount_e8s,
        Some(&tuple.memo),
        Some(tuple.created_at_time_ns),
    )?;
    if block.fee.is_some_and(|fee| fee != 0) {
        return Err("ICRC-3 mint receipt unexpectedly records a nonzero fee".into());
    }
    Ok(())
}

/// Fetch an archive-aware candidate block and validate it against the exact
/// persisted mint arguments. Candidate indexes are untrusted caller input.
pub async fn verify_icrc3_borrow_mint_block(
    ledger: Principal,
    block_index: u64,
    tuple: &crate::state::BorrowMintTuple,
) -> Result<(), String> {
    if tuple.ledger != ledger {
        return Err("borrow mint candidate ledger differs from the persisted tuple".into());
    }
    // Borrow mint authority requires a direct ledger-global block. Archive
    // callback metadata is only a locator and does not authenticate membership.
    let request = vec![GetBlocksRequest { start: Nat::from(block_index), length: Nat::from(1u64) }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (request,))
        .await
        .map_err(|(code, message)| format!("direct icUSD ICRC-3 read failed: {code:?} {message}"))?;
    if response.archived_blocks.len() != 0 || response.blocks.len() != 1
        || response.blocks[0].id.0.to_u64() != Some(block_index)
        || response.log_length.0.to_u64().is_none_or(|length| block_index >= length)
    {
        return Err("candidate is not a direct contiguous ledger block; archive evidence remains held".into());
    }
    let block = decode_block(&response.blocks[0].block)?;
    validate_icrc3_borrow_mint_block(&block, tuple)
}

/// Read the current ledger-global exclusive log length. This is a metadata
/// fence, not a mint probe; callers persist it before dispatch or after typed
/// TooOld and then scan only fixed, direct contiguous pages.
pub async fn icrc3_history_floor(ledger: Principal) -> Result<u64, String> {
    icrc3_log_length(ledger).await
}

/// Validate a complete, direct ledger-global page within [start, end).
/// Returns the unique exact mint index, or None for proven absence on this page.
pub fn validate_borrow_mint_scan_page(
    start: u64, end: u64, fixed_tip: u64, response: &GetBlocksResult,
    tuple: &crate::state::BorrowMintTuple,
) -> Result<Option<u64>, String> {
    if end < start || end - start > 64 || end > fixed_tip || response.log_length.0.to_u64().is_none_or(|n| n < fixed_tip) {
        return Err("ICRC-3 page does not cover the pinned ledger prefix".into());
    }
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed history is unsupported; absence remains unproven".into());
    }
    let count = usize::try_from(end - start).map_err(|_| "page length exceeds address space")?;
    if response.blocks.len() != count { return Err("ICRC-3 page is short or has extra blocks".into()); }
    let mut found = None;
    for (offset, item) in response.blocks.iter().enumerate() {
        let index = start.checked_add(offset as u64).ok_or("block index overflow")?;
        if item.id.0.to_u64() != Some(index) { return Err("ICRC-3 page has a gap, duplicate, or out-of-order ID".into()); }
        let decoded = decode_block(&item.block)?;
        if validate_icrc3_borrow_mint_block(&decoded, tuple).is_ok() {
            if found.replace(index).is_some() { return Err("exact borrow mint appears more than once".into()); }
        } else if (decoded.op == "mint" || decoded.btype.as_deref() == Some("1mint"))
            && decoded.to.as_ref().is_some_and(|to| to.owner == tuple.destination)
            && decoded.amount == u128::from(tuple.amount_e8s)
        {
            return Err("plausible borrow mint has missing or conflicting exact metadata".into());
        }
    }
    Ok(found)
}

fn accounts_match_default_subaccount(actual: &Account, expected: &Account) -> bool {
    actual.owner == expected.owner
        && match (&actual.subaccount, &expected.subaccount) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual == expected,
            (Some(actual), None) | (None, Some(actual)) => *actual == [0; 32],
        }
}

/// Fetch an exact ledger-global block. Archive metadata must identify exactly
/// one bounded callback covering the requested index; nested archives fail
/// closed.
pub async fn fetch_icrc3_block(
    ledger: Principal,
    block_index: u64,
) -> Result<DecodedBlock, String> {
    let request = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = result.map_err(|(code, message)| {
        format!("icrc3_get_blocks call to {ledger} failed: {code:?} {message}")
    })?;
    let log_length = response
        .log_length
        .0
        .to_u64()
        .ok_or_else(|| "ledger log length does not fit in u64".to_string())?;
    if block_index >= log_length {
        return Err("requested block index is outside the ledger log length".into());
    }
    if response.blocks.len() == 1
        && response.blocks[0].id.0.to_u64() == Some(block_index)
        && response.archived_blocks.is_empty()
    {
        return decode_block(&response.blocks[0].block);
    }
    if !response.blocks.is_empty() || response.archived_blocks.len() != 1 {
        return Err("icrc3_get_blocks returned ambiguous or incomplete archive evidence".into());
    }
    let archive = &response.archived_blocks[0];
    if archive.callback.method.trim().is_empty() {
        return Err("archive callback method is empty".into());
    }
    validate_exact_archive_ranges(block_index, &archive.args)?;
    let archive_request = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> = ic_cdk::call(
        archive.callback.canister_id,
        &archive.callback.method,
        (archive_request,),
    )
    .await;
    let (archived,) = result
        .map_err(|(code, message)| format!("icrc3 archive call failed: {code:?} {message}"))?;
    decode_block(exact_archive_callback_block(block_index, &archived)?)
}

fn exact_archive_callback_block(
    block_index: u64,
    response: &GetBlocksResult,
) -> Result<&ICRC3Value, String> {
    if !response.archived_blocks.is_empty()
        || response.blocks.len() != 1
        || response.blocks[0].id.0.to_u64() != Some(block_index)
    {
        return Err("archive callback returned malformed exact block response".into());
    }
    Ok(&response.blocks[0].block)
}

fn validate_exact_archive_ranges(
    block_index: u64,
    args: &[GetBlocksRequest],
) -> Result<(), String> {
    if args.len() > 32 {
        return Err("archive callback has too many advertised ranges".into());
    }
    let mut coverage_count = 0usize;
    for arg in args {
        let start = arg.start.0.to_u64().ok_or_else(|| "archive range start does not fit in u64".to_string())?;
        let len = arg.length.0.to_u64().ok_or_else(|| "archive range length does not fit in u64".to_string())?;
        let end = start.checked_add(len).ok_or_else(|| "archive range overflows u64".to_string())?;
        if start <= block_index && block_index < end {
            coverage_count += 1;
        }
    }
    if coverage_count != 1 {
        return Err("archive callback does not uniquely cover the requested block".into());
    }
    Ok(())
}

pub async fn icrc3_log_length(ledger: Principal) -> Result<u64, String> {
    let request = vec![GetBlocksRequest {
        start: Nat::from(0u64),
        length: Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = result
        .map_err(|(code, message)| format!("icrc3 log-length query failed: {code:?} {message}"))?;
    response
        .log_length
        .0
        .to_u64()
        .ok_or_else(|| "ICRC-3 log length exceeds u64".into())
}

#[cfg(test)]
mod borrow_mint_receipt_tests {
    use super::{decode_block, make_test_block, validate_borrow_mint_scan_page, validate_icrc3_borrow_mint_block, DecodedBlock};
    use crate::state::BorrowMintTuple;
    use candid::Principal;
    use icrc_ledger_types::icrc::generic_value::ICRC3Value;
    use icrc_ledger_types::icrc1::account::Account;

    fn tuple() -> BorrowMintTuple {
        BorrowMintTuple {
            ledger: Principal::from_slice(&[1]),
            destination: Principal::from_slice(&[2]),
            amount_e8s: 123_456,
            memo: [0x5a; 16],
            created_at_time_ns: 987_654,
            op_nonce: 77,
        }
    }

    fn block(tuple: &BorrowMintTuple) -> DecodedBlock {
        DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(Account { owner: tuple.destination, subaccount: None }),
            spender: None,
            amount: u128::from(tuple.amount_e8s),
            fee: None,
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        }
    }

    #[test]
    fn receipt_requires_exact_mint_schema_and_persisted_tuple() {
        let tuple = tuple();
        let expected = block(&tuple);
        assert!(validate_icrc3_borrow_mint_block(&expected, &tuple).is_ok());

        let mut changed = expected.clone();
        changed.btype = None;
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_ok());
        changed.btype = Some("1xfer".into());
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
        let mut changed = expected.clone();
        changed.op = "xfer".into();
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
        let mut changed = expected.clone();
        changed.amount += 1;
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
        let mut changed = expected.clone();
        changed.to.as_mut().unwrap().owner = Principal::from_slice(&[3]);
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
        let mut changed = expected.clone();
        changed.memo.as_mut().unwrap()[0] ^= 1;
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
        let mut changed = expected;
        changed.created_at_time = Some(tuple.created_at_time_ns + 1);
        assert!(validate_icrc3_borrow_mint_block(&changed, &tuple).is_err());
    }

    #[test]
    fn decoder_rejects_conflicting_btype_and_tx_op_but_accepts_legacy_mint() {
        let tuple = tuple();
        let mut legacy = make_test_block(
            "mint",
            None,
            Some(Account { owner: tuple.destination, subaccount: None }),
            tuple.amount_e8s,
            Some(&tuple.memo),
            false,
        );
        if let ICRC3Value::Map(block) = &mut legacy {
            if let Some(ICRC3Value::Map(tx)) = block.get_mut("tx") {
                tx.insert(
                    "ts".into(),
                    ICRC3Value::Nat(candid::Nat::from(tuple.created_at_time_ns)),
                );
            }
        }
        let decoded = decode_block(&legacy).expect("legacy tx.op mint schema decodes");
        assert!(validate_icrc3_borrow_mint_block(&decoded, &tuple).is_ok());

        if let ICRC3Value::Map(block) = &mut legacy {
            block.insert("btype".into(), ICRC3Value::Text("1mint".into()));
            if let Some(ICRC3Value::Map(tx)) = block.get_mut("tx") {
                tx.insert("op".into(), ICRC3Value::Text("burn".into()));
            }
        }
        assert!(decode_block(&legacy).is_err());
    }

    #[test]
    fn absence_scan_requires_complete_direct_exact_page_and_finds_exact_mint() {
        use icrc_ledger_types::icrc3::blocks::{ArchivedBlocks, BlockWithId, GetBlocksResult};
        use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
        let tuple = tuple();
        let mut exact = make_test_block("mint", None,
            Some(Account { owner: tuple.destination, subaccount: None }), tuple.amount_e8s,
            Some(&tuple.memo), false);
        if let ICRC3Value::Map(root) = &mut exact {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") {
                tx.insert("ts".into(), ICRC3Value::Nat(candid::Nat::from(tuple.created_at_time_ns)));
            }
        }
        let page = GetBlocksResult { log_length: 2u64.into(),
            blocks: vec![BlockWithId { id: 1u64.into(), block: exact }], archived_blocks: vec![] };
        assert_eq!(validate_borrow_mint_scan_page(1, 2, 2, &page, &tuple).unwrap(), Some(1));

        let incomplete = GetBlocksResult { log_length: 2u64.into(), blocks: vec![], archived_blocks: vec![] };
        assert!(validate_borrow_mint_scan_page(1, 2, 2, &incomplete, &tuple).is_err());
        let empty_complete = GetBlocksResult { log_length: 2u64.into(), blocks: vec![], archived_blocks: vec![] };
        assert_eq!(validate_borrow_mint_scan_page(2, 2, 2, &empty_complete, &tuple).unwrap(), None);

        let mut conflicting = make_test_block("mint", None,
            Some(Account { owner: tuple.destination, subaccount: None }), tuple.amount_e8s,
            Some(&[0x99; 16]), false);
        if let ICRC3Value::Map(root) = &mut conflicting {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") {
                tx.insert("ts".into(), ICRC3Value::Nat(candid::Nat::from(tuple.created_at_time_ns)));
            }
        }
        let suspicious = GetBlocksResult { log_length: 2u64.into(),
            blocks: vec![BlockWithId { id: 1u64.into(), block: conflicting }], archived_blocks: vec![] };
        assert!(validate_borrow_mint_scan_page(1, 2, 2, &suspicious, &tuple).is_err());

        let archive = ArchivedBlocks { args: vec![], callback: QueryArchiveFn::new(Principal::from_slice(&[9]), "archive") };
        let archived = GetBlocksResult { log_length: 2u64.into(), blocks: vec![], archived_blocks: vec![archive] };
        assert!(validate_borrow_mint_scan_page(1, 2, 2, &archived, &tuple).is_err());
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
///   * This legacy proof kind does not validate ICRC-1 transaction metadata;
///     the P08 V2 route uses `validate_three_usd_reserve_ingress_block` for
///     its exact ICRC-2 tuple. Here the backend constructs proof after the
///     transfer, and the consumed-proof set blocks block-index replay.
pub fn validate_block(block: &DecodedBlock, expected: &ProofExpectations) -> Result<u64, String> {
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
                    "expected default-account transfer block on 3USD ledger, got op={}",
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
        SpProofLedger::ThreePoolTransfer | SpProofLedger::ThreePoolTransferDefault => {
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
    let block = fetch_icrc3_block(ledger_principal, block_index).await?;
    validate_block(&block, expected)
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

fn normalize_semantic_op(raw: &str) -> String {
    let normalized = normalize_op(raw);
    match normalized.as_str() {
        "transfer" => "xfer".into(),
        _ => normalized,
    }
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
        ICRC3Value::Nat(n) => {
            n.0.to_u128()
                .ok_or_else(|| format!("Nat {} does not fit in u128", n))
        }
        _ => Err("expected Nat value".to_string()),
    }
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
    make_test_block(
        "xfer",
        Some(from),
        Some(to),
        amount_e8s,
        Some(memo),
        with_btype,
    )
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
        block.insert("btype".to_string(), ICRC3Value::Text(format!("1{}", op)));
    }
    block.insert("ts".to_string(), ICRC3Value::Nat(Nat::from(0u64)));
    block.insert("tx".to_string(), ICRC3Value::Map(tx));
    ICRC3Value::Map(block)
}

fn account_to_value(account: Account) -> ICRC3Value {
    let mut parts = vec![ICRC3Value::Blob(ByteBuf::from(
        account.owner.as_slice().to_vec(),
    ))];
    if let Some(sub) = account.subaccount {
        parts.push(ICRC3Value::Blob(ByteBuf::from(sub.to_vec())));
    }
    ICRC3Value::Array(parts)
}

#[cfg(test)]
mod three_usd_reserve_ingress_tests {
    use super::{decode_block, exact_archive_callback_block, validate_exact_archive_ranges, validate_icrc1_mint_block, validate_three_usd_ingress_scan_page, validate_three_usd_reserve_ingress_block, DecodedBlock};
    use candid::Nat;
    use icrc_ledger_types::icrc3::blocks::{ArchivedBlocks, BlockWithId, GetBlocksRequest, GetBlocksResult};
    use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
    use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};
    use serde_bytes::ByteBuf;
    use crate::state::ThreeUsdReserveIngressTuple;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn tuple() -> ThreeUsdReserveIngressTuple {
        ThreeUsdReserveIngressTuple {
            spender_owner: Principal::from_slice(&[3]),
            spender_subaccount: None,
            source: Account { owner: Principal::from_slice(&[2]), subaccount: None },
            destination: Account { owner: Principal::from_slice(&[3]), subaccount: None },
            amount_e8s: 42,
            fee_e8s: Some(0),
            ledger_fee_e8s: 0,
            memo: [7; 16],
            created_at_time_ns: 99,
            op_nonce: 4,
            parent_absorb_id: 8,
        }
    }

    fn block(tuple: &ThreeUsdReserveIngressTuple) -> DecodedBlock {
        DecodedBlock {
            btype: Some("2xfer".into()),
            op: "transfer".into(),
            from: Some(tuple.source.clone()),
            to: Some(tuple.destination.clone()),
            spender: Some(Account { owner: tuple.spender_owner, subaccount: tuple.spender_subaccount }),
            amount: tuple.amount_e8s.into(),
            fee: Some(tuple.ledger_fee_e8s),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        }
    }

    fn encoded_block(tuple: &ThreeUsdReserveIngressTuple) -> ICRC3Value {
        let mut tx: ICRC3Map = std::collections::BTreeMap::new();
        tx.insert("op".into(), ICRC3Value::Text("transfer".into()));
        tx.insert("from".into(), super::account_to_value(tuple.source.clone()));
        tx.insert("to".into(), super::account_to_value(tuple.destination.clone()));
        tx.insert("spender".into(), super::account_to_value(Account { owner: tuple.spender_owner, subaccount: tuple.spender_subaccount }));
        tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(tuple.amount_e8s)));
        tx.insert("fee".into(), ICRC3Value::Nat(Nat::from(tuple.ledger_fee_e8s)));
        tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(tuple.created_at_time_ns)));
        tx.insert("memo".into(), ICRC3Value::Blob(ByteBuf::from(tuple.memo.to_vec())));
        let mut outer: ICRC3Map = std::collections::BTreeMap::new();
        outer.insert("btype".into(), ICRC3Value::Text("2xfer".into()));
        outer.insert("tx".into(), ICRC3Value::Map(tx));
        ICRC3Value::Map(outer)
    }

    fn response(ids: &[u64], log_length: u64, tuple: &ThreeUsdReserveIngressTuple) -> GetBlocksResult {
        GetBlocksResult {
            log_length: Nat::from(log_length),
            blocks: ids.iter().map(|id| BlockWithId { id: Nat::from(*id), block: encoded_block(tuple) }).collect(),
            archived_blocks: vec![],
        }
    }

    #[test]
    fn full_prefix_pages_require_every_global_id_and_detect_exact_transfer() {
        let tuple = tuple();
        assert_eq!(validate_three_usd_ingress_scan_page(0, 1, 3, &response(&[0], 3, &tuple), &tuple), Ok(Some(0)));
        let mut different_tuple = tuple.clone();
        different_tuple.amount_e8s += 1;
        assert_eq!(validate_three_usd_ingress_scan_page(0, 1, 3, &response(&[0], 3, &tuple), &different_tuple), Ok(None));
        let mut owner_transfer = response(&[0], 3, &tuple);
        if let ICRC3Value::Map(block) = &mut owner_transfer.blocks[0].block {
            if let Some(ICRC3Value::Map(tx)) = block.get_mut("tx") { tx.remove("spender"); }
        }
        assert!(validate_three_usd_ingress_scan_page(0, 1, 3, &owner_transfer, &tuple).is_err());
        assert!(validate_three_usd_ingress_scan_page(0, 2, 3, &response(&[0], 3, &tuple), &tuple).is_err());
        assert!(validate_three_usd_ingress_scan_page(0, 2, 3, &response(&[0, 2], 3, &tuple), &tuple).is_err());
        assert!(validate_three_usd_ingress_scan_page(0, 2, 3, &response(&[0, 1], 1, &tuple), &tuple).is_err());
        let archived = GetBlocksResult {
            log_length: Nat::from(2u64), blocks: vec![],
            archived_blocks: vec![ArchivedBlocks {
                args: vec![GetBlocksRequest { start: Nat::from(0u64), length: Nat::from(2u64) }],
                callback: QueryArchiveFn { canister_id: Principal::from_slice(&[8]), method: "get_blocks".into(), _marker: std::marker::PhantomData },
            }],
        };
        assert!(validate_three_usd_ingress_scan_page(0, 2, 2, &archived, &tuple).is_err());
    }

    #[test]
    fn real_3pool_encoder_output_satisfies_backend_exact_transfer_from_verifier() {
        let tuple = tuple();
        let block = rumi_3pool::types::Icrc3Block {
            id: 5,
            timestamp: 777,
            tx: rumi_3pool::types::Icrc3Transaction::Transfer {
                from: tuple.source.owner,
                to: tuple.destination.owner,
                amount: u128::from(tuple.amount_e8s),
                spender: Some(tuple.spender_owner),
                from_subaccount: tuple.source.subaccount.map(|sub| sub.to_vec()),
                to_subaccount: tuple.destination.subaccount.map(|sub| sub.to_vec()),
                spender_subaccount: tuple.spender_subaccount.map(|sub| sub.to_vec()),
                memo: Some(tuple.memo.to_vec()),
                created_at_time: Some(tuple.created_at_time_ns),
            },
        };
        let encoded = rumi_3pool::icrc3::encode_block_with_phash(&block, None);
        let bytes = candid::encode_one(encoded).unwrap();
        let wire_value: ICRC3Value = candid::decode_one(&bytes).unwrap();
        let decoded = decode_block(&wire_value).unwrap();
        assert_eq!(validate_three_usd_reserve_ingress_block(&decoded, &tuple), Ok(0));

        let owner_transfer = rumi_3pool::types::Icrc3Block {
            id: 6,
            timestamp: 778,
            tx: rumi_3pool::types::Icrc3Transaction::Transfer {
                from: tuple.source.owner,
                to: tuple.destination.owner,
                amount: u128::from(tuple.amount_e8s),
                spender: None,
                from_subaccount: tuple.source.subaccount.map(|sub| sub.to_vec()),
                to_subaccount: tuple.destination.subaccount.map(|sub| sub.to_vec()),
                spender_subaccount: tuple.spender_subaccount.map(|sub| sub.to_vec()),
                memo: Some(tuple.memo.to_vec()),
                created_at_time: Some(tuple.created_at_time_ns),
            },
        };
        let owner_wire = rumi_3pool::icrc3::encode_block_with_phash(&owner_transfer, None);
        let owner_bytes = candid::encode_one(owner_wire).unwrap();
        let owner_wire_value: ICRC3Value = candid::decode_one(&owner_bytes).unwrap();
        let decoded_owner_transfer = decode_block(&owner_wire_value).unwrap();
        assert!(validate_three_usd_reserve_ingress_block(&decoded_owner_transfer, &tuple).is_err());
    }

    #[test]
    fn reserve_ingress_proof_binds_the_exact_default_account_tuple() {
        let tuple = tuple();
        let exact = block(&tuple);
        assert_eq!(validate_three_usd_reserve_ingress_block(&exact, &tuple), Ok(0));

        // V2 pins the current 3pool's zero fee in the ICRC-2 tuple. A fee
        // change between preflight and execution must fail closed instead of
        // changing tracked pool accounting.
        let mut fee_drifted = exact.clone();
        fee_drifted.fee = Some(17);
        assert!(validate_three_usd_reserve_ingress_block(&fee_drifted, &tuple).is_err());

        let mut wrong = exact.clone();
        wrong.to.as_mut().unwrap().subaccount = Some([1; 32]);
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact.clone();
        wrong.spender.as_mut().unwrap().owner = Principal::from_slice(&[9]);
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact.clone();
        wrong.amount += 1;
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut missing_fee = exact.clone();
        missing_fee.fee = None;
        assert!(validate_three_usd_reserve_ingress_block(&missing_fee, &tuple).is_err());
        let mut wrong = exact.clone();
        wrong.btype = Some("1xfer".into());
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact.clone();
        wrong.created_at_time = Some(100);
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
        let mut wrong = exact;
        wrong.memo = Some(vec![0; 16]);
        assert!(validate_three_usd_reserve_ingress_block(&wrong, &tuple).is_err());
    }

    #[test]
    fn amm1_receipt_requires_exact_standard_mint_tuple() {
        let to = Account { owner: Principal::from_slice(&[19]), subaccount: Some([8; 32]) };
        let memo = [7u8; 16];
        let exact = DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(to.clone()),
            spender: None,
            amount: 42,
            fee: None,
            created_at_time: Some(99),
            memo: Some(memo.to_vec()),
        };
        assert!(validate_icrc1_mint_block(&exact, to.clone(), 42, &memo, 99).is_ok());

        let mut wrong = exact.clone();
        wrong.btype = None;
        assert!(validate_icrc1_mint_block(&wrong, to.clone(), 42, &memo, 99).is_err());
        let mut wrong = exact.clone();
        wrong.op = "burn".into();
        assert!(validate_icrc1_mint_block(&wrong, to.clone(), 42, &memo, 99).is_err());
        let mut wrong = exact.clone();
        wrong.from = Some(Account { owner: Principal::from_slice(&[20]), subaccount: None });
        assert!(validate_icrc1_mint_block(&wrong, to.clone(), 42, &memo, 99).is_err());
        let mut wrong = exact.clone();
        wrong.spender = Some(Account { owner: Principal::from_slice(&[20]), subaccount: None });
        assert!(validate_icrc1_mint_block(&wrong, to.clone(), 42, &memo, 99).is_err());
        let mut wrong = exact.clone();
        wrong.to.as_mut().unwrap().subaccount = None;
        assert!(validate_icrc1_mint_block(&wrong, to.clone(), 42, &memo, 99).is_err());
        assert!(validate_icrc1_mint_block(&exact, to.clone(), 43, &memo, 99).is_err());
        assert!(validate_icrc1_mint_block(&exact, to.clone(), 42, &[9; 16], 99).is_err());
        assert!(validate_icrc1_mint_block(&exact, to, 42, &memo, 100).is_err());

        // btype cannot override a contradictory transaction operation.
        if let ICRC3Value::Map(outer) = super::make_test_block(
            "mint", None, Some(Account { owner: Principal::from_slice(&[19]), subaccount: Some([8; 32]) }), 42, Some(&memo), true,
        ) {
            let mut outer = outer;
            if let Some(ICRC3Value::Map(tx)) = outer.get_mut("tx") {
                tx.insert("op".into(), ICRC3Value::Text("burn".into()));
            }
            assert!(decode_block(&ICRC3Value::Map(outer)).is_err());
        }
    }

    #[test]
    fn exact_archive_ranges_reject_overflow_and_ambiguous_or_unrepresentable_ranges() {
        let range = |start: Nat, length: Nat| GetBlocksRequest { start, length };
        assert!(validate_exact_archive_ranges(5, &[range(Nat::from(4u64), Nat::from(2u64))]).is_ok());
        assert!(validate_exact_archive_ranges(5, &[range(Nat::from(u64::MAX), Nat::from(2u64))]).is_err());
        assert!(validate_exact_archive_ranges(5, &[range(Nat::from(0u64), Nat::from(6u64)), range(Nat::from(5u64), Nat::from(1u64))]).is_err());
        assert!(validate_exact_archive_ranges(5, &[range(Nat::from(0u64), Nat::from(5u64))]).is_err());
        assert!(validate_exact_archive_ranges(5, &[range(Nat::from(0u64), Nat::from(u128::from(u64::MAX) + 1))]).is_err());
        let too_many = (0..33).map(|_| range(Nat::from(0u64), Nat::from(6u64))).collect::<Vec<_>>();
        assert!(validate_exact_archive_ranges(5, &too_many).is_err());
    }

    #[test]
    fn archive_callback_must_return_one_exact_non_nested_block() {
        let tuple = tuple();
        let exact = response(&[7], 8, &tuple);
        assert!(exact_archive_callback_block(7, &exact).is_ok());
        assert!(exact_archive_callback_block(6, &exact).is_err());
        assert!(exact_archive_callback_block(7, &response(&[], 8, &tuple)).is_err());
        assert!(exact_archive_callback_block(7, &response(&[7, 8], 9, &tuple)).is_err());
        let mut nested = response(&[7], 8, &tuple);
        nested.archived_blocks.push(ArchivedBlocks {
            args: vec![GetBlocksRequest { start: Nat::from(7u64), length: Nat::from(1u64) }],
            callback: QueryArchiveFn { canister_id: Principal::from_slice(&[9]), method: "nested".into(), _marker: std::marker::PhantomData },
        });
        assert!(exact_archive_callback_block(7, &nested).is_err());
    }

    #[test]
    fn archived_proof_block_is_checked_against_the_configured_expectations() {
        let tuple = tuple();
        let archive_response = response(&[7], 8, &tuple);
        let decoded = decode_block(exact_archive_callback_block(7, &archive_response).unwrap())
            .unwrap();
        let expected = super::ProofExpectations {
            ledger_kind: super::SpProofLedger::ThreePoolTransferDefault,
            expected_amount_e8s: tuple.amount_e8s,
            sp_principal: tuple.source.owner,
            reserves_account: tuple.destination.clone(),
            vault_id_memo: 42,
        };

        assert_eq!(super::validate_block(&decoded, &expected), Ok(42));

        let mut wrong_expectations = expected;
        wrong_expectations.expected_amount_e8s += 1;
        assert!(super::validate_block(&decoded, &wrong_expectations).is_err());
    }
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
mod bot_payment_proof_tests {
    use super::*;

    fn fixture() -> (DecodedBlock, Account, Account, Vec<u8>) {
        let bot = Account { owner: Principal::from_slice(&[1]), subaccount: None };
        let backend = Account { owner: Principal::from_slice(&[2]), subaccount: None };
        let mut memo = b"RUMI-BOT-PAYMENT-V1:".to_vec();
        memo.extend_from_slice(&7u64.to_be_bytes());
        memo.extend_from_slice(&42u64.to_be_bytes());
        let block = DecodedBlock {
            btype: Some("1xfer".into()), op: "xfer".into(),
            from: Some(bot.clone()), to: Some(backend.clone()), spender: None,
            amount: 1_000_000, fee: None, created_at_time: Some(123), memo: Some(memo.clone()),
        };
        (block, bot, backend, memo)
    }

    #[test]
    fn bot_payment_proof_accepts_exact_tuple_and_rejects_forged_fields() {
        let (block, bot, backend, memo) = fixture();
        assert!(validate_icrc3_transfer_block(
            &block, Some(bot.clone()), backend.clone(), 1_000_000, Some(&memo), Some(123)
        ).is_ok());

        let mut forged = block.clone();
        forged.from = Some(Account { owner: Principal::from_slice(&[3]), subaccount: None });
        assert!(validate_icrc3_transfer_block(
            &forged, Some(bot), backend, 1_000_000, Some(&memo), Some(123)
        ).is_err());

        let (mut forged, bot, backend, memo) = fixture();
        forged.to = Some(Account { owner: Principal::from_slice(&[4]), subaccount: None });
        assert!(validate_icrc3_transfer_block(
            &forged, Some(bot), backend, 1_000_000, Some(&memo), Some(123)
        ).is_err());
    }

    #[test]
    fn bot_payment_proof_rejects_short_amount_wrong_memo_and_wrong_time() {
        let (mut block, bot, backend, memo) = fixture();
        block.amount -= 1;
        assert!(validate_icrc3_transfer_block(
            &block, Some(bot.clone()), backend.clone(), 1_000_000, Some(&memo), Some(123)
        ).is_err());
        block.amount += 1;
        let other_memo = b"other";
        assert!(validate_icrc3_transfer_block(
            &block, Some(bot.clone()), backend.clone(), 1_000_000, Some(other_memo), Some(123)
        ).is_err());
        assert!(validate_icrc3_transfer_block(
            &block, Some(bot), backend, 1_000_000, Some(&memo), Some(124)
        ).is_err());
    }
}
