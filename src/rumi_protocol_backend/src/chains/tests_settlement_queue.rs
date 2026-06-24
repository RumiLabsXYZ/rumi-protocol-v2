use super::settlement_queue::{
    SettlementOp, SettlementOpKind, SettlementOpStatus, SettlementQueueError,
    SettlementQueueV1,
};
use candid::{Decode, Encode};

#[test]
fn empty_queue_has_zero_head_and_tail() {
    let q = SettlementQueueV1::default();
    assert_eq!(q.head, 0);
    assert_eq!(q.tail, 0);
    assert!(q.pending.is_empty());
}

#[test]
fn enqueue_assigns_increasing_op_ids() {
    let mut q = SettlementQueueV1::default();
    let op_a = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xabc".to_string(), amount_e8s: 100, vault_id: 1 },
        "key-a".to_string(),
        0,
    );
    let op_b = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xdef".to_string(), amount_e8s: 200, vault_id: 2 },
        "key-b".to_string(),
        0,
    );
    let id_a = q.enqueue(op_a).expect("first enqueue");
    let id_b = q.enqueue(op_b).expect("second enqueue");
    assert_eq!(id_a, 0);
    assert_eq!(id_b, 1);
    assert_eq!(q.pending.len(), 2);
    assert_eq!(q.tail, 2);
}

#[test]
fn has_active_op_tracks_non_terminal_ops_only() {
    let mut q = SettlementQueueV1::default();
    // Empty queue: no active op (so the observer skips the hot-wallet refresh).
    assert!(!q.has_active_op(), "empty queue has no active op");

    let id = q
        .enqueue(SettlementOp::new(
            SettlementOpKind::Mint { recipient: "0xabc".to_string(), amount_e8s: 100, vault_id: 1 },
            "key-a".to_string(),
            0,
        ))
        .expect("enqueue");
    // Queued → active.
    assert!(q.has_active_op(), "Queued op is active");

    // Inflight → still active.
    q.pending.get_mut(&id).unwrap().mark_inflight(1);
    assert!(q.has_active_op(), "Inflight op is active");

    // Succeeded (terminal) → no longer active.
    q.pending.get_mut(&id).unwrap().mark_succeeded("0xtx".to_string(), 2);
    assert!(!q.has_active_op(), "Succeeded op is terminal, not active");

    // Failed (terminal) → not active either.
    q.pending.get_mut(&id).unwrap().mark_failed("nope".to_string(), 3);
    assert!(!q.has_active_op(), "Failed op is terminal, not active");
}

#[test]
fn enqueue_rejects_duplicate_idempotency_key() {
    let mut q = SettlementQueueV1::default();
    let op_a = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xa".to_string(), amount_e8s: 1, vault_id: 1 },
        "duplicate".to_string(),
        0,
    );
    let op_b = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xb".to_string(), amount_e8s: 2, vault_id: 2 },
        "duplicate".to_string(),
        0,
    );
    q.enqueue(op_a).expect("first");
    let err = q.enqueue(op_b).expect_err("second must reject");
    assert!(matches!(err, SettlementQueueError::DuplicateIdempotencyKey(_)));
}

#[test]
fn round_trip_via_candid() {
    let mut q = SettlementQueueV1::default();
    let op = SettlementOp::new(
        SettlementOpKind::NativeWithdrawal { recipient: "0xrecip".to_string(), amount_e18: 42, vault_id: 3 },
        "k1".to_string(),
        0,
    );
    q.enqueue(op).expect("enqueue");
    let bytes = Encode!(&q).expect("encode");
    let back: SettlementQueueV1 = Decode!(&bytes, SettlementQueueV1).expect("decode");
    assert_eq!(back.pending.len(), 1);
    assert_eq!(back.tail, 1);
}

#[test]
fn chain_collateral_payout_round_trips_and_is_not_a_mint() {
    let mut q = SettlementQueueV1::default();
    let op = SettlementOp::new(
        SettlementOpKind::ChainCollateralPayout {
            recipient: "0xclaimant".to_string(),
            amount_e18: 42,
            vault_id: 7,
            claimant: candid::Principal::anonymous(),
        },
        "claim:7:2vxsx-fae:0xclaimant".to_string(),
        0,
    );
    q.enqueue(op).expect("enqueue");
    assert!(!q.has_active_mint_op(), "claim payout does not change icUSD supply");

    let bytes = Encode!(&q).expect("encode");
    let back: SettlementQueueV1 = Decode!(&bytes, SettlementQueueV1).expect("decode");
    let op = back.pending.get(&0).expect("op survived");
    match &op.kind {
        SettlementOpKind::ChainCollateralPayout { recipient, amount_e18, vault_id, claimant } => {
            assert_eq!(recipient, "0xclaimant");
            assert_eq!(*amount_e18, 42);
            assert_eq!(*vault_id, 7);
            assert_eq!(*claimant, candid::Principal::anonymous());
        }
        other => panic!("unexpected op kind: {other:?}"),
    }
}

#[test]
fn op_status_transitions_only_to_terminal_states() {
    let mut op = SettlementOp::new(
        SettlementOpKind::Mint { recipient: "0xa".to_string(), amount_e8s: 1, vault_id: 1 },
        "k".to_string(),
        0,
    );
    assert!(matches!(op.status, SettlementOpStatus::Queued));
    op.mark_inflight(1_700_000_000_000_000_000);
    assert!(matches!(op.status, SettlementOpStatus::Inflight { .. }));
    op.mark_succeeded("0xdeadbeef".to_string(), 1_700_000_000_001_000_000);
    assert!(matches!(op.status, SettlementOpStatus::Succeeded { .. }));
}

#[test]
fn has_active_mint_op_true_only_for_live_mint() {
    let mut q = SettlementQueueV1::default();
    assert!(!q.has_active_mint_op(), "empty queue has no active mint");

    // A live (Queued) NativeWithdrawal must NOT count, it does not change supply.
    q.enqueue(SettlementOp::new(
        SettlementOpKind::NativeWithdrawal { recipient: "0xabc".into(), amount_e18: 1, vault_id: 1 },
        "w1".into(),
        0,
    ))
    .unwrap();
    assert!(!q.has_active_mint_op(), "withdrawal-only queue has no active mint");

    // A Queued Mint counts.
    let mint_id = q
        .enqueue(SettlementOp::new(
            SettlementOpKind::Mint { recipient: "0xabc".into(), amount_e8s: 100, vault_id: 2 },
            "m1".into(),
            0,
        ))
        .unwrap();
    assert!(q.has_active_mint_op(), "queued mint counts");

    // An Inflight Mint counts.
    q.pending.get_mut(&mint_id).unwrap().mark_inflight(1);
    assert!(q.has_active_mint_op(), "inflight mint counts");

    // A terminal (Succeeded) Mint does NOT count.
    q.pending.get_mut(&mint_id).unwrap().mark_succeeded("0xhash".into(), 2);
    assert!(!q.has_active_mint_op(), "succeeded mint does not count");
}

// ─── Increment 3: LiquidationSwap is now actionable (select_next_op no longer skips) ───
#[test]
fn liquidation_swap_op_is_selectable_in_inc3() {
    use super::evm::settlement::select_next_op;
    let mut q = SettlementQueueV1::default();
    // The lowest-op_id Queued op is selected for Submit. A LiquidationSwap enqueued
    // first is now picked (Inc 2 skipped it; Inc 3 routes it to the swap submit path).
    q.enqueue(SettlementOp::new(
        SettlementOpKind::LiquidationSwap {
            vault_id: 7,
            collateral_in_native: 1,
            min_usdc_out_native: 0,
            debt_to_clear_e8s: 1,
            router: "0xr".into(),
            pair: "0xp".into(),
            path: vec!["0xa".into(), "0xb".into()],
            reserve_recipient: "0xres".into(),
            deadline_secs: 180,
        },
        "liq-71-7-1".into(),
        1,
    ))
    .unwrap();
    let (id, action) = select_next_op(&q).expect("an actionable op");
    let selected = q.pending.get(&id).unwrap();
    assert!(
        matches!(selected.kind, SettlementOpKind::LiquidationSwap { .. }),
        "swap is now selectable in Inc 3"
    );
    assert!(matches!(action, super::evm::settlement::OpAction::Submit));
}
