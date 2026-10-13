//! Developer-only, read-only inventory of retained 3USD reserve reconciliation state.
use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};

use crate::state::{self, ThreeUsdReserveIngressKey};

pub const MAX_THREE_USD_RESERVE_INVENTORY_PAGE: u16 = 100;

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeUsdReserveInventoryCursor {
    AfterRefund(u128),
    AfterIngress(ThreeUsdReserveIngressKey),
    AfterAbsorbProof(crate::icrc3_proof::SpProofLedger, u64),
    AfterCollateralPayout(ThreeUsdReserveIngressKey),
    AfterPayoutCandidateScan(ThreeUsdReserveIngressKey),
    AfterPayoutOperation(u128),
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeUsdReserveInventoryEntry {
    Refund {
        op_nonce: u128,
        row: state::PendingThreeUsdRefund,
    },
    Ingress {
        key: ThreeUsdReserveIngressKey,
        row: state::ThreeUsdReserveIngressJournal,
    },
    AbsorbProof {
        ledger: crate::icrc3_proof::SpProofLedger,
        block_index: u64,
        row: state::StoredThreeUsdReserveAbsorbResult,
    },
    CollateralPayout {
        key: ThreeUsdReserveIngressKey,
        row: state::ThreeUsdReserveCollateralPayout,
    },
    PayoutCandidateScan {
        key: ThreeUsdReserveIngressKey,
        row: state::ThreeUsdReservePayoutCandidateScan,
    },
    PayoutOperation {
        operation_id: u128,
        key: ThreeUsdReserveIngressKey,
    },
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeUsdReserveInventoryContext {
    pub ingress_enabled: bool,
    pub v2_client_ready: bool,
    /// The ledger configured for the 3USD reserve route (stored in the legacy
    /// `three_pool_canister` configuration slot).
    pub three_usd_ledger: Option<Principal>,
    pub stability_pool_canister: Option<Principal>,
    pub v1_cutover: state::ThreeUsdReserveV1Cutover,
    pub protocol_3usd_reserves_e8s: u64,
    pub refund_rows: u64,
    pub ingress_rows: u64,
    pub proof_result_rows: u64,
    pub payout_rows: u64,
    pub payout_candidate_scan_rows: u64,
    pub payout_operation_rows: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeUsdReserveInventoryPage {
    pub entries: Vec<ThreeUsdReserveInventoryEntry>,
    pub next_cursor: Option<ThreeUsdReserveInventoryCursor>,
    pub context: ThreeUsdReserveInventoryContext,
}

fn tail<'a, K: Ord, V>(
    map: &'a BTreeMap<K, V>,
    after: Option<&'a K>,
) -> Box<dyn Iterator<Item = (&'a K, &'a V)> + 'a> {
    match after {
        Some(key) => Box::new(map.range((Excluded(key), Unbounded))),
        None => Box::new(map.iter()),
    }
}

fn cursor_for(entry: &ThreeUsdReserveInventoryEntry) -> ThreeUsdReserveInventoryCursor {
    match entry {
        ThreeUsdReserveInventoryEntry::Refund { op_nonce, .. } => {
            ThreeUsdReserveInventoryCursor::AfterRefund(*op_nonce)
        }
        ThreeUsdReserveInventoryEntry::Ingress { key, .. } => {
            ThreeUsdReserveInventoryCursor::AfterIngress(key.clone())
        }
        ThreeUsdReserveInventoryEntry::AbsorbProof {
            ledger,
            block_index,
            ..
        } => ThreeUsdReserveInventoryCursor::AfterAbsorbProof(*ledger, *block_index),
        ThreeUsdReserveInventoryEntry::CollateralPayout { key, .. } => {
            ThreeUsdReserveInventoryCursor::AfterCollateralPayout(key.clone())
        }
        ThreeUsdReserveInventoryEntry::PayoutCandidateScan { key, .. } => {
            ThreeUsdReserveInventoryCursor::AfterPayoutCandidateScan(key.clone())
        }
        ThreeUsdReserveInventoryEntry::PayoutOperation { operation_id, .. } => {
            ThreeUsdReserveInventoryCursor::AfterPayoutOperation(*operation_id)
        }
    }
}

fn cursor_stage(cursor: Option<&ThreeUsdReserveInventoryCursor>) -> usize {
    match cursor {
        None => 0,
        Some(ThreeUsdReserveInventoryCursor::AfterRefund(_)) => 0,
        Some(ThreeUsdReserveInventoryCursor::AfterIngress(_)) => 1,
        Some(ThreeUsdReserveInventoryCursor::AfterAbsorbProof(..)) => 2,
        Some(ThreeUsdReserveInventoryCursor::AfterCollateralPayout(_)) => 3,
        Some(ThreeUsdReserveInventoryCursor::AfterPayoutCandidateScan(_)) => 4,
        Some(ThreeUsdReserveInventoryCursor::AfterPayoutOperation(_)) => 5,
    }
}

/// Return a keyset page. Resolved refunds and terminal ingress/payout rows are
/// deliberately included: these are retained source records, not an active-only view.
pub fn page(
    caller: Principal,
    after: Option<ThreeUsdReserveInventoryCursor>,
    limit: u16,
) -> Result<ThreeUsdReserveInventoryPage, String> {
    if !authorized_developer(caller, state::read_state(|s| s.developer_principal)) {
        return Err("caller is not the configured developer".into());
    }
    if limit == 0 || limit > MAX_THREE_USD_RESERVE_INVENTORY_PAGE {
        return Err(format!(
            "limit must be between 1 and {MAX_THREE_USD_RESERVE_INVENTORY_PAGE}"
        ));
    }
    Ok(state::read_state(|s| {
        let context = ThreeUsdReserveInventoryContext {
            ingress_enabled: s.three_usd_reserve_ingress_enabled,
            v2_client_ready: s.three_usd_reserve_v2_client_ready,
            three_usd_ledger: s.three_pool_canister,
            stability_pool_canister: s.stability_pool_canister,
            v1_cutover: s.three_usd_reserve_v1_cutover.clone(),
            protocol_3usd_reserves_e8s: s.protocol_3usd_reserves,
            refund_rows: s.pending_3usd_refunds.len() as u64,
            ingress_rows: s.three_usd_reserve_ingress_journals.len() as u64,
            proof_result_rows: s.sp_three_usd_reserve_absorb_results_by_proof.len() as u64,
            payout_rows: s.three_usd_reserve_collateral_payouts.len() as u64,
            payout_candidate_scan_rows: s.three_usd_reserve_payout_candidate_scans.len() as u64,
            payout_operation_rows: s.three_usd_reserve_payout_operation_keys.len() as u64,
        };
        let target = usize::from(limit) + 1;
        let stage = cursor_stage(after.as_ref());
        let mut entries = Vec::with_capacity(target);

        macro_rules! extend_map {
            ($rank:expr, $map:expr, $key:expr, $variant:expr, $after_key:expr) => {
                if stage <= $rank && entries.len() < target {
                    let start = if stage == $rank { $after_key } else { None };
                    for (key, value) in tail($map, start).take(target - entries.len()) {
                        entries.push(($variant)(key.clone(), value.clone()));
                    }
                }
            };
        }
        extend_map!(
            0,
            &s.pending_3usd_refunds,
            (),
            |key, row| ThreeUsdReserveInventoryEntry::Refund { op_nonce: key, row },
            match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterRefund(key)) => Some(key),
                _ => None,
            }
        );
        extend_map!(
            1,
            &s.three_usd_reserve_ingress_journals,
            (),
            |key, row| ThreeUsdReserveInventoryEntry::Ingress { key, row },
            match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterIngress(key)) => Some(key),
                _ => None,
            }
        );

        if stage <= 2 && entries.len() < target {
            let start = match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterAbsorbProof(ledger, block)) => {
                    Some((*ledger, *block))
                }
                _ => None,
            };
            let iter: Box<
                dyn Iterator<
                    Item = (
                        &(crate::icrc3_proof::SpProofLedger, u64),
                        &state::StoredThreeUsdReserveAbsorbResult,
                    ),
                >,
            > = match start {
                Some(key) => Box::new(
                    s.sp_three_usd_reserve_absorb_results_by_proof
                        .range((Excluded(key), Unbounded)),
                ),
                None => Box::new(s.sp_three_usd_reserve_absorb_results_by_proof.iter()),
            };
            for ((ledger, block_index), row) in iter.take(target - entries.len()) {
                entries.push(ThreeUsdReserveInventoryEntry::AbsorbProof {
                    ledger: *ledger,
                    block_index: *block_index,
                    row: row.clone(),
                });
            }
        }

        extend_map!(
            3,
            &s.three_usd_reserve_collateral_payouts,
            (),
            |key, row| ThreeUsdReserveInventoryEntry::CollateralPayout { key, row },
            match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterCollateralPayout(key)) => Some(key),
                _ => None,
            }
        );
        extend_map!(
            4,
            &s.three_usd_reserve_payout_candidate_scans,
            (),
            |key, row| ThreeUsdReserveInventoryEntry::PayoutCandidateScan { key, row },
            match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterPayoutCandidateScan(key)) => Some(key),
                _ => None,
            }
        );
        extend_map!(
            5,
            &s.three_usd_reserve_payout_operation_keys,
            (),
            |key, row| ThreeUsdReserveInventoryEntry::PayoutOperation {
                operation_id: key,
                key: row
            },
            match &after {
                Some(ThreeUsdReserveInventoryCursor::AfterPayoutOperation(key)) => Some(key),
                _ => None,
            }
        );

        let has_more = entries.len() > usize::from(limit);
        entries.truncate(usize::from(limit));
        let next_cursor = if has_more {
            entries.last().map(cursor_for)
        } else {
            None
        };
        ThreeUsdReserveInventoryPage {
            entries,
            next_cursor,
            context,
        }
    }))
}

pub fn authorized_developer(caller: Principal, developer: Principal) -> bool {
    caller != Principal::anonymous() && caller == developer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{PendingThreeUsdRefund, ThreeUsdRefundResolution, ThreeUsdRefundSource};

    fn refund(resolved: bool) -> PendingThreeUsdRefund {
        PendingThreeUsdRefund {
            stability_pool: Principal::from_slice(&[1]),
            ledger: Principal::from_slice(&[2]),
            amount_e8s: 5,
            vault_id: 7,
            retry_count: 0,
            op_nonce: 0,
            source: ThreeUsdRefundSource::DefaultAccount,
            parent_absorb_id: Some(9),
            dispatch_amount_e8s: None,
            dispatch_fee_e8s: None,
            dispatch_submitted: false,
            legacy_dispatch_retryable: false,
            dispatch_block_index: None,
            resolution: resolved.then_some(ThreeUsdRefundResolution::ReceiptVerified {
                block_index: 11,
                fee_e8s: 0,
                reconciled_at_ns: 12,
            }),
        }
    }

    #[test]
    fn cursor_transitions_are_discriminated_by_source_map() {
        let key = ThreeUsdReserveIngressKey {
            stability_pool: Principal::from_slice(&[1]),
            vault_id: 4,
            absorb_id: 5,
        };
        let account = icrc_ledger_types::icrc1::account::Account {
            owner: Principal::from_slice(&[2]),
            subaccount: None,
        };
        let absorb_result = crate::StabilityPoolLiquidationResult {
            success: true,
            vault_id: 4,
            liquidated_debt: 3,
            collateral_received: 2,
            collateral_type: "ICP".into(),
            block_index: 7,
            fee: 0,
            collateral_price_e8s: 8,
        };
        let entries = [
            ThreeUsdReserveInventoryEntry::Refund {
                op_nonce: 8,
                row: refund(false),
            },
            ThreeUsdReserveInventoryEntry::Ingress {
                key: key.clone(),
                row: state::ThreeUsdReserveIngressJournal {
                    request: state::ThreeUsdReserveIngressRequest {
                        icusd_debt_covered_e8s: 3,
                        three_usd_amount_e8s: 4,
                        ledger: Principal::from_slice(&[3]),
                    },
                    phase: state::ThreeUsdReserveIngressPhase::AdmissionPending,
                    ingress_proof_verified: false,
                    refund: None,
                    protocol_refund_fee_reserve_e8s: 0,
                    non_inclusion_scan: None,
                },
            },
            ThreeUsdReserveInventoryEntry::AbsorbProof {
                ledger: crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault,
                block_index: 7,
                row: state::StoredThreeUsdReserveAbsorbResult {
                    caller: Principal::from_slice(&[4]),
                    vault_id: 4,
                    absorb_id: 5,
                    icusd_debt_covered_e8s: 3,
                    three_usd_amount_e8s: 4,
                    ledger: Principal::from_slice(&[3]),
                    proof: crate::icrc3_proof::SpWritedownProof {
                        block_index: 7,
                        ledger_kind: crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault,
                        vault_id_memo: 4,
                    },
                    result: absorb_result,
                },
            },
            ThreeUsdReserveInventoryEntry::CollateralPayout {
                key: key.clone(),
                row: state::ThreeUsdReserveCollateralPayout {
                    operation_id: 9,
                    op_nonce: 10,
                    collateral_type: Principal::from_slice(&[5]),
                    ledger: Principal::from_slice(&[6]),
                    source: account.clone(),
                    destination: account,
                    gross_e8s: 2,
                    net_e8s: 1,
                    expected_fee_e8s: 1,
                    memo: [0; 16],
                    created_at_time_ns: 11,
                    fee_arg_e8s: None,
                    candidate_block_index: None,
                    observed_fee_e8s: None,
                    rearmed_attempts: Vec::new(),
                },
            },
            ThreeUsdReserveInventoryEntry::PayoutCandidateScan {
                key: key.clone(),
                row: state::ThreeUsdReservePayoutCandidateScan {
                    operation_id: 9,
                    attempt_nonce: 10,
                    start_index: 0,
                    snapshot_log_length: 1,
                    snapshot_count: 1,
                    next_index: 0,
                    candidate_block_index: None,
                    multiple_candidates: false,
                },
            },
            ThreeUsdReserveInventoryEntry::PayoutOperation {
                operation_id: 9,
                key,
            },
        ];
        assert_eq!(cursor_stage(None), 0);
        for (expected_stage, entry) in entries.iter().enumerate() {
            let cursor = cursor_for(entry);
            assert_eq!(cursor_stage(Some(&cursor)), expected_stage);
            if expected_stage + 1 < entries.len() {
                let next_cursor = cursor_for(&entries[expected_stage + 1]);
                assert_eq!(
                    cursor_stage(Some(&cursor)) + 1,
                    cursor_stage(Some(&next_cursor))
                );
            }
        }
    }

    #[test]
    fn bounded_raw_refund_progress_includes_resolved_rows() {
        let mut rows = BTreeMap::new();
        rows.insert(3, refund(true));
        rows.insert(5, refund(false));
        let first: Vec<_> = tail(&rows, None).take(1).collect();
        assert_eq!(first.len(), 1);
        assert_eq!(*first[0].0, 3);
        assert!(
            first[0].1.resolution.is_some(),
            "resolved tombstone stays visible"
        );
        let second: Vec<_> = tail(&rows, Some(&3)).take(1).collect();
        assert_eq!(second.len(), 1);
        assert_eq!(*second[0].0, 5);
        assert!(second[0].1.resolution.is_none());
    }

    #[test]
    fn page_cursor_crosses_refund_map_into_later_payout_map_without_duplicates() {
        let developer = Principal::from_slice(&[42]);
        let mut state = state::State::default();
        state.developer_principal = developer;
        let mut first_refund = refund(true);
        first_refund.op_nonce = 1;
        state.pending_3usd_refunds.insert(1, first_refund);
        let mut second_refund = refund(false);
        second_refund.op_nonce = 2;
        state.pending_3usd_refunds.insert(2, second_refund);
        state.three_usd_reserve_payout_operation_keys.insert(
            3,
            ThreeUsdReserveIngressKey {
                stability_pool: Principal::from_slice(&[1]),
                vault_id: 4,
                absorb_id: 5,
            },
        );
        state::replace_state(state);

        let first = page(developer, None, 1).unwrap();
        assert!(matches!(
            first.entries.as_slice(),
            [ThreeUsdReserveInventoryEntry::Refund { op_nonce: 1, .. }]
        ));
        assert_eq!(
            first.next_cursor,
            Some(ThreeUsdReserveInventoryCursor::AfterRefund(1))
        );

        let second = page(developer, first.next_cursor, 1).unwrap();
        assert!(matches!(
            second.entries.as_slice(),
            [ThreeUsdReserveInventoryEntry::Refund { op_nonce: 2, .. }]
        ));
        assert_eq!(
            second.next_cursor,
            Some(ThreeUsdReserveInventoryCursor::AfterRefund(2))
        );

        let third = page(developer, second.next_cursor, 1).unwrap();
        assert!(matches!(
            third.entries.as_slice(),
            [ThreeUsdReserveInventoryEntry::PayoutOperation {
                operation_id: 3,
                ..
            }]
        ));
        assert_eq!(third.next_cursor, None);
    }

    #[test]
    fn developer_inventory_rejects_anonymous_and_wrong_callers() {
        let developer = Principal::from_slice(&[42]);
        assert!(authorized_developer(developer, developer));
        assert!(!authorized_developer(Principal::anonymous(), developer));
        assert!(!authorized_developer(
            Principal::from_slice(&[43]),
            developer
        ));
    }
}
