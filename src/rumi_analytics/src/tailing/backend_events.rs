//! Backend event tailing. Routes events to EVT_LIQUIDATIONS and EVT_VAULTS.

use candid::Principal;
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;
use super::{BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

pub async fn run() {
    let backend = state::read_state(|s| s.sources.backend);
    let cursor = cursors::backend_events::get();

    let count = match sources::backend::get_event_count(backend).await {
        Ok(c) => c,
        Err(e) => {
            ic_cdk::println!("[tail_backend] get_event_count failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.backend += 1;
                update_cursor_error(s, cursors::CURSOR_ID_BACKEND_EVENTS, e);
            });
            return;
        }
    };

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_BACKEND_EVENTS, count);
    });

    if count <= cursor { return; }

    let fetch_len = (count - cursor).min(BATCH_SIZE);
    let events = match sources::backend::get_events(backend, cursor, fetch_len).await {
        Ok(e) => e,
        Err(e) => {
            ic_cdk::println!("[tail_backend] get_events failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.backend += 1;
                update_cursor_error(s, cursors::CURSOR_ID_BACKEND_EVENTS, e);
            });
            return;
        }
    };

    let mut processed = 0u64;
    for (i, event) in events.iter().enumerate() {
        let event_id = cursor + i as u64;
        route_backend_event(event_id, event);
        processed += 1;
    }

    if processed > 0 {
        cursors::backend_events::set(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_BACKEND_EVENTS, ic_cdk::api::time());
        });
    }
}

fn route_backend_event(event_id: u64, event: &sources::backend::BackendEvent) {
    use sources::backend::BackendEvent::*;

    match event {
        LiquidateVault { vault_id, timestamp, .. } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(),
                collateral_amount: 0,
                debt_amount: 0,
                liquidation_kind: LiquidationKind::Full,
            });
        }
        PartialLiquidateVault { vault_id, liquidator_payment, icp_to_liquidator, timestamp, .. } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(),
                collateral_amount: *icp_to_liquidator,
                debt_amount: *liquidator_payment,
                liquidation_kind: LiquidationKind::Partial,
            });
        }
        RedistributeVault { vault_id, timestamp } => {
            evt_liquidations::push(AnalyticsLiquidationEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                collateral_type: Principal::anonymous(),
                collateral_amount: 0,
                debt_amount: 0,
                liquidation_kind: LiquidationKind::Redistribution,
            });
        }
        OpenVault { vault, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: vault.vault_id,
                owner: vault.owner,
                event_kind: VaultEventKind::Opened,
                collateral_type: vault.collateral_type,
                amount: vault.collateral_amount,
            });
        }
        BorrowFromVault { vault_id, borrowed_amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::Borrowed,
                collateral_type: Principal::anonymous(),
                amount: *borrowed_amount,
            });
        }
        RepayToVault { vault_id, repayed_amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::Repaid,
                collateral_type: Principal::anonymous(),
                amount: *repayed_amount,
            });
        }
        CollateralWithdrawn { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::CollateralWithdrawn,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        PartialCollateralWithdrawn { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::PartialCollateralWithdrawn,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        WithdrawAndCloseVault { vault_id, amount, caller, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: caller.unwrap_or(Principal::anonymous()),
                event_kind: VaultEventKind::WithdrawAndClose,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        VaultWithdrawnAndClosed { vault_id, timestamp, caller, amount } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: *timestamp,
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: *caller,
                event_kind: VaultEventKind::Closed,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        DustForgiven { vault_id, amount, timestamp } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: *vault_id,
                owner: Principal::anonymous(),
                event_kind: VaultEventKind::DustForgiven,
                collateral_type: Principal::anonymous(),
                amount: *amount,
            });
        }
        RedemptionOnVaults { icusd_amount, owner, collateral_type, timestamp, .. } => {
            evt_vaults::push(AnalyticsVaultEvent {
                timestamp_ns: timestamp.unwrap_or(0),
                source_event_id: event_id,
                vault_id: 0,
                owner: *owner,
                event_kind: VaultEventKind::Redeemed,
                collateral_type: collateral_type.unwrap_or(Principal::anonymous()),
                amount: *icusd_amount,
            });
        }
        // All other variants (admin/config events) are not routed
        _ => {}
    }
}
