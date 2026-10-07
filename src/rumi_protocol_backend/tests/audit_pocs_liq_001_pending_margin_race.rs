//! LIQ-001 regression tests for durable, operation-keyed liquidation payouts.

use candid::Principal;
use rumi_protocol_backend::numeric::ICP;
use rumi_protocol_backend::state::{PendingMarginTransfer, State};
use rumi_protocol_backend::InitArg;
use std::collections::BTreeMap;

fn owner(seed: u8) -> Principal {
    Principal::from_slice(&[seed])
}

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

fn pending(vault_id: u64, who: Principal, margin_e8s: u64, nonce: u128) -> PendingMarginTransfer {
    PendingMarginTransfer {
        vault_id,
        owner: who,
        margin: ICP::new(margin_e8s),
        collateral_type: Principal::anonymous(),
        retry_count: 0,
        op_nonce: nonce,
        ledger: Some(owner(9)),
        transfer_amount_raw: Some(margin_e8s.saturating_sub(10_000)),
        redemption_transfer: None,
        held_for_manual_retry: false,
        reconciliation_required: false,
        min_net_collateral_raw: None,
    }
}

#[test]
fn same_vault_and_same_recipient_keep_distinct_payout_operations() {
    let mut state = fresh_state();
    let recipient = owner(1);
    state
        .pending_margin_transfers
        .insert(101, pending(42, recipient, 100_000_000, 101));
    state
        .pending_margin_transfers
        .insert(102, pending(42, recipient, 50_000_000, 102));

    assert_eq!(state.pending_margin_transfers.len(), 2);
    assert_eq!(state.pending_margin_transfers[&101].owner, recipient);
    assert_eq!(
        state.pending_margin_transfers[&102].margin,
        ICP::new(50_000_000)
    );
}

#[test]
fn typed_queue_maps_may_reuse_migrated_id_without_aliasing() {
    let mut state = fresh_state();
    // Legacy synthetic IDs are allocated per stable field. Queue kind is part of
    // the typed public identity, so an old margin and excess row remain distinct.
    state
        .pending_margin_transfers
        .insert(u128::MAX, pending(7, owner(1), 80_000_000, 0));
    state
        .pending_excess_transfers
        .insert(u128::MAX, pending(7, owner(2), 5_000_000, 0));
    assert_eq!(state.pending_margin_transfers.len(), 1);
    assert_eq!(state.pending_excess_transfers.len(), 1);
    assert_ne!(
        state.pending_margin_transfers[&u128::MAX].owner,
        state.pending_excess_transfers[&u128::MAX].owner
    );
}

#[derive(serde::Serialize)]
struct LegacyPendingTransfer {
    owner: Principal,
    margin: ICP,
    collateral_type: Principal,
    retry_count: u8,
    op_nonce: u128,
    min_net_collateral_raw: Option<u64>,
}

fn legacy(who: Principal, amount: u64, nonce: u128) -> LegacyPendingTransfer {
    LegacyPendingTransfer {
        owner: who,
        margin: ICP::new(amount),
        collateral_type: Principal::anonymous(),
        retry_count: 2,
        op_nonce: nonce,
        min_net_collateral_raw: None,
    }
}

#[derive(serde::Serialize)]
struct LegacyPendingMaps {
    // Wave-4 stable key shape and pre-Wave-4 vault-only shape.
    pending_margin_transfers: BTreeMap<(u64, Principal), LegacyPendingTransfer>,
    pending_excess_transfers: BTreeMap<u64, LegacyPendingTransfer>,
}

#[test]
fn legacy_snapshot_preserves_rows_and_holds_unpinned_identities() {
    let mut margin = BTreeMap::new();
    margin.insert((11, owner(1)), legacy(owner(1), 100_000_000, 0));
    margin.insert((11, owner(2)), legacy(owner(2), 50_000_000, 0));
    let mut excess = BTreeMap::new();
    excess.insert(11, legacy(owner(3), 80_000_000, 0));
    let old = LegacyPendingMaps {
        pending_margin_transfers: margin,
        pending_excess_transfers: excess,
    };
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&old, &mut bytes).unwrap();

    let restored: State =
        ciborium::de::from_reader(bytes.as_slice()).expect("decode legacy snapshot");
    assert_eq!(restored.pending_margin_transfers.len(), 2);
    assert_eq!(restored.pending_excess_transfers.len(), 1);
    for payout in restored
        .pending_margin_transfers
        .values()
        .chain(restored.pending_excess_transfers.values())
    {
        assert!(payout.held_for_manual_retry);
        assert!(payout.reconciliation_required);
        assert_eq!(payout.ledger, None);
        assert_eq!(payout.transfer_amount_raw, None);
    }
    assert!(restored
        .pending_margin_transfers
        .values()
        .any(|p| p.vault_id == 11 && p.owner == owner(1)));
    assert!(restored
        .pending_margin_transfers
        .values()
        .any(|p| p.vault_id == 11 && p.owner == owner(2)));
    assert!(restored
        .pending_excess_transfers
        .values()
        .any(|p| p.vault_id == 11 && p.owner == owner(3)));
}

#[test]
fn new_operation_keyed_snapshot_round_trips_exact_pinned_args_and_hold_state() {
    let mut state = fresh_state();
    let operation_id = (1u128 << 64) | 501;
    let mut payout = pending(5, owner(1), 100_000_000, operation_id);
    payout.held_for_manual_retry = true;
    state.pending_margin_transfers.insert(operation_id, payout);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&state, &mut bytes).unwrap();
    let restored: State = ciborium::de::from_reader(bytes.as_slice()).expect("decode new snapshot");
    assert_eq!(restored.pending_margin_transfers[&operation_id], payout);
}

#[test]
fn low_integer_key_with_matching_pins_is_still_held_as_legacy_ambiguous() {
    let operation_id = 11u128;
    let mut state = fresh_state();
    state.pending_margin_transfers.insert(
        operation_id,
        pending(11, owner(1), 100_000_000, operation_id),
    );
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&state, &mut bytes).unwrap();

    let restored: State = ciborium::de::from_reader(bytes.as_slice()).expect("decode low key");
    let payout = &restored.pending_margin_transfers[&operation_id];
    assert_eq!(payout.vault_id, 11);
    assert!(payout.held_for_manual_retry);
    assert!(payout.reconciliation_required);
    assert_eq!(payout.ledger, Some(owner(9)));
    assert_eq!(payout.transfer_amount_raw, Some(99_990_000));
}
