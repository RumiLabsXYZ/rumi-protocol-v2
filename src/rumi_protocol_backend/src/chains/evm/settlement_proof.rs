//! Pure proof verification for manual foreign-chain burn settlement.
//!
//! These helpers verify the operator-supplied transaction receipts before the
//! accounting layer consumes Inc 5 pending burn balances. They intentionally
//! inspect actual receipt logs and ignore caller-supplied amount claims.

use candid::{CandidType, Deserialize};

use super::evm_rpc::{
    decode_burn_log_with_burner, TransferLog, TxReceiptWithLogs, BURN_EVENT_TOPIC0,
    TRANSFER_EVENT_TOPIC0,
};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BurnSettlementProofArg {
    pub tx_hash: String,
    pub log_index: u64,
    pub expected_burner: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettlementProofArg {
    pub burn_tx_hash: String,
    pub burn_log_index: u64,
    pub reserve_tx_hash: String,
    pub reserve_transfer_log_index: u64,
    pub expected_burner: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct VerifiedBurnSettlementProof {
    pub proof_id: String,
    pub tx_hash: String,
    pub log_index: u64,
    pub block_number: u64,
    pub vault_id: u64,
    pub burner: String,
    pub amount_e8s: u128,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct VerifiedReserveSettlementProof {
    pub proof_id: String,
    pub burn_tx_hash: String,
    pub burn_log_index: u64,
    pub burn_block_number: u64,
    pub reserve_tx_hash: String,
    pub reserve_transfer_log_index: u64,
    pub reserve_block_number: u64,
    pub vault_id: u64,
    pub burner: String,
    pub amount_e8s: u128,
    pub reserve_transfer_amount_native: u128,
    pub reserve_transfer_amount_e8s: u128,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SettlementProofError {
    InvalidAddress(String),
    InvalidTxHash(String),
    MalformedLog(String),
    ReceiptReverted,
    NoMatchingBurnLog,
    UnexpectedBurner {
        expected: String,
        actual: String,
    },
    ReceiptTxHashMismatch {
        expected: String,
        actual: String,
    },
    ReserveTransferMissing,
    ReserveTransferTooSmall {
        expected_e8s: u128,
        actual_e8s: u128,
    },
    ReserveTransferScaleOverflow,
}

pub fn verify_pending_burn_receipt(
    icusd_contract: &str,
    proof: &BurnSettlementProofArg,
    receipt: &TxReceiptWithLogs,
) -> Result<VerifiedBurnSettlementProof, SettlementProofError> {
    let tx_hash = canonical_tx_hash(&proof.tx_hash)?;
    verify_receipt_tx_hash(receipt, &tx_hash)?;
    verify_receipt_success(receipt)?;
    let icusd_contract = canonical_address(icusd_contract)?;
    let expected_burner = proof
        .expected_burner
        .as_deref()
        .map(canonical_address)
        .transpose()?;

    let (_, topics, data, log_index) = receipt
        .logs
        .iter()
        .find(|(address, topics, _, log_index)| {
            *log_index == proof.log_index
                && address.eq_ignore_ascii_case(&icusd_contract)
                && topics
                    .first()
                    .map(|topic0| topic0.eq_ignore_ascii_case(BURN_EVENT_TOPIC0))
                    .unwrap_or(false)
        })
        .ok_or(SettlementProofError::NoMatchingBurnLog)?;

    let burn = decode_burn_log_with_burner(topics, data, &tx_hash, receipt.block_number)
        .map_err(SettlementProofError::MalformedLog)?;

    if let Some(expected) = expected_burner {
        if burn.burner != expected {
            return Err(SettlementProofError::UnexpectedBurner {
                expected,
                actual: burn.burner,
            });
        }
    }

    Ok(VerifiedBurnSettlementProof {
        proof_id: format!("pending:{tx_hash}:{log_index}"),
        tx_hash,
        log_index: *log_index,
        block_number: receipt.block_number,
        vault_id: burn.vault_id,
        burner: burn.burner,
        amount_e8s: burn.amount_e8s,
    })
}

pub fn verify_reserve_burn_receipts(
    icusd_contract: &str,
    settle_stable_token: &str,
    reserve_address: &str,
    settle_stable_decimals: u8,
    proof: &ReserveSettlementProofArg,
    burn_receipt: &TxReceiptWithLogs,
    reserve_receipt: &TxReceiptWithLogs,
) -> Result<VerifiedReserveSettlementProof, SettlementProofError> {
    let burn_proof = BurnSettlementProofArg {
        tx_hash: proof.burn_tx_hash.clone(),
        log_index: proof.burn_log_index,
        expected_burner: proof.expected_burner.clone(),
    };
    let burn = verify_pending_burn_receipt(icusd_contract, &burn_proof, burn_receipt)?;

    let reserve_tx_hash = canonical_tx_hash(&proof.reserve_tx_hash)?;
    verify_receipt_tx_hash(reserve_receipt, &reserve_tx_hash)?;
    verify_receipt_success(reserve_receipt)?;
    let settle_stable_token = canonical_address(settle_stable_token)?;
    let reserve_address = canonical_address(reserve_address)?;

    let (_, topics, data, log_index) = reserve_receipt
        .logs
        .iter()
        .find(|(address, topics, _, log_index)| {
            *log_index == proof.reserve_transfer_log_index
                && address.eq_ignore_ascii_case(&settle_stable_token)
                && topics
                    .first()
                    .map(|topic0| topic0.eq_ignore_ascii_case(TRANSFER_EVENT_TOPIC0))
                    .unwrap_or(false)
        })
        .ok_or(SettlementProofError::ReserveTransferMissing)?;

    let transfer =
        TransferLog::from_raw(topics, data).map_err(SettlementProofError::MalformedLog)?;
    if transfer.to != reserve_address {
        return Err(SettlementProofError::ReserveTransferMissing);
    }
    let reserve_transfer_amount_e8s =
        stable_native_to_e8s(transfer.amount, settle_stable_decimals)?;
    if reserve_transfer_amount_e8s < burn.amount_e8s {
        return Err(SettlementProofError::ReserveTransferTooSmall {
            expected_e8s: burn.amount_e8s,
            actual_e8s: reserve_transfer_amount_e8s,
        });
    }

    Ok(VerifiedReserveSettlementProof {
        proof_id: format!(
            "reserve:{}:{}:{}:{}",
            burn.tx_hash, burn.log_index, reserve_tx_hash, log_index
        ),
        burn_tx_hash: burn.tx_hash,
        burn_log_index: burn.log_index,
        burn_block_number: burn.block_number,
        reserve_tx_hash,
        reserve_transfer_log_index: *log_index,
        reserve_block_number: reserve_receipt.block_number,
        vault_id: burn.vault_id,
        burner: burn.burner,
        amount_e8s: burn.amount_e8s,
        reserve_transfer_amount_native: transfer.amount,
        reserve_transfer_amount_e8s,
    })
}

fn verify_receipt_success(receipt: &TxReceiptWithLogs) -> Result<(), SettlementProofError> {
    if receipt.success {
        Ok(())
    } else {
        Err(SettlementProofError::ReceiptReverted)
    }
}

fn verify_receipt_tx_hash(
    receipt: &TxReceiptWithLogs,
    expected: &str,
) -> Result<(), SettlementProofError> {
    match receipt.tx_hash.as_deref() {
        Some(actual) if actual.eq_ignore_ascii_case(expected) => Ok(()),
        Some(actual) => Err(SettlementProofError::ReceiptTxHashMismatch {
            expected: expected.to_string(),
            actual: actual.to_ascii_lowercase(),
        }),
        None => Err(SettlementProofError::ReceiptTxHashMismatch {
            expected: expected.to_string(),
            actual: "<missing>".to_string(),
        }),
    }
}

fn stable_native_to_e8s(
    amount_native: u128,
    decimals: u8,
) -> Result<u128, SettlementProofError> {
    match decimals.cmp(&8) {
        core::cmp::Ordering::Equal => Ok(amount_native),
        core::cmp::Ordering::Greater => {
            let scale = pow10_u128(decimals - 8)?;
            Ok(amount_native / scale)
        }
        core::cmp::Ordering::Less => {
            let scale = pow10_u128(8 - decimals)?;
            amount_native
                .checked_mul(scale)
                .ok_or(SettlementProofError::ReserveTransferScaleOverflow)
        }
    }
}

fn pow10_u128(exp: u8) -> Result<u128, SettlementProofError> {
    let mut out = 1u128;
    for _ in 0..exp {
        out = out
            .checked_mul(10)
            .ok_or(SettlementProofError::ReserveTransferScaleOverflow)?;
    }
    Ok(out)
}

fn canonical_address(address: &str) -> Result<String, SettlementProofError> {
    let hex = address
        .trim()
        .strip_prefix("0x")
        .or_else(|| address.trim().strip_prefix("0X"))
        .ok_or_else(|| SettlementProofError::InvalidAddress(address.to_string()))?;
    if hex.len() != 40 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SettlementProofError::InvalidAddress(address.to_string()));
    }
    Ok(format!("0x{}", hex.to_ascii_lowercase()))
}

fn canonical_tx_hash(tx_hash: &str) -> Result<String, SettlementProofError> {
    let hex = tx_hash
        .trim()
        .strip_prefix("0x")
        .or_else(|| tx_hash.trim().strip_prefix("0X"))
        .ok_or_else(|| SettlementProofError::InvalidTxHash(tx_hash.to_string()))?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SettlementProofError::InvalidTxHash(tx_hash.to_string()));
    }
    Ok(format!("0x{}", hex.to_ascii_lowercase()))
}
