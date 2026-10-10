//! Positive-only recovery of an aged ambiguous AMM payout.
//!
//! A caller supplies one candidate ledger block index. This module accepts it
//! only when the ledger directly serves the exact ICRC-3 block.
//! It deliberately does not interpret missing, partial, or unavailable history
//! as proof that a payout did not happen.

use crate::state::OutboundPayout;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use serde::Deserialize;
use sha2::{Digest, Sha224};

const NATIVE_ICP_LEDGER_TEXT: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";

pub(crate) fn is_native_icp_ledger(ledger: Principal) -> bool {
    ledger.to_text() == NATIVE_ICP_LEDGER_TEXT
}

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
    transfer: &OutboundPayout,
) -> Result<(), String> {
    let from = Account {
        owner: transfer.from,
        subaccount: transfer.from_subaccount,
    };
    let to = Account {
        owner: transfer.to,
        subaccount: transfer.to_subaccount,
    };
    if block.btype.as_deref().is_some_and(|value| value != "1xfer")
        || block.op != "xfer"
        || !block
            .from
            .as_ref()
            .is_some_and(|actual| account_matches(actual, &from))
        || !block
            .to
            .as_ref()
            .is_some_and(|actual| account_matches(actual, &to))
        || block.spender.is_some()
        || block.amount != transfer.net_amount
        || block.fee != Some(transfer.fee)
        || block.memo.as_deref() != Some(transfer.memo.as_slice())
        || block.created_at_time != Some(transfer.created_at_time)
        || transfer.net_amount.checked_add(transfer.fee) != Some(transfer.gross_amount)
    {
        return Err("ICRC-3 block does not match the exact persisted payout tuple".into());
    }
    Ok(())
}

/// Some ledgers encode an omitted default subaccount as an explicit zero
/// subaccount. Accept that canonicalization only for `None`; nonzero
/// subaccounts must match byte-for-byte.
fn account_matches(actual: &Account, expected: &Account) -> bool {
    if actual.owner != expected.owner {
        return false;
    }
    match (actual.subaccount, expected.subaccount) {
        (actual, expected) if actual == expected => true,
        (Some(actual), None) | (None, Some(actual)) => actual == [0; 32],
        _ => false,
    }
}

pub(crate) async fn verify_exact_block(
    ledger: Principal,
    block_index: u64,
    transfer: &OutboundPayout,
) -> Result<(), String> {
    if is_native_icp_ledger(ledger) {
        return verify_exact_native_block(ledger, block_index, transfer).await;
    }
    let args = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args,))
        .await
        .map_err(|(code, message)| {
        format!("icrc3_get_blocks call to {ledger} failed: {code:?} {message}")
    })?;
    if nat_to_u64(&response.log_length)? <= block_index {
        return Err("ICRC-3 log length does not include the supplied block".into());
    }

    let value = direct_block_value(&response, block_index)?;

    let decoded = decode_block(&value)?;
    validate_exact_transfer(&decoded, transfer)
}

// The canonical ICP ledger uses its legacy query_blocks schema rather than
// ICRC-3. This trusts the replicated response from the configured canonical
// ledger canister; it is not a certified block-hash proof. Keep these wire
// types local and accept only a direct response. Archive callback results are
// intentionally not followed or treated as authenticated membership here.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeTimestamp {
    timestamp_nanos: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeTokens {
    e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum NativeOperation {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeTokens,
    },
    Mint {
        to: Vec<u8>,
        amount: NativeTokens,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: NativeTokens,
        fee: NativeTokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: NativeTokens,
        fee: NativeTokens,
        expires_at: Option<NativeTimestamp>,
        expected_allowance: Option<NativeTokens>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<NativeOperation>,
    created_at_time: NativeTimestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: NativeTransaction,
    timestamp: NativeTimestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeGetBlocksArgs {
    start: u64,
    length: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeBlockRange {
    blocks: Vec<NativeBlock>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum NativeArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}

type NativeArchiveResult = Result<NativeBlockRange, NativeArchiveError>;
candid::define_function!(NativeArchiveCallback : (NativeGetBlocksArgs) -> (NativeArchiveResult) query);

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeArchivedBlocks {
    start: u64,
    length: u64,
    callback: NativeArchiveCallback,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct NativeQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<NativeBlock>,
    first_block_index: u64,
    archived_blocks: Vec<NativeArchivedBlocks>,
}

async fn verify_exact_native_block(
    ledger: Principal,
    block_index: u64,
    transfer: &OutboundPayout,
) -> Result<(), String> {
    if !is_native_icp_ledger(ledger) {
        return Err("native ICP proof requested for a non-native ledger".into());
    }
    let (response,): (NativeQueryBlocksResponse,) = ic_cdk::call(
        ledger,
        "query_blocks",
        (NativeGetBlocksArgs {
            start: block_index,
            length: 1,
        },),
    )
    .await
    .map_err(|(code, message)| format!("native ICP query_blocks failed: {code:?} {message}"))?;
    if response.chain_length <= block_index
        || response.first_block_index != block_index
        || response.blocks.len() != 1
        || !response.archived_blocks.is_empty()
    {
        return Err("native ICP did not directly serve the exact block; archive or ambiguous evidence remains held".into());
    }
    let block = response
        .blocks
        .first()
        .ok_or("native ICP block is missing")?;
    validate_native_transfer(block, transfer)
}

fn validate_native_transfer(block: &NativeBlock, transfer: &OutboundPayout) -> Result<(), String> {
    let Some(NativeOperation::Transfer {
        from,
        to,
        spender,
        amount,
        fee,
    }) = block.transaction.operation.as_ref()
    else {
        return Err("native ICP block is not an ordinary transfer".into());
    };
    let expected_from = native_account_identifier(transfer.from, transfer.from_subaccount);
    let expected_to = native_account_identifier(transfer.to, transfer.to_subaccount);
    let expected_amount = u64::try_from(transfer.net_amount)
        .map_err(|_| "native ICP payout net amount exceeds u64".to_string())?;
    let expected_fee =
        u64::try_from(transfer.fee).map_err(|_| "native ICP payout fee exceeds u64".to_string())?;
    if spender.is_some()
        || from.as_slice() != expected_from
        || to.as_slice() != expected_to
        || amount.e8s != expected_amount
        || fee.e8s != expected_fee
        || block.transaction.icrc1_memo.as_deref() != Some(transfer.memo.as_slice())
        || block.transaction.created_at_time.timestamp_nanos != transfer.created_at_time
        || transfer.net_amount.checked_add(transfer.fee) != Some(transfer.gross_amount)
    {
        return Err("native ICP block does not match the exact persisted payout tuple".into());
    }
    Ok(())
}

fn native_account_identifier(owner: Principal, subaccount: Option<[u8; 32]>) -> [u8; 32] {
    let subaccount = subaccount.unwrap_or([0; 32]);
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update(subaccount);
    let hash = hasher.finalize();
    let checksum = crc32_ieee(&hash).to_be_bytes();
    let mut identifier = [0; 32];
    identifier[..4].copy_from_slice(&checksum);
    identifier[4..].copy_from_slice(&hash);
    identifier
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn direct_block_value(response: &GetBlocksResult, block_index: u64) -> Result<ICRC3Value, String> {
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed payout proof is unsupported; payout remains held".into());
    }
    if response.blocks.len() != 1 || nat_to_u64(&response.blocks[0].id)? != block_index {
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
            .clone()
            .try_into()
            .map_err(|_| "ICRC-3 integer exceeds u128".to_string()),
        _ => Err("ICRC-3 integer is not Nat".into()),
    }
}

fn nat_to_u64(value: &Nat) -> Result<u64, String> {
    value
        .0
        .clone()
        .try_into()
        .map_err(|_| "ICRC-3 integer exceeds u64".to_string())
}

fn normalize_btype(btype: &str) -> Option<String> {
    match btype {
        "1xfer" => Some("xfer".into()),
        "1mint" => Some("mint".into()),
        "1burn" => Some("burn".into()),
        "2xfer" => Some("transfer_from".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::account_matches;
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    #[test]
    fn omitted_account_subaccount_matches_only_explicit_zero() {
        let owner = Principal::from_slice(&[1, 2, 3]);
        let omitted = Account {
            owner,
            subaccount: None,
        };
        assert!(account_matches(
            &Account {
                owner,
                subaccount: Some([0; 32])
            },
            &omitted
        ));
        assert!(!account_matches(
            &Account {
                owner,
                subaccount: Some([7; 32])
            },
            &omitted
        ));
        assert!(account_matches(
            &Account {
                owner,
                subaccount: None
            },
            &Account {
                owner,
                subaccount: Some([0; 32])
            }
        ));
    }
}
