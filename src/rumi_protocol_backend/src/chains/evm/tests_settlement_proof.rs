use super::evm_rpc::{TxReceiptWithLogs, BURN_EVENT_TOPIC0, TRANSFER_EVENT_TOPIC0};
use super::settlement_proof::{
    verify_pending_burn_receipt, verify_reserve_burn_receipts, BurnSettlementProofArg,
    ReserveSettlementProofArg, SettlementProofError,
};

fn word(v: u128) -> String {
    format!("0x{v:064x}")
}

fn word_addr(addr: &str) -> String {
    let h = addr.trim_start_matches("0x");
    format!("0x{h:0>64}")
}

fn receipt(
    tx_hash: &str,
    success: bool,
    block_number: u64,
    logs: Vec<(String, Vec<String>, String, u64)>,
) -> TxReceiptWithLogs {
    TxReceiptWithLogs {
        tx_hash: Some(tx_hash.to_string()),
        success,
        block_number,
        logs,
    }
}

fn burn_log(
    contract: &str,
    vault_id: u64,
    burner: &str,
    amount_e8s: u128,
    log_index: u64,
) -> (String, Vec<String>, String, u64) {
    (
        contract.to_ascii_lowercase(),
        vec![
            BURN_EVENT_TOPIC0.to_string(),
            word(vault_id as u128),
            word_addr(burner),
        ],
        word(amount_e8s),
        log_index,
    )
}

fn transfer_log(
    token: &str,
    to: &str,
    amount_native: u128,
    log_index: u64,
) -> (String, Vec<String>, String, u64) {
    (
        token.to_ascii_lowercase(),
        vec![
            TRANSFER_EVENT_TOPIC0.to_string(),
            word_addr("0x000000000000000000000000000000000000aaaa"),
            word_addr(to),
        ],
        word(amount_native),
        log_index,
    )
}

#[test]
fn pending_burn_proof_accepts_exact_contract_log_index_and_amount_from_log() {
    let contract = "0x000000000000000000000000000000000000cafe";
    let proof = BurnSettlementProofArg {
        tx_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        log_index: 7,
        expected_burner: Some("0x000000000000000000000000000000000000beef".to_string()),
    };
    let receipt = receipt(
        &proof.tx_hash,
        true,
        55,
        vec![
            transfer_log(
                "0x000000000000000000000000000000000000feed",
                "0x000000000000000000000000000000000000aaaa",
                1,
                3,
            ),
            burn_log(
                contract,
                99,
                "0x000000000000000000000000000000000000beef",
                25_000_000,
                7,
            ),
        ],
    );

    let verified = verify_pending_burn_receipt(contract, &proof, &receipt).expect("verified proof");
    assert_eq!(verified.amount_e8s, 25_000_000);
    assert_eq!(verified.block_number, 55);
    assert_eq!(verified.log_index, 7);
    assert_eq!(
        verified.burner,
        "0x000000000000000000000000000000000000beef"
    );
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

    let wrong_contract = receipt(
        &proof.tx_hash,
        true,
        55,
        vec![burn_log(
            "0x000000000000000000000000000000000000dead",
            99,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            2,
        )],
    );
    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &wrong_contract),
        Err(SettlementProofError::NoMatchingBurnLog)
    ));

    let wrong_index = receipt(
        &proof.tx_hash,
        true,
        55,
        vec![burn_log(
            contract,
            99,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            3,
        )],
    );
    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &wrong_index),
        Err(SettlementProofError::NoMatchingBurnLog)
    ));

    let wrong_burner = receipt(
        &proof.tx_hash,
        true,
        55,
        vec![burn_log(
            contract,
            99,
            "0x000000000000000000000000000000000000badd",
            25_000_000,
            2,
        )],
    );
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
        burn_tx_hash: "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
            .to_string(),
        burn_log_index: 4,
        reserve_tx_hash: "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
            .to_string(),
        reserve_transfer_log_index: 8,
        expected_burner: Some("0x000000000000000000000000000000000000beef".to_string()),
    };
    let burn_receipt = receipt(
        &proof.burn_tx_hash,
        true,
        70,
        vec![burn_log(
            icusd,
            0,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            4,
        )],
    );
    let reserve_receipt = receipt(
        &proof.reserve_tx_hash,
        true,
        71,
        vec![transfer_log(
            usdc,
            reserve,
            25_000_000u128 * 10_000_000_000u128,
            8,
        )],
    );

    let verified = verify_reserve_burn_receipts(
        icusd,
        usdc,
        reserve,
        18,
        &proof,
        &burn_receipt,
        &reserve_receipt,
    )
    .expect("reserve proof verified");
    assert_eq!(verified.amount_e8s, 25_000_000);
    assert_eq!(
        verified.reserve_transfer_amount_native,
        25_000_000u128 * 10_000_000_000u128
    );
    assert_eq!(verified.reserve_transfer_amount_e8s, 25_000_000);
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
        burn_tx_hash: "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_string(),
        burn_log_index: 4,
        reserve_tx_hash: "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_string(),
        reserve_transfer_log_index: 8,
        expected_burner: None,
    };
    let burn_receipt = receipt(
        &proof.burn_tx_hash,
        true,
        70,
        vec![burn_log(
            icusd,
            0,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            4,
        )],
    );

    let wrong_recipient = receipt(
        &proof.reserve_tx_hash,
        true,
        71,
        vec![transfer_log(
            usdc,
            "0x0000000000000000000000000000000000003333",
            25_000_000,
            8,
        )],
    );
    assert!(matches!(
        verify_reserve_burn_receipts(
            icusd,
            usdc,
            reserve,
            18,
            &proof,
            &burn_receipt,
            &wrong_recipient
        ),
        Err(SettlementProofError::ReserveTransferMissing)
    ));

    let too_small = receipt(
        &proof.reserve_tx_hash,
        true,
        71,
        vec![transfer_log(
            usdc,
            reserve,
            25_000_000u128 * 10_000_000_000u128 - 1,
            8,
        )],
    );
    assert!(matches!(
        verify_reserve_burn_receipts(icusd, usdc, reserve, 18, &proof, &burn_receipt, &too_small),
        Err(SettlementProofError::ReserveTransferTooSmall { .. })
    ));
}

#[test]
fn reserve_burn_proof_rejects_unscaled_native_transfer_for_18_decimal_stable() {
    let icusd = "0x000000000000000000000000000000000000cafe";
    let usdc = "0x0000000000000000000000000000000000001000";
    let reserve = "0x0000000000000000000000000000000000002222";
    let proof = ReserveSettlementProofArg {
        burn_tx_hash: "0x1212121212121212121212121212121212121212121212121212121212121212"
            .to_string(),
        burn_log_index: 4,
        reserve_tx_hash: "0x3434343434343434343434343434343434343434343434343434343434343434"
            .to_string(),
        reserve_transfer_log_index: 8,
        expected_burner: None,
    };
    let burn_receipt = receipt(
        &proof.burn_tx_hash,
        true,
        70,
        vec![burn_log(
            icusd,
            0,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            4,
        )],
    );
    let unscaled_reserve_receipt = receipt(
        &proof.reserve_tx_hash,
        true,
        71,
        vec![transfer_log(usdc, reserve, 25_000_000, 8)],
    );

    assert!(matches!(
        verify_reserve_burn_receipts(
            icusd,
            usdc,
            reserve,
            18,
            &proof,
            &burn_receipt,
            &unscaled_reserve_receipt,
        ),
        Err(SettlementProofError::ReserveTransferTooSmall { .. })
    ));
}

#[test]
fn pending_burn_proof_rejects_receipt_transaction_hash_mismatch() {
    let contract = "0x000000000000000000000000000000000000cafe";
    let proof = BurnSettlementProofArg {
        tx_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        log_index: 7,
        expected_burner: None,
    };
    let mismatched_receipt = receipt(
        "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        true,
        55,
        vec![burn_log(
            contract,
            99,
            "0x000000000000000000000000000000000000beef",
            25_000_000,
            7,
        )],
    );

    assert!(matches!(
        verify_pending_burn_receipt(contract, &proof, &mismatched_receipt),
        Err(SettlementProofError::ReceiptTxHashMismatch { .. })
    ));
}
