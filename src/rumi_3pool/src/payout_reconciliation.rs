//! Positive-only recovery of an aged ambiguous 3pool payout.
//!
//! A caller supplies one candidate ledger block index. This module accepts it
//! only when the ledger directly serves the exact ICRC-3 block.
//! It deliberately does not interpret missing, partial, or unavailable history
//! as proof that a payout did not happen.

use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;

use crate::payouts::PayoutTransfer;

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

pub(crate) async fn verify_exact_block(
    ledger: Principal,
    block_index: u64,
    transfer: &PayoutTransfer,
) -> Result<(), String> {
    let args = vec![GetBlocksRequest {
        start: Nat::from(block_index),
        length: Nat::from(1u64),
    }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args,))
        .await
        .map_err(|(code, message)| {
        format!("icrc3_get_blocks call to {ledger} failed: {code:?} {message}")
    })?;
    if response
        .log_length
        .0
        .to_u64()
        .is_none_or(|length| length <= block_index)
    {
        return Err("ICRC-3 log length does not include the supplied block".into());
    }

    let value = direct_block_value(&response, block_index)?;

    let decoded = decode_block(&value)?;
    validate_exact_transfer(&decoded, transfer)
}

fn direct_block_value(response: &GetBlocksResult, block_index: u64) -> Result<ICRC3Value, String> {
    if !response.archived_blocks.is_empty() {
        return Err("archive-backed payout proof is unsupported; payout remains held".into());
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
        "2xfer" => Some("transfer_from".into()),
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
            .contains("archive-backed payout proof is unsupported"));
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
