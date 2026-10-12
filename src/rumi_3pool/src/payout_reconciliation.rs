//! Positive-only recovery of an aged ambiguous 3pool payout.
//!
//! A caller supplies one candidate ledger block index. This module accepts it
//! only when the ledger directly serves the exact ICRC-3 block or serves an
//! archived candidate linked through consecutive phash values to a direct
//! ledger block.
//! It deliberately does not interpret missing, partial, or unavailable history
//! as proof that a payout did not happen.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::payouts::{PayoutInputTransfer, PayoutTransfer};

/// One bounded step in proving an archived payout block belongs to the
/// configured ledger's append-only ICRC-3 chain. This lives in its own stable
/// memory region so existing payout records and their Candid shape do not
/// change.
#[derive(CandidType, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ArchivePayoutProofProgress {
    pub(crate) ledger: Principal,
    pub(crate) candidate_block_index: u64,
    pub(crate) attempt_number: u32,
    pub(crate) transfer_digest: [u8; 32],
    pub(crate) transfer: PayoutTransfer,
    pub(crate) latest_verified_block_index: u64,
    /// The next block whose parent hash must equal `prior_block_hash`.
    pub(crate) next_block_index: u64,
    pub(crate) prior_block_hash: [u8; 32],
}

/// Provisional inbound proof cursor, separate from outbound payout proofs.
#[derive(CandidType, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ArchiveInputProofProgress {
    pub(crate) ledger: Principal,
    pub(crate) candidate_block_index: u64,
    pub(crate) transfer_digest: [u8; 32],
    pub(crate) transfer: PayoutInputTransfer,
    pub(crate) spender: Account,
    pub(crate) input_action: Option<crate::payouts::PayoutInputAction>,
    pub(crate) payout_kind: crate::payouts::PayoutKind,
    pub(crate) compensation_for: Option<u64>,
    pub(crate) latest_verified_block_index: u64,
    pub(crate) next_block_index: u64,
    pub(crate) prior_block_hash: [u8; 32],
}

// One callback per block is a valid archive response shape. Keep the maximum
// page at the callback cap so even that shape can make bounded progress.
const MAX_ARCHIVE_CHAIN_BLOCKS_PER_CALL: u64 = 8;
const MAX_ARCHIVE_CALLBACKS_PER_CALL: usize = 8;
const MAX_ARCHIVE_BLOCK_BYTES: usize = 64 * 1024;
const MAX_ARCHIVE_PAGE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecodedTransferBlock {
    pub(crate) btype: Option<String>,
    pub(crate) op: String,
    pub(crate) from: Option<Account>,
    pub(crate) to: Option<Account>,
    pub(crate) spender: Option<Account>,
    pub(crate) amount: u128,
    pub(crate) fee: Option<u128>,
    pub(crate) memo: Option<Vec<u8>>,
    pub(crate) created_at_time: Option<u64>,
}

pub(crate) fn validate_exact_transfer(
    block: &DecodedTransferBlock,
    transfer: &PayoutTransfer,
) -> Result<(), String> {
    if block.btype.as_deref().is_some_and(|value| value != "1xfer")
        || block.op != "xfer"
        || block.from.as_ref() != Some(&transfer.from)
        || block.to.as_ref() != Some(&transfer.to)
        || block.spender.is_some()
        || block.amount != transfer.net
        || block.fee != Some(transfer.fee)
        || block.memo.as_deref() != Some(transfer.memo.as_slice())
        || block.created_at_time != Some(transfer.created_at_time)
    {
        return Err("ICRC-3 block does not match the exact persisted payout tuple".into());
    }
    Ok(())
}

/// A positive input proof must identify the same ICRC-2 spender and every
/// immutable field of the pre-call journal. Legacy inputs missing a pinned fee
/// or memo cannot be proved by this route.
pub(crate) fn validate_exact_input_transfer(
    block: &DecodedTransferBlock,
    transfer: &PayoutInputTransfer,
    spender: Account,
) -> Result<(), String> {
    // Early ICRC-3 histories may omit btype while retaining tx.op=xfer.
    // In that form the exact spender distinguishes this ICRC-2 pull from an
    // ordinary transfer. An explicit 1xfer type must never pass.
    if !matches!(block.btype.as_deref(), None | Some("2xfer"))
        || block.op != "xfer"
        || block.from.as_ref() != Some(&transfer.from)
        || block.to.as_ref() != Some(&transfer.to)
        || block.spender.as_ref() != Some(&spender)
        || block.amount != transfer.amount
        || transfer.fee.is_none()
        || block.fee != transfer.fee
        || transfer.memo.is_none()
        || block.memo != transfer.memo
        || block.created_at_time != Some(transfer.created_at_time)
    {
        return Err("ICRC-3 block does not match the exact persisted input tuple".into());
    }
    Ok(())
}

/// Verify an exact payout block. A block served directly by the configured
/// ledger is already consensus-backed by this inter-canister update call. For
/// an archived candidate, walk a bounded number of consecutive blocks at a
/// time and persist the verified cursor until the chain reaches a block served
/// directly by that ledger.
pub(crate) async fn verify_exact_payout_block(
    claim_id: u64,
    attempt_number: u32,
    ledger: Principal,
    block_index: u64,
    transfer: &PayoutTransfer,
) -> Result<(), String> {
    let mut progress = match crate::storage::payout_archive_proofs::get(claim_id) {
        Some(progress) => {
            if progress.ledger != ledger
                || progress.candidate_block_index != block_index
                || progress.attempt_number != attempt_number
                || progress.transfer_digest != transfer_digest(transfer)?
                || progress.transfer != *transfer
                || progress.latest_verified_block_index.checked_add(1)
                    != Some(progress.next_block_index)
            {
                crate::storage::payout_archive_proofs::remove(claim_id);
                return Err("stored archive proof belongs to a different payout attempt".into());
            }
            progress
        }
        None => {
            let candidate_page = fetch_ledger_page(ledger, block_index, 1).await?;
            let candidate = candidate_page
                .into_iter()
                .next()
                .filter(|candidate| candidate.index == block_index)
                .ok_or_else(|| "ledger did not return the exact candidate block".to_string())?;
            let decoded = decode_block(&candidate.block)?;
            validate_exact_transfer(&decoded, transfer)?;
            if candidate.direct_from_ledger {
                return Ok(());
            }
            // Non-genesis ICRC-3 blocks must carry their parent hash even when
            // the proof starts at this archived candidate.
            block_parent_hash(&candidate.block, block_index)?;
            let next_block_index = block_index
                .checked_add(1)
                .ok_or_else(|| "candidate block index overflow".to_string())?;
            let progress = ArchivePayoutProofProgress {
                ledger,
                candidate_block_index: block_index,
                attempt_number,
                transfer_digest: transfer_digest(transfer)?,
                transfer: transfer.clone(),
                latest_verified_block_index: block_index,
                next_block_index,
                prior_block_hash: block_hash(&candidate.block)?,
            };
            if !crate::storage::payout_archive_proofs::insert(claim_id, progress.clone()) {
                return Err("bounded archive proof cursor capacity is exhausted".into());
            }
            progress
        }
    };

    let page = fetch_ledger_page(
        ledger,
        progress.next_block_index,
        MAX_ARCHIVE_CHAIN_BLOCKS_PER_CALL,
    )
    .await?;
    if page.is_empty() {
        return Err("ledger returned no blocks while archive proof remains incomplete".into());
    }

    for block in page {
        if block.index != progress.next_block_index {
            crate::storage::payout_archive_proofs::remove(claim_id);
            return Err("ICRC-3 archive proof contains a block gap or wrong ID".into());
        }
        let parent_hash = block_parent_hash(&block.block, block.index).map_err(|reason| {
            crate::storage::payout_archive_proofs::remove(claim_id);
            reason
        })?;
        if !verify_chain_link(progress.prior_block_hash, parent_hash) {
            crate::storage::payout_archive_proofs::remove(claim_id);
            return Err("ICRC-3 archive block does not link to the prior verified block".into());
        }
        if block.direct_from_ledger {
            // Its parent hash authenticates the archived prefix against this
            // directly served block. The anchor's own hash is not needed.
            crate::storage::payout_archive_proofs::remove(claim_id);
            return Ok(());
        }
        progress.prior_block_hash = block_hash(&block.block).map_err(|reason| {
            crate::storage::payout_archive_proofs::remove(claim_id);
            reason
        })?;
        progress.latest_verified_block_index = block.index;
        progress.next_block_index = block
            .index
            .checked_add(1)
            .ok_or_else(|| "archive proof block index overflow".to_string())?;

        // Commit each verified link so a later archive outage or upgrade can
        // resume from the last authenticated block, never skip over a gap.
        if !crate::storage::payout_archive_proofs::insert(claim_id, progress.clone()) {
            return Err("bounded archive proof cursor capacity is exhausted".into());
        }
    }

    Err("archive proof is still progressing; retry reconciliation to continue".into())
}

fn verify_chain_link(prior_hash: [u8; 32], parent_hash: Option<[u8; 32]>) -> bool {
    parent_hash == Some(prior_hash)
}

fn transfer_digest(transfer: &PayoutTransfer) -> Result<[u8; 32], String> {
    use sha2::Digest;
    let encoded = candid::encode_one(transfer)
        .map_err(|error| format!("could not encode payout tuple for archive proof: {error}"))?;
    Ok(sha2::Sha256::digest(encoded).into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FetchedBlock {
    index: u64,
    block: ICRC3Value,
    direct_from_ledger: bool,
}

/// Fetch at most `length` blocks, following only the exact archive callbacks
/// returned by the ledger for this range. The result is the longest complete
/// consecutive prefix; a gap before any later returned block is rejected.
async fn fetch_ledger_page(
    ledger: Principal,
    start: u64,
    length: u64,
) -> Result<Vec<FetchedBlock>, String> {
    if length == 0 || length > MAX_ARCHIVE_CHAIN_BLOCKS_PER_CALL {
        return Err("ICRC-3 proof page length is outside its fixed bound".into());
    }
    let requested_end = start
        .checked_add(length)
        .ok_or_else(|| "ICRC-3 proof page range overflow".to_string())?;
    let args = vec![GetBlocksRequest {
        start: Nat::from(start),
        length: Nat::from(length),
    }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args,))
        .await
        .map_err(|(code, message)| {
            format!("icrc3_get_blocks call to {ledger} failed: {code:?} {message}")
        })?;
    let log_length = response
        .log_length
        .0
        .to_u64()
        .ok_or_else(|| "ICRC-3 log length exceeds u64".to_string())?;
    if log_length <= start {
        return Err("ICRC-3 log length does not include the requested block".into());
    }
    let page_end = requested_end.min(log_length);
    let expected_count = usize::try_from(page_end - start)
        .map_err(|_| "ICRC-3 proof page length exceeds platform limits".to_string())?;
    if response.blocks.len() > expected_count {
        return Err("ledger returned too many direct blocks for the bounded page".into());
    }
    let mut blocks = BTreeMap::<u64, FetchedBlock>::new();

    let mut page_bytes = 0usize;
    for block in response.blocks {
        page_bytes = page_bytes.checked_add(checked_block_bytes(&block.block)?)
            .filter(|bytes| *bytes <= MAX_ARCHIVE_PAGE_BYTES)
            .ok_or_else(|| "ICRC-3 direct block page exceeds the byte bound".to_string())?;
        insert_page_block(&mut blocks, block.id, block.block, true, start, page_end)?;
    }

    if response.archived_blocks.len() > MAX_ARCHIVE_CALLBACKS_PER_CALL
        || response.archived_blocks.len() > expected_count
    {
        return Err("ledger returned too many archive callbacks for the bounded page".into());
    }
    let mut archive_requested_ids = std::collections::BTreeSet::new();
    for archived in response.archived_blocks {
        if archived.callback.method.trim().is_empty()
            || archived.callback.method.as_bytes().len() > 128
            || archived.callback.canister_id == Principal::anonymous()
            || archived.args.is_empty()
            || archived.args.len() > expected_count
        {
            return Err("ledger returned an invalid ICRC-3 archive callback".into());
        }
        let mut callback_ids = Vec::new();
        for arg in &archived.args {
            let arg_start = arg
                .start
                .0
                .to_u64()
                .ok_or_else(|| "archive callback start exceeds u64".to_string())?;
            let arg_length = arg
                .length
                .0
                .to_u64()
                .ok_or_else(|| "archive callback length exceeds u64".to_string())?;
            let arg_end = arg_start
                .checked_add(arg_length)
                .ok_or_else(|| "archive callback range overflow".to_string())?;
            if arg_length == 0 || arg_start < start || arg_end > page_end {
                return Err("archive callback range exceeds the requested proof page".into());
            }
            for index in arg_start..arg_end {
                if !archive_requested_ids.insert(index) {
                    return Err("archive callbacks contain overlapping block ranges".into());
                }
                callback_ids.push(index);
            }
        }
        if callback_ids.len() > expected_count {
            return Err("archive callback work exceeds the bounded proof page".into());
        }
        let callback_args = candid::encode_args((&archived.args,))
            .map_err(|error| format!("could not encode bounded archive callback args: {error}"))?;
        page_bytes = page_bytes.checked_add(callback_args.len())
            .filter(|bytes| *bytes <= MAX_ARCHIVE_PAGE_BYTES)
            .ok_or_else(|| "archive callback args exceed the page byte bound".to_string())?;
        let (archive_response,): (GetBlocksResult,) = ic_cdk::call(
            archived.callback.canister_id,
            archived.callback.method.as_str(),
            (archived.args,),
        )
        .await
        .map_err(|(code, message)| {
            format!("ICRC-3 archive callback failed: {code:?} {message}")
        })?;
        if !archive_response.archived_blocks.is_empty()
            || archive_response.blocks.len() > callback_ids.len()
        {
            return Err("archive callback returned nested or oversized block history".into());
        }
        let allowed_ids: std::collections::BTreeSet<u64> = callback_ids.into_iter().collect();
        for block in archive_response.blocks {
            page_bytes = page_bytes.checked_add(checked_block_bytes(&block.block)?)
                .filter(|bytes| *bytes <= MAX_ARCHIVE_PAGE_BYTES)
                .ok_or_else(|| "ICRC-3 archive page exceeds the byte bound".to_string())?;
            let id = block
                .id
                .0
                .to_u64()
                .ok_or_else(|| "archived block ID exceeds u64".to_string())?;
            if !allowed_ids.contains(&id) {
                return Err("archive callback returned a block outside its requested range".into());
            }
            insert_page_block(&mut blocks, block.id, block.block, false, start, page_end)?;
        }
    }

    complete_block_prefix(blocks, start, page_end)
}

fn checked_block_bytes(block: &ICRC3Value) -> Result<usize, String> {
    let encoded = candid::encode_one(block)
        .map_err(|error| format!("could not encode ICRC-3 block for size check: {error}"))?;
    if encoded.len() > MAX_ARCHIVE_BLOCK_BYTES {
        return Err("ICRC-3 block exceeds the per-block byte bound".into());
    }
    Ok(encoded.len())
}

fn complete_block_prefix(
    blocks: BTreeMap<u64, FetchedBlock>,
    start: u64,
    end: u64,
) -> Result<Vec<FetchedBlock>, String> {
    let mut prefix_end = start;
    while prefix_end < end && blocks.contains_key(&prefix_end) {
        prefix_end += 1;
    }
    if (prefix_end..end).any(|index| blocks.contains_key(&index)) {
        return Err("ICRC-3 ledger and archive responses contain a gap before a later block".into());
    }
    if prefix_end == start {
        return Err("ICRC-3 ledger and archive responses contain no complete block prefix".into());
    }
    Ok(blocks
        .into_iter()
        .take_while(|(index, _)| *index < prefix_end)
        .map(|(_, block)| block)
        .collect())
}

fn insert_page_block(
    blocks: &mut BTreeMap<u64, FetchedBlock>,
    id: Nat,
    block: ICRC3Value,
    direct_from_ledger: bool,
    start: u64,
    end: u64,
) -> Result<(), String> {
    let index = id
        .0
        .to_u64()
        .ok_or_else(|| "ICRC-3 block ID exceeds u64".to_string())?;
    if index < start || index >= end {
        return Err("ICRC-3 response returned a block outside the requested range".into());
    }
    if blocks
        .insert(
            index,
            FetchedBlock {
                index,
                block,
                direct_from_ledger,
            },
        )
        .is_some()
    {
        return Err("ICRC-3 response contains duplicate block IDs".into());
    }
    Ok(())
}

fn block_parent_hash(block: &ICRC3Value, index: u64) -> Result<Option<[u8; 32]>, String> {
    let map = match block {
        ICRC3Value::Map(map) => map,
        _ => return Err("ICRC-3 block is not a map".into()),
    };
    match map.get("phash") {
        Some(ICRC3Value::Blob(bytes)) if bytes.len() == 32 => {
            let mut hash = [0; 32];
            hash.copy_from_slice(bytes);
            Ok(Some(hash))
        }
        Some(_) => Err("ICRC-3 parent hash is not a 32-byte blob".into()),
        None if index == 0 => Ok(None),
        None => Err("non-genesis ICRC-3 block has no parent hash".into()),
    }
}

/// `icrc-ledger-types` 0.1.x hashes `Int` by converting through i128 and can
/// panic for wider Candid integers. Its positive-Int encoding also differs
/// from the signed representation required by ICRC-3. Refuse any Int value
/// (including nested values) before calling the dependency hash implementation
/// so unsupported ledger blocks fail closed instead of trapping this canister.
fn block_hash(block: &ICRC3Value) -> Result<[u8; 32], String> {
    fn contains_int(value: &ICRC3Value) -> bool {
        match value {
            ICRC3Value::Int(_) => true,
            ICRC3Value::Array(values) => values.iter().any(contains_int),
            ICRC3Value::Map(values) => values.values().any(contains_int),
            ICRC3Value::Blob(_) | ICRC3Value::Text(_) | ICRC3Value::Nat(_) => false,
        }
    }

    if contains_int(block) {
        return Err(
            "ICRC-3 block contains Int, which the pinned hash implementation cannot safely authenticate"
                .into(),
        );
    }
    Ok(block.clone().hash())
}

pub(crate) async fn verify_exact_input_block(
    claim_id: u64,
    ledger: Principal,
    block_index: u64,
    transfer: &PayoutInputTransfer,
    spender: Account,
    input_action: Option<crate::payouts::PayoutInputAction>,
    payout_kind: crate::payouts::PayoutKind,
    compensation_for: Option<u64>,
) -> Result<(), String> {
    let mut progress = match crate::storage::input_archive_proofs::get(claim_id) {
        Some(progress) => {
            if progress.ledger != ledger
                || progress.candidate_block_index != block_index
                || progress.transfer != *transfer
                || progress.spender != spender
                || progress.input_action != input_action
                || progress.payout_kind != payout_kind
                || progress.compensation_for != compensation_for
                || progress.transfer_digest != input_transfer_digest(transfer, &spender, input_action, payout_kind, compensation_for)?
                || progress.latest_verified_block_index.checked_add(1)
                    != Some(progress.next_block_index)
            {
                crate::storage::input_archive_proofs::remove(claim_id);
                return Err("stored archive proof belongs to a different input identity".into());
            }
            progress
        }
        None => {
            let page = fetch_ledger_page(ledger, block_index, 1).await?;
            let candidate = page.into_iter().next()
                .filter(|candidate| candidate.index == block_index)
                .ok_or_else(|| "ledger did not return the exact input candidate block".to_string())?;
            let decoded = decode_block(&candidate.block)?;
            validate_exact_input_transfer(&decoded, transfer, spender.clone())?;
            if candidate.direct_from_ledger {
                return Ok(());
            }
            block_parent_hash(&candidate.block, block_index)?;
            let next_block_index = block_index.checked_add(1)
                .ok_or_else(|| "input candidate block index overflow".to_string())?;
            let progress = ArchiveInputProofProgress {
                ledger,
                candidate_block_index: block_index,
                transfer_digest: input_transfer_digest(transfer, &spender, input_action, payout_kind, compensation_for)?,
                transfer: transfer.clone(),
                spender,
                input_action,
                payout_kind,
                compensation_for,
                latest_verified_block_index: block_index,
                next_block_index,
                prior_block_hash: block_hash(&candidate.block)?,
            };
            if !crate::storage::input_archive_proofs::insert(claim_id, progress.clone()) {
                return Err("bounded input archive proof cursor capacity is exhausted".into());
            }
            progress
        }
    };

    let page = fetch_ledger_page(ledger, progress.next_block_index, MAX_ARCHIVE_CHAIN_BLOCKS_PER_CALL).await?;
    if page.is_empty() {
        return Err("ledger returned no blocks while input archive proof remains incomplete".into());
    }
    for block in page {
        if block.index != progress.next_block_index {
            crate::storage::input_archive_proofs::remove(claim_id);
            return Err("ICRC-3 input archive proof contains a block gap or wrong ID".into());
        }
        let parent_hash = block_parent_hash(&block.block, block.index).map_err(|reason| {
            crate::storage::input_archive_proofs::remove(claim_id);
            reason
        })?;
        if !verify_chain_link(progress.prior_block_hash, parent_hash) {
            crate::storage::input_archive_proofs::remove(claim_id);
            return Err("ICRC-3 input archive block does not link to the prior verified block".into());
        }
        if block.direct_from_ledger {
            crate::storage::input_archive_proofs::remove(claim_id);
            return Ok(());
        }
        progress.prior_block_hash = block_hash(&block.block).map_err(|reason| {
            crate::storage::input_archive_proofs::remove(claim_id);
            reason
        })?;
        progress.latest_verified_block_index = block.index;
        progress.next_block_index = block.index.checked_add(1)
            .ok_or_else(|| "input archive proof block index overflow".to_string())?;
        if !crate::storage::input_archive_proofs::insert(claim_id, progress.clone()) {
            return Err("bounded input archive proof cursor capacity is exhausted".into());
        }
    }
    Err("input archive proof is still progressing; retry reconciliation to continue".into())
}

fn input_transfer_digest(
    transfer: &PayoutInputTransfer,
    spender: &Account,
    input_action: Option<crate::payouts::PayoutInputAction>,
    payout_kind: crate::payouts::PayoutKind,
    compensation_for: Option<u64>,
) -> Result<[u8; 32], String> {
    use sha2::Digest;
    let encoded = candid::encode_args((transfer, spender, input_action, payout_kind, compensation_for))
        .map_err(|error| format!("could not encode input tuple for archive proof: {error}"))?;
    Ok(sha2::Sha256::digest(encoded).into())
}

#[cfg(test)]
fn direct_block_value(response: &GetBlocksResult, block_index: u64) -> Result<ICRC3Value, String> {
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed ICRC-3 proof is unsupported; transfer remains held".into());
    }
    if response.blocks.len() != 1 || response.blocks[0].id.0.to_u64() != Some(block_index) {
        return Err("ICRC-3 did not return the exact requested ledger block".into());
    }
    Ok(response.blocks[0].block.clone())
}

fn decode_block(value: &ICRC3Value) -> Result<DecodedTransferBlock, String> {
    let block = match value {
        ICRC3Value::Map(map) => map,
        _ => return Err("ICRC-3 block is not a map".into()),
    };
    block
        .get("ts")
        .ok_or_else(|| "ICRC-3 block has no required timestamp".to_string())
        .and_then(nat_to_u128)?;
    if let Some(phash) = block.get("phash") {
        match phash {
            ICRC3Value::Blob(bytes) if bytes.len() == 32 => {}
            _ => return Err("ICRC-3 parent hash is not a 32-byte blob".into()),
        }
    }
    let tx = match block.get("tx") {
        Some(ICRC3Value::Map(map)) => map,
        _ => return Err("ICRC-3 block has no transaction map".into()),
    };
    let btype = block
        .get("btype")
        .map(|value| match value {
            ICRC3Value::Text(value) => Ok(value.clone()),
            _ => Err("ICRC-3 block type is not text".to_string()),
        })
        .transpose()?;
    let btype_op = btype
        .as_deref()
        .map(|value| {
            normalize_btype(value).ok_or_else(|| "ICRC-3 block type is unsupported".to_string())
        })
        .transpose()?;
    let tx_op = tx
        .get("op")
        .map(|value| match value {
            ICRC3Value::Text(value) => Ok(value.clone()),
            _ => Err("ICRC-3 transaction operation is not text".to_string()),
        })
        .transpose()?;
    if btype_op
        .as_ref()
        .zip(tx_op.as_ref())
        .is_some_and(|(left, right)| left != right)
    {
        return Err("ICRC-3 block type conflicts with transaction operation".into());
    }
    let op = btype_op
        .or(tx_op)
        .ok_or_else(|| "ICRC-3 block has no recognized transfer operation".to_string())?;
    let from = tx.get("from").map(account_value).transpose()?;
    let to = tx.get("to").map(account_value).transpose()?;
    let spender = tx.get("spender").map(account_value).transpose()?;
    let amount = tx
        .get("amt")
        .ok_or_else(|| "ICRC-3 transfer has no amount".to_string())
        .and_then(nat_to_u128)?;
    let tx_fee = tx.get("fee").map(nat_to_u128).transpose()?;
    let block_fee = block.get("fee").map(nat_to_u128).transpose()?;
    if tx_fee.zip(block_fee).is_some_and(|(tx, block)| tx != block) {
        return Err("ICRC-3 block has conflicting fee fields".into());
    }
    let fee = tx_fee.or(block_fee);
    let memo = tx
        .get("memo")
        .map(|value| match value {
            ICRC3Value::Blob(bytes) => Ok(bytes.to_vec()),
            _ => Err("ICRC-3 transfer memo is not a blob".to_string()),
        })
        .transpose()?;
    let ts = tx.get("ts").map(nat_to_u128).transpose()?;
    let named_ts = tx.get("created_at_time").map(nat_to_u128).transpose()?;
    if ts.zip(named_ts).is_some_and(|(ts, named)| ts != named) {
        return Err("ICRC-3 block has conflicting transfer timestamp fields".into());
    }
    let created_at_time = ts
        .or(named_ts)
        .map(|time| {
            u64::try_from(time).map_err(|_| "ICRC-3 transfer timestamp exceeds u64".to_string())
        })
        .transpose()?;

    Ok(DecodedTransferBlock {
        btype,
        op,
        from,
        to,
        spender,
        amount,
        fee,
        memo,
        created_at_time,
    })
}

fn account_value(value: &ICRC3Value) -> Result<Account, String> {
    let parts = match value {
        ICRC3Value::Array(parts) if (1..=2).contains(&parts.len()) => parts,
        _ => return Err("ICRC-3 account has malformed shape".into()),
    };
    let owner = match &parts[0] {
        ICRC3Value::Blob(bytes) => Principal::try_from_slice(bytes)
            .map_err(|_| "ICRC-3 account owner is malformed".to_string())?,
        _ => return Err("ICRC-3 account owner is not a blob".into()),
    };
    let subaccount = parts
        .get(1)
        .map(|value| match value {
            ICRC3Value::Blob(bytes) if bytes.len() == 32 => {
                let mut subaccount = [0; 32];
                subaccount.copy_from_slice(bytes);
                Ok(subaccount)
            }
            _ => Err("ICRC-3 account subaccount is malformed".to_string()),
        })
        .transpose()?;
    Ok(Account { owner, subaccount })
}

fn nat_to_u128(value: &ICRC3Value) -> Result<u128, String> {
    match value {
        ICRC3Value::Nat(value) => value
            .0
            .to_u128()
            .ok_or_else(|| "ICRC-3 integer exceeds u128".to_string()),
        _ => Err("ICRC-3 integer is not Nat".into()),
    }
}

fn normalize_btype(btype: &str) -> Option<String> {
    match btype {
        "1xfer" => Some("xfer".into()),
        "1mint" => Some("mint".into()),
        "1burn" => Some("burn".into()),
        // ICRC-3 distinguishes transfer_from by btype; tx.op remains xfer.
        "2xfer" => Some("xfer".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icrc_ledger_types::icrc::generic_value::ICRC3Value as V;
    use std::collections::BTreeMap;

    fn transfer() -> PayoutTransfer {
        PayoutTransfer {
            ledger: Principal::from_slice(&[9]),
            from: Account {
                owner: Principal::from_slice(&[1]),
                subaccount: None,
            },
            to: Account {
                owner: Principal::from_slice(&[2]),
                subaccount: None,
            },
            gross: 110,
            net: 100,
            fee: 10,
            memo: vec![3; 32],
            created_at_time: 77,
        }
    }

    fn exact_block(transfer: &PayoutTransfer) -> DecodedTransferBlock {
        DecodedTransferBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(transfer.from.clone()),
            to: Some(transfer.to.clone()),
            spender: None,
            amount: transfer.net,
            fee: Some(transfer.fee),
            memo: Some(transfer.memo.clone()),
            created_at_time: Some(transfer.created_at_time),
        }
    }

    #[test]
    fn icrc3_representation_independent_hash_matches_spec_vector() {
        let hash = V::Nat(Nat::from(42u32)).hash();
        assert_eq!(
            hash,
            [
                0x68, 0x48, 0x88, 0xc0, 0xeb, 0xb1, 0x7f, 0x37, 0x42, 0x98, 0xb6, 0x5e,
                0xe2, 0x80, 0x75, 0x26, 0xc0, 0x66, 0x09, 0x4c, 0x70, 0x1b, 0xcc, 0x7e,
                0xbb, 0xe1, 0xc1, 0x09, 0x5f, 0x49, 0x4f, 0xc1,
            ]
        );
    }

    #[test]
    fn unsupported_int_values_fail_closed_without_hashing_or_trapping() {
        let values = [
            V::Int(64i64.into()),
            V::Int((-42i64).into()),
            V::Map(BTreeMap::from([(
                "nested".into(),
                V::Array(vec![V::Int(candid::Int(candid::Int::from(1u64).0 << 200usize))]),
            )])),
        ];
        for value in values {
            assert!(block_hash(&value)
                .unwrap_err()
                .contains("contains Int"));
        }
    }

    #[test]
    fn archive_candidate_must_link_through_child_to_direct_anchor() {
        let candidate = V::Map(BTreeMap::from([("ts".into(), V::Nat(Nat::from(1u64)))]));
        let candidate_hash = candidate.clone().hash();
        let child = V::Map(BTreeMap::from([(
            "phash".into(),
            V::Blob(candidate_hash.to_vec().into()),
        )]));
        let child_hash = child.clone().hash();
        let direct_anchor = V::Map(BTreeMap::from([(
            "phash".into(),
            V::Blob(child_hash.to_vec().into()),
        )]));

        assert!(verify_chain_link(candidate_hash, block_parent_hash(&child, 1).unwrap()));
        assert!(verify_chain_link(child_hash, block_parent_hash(&direct_anchor, 2).unwrap()));
        assert!(!verify_chain_link([0; 32], block_parent_hash(&child, 1).unwrap()));
        assert!(block_parent_hash(&V::Map(BTreeMap::new()), 1).is_err());
        assert!(block_parent_hash(
            &V::Map(BTreeMap::from([("phash".into(), V::Blob(vec![0; 31].into()))])),
            1
        )
        .is_err());
    }

    #[test]
    fn direct_anchor_needs_matching_parent_hash_but_not_its_own_value_hash() {
        let candidate = V::Map(BTreeMap::from([("ts".into(), V::Nat(Nat::from(1u64)))]));
        let candidate_hash = block_hash(&candidate).unwrap();
        let direct_anchor = V::Map(BTreeMap::from([
            ("phash".into(), V::Blob(candidate_hash.to_vec().into())),
            ("extension".into(), V::Int(64i64.into())),
        ]));
        assert!(verify_chain_link(
            candidate_hash,
            block_parent_hash(&direct_anchor, 1).unwrap()
        ));
        assert!(block_hash(&direct_anchor).is_err());
    }

    #[test]
    fn bounded_page_accepts_short_complete_prefix_but_rejects_internal_gap() {
        let block = |index| FetchedBlock {
            index,
            block: V::Map(BTreeMap::new()),
            direct_from_ledger: true,
        };
        let short = BTreeMap::from([(10, block(10)), (11, block(11))]);
        assert_eq!(complete_block_prefix(short, 10, 14).unwrap().len(), 2);

        let gap = BTreeMap::from([(10, block(10)), (12, block(12))]);
        assert!(complete_block_prefix(gap, 10, 14)
            .unwrap_err()
            .contains("gap before a later block"));
    }

    #[test]
    fn archive_progress_is_storable_across_upgrade_and_binds_attempt() {
        use ic_stable_structures::storable::Storable;
        let transfer = transfer();
        let progress = ArchivePayoutProofProgress {
            ledger: transfer.ledger,
            candidate_block_index: 17,
            attempt_number: 3,
            transfer_digest: transfer_digest(&transfer).unwrap(),
            transfer,
            latest_verified_block_index: 17,
            next_block_index: 18,
            prior_block_hash: [0x5a; 32],
        };
        let encoded = progress.to_bytes();
        let decoded = ArchivePayoutProofProgress::from_bytes(encoded);
        assert_eq!(decoded, progress);
        assert_ne!(
            decoded.transfer_digest,
            transfer_digest(&PayoutTransfer {
                memo: vec![4; 32],
                ..decoded.transfer.clone()
            })
            .unwrap()
        );
    }

    #[test]
    fn input_archive_progress_is_storable_and_binds_spender_and_settlement_context() {
        use ic_stable_structures::storable::Storable;
        let payout = transfer();
        let input = PayoutInputTransfer {
            ledger: payout.ledger,
            from: payout.from,
            to: payout.to,
            amount: 100,
            fee: Some(10),
            memo: Some(vec![8; 32]),
            created_at_time: 77,
        };
        let spender = Account { owner: Principal::from_slice(&[7]), subaccount: None };
        let progress = ArchiveInputProofProgress {
            ledger: input.ledger,
            candidate_block_index: 12,
            transfer_digest: input_transfer_digest(
                &input, &spender, Some(crate::payouts::PayoutInputAction::Swap),
                crate::payouts::PayoutKind::SwapInputRefund, Some(44),
            ).unwrap(),
            transfer: input.clone(),
            spender: spender.clone(),
            input_action: Some(crate::payouts::PayoutInputAction::Swap),
            payout_kind: crate::payouts::PayoutKind::SwapInputRefund,
            compensation_for: Some(44),
            latest_verified_block_index: 12,
            next_block_index: 13,
            prior_block_hash: [0x5a; 32],
        };
        let decoded = ArchiveInputProofProgress::from_bytes(progress.to_bytes());
        assert_eq!(decoded, progress);
        assert_ne!(
            decoded.transfer_digest,
            input_transfer_digest(
                &input,
                &Account { owner: Principal::from_slice(&[6]), subaccount: None },
                decoded.input_action,
                decoded.payout_kind,
                decoded.compensation_for,
            ).unwrap(),
        );
        assert_ne!(
            decoded.transfer_digest,
            input_transfer_digest(
                &input, &spender, Some(crate::payouts::PayoutInputAction::Donation),
                decoded.payout_kind, decoded.compensation_for,
            ).unwrap(),
        );
    }

    #[test]
    fn oversized_nested_icrc3_vectors_fail_before_hashing_or_persisting() {
        let block = V::Map(BTreeMap::from([(
            "tx".into(), V::Array(vec![V::Blob(vec![7; MAX_ARCHIVE_BLOCK_BYTES].into())]),
        )]));
        assert!(checked_block_bytes(&block).unwrap_err().contains("per-block byte bound"));
    }

    #[test]
    fn input_proof_requires_exact_icrc2_tuple_and_spender() {
        let pool = Account { owner: Principal::from_slice(&[7]), subaccount: None };
        let transfer = PayoutInputTransfer {
            ledger: Principal::from_slice(&[9]),
            from: Account { owner: Principal::from_slice(&[1]), subaccount: None },
            to: pool,
            amount: 100,
            fee: Some(10),
            memo: Some(vec![3; 32]),
            created_at_time: 77,
        };
        let block = DecodedTransferBlock {
            btype: Some("2xfer".into()),
            op: "xfer".into(),
            from: Some(transfer.from),
            to: Some(transfer.to),
            spender: Some(pool),
            amount: transfer.amount,
            fee: transfer.fee,
            memo: transfer.memo.clone(),
            created_at_time: Some(transfer.created_at_time),
        };
        assert!(validate_exact_input_transfer(&block, &transfer, pool).is_ok());
        let mut legacy_without_btype = block.clone();
        legacy_without_btype.btype = None;
        assert!(validate_exact_input_transfer(&legacy_without_btype, &transfer, pool).is_ok());
        for mutate in 0..9 {
            let mut altered = block.clone();
            match mutate {
                0 => altered.btype = Some("1xfer".into()),
                1 => altered.from.as_mut().unwrap().owner = Principal::from_slice(&[4]),
                2 => altered.to.as_mut().unwrap().owner = Principal::from_slice(&[4]),
                3 => altered.spender.as_mut().unwrap().owner = Principal::from_slice(&[4]),
                4 => altered.spender = None,
                5 => altered.amount += 1,
                6 => altered.fee = Some(11),
                7 => altered.memo.as_mut().unwrap()[0] ^= 1,
                _ => altered.created_at_time = Some(78),
            }
            assert!(validate_exact_input_transfer(&altered, &transfer, pool).is_err());
        }
        let mut legacy = transfer.clone();
        legacy.memo = None;
        assert!(validate_exact_input_transfer(&block, &legacy, pool).is_err());
        legacy.memo = transfer.memo;
        legacy.fee = None;
        assert!(validate_exact_input_transfer(&block, &legacy, pool).is_err());
    }

    #[test]
    fn decoder_accepts_icrc3_transfer_from_shape() {
        let owner = Account { owner: Principal::from_slice(&[1]), subaccount: None };
        let pool = Account { owner: Principal::from_slice(&[7]), subaccount: None };
        let account = |value: Account| V::Array(vec![V::Blob(value.owner.as_slice().to_vec().into())]);
        let mut tx = BTreeMap::new();
        tx.insert("op".into(), V::Text("xfer".into()));
        tx.insert("from".into(), account(owner));
        tx.insert("to".into(), account(pool));
        tx.insert("spender".into(), account(pool));
        tx.insert("amt".into(), V::Nat(Nat::from(100u64)));
        tx.insert("fee".into(), V::Nat(Nat::from(10u64)));
        tx.insert("memo".into(), V::Blob(vec![3; 32].into()));
        tx.insert("ts".into(), V::Nat(Nat::from(77u64)));
        let mut block = BTreeMap::new();
        block.insert("btype".into(), V::Text("2xfer".into()));
        block.insert("ts".into(), V::Nat(Nat::from(78u64)));
        block.insert("tx".into(), V::Map(tx));
        let decoded = decode_block(&V::Map(block)).expect("ICRC-3 transfer_from has xfer operation");
        assert_eq!(decoded.btype.as_deref(), Some("2xfer"));
        assert_eq!(decoded.op, "xfer");
        assert_eq!(decoded.spender, Some(pool));
    }

    #[test]
    fn exact_block_requires_every_persisted_transfer_field() {
        let transfer = transfer();
        let block = exact_block(&transfer);
        assert!(validate_exact_transfer(&block, &transfer).is_ok());
        for mutate in 0..7 {
            let mut altered = block.clone();
            match mutate {
                0 => altered.from.as_mut().unwrap().owner = Principal::from_slice(&[4]),
                1 => altered.to.as_mut().unwrap().owner = Principal::from_slice(&[4]),
                2 => altered.amount += 1,
                3 => altered.fee = Some(11),
                4 => altered.memo.as_mut().unwrap()[0] ^= 1,
                5 => altered.created_at_time = Some(78),
                _ => {
                    altered.spender = Some(Account {
                        owner: Principal::from_slice(&[4]),
                        subaccount: None,
                    })
                }
            }
            assert!(validate_exact_transfer(&altered, &transfer).is_err());
        }
        let mut missing_metadata = block;
        missing_metadata.memo = None;
        assert!(validate_exact_transfer(&missing_metadata, &transfer).is_err());
    }

    #[test]
    fn decoder_rejects_malformed_accounts_and_ambiguous_fee_metadata() {
        let transfer = transfer();
        let mut tx = BTreeMap::new();
        tx.insert("op".into(), V::Text("xfer".into()));
        tx.insert(
            "from".into(),
            V::Array(vec![V::Blob(
                transfer.from.owner.as_slice().to_vec().into(),
            )]),
        );
        tx.insert(
            "to".into(),
            V::Array(vec![V::Blob(transfer.to.owner.as_slice().to_vec().into())]),
        );
        tx.insert("amt".into(), V::Nat(Nat::from(transfer.net)));
        tx.insert("fee".into(), V::Nat(Nat::from(transfer.fee)));
        tx.insert("memo".into(), V::Blob(transfer.memo.clone().into()));
        tx.insert("ts".into(), V::Nat(Nat::from(transfer.created_at_time)));
        let mut block = BTreeMap::new();
        block.insert("btype".into(), V::Text("1xfer".into()));
        block.insert("ts".into(), V::Nat(Nat::from(1u64)));
        block.insert("tx".into(), V::Map(tx));
        assert!(decode_block(&V::Map(block.clone())).is_ok());

        let mut missing_timestamp = block.clone();
        missing_timestamp.remove("ts");
        assert!(decode_block(&V::Map(missing_timestamp)).is_err());

        let mut malformed_parent_hash = block.clone();
        malformed_parent_hash.insert("phash".into(), V::Blob(vec![0; 31].into()));
        assert!(decode_block(&V::Map(malformed_parent_hash)).is_err());

        let mut conflicting_fee = block.clone();
        conflicting_fee.insert("fee".into(), V::Nat(Nat::from(11u64)));
        assert!(decode_block(&V::Map(conflicting_fee)).is_err());

        let mut malformed_account = block.clone();
        let mut tx = match malformed_account.remove("tx").unwrap() {
            V::Map(tx) => tx,
            _ => unreachable!(),
        };
        tx.insert(
            "from".into(),
            V::Array(vec![V::Text("not an owner blob".into())]),
        );
        malformed_account.insert("tx".into(), V::Map(tx));
        assert!(decode_block(&V::Map(malformed_account)).is_err());

        block.insert("fee".into(), V::Text("not-a-number".into()));
        let mut tx = match block.remove("tx").unwrap() {
            V::Map(tx) => tx,
            _ => unreachable!(),
        };
        tx.remove("fee");
        block.insert("tx".into(), V::Map(tx));
        assert!(decode_block(&V::Map(block)).is_err());
    }

    #[test]
    fn archive_backed_response_is_held_without_following_callback() {
        use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
        use icrc_ledger_types::icrc3::blocks::ArchivedBlocks;

        let response = GetBlocksResult {
            log_length: Nat::from(20u64),
            blocks: vec![],
            archived_blocks: vec![ArchivedBlocks {
                args: vec![GetBlocksRequest {
                    start: Nat::from(10u64),
                    length: Nat::from(1u64),
                }],
                callback: QueryArchiveFn::new(Principal::from_slice(&[4]), "icrc3_get_blocks"),
            }],
        };
        assert!(direct_block_value(&response, 10)
            .unwrap_err()
            .contains("archive-backed ICRC-3 proof is unsupported"));
    }

    #[test]
    fn direct_block_response_requires_one_exact_id_and_no_archive() {
        use icrc_ledger_types::icrc3::blocks::BlockWithId;

        let block = V::Map(BTreeMap::new());
        let response = GetBlocksResult {
            log_length: Nat::from(20u64),
            blocks: vec![BlockWithId {
                id: Nat::from(10u64),
                block: block.clone(),
            }],
            archived_blocks: vec![],
        };
        assert_eq!(direct_block_value(&response, 10), Ok(block));
        assert!(direct_block_value(&response, 11).is_err());
    }
}
