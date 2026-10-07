//! CL-03 short bot payment and exact aggregate recovery regression.

include!("common/bot_claim_fixture.rs");

#[derive(CandidType, Deserialize)]
struct BotLiquidationResultWithMemo {
    vault_id: u64,
    collateral_amount: u64,
    debt_covered: u64,
    collateral_price_e8s: u64,
    claim_timestamp: Option<u64>,
    payment_memo: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Debug)]
struct BotPartialPaymentLockEvidence {
    ledger: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: Vec<u8>,
    payment_block_index: u64,
    total_amount_e6: u64,
    aggregate_complete: bool,
}

fn claim_payment_subaccount(vault_id: u64, claim_timestamp: u64) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    subaccount[..16].copy_from_slice(b"RUMI-CLAIM-PAY01");
    subaccount[16..24].copy_from_slice(&vault_id.to_be_bytes());
    subaccount[24..32].copy_from_slice(&claim_timestamp.to_be_bytes());
    subaccount
}

fn mint_ckusdc_to_claim_account(f: &Fixture, claim_timestamp: u64, amount: u64) {
    let result = f
        .pic
        .update_call(
            f.ckusdc_ledger,
            Principal::anonymous(),
            "mint",
            encode_args((
                Account {
                    owner: f.bot_id,
                    subaccount: Some(claim_payment_subaccount(f.vault_id, claim_timestamp)),
                },
                Nat::from(amount),
            ))
            .unwrap(),
        )
        .expect("mint test ckUSDC into the claim-specific bot account");
    match result {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode ckUSDC mint"),
        WasmResult::Reject(message) => panic!("ckUSDC mint rejected: {message}"),
    }
}

fn transfer_ckusdc_payment(f: &Fixture, claim_timestamp: u64, memo: &[u8], amount: u64) -> u64 {
    let args = TransferArg {
        from_subaccount: Some(claim_payment_subaccount(f.vault_id, claim_timestamp)),
        to: account(f.protocol_id),
        amount: Nat::from(amount),
        fee: Some(Nat::from(0u64)),
        memo: Some(memo.to_vec()),
        created_at_time: Some(now_ns(&f.pic).max(claim_timestamp)),
    };
    let result = f
        .pic
        .update_call(
            f.ckusdc_ledger,
            f.bot_id,
            "icrc1_transfer",
            encode_one(args).unwrap(),
        )
        .expect("ckUSDC claim payment transfer call");
    let block_index: Nat = match result {
        WasmResult::Reply(bytes) => {
            let parsed: Result<Nat, TransferError> =
                decode_one(&bytes).expect("decode ckUSDC transfer");
            parsed.expect("ckUSDC claim payment transfer succeeds")
        }
        WasmResult::Reject(message) => panic!("ckUSDC transfer rejected: {message}"),
    };
    use num_traits::ToPrimitive;
    block_index.0.to_u64().expect("block index fits u64")
}

fn confirm_payment_blocks(
    f: &Fixture,
    claim_timestamp: u64,
    indexes: Vec<u64>,
) -> Result<(), ProtocolError> {
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.bot_id,
            "bot_confirm_liquidation_with_payments",
            encode_args((f.vault_id, claim_timestamp, indexes)).unwrap(),
        )
        .expect("aggregate payment confirmation call");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode aggregate confirmation"),
        WasmResult::Reject(message) => panic!("aggregate confirmation rejected: {message}"),
    }
}

fn partial_payment_evidence(
    f: &Fixture,
    claim_timestamp: u64,
    block_index: u64,
) -> Result<Option<BotPartialPaymentLockEvidence>, ProtocolError> {
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.bot_id,
            "bot_claim_partial_payment_block_evidence",
            encode_args((f.vault_id, claim_timestamp, block_index)).unwrap(),
        )
        .expect("partial-payment evidence call");
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode partial-payment evidence"),
        WasmResult::Reject(message) => panic!("partial-payment evidence rejected: {message}"),
    }
}

#[test]
fn cl03_short_payment_is_locked_then_exact_topup_settles_once() {
    let f = setup_fixture();
    set_liquidation_bot_config_admin(&f, f.bot_id, 1_000_000_000_000);
    assert_eq!(enroll_bot_request_id_floor(&f), 0);
    drop_icp_price(&f, 250_000_000);
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.bot_id,
            "bot_claim_liquidation_with_request_id",
            encode_args((f.vault_id, 1u64)).unwrap(),
        )
        .expect("claim liquidation call");
    let claim_result: Result<BotLiquidationResultWithMemo, ProtocolError> = match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode memo-bound bot claim result"),
        WasmResult::Reject(message) => panic!("bot claim rejected: {message}"),
    };
    let claim = claim_result.expect("memo-bound bot claim succeeds");
    let claim_timestamp = claim.claim_timestamp.expect("claim generation timestamp");
    let memo = claim.payment_memo.expect("claim payment memo");
    assert_eq!(claim.vault_id, f.vault_id);
    assert!(claim.collateral_amount > 0);
    assert!(claim.collateral_price_e8s > 0);

    let minimum = claim.debt_covered / 100 + u64::from(claim.debt_covered % 100 != 0);
    assert!(minimum > 1, "fixture must need a nontrivial ckUSDC payment");
    let short_amount = minimum - 1;
    mint_ckusdc_to_claim_account(&f, claim_timestamp, minimum);
    let short_index = transfer_ckusdc_payment(&f, claim_timestamp, &memo, short_amount);

    let first = f
        .pic
        .update_call(
            f.protocol_id,
            f.bot_id,
            "bot_confirm_liquidation_with_payment",
            encode_args((f.vault_id, claim_timestamp, short_index)).unwrap(),
        )
        .expect("short payment confirmation call");
    let short_result: Result<(), ProtocolError> = match first {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode short payment result"),
        WasmResult::Reject(message) => panic!("short payment call rejected: {message}"),
    };
    assert!(
        short_result.is_err(),
        "under-minimum payment must not settle: {short_result:?}"
    );

    let evidence = partial_payment_evidence(&f, claim_timestamp, short_index)
        .expect("partial-payment evidence query succeeds")
        .expect("short block is durably locked to its claim");
    assert_eq!(evidence.ledger, f.ckusdc_ledger);
    assert_eq!(evidence.vault_id, f.vault_id);
    assert_eq!(evidence.claim_timestamp, claim_timestamp);
    assert_eq!(evidence.payment_memo, memo);
    assert_eq!(evidence.payment_block_index, short_index);
    assert_eq!(evidence.total_amount_e6, short_amount);
    assert!(!evidence.aggregate_complete);
    assert!(partial_payment_evidence(&f, claim_timestamp + 1, short_index).is_err());
    assert!(
        partial_payment_evidence(&f, claim_timestamp, short_index + 100)
            .unwrap()
            .is_none()
    );

    let amount_call = f
        .pic
        .update_call(
            f.protocol_id,
            f.bot_id,
            "bot_claim_partial_payment_amount",
            encode_args((f.vault_id, claim_timestamp)).unwrap(),
        )
        .expect("partial-payment amount call");
    let partial_amount: Result<Option<u64>, ProtocolError> = match amount_call {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode partial amount"),
        WasmResult::Reject(message) => panic!("partial-payment amount rejected: {message}"),
    };
    assert_eq!(
        partial_amount.expect("partial amount query succeeds"),
        Some(short_amount)
    );
    assert!(get_active_claim_ids(&f.pic, f.protocol_id).contains(&f.vault_id));

    let topup_index = transfer_ckusdc_payment(&f, claim_timestamp, &memo, minimum - short_amount);
    assert_ne!(topup_index, short_index);
    let settled = confirm_payment_blocks(&f, claim_timestamp, vec![short_index, topup_index]);
    assert!(
        settled.is_ok(),
        "exact short+top-up aggregate settles: {settled:?}"
    );
    assert!(!get_active_claim_ids(&f.pic, f.protocol_id).contains(&f.vault_id));

    let exact_replay = confirm_payment_blocks(&f, claim_timestamp, vec![topup_index, short_index]);
    assert!(
        exact_replay.is_ok(),
        "canonical exact aggregate replay is idempotent"
    );
    assert!(
        partial_payment_evidence(&f, claim_timestamp, short_index).is_err(),
        "partial evidence endpoint no longer applies after the claim is settled"
    );
}
