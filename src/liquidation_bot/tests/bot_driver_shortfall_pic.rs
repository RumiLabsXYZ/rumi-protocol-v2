//! Exercises the production bot worker and recovery update handlers against
//! the real backend and receipt-capable local ledgers. The only substituted
//! component is the pool quote/output, behind `bot_driver_test`.

include!("../../rumi_protocol_backend/tests/common/bot_claim_fixture.rs");

#[derive(CandidType, Deserialize)]
struct DriverVault {
    vault_id: u64,
    collateral_type: Principal,
    debt_amount: u64,
    collateral_amount: u64,
    recommended_liquidation_amount: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize)]
struct ClaimWithMemo {
    vault_id: u64,
    collateral_amount: u64,
    debt_covered: u64,
    collateral_price_e8s: u64,
    claim_timestamp: Option<u64>,
    payment_memo: Option<Vec<u8>>,
}

fn call_unit(pic: &PocketIc, canister: Principal, caller: Principal, method: &str, args: Vec<u8>) {
    match pic.update_call(canister, caller, method, args).expect("update call") {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode unit reply"),
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

fn bot_claim_subaccount(vault_id: u64, claim_timestamp: u64) -> [u8; 32] {
    let mut subaccount = [0u8; 32];
    subaccount[..16].copy_from_slice(b"RUMI-CLAIM-PAY01");
    subaccount[16..24].copy_from_slice(&vault_id.to_be_bytes());
    subaccount[24..32].copy_from_slice(&claim_timestamp.to_be_bytes());
    subaccount
}

#[test]
fn worker_holds_short_swap_then_recovers_claim_payment_after_upgrade() {
    let f = setup_fixture();
    set_liquidation_bot_config_admin(&f, f.bot_id, 1_000_000_000_000);
    assert_eq!(enroll_bot_request_id_floor(&f), 0);
    drop_icp_price(&f, 250_000_000);

    let vault = DriverVault {
        vault_id: f.vault_id,
        collateral_type: f.icp_ledger,
        debt_amount: 10_000_000_000,
        collateral_amount: 5_000_000_000,
        recommended_liquidation_amount: 10_000_000_000,
        collateral_price_e8s: 250_000_000,
    };
    // A quote above the payment floor followed by a much shorter wallet
    // credit reproduces a pool that violates its advertised minimum.
    let driven = f.pic.update_call(
        f.bot_id,
        f.developer,
        "test_drive_liquidation_worker",
        encode_args((vault, 250_000_000u64, 1_000_000u64)).unwrap(),
    ).expect("run bot claim worker");
    match driven {
        WasmResult::Reply(bytes) => {
            let result: Result<(), String> = decode_one(&bytes).expect("decode worker result");
            result.expect("worker completes by recording the shortfall hold");
        }
        WasmResult::Reject(message) => panic!("bot worker rejected: {message}"),
    }

    let active = f.pic.update_call(
        f.protocol_id,
        f.bot_id,
        "get_bot_claim_vault_ids",
        encode_args(()).unwrap(),
    ).expect("read active bot claims");
    let active_ids: Vec<u64> = match active {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode active bot claims"),
        WasmResult::Reject(message) => panic!("active claim query rejected: {message}"),
    };
    assert!(active_ids.contains(&f.vault_id), "short swap must leave the backend claim active");

    let bot_record = f.pic.query_call(
        f.bot_id,
        f.developer,
        "get_liquidation",
        encode_args((0u64,)).unwrap(),
    ).expect("read bot liquidation record");
    let bot_record = match bot_record {
        WasmResult::Reply(bytes) => candid::IDLArgs::from_bytes(&bytes).expect("decode bot record"),
        WasmResult::Reject(message) => panic!("bot history query rejected: {message}"),
    };
    assert!(
        !format!("{bot_record:#?}")
            .contains("backend claim lacks complete exact collateral-transfer provenance"),
        "backend PIC fixture Wasm is stale: it omits the claim-transfer proof required by the bot"
    );

    // Request zero is the bot's first durable request ID. Replaying it is
    // idempotent and provides the exact claim timestamp/memo used by recovery.
    let claim = f.pic.update_call(
        f.protocol_id,
        f.bot_id,
        "bot_claim_liquidation_with_request_id",
        encode_args((f.vault_id, 0u64)).unwrap(),
    ).expect("read exact claim receipt");
    let claim: Result<ClaimWithMemo, ProtocolError> = match claim {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode exact claim receipt"),
        WasmResult::Reject(message) => panic!("claim receipt rejected: {message}"),
    };
    let claim = claim.expect("existing claim receipt");
    let claim_timestamp = claim.claim_timestamp.expect("claim timestamp");
    let memo = claim.payment_memo.expect("claim payment memo");
    let minimum = claim.debt_covered / 100 + u64::from(claim.debt_covered % 100 != 0);
    assert!(minimum > 1_000_000, "fixture must remain short after the mock swap");

    // Recovery is funded only in this claim generation's account. The
    // default bot account still contains only the measured swap output.
    let source = Account {
        owner: f.bot_id,
        subaccount: Some(bot_claim_subaccount(f.vault_id, claim_timestamp)),
    };
    let minted = f.pic.update_call(
        f.ckusdc_ledger,
        Principal::anonymous(),
        "mint",
        encode_args((source, Nat::from(minimum))).unwrap(),
    ).expect("fund exact claim account");
    match minted {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode mint"),
        WasmResult::Reject(message) => panic!("claim account funding rejected: {message}"),
    }

    // Commit the exact full-minimum payment and lose its reply. The bot's
    // durable ledger-history reconciliation must recover it before upgrade.
    call_unit(
        &f.pic,
        f.ckusdc_ledger,
        Principal::anonymous(),
        "set_phantom_failures",
        encode_one(1u32).unwrap(),
    );
    let recovery = f.pic.update_call(
        f.bot_id,
        f.developer,
        "admin_recover_ckusdc_shortfall",
        encode_args((f.vault_id, u64::MAX)).unwrap(),
    ).expect("admin shortfall recovery");
    match recovery {
        WasmResult::Reply(bytes) => {
            let result: Result<(), String> = decode_one(&bytes).expect("decode shortfall recovery");
            result.expect("exact lost-reply payment recovers from ledger history");
        }
        WasmResult::Reject(message) => panic!("shortfall recovery rejected: {message}"),
    }

    // Preserve the paid-but-unconfirmed journal over an actual bot upgrade,
    // then let the real retry handler confirm the exact ledger block.
    f.pic.upgrade_canister(
        f.bot_id,
        liquidation_bot_wasm(),
        encode_one(()).unwrap(),
        None,
    ).expect("upgrade bot with payment journal pending confirmation");
    let retried = f.pic.update_call(
        f.bot_id,
        f.developer,
        "admin_retry_stuck_claim",
        encode_args((f.vault_id,)).unwrap(),
    ).expect("retry backend confirmation");
    match retried {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode confirmation retry"),
        WasmResult::Reject(message) => panic!("confirmation retry rejected: {message}"),
    }

    let final_active = f.pic.update_call(
        f.protocol_id,
        f.bot_id,
        "get_bot_claim_vault_ids",
        encode_args(()).unwrap(),
    ).expect("read final active claims");
    let final_ids: Vec<u64> = match final_active {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode final active claims"),
        WasmResult::Reject(message) => panic!("final claim query rejected: {message}"),
    };
    assert!(!final_ids.contains(&f.vault_id), "exact recovery should settle the original claim");
    assert!(!memo.is_empty());
}
