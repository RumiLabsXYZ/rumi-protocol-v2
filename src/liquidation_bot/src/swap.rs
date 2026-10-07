use candid::{CandidType, Nat, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use icrc_ledger_types::icrc2::approve::ApproveArgs;
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
use serde::Deserialize;

use crate::icpswap;
use crate::history::{BotCkUsdcPaymentTransfer, BotClaimReturnTransfer, IcpTreasuryBonusTransfer};

use crate::state::BotConfig;

#[derive(Clone, Copy, Debug)]
pub struct CkUsdcPaymentReceipt {
    pub amount_e6: u64,
    pub block_index: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCkUsdcPayment {
    pub amount_e6: u64,
    pub fee_e6: u64,
    pub created_at_time: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CkUsdcHistoryMatch { NoMatch, ExactMatch }

fn nat_u64(value: &ICRC3Value, field: &str) -> Result<u64, String> {
    match value {
        ICRC3Value::Nat(value) => value.0.to_string().parse::<u64>()
            .map_err(|_| format!("ckUSDC block {field} exceeds u64")),
        _ => Err(format!("ckUSDC block {field} is not a Nat")),
    }
}

fn block_map_value<'a>(map: &'a std::collections::BTreeMap<String, ICRC3Value>, key: &str) -> Result<&'a ICRC3Value, String> {
    map.get(key).ok_or_else(|| format!("ckUSDC block is missing {key}"))
}

fn decode_ckusdc_account(value: &ICRC3Value) -> Result<Account, String> {
    let ICRC3Value::Array(parts) = value else { return Err("ckUSDC block account is not an Array".into()) };
    if !(1..=2).contains(&parts.len()) { return Err("ckUSDC block account has an invalid shape".into()); }
    let ICRC3Value::Blob(owner) = &parts[0] else { return Err("ckUSDC block account owner is not a Blob".into()) };
    let owner = Principal::try_from_slice(owner.as_ref()).map_err(|e| format!("ckUSDC block account principal is invalid: {e}"))?;
    let subaccount = if let Some(ICRC3Value::Blob(bytes)) = parts.get(1) {
        Some(bytes.as_ref().try_into().map_err(|_| "ckUSDC block subaccount is not 32 bytes")?)
    } else if parts.len() == 2 {
        return Err("ckUSDC block account subaccount is not a Blob".into());
    } else { None };
    Ok(Account { owner, subaccount })
}

/// Identify a legacy bundled-ledger ICRC-1 `tx.op = xfer` only when the
/// ICRC-2 `spender` field is absent. Explicit `1xfer` blocks are accepted
/// only when their operation is consistent and they carry no spender.
fn is_ckusdc_icrc1_transfer(
    block: &std::collections::BTreeMap<String, ICRC3Value>,
    tx: &std::collections::BTreeMap<String, ICRC3Value>,
) -> Result<bool, String> {
    let op_is_xfer = match tx.get("op") {
        Some(ICRC3Value::Text(op)) => op.eq_ignore_ascii_case("xfer") || op.eq_ignore_ascii_case("transfer"),
        Some(_) => return Err("ckUSDC transaction op is not Text".into()),
        None => false,
    };
    match block.get("btype") {
        Some(ICRC3Value::Text(kind)) if kind.eq_ignore_ascii_case("1xfer") => {
            if !op_is_xfer || tx.contains_key("spender") {
                return Err("ckUSDC 1xfer block has an inconsistent operation or spender".into());
            }
            Ok(true)
        }
        Some(ICRC3Value::Text(kind)) if kind.eq_ignore_ascii_case("2xfer") => Ok(false),
        Some(ICRC3Value::Text(_)) => Ok(false),
        Some(_) => Err("ckUSDC block btype is not Text".into()),
        None => Ok(op_is_xfer && !tx.contains_key("spender")),
    }
}

fn decode_ckusdc_payment_block(block: &ICRC3Value, from: Principal, to: Principal, memo: &[u8]) -> Result<VerifiedCkUsdcPayment, String> {
    decode_ckusdc_payment_block_from_accounts(
        block,
        Account { owner: from, subaccount: None },
        Account { owner: to, subaccount: None },
        memo,
    )
}

fn decode_ckusdc_payment_block_from_accounts(block: &ICRC3Value, from: Account, to: Account, memo: &[u8]) -> Result<VerifiedCkUsdcPayment, String> {
    let ICRC3Value::Map(block) = block else { return Err("ckUSDC ledger block is not a Map".into()) };
    let ICRC3Value::Map(tx) = block_map_value(block, "tx")? else { return Err("ckUSDC ledger transaction is not a Map".into()) };
    let is_transfer = is_ckusdc_icrc1_transfer(block, tx)?;
    if !is_transfer { return Err("ckUSDC block is not an ICRC-1 transfer".into()); }
    let block_from = decode_ckusdc_account(block_map_value(tx, "from")?)?;
    let block_to = decode_ckusdc_account(block_map_value(tx, "to")?)?;
    if block_from != from || block_to != to {
        return Err("ckUSDC block source or destination account does not match the claim".into());
    }
    match tx.get("memo") {
        Some(ICRC3Value::Blob(actual)) if actual.as_ref() == memo => (),
        _ => return Err("ckUSDC block memo does not match the active claim".into()),
    }
    let amount_e6 = nat_u64(block_map_value(tx, "amt")?, "amount")?;
    let tx_fee = tx.get("fee").map(|value| nat_u64(value, "transaction fee")).transpose()?;
    let block_fee = block.get("fee").map(|value| nat_u64(value, "fee")).transpose()?;
    let fee_e6 = match (tx_fee, block_fee) {
        (Some(a), Some(b)) if a != b => return Err("ckUSDC block fee fields conflict".into()),
        (Some(value), _) | (_, Some(value)) => value,
        (None, None) => return Err("ckUSDC block does not disclose its transfer fee".into()),
    };
    let created_at_time = tx.get("ts").or_else(|| tx.get("created_at_time"))
        .ok_or_else(|| "ckUSDC block does not disclose created_at_time".to_string())
        .and_then(|value| nat_u64(value, "created_at_time"))?;
    Ok(VerifiedCkUsdcPayment { amount_e6, fee_e6, created_at_time })
}

/// Classify one indexed ICRC-3 block against the immutable original transfer
/// tuple. Unrelated well-formed blocks are non-matches; malformed transfer
/// blocks fail closed because they cannot safely establish absence.
pub fn classify_ckusdc_payment_block(
    block: &ICRC3Value,
    from: Account,
    args: &icrc_ledger_types::icrc1::transfer::TransferArg,
) -> Result<CkUsdcHistoryMatch, String> {
    let ICRC3Value::Map(block_map) = block else { return Err("ckUSDC history block is not a Map".into()) };
    let ICRC3Value::Map(tx) = block_map_value(block_map, "tx")? else { return Err("ckUSDC history transaction is not a Map".into()) };
    let is_transfer = is_ckusdc_icrc1_transfer(block_map, tx)?;
    if !is_transfer { return Ok(CkUsdcHistoryMatch::NoMatch); }
    let from_actual = decode_ckusdc_account(block_map_value(tx, "from")?)?;
    let to_actual = decode_ckusdc_account(block_map_value(tx, "to")?)?;
    let amount = nat_u64(block_map_value(tx, "amt")?, "amount")?;
    let expected_amount = args.amount.0.to_string().parse::<u64>().map_err(|_| "persisted amount exceeds u64")?;
    let memo_matches = match (tx.get("memo"), args.memo.as_ref()) {
        (Some(ICRC3Value::Blob(actual)), Some(expected)) => actual.as_ref() == expected.0.as_ref(),
        (None, None) => true,
        (Some(ICRC3Value::Blob(_)), _) | (None, Some(_)) => false,
        _ => false,
    };
    // Reject unrelated well-formed transfers before demanding every field
    // needed to authenticate the candidate. Standard ledgers may omit
    // created_at_time (and a fee may be reported at block level) for ordinary
    // transfers; those omissions say nothing about our exact tuple.
    if from_actual != from || to_actual != args.to || amount != expected_amount || !memo_matches {
        return Ok(CkUsdcHistoryMatch::NoMatch);
    }

    let expected_fee = args.fee.as_ref().ok_or("persisted fee is absent")?.0.to_string().parse::<u64>()
        .map_err(|_| "persisted fee exceeds u64")?;
    let expected_timestamp = args.created_at_time.ok_or("persisted created_at_time is absent")?;
    let verified = decode_ckusdc_payment_block_from_accounts(
        block,
        from,
        args.to,
        args.memo.as_ref().map(|memo| memo.0.as_ref()).unwrap_or_default(),
    )?;
    if verified.amount_e6 == expected_amount
        && verified.fee_e6 == expected_fee
        && verified.created_at_time == expected_timestamp
    {
        Ok(CkUsdcHistoryMatch::ExactMatch)
    } else {
        Ok(CkUsdcHistoryMatch::NoMatch)
    }
}

/// Read the ledger's current fixed log length. The caller persists this value
/// before scanning, so later appended blocks do not change the scan boundary.
pub async fn ckusdc_history_log_length(ledger: Principal) -> Result<u64, String> {
    let request = vec![GetBlocksRequest { start: Nat::from(0u64), length: Nat::from(1u64) }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await
        .map_err(|(code, message)| format!("ckUSDC history log-length query failed: {code:?}: {message}"))?;
    response.log_length.0.to_string().parse::<u64>().map_err(|_| "ckUSDC history log_length exceeds u64".into())
}

/// Return the exact raw block at `index`, following at most one ledger archive
/// descriptor. Missing coverage, overlaps, malformed responses, and archive
/// errors are errors, never evidence of absence.
pub async fn fetch_ckusdc_history_block(ledger: Principal, index: u64) -> Result<ICRC3Value, String> {
    let request = vec![GetBlocksRequest { start: Nat::from(index), length: Nat::from(1u64) }];
    let (response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (request.clone(),)).await
        .map_err(|(code, message)| format!("ckUSDC history get_blocks failed: {code:?}: {message}"))?;
    if !response.blocks.is_empty() { return exact_ckusdc_indexed_block(&response.blocks, index); }
    if response.blocks.is_empty() {
        if response.archived_blocks.len() > 32 || response.archived_blocks.iter().any(|a| a.args.len() > 32) {
            return Err("ckUSDC ledger returned excessive archive metadata".into());
        }
        let mut matching = response.archived_blocks.iter().filter(|archive| archive.args.iter().any(|arg| ckusdc_archive_request_covers(arg, index)));
        let archive = matching.next().ok_or("ckUSDC ledger has no archive descriptor for requested history index")?;
        if matching.next().is_some() { return Err("ckUSDC ledger has overlapping archive descriptors".into()); }
        let (archive_response,): (GetBlocksResult,) = ic_cdk::call(archive.callback.canister_id, &archive.callback.method, (request,)).await
            .map_err(|(code, message)| format!("ckUSDC archive history lookup failed: {code:?}: {message}"))?;
        if !archive_response.archived_blocks.is_empty() {
            return Err("ckUSDC archive did not return exactly the indexed history block".into());
        }
        return exact_ckusdc_indexed_block(&archive_response.blocks, index);
    }
    Err("ckUSDC history response has no direct or archived block".into())
}

fn ckusdc_archive_request_covers(request: &GetBlocksRequest, index: u64) -> bool {
    let start = request.start.0.to_string().parse::<u64>().ok();
    let length = request.length.0.to_string().parse::<u64>().ok();
    matches!((start, length), (Some(start), Some(length)) if start <= index && start.checked_add(length).is_some_and(|end| index < end))
}

fn exact_ckusdc_indexed_block(
    blocks: &[icrc_ledger_types::icrc3::blocks::BlockWithId],
    index: u64,
) -> Result<ICRC3Value, String> {
    if blocks.len() != 1 || blocks[0].id.0.to_string().parse::<u64>().ok() != Some(index) {
        return Err("ckUSDC history returned malformed or incomplete exact-index response".into());
    }
    Ok(blocks[0].block.clone())
}

/// Fetch and validate one exact ICRC-3 transfer from the configured ledger.
/// An archived lookup is accepted only when the ledger's own descriptor
/// covers this index, and the archive must return exactly that block.
pub async fn verify_ckusdc_payment_block(ledger: Principal, index: u64, from: Principal, to: Principal, memo: &[u8]) -> Result<VerifiedCkUsdcPayment, String> {
    verify_ckusdc_transfer_block(
        ledger,
        index,
        Account { owner: from, subaccount: None },
        Account { owner: to, subaccount: None },
        memo,
    ).await
}

pub async fn verify_ckusdc_transfer_block(ledger: Principal, index: u64, from: Account, to: Account, memo: &[u8]) -> Result<VerifiedCkUsdcPayment, String> {
    let request = vec![GetBlocksRequest { start: Nat::from(index), length: Nat::from(1u64) }];
    let (mut response,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (request.clone(),)).await
        .map_err(|(code, message)| format!("ckUSDC get_blocks failed: {code:?}: {message}"))?;
    if response.blocks.len() > 1 || response.blocks.iter().any(|block| block.id.0.to_string().parse::<u64>().ok() != Some(index)) {
        return Err("ckUSDC ledger returned malformed exact-block response".into());
    }
    if response.blocks.is_empty() {
        if response.archived_blocks.len() > 32 || response.archived_blocks.iter().any(|a| a.args.len() > 32) {
            return Err("ckUSDC ledger returned excessive archive metadata".into());
        }
        let mut matching = response.archived_blocks.iter().filter(|archive| archive.args.iter().any(|arg| {
            let start = arg.start.0.to_string().parse::<u64>().ok();
            let length = arg.length.0.to_string().parse::<u64>().ok();
            matches!((start, length), (Some(start), Some(length)) if start <= index && start.checked_add(length).is_some_and(|end| index < end))
        }));
        let archive = matching.next().ok_or("ckUSDC ledger has no archive descriptor for the requested block")?;
        if matching.next().is_some() { return Err("ckUSDC ledger has ambiguous archive descriptors".into()); }
        let archive_id = archive.callback.canister_id;
        let method = archive.callback.method.clone();
        let (archive_response,): (GetBlocksResult,) = ic_cdk::call(archive_id, &method, (request,)).await
            .map_err(|(code, message)| format!("ckUSDC archive lookup failed: {code:?}: {message}"))?;
        if !archive_response.archived_blocks.is_empty() || archive_response.blocks.len() != 1
            || archive_response.blocks[0].id.0.to_string().parse::<u64>().ok() != Some(index) {
            return Err("ckUSDC archive did not return exactly the requested block".into());
        }
        response.blocks = archive_response.blocks;
    }
    decode_ckusdc_payment_block_from_accounts(&response.blocks[0].block, from, to, memo)
}

/// Dispatch a previously journaled top-up tuple while preserving the typed
/// ledger error needed to distinguish TooOld from transport/reply loss.
pub async fn transfer_ckusdc_top_up_exact_typed(
    ledger: Principal,
    args: icrc_ledger_types::icrc1::transfer::TransferArg,
) -> Result<u64, CkUsdcPaymentDispatchError> {
    use icrc_ledger_types::icrc1::transfer::TransferError;
    if args.fee.is_none() || args.memo.is_none() || args.created_at_time.is_none() {
        return Err(CkUsdcPaymentDispatchError::Call("persisted top-up lacks fee, memo, or created_at_time".into()));
    }
    let result: Result<(Result<Nat, TransferError>,), _> = ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;
    let block = match result {
        Ok((Ok(block),)) => block,
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => duplicate_of,
        Ok((Err(error),)) => return Err(CkUsdcPaymentDispatchError::Ledger(error)),
        Err((code, message)) => return Err(CkUsdcPaymentDispatchError::Call(format!("{code:?}: {message}"))),
    };
    block.0.to_string().parse::<u64>().map_err(|_| CkUsdcPaymentDispatchError::InvalidBlockIndex)
}

/// Dispatch the original payment with the ledger's typed error preserved so
/// callers can persist a genuine `TooOld` observation before reconciliation.
/// Transport failures and every other ledger error stay ambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CkUsdcPaymentDispatchError {
    Ledger(icrc_ledger_types::icrc1::transfer::TransferError),
    Call(String),
    InvalidBlockIndex,
}

impl CkUsdcPaymentDispatchError {
    pub fn observation(&self) -> crate::history::BotCkUsdcPaymentDispatchObservation {
        match self {
            Self::Ledger(icrc_ledger_types::icrc1::transfer::TransferError::TooOld) =>
                crate::history::BotCkUsdcPaymentDispatchObservation::TooOld,
            Self::Ledger(icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee }) =>
                expected_fee.0.to_string().parse::<u64>()
                    .map(|expected_fee| crate::history::BotCkUsdcPaymentDispatchObservation::BadFee { expected_fee })
                    .unwrap_or(crate::history::BotCkUsdcPaymentDispatchObservation::Ambiguous),
            Self::Ledger(icrc_ledger_types::icrc1::transfer::TransferError::InsufficientFunds { balance }) =>
                balance.0.to_string().parse::<u64>()
                    .map(|balance| crate::history::BotCkUsdcPaymentDispatchObservation::InsufficientFunds { balance })
                    .unwrap_or(crate::history::BotCkUsdcPaymentDispatchObservation::Ambiguous),
            _ => crate::history::BotCkUsdcPaymentDispatchObservation::Ambiguous,
        }
    }
}

pub async fn transfer_ckusdc_payment_exact_typed(
    transfer: &BotCkUsdcPaymentTransfer,
) -> Result<u64, CkUsdcPaymentDispatchError> {
    use icrc_ledger_types::icrc1::transfer::TransferError;
    if transfer.args.fee.is_none() || transfer.args.memo.is_none() || transfer.args.created_at_time.is_none() {
        return Err(CkUsdcPaymentDispatchError::Call("persisted original payment lacks fee, memo, or created_at_time".into()));
    }
    let result: Result<(Result<Nat, TransferError>,), _> = ic_cdk::call(
        transfer.ledger, "icrc1_transfer", (transfer.args.clone(),),
    ).await;
    let block = match result {
        Ok((Ok(block),)) => block,
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => duplicate_of,
        Ok((Err(error),)) => return Err(CkUsdcPaymentDispatchError::Ledger(error)),
        Err((code, message)) => return Err(CkUsdcPaymentDispatchError::Call(format!("{code:?}: {message}"))),
    };
    block.0.to_string().parse::<u64>().map_err(|_| CkUsdcPaymentDispatchError::InvalidBlockIndex)
}

#[cfg(test)]
mod ckusdc_payment_block_tests {
    use super::{ckusdc_archive_request_covers, decode_ckusdc_payment_block, exact_ckusdc_indexed_block, VerifiedCkUsdcPayment};

    #[test]
    fn only_typed_ledger_too_old_maps_to_too_old_observation() {
        use super::CkUsdcPaymentDispatchError as DispatchError;
        use crate::history::BotCkUsdcPaymentDispatchObservation as Observation;
        use icrc_ledger_types::icrc1::transfer::TransferError;

        assert_eq!(DispatchError::Ledger(TransferError::TooOld).observation(), Observation::TooOld);
        assert_eq!(DispatchError::Ledger(TransferError::BadFee { expected_fee: 25u64.into() }).observation(),
            Observation::BadFee { expected_fee: 25 });
        assert_eq!(DispatchError::Ledger(TransferError::InsufficientFunds { balance: 99u64.into() }).observation(),
            Observation::InsufficientFunds { balance: 99 });
        assert_eq!(DispatchError::Ledger(TransferError::TemporarilyUnavailable).observation(), Observation::Ambiguous);
        assert_eq!(DispatchError::Call("too old".into()).observation(), Observation::Ambiguous);
    }
    use candid::{Nat, Principal};
    use icrc_ledger_types::icrc::generic_value::{ICRC3Map, ICRC3Value};

    fn blob(value: Vec<u8>) -> ICRC3Value { ICRC3Value::Blob(value.into()) }

    fn account(owner: Principal) -> ICRC3Value {
        ICRC3Value::Array(vec![blob(owner.as_slice().to_vec())])
    }

    fn account_with_subaccount(owner: Principal, subaccount: [u8; 32]) -> ICRC3Value {
        ICRC3Value::Array(vec![
            blob(owner.as_slice().to_vec()),
            blob(subaccount.to_vec()),
        ])
    }

    fn block(from: Principal, to: Principal, memo: &[u8]) -> ICRC3Value {
        let mut tx = ICRC3Map::new();
        tx.insert("op".into(), ICRC3Value::Text("xfer".into()));
        tx.insert("from".into(), account(from));
        tx.insert("to".into(), account(to));
        tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(999u64)));
        tx.insert("fee".into(), ICRC3Value::Nat(Nat::from(10_000u64)));
        tx.insert("memo".into(), blob(memo.to_vec()));
        tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(123_456u64)));
        let mut result = ICRC3Map::new();
        result.insert("btype".into(), ICRC3Value::Text("1xfer".into()));
        result.insert("tx".into(), ICRC3Value::Map(tx));
        result.insert("fee".into(), ICRC3Value::Nat(Nat::from(10_000u64)));
        ICRC3Value::Map(result)
    }

    #[test]
    fn exact_short_block_proof_checks_accounts_memo_fee_and_created_at() {
        let from = Principal::from_slice(&[0x41]);
        let to = Principal::from_slice(&[0x42]);
        let memo = b"RUMI-BOT-LIQ:0001";
        assert_eq!(decode_ckusdc_payment_block(&block(from, to, memo), from, to, memo), Ok(VerifiedCkUsdcPayment {
            amount_e6: 999, fee_e6: 10_000, created_at_time: 123_456,
        }));
        assert!(decode_ckusdc_payment_block(&block(from, to, b"other"), from, to, memo).is_err());
        assert!(decode_ckusdc_payment_block(&block(from, Principal::from_slice(&[0x43]), memo), from, to, memo).is_err());
    }

    #[test]
    fn claim_payment_block_requires_exact_generation_subaccount() {
        use super::{classify_ckusdc_payment_block, decode_ckusdc_payment_block_from_accounts, CkUsdcHistoryMatch};
        use icrc_ledger_types::icrc1::{account::Account, transfer::{Memo, TransferArg}};
        let from = Principal::from_slice(&[0x41]);
        let to = Principal::from_slice(&[0x42]);
        let generation = [0x55; 32];
        let other_generation = [0x56; 32];
        let memo = b"claim-generation-payment";
        let mut payment_block = block(from, to, memo);
        if let ICRC3Value::Map(root) = &mut payment_block {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") {
                tx.insert("from".into(), account_with_subaccount(from, generation));
            }
        }
        let source = Account { owner: from, subaccount: Some(generation) };
        let destination = Account { owner: to, subaccount: None };
        assert!(decode_ckusdc_payment_block_from_accounts(
            &payment_block, source, destination, memo,
        ).is_ok());
        let args = TransferArg {
            from_subaccount: Some(generation),
            to: destination,
            amount: Nat::from(999u64),
            fee: Some(Nat::from(10_000u64)),
            memo: Some(Memo::from(memo.to_vec())),
            created_at_time: Some(123_456),
        };
        assert_eq!(classify_ckusdc_payment_block(&payment_block, source, &args), Ok(CkUsdcHistoryMatch::ExactMatch));
        assert_eq!(classify_ckusdc_payment_block(
            &payment_block,
            Account { owner: from, subaccount: Some(other_generation) },
            &args,
        ), Ok(CkUsdcHistoryMatch::NoMatch));
    }

    #[test]
    fn history_classifier_ignores_unrelated_transfer_metadata_but_requires_exact_candidate() {
        use super::{classify_ckusdc_payment_block, CkUsdcHistoryMatch};
        use icrc_ledger_types::icrc1::{account::Account, transfer::{Memo, TransferArg}};
        let from = Principal::from_slice(&[0x41]);
        let to = Principal::from_slice(&[0x42]);
        let args = TransferArg {
            from_subaccount: None,
            to: Account { owner: to, subaccount: None },
            amount: Nat::from(999u64),
            fee: Some(Nat::from(10_000u64)),
            memo: Some(Memo::from(b"RUMI-BOT-LIQ:0001".to_vec())),
            created_at_time: Some(123_456),
        };
        let block = block(from, to, b"RUMI-BOT-LIQ:0001");
        assert_eq!(classify_ckusdc_payment_block(&block, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::ExactMatch));
        let mut transfer_from = block.clone();
        if let ICRC3Value::Map(root) = &mut transfer_from {
            root.insert("btype".into(), ICRC3Value::Text("2xfer".into()));
        }
        assert_eq!(classify_ckusdc_payment_block(&transfer_from, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::NoMatch));
        assert!(decode_ckusdc_payment_block(&transfer_from, from, to, b"RUMI-BOT-LIQ:0001").is_err());
        let mut legacy_icrc1 = block.clone();
        if let ICRC3Value::Map(root) = &mut legacy_icrc1 { root.remove("btype"); }
        assert_eq!(classify_ckusdc_payment_block(&legacy_icrc1, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::ExactMatch));
        assert_eq!(decode_ckusdc_payment_block(&legacy_icrc1, from, to, b"RUMI-BOT-LIQ:0001").unwrap().amount_e6, 999);
        let mut legacy_icrc2 = legacy_icrc1;
        if let ICRC3Value::Map(root) = &mut legacy_icrc2 {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") {
                tx.insert("spender".into(), account(Principal::from_slice(&[0x44])));
            }
        }
        assert_eq!(classify_ckusdc_payment_block(&legacy_icrc2, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::NoMatch));
        assert!(decode_ckusdc_payment_block(&legacy_icrc2, from, to, b"RUMI-BOT-LIQ:0001").is_err());
        assert_eq!(classify_ckusdc_payment_block(&block, Account { owner: Principal::anonymous(), subaccount: None }, &args), Ok(CkUsdcHistoryMatch::NoMatch));
        let mut wrong_amount = block.clone();
        if let ICRC3Value::Map(root) = &mut wrong_amount {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") { tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(1u64))); }
        }
        assert_eq!(classify_ckusdc_payment_block(&wrong_amount, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::NoMatch));
        let mut unrelated_without_optional_metadata = block.clone();
        if let ICRC3Value::Map(root) = &mut unrelated_without_optional_metadata {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") {
                tx.insert("from".into(), account(Principal::from_slice(&[0x43])));
                tx.remove("fee");
                tx.remove("ts");
            }
        }
        assert_eq!(classify_ckusdc_payment_block(&unrelated_without_optional_metadata, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::NoMatch));
        let mut top_level_fee_only = block.clone();
        if let ICRC3Value::Map(root) = &mut top_level_fee_only {
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") { tx.remove("fee"); }
        }
        assert_eq!(classify_ckusdc_payment_block(&top_level_fee_only, Account { owner: from, subaccount: None }, &args), Ok(CkUsdcHistoryMatch::ExactMatch));
        let mut missing_fee = block;
        if let ICRC3Value::Map(root) = &mut missing_fee {
            root.remove("fee");
            if let Some(ICRC3Value::Map(tx)) = root.get_mut("tx") { tx.remove("fee"); }
        }
        assert!(classify_ckusdc_payment_block(&missing_fee, Account { owner: from, subaccount: None }, &args).is_err());
    }

    #[test]
    fn direct_and_archived_index_proofs_require_exact_coverage() {
        use icrc_ledger_types::icrc3::blocks::{BlockWithId, GetBlocksRequest};
        let direct = ICRC3Value::Text("exact".into());
        assert_eq!(exact_ckusdc_indexed_block(&[BlockWithId { id: Nat::from(45u64), block: direct.clone() }], 45), Ok(direct.clone()));
        assert!(exact_ckusdc_indexed_block(&[], 45).is_err());
        assert!(exact_ckusdc_indexed_block(&[BlockWithId { id: Nat::from(46u64), block: direct }], 45).is_err());
        let archive_range = GetBlocksRequest { start: Nat::from(40u64), length: Nat::from(10u64) };
        assert!(ckusdc_archive_request_covers(&archive_range, 45));
        assert!(!ckusdc_archive_request_covers(&archive_range, 50));
        let overflow = GetBlocksRequest { start: Nat::from(u64::MAX - 1), length: Nat::from(10u64) };
        assert!(!ckusdc_archive_request_covers(&overflow, u64::MAX - 1));
    }
}

/// Belt-and-suspenders default when `BotConfig.*_fee` is unset. The real fee
/// for ICP and ckUSDC is 10_000 in their respective base units (e8s for ICP
/// and e6 for ckUSDC, since ckUSDC has 6 decimals and a fee of $0.01).
/// Previously this default was 10 for ckUSDC, which is what blew up every
/// ICPSwap swap with "Wrong fee cache (expected: 10000, received: 10)".
pub(crate) const FALLBACK_LEDGER_FEE: u64 = 10_000;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct SwapResult {
    pub ckusdc_received_e6: u64,
    pub effective_price_e8s: u64,
}

// Native ICP ledger's stable `query_blocks` Candid schema. In particular,
// native blocks store account identifiers, not ICRC-1 Account arrays, and
// store the ICRC-1 memo separately from the legacy numeric memo.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpTokens { e8s: u64 }

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpTimestamp { timestamp_nanos: u64 }

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpGetBlocksArgs { start: u64, length: u64 }

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpCandidTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<IcpCandidOperation>,
    created_at_time: IcpTimestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub(crate) struct IcpCandidBlock {
    parent_hash: Option<Vec<u8>>,
    transaction: IcpCandidTransaction,
    timestamp: IcpTimestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum IcpCandidOperation {
    Burn { from: Vec<u8>, spender: Option<Vec<u8>>, amount: IcpTokens },
    Mint { to: Vec<u8>, amount: IcpTokens },
    Transfer { from: Vec<u8>, to: Vec<u8>, spender: Option<Vec<u8>>, amount: IcpTokens, fee: IcpTokens },
    Approve {
        from: Vec<u8>, spender: Vec<u8>, allowance_e8s: i128, allowance: IcpTokens,
        expected_allowance: Option<IcpTokens>, fee: IcpTokens, expires_at: Option<IcpTimestamp>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpBlockRange { blocks: Vec<IcpCandidBlock> }

#[derive(CandidType, Deserialize, Clone, Debug)]
enum IcpQueryArchiveError {
    BadFirstBlockIndex { requested_index: u64, first_valid_index: u64 },
    Other { error_code: u64, error_message: String },
}

type IcpQueryArchiveResult = Result<IcpBlockRange, IcpQueryArchiveError>;

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpArchivedBlocksRange {
    start: u64,
    length: u64,
    callback: QueryArchiveFn<IcpGetBlocksArgs, IcpQueryArchiveResult>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct IcpQueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<IcpCandidBlock>,
    first_block_index: u64,
    archived_blocks: Vec<IcpArchivedBlocksRange>,
}

/// Fetch a token ledger's current ICRC-1 transfer fee.
pub async fn fetch_ledger_fee(ledger: Principal) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(ledger, "icrc1_fee", ()).await;
    match result {
        Ok((n,)) => n
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("icrc1_fee returned non-u64 value from {}", ledger)),
        Err((code, msg)) => Err(format!("icrc1_fee call failed ({:?}): {}", code, msg)),
    }
}

/// One-time infinite ICRC-2 approve. Amount = u128::MAX, no expiry.
pub async fn approve_infinite(
    token_ledger: Principal,
    spender: Principal,
) -> Result<(), String> {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: spender,
            subaccount: None,
        },
        amount: Nat::from(u128::MAX),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc2::approve::ApproveError>,),
        _,
    > = ic_cdk::call(token_ledger, "icrc2_approve", (args,)).await;

    match result {
        Ok((Ok(_),)) => Ok(()),
        Ok((Err(e),)) => Err(format!("Approve failed: {:?}", e)),
        Err((code, msg)) => Err(format!("Approve call failed: {:?} {}", code, msg)),
    }
}

/// Reject swaps when `icpswap_pool` is the anonymous-principal sentinel that
/// `BotConfig` falls back to after a legacy-state migration where the field
/// did not exist. Forces admin to call `set_config` with a real pool before
/// any liquidation routes through ICPSwap.
fn require_icpswap_pool_set(config: &BotConfig) -> Result<(), String> {
    if config.icpswap_pool == Principal::anonymous() {
        return Err(
            "icpswap_pool not configured (defaulted from legacy migration). \
             Admin must call set_config with a real ICPSwap pool principal."
                .to_string(),
        );
    }
    Ok(())
}

/// Quote how much ckUSDC we'd get for `icp_amount_e8s` ICP.
pub async fn quote_icp_for_ckusdc(config: &BotConfig, icp_amount_e8s: u64) -> Result<u64, String> {
    require_icpswap_pool_set(config)?;
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one).await
}

/// Swap ICP for ckUSDC on ICPSwap.
/// Flow: get quote -> apply slippage -> depositFromAndSwap.
/// Requires infinite approve to already be in place.
pub async fn swap_icp_for_ckusdc(
    config: &BotConfig,
    icp_amount_e8s: u64,
) -> Result<SwapResult, String> {
    swap_icp_for_ckusdc_with_floor(config, icp_amount_e8s, 0).await
}

pub async fn swap_icp_for_ckusdc_with_floor(
    config: &BotConfig,
    icp_amount_e8s: u64,
    required_output_e6: u64,
) -> Result<SwapResult, String> {
    let (quoted_output, min_output) =
        quote_icp_for_ckusdc_with_floor(config, icp_amount_e8s, required_output_e6).await?;
    execute_quoted_icp_for_ckusdc(config, icp_amount_e8s, min_output, quoted_output).await
}

/// Quote and validate a swap without initiating it. Callers that persist a
/// durable swap-started marker can do so after this read-only await and just
/// before the pool call that may move assets.
pub async fn quote_icp_for_ckusdc_with_floor(
    config: &BotConfig,
    icp_amount_e8s: u64,
    required_output_e6: u64,
) -> Result<(u64, u64), String> {
    require_icpswap_pool_set(config)?;
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    let quoted_output =
        icpswap::quote(config.icpswap_pool, icp_amount_e8s, zero_for_one).await?;

    if quoted_output == 0 {
        return Err("Quote returned zero output".to_string());
    }

    let min_output = apply_slippage(quoted_output, config.max_slippage_bps).max(required_output_e6);
    if quoted_output < required_output_e6 {
        return Err(format!("ICPSwap quote {} is below claim payment floor {}", quoted_output, required_output_e6));
    }

    Ok((quoted_output, min_output))
}

/// Dispatch a previously quoted swap. Errors from this call have an unknown
/// settlement outcome and must not authorize another swap or collateral
/// return without independent output proof.
pub async fn execute_quoted_icp_for_ckusdc(
    config: &BotConfig,
    icp_amount_e8s: u64,
    min_output: u64,
    quoted_output: u64,
) -> Result<SwapResult, String> {
    require_icpswap_pool_set(config)?;
    let zero_for_one = config
        .icpswap_zero_for_one
        .ok_or("Pool ordering not configured. Call admin_resolve_pool_ordering first.")?;

    log!(
        crate::INFO,
        "ICPSwap quote: {} ICP e8s -> {} ckUSDC e6 (min: {})",
        icp_amount_e8s,
        quoted_output,
        min_output
    );

    let icp_fee = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
    let ckusdc_fee = config.ckusdc_fee_e6.unwrap_or(FALLBACK_LEDGER_FEE);

    let received = icpswap::deposit_and_swap(
        config.icpswap_pool,
        icp_amount_e8s,
        min_output,
        zero_for_one,
        icp_fee,
        ckusdc_fee,
    )
    .await?;

    // Effective price in e8 format: (ckusdc_e6 / icp_e8s) * 1e8
    // = ckusdc_e6 * 1e2 * 1e8 / icp_e8s = ckusdc_e6 * 10_000_000_000 / icp_e8s
    let effective_price_e8s = if icp_amount_e8s > 0 {
        (received as u128 * 10_000_000_000 / icp_amount_e8s as u128) as u64
    } else {
        0
    };

    log!(
        crate::INFO,
        "ICPSwap swap complete: {} ckUSDC e6 received, effective price {} e8s",
        received,
        effective_price_e8s
    );

    Ok(SwapResult {
        ckusdc_received_e6: received,
        effective_price_e8s,
    })
}

fn apply_slippage(amount: u64, max_slippage_bps: u16) -> u64 {
    let reduction = amount as u128 * max_slippage_bps as u128 / 10_000;
    (amount as u128 - reduction) as u64
}

/// Build and persist this tuple before calling `return_collateral_to_backend`.
/// The memo, timestamp, destination subaccount, amount, and ledger are fixed
/// for the lifetime of this claim generation.
pub fn prepare_claim_return_transfer(
    bot_id: Principal,
    config: &BotConfig,
    collateral_amount_e8s: u64,
    vault_id: u64,
    claim_timestamp: u64,
    created_at_time: u64,
) -> Result<BotClaimReturnTransfer, String> {
    let fee = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
    // The backend must consolidate this return account in a second ledger
    // transfer. Return C+F so that after the bot pays F, the escrow contains
    // exactly C+F and the backend can pay F while crediting C to its vault.
    let send_amount = collateral_amount_e8s.checked_add(fee)
        .ok_or_else(|| "Collateral return amount plus ledger fee overflows".to_string())?;

    let subaccount = bot_claim_return_subaccount(vault_id, claim_timestamp);
    let memo = bot_claim_return_memo(vault_id, claim_timestamp);
    let args = icrc_ledger_types::icrc1::transfer::TransferArg {
        from_subaccount: None,
        to: Account {
            owner: config.backend_principal,
            subaccount: Some(subaccount),
        },
        amount: Nat::from(send_amount),
        fee: Some(Nat::from(fee)),
        memo: Some(icrc_ledger_types::icrc1::transfer::Memo::from(memo)),
        created_at_time: Some(created_at_time),
    };
    Ok(BotClaimReturnTransfer {
        ledger: config.icp_ledger,
        from: Account { owner: bot_id, subaccount: None },
        collateral_amount_e8s,
        ledger_fee_e8s: fee,
        claim_timestamp,
        args,
        block_index: None,
        dispatch_attempt_count: Some(0),
        prior_no_effects: Some(Vec::new()),
        dispatch_observation: None,
    })
}

pub fn claim_return_transfer_matches(
    transfer: &BotClaimReturnTransfer,
    bot_id: Principal,
    config: &BotConfig,
    vault_id: u64,
    claim_timestamp: u64,
) -> bool {
    let expected_memo = bot_claim_return_memo(vault_id, claim_timestamp);
    let amount_matches = transfer.args.amount.0.to_string().parse::<u64>().ok()
        == transfer.collateral_amount_e8s.checked_add(transfer.ledger_fee_e8s);
    transfer.ledger == config.icp_ledger
        && transfer.from == (Account { owner: bot_id, subaccount: None })
        && transfer.claim_timestamp == claim_timestamp
        && transfer.collateral_amount_e8s > 0
        && transfer.args.from_subaccount.is_none()
        && transfer.args.to == (Account {
            owner: config.backend_principal,
            subaccount: Some(bot_claim_return_subaccount(vault_id, claim_timestamp)),
        })
        && transfer.args.fee.as_ref().map(|fee| fee.0.to_string()) == Some(transfer.ledger_fee_e8s.to_string())
        && transfer.args.created_at_time.is_some()
        && transfer.args.memo.as_ref().map(|memo| memo.0.as_ref()) == Some(expected_memo.as_slice())
        && amount_matches
}

/// Transfer the exact persisted collateral return tuple to the backend.
/// Both a returned block index and an exact ICRC Duplicate response are
/// positive evidence that this request identity landed.
pub async fn return_collateral_to_backend(
    config: &BotConfig,
    transfer: &BotClaimReturnTransfer,
) -> Result<u64, String> {
    return_collateral_to_backend_typed(config, transfer)
        .await
        .map_err(|error| format!("Collateral return dispatch failed: {error:?}"))
}

pub async fn return_collateral_to_backend_typed(
    config: &BotConfig,
    transfer: &BotClaimReturnTransfer,
) -> Result<u64, CkUsdcPaymentDispatchError> {
    if transfer.ledger != config.icp_ledger {
        return Err(CkUsdcPaymentDispatchError::Call(
            "persisted collateral return ledger does not match configuration".into(),
        ));
    }
    if transfer.args.fee.is_none() || transfer.args.memo.is_none() || transfer.args.created_at_time.is_none() {
        return Err(CkUsdcPaymentDispatchError::Call(
            "persisted collateral return lacks fee, memo, or created_at_time".into(),
        ));
    }

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(transfer.ledger, "icrc1_transfer", (transfer.args.clone(),)).await;

    use icrc_ledger_types::icrc1::transfer::TransferError;
    match result {
        Ok((Ok(block_index),)) => block_index.0.to_string().parse::<u64>()
            .map_err(|_| CkUsdcPaymentDispatchError::InvalidBlockIndex),
        // Audit Wave-3: a Duplicate response means the previous attempt landed.
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            log!(
                crate::INFO,
                "[return_collateral_to_backend] ledger reported Duplicate (block {}); treating as success",
                duplicate_of
            );
            duplicate_of.0.to_string().parse::<u64>()
                .map_err(|_| CkUsdcPaymentDispatchError::InvalidBlockIndex)
        }
        Ok((Err(error),)) => Err(CkUsdcPaymentDispatchError::Ledger(error)),
        Err((code, msg)) => Err(CkUsdcPaymentDispatchError::Call(format!("{code:?}: {msg}"))),
    }
}

pub fn bot_claim_return_memo(vault_id: u64, claim_timestamp: u64) -> Vec<u8> {
    let mut memo = b"BOTRET02".to_vec();
    memo.extend_from_slice(&vault_id.to_be_bytes());
    memo.extend_from_slice(&claim_timestamp.to_be_bytes());
    memo
}

/// Isolate a returned-collateral receipt from every other vault and claim.
/// The backend derives the identical subaccount from its active claim.
pub fn bot_claim_return_subaccount(vault_id: u64, claim_timestamp: u64) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    subaccount[..8].copy_from_slice(&vault_id.to_be_bytes());
    subaccount[8..16].copy_from_slice(&claim_timestamp.to_be_bytes());
    subaccount[16..24].copy_from_slice(b"BOTRET01");
    subaccount
}

/// Read the bot's own ckUSDC main-account balance from the ledger.
///
/// Used by `process_pending` to bracket the swap call and compute the
/// per-claim delta — the ckUSDC this specific liquidation actually deposited
/// in the bot wallet, regardless of any pre-existing balance left over from
/// prior runs or the swap router's claimed output.
pub async fn balance_of_self_ckusdc(config: &BotConfig) -> Result<u64, String> {
    balance_of_ckusdc_account(
        config.ckusdc_ledger,
        Account { owner: ic_cdk::id(), subaccount: None },
    ).await
}

pub async fn balance_of_ckusdc_account(ledger: Principal, account: Account) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (account,),
    )
    .await;
    match result {
        Ok((n,)) => n
            .0
            .to_string()
            .parse::<u64>()
            .map_err(|_| format!("icrc1_balance_of returned non-u64 from {}", ledger)),
        Err((code, msg)) => Err(format!("icrc1_balance_of call failed ({:?}): {}", code, msg)),
    }
}

/// Prepare the exact original ckUSDC payment identity. Callers must persist
/// this value before dispatching it to the ledger.
pub fn prepare_ckusdc_payment_transfer(
    config: &BotConfig,
    amount_e6: u64,
    payment_memo: &[u8],
    fee_e6: u64,
    created_at_time: u64,
) -> Result<BotCkUsdcPaymentTransfer, String> {
    prepare_ckusdc_payment_transfer_from_subaccount(
        config, amount_e6, payment_memo, fee_e6, created_at_time, None,
    )
}

pub fn prepare_ckusdc_payment_transfer_from_subaccount(
    config: &BotConfig,
    amount_e6: u64,
    payment_memo: &[u8],
    fee_e6: u64,
    created_at_time: u64,
    from_subaccount: Option<[u8; 32]>,
) -> Result<BotCkUsdcPaymentTransfer, String> {
    let send_amount = amount_e6.checked_sub(fee_e6)
        .ok_or_else(|| "ckUSDC amount is below the live ledger fee".to_string())?;
    if send_amount == 0 {
        return Err("ckUSDC amount too small to cover transfer fee".to_string());
    }

    Ok(BotCkUsdcPaymentTransfer {
        ledger: config.ckusdc_ledger,
        args: icrc_ledger_types::icrc1::transfer::TransferArg {
            from_subaccount,
            to: Account {
                owner: config.backend_principal,
                subaccount: None,
            },
            amount: Nat::from(send_amount),
            fee: Some(Nat::from(fee_e6)),
            memo: Some(payment_memo.to_vec().into()),
            created_at_time: Some(created_at_time),
        },
        block_index: None,
        dispatch_attempt_count: Some(0),
        prior_no_effects: Some(Vec::new()),
        dispatch_observation: None,
        history_scan: None,
    })
}

/// Submit one previously persisted original payment tuple. Duplicate is
/// accepted as the same transfer's receipt; every other error keeps the
/// journal unresolved and must not be replaced with a fresh timestamp.
pub async fn transfer_ckusdc_payment_exact(
    transfer: &BotCkUsdcPaymentTransfer,
) -> Result<u64, String> {
    transfer_ckusdc_payment_exact_typed(transfer).await.map_err(|error| format!("original ckUSDC payment outcome unresolved: {error:?}"))
}

/// Build and persist this exact identity before its first ledger call. The
/// amount is the recipient credit; adding the explicit fee yields the gross
/// obligation debited from the bot's default account.
pub fn prepare_icp_treasury_bonus_transfer(
    config: &BotConfig,
    amount_e8s: u64,
    record_id: u64,
) -> Result<IcpTreasuryBonusTransfer, String> {
    let fee = config.icp_fee_e8s.unwrap_or(FALLBACK_LEDGER_FEE);
    let send_amount = icp_treasury_send_amount(amount_e8s, fee)?;
    // ICP ledger caps ICRC-1 memos at 32 bytes. This 18-byte domain prefix
    // plus the 8-byte record id leaves room below that limit.
    let mut memo = b"RUMI:ICP_BONUS:V1:".to_vec();
    memo.extend_from_slice(&record_id.to_be_bytes());
    Ok(IcpTreasuryBonusTransfer {
        ledger: config.icp_ledger,
        from: Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        args: icrc_ledger_types::icrc1::transfer::TransferArg {
            from_subaccount: None,
            to: Account {
                owner: config.treasury_principal,
                subaccount: None,
            },
            amount: Nat::from(send_amount),
            fee: Some(Nat::from(fee)),
            memo: Some(memo.into()),
            created_at_time: Some(ic_cdk::api::time()),
        },
        block_index: None,
    })
}

/// Fetch one exact native ICP ledger block, making at most one archive
/// callback and only when the ledger's own descriptor covers the index. No
/// history scan or archive discovery call is performed.
pub(crate) async fn fetch_icp_treasury_bonus_block(
    ledger: Principal,
    block_index: u64,
) -> Result<IcpCandidBlock, String> {
    let request = IcpGetBlocksArgs { start: block_index, length: 1 };
    let response: (IcpQueryBlocksResponse,) = ic_cdk::call::<_, (IcpQueryBlocksResponse,)>(
        ledger,
        "query_blocks",
        (request.clone(),),
    )
        .await
        .map_err(|(code, message)| format!("native ICP ledger query_blocks failed: {code:?} {message}"))?;

    if let Some(offset) = block_offset(response.0.first_block_index, response.0.blocks.len(), block_index) {
        return Ok(response.0.blocks[offset].clone());
    }

    let descriptor = response.0.archived_blocks.into_iter().find(|archive| {
        range_covers_index(archive.start, archive.length, block_index)
    }).ok_or_else(|| format!("configured ICP ledger returned no block/archive descriptor for index {block_index}"))?;
    let callback = descriptor.callback;
    let archived: (IcpQueryArchiveResult,) = ic_cdk::call::<_, (IcpQueryArchiveResult,)>(
        callback.canister_id,
        &callback.method,
        (request,),
    )
    .await
    .map_err(|(code, message)| format!("native ICP archive callback failed: {code:?} {message}"))?;
    match archived.0 {
        Ok(range) if range.blocks.len() == 1 => Ok(range.blocks.into_iter().next().expect("one block")),
        Ok(_) => Err(format!("ICP archive {} did not return exactly one block for index {block_index}", callback.canister_id)),
        Err(error) => Err(format!("ICP archive {} rejected exact block lookup: {error:?}", callback.canister_id)),
    }
}

fn nat_to_u64(value: &Nat) -> Option<u64> {
    value.0.to_string().parse::<u64>().ok()
}

/// Native ICP ledger's legacy block schema stores account identifiers rather
/// than principal/subaccount pairs. Ask that same configured ledger for the
/// canonical identifier. Reconciliation fails closed if this native ledger
/// method is unavailable or returns an identifier with the wrong length.
pub async fn fetch_icp_account_identifier(ledger: Principal, account: Account) -> Result<Vec<u8>, String> {
    let result: Result<(Vec<u8>,), _> = ic_cdk::call(ledger, "account_identifier", (account,)).await;
    let identifier = result
        .map_err(|(code, message)| format!("native ICP account_identifier failed: {code:?} {message}"))?
        .0;
    if identifier.len() != 32 {
        return Err(format!("native ICP ledger returned account identifier with {} bytes, expected 32", identifier.len()));
    }
    Ok(identifier)
}

/// Verify the exact persisted transfer tuple against a native ICP CandidBlock.
/// The transfer operation stores legacy AccountIdentifier blobs and the
/// supplied ICRC-1 memo in `icrc1_memo`; the old numeric `memo` is unrelated.
pub(crate) fn validate_icp_treasury_bonus_block(
    block: &IcpCandidBlock,
    transfer: &IcpTreasuryBonusTransfer,
    expected_ledger: Principal,
    expected_sender: Principal,
    expected_treasury: Principal,
    sender_account_identifier: &[u8],
    treasury_account_identifier: &[u8],
) -> Result<(), String> {
    if transfer.ledger != expected_ledger || transfer.from != (Account { owner: expected_sender, subaccount: None }) {
        return Err("persisted ICP bonus journal has the wrong ledger or sender".into());
    }
    let expected_to = Account { owner: expected_treasury, subaccount: None };
    if transfer.args.from_subaccount.is_some() || transfer.args.to != expected_to {
        return Err("persisted ICP bonus journal has the wrong source or recipient account".into());
    }
    let amount = nat_to_u64(&transfer.args.amount).ok_or("persisted ICP bonus amount is out of range")?;
    let fee = transfer.args.fee.as_ref().and_then(nat_to_u64).ok_or("persisted ICP bonus fee is missing or out of range")?;
    let memo = transfer.args.memo.as_ref().ok_or("persisted ICP bonus memo is missing")?;
    let created_at_time = transfer.args.created_at_time.ok_or("persisted ICP bonus timestamp is missing")?;
    if memo.0.len() > 32 || memo.0.len() < 8 || !memo.0.starts_with(b"RUMI:ICP_BONUS:V1:") {
        return Err("persisted ICP bonus memo is invalid or exceeds the ICP ledger limit".into());
    }
    if amount.checked_add(fee).is_none() {
        return Err("persisted ICP bonus gross amount overflows".into());
    }

    if sender_account_identifier.len() != 32 || treasury_account_identifier.len() != 32 {
        return Err("native ICP account identifiers are not 32 bytes".into());
    }
    let operation = block.transaction.operation.as_ref().ok_or("native ICP block operation is missing")?;
    let IcpCandidOperation::Transfer { from, to, amount: block_amount, fee: block_fee, spender, .. } = operation else {
        return Err("native ICP block operation is not Transfer".into());
    };
    if spender.is_some() {
        return Err("native ICP block transfer has a delegated spender".into());
    }
    if block_amount.e8s != amount
        || block_fee.e8s != fee
        || block.transaction.icrc1_memo.as_deref() != Some(memo.0.as_ref())
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("ICP ledger transfer block amount, fee, memo, or created_at_time does not match the journal".into());
    }
    if from.as_slice() != sender_account_identifier || to.as_slice() != treasury_account_identifier {
        return Err("ICP ledger transfer block source or recipient account does not match the journal".into());
    }
    Ok(())
}

fn block_offset(first_index: u64, block_count: usize, requested: u64) -> Option<usize> {
    let offset = requested.checked_sub(first_index)?;
    let offset: usize = offset.try_into().ok()?;
    (offset < block_count).then_some(offset)
}

fn range_covers_index(start: u64, length: u64, index: u64) -> bool {
    length > 0 && start <= index && start.checked_add(length).map_or(true, |end| index < end)
}

/// Replay an already-persisted request without changing any field that
/// participates in ICRC-1 duplicate detection.
pub async fn transfer_icp_treasury_bonus_exact(
    transfer: &IcpTreasuryBonusTransfer,
) -> Result<u64, String> {
    if transfer.from.owner != ic_cdk::id()
        || transfer.from.subaccount != transfer.args.from_subaccount
        || transfer.args.created_at_time.is_none()
        || transfer.args.memo.is_none()
        || transfer.args.fee.is_none()
    {
        return Err("persisted ICP treasury transfer identity is incomplete or has a wrong sender".into());
    }

    let result: Result<
        (Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,),
        _,
    > = ic_cdk::call(transfer.ledger, "icrc1_transfer", (transfer.args.clone(),)).await;

    let result = match result {
        Ok((inner,)) => inner,
        Err((code, msg)) => return Err(format!("ICP transfer call failed: {:?} {}", code, msg)),
    };
    icp_transfer_result_block(result)
}

fn icp_transfer_result_block(
    result: Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError>,
) -> Result<u64, String> {
    use icrc_ledger_types::icrc1::transfer::TransferError;
    let (block, duplicate) = match result {
        Ok(block) => (block, false),
        Err(TransferError::Duplicate { duplicate_of }) => (duplicate_of, true),
        Err(error) => return Err(format!("ICP transfer to treasury failed: {:?}", error)),
    };
    block.0.to_string().parse::<u64>().map_err(|_| {
        if duplicate {
            "ICP ledger returned an out-of-range duplicate block index".to_string()
        } else {
            "ICP ledger returned an out-of-range block index".to_string()
        }
    })
}

fn icp_treasury_send_amount(amount_e8s: u64, fee_e8s: u64) -> Result<u64, String> {
    let send_amount = amount_e8s.saturating_sub(fee_e8s);
    if send_amount == 0 {
        return Err(format!(
            "ICP treasury bonus {} e8s does not exceed transfer fee {} e8s",
            amount_e8s, fee_e8s
        ));
    }
    Ok(send_amount)
}

#[cfg(test)]
mod tests {
    use super::{
        block_offset, bot_claim_return_memo, claim_return_transfer_matches,
        icp_treasury_send_amount, icp_transfer_result_block,
        prepare_claim_return_transfer, prepare_ckusdc_payment_transfer,
        range_covers_index,
        validate_icp_treasury_bonus_block,
        IcpCandidBlock, IcpCandidOperation, IcpCandidTransaction, IcpTimestamp, IcpTokens,
    };
    use candid::{Nat, Principal};
    use icrc_ledger_types::icrc1::transfer::TransferError;
    use icrc_ledger_types::icrc1::account::Account;
    use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg};
    use crate::history::IcpTreasuryBonusTransfer;
    use crate::state::BotConfig;

    fn test_accounts() -> (Principal, Principal, Principal) {
        (
            Principal::from_text("aaaaa-aa").unwrap(),
            Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap(),
            Principal::from_text("rrkah-fqaaa-aaaaa-aaaaq-cai").unwrap(),
        )
    }

    fn bot_config() -> BotConfig {
        let (_, ledger, backend) = test_accounts();
        BotConfig {
            backend_principal: backend,
            treasury_principal: Principal::anonymous(),
            admin: Principal::anonymous(),
            max_slippage_bps: 100,
            icp_ledger: ledger,
            ckusdc_ledger: Principal::anonymous(),
            icpswap_pool: Principal::anonymous(),
            icpswap_zero_for_one: None,
            icp_fee_e8s: Some(10_000),
            ckusdc_fee_e6: None,
            three_pool_principal: None,
            kong_swap_principal: None,
            ckusdt_ledger: None,
            icusd_ledger: None,
        }
    }

    #[test]
    fn claim_return_tuple_is_generation_bound_and_reusable_without_new_time() {
        let config = bot_config();
        let bot_id = Principal::from_text("aaaaa-aa").unwrap();
        let transfer = prepare_claim_return_transfer(
            bot_id, &config, 2_000_000, 73, 123_456, 987_654,
        ).unwrap();
        assert!(claim_return_transfer_matches(&transfer, bot_id, &config, 73, 123_456));
        assert!(!claim_return_transfer_matches(&transfer, bot_id, &config, 74, 123_456));
        assert!(!claim_return_transfer_matches(&transfer, bot_id, &config, 73, 123_457));
        assert_eq!(transfer.args.created_at_time, Some(987_654));
        assert_eq!(transfer.args.amount.0.to_string(), "2010000");
        assert_eq!(transfer.args.fee.as_ref().map(|fee| fee.0.to_string()), Some("10000".to_string()));
        assert_eq!(
            transfer.args.memo.as_ref().map(|memo| memo.0.to_vec()),
            Some(bot_claim_return_memo(73, 123_456)),
        );
        assert_eq!(transfer.args.to.subaccount, Some(super::bot_claim_return_subaccount(73, 123_456)));
    }

    #[test]
    fn original_ckusdc_payment_pins_live_fee_and_exact_identity() {
        let config = bot_config();
        let memo = b"RUMI-BOT-LIQ:claim-123456";
        let transfer = prepare_ckusdc_payment_transfer(&config, 1_010, memo, 10, 987_654).unwrap();
        assert_eq!(transfer.ledger, config.ckusdc_ledger);
        assert_eq!(transfer.args.to.owner, config.backend_principal);
        assert_eq!(transfer.args.to.subaccount, None);
        assert_eq!(transfer.args.amount, Nat::from(1_000u64));
        assert_eq!(transfer.args.fee, Some(Nat::from(10u64)));
        assert_eq!(transfer.args.memo.as_ref().map(|value| value.0.to_vec()), Some(memo.to_vec()));
        assert_eq!(transfer.args.created_at_time, Some(987_654));
        assert_eq!(transfer.block_index, None);
        assert!(prepare_ckusdc_payment_transfer(&config, 9, memo, 10, 987_654).is_err());
        assert!(prepare_ckusdc_payment_transfer(&config, 10, memo, 10, 987_654).is_err());
    }

    fn transfer() -> IcpTreasuryBonusTransfer {
        let (sender, ledger, treasury) = test_accounts();
        let mut memo = b"RUMI:ICP_BONUS:V1:".to_vec();
        memo.extend_from_slice(&7u64.to_be_bytes());
        IcpTreasuryBonusTransfer {
            ledger,
            from: Account { owner: sender, subaccount: None },
            args: TransferArg {
                from_subaccount: None,
                to: Account { owner: treasury, subaccount: None },
                amount: Nat::from(10_000u64),
                fee: Some(Nat::from(10_000u64)),
                memo: Some(Memo::from(memo)),
                created_at_time: Some(123),
            },
            block_index: None,
        }
    }

    fn valid_block(transfer: &IcpTreasuryBonusTransfer, sender_id: Vec<u8>, treasury_id: Vec<u8>) -> IcpCandidBlock {
        IcpCandidBlock {
            parent_hash: None,
            transaction: IcpCandidTransaction {
                memo: 0,
                icrc1_memo: transfer.args.memo.as_ref().map(|memo| memo.0.to_vec()),
                operation: Some(IcpCandidOperation::Transfer {
                    from: sender_id,
                    to: treasury_id,
                    spender: None,
                    amount: IcpTokens { e8s: 10_000 },
                    fee: IcpTokens { e8s: 10_000 },
                }),
                created_at_time: IcpTimestamp { timestamp_nanos: 123 },
            },
            timestamp: IcpTimestamp { timestamp_nanos: 456 },
        }
    }

    #[test]
    fn icp_treasury_bonus_must_leave_a_positive_transfer_amount() {
        assert_eq!(icp_treasury_send_amount(10_001, 10_000), Ok(1));
        for amount in [0, 9_999, 10_000] {
            let error = icp_treasury_send_amount(amount, 10_000)
                .expect_err("zero-net transfer must stay a pending obligation");
            assert!(error.contains("does not exceed transfer fee"));
        }
    }

    #[test]
    fn duplicate_retry_is_accepted_as_the_original_block_receipt() {
        assert_eq!(
            icp_transfer_result_block(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(77u64),
            })),
            Ok(77),
        );
    }

    #[test]
    fn ambiguous_or_rejected_icp_outcomes_do_not_produce_a_paid_block() {
        assert!(icp_transfer_result_block(Err(
            TransferError::TemporarilyUnavailable {}
        )).is_err());
    }

    #[test]
    fn native_icp_query_blocks_format_reconciles_exact_tuple_and_rejects_tampering() {
        let journal = transfer();
        let (sender, ledger, treasury) = test_accounts();
        // Native ICP `query_blocks` uses 32-byte AccountIdentifier blobs and
        // keeps the ICRC-1 memo in its separate `icrc1_memo` field.
        let sender_id = vec![1u8; 32];
        let treasury_id = vec![2u8; 32];
        let block = valid_block(&journal, sender_id.clone(), treasury_id.clone());
        assert!(validate_icp_treasury_bonus_block(
            &block, &journal, ledger, sender, treasury, &sender_id, &treasury_id,
        ).is_ok());

        assert!(validate_icp_treasury_bonus_block(
            &block, &journal, Principal::anonymous(), sender, treasury, &sender_id, &treasury_id,
        ).is_err());

        for field in ["amt", "fee", "memo", "ts", "to", "from", "spender", "op"] {
            let mut altered = block.clone();
            let Some(IcpCandidOperation::Transfer { from, to, amount, fee, spender, .. }) = altered.transaction.operation.as_mut() else { unreachable!() };
            match field {
                "amt" => amount.e8s = 10_001,
                "fee" => fee.e8s = 10_001,
                "memo" => altered.transaction.icrc1_memo = Some(b"wrong".to_vec()),
                "ts" => altered.transaction.created_at_time.timestamp_nanos = 124,
                "to" => *to = vec![3u8; 32],
                "from" => *from = vec![3u8; 32],
                "spender" => *spender = Some(Vec::new()),
                "op" => altered.transaction.operation = Some(IcpCandidOperation::Burn {
                    from: sender_id.clone(), spender: None, amount: IcpTokens { e8s: 10_000 },
                }),
                _ => unreachable!(),
            }
            assert!(validate_icp_treasury_bonus_block(
                &altered, &journal, ledger, sender, treasury, &sender_id, &treasury_id,
            ).is_err(), "tampered {field} must not reconcile");
        }
    }

    #[test]
    fn exact_index_and_archive_range_are_bounded() {
        assert_eq!(block_offset(10, 1, 10), Some(0));
        assert_eq!(block_offset(10, 1, 11), None);
        assert!(range_covers_index(20, 1, 20));
        assert!(!range_covers_index(20, 1, 21));
        assert!(!range_covers_index(20, 0, 20));
    }
}
