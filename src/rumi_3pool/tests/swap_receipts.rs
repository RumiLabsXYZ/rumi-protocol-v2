//! Local ledger integration: durable receipts bind real returned transfer IDs,
//! duplicate requests never run another economic operation, and uncertainty
//! keeps reserve mutations fenced even after an upgrade.
mod common;
use candid::{decode_one, encode_args, encode_one, Nat, Principal};
use common::{
    deploy_pool_with_archive_cycles, deploy_pool_with_liquidity_fee_and_swaps, three_pool_wasm,
    ThreePoolHarness,
};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use pocket_ic::WasmResult;
use rumi_3pool::receipts::*;
use rumi_3pool::types::ThreePoolError;

fn bytes(r: WasmResult) -> Vec<u8> {
    match r {
        WasmResult::Reply(b) => b,
        other => panic!("{other:?}"),
    }
}
fn request() -> SwapRequestV1 {
    SwapRequestV1 {
        intent_id: vec![8; 32],
        i: 0,
        j: 1,
        dx: 100_000_000,
        min_dy: 1,
    }
}
fn submit(h: &ThreePoolHarness, r: SwapRequestV1) -> Result<SwapReceiptV1, SwapReceiptErrorV1> {
    decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "swap_with_receipt_v1",
                encode_one(r).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}
fn query(h: &ThreePoolHarness, owner: Principal) -> Option<SwapReceiptV1> {
    decode_one(&bytes(
        h.pic
            .query_call(
                h.three_pool,
                owner,
                "get_swap_receipt_v1",
                encode_one(request().intent_id).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}
fn balance(h: &ThreePoolHarness, ledger: Principal, owner: Principal) -> u128 {
    let n: Nat = decode_one(&bytes(
        h.pic
            .query_call(
                ledger,
                h.user,
                "icrc1_balance_of",
                encode_one(Account {
                    owner,
                    subaccount: None,
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    n.0.try_into().unwrap()
}
fn verify_ledger_transaction(h: &ThreePoolHarness, t: &SwapTransferV1) {
    use icrc_ledger_types::icrc3::transactions::{GetTransactionsRequest, GetTransactionsResponse};
    let response: GetTransactionsResponse = decode_one(&bytes(
        h.pic
            .query_call(
                t.ledger,
                h.user,
                "get_transactions",
                encode_one(GetTransactionsRequest {
                    start: t.block_index.clone().unwrap(),
                    length: Nat::from(1u64),
                })
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(response.first_index, t.block_index.clone().unwrap());
    assert_eq!(response.transactions.len(), 1);
    let actual = response.transactions[0].transfer.as_ref().unwrap();
    assert_eq!(actual.from, t.from);
    assert_eq!(actual.to, t.to);
    assert_eq!(actual.amount, Nat::from(t.amount));
    assert_eq!(actual.fee, Some(Nat::from(t.fee)));
    assert_eq!(actual.memo, Some(t.memo.clone().into()));
    assert_eq!(actual.created_at_time, Some(t.created_at_time));
}
fn enable(h: &ThreePoolHarness) {
    let r: Result<(), SwapReceiptErrorV1> = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.admin,
                "set_swap_receipt_client_v1",
                encode_args((h.user, true)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    r.unwrap();
}
#[test]
fn wallet_receipts_bind_ledger_economics_and_replay_survives_upgrade() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    assert!(query(&h, h.user).is_none());
    let before_in = balance(&h, h.ledgers[0], h.user);
    let before_out = balance(&h, h.ledgers[1], h.user);
    let r = submit(&h, request()).unwrap();
    assert_eq!(r.status, SwapReceiptStatusV1::Completed);
    assert_eq!(r.owner, h.user);
    assert_eq!(r.version, 1);
    assert_eq!(r.request, request());
    let input = r.input.as_ref().unwrap();
    let output = r.output.as_ref().unwrap();
    assert_eq!(
        input.from,
        Account {
            owner: h.user,
            subaccount: None
        }
    );
    assert_eq!(input.to.owner, h.three_pool);
    assert_eq!(output.from.owner, h.three_pool);
    assert_eq!(output.to.owner, h.user);
    assert_eq!(input.ledger, h.ledgers[0]);
    assert_eq!(output.ledger, h.ledgers[1]);
    assert_eq!(input.fee, 10_000);
    assert_eq!(output.fee, 10_000);
    assert!(input.block_index.is_some());
    assert!(output.block_index.is_some());
    assert_eq!(input.status, SwapTransferStatusV1::Confirmed);
    assert_eq!(output.status, SwapTransferStatusV1::Confirmed);
    assert_ne!(input.memo, output.memo);
    verify_ledger_transaction(&h, input);
    verify_ledger_transaction(&h, output);
    assert_eq!(
        before_in - balance(&h, h.ledgers[0], h.user),
        request().dx + input.fee
    );
    assert_eq!(
        balance(&h, h.ledgers[1], h.user) - before_out,
        output.amount
    );
    assert_eq!(output.amount + output.fee, r.gross_output.unwrap());
    assert_eq!(submit(&h, request()).unwrap(), r);
    let mut conflict = request();
    conflict.min_dy += 1;
    assert_eq!(
        submit(&h, conflict).unwrap_err(),
        SwapReceiptErrorV1::IntentConflict
    );
    assert!(query(&h, h.admin).is_none());
    h.pic
        .upgrade_canister(
            h.three_pool,
            three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    assert_eq!(query(&h, h.user), Some(r.clone()));
    assert_eq!(submit(&h, request()).unwrap(), r);
    assert_eq!(
        before_in - balance(&h, h.ledgers[0], h.user),
        request().dx + input.fee
    );
}
#[test]
fn stopped_output_ledger_never_refunds_or_replays_and_fence_survives_upgrade() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    enable(&h);
    h.pic.stop_canister(h.ledgers[1], None).unwrap();
    let before = balance(&h, h.ledgers[0], h.user);
    let r = submit(&h, request()).unwrap();
    assert_eq!(r.status, SwapReceiptStatusV1::Unresolved);
    assert!(r.input.as_ref().unwrap().block_index.is_some());
    // A stopped output ledger can trap the fee lookup before the output
    // identity is prepared. The already-confirmed input remains fenced.
    assert!(r.output.as_ref().map_or(true, |output| output.block_index.is_none()));
    assert!(r.refund.is_none());
    assert_eq!(
        before - balance(&h, h.ledgers[0], h.user),
        request().dx + 10_000
    );
    h.pic
        .upgrade_canister(
            h.three_pool,
            three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    h.pic.start_canister(h.ledgers[1], None).unwrap();
    let resumed = submit(&h, request()).unwrap();
    assert_eq!(resumed.status, SwapReceiptStatusV1::Completed);
    assert_eq!(resumed.input, r.input, "resume must preserve the original input identity");
    assert!(resumed.output.as_ref().unwrap().block_index.is_some());
    assert_eq!(
        before - balance(&h, h.ledgers[0], h.user),
        request().dx + 10_000,
        "resuming after upgrade must not debit input again",
    );
    let blocked: Result<u128, ThreePoolError> = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, request().dx, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(blocked, Err(ThreePoolError::TransferFailed { .. })));
    assert_eq!(
        before - balance(&h, h.ledgers[0], h.user),
        request().dx + 10_000
    );
}

#[test]
fn too_old_after_ambiguous_no_effect_rotates_only_after_complete_fixed_tip_scan() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    let seeded: () = decode_one(&bytes(
        h.pic.update_call(
            h.three_pool,
            h.user,
            "test_seed_absent_swap_input_v1",
            encode_one(request()).unwrap(),
        ).unwrap(),
    )).unwrap();
    let _ = seeded;

    // The persisted identity is now older than the ledger's transaction
    // window. Its first exact retry returns typed TooOld; that alone does not
    // rotate the identity or release the fence.
    h.pic.advance_time(std::time::Duration::from_secs(30 * 60 * 60));
    let aged = submit(&h, request()).unwrap();
    let aged_input = aged.input.as_ref().unwrap();
    assert_eq!(aged.status, SwapReceiptStatusV1::Unresolved);
    assert_eq!(aged_input.dispatch_count, Some(2));
    assert_eq!(aged_input.too_old_after_ambiguity, Some(true));
    assert!(aged_input.absence_scan.is_none());

    // The bounded scan's fixed tip equals the pre-dispatch baseline here.
    // An empty complete interval is still persisted, checked, and tombstoned
    // before the replacement identity can be sent.
    let scanned: Result<SwapReceiptV1, SwapReceiptErrorV1> = decode_one(&bytes(
        h.pic.update_call(
            h.three_pool,
            h.user,
            "advance_swap_absence_scan_v1",
            encode_args((request().intent_id.clone(), 0u8)).unwrap(),
        ).unwrap(),
    )).unwrap();
    let scanned = scanned.unwrap();
    let replacement = scanned.input.as_ref().unwrap();
    assert_eq!(replacement.status, SwapTransferStatusV1::Unresolved);
    assert_eq!(replacement.ready_to_dispatch, Some(true));
    assert_eq!(replacement.generation, Some(1));
    assert!(replacement.retired_identity_hash.is_some());
    assert!(!replacement.too_old_after_ambiguity.unwrap_or(false));

    let completed = submit(&h, request()).unwrap();
    assert_eq!(completed.status, SwapReceiptStatusV1::Completed);
    assert_eq!(completed.input.as_ref().unwrap().generation, Some(1));
}

#[test]
fn archived_nonmatching_history_is_scanned_and_cursor_survives_upgrade() {
    use icrc_ledger_types::icrc1::transfer::TransferArg;
    use rumi_3pool::icrc3::{GetBlocksArgs, GetBlocksResult};
    use std::time::UNIX_EPOCH;

    let h = deploy_pool_with_archive_cycles(0, 10_000, Some(1_000_000_000_000));
    let _: () = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "test_seed_absent_swap_input_v1",
                encode_one(request()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let baseline = query(&h, h.user)
        .unwrap()
        .input
        .unwrap()
        .history_start
        .unwrap();
    let baseline: u64 = baseline.0.try_into().unwrap();

    // The harness ledger archives after 1,000 blocks. Create an archived run
    // of valid, timestamped ICRC-3 transfers after the seeded receipt baseline;
    // none can match the caller-to-pool receipt tuple.
    let recipient = Principal::self_authenticating(b"archive-absence-recipient");
    let timestamp = h
        .pic
        .get_time()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    for n in 0..1_100u64 {
        let arg = TransferArg {
            from_subaccount: None,
            to: Account {
                owner: recipient,
                subaccount: None,
            },
            amount: Nat::from(1_000_000u64),
            fee: Some(Nat::from(10_000u64)),
            memo: Some(n.to_be_bytes().to_vec().into()),
            created_at_time: Some(timestamp),
        };
        let result: Result<Nat, TransferError> = decode_one(&bytes(
            h.pic
                .update_call(
                    h.ledgers[0],
                    h.user,
                    "icrc1_transfer",
                    encode_one(arg).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        result.expect("archive filler transfer should succeed");
    }

    let query_args = vec![GetBlocksArgs {
        start: Nat::from(baseline),
        length: Nat::from(100u64),
    }];
    let response: GetBlocksResult = decode_one(&bytes(
        h.pic
            .query_call(
                h.ledgers[0],
                h.user,
                "icrc3_get_blocks",
                encode_one(query_args.clone()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(
        !response.archived_blocks.is_empty(),
        "ledger should route this history page to its archive"
    );
    let archive = &response.archived_blocks[0];
    let archived: GetBlocksResult = decode_one(&bytes(
        h.pic
            .query_call(
                archive.callback.canister_id,
                h.user,
                &archive.callback.method,
                encode_one(archive.args.clone()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(
        !archived.blocks.is_empty(),
        "archive response must contain the requested nonempty history"
    );

    h.pic
        .advance_time(std::time::Duration::from_secs(30 * 60 * 60));
    let aged = submit(&h, request()).unwrap();
    assert_eq!(aged.status, SwapReceiptStatusV1::Unresolved);
    assert_eq!(
        aged.input.as_ref().unwrap().too_old_after_ambiguity,
        Some(true)
    );

    // One page must consume the nonempty archive response and persist its
    // cursor. An upgrade between pages must not restart at the old baseline.
    let mut scanned = advance_absence_page(&h);
    let first_scan = scanned
        .input
        .as_ref()
        .unwrap()
        .absence_scan
        .as_ref()
        .unwrap()
        .clone();
    assert!(first_scan.cursor > Nat::from(baseline));
    h.pic
        .upgrade_canister(
            h.three_pool,
            three_pool_wasm(),
            encode_args(()).unwrap(),
            None,
        )
        .unwrap();
    scanned = query(&h, h.user).expect("receipt survives upgrade");
    assert_eq!(
        scanned.input.as_ref().unwrap().absence_scan.as_ref(),
        Some(&first_scan)
    );

    for _ in 0..20 {
        if scanned.input.as_ref().unwrap().ready_to_dispatch == Some(true) {
            break;
        }
        scanned = advance_absence_page(&h);
    }
    let replacement = scanned.input.as_ref().unwrap();
    assert_eq!(replacement.ready_to_dispatch, Some(true));
    assert_eq!(replacement.generation, Some(1));
    assert!(replacement.retired_identity_hash.is_some());
}

fn advance_absence_page(h: &ThreePoolHarness) -> SwapReceiptV1 {
    let result: Result<SwapReceiptV1, SwapReceiptErrorV1> = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "advance_swap_absence_scan_v1",
                encode_args((request().intent_id, 0u8)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    result.expect("complete contiguous ledger/archive page")
}

#[test]
fn ingress_archived_absence_scan_rotates_after_nonempty_final_page() {
    use icrc_ledger_types::icrc1::transfer::TransferArg;
    use rumi_3pool::icrc3::{GetBlocksArgs, GetBlocksResult};
    use rumi_3pool::receipts::{IngressReceiptErrorV1, IngressReceiptV1};
    use std::time::UNIX_EPOCH;

    let h = deploy_pool_with_archive_cycles(0, 10_000, Some(1_000_000_000_000));
    let intent_id = vec![9; 32];
    let _: () = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "test_seed_absent_ingress_pull_v1",
                encode_args((intent_id.clone(), 0u8)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    let baseline = query_ingress(&h, &intent_id)
        .expect("seeded ingress receipt")
        .pulls[0]
        .history_start
        .clone()
        .unwrap();
    let baseline: u64 = baseline.0.try_into().unwrap();

    let recipient = Principal::self_authenticating(b"ingress-archive-absence-recipient");
    let timestamp = h
        .pic
        .get_time()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    for n in 0..1_100u64 {
        let arg = TransferArg {
            from_subaccount: None,
            to: Account {
                owner: recipient,
                subaccount: None,
            },
            amount: Nat::from(1_000_000u64),
            fee: Some(Nat::from(10_000u64)),
            memo: Some(n.to_be_bytes().to_vec().into()),
            created_at_time: Some(timestamp),
        };
        let result: Result<Nat, TransferError> = decode_one(&bytes(
            h.pic
                .update_call(
                    h.ledgers[0],
                    h.user,
                    "icrc1_transfer",
                    encode_one(arg).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        result.expect("archive filler transfer should succeed");
    }

    let response: GetBlocksResult = decode_one(&bytes(
        h.pic
            .query_call(
                h.ledgers[0],
                h.user,
                "icrc3_get_blocks",
                encode_one(vec![GetBlocksArgs {
                    start: Nat::from(baseline),
                    length: Nat::from(100u64),
                }])
                .unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(!response.archived_blocks.is_empty());
    let archive = &response.archived_blocks[0];
    let archived: GetBlocksResult = decode_one(&bytes(
        h.pic
            .query_call(
                archive.callback.canister_id,
                h.user,
                &archive.callback.method,
                encode_one(archive.args.clone()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(!archived.blocks.is_empty());

    let mut receipt = query_ingress(&h, &intent_id).expect("seeded ingress receipt");
    for _ in 0..20 {
        let transfer = &receipt.pulls[0];
        if transfer.ready_to_dispatch == Some(true) {
            break;
        }
        let result: Result<IngressReceiptV1, IngressReceiptErrorV1> = decode_one(&bytes(
            h.pic
                .update_call(
                    h.three_pool,
                    h.user,
                    "advance_ingress_absence_scan_v1",
                    encode_args((intent_id.clone(), 0u8)).unwrap(),
                )
                .unwrap(),
        ))
        .unwrap();
        receipt = result.expect("complete archived ingress page");
    }
    let transfer = &receipt.pulls[0];
    assert_eq!(transfer.ready_to_dispatch, Some(true));
    assert_eq!(transfer.generation, Some(1));
    assert!(transfer.retired_identity_hash.is_some());
}

fn query_ingress(h: &ThreePoolHarness, intent_id: &[u8]) -> Option<IngressReceiptV1> {
    decode_one(&bytes(
        h.pic
            .query_call(
                h.three_pool,
                h.user,
                "get_ingress_receipt_v1",
                encode_one(intent_id.to_vec()).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap()
}

#[test]
fn definitive_output_rejection_records_exact_refund_block_and_fees() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    enable(&h);
    // Test fixture: remove output-ledger liquidity to elicit a real ledger
    // InsufficientFunds response, without modifying the pool implementation.
    let held = balance(&h, h.ledgers[1], h.three_pool);
    let args = TransferArg {
        from_subaccount: None,
        to: Account {
            owner: h.user,
            subaccount: None,
        },
        amount: Nat::from(held - 10_000),
        fee: Some(Nat::from(10_000u64)),
        memo: None,
        created_at_time: None,
    };
    let drained: Result<Nat, TransferError> = decode_one(&bytes(
        h.pic
            .update_call(
                h.ledgers[1],
                h.three_pool,
                "icrc1_transfer",
                encode_one(args).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    drained.unwrap();
    let before = balance(&h, h.ledgers[0], h.user);
    let r = submit(&h, request()).unwrap();
    assert_eq!(r.status, SwapReceiptStatusV1::Refunded);
    assert_eq!(
        r.output.as_ref().unwrap().status,
        SwapTransferStatusV1::Rejected
    );
    let refund = r.refund.as_ref().unwrap();
    assert_eq!(refund.amount, request().dx - 10_000);
    assert_eq!(refund.fee, 10_000);
    assert!(refund.block_index.is_some());
    verify_ledger_transaction(&h, refund);
    assert_eq!(refund.from.owner, h.three_pool);
    assert_eq!(refund.to.owner, h.user);
    assert_eq!(refund.ledger, h.ledgers[0]);
    assert_eq!(before - balance(&h, h.ledgers[0], h.user), 20_000);
    assert_eq!(submit(&h, request()).unwrap(), r);
    assert_eq!(before - balance(&h, h.ledgers[0], h.user), 20_000);
}

#[test]
fn concurrent_identical_requests_share_one_attempt() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    enable(&h);
    let before = balance(&h, h.ledgers[0], h.user);
    let first = h
        .pic
        .submit_call(
            h.three_pool,
            h.user,
            "swap_with_receipt_v1",
            encode_one(request()).unwrap(),
        )
        .unwrap();
    let second = h
        .pic
        .submit_call(
            h.three_pool,
            h.user,
            "swap_with_receipt_v1",
            encode_one(request()).unwrap(),
        )
        .unwrap();
    let a: Result<SwapReceiptV1, SwapReceiptErrorV1> =
        decode_one(&bytes(h.pic.await_call(first).unwrap())).unwrap();
    let b: Result<SwapReceiptV1, SwapReceiptErrorV1> =
        decode_one(&bytes(h.pic.await_call(second).unwrap())).unwrap();
    assert_eq!(a.unwrap().request, b.unwrap().request);
    let final_receipt = query(&h, h.user).unwrap();
    assert_eq!(final_receipt.status, SwapReceiptStatusV1::Completed);
    assert_eq!(
        before - balance(&h, h.ledgers[0], h.user),
        request().dx + 10_000
    );
}

#[test]
fn receipt_client_management_requires_admin_and_receipts_remain_owner_scoped() {
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    let unauthorized: Result<(), SwapReceiptErrorV1> = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "set_swap_receipt_client_v1",
                encode_args((h.user, true)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(unauthorized, Err(SwapReceiptErrorV1::Unauthorized));
    let mut invalid = request();
    invalid.dx = 0;
    assert_eq!(
        submit(&h, invalid).unwrap_err(),
        SwapReceiptErrorV1::InvalidRequest
    );
    assert!(query(&h, h.user).is_none());
    let r = submit(&h, request()).unwrap();
    // Receipt V1 is caller-scoped for wallet callers; the operator list is
    // informational and cannot revoke an owner's idempotent replay access.
    assert_eq!(submit(&h, request()).unwrap(), r);
    assert_eq!(query(&h, h.user), Some(r));
}

#[test]
fn definitive_input_rejection_records_no_debit_and_does_not_replay() {
    use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
    let h = deploy_pool_with_liquidity_fee_and_swaps(0, 10_000);
    enable(&h);
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account {
            owner: h.three_pool,
            subaccount: None,
        },
        amount: Nat::from(0u64),
        expected_allowance: None,
        expires_at: None,
        fee: Some(Nat::from(10_000u64)),
        memo: None,
        created_at_time: None,
    };
    let approved: Result<Nat, ApproveError> = decode_one(&bytes(
        h.pic
            .update_call(
                h.ledgers[0],
                h.user,
                "icrc2_approve",
                encode_one(args).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    approved.unwrap();
    let before = balance(&h, h.ledgers[0], h.user);
    let r = submit(&h, request()).unwrap();
    assert_eq!(r.status, SwapReceiptStatusV1::Failed);
    let input = r.input.as_ref().unwrap();
    assert_eq!(input.status, SwapTransferStatusV1::Rejected);
    assert!(input.block_index.is_none());
    assert!(r.output.is_none());
    assert!(r.refund.is_none());
    assert_eq!(submit(&h, request()).unwrap(), r);
    assert_eq!(balance(&h, h.ledgers[0], h.user), before);
    let legacy: Result<u128, ThreePoolError> = decode_one(&bytes(
        h.pic
            .update_call(
                h.three_pool,
                h.user,
                "swap",
                encode_args((0u8, 1u8, request().dx, 1u128)).unwrap(),
            )
            .unwrap(),
    ))
    .unwrap();
    assert!(matches!(legacy, Err(ThreePoolError::TransferFailed { .. })));
}
