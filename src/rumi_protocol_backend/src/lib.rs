use icrc_ledger_types::icrc1::transfer::TransferError;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use serde::Serialize;

use crate::guard::GuardError;
use crate::logs::{DEBUG, INFO};
use crate::numeric::{Ratio, UsdIcp, ICP, ICUSD};
use crate::state::{mutate_state, read_state, Mode};
use crate::vault::Vault;
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use num_traits::ToPrimitive;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Maximum number of automatic retries before a failed obligation is held for
/// manual recovery. At 5-second intervals, 60 retries = 5 minutes of attempts.
pub const MAX_PENDING_RETRIES: u8 = 60;
/// Bound payout work per timer tick so durable held receipts cannot turn the
/// drain into an unbounded canister message.
const MAX_PENDING_PAYOUTS_PER_TICK: usize = 100;

fn pending_refund_is_automatically_retryable(retry_count: u8) -> bool {
    retry_count < MAX_PENDING_RETRIES
}

fn pending_3usd_refund_is_automatically_retryable(retry_count: u8) -> bool {
    retry_count < MAX_PENDING_RETRIES
}

fn pending_3usd_refund_can_be_retried(
    state: &crate::state::State,
    refund: &crate::state::PendingThreeUsdRefund,
) -> bool {
    // Capped rows remain durable but leave the timer until the developer
    // explicitly rearms an eligible, known-unsent legacy row. This bounds
    // repeated fee/balance queries when the protocol reserve is underfunded.
    if !pending_3usd_refund_is_automatically_retryable(refund.retry_count)
        || refund.resolution.is_some()
        || (refund.source == crate::state::ThreeUsdRefundSource::DefaultAccount
            && refund.parent_absorb_id.is_none())
    {
        return false;
    }
    if refund.source == crate::state::ThreeUsdRefundSource::LegacyHashedReserve
        && (!refund.legacy_dispatch_retryable
            || refund.dispatch_submitted
            || crate::management::hold_legacy_three_usd_refund_for_cutover(state, refund.ledger))
    {
        return false;
    }
    true
}

fn pin_legacy_three_usd_refund_for_dispatch(
    state: &mut crate::state::State,
    nonce: u128,
    expected: crate::state::PendingThreeUsdRefund,
    amount_e8s: u64,
    fee_e8s: u64,
) -> bool {
    // Check against immutable state before taking the mutable map entry, since
    // the cutover predicate reads the full state.
    if state.pending_3usd_refunds.get(&nonce) != Some(&expected)
        || !pending_3usd_refund_can_be_retried(state, &expected)
    {
        return false;
    }
    let Some(current) = state.pending_3usd_refunds.get_mut(&nonce) else {
        return false;
    };
    if *current != expected
        || current.source != crate::state::ThreeUsdRefundSource::LegacyHashedReserve
        || !current.legacy_dispatch_retryable
        || current.dispatch_submitted
        || current.dispatch_block_index.is_some()
        || current.resolution.is_some()
        || current.dispatch_amount_e8s.is_some()
        || current.dispatch_fee_e8s.is_some()
        || amount_e8s != current.amount_e8s
    {
        return false;
    }
    // This durable fence must precede the ledger await. If the ledger commits
    // but its reply is lost, an upgrade or timer retry must not submit again.
    current.dispatch_amount_e8s = Some(amount_e8s);
    current.dispatch_fee_e8s = Some(fee_e8s);
    current.dispatch_submitted = true;
    true
}

fn release_legacy_3usd_refund_after_bad_fee(
    state: &mut crate::state::State,
    nonce: u128,
    expected: crate::state::PendingThreeUsdRefund,
) -> bool {
    let Some(current) = state.pending_3usd_refunds.get_mut(&nonce) else {
        return false;
    };
    let submitted_expected = crate::state::PendingThreeUsdRefund {
        dispatch_submitted: true,
        dispatch_amount_e8s: if expected.legacy_dispatch_retryable {
            Some(expected.amount_e8s)
        } else {
            expected.dispatch_amount_e8s
        },
        dispatch_fee_e8s: if expected.legacy_dispatch_retryable {
            current.dispatch_fee_e8s
        } else {
            expected.dispatch_fee_e8s
        },
        ..expected
    };
    if *current != submitted_expected
        || current.dispatch_block_index.is_some()
        || (expected.legacy_dispatch_retryable && current.dispatch_fee_e8s.is_none())
    {
        return false;
    }
    // ICRC BadFee is a typed no-effect response. Reusing the same historical
    // memo/time with a refreshed, pinned fee is safe because no transfer was
    // recorded. Every ambiguous transport/ledger error keeps its submitted
    // fence.
    current.dispatch_submitted = false;
    if expected.legacy_dispatch_retryable {
        current.dispatch_amount_e8s = None;
        current.dispatch_fee_e8s = None;
    }
    true
}

async fn dispatch_legacy_three_usd_refund(
    nonce: u128,
    refund: crate::state::PendingThreeUsdRefund,
    destination: icrc_ledger_types::icrc1::account::Account,
) -> Result<u64, TransferError> {
    if refund.dispatch_submitted {
        if let Some(index) = refund.dispatch_block_index {
            return Ok(index);
        }
        return Err(TransferError::GenericError {
            error_code: candid::Nat::from(0u8),
            message: "legacy refund dispatch outcome is ambiguous; candidate receipt required".into(),
        });
    }
    if !refund.legacy_dispatch_retryable {
        return Err(TransferError::GenericError {
            error_code: candid::Nat::from(0u8),
            message: "legacy snapshot refund requires candidate receipt reconciliation".into(),
        });
    }

    let pinned = match (refund.dispatch_amount_e8s, refund.dispatch_fee_e8s) {
        (Some(amount), Some(fee)) if amount == refund.amount_e8s => Some((amount, fee)),
        (None, None) => {
            let fee = crate::management::get_or_refresh_fee(refund.ledger)
                .await
                .map_err(|message| TransferError::GenericError {
                    error_code: candid::Nat::from(0u8),
                    message: format!("legacy refund fee unavailable: {message}"),
                })?;
            let reserve_account = icrc_ledger_types::icrc1::account::Account {
                owner: ic_cdk::id(),
                subaccount: Some(crate::management::protocol_3usd_reserves_subaccount()),
            };
            let balance = crate::management::get_balance_of(reserve_account, refund.ledger)
                .await
                .map_err(|message| TransferError::GenericError {
                    error_code: candid::Nat::from(0u8),
                    message: format!("legacy refund reserve balance unavailable: {message}"),
                })?;
            let enough = read_state(|state| {
                crate::management::three_usd_legacy_refund_fee_buffer_sufficient(
                    state,
                    refund.ledger,
                    u128::from(balance),
                    fee,
                )
            });
            if enough != Some(true) {
                mutate_state(|state| {
                    if let Some(current) = state.pending_3usd_refunds.get_mut(&nonce) {
                        if *current == refund {
                            current.retry_count = MAX_PENDING_RETRIES;
                        }
                    }
                });
                return Err(TransferError::GenericError {
                    error_code: candid::Nat::from(0u8),
                    message: "legacy refund held: reserve principal and protocol fee buffer are not proven".into(),
                });
            }
            Some((refund.amount_e8s, fee))
        }
        _ => None,
    };
    let Some((amount, fee)) = pinned else {
        return Err(TransferError::GenericError {
            error_code: candid::Nat::from(0u8),
            message: "legacy refund pinned transfer tuple is inconsistent".into(),
        });
    };

    let persisted = mutate_state(|state| {
        pin_legacy_three_usd_refund_for_dispatch(state, nonce, refund, amount, fee)
    });
    if !persisted {
        return Err(TransferError::GenericError {
            error_code: candid::Nat::from(0u8),
            message: "legacy refund changed before durable submission fence".into(),
        });
    }
    crate::management::transfer_idempotent_pinned_fee(
        refund.ledger,
        Some(crate::management::protocol_3usd_reserves_subaccount()),
        destination,
        u128::from(amount),
        refund.op_nonce,
        fee,
    )
    .await
}

pub fn verify_legacy_three_usd_refund_receipt(
    refund: &crate::state::PendingThreeUsdRefund,
    backend: Principal,
    block_index: u64,
    block: Result<crate::icrc3_proof::DecodedBlock, String>,
) -> Result<crate::state::ThreeUsdReserveRefundReceipt, String> {
    let block = block?;
    let observed_fee = block
        .fee
        .ok_or_else(|| "legacy refund receipt omits its charged fee".to_string())?;
    if !refund.legacy_dispatch_retryable && observed_fee != 0 {
        return Err("old legacy refund fee is nonzero; exact principal restoration needs operator adjudication".into());
    }
    let tuple = crate::management::legacy_three_usd_refund_receipt_tuple(
        refund,
        backend,
        Some(observed_fee),
    )
        .ok_or_else(|| "legacy refund row lacks an exact historical transfer tuple".to_string())?;
    crate::icrc3_proof::validate_three_usd_reserve_refund_block(&block, &tuple)?;
    Ok(crate::state::ThreeUsdReserveRefundReceipt { block_index, tuple })
}

pub fn commit_legacy_three_usd_refund_receipt(
    state: &mut crate::state::State,
    nonce: u128,
    expected: crate::state::PendingThreeUsdRefund,
    receipt: &crate::state::ThreeUsdReserveRefundReceipt,
    reconciled_at_ns: u64,
    backend: Principal,
) -> bool {
    let Some(current) = state.pending_3usd_refunds.get_mut(&nonce) else {
        return false;
    };
    if *current != expected
        || current.source != crate::state::ThreeUsdRefundSource::LegacyHashedReserve
        || !current.dispatch_submitted
        || current.dispatch_block_index != Some(receipt.block_index)
        || current.resolution.is_some()
        || receipt.tuple.source_owner != backend
        || crate::management::legacy_three_usd_refund_receipt_tuple(
            current,
            backend,
            Some(receipt.tuple.charged_fee_e8s),
        )
        .as_ref()
            != Some(&receipt.tuple)
    {
        return false;
    }
    current.resolution = Some(crate::state::ThreeUsdRefundResolution::ReceiptVerified {
        block_index: receipt.block_index,
        fee_e8s: receipt.tuple.charged_fee_e8s,
        reconciled_at_ns,
    });
    true
}

pub fn commit_legacy_three_usd_refund_candidate(
    state: &mut crate::state::State,
    nonce: u128,
    expected: crate::state::PendingThreeUsdRefund,
    receipt: &crate::state::ThreeUsdReserveRefundReceipt,
    reconciled_at_ns: u64,
    developer: Principal,
    backend: Principal,
) -> bool {
    let Some(current) = state.pending_3usd_refunds.get_mut(&nonce) else {
        return false;
    };
    if *current != expected
        || state.developer_principal != developer
        || developer == Principal::anonymous()
        || current.source != crate::state::ThreeUsdRefundSource::LegacyHashedReserve
        || current.resolution.is_some()
        || receipt.tuple.source_owner != backend
        || crate::management::legacy_three_usd_refund_receipt_tuple(
            current,
            backend,
            Some(receipt.tuple.charged_fee_e8s),
        )
        .as_ref()
            != Some(&receipt.tuple)
    {
        return false;
    }
    current.dispatch_submitted = true;
    current.dispatch_block_index = Some(receipt.block_index);
    current.resolution = Some(crate::state::ThreeUsdRefundResolution::ReceiptVerified {
        block_index: receipt.block_index,
        fee_e8s: receipt.tuple.charged_fee_e8s,
        reconciled_at_ns,
    });
    true
}

#[cfg(test)]
mod pending_3usd_refund_safety_tests {
    use super::{
        pending_3usd_refund_can_be_retried, pin_legacy_three_usd_refund_for_dispatch,
        release_legacy_3usd_refund_after_bad_fee, MAX_PENDING_RETRIES,
    };
    use crate::state::{PendingThreeUsdRefund, State, ThreeUsdRefundSource};
    use candid::Principal;

    fn refund(
        source: ThreeUsdRefundSource,
        parent_absorb_id: Option<u64>,
    ) -> PendingThreeUsdRefund {
        PendingThreeUsdRefund {
            stability_pool: Principal::from_slice(&[1]),
            ledger: Principal::from_slice(&[2]),
            amount_e8s: 300,
            vault_id: 4,
            retry_count: 0,
            op_nonce: 5,
            source,
            parent_absorb_id,
            dispatch_amount_e8s: None,
            dispatch_fee_e8s: None,
            dispatch_submitted: false,
            legacy_dispatch_retryable: source == ThreeUsdRefundSource::LegacyHashedReserve,
            dispatch_block_index: None,
            resolution: None,
        }
    }

    #[test]
    fn parentless_default_account_refunds_are_held_and_child_refunds_remain_live() {
        assert!(!pending_3usd_refund_can_be_retried(&State::default(), &refund(
            ThreeUsdRefundSource::DefaultAccount,
            None,
        )));
        assert!(pending_3usd_refund_can_be_retried(&State::default(), &refund(
            ThreeUsdRefundSource::DefaultAccount,
            Some(6),
        )));
    }

    #[test]
    fn legacy_refund_is_fenced_before_await_and_stays_held_after_upgrade() {
        let mut state = State::default();
        let row = refund(ThreeUsdRefundSource::LegacyHashedReserve, None);
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(pending_3usd_refund_can_be_retried(&state, &row));
        assert!(pin_legacy_three_usd_refund_for_dispatch(
            &mut state,
            row.op_nonce,
            row,
            row.amount_e8s,
            2,
        ));
        let submitted = state.pending_3usd_refunds[&row.op_nonce];
        assert!(submitted.dispatch_submitted);
        assert_eq!(submitted.dispatch_amount_e8s, Some(row.amount_e8s));
        assert_eq!(submitted.dispatch_fee_e8s, Some(2));
        assert!(!pending_3usd_refund_can_be_retried(&state, &submitted));
        assert!(!pin_legacy_three_usd_refund_for_dispatch(
            &mut state,
            row.op_nonce,
            submitted,
            row.amount_e8s,
            2,
        ));

        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&state, &mut bytes).unwrap();
        let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        let after_upgrade = restored.pending_3usd_refunds[&row.op_nonce];
        assert_eq!(after_upgrade, submitted);
        assert!(!pending_3usd_refund_can_be_retried(&restored, &after_upgrade));
    }

    #[test]
    fn changed_or_already_indexed_legacy_refunds_cannot_be_fenced_for_resubmission() {
        let mut state = State::default();
        let row = refund(ThreeUsdRefundSource::LegacyHashedReserve, None);
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        let changed = PendingThreeUsdRefund {
            amount_e8s: 301,
            ..row
        };
        assert!(!pin_legacy_three_usd_refund_for_dispatch(
            &mut state,
            row.op_nonce,
            changed,
            row.amount_e8s,
            2,
        ));
        let indexed = PendingThreeUsdRefund {
            dispatch_block_index: Some(9),
            ..row
        };
        state.pending_3usd_refunds.insert(row.op_nonce, indexed);
        assert!(!pin_legacy_three_usd_refund_for_dispatch(
            &mut state,
            row.op_nonce,
            indexed,
            row.amount_e8s,
            2,
        ));
    }

    #[test]
    fn capped_legacy_refund_waits_for_developer_rearm_and_then_resumes() {
        let developer = Principal::from_slice(&[9]);
        let mut state = State::default();
        state.developer_principal = developer;
        state.three_pool_canister = Some(Principal::from_slice(&[2]));
        state.stability_pool_canister = Some(Principal::from_slice(&[1]));
        let row = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            ..refund(ThreeUsdRefundSource::LegacyHashedReserve, None)
        };
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(!pending_3usd_refund_can_be_retried(&state, &row));

        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            Principal::from_slice(&[8]),
            row.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&row.op_nonce], row);

        crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            row.op_nonce,
        )
        .expect("developer may rearm a capped, known-unsent legacy row");
        let rearmed = state.pending_3usd_refunds[&row.op_nonce];
        assert_eq!(rearmed.retry_count, 0);
        assert_eq!(rearmed.op_nonce, row.op_nonce);
        assert_eq!(rearmed.amount_e8s, row.amount_e8s);
        assert_eq!(rearmed.ledger, row.ledger);
        assert_eq!(rearmed.stability_pool, row.stability_pool);
        assert_eq!(rearmed.source, row.source);
        assert_eq!(rearmed.dispatch_amount_e8s, None);
        assert_eq!(rearmed.dispatch_fee_e8s, None);
        assert!(!rearmed.dispatch_submitted);
        assert!(pending_3usd_refund_can_be_retried(&state, &rearmed));
        assert!(pin_legacy_three_usd_refund_for_dispatch(
            &mut state,
            row.op_nonce,
            rearmed,
            row.amount_e8s,
            2,
        ));
        let resumed = state.pending_3usd_refunds[&row.op_nonce];
        assert!(resumed.dispatch_submitted);
        assert_eq!(resumed.op_nonce, row.op_nonce);
        assert_eq!(resumed.dispatch_amount_e8s, Some(row.amount_e8s));

        let old_snapshot_row = PendingThreeUsdRefund {
            legacy_dispatch_retryable: false,
            retry_count: MAX_PENDING_RETRIES,
            ..rearmed
        };
        state.pending_3usd_refunds.insert(row.op_nonce, old_snapshot_row);
        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            row.op_nonce,
        )
        .is_err());
        assert!(!pending_3usd_refund_can_be_retried(&state, &old_snapshot_row));

        let ambiguous = PendingThreeUsdRefund {
            dispatch_submitted: true,
            retry_count: MAX_PENDING_RETRIES,
            ..rearmed
        };
        state.pending_3usd_refunds.insert(row.op_nonce, ambiguous);
        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            row.op_nonce,
        )
        .is_err());
        assert!(!pending_3usd_refund_can_be_retried(&state, &ambiguous));
    }

    #[test]
    fn capped_refund_with_a_pinned_tuple_cannot_be_rearmed() {
        let developer = Principal::from_slice(&[9]);
        let mut state = State::default();
        state.developer_principal = developer;
        state.three_pool_canister = Some(Principal::from_slice(&[2]));
        state.stability_pool_canister = Some(Principal::from_slice(&[1]));
        let row = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            dispatch_amount_e8s: Some(300),
            dispatch_fee_e8s: Some(2),
            ..refund(ThreeUsdRefundSource::LegacyHashedReserve, None)
        };
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            row.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&row.op_nonce], row);
    }

    #[test]
    fn capped_refund_for_a_changed_ledger_or_pool_cannot_be_rearmed() {
        let developer = Principal::from_slice(&[9]);
        let mut state = State::default();
        state.developer_principal = developer;
        state.three_pool_canister = Some(Principal::from_slice(&[2]));
        state.stability_pool_canister = Some(Principal::from_slice(&[1]));
        let row = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            ledger: Principal::from_slice(&[3]),
            ..refund(ThreeUsdRefundSource::LegacyHashedReserve, None)
        };
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            row.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&row.op_nonce], row);

        let mismatched_pool = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            ledger: Principal::from_slice(&[2]),
            stability_pool: Principal::from_slice(&[4]),
            ..refund(ThreeUsdRefundSource::LegacyHashedReserve, None)
        };
        state.pending_3usd_refunds.insert(row.op_nonce, mismatched_pool);
        assert!(crate::management::rearm_unsent_legacy_three_usd_refund(
            &mut state,
            developer,
            mismatched_pool.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&row.op_nonce], mismatched_pool);
    }

    #[test]
    fn typed_bad_fee_no_effect_releases_only_the_exact_legacy_dispatch_fence() {
        let mut state = State::default();
        let row = refund(ThreeUsdRefundSource::LegacyHashedReserve, None);
        let submitted = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(row.amount_e8s),
            dispatch_fee_e8s: Some(1),
            ..row
        };
        state.pending_3usd_refunds.insert(row.op_nonce, submitted);
        assert!(release_legacy_3usd_refund_after_bad_fee(
            &mut state,
            row.op_nonce,
            row,
        ));
        assert!(!state.pending_3usd_refunds[&row.op_nonce].dispatch_submitted);

        let indexed = PendingThreeUsdRefund {
            dispatch_block_index: Some(10),
            dispatch_amount_e8s: Some(row.amount_e8s),
            dispatch_fee_e8s: Some(1),
            ..submitted
        };
        state.pending_3usd_refunds.insert(row.op_nonce, indexed);
        assert!(!release_legacy_3usd_refund_after_bad_fee(
            &mut state,
            row.op_nonce,
            row,
        ));
    }
}

#[cfg(test)]
mod v2_default_account_refund_bad_fee_tests {
    use super::{pending_3usd_refund_can_be_retried, MAX_PENDING_RETRIES};
    use crate::state::{
        PendingThreeUsdRefund, State, ThreeUsdRefundSource, ThreeUsdReserveIngressKey,
        ThreeUsdReserveIngressJournal, ThreeUsdReserveIngressPhase,
        ThreeUsdReserveIngressRefund, ThreeUsdReserveIngressRequest,
        ThreeUsdReserveIngressTuple,
    };
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn identities() -> (Principal, Principal, Principal, Principal) {
        (
            Principal::from_slice(&[1]), // Stability Pool
            Principal::from_slice(&[2]), // 3USD ledger
            Principal::from_slice(&[3]), // developer
            Principal::from_slice(&[4]), // backend tuple destination
        )
    }

    fn row(pool: Principal, ledger: Principal) -> PendingThreeUsdRefund {
        PendingThreeUsdRefund {
            stability_pool: pool,
            ledger,
            amount_e8s: 100,
            vault_id: 5,
            retry_count: 2,
            op_nonce: 88,
            source: ThreeUsdRefundSource::DefaultAccount,
            parent_absorb_id: Some(7),
            dispatch_amount_e8s: None,
            dispatch_fee_e8s: None,
            dispatch_submitted: false,
            legacy_dispatch_retryable: false,
            dispatch_block_index: None,
            resolution: None,
        }
    }

    fn state_with_parent(refund: PendingThreeUsdRefund, backend: Principal) -> State {
        let mut state = State::default();
        state.developer_principal = identities().2;
        state.stability_pool_canister = Some(refund.stability_pool);
        state.three_pool_canister = Some(refund.ledger);
        let absorb_id = refund.parent_absorb_id.expect("fixture must be a V2 child");
        let key = ThreeUsdReserveIngressKey {
            stability_pool: refund.stability_pool,
            vault_id: refund.vault_id,
            absorb_id,
        };
        let tuple = ThreeUsdReserveIngressTuple {
            spender_owner: refund.stability_pool,
            spender_subaccount: None,
            source: Account {
                owner: refund.stability_pool,
                subaccount: None,
            },
            destination: Account {
                owner: backend,
                subaccount: None,
            },
            amount_e8s: 200,
            fee_e8s: Some(0),
            ledger_fee_e8s: 0,
            memo: [9; 16],
            created_at_time_ns: 10,
            op_nonce: 77,
            parent_absorb_id: absorb_id,
        };
        state.three_usd_reserve_ingress_journals.insert(
            key,
            ThreeUsdReserveIngressJournal {
                request: ThreeUsdReserveIngressRequest {
                    icusd_debt_covered_e8s: 200,
                    three_usd_amount_e8s: 200,
                    ledger: refund.ledger,
                },
                phase: ThreeUsdReserveIngressPhase::FailedAfterTransfer {
                    tuple,
                    block_index: 12,
                    error: "injected downstream failure".into(),
                },
                ingress_proof_verified: true,
                refund: Some(ThreeUsdReserveIngressRefund {
                    op_nonce: refund.op_nonce,
                    required_net_credit_e8s: refund.amount_e8s,
                    settled_receipt: None,
                }),
                protocol_refund_fee_reserve_e8s: refund.amount_e8s,
                non_inclusion_scan: None,
            },
        );
        state
    }

    #[test]
    fn typed_bad_fee_clears_only_the_exact_v2_fence_and_keeps_liability_capped() {
        let (pool, ledger, _, backend) = identities();
        let expected = row(pool, ledger);
        let mut state = state_with_parent(expected, backend);
        let submitted = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(expected.amount_e8s),
            dispatch_fee_e8s: Some(0),
            ..expected
        };
        state.pending_3usd_refunds.insert(expected.op_nonce, submitted);

        assert!(crate::management::release_v2_default_account_refund_after_bad_fee(
            &mut state,
            expected.op_nonce,
            expected,
        ));
        let held = state.pending_3usd_refunds[&expected.op_nonce];
        assert!(!held.dispatch_submitted);
        assert_eq!(held.dispatch_block_index, None);
        assert_eq!(held.dispatch_amount_e8s, None);
        assert_eq!(held.dispatch_fee_e8s, None);
        assert_eq!(held.retry_count, MAX_PENDING_RETRIES);
        assert_eq!(held.op_nonce, expected.op_nonce);
        assert_eq!(held.amount_e8s, expected.amount_e8s);
        assert_eq!(held.parent_absorb_id, expected.parent_absorb_id);
        assert_eq!(held.resolution, None);
        assert!(!pending_3usd_refund_can_be_retried(&state, &held));
        assert_eq!(
            state.three_usd_reserve_ingress_journals.values().next().unwrap()
                .refund.as_ref().unwrap().required_net_credit_e8s,
            expected.amount_e8s,
            "typed no-effect must retain the parent principal liability",
        );
    }

    #[test]
    fn bad_fee_release_rejects_changed_rows_or_parent_children() {
        let (pool, ledger, _, backend) = identities();
        let expected = row(pool, ledger);
        let mut state = state_with_parent(expected, backend);
        let submitted = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(expected.amount_e8s),
            dispatch_fee_e8s: Some(0),
            ..expected
        };
        let changed = PendingThreeUsdRefund {
            amount_e8s: expected.amount_e8s + 1,
            ..submitted
        };
        state.pending_3usd_refunds.insert(expected.op_nonce, changed);
        assert!(!crate::management::release_v2_default_account_refund_after_bad_fee(
            &mut state,
            expected.op_nonce,
            expected,
        ));
        assert_eq!(state.pending_3usd_refunds[&expected.op_nonce], changed);

        state.pending_3usd_refunds.insert(expected.op_nonce, submitted);
        let journal = state.three_usd_reserve_ingress_journals.values_mut().next().unwrap();
        journal.refund.as_mut().unwrap().op_nonce += 1;
        assert!(!crate::management::release_v2_default_account_refund_after_bad_fee(
            &mut state,
            expected.op_nonce,
            expected,
        ));
        assert_eq!(state.pending_3usd_refunds[&expected.op_nonce], submitted);
    }

    #[test]
    fn developer_rearm_requires_exact_unsent_v2_child_and_preserves_identity() {
        let (pool, ledger, developer, backend) = identities();
        let expected = row(pool, ledger);
        let mut state = state_with_parent(expected, backend);
        let submitted = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(expected.amount_e8s),
            dispatch_fee_e8s: Some(0),
            ..expected
        };
        state.pending_3usd_refunds.insert(expected.op_nonce, submitted);
        assert!(crate::management::release_v2_default_account_refund_after_bad_fee(
            &mut state,
            expected.op_nonce,
            expected,
        ));
        let held = state.pending_3usd_refunds[&expected.op_nonce];
        assert!(crate::management::rearm_unsent_v2_default_account_refund(
            &mut state,
            Principal::from_slice(&[8]),
            expected.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&expected.op_nonce], held);

        state.stability_pool_canister = Some(Principal::from_slice(&[9]));
        assert!(crate::management::rearm_unsent_v2_default_account_refund(
            &mut state,
            developer,
            expected.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&expected.op_nonce], held);
        state.stability_pool_canister = Some(expected.stability_pool);

        crate::management::rearm_unsent_v2_default_account_refund(
            &mut state,
            developer,
            expected.op_nonce,
        )
        .expect("developer may rearm exact known-unsent V2 child");
        let rearmed = state.pending_3usd_refunds[&expected.op_nonce];
        assert_eq!(rearmed.retry_count, 0);
        assert_eq!(rearmed.op_nonce, expected.op_nonce);
        assert_eq!(rearmed.amount_e8s, expected.amount_e8s);
        assert_eq!(rearmed.ledger, expected.ledger);
        assert_eq!(rearmed.stability_pool, expected.stability_pool);
        assert_eq!(rearmed.parent_absorb_id, expected.parent_absorb_id);
        assert!(pending_3usd_refund_can_be_retried(&state, &rearmed));
    }

    #[test]
    fn developer_rearm_rejects_ambiguous_or_parent_mismatched_v2_rows() {
        let (pool, ledger, developer, backend) = identities();
        let expected = row(pool, ledger);
        let mut state = state_with_parent(expected, backend);
        let mut ambiguous = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(expected.amount_e8s),
            dispatch_fee_e8s: Some(0),
            ..expected
        };
        state.pending_3usd_refunds.insert(expected.op_nonce, ambiguous);
        assert!(crate::management::rearm_unsent_v2_default_account_refund(
            &mut state,
            developer,
            expected.op_nonce,
        )
        .is_err());
        assert_eq!(state.pending_3usd_refunds[&expected.op_nonce], ambiguous);
        assert!(state.pending_3usd_refunds[&expected.op_nonce].dispatch_submitted);
        assert_eq!(
            state.pending_3usd_refunds[&expected.op_nonce].dispatch_amount_e8s,
            Some(expected.amount_e8s),
        );
        assert!(!pending_3usd_refund_can_be_retried(
            &state,
            &state.pending_3usd_refunds[&expected.op_nonce],
        ));

        ambiguous = PendingThreeUsdRefund {
            retry_count: MAX_PENDING_RETRIES,
            dispatch_submitted: false,
            dispatch_amount_e8s: None,
            dispatch_fee_e8s: None,
            ..expected
        };
        state.pending_3usd_refunds.insert(expected.op_nonce, ambiguous);
        let journal = state.three_usd_reserve_ingress_journals.values_mut().next().unwrap();
        journal.request.ledger = Principal::from_slice(&[6]);
        assert!(crate::management::rearm_unsent_v2_default_account_refund(
            &mut state,
            developer,
            expected.op_nonce,
        )
        .is_err());
    }
}

#[cfg(test)]
mod legacy_3usd_refund_receipt_tests {
    use super::{
        commit_legacy_three_usd_refund_candidate, commit_legacy_three_usd_refund_receipt,
        verify_legacy_three_usd_refund_receipt,
    };
    use crate::state::{
        PendingThreeUsdRefund, State, ThreeUsdRefundResolution, ThreeUsdRefundSource,
    };
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn refund() -> PendingThreeUsdRefund {
        PendingThreeUsdRefund {
            stability_pool: Principal::from_slice(&[2]),
            ledger: Principal::from_slice(&[1]),
            amount_e8s: 10,
            vault_id: 3,
            retry_count: 0,
            op_nonce: (7u128 << 64) | 4,
            source: ThreeUsdRefundSource::LegacyHashedReserve,
            parent_absorb_id: None,
            dispatch_amount_e8s: Some(10),
            dispatch_fee_e8s: Some(0),
            dispatch_submitted: false,
            legacy_dispatch_retryable: true,
            dispatch_block_index: None,
            resolution: None,
        }
    }

    fn valid_block(row: &PendingThreeUsdRefund, backend: Principal) -> crate::icrc3_proof::DecodedBlock {
        let tuple = crate::management::legacy_three_usd_refund_receipt_tuple(
            row,
            backend,
            Some(0),
        )
        .unwrap();
        crate::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(Account {
                owner: tuple.source_owner,
                subaccount: tuple.source_subaccount,
            }),
            to: Some(tuple.destination),
            spender: None,
            amount: u128::from(tuple.amount_e8s),
            fee: Some(0),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(tuple.memo.to_vec()),
        }
    }

    #[test]
    fn exact_direct_tuple_resolves_and_tombstones_the_legacy_liability() {
        let backend = Principal::from_slice(&[8]);
        let row = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_block_index: Some(77),
            ..refund()
        };
        let receipt = verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            77,
            Ok(valid_block(&row, backend)),
        )
        .expect("exact receipt must verify");
        let mut state = State::default();
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(commit_legacy_three_usd_refund_receipt(
            &mut state,
            row.op_nonce,
            row,
            &receipt,
            99,
            backend,
        ));
        assert_eq!(
            state.pending_3usd_refunds[&row.op_nonce].resolution,
            Some(ThreeUsdRefundResolution::ReceiptVerified {
                block_index: 77,
                fee_e8s: 0,
                reconciled_at_ns: 99,
            })
        );
        assert!(!super::pending_3usd_refund_can_be_retried(
            &state,
            &state.pending_3usd_refunds[&row.op_nonce]
        ));
    }

    #[test]
    fn pinned_nonzero_fee_preserves_full_principal_as_sp_credit() {
        let backend = Principal::from_slice(&[8]);
        let row = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_amount_e8s: Some(10),
            dispatch_fee_e8s: Some(2),
            ..refund()
        };
        let mut block = valid_block(&row, backend);
        block.fee = Some(2);
        let receipt = verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            78,
            Ok(block),
        )
        .expect("pinned nonzero fee must validate when principal is unchanged");
        assert_eq!(receipt.tuple.amount_e8s, 10);
        assert_eq!(receipt.tuple.fee_e8s, Some(2));
        assert_eq!(receipt.tuple.charged_fee_e8s, 2);
    }

    #[test]
    fn archive_unavailable_mismatch_and_changed_rows_remain_unresolved() {
        let backend = Principal::from_slice(&[8]);
        let row = refund();
        assert!(verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            77,
            Err("direct ledger response advertised archive callback".into()),
        )
        .is_err());
        assert!(verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            77,
            Err("direct ledger query unavailable".into()),
        )
        .is_err());

        let mut mismatch = valid_block(&row, backend);
        mismatch.to.as_mut().unwrap().owner = Principal::from_slice(&[9]);
        assert!(verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            77,
            Ok(mismatch),
        )
        .is_err());

        let submitted = PendingThreeUsdRefund {
            dispatch_submitted: true,
            dispatch_block_index: Some(77),
            ..row
        };
        let receipt = verify_legacy_three_usd_refund_receipt(
            &submitted,
            backend,
            77,
            Ok(valid_block(&submitted, backend)),
        )
        .unwrap();
        let mut changed = submitted;
        changed.amount_e8s += 1;
        let mut state = State::default();
        state.pending_3usd_refunds.insert(row.op_nonce, changed);
        assert!(!commit_legacy_three_usd_refund_receipt(
            &mut state,
            row.op_nonce,
            submitted,
            &receipt,
            99,
            backend,
        ));
        assert_eq!(state.pending_3usd_refunds[&row.op_nonce].resolution, None);
    }

    #[test]
    fn developer_candidate_proof_resolves_pre_marker_legacy_row() {
        let backend = Principal::from_slice(&[8]);
        let developer = Principal::from_slice(&[9]);
        let row = PendingThreeUsdRefund {
            legacy_dispatch_retryable: false,
            dispatch_amount_e8s: None,
            dispatch_fee_e8s: None,
            ..refund()
        }; // old snapshot: provenance marker defaults false
        assert!(!row.legacy_dispatch_retryable);
        let receipt = verify_legacy_three_usd_refund_receipt(
            &row,
            backend,
            88,
            Ok(valid_block(&row, backend)),
        )
        .expect("candidate must prove the exact old tuple");
        let mut state = State::default();
        state.developer_principal = developer;
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        assert!(!commit_legacy_three_usd_refund_candidate(
            &mut state,
            row.op_nonce,
            row,
            &receipt,
            100,
            Principal::from_slice(&[7]),
            backend,
        ));
        assert!(commit_legacy_three_usd_refund_candidate(
            &mut state,
            row.op_nonce,
            row,
            &receipt,
            101,
            developer,
            backend,
        ));
        let resolved = state.pending_3usd_refunds[&row.op_nonce];
        assert_eq!(resolved.dispatch_block_index, Some(88));
        assert!(!resolved.legacy_dispatch_retryable);
        assert_eq!(
            resolved.resolution,
            Some(ThreeUsdRefundResolution::ReceiptVerified {
                block_index: 88,
                fee_e8s: 0,
                reconciled_at_ns: 101,
            })
        );
    }

    #[test]
    fn lost_reply_or_upgrade_fence_without_index_is_held_without_resubmission() {
        let backend = Principal::from_slice(&[8]);
        let row = PendingThreeUsdRefund {
            dispatch_submitted: true,
            ..refund()
        };
        assert!(!super::pending_3usd_refund_can_be_retried(&State::default(), &row));
        let mut state = State::default();
        state.pending_3usd_refunds.insert(row.op_nonce, row);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&state, &mut bytes).unwrap();
        let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        let restored_row = restored.pending_3usd_refunds[&row.op_nonce];
        assert!(restored_row.dispatch_submitted);
        assert_eq!(restored_row.dispatch_block_index, None);
        assert!(!super::pending_3usd_refund_can_be_retried(&restored, &restored_row));
        assert!(verify_legacy_three_usd_refund_receipt(
            &restored_row,
            backend,
            77,
            Err("receipt index absent after lost reply".into()),
        )
        .is_err());
    }
}

fn redemption_transfer_meets_minimum(
    gross_raw: u64,
    fee_raw: u64,
    minimum_net_raw: Option<u64>,
) -> bool {
    minimum_net_raw
        .map(|minimum| {
            let net = gross_raw.saturating_sub(fee_raw);
            net > 0 && net >= minimum
        })
        .unwrap_or(true)
}

pub mod chains;
pub mod dashboard;
pub mod event;
pub mod guard;
pub mod icrc21;
pub mod icrc3_proof;
pub mod sp_burn_refund;
pub mod liquidity_pool;
pub mod logs;
pub mod management;
pub mod native_icp_proof;
pub mod numeric;
pub mod payout_history;
pub mod state;
pub mod storage;
pub mod treasury;
pub mod vault;
pub mod xrc;

#[cfg(any(test, feature = "test_endpoints"))]
pub mod test_helpers;

#[cfg(test)]
mod tests;

/// Reject non-finite (NaN, ±Infinity) or out-of-range f64 admin inputs.
/// Range is inclusive on both ends. Returns a human-readable error string
/// suitable for wrapping in `ProtocolError::GenericError`.
///
/// Caution: `f64` ordering returns `false` for any comparison with NaN, so
/// naked `value < min || value > max` checks let NaN slip through.
pub fn validate_f64_inclusive(name: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() || value < min || value > max {
        return Err(format!(
            "{} ({}) must be a finite number in [{}, {}]",
            name, value, min, max
        ));
    }
    Ok(())
}

pub const SEC_NANOS: u64 = 1_000_000_000;
pub const E8S: u64 = 100_000_000;

pub const MIN_LIQUIDITY_AMOUNT: ICUSD = ICUSD::new(1_000_000_000);
pub const MIN_ICP_AMOUNT: ICP = ICP::new(100_000); // Instead of MIN_CKBTC_AMOUNT
pub const MIN_ICUSD_AMOUNT: ICUSD = ICUSD::new(10_000_000); // 0.1 icUSD minimum for all stablecoin operations
pub const DUST_THRESHOLD: ICUSD = ICUSD::new(100); // 0.000001 icUSD - dust threshold for vault closing

// Update collateral ratios per whitepaper
pub const RECOVERY_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.5)); // 150%
pub const MINIMUM_COLLATERAL_RATIO: Ratio = Ratio::new(dec!(1.33)); // 133%
/// Default protocol share of liquidator's bonus profit (3%).
pub const DEFAULT_LIQUIDATION_PROTOCOL_SHARE: Ratio = Ratio::new(dec!(0.03));

/// Wave-9c DOS-005: default alert band (in basis points) above each
/// collateral's `min_liquidation_ratio` within which `check_vaults`
/// walks the sorted-troves index. 1000 bps = 10% headroom. Tuned via
/// `set_check_vaults_alert_band_bps`.
pub const DEFAULT_CHECK_VAULTS_ALERT_BAND_BPS: u64 = 1000;

/// Wave-9c DOS-005: default cadence (in 5-minute XRC ticks) for the
/// safety-belt full sweep that walks every vault regardless of CR
/// band. 12 = once per hour. 0 or 1 means full sweep every tick
/// (effectively reverts to pre-Wave-9c behavior). Tuned via
/// `set_check_vaults_full_sweep_every_n_ticks`.
pub const DEFAULT_CHECK_VAULTS_FULL_SWEEP_EVERY_N_TICKS: u64 = 12;

/// Default tolerance (in basis points) added to the per-collateral
/// `min_liquidation_ratio` when the liquidation bot calls
/// `bot_claim_liquidation`. Closes the TOCTOU window between the
/// `scan_unhealthy_vaults` flag (CR < min_ratio) and the bot's
/// follow-up claim, where an XRC tick between those two moments can
/// nudge the recomputed CR back above the strict threshold.
/// 200 bps (2%) covers the typical price moves in a single 30s bot
/// tick. Tunable via `set_bot_cr_tolerance_bps`.
pub const DEFAULT_BOT_CR_TOLERANCE_BPS: u64 = 200;

/// Hard cap on the bot CR tolerance an admin may configure. 500 bps
/// (5%) — beyond this the bot starts liquidating vaults that have
/// substantially recovered between scan and claim, which is closer to
/// "near-threshold liquidation" than "race-window absorption".
pub const MAX_BOT_CR_TOLERANCE_BPS: u64 = 500;

/// Stable token types accepted for vault repayment (1:1 with icUSD)
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StableTokenType {
    /// ckUSDT stablecoin
    CKUSDT,
    /// ckUSDC stablecoin
    CKUSDC,
}

/// Arguments for repaying vault with a stable token
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultArgWithToken {
    pub vault_id: u64,
    pub amount: u64,
    pub token_type: StableTokenType,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolArg {
    Init(InitArg),
    Upgrade(UpgradeArg),
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitArg {
    pub xrc_principal: Principal,
    pub icusd_ledger_principal: Principal,
    pub icp_ledger_principal: Principal,
    pub fee_e8s: u64,
    pub developer_principal: Principal,
    pub treasury_principal: Option<Principal>,
    pub stability_pool_principal: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeArg {
    pub mode: Option<Mode>,
    /// Human-readable description of what changed in this upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ProtocolStatus {
    pub last_icp_rate: f64,
    pub last_icp_timestamp: u64,
    pub total_icp_margin: u64,
    pub total_icusd_borrowed: u64,
    pub total_collateral_ratio: f64,
    pub mode: Mode,
    pub liquidation_bonus: f64,
    pub recovery_target_cr: f64,
    pub recovery_mode_threshold: f64,
    pub recovery_cr_multiplier: f64,
    pub reserve_redemptions_enabled: bool,
    pub reserve_redemption_fee: f64,
    pub ckstable_repay_fee: f64,
    pub min_icusd_amount: u64,
    /// LIQ-0XX: admin-settable dust-liquidation threshold (icUSD, e8s). A
    /// vault whose debt is at or below this threshold is always liquidated
    /// in full rather than partially. See `State::effective_liquidation_amount`.
    pub dust_liquidation_threshold_e8s: u64,
    pub global_icusd_mint_cap: u64,
    pub frozen: bool,
    pub manual_mode_override: bool,
    pub interest_pool_share: f64,
    pub weighted_average_interest_rate: f64,
    pub borrowing_fee_curve_resolved: Vec<(f64, f64)>,
    pub per_collateral_interest: Vec<CollateralInterestInfo>,
    pub per_collateral_rate_curves: Vec<PerCollateralRateCurve>,
    pub interest_split: Vec<InterestSplitArg>,
    /// Wave-8e LIQ-005: cumulative bad debt absorbed from underwater
    /// liquidations, awaiting fee-driven repayment.
    pub protocol_deficit_icusd: u64,
    /// Wave-8e LIQ-005: lifetime sum of icUSD applied as deficit repayment.
    pub total_deficit_repaid_icusd: u64,
    /// Wave-8e LIQ-005: fraction of each fee routed to deficit repayment.
    pub deficit_repayment_fraction: f64,
    /// Wave-8e LIQ-005: e8s threshold above which the protocol auto-latches
    /// to ReadOnly. 0 disables the latch.
    pub deficit_readonly_threshold_e8s: u64,
    /// Wave-10 LIQ-008: rolling window length for the mass-liquidation
    /// circuit breaker, in nanoseconds. 0 disables the breaker.
    pub breaker_window_ns: u64,
    /// Wave-10 LIQ-008: cumulative-debt ceiling within the window, in icUSD
    /// e8s. 0 disables tripping.
    pub breaker_window_debt_ceiling_e8s: u64,
    /// Wave-10 LIQ-008: live windowed sum of debt cleared (icUSD e8s).
    /// Compares to `breaker_window_debt_ceiling_e8s` to project breaker headroom.
    pub windowed_liquidation_total_e8s: u64,
    /// Wave-10 LIQ-008: true once the breaker has tripped on the current
    /// window total. Cleared by admin via `clear_liquidation_breaker`.
    pub liquidation_breaker_tripped: bool,
    /// Wave-9b DOS-006: nanosecond timestamp at which the cached heavy
    /// aggregates (totals, weighted rates, per-collateral rollups) were
    /// last computed. Two calls within `PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS`
    /// observe the same value, proving cache hit. Live fields elsewhere
    /// in this struct still reflect current state on every call.
    pub snapshot_ts_ns: u64,
}

/// Per-collateral debt and weighted interest rate for APR calculations.
#[derive(CandidType, Deserialize, Debug)]
pub struct CollateralInterestInfo {
    pub collateral_type: Principal,
    pub total_debt_e8s: u64,
    pub weighted_interest_rate: f64,
}

/// Phase 1a: per-chain icUSD supply entry for `get_supply_audit()`.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SupplyAuditEntry {
    pub chain_id: crate::chains::config::ChainId,
    pub display_name: String,
    pub supply_e8s: u128,
}

/// Phase 1a: per-chain breakdown of canonical multi-chain icUSD supply.
/// Returned by `get_supply_audit()` for external auditors and dashboards.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SupplyAudit {
    pub total_e8s: u128,
    pub per_chain: Vec<SupplyAuditEntry>,
}

/// Per-collateral Layer 1 interest rate curve for frontend interpolation.
#[derive(CandidType, Deserialize, Debug)]
pub struct PerCollateralRateCurve {
    pub collateral_type: Principal,
    pub base_rate: f64,
    pub markers: Vec<(f64, f64)>,
}

/// Candid-compatible representation of an interest split entry for the API.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterestSplitArg {
    pub destination: String, // "stability_pool", "treasury", "three_pool"
    pub bps: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveRedemptionResult {
    pub icusd_block_index: u64,
    pub stable_amount_sent: u64,
    pub fee_amount: u64,
    pub stable_token_used: Principal,
    pub vault_spillover_amount: u64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ReserveBalance {
    pub ledger: Principal,
    pub balance: u64,
    pub symbol: String,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct Fees {
    pub borrowing_fee: f64,
    pub redemption_fee: f64,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct SuccessWithFee {
    pub block_index: u64,
    pub fee_amount_paid: u64,
    /// Total collateral (native units) awarded to the liquidator / stability pool.
    /// Added so the stability pool can correctly credit depositors with their
    /// proportional share of the actual collateral received, rather than only
    /// the liquidator bonus (`fee_amount_paid`).
    pub collateral_amount_received: Option<u64>,
    /// SP-101: the icUSD-denominated debt the backend ACTUALLY cleared (in e8s).
    /// The partial-liquidation paths cap the requested draw to the vault's
    /// `max_liquidatable_debt`; the stability pool must debit depositors by this
    /// realized amount, not the (possibly larger) amount it requested, or the
    /// tracked aggregate drifts above the pool's real balance. `None` on the
    /// non-liquidation paths (redeem / borrow). Optional for Candid back-compat.
    pub debt_liquidated_e8s: Option<u64>,
    /// SP-110 (audit 2026-06-05): on the ckUSDT/ckUSDC liquidation path the
    /// backend pulls `base + repay-fee surcharge` stable from the caller, but
    /// `debt_liquidated_e8s` only reflects the base debt. The stability pool
    /// must debit depositors by the TOTAL stable pulled (this field, in the
    /// stable token's e6 native units), or the repay-fee surcharge leaves the
    /// pool un-debited and the tracked aggregate drifts above the real ledger
    /// balance. `Some` only on the ckStable path; `None` elsewhere. Optional for
    /// Candid back-compat.
    pub stable_pulled_e6s: Option<u64>,
    /// Native-XRP manual liquidation payout claim id for the liquidator reward.
    /// `None` for non-XRP collateral and non-liquidation SuccessWithFee results.
    pub xrp_claim_id: Option<u64>,
}

/// Snapshot of one consecutive run of same-collateral vaults in global
/// redemption-health order. Capacities are for this run only; a redemption
/// never crosses into the next row automatically.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedemptionQueueEntry {
    pub run_index: u32,
    pub collateral_type: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub price_usd: f64,
    pub price_timestamp_ns: u64,
    pub price_fresh: bool,
    pub min_cr: f64,
    pub liquidation_cr: f64,
    pub weakest_vault_cr: f64,
    /// Unclamped shade headroom. Lower means closer to the displayed red zone.
    pub health_headroom: f64,
    pub vault_count: u64,
    /// Total physical collateral in this eligible run, in native token units.
    pub eligible_collateral_raw: u128,
    pub eligible_debt_e8s: u64,
    pub max_input_icusd_e8s: u64,
    /// Maximum backed payout after the collateral ledger fee, in native units.
    pub max_net_collateral_raw: u64,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedemptionQueue {
    pub observed_at_ns: u64,
    /// False if any eligible collateral that could affect the global order is
    /// missing a valid in-window price or cannot be represented in the runs.
    pub ranking_fresh: bool,
    pub rmr: f64,
    pub price_freshness_window_ns: u64,
    pub entries: Vec<RedemptionQueueEntry>,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedemptionQuote {
    pub quoted_at_ns: u64,
    /// Lifetime of the displayed quote snapshot; distinct from cached price age.
    pub quote_validity_window_ns: u64,
    pub ranking_fresh: bool,
    pub amount_e8s: u64,
    pub run_index: u32,
    pub collateral_type: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub price_usd: f64,
    pub price_timestamp_ns: u64,
    pub price_fresh: bool,
    pub fee_e8s: u64,
    pub rmr: f64,
    pub effective_icusd_e8s: u64,
    pub gross_collateral_raw: u64,
    pub ledger_fee_raw: u64,
    pub net_collateral_raw: u64,
    pub max_input_icusd_e8s: u64,
}

/// Cached redemption preview. It may describe an old but complete price
/// ranking; consumers must treat it as advisory and never submit it directly.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedemptionPreview {
    pub queue: RedemptionQueue,
    pub estimate: Result<RedemptionQuote, RedemptionError>,
}

/// Fresh, read-only offer preparation result. A fresh queue is returned even
/// when the requested amount is below minimum or exceeds current capacity.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct PreparedRedemptionOffer {
    pub queue: RedemptionQueue,
    pub quote: Result<RedemptionQuote, RedemptionError>,
}

/// Failure while refreshing prices for a read-only redemption offer. These
/// variants are intentionally separate from the historical ProtocolError and
/// RedemptionError contracts.
#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum RedemptionOfferRefreshError {
    /// Another bounded candidate refresh is in flight. `retry_after_ns` is a
    /// duration, not an absolute timestamp.
    RefreshInProgress {
        retry_after_ns: u64,
    },
    /// A previous stale-price batch is still in its global cooldown.
    RefreshCooldown {
        retry_after_ns: u64,
    },
    CandidateLimitExceeded {
        max_candidates: u64,
    },
    RefreshUnavailable {
        message: String,
        retry_after_ns: u64,
    },
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedeemQuotedRequest {
    pub amount_e8s: u64,
    pub expected_collateral_type: Principal,
    pub min_net_collateral_raw: u64,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct RedemptionResult {
    pub icusd_block_index: u64,
    pub fee_paid_e8s: u64,
    pub collateral_type: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub net_collateral_raw: u64,
    /// Queued means the durable payout is pending ledger delivery.
    pub payout_status: RedemptionPayoutStatus,
}

#[derive(CandidType, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RedemptionPayoutStatus {
    Queued,
}

/// Result from stability pool liquidation (both standard and debt-already-burned paths).
#[derive(CandidType, Deserialize, Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeUsdReserveIngressV2StatusView {
    pub stability_pool: Principal,
    pub vault_id: u64,
    pub absorb_id: u64,
    pub status: ThreeUsdReserveIngressV2Status,
}

/// SP may release an absorb identity only on `PreTransferRejected`, a replayed
/// exact `Absorbed` result, or an exact `FailedAfterTransferRefunded` receipt.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreeUsdReserveIngressV2Status {
    Unseen,
    AdmissionPending,
    PreTransferRejected { reason: String },
    TransferSubmittedOrUnknown,
    TransferConfirmed { transfer_block_index: u64 },
    Absorbed {
        transfer_block_index: u64,
        ingress_fee_e8s: u64,
        result: StabilityPoolLiquidationResult,
        proportional_refund: Option<crate::state::ThreeUsdReserveRefundReceipt>,
    },
    AbsorbedRefundPending {
        transfer_block_index: u64,
        result: StabilityPoolLiquidationResult,
        refund_amount_e8s: u64,
    },
    FailedRefundPending {
        transfer_block_index: u64,
        refund_amount_e8s: u64,
        error: String,
    },
    FailedAfterTransferRefunded {
        transfer_block_index: u64,
        ingress_fee_e8s: u64,
        refund_fee_e8s: u64,
        error: String,
        refund_receipt: crate::state::ThreeUsdReserveRefundReceipt,
    },
    ReconciliationRequired { reason: String },
}

pub const MAX_XRP_SP_PAYOUT_ALLOCATIONS: usize = 500;

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpSpAbsorbPreflight {
    pub vault_id: u64,
    pub icusd_burn_e8s: u64,
    pub collateral_received_drops: u64,
    pub collateral_price_e8s: u64,
    pub expires_at_ns: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpSpPayoutAllocation {
    pub claimant: Principal,
    pub payout_address: String,
    pub destination_tag: Option<u32>,
    pub drops: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpSpAbsorbRequest {
    pub vault_id: u64,
    pub icusd_burned_e8s: u64,
    pub proof: crate::icrc3_proof::SpWritedownProof,
    pub allocations: Vec<XrpSpPayoutAllocation>,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpSpPayoutClaim {
    pub claimant: Principal,
    pub claim_id: u64,
    pub payout_address: String,
    pub destination_tag: Option<u32>,
    pub drops: u64,
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpSpAbsorbResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt_e8s: u64,
    pub collateral_received_drops: u64,
    pub payout_claims: Vec<XrpSpPayoutClaim>,
    pub block_index: u64,
    pub collateral_price_e8s: u64,
}

/// Exact backend terminal state for one persisted native-XRP burn intent.
#[derive(CandidType, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum XrpSpAbsorbStatus {
    Accepted(XrpSpAbsorbResult),
    RefundJournaled,
    Unseen,
    ConsumedWithoutResult,
}

/// Coarse classification of an `Event` for the explorer's type facet.
/// Each variant maps to one or more concrete `Event` cases via
/// `Event::type_filter()`. Adding a new `Event` variant requires extending
/// both the mapping there and (if needed) this enum.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventTypeFilter {
    OpenVault,
    CloseVault,
    AdjustVault,
    Borrow,
    Repay,
    Liquidation,
    PartialLiquidation,
    Redemption,
    ReserveRedemption,
    StabilityPoolDeposit,
    StabilityPoolWithdraw,
    AdminMint,
    AdminSweepToTreasury,
    Admin,
    PriceUpdate,
    AccrueInterest,
    /// Wave-8e LIQ-005: bad-debt accrued from an underwater liquidation.
    DeficitAccrued,
    /// Wave-8e LIQ-005: deficit repaid via fee revenue routing.
    DeficitRepaid,
    /// Wave-10 LIQ-008: an automatic mass-liquidation circuit-breaker trip.
    /// Distinct from the admin tunables (which collapse to `Admin`) so
    /// operators can audit every breaker firing in isolation.
    BreakerTripped,
    /// Wave-11 BOT-001: a `check_vaults` auto-cancel was skipped because the
    /// bot did not return the collateral within the 10-minute window. Distinct
    /// filter so operators can directly query "stuck claims awaiting
    /// reconciliation" without scanning the noisier admin bucket.
    BotClaimReconciliationNeeded,
}

/// Inclusive nanosecond timestamp window for the time facet.
#[derive(candid::CandidType, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventTimeRange {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(candid::CandidType, Deserialize, Default, Clone, Debug)]
pub struct GetEventsArg {
    pub start: u64,
    pub length: u64,
    /// OR-combined within the vec; AND with other filters. Empty vec or null
    /// means "no filter on type" — preserves the legacy behavior of hiding
    /// `AccrueInterest` and `PriceUpdate`. When non-empty, only events whose
    /// `type_filter` matches one of the variants are returned (including
    /// `AccrueInterest`/`PriceUpdate` if explicitly requested).
    #[serde(default)]
    pub types: Option<Vec<EventTypeFilter>>,
    /// Match against the event's owner / caller / liquidator / target principal
    /// using `Event::involves_principal`.
    #[serde(default)]
    pub principal: Option<Principal>,
    /// Collateral token ledger principal. For vault-id events, resolved by
    /// looking up the vault's collateral type at open time.
    #[serde(default)]
    pub collateral_token: Option<Principal>,
    #[serde(default)]
    pub time_range: Option<EventTimeRange>,
    /// Minimum event size in icUSD e8s (= USD e8s). ICP/collateral amounts are
    /// converted at the current spot price. Events with no meaningful size
    /// pass through.
    #[serde(default)]
    pub min_size_e8s: Option<u64>,
    /// Narrow `EventTypeFilter::Admin` matches to these specific admin labels
    /// (variant names, e.g. `"SetBorrowingFee"`). No-op when `Admin` isn't in
    /// `types` or when the list is empty/null. Non-admin events are never
    /// affected by this field.
    #[serde(default)]
    pub admin_labels: Option<Vec<String>>,
}

#[derive(candid::CandidType, Clone)]
pub struct GetEventsFilteredResponse {
    pub total: u64,
    pub events: Vec<(u64, crate::event::Event)>,
}

/// Response for `get_events_forward_filtered`: events matching the type filter
/// within the forward scan window, each paired with its GLOBAL event-log index,
/// plus a resume cursor. Drives gap-free, id-cursored incremental ingestion by an
/// off-chain/inter-canister poller (e.g. `rumi_points`), unlike
/// `get_events_filtered`, which is newest-first and paged by page number.
///
/// A poller ingests every matching event exactly once by passing `start = 0` and
/// then `start := next_start` on each call until `reached_end`, after which it
/// resumes from the same cursor as new events append (no gaps, no repeats).
#[derive(candid::CandidType, Clone)]
pub struct ForwardFilteredEventsResponse {
    /// Matching events as `(global_log_index, event)`, oldest-first.
    pub events: Vec<(u64, crate::event::Event)>,
    /// Global log index to pass as the next `start`.
    pub next_start: u64,
    /// True when the scan reached the tail of the log (poller is caught up).
    pub reached_end: bool,
}

/// Output cap on `get_vault_history` (DOS-001 legacy entry point) and
/// page-size cap on `get_vault_history_paged`. Bounds the per-call
/// reply size; for full historical access callers page via
/// `get_vault_history_paged`. Audit Wave 9a (DOS-001).
pub const MAX_VAULT_HISTORY: usize = 200;

/// Output cap on `get_events_by_principal` (DOS-003 legacy entry point):
/// the function returns the most recent matches in a bounded ring
/// buffer of this size. Audit Wave 9a (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_LEGACY: usize = 500;

/// Per-call scan-window cap on `get_events_by_principal_paged`. A
/// caller cannot scan more than this many event-log entries in a
/// single call — pages chain to cover larger ranges. Audit Wave 9a
/// (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_SCAN: u64 = 5_000;

/// Output cap on `get_events_by_principal_paged`: matches found in the
/// scan window beyond this count truncate (the caller resumes from
/// `scan_end`). Audit Wave 9a (DOS-003).
pub const MAX_EVENTS_BY_PRINCIPAL_OUTPUT: usize = 500;

/// Output cap on `get_all_vaults`, `get_vaults(None)`, and
/// `get_liquidatable_vaults` legacy entry points. Bounds the per-call
/// reply size; for full enumeration callers use the `*_page` paged
/// variants. Audit Wave 9a (DOS-004).
pub const MAX_VAULTS_LEGACY_PAGE: usize = 500;

/// Page-size cap on `get_vaults_page` and
/// `get_liquidatable_vaults_page`. Audit Wave 9a (DOS-004).
pub const MAX_VAULTS_PAGE_LIMIT: u64 = 500;

/// Wave-9b DOS-006: cache TTL for `get_protocol_status` aggregate
/// snapshot. Two consecutive query calls within this window serve the
/// same heavy fields (sum-over-vaults, weighted rate, per-collateral
/// totals) without re-aggregating. The 5-minute XRC tick refreshes
/// the cache as part of its existing vault walk; this 5-second TTL
/// covers cold start, post-upgrade, and the gap between ticks. Live
/// fields (mode, frozen, last_icp_rate, etc.) are NOT served from the
/// snapshot, see `main.rs::get_protocol_status` for the exact list.
pub const PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS: u64 = 5_000_000_000;

/// Wave-9b DOS-007: cache TTL for `get_treasury_stats`. Same rationale
/// as `PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS`. Heavy field cached:
/// `total_accrued_interest_system` (sum of `accrued_interest` across
/// every vault). All other fields in `TreasuryStats` are O(1) or
/// O(small) and read fresh on every call.
pub const TREASURY_STATS_SNAPSHOT_TTL_NANOS: u64 = 5_000_000_000;

/// Paginated response for `get_vault_history_paged`. `events` is the
/// page of matches in newest-first order within the requested window.
/// `total` is the total matched-event count for this vault so the
/// caller can render accurate page indicators. Audit Wave 9a (DOS-001).
#[derive(candid::CandidType, Clone)]
pub struct VaultHistoryPagedResponse {
    pub total: u64,
    pub events: Vec<(u64, crate::event::Event)>,
}

/// Paginated response for `get_events_by_principal_paged`. Cursor-based
/// pagination over the global event log: caller passes `scan_start` and
/// the response reports `scan_end` (resume offset for the next call)
/// plus an `exhausted` flag once the scan has reached `total_events`.
/// `events` are the matches found in the scanned window, in scan order.
/// Audit Wave 9a (DOS-003).
#[derive(candid::CandidType, Clone)]
pub struct EventsByPrincipalPagedResponse {
    pub events: Vec<(u64, crate::event::Event)>,
    pub scan_end: u64,
    pub exhausted: bool,
    pub total_events: u64,
}

/// A bot proof audit record in the private journal. `journal_index` is the
/// stable cursor position in that journal; `after_event_count` anchors replay
/// and relates the transition to the public event log without consuming a
/// public event index.
#[derive(candid::CandidType, Clone)]
pub struct BotProofAuditRecord {
    pub journal_index: u64,
    pub after_event_count: u64,
    pub event: crate::event::BotProofAuditEvent,
}

/// Bounded scan response for `get_bot_proof_audit_events`.
#[derive(candid::CandidType, Clone)]
pub struct BotProofAuditEventsResponse {
    pub events: Vec<BotProofAuditRecord>,
    pub scan_end: u64,
    pub exhausted: bool,
    pub total_journal_entries: u64,
}

/// Paginated response for `get_vaults_page` / `get_liquidatable_vaults_page`.
/// `vaults` is the page slice ordered by ascending `vault_id` starting at
/// `start_id`. `next_start_id` is `Some(id)` to continue paging, `None`
/// when the end of the map is reached. Audit Wave 9a (DOS-004).
#[derive(candid::CandidType, candid::Deserialize, Debug)]
pub struct VaultsPageResponse {
    pub vaults: Vec<crate::vault::CandidVault>,
    pub next_start_id: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct LiquidityStatus {
    pub liquidity_provided: u64,
    pub total_liquidity_provided: u64,
    pub liquidity_pool_share: f64,
    pub available_liquidity_reward: u64,
    pub total_available_returns: u64,
}

/// Read-only dump of all admin-settable protocol parameters in one call.
/// Returned by `get_protocol_config()` so operators can eyeball every threshold,
/// fee, ceiling, and collateral setting without multiple queries.
#[derive(CandidType, Deserialize, Debug)]
pub struct ProtocolConfig {
    // -- Protocol mode & safety --
    pub mode: Mode,
    pub frozen: bool,
    pub manual_mode_override: bool,

    // -- Global fees --
    pub borrowing_fee: f64,
    pub redemption_fee_floor: f64,
    pub redemption_fee_ceiling: f64,
    pub reserve_redemption_fee: f64,
    pub ckstable_repay_fee: f64,
    pub liquidation_bonus: f64,
    pub liquidation_protocol_share: f64,

    // -- RMR parameters --
    pub rmr_floor: f64,
    pub rmr_ceiling: f64,
    pub rmr_floor_cr: f64,
    pub rmr_ceiling_cr: f64,

    // -- Recovery mode --
    pub recovery_cr_multiplier: f64,
    pub recovery_mode_threshold: f64,
    pub max_partial_liquidation_ratio: f64,

    // -- Limits --
    pub min_icusd_amount: u64,
    pub global_icusd_mint_cap: u64,
    pub interest_flush_threshold_e8s: u64,

    // -- Interest split --
    pub interest_split: Vec<InterestSplitArg>,

    // -- Rate curves --
    pub global_rate_curve: Vec<(f64, f64)>,
    pub recovery_rate_curve: Vec<(String, f64)>,
    pub borrowing_fee_curve: Vec<(f64, f64)>,

    // -- Reserve redemptions --
    pub reserve_redemptions_enabled: bool,
    pub ckusdt_enabled: bool,
    pub ckusdc_enabled: bool,

    // -- Swap routing --
    /// Kill switch for ICPswap-backed swap routing. When false, frontend skips
    /// all ICPswap providers. Flipped via set_icpswap_routing_enabled.
    pub icpswap_routing_enabled: bool,

    // -- External principals --
    pub treasury_principal: Option<Principal>,
    pub stability_pool_canister: Option<Principal>,
    pub three_pool_canister: Option<Principal>,
    pub ckusdt_ledger_principal: Option<Principal>,
    pub ckusdc_ledger_principal: Option<Principal>,

    // -- Bot config --
    pub liquidation_bot_principal: Option<Principal>,
    pub bot_budget_total_e8s: u64,
    pub bot_budget_remaining_e8s: u64,
    pub bot_allowed_collateral_types: Vec<Principal>,
    pub bot_cr_tolerance_bps: u64,

    // -- Per-collateral configs (all collateral types) --
    pub collateral_configs: Vec<(Principal, state::CollateralConfig)>,
}

/// Per-collateral aggregate totals — lightweight alternative to fetching all vaults.
#[derive(CandidType, Deserialize, Debug)]
pub struct CollateralTotals {
    pub collateral_type: Principal,
    pub symbol: String,
    pub decimals: u8,
    pub total_collateral: u64, // Raw token units
    pub total_debt: u64,       // icUSD e8s
    pub vault_count: u64,
    pub price: f64, // Last USD price
}

/// Per-collateral data captured in each hourly protocol snapshot.
#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct CollateralSnapshot {
    pub collateral_type: Principal,
    pub total_collateral: u64,
    pub total_debt: u64,
    pub vault_count: u64,
    pub price: f64,
}

/// Hourly protocol snapshot for historical charts.
#[derive(CandidType, Deserialize, Serialize, Debug, Clone)]
pub struct ProtocolSnapshot {
    pub timestamp: u64,
    pub total_collateral_value_usd: u64,
    pub total_debt: u64,
    pub total_vault_count: u64,
    pub collateral_snapshots: Vec<CollateralSnapshot>,
}

#[derive(CandidType, Deserialize)]
pub struct GetSnapshotsArg {
    pub start: u64,
    pub length: u64,
}

/// Argument for adding a new collateral type via admin endpoint.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct AddCollateralArg {
    /// ICRC-1 ledger canister ID for the new collateral token
    pub ledger_canister_id: Principal,
    /// How to fetch the USD price (e.g., XRC with specific asset pair)
    pub price_source: state::PriceSource,
    /// Below this ratio, the vault can be liquidated (e.g., 1.33)
    pub liquidation_ratio: f64,
    /// Below this ratio, recovery mode triggers (e.g., 1.5)
    pub borrow_threshold_ratio: f64,
    /// Bonus multiplier for liquidators (e.g., 1.15)
    pub liquidation_bonus: f64,
    /// One-time fee at borrow/mint time (e.g., 0.005)
    pub borrowing_fee: f64,
    /// Maximum total debt for this collateral (u64::MAX = no cap)
    pub debt_ceiling: u64,
    /// Minimum vault debt (dust threshold)
    pub min_vault_debt: u64,
    /// Ongoing annual interest rate (e.g., 0.02 = 2% APR)
    pub interest_rate_apr: f64,
    /// Minimum collateral deposit in native token units (e.g., 100_000 for 0.001 ICP)
    pub min_collateral_deposit: u64,
    /// Hex color for frontend display (e.g., "#F7931A")
    pub display_color: Option<String>,
    /// Minimum redemption fee (floor), e.g., 0.005 = 0.5%
    pub redemption_fee_floor: Option<f64>,
    /// Maximum redemption fee (ceiling), e.g., 0.05 = 5%
    pub redemption_fee_ceiling: Option<f64>,
    /// Redemption priority tier (1/2/3). Default: 1 if omitted.
    pub redemption_tier: Option<u8>,
}

#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum ProtocolError {
    TransferFromError(TransferFromError, u64),
    TransferError(TransferError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    AnonymousCallerNotAllowed,
    CallerNotOwner,
    AmountTooLow {
        minimum_amount: u64,
    },
    GenericError(String),
    /// Wave-8b LIQ-002 band-gate rejection. **Deactivated 2026-05-18 — no
    /// live code path emits this variant.** Retained in the enum so
    /// historical on-chain events recorded before the deactivation still
    /// decode cleanly. See `state::is_within_liquidation_band` and the
    /// "Layer 2.5 — band gate DEACTIVATION fence" comment in
    /// `tests/audit_pocs_liq_002_sorted_troves_index.rs` for background.
    NotLowestCR,
    /// Phase 1a: the periodic supply-invariant self-check (Timer B) caught
    /// a `sum(chain_supplies) != total_debt` divergence. Every entry that
    /// touches debt or chain supply returns this error until an operator
    /// clears `multi_chain.invariant_halted`.
    SupplyInvariantHalted,
    /// Phase 1a: admin-endpoint error for `register_chain`, `disable_chain`,
    /// `set_chain_config`. Wraps a developer-facing message string. The
    /// structured `ChainAdminError` enum lives in `chains::config` and is
    /// stringified here so the Candid surface stays append-only.
    ChainAdmin(String),
    /// M2 EVM-native self-serve auth failure (bad signature, recovered signer !=
    /// owner, nonce replay, expired deadline, recipient != owner, per-owner cap,
    /// unknown/unregistered chain, custody-derive failure, or an underlying vault
    /// rejection). Wraps a developer-facing message. Appended AFTER `ChainAdmin`
    /// so historical on-chain events keep decoding (append-only Candid surface).
    EvmAuth(String),
}

/// Structured errors for the quote-based redemption API. Keeping these
/// variants separate from `ProtocolError` preserves the established error
/// shape used by the existing public methods.
#[derive(CandidType, Debug, Clone, Deserialize)]
pub enum RedemptionError {
    /// A truthful vault-collateral redemption quote could not be produced.
    RedemptionQuoteUnavailable(String),
    /// Requested input exceeds the first globally ordered collateral run.
    RedemptionCapacityExceeded { max_input_icusd_e8s: u64 },
    /// The globally first collateral changed after the quote was observed.
    RedemptionPriorityChanged {
        expected: Principal,
        actual: Principal,
    },
    /// Actual backed native-token payout cannot satisfy the caller's bound.
    RedemptionMinimumNotMet {
        minimum_net_raw: u64,
        actual_net_raw: u64,
    },
    /// An existing protocol error such as authorization, transfer, or amount validation.
    Protocol(ProtocolError),
}

impl From<ProtocolError> for RedemptionError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<GuardError> for RedemptionError {
    fn from(error: GuardError) -> Self {
        Self::Protocol(ProtocolError::from(error))
    }
}

impl From<GuardError> for ProtocolError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
            GuardError::StaleOperation => {
                Self::TemporarilyUnavailable("previous operation is being cleaned up".to_string())
            }
        }
    }
}

impl ProtocolError {
    /// The exact `TemporarilyUnavailable` rejection returned whenever the
    /// protocol is latched `Mode::ReadOnly` (insolvency: total collateral ratio
    /// below 100%, or the deficit account over its configured threshold).
    ///
    /// Single source of truth so the Candid entry-layer gate
    /// (`main.rs::validate_mode`) and the shared vault-module redemption gates
    /// (`vault::redeem_collateral` / `vault::redeem_reserves`) return a
    /// byte-identical error. Audit RED-101 (regression of RED-003) showed that a
    /// second entry point (`redeem_icp`) silently bypassed the entry-layer-only
    /// gate; gating inside the vault module closes any present/future redemption
    /// surface by construction, and this constructor keeps every layer's message
    /// in lockstep.
    pub fn read_only_mode() -> Self {
        ProtocolError::TemporarilyUnavailable(
            "protocol temporarly unavailable, please wait for an upgrade or for total collateral ratio to go above 100%"
                .to_string(),
        )
    }
}

/// Candid-compatible struct matching the stability pool's and bot's `LiquidatableVaultInfo`.
/// Defined inline to avoid a crate dependency between backend and pool/bot.
#[derive(CandidType, Clone, Debug, Deserialize)]
pub struct LiquidatableVaultInfo {
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub debt_amount: u64,
    pub collateral_amount: u64,
    pub recommended_liquidation_amount: u64,
    pub collateral_price_e8s: u64,
}

/// Wave-14a CDP-10: post-spawn handler for the stability_pool
/// `notify_liquidatable_vaults` call.
///
/// On `Ok`: marks every dispatched vault id as SP-attempted so the next
/// `check_vaults` tick won't re-send it (the SP retry budget is one shot
/// per unhealthy episode).
///
/// On `Err`: leaves `sp_attempted_vaults` unchanged so the next tick can
/// retry, and returns a `StabilityPoolCallFailed` event so external
/// liquidators polling `get_events` can react. This closes the pre-fix
/// regression where a transport `Err` (cycle pressure, queue-full during
/// a market crash) permanently blacklisted the vault from the SP path
/// even though no liquidator had actually been notified.
///
/// Pure-state function: the caller is responsible for `record_event` if
/// the return is `Some`.
pub fn record_sp_notification_result(
    state: &mut state::State,
    vault_ids: Vec<u64>,
    result: Result<(), (i32, String)>,
) -> Option<event::Event> {
    record_sp_notification_result_at(state, vault_ids, result, ic_cdk::api::time())
}

/// Pure-state variant taking an explicit `now_ns` for tests. Production
/// callers go through `record_sp_notification_result`.
pub fn record_sp_notification_result_at(
    state: &mut state::State,
    vault_ids: Vec<u64>,
    result: Result<(), (i32, String)>,
    now_ns: u64,
) -> Option<event::Event> {
    if vault_ids.is_empty() {
        return None;
    }
    match result {
        Ok(()) => {
            for vid in &vault_ids {
                state.sp_attempted_vaults.insert(*vid);
            }
            None
        }
        Err((reject_code, reject_message)) => Some(event::Event::StabilityPoolCallFailed {
            vault_ids,
            reject_code,
            reject_message,
            timestamp: now_ns,
        }),
    }
}

/// Prune routing-state entries (`sp_attempted_vaults`, `bot_pending_vaults`)
/// for vaults that have either recovered above their liquidation floor or
/// timed out of their bot window.
///
/// Designed to be called from `check_vaults` UNCONDITIONALLY on every
/// tick, including ticks where no vault is currently liquidatable. On a
/// quiet tick `unhealthy_ids` is empty, so both `retain` predicates
/// evaluate false for every entry and both collections are flushed.
///
/// # Background (2026-05-18 follow-up)
///
/// Pre-fix, this cleanup lived INLINE inside `check_vaults` and only ran
/// inside the `if !unhealthy_vaults.is_empty()` branch. The 2026-05-17
/// incident exposed the consequence: vaults #149/#179/#182/#183 were
/// SP-attempted, the now-deactivated `NotLowestCR` band gate rejected
/// every call, the SP transport still returned `Ok(())` so
/// `record_sp_notification_result` blacklisted them, and after price
/// recovery the blacklist did not clear because the cleanup was skipped
/// on every quiet tick that followed.
///
/// `bot_pending_vaults` entries past `bot_timeout_ns` are dropped here
/// even if the vault is still unhealthy — the routing logic in
/// `check_vaults` reads absence-from-map as the signal to fall back from
/// the bot to the stability pool.
///
/// Pure-state. No I/O.
pub fn prune_recovered_routing_state(
    state: &mut state::State,
    unhealthy_ids: &std::collections::BTreeSet<u64>,
    now_ns: u64,
    bot_timeout_ns: u64,
) {
    state.bot_pending_vaults.retain(|vid, ts| {
        unhealthy_ids.contains(vid) && now_ns.saturating_sub(*ts) < bot_timeout_ns
    });
    state
        .sp_attempted_vaults
        .retain(|vid| unhealthy_ids.contains(vid));
}

/// Wave-14b CDP-03: write the post-redemption base rate and timestamp to
/// the per-collateral `CollateralConfig` (NOT the legacy global
/// `s.current_base_rate` / `s.last_redemption_time`).
///
/// Pre-Wave-14b the redemption path wrote to the global fields, which
/// meant a redemption against ckBTC corrupted the rate that subsequently
/// priced ICP redemptions, leaking value across collateral types.
///
/// Pure-state function. Silently no-ops for unknown collateral types
/// (the surrounding redemption path already validated the collateral via
/// `get_collateral_price_decimal`, but defense in depth is cheap).
pub fn record_per_collateral_redemption_fee(
    state: &mut state::State,
    collateral_type: &candid::Principal,
    base_fee: numeric::Ratio,
    now_ns: u64,
) {
    if let Some(config) = state.collateral_configs.get_mut(collateral_type) {
        config.current_base_rate = base_fee;
        config.last_redemption_time = now_ns;
    }
}

fn bot_claim_return_is_releasable(claim: &crate::state::BotClaim) -> bool {
    let conserving_outbound = claim
        .collateral_transfer
        .as_ref()
        .filter(|transfer| transfer.block_index.is_some())
        .and_then(|transfer| transfer.net_amount_e8s.zip(transfer.fee_e8s))
        .is_some_and(|(net, fee)| net.checked_add(fee) == Some(claim.collateral_amount));
    let exact_return = claim
        .collateral_return_proof
        .as_ref()
        .is_some_and(|proof| proof.amount == claim.collateral_amount);
    conserving_outbound && exact_return
}

#[cfg(test)]
mod bot_claim_return_tests {
    use super::bot_claim_return_is_releasable;
    use crate::state::{
        BotClaim, BotCollateralReturnProof, BotCollateralTransfer, BotCollateralTransferStatus,
        BotMemoVersion,
    };
    use candid::Principal;

    fn claim(net: Option<u64>, fee: Option<u64>, returned: Option<u64>) -> BotClaim {
        BotClaim {
            vault_id: 7,
            generation: 42,
            memo_version: Some(BotMemoVersion::V2),
            collateral_transfer: Some(BotCollateralTransfer {
                op_nonce: 1,
                created_at_time: 1,
                block_index: Some(3),
                net_amount_e8s: net,
                fee_e8s: fee,
                status: BotCollateralTransferStatus::Confirmed,
            }),
            prior_collateral_transfer: None,
            payment_ledger_principal: None,
            collateral_amount: 100,
            debt_amount: 1,
            collateral_type: Principal::from_slice(&[10]),
            claimed_at: 1,
            collateral_price_e8s: 1,
            collateral_return_proof: returned.map(|amount| BotCollateralReturnProof {
                block_index: 4,
                amount,
                created_at_time: 2,
            }),
        }
    }

    #[test]
    fn auto_cancel_requires_conserving_outbound_terms_and_exact_gross_return() {
        assert!(bot_claim_return_is_releasable(&claim(
            Some(90),
            Some(10),
            Some(100)
        )));
        assert!(!bot_claim_return_is_releasable(&claim(
            Some(90),
            Some(10),
            Some(99)
        )));
        assert!(!bot_claim_return_is_releasable(&claim(
            None,
            None,
            Some(100)
        )));
        assert!(!bot_claim_return_is_releasable(&claim(
            Some(89),
            Some(10),
            Some(100)
        )));
    }
}

pub async fn check_vaults() {
    // Auto-cancel bot claims that have been pending too long (10 minutes).
    // This prevents vaults from being permanently locked if the bot crashes.
    //
    // CL-02: a pooled default-account balance cannot prove that this claim's
    // collateral was returned. Only an exact return of gross collateral C,
    // with a conserving persisted outbound net+fee tuple, may release its lock
    // or restore its budget. Missing legacy proof data is held for reconciliation.
    //
    // The guard re-emits the event on every tick the gate fires (no
    // per-claim "already emitted" flag, since a state-shape change is
    // out of scope for this wave). The explorer can group by `vault_id`
    // to dedupe; operator action is unchanged regardless of count.
    const BOT_CLAIM_TIMEOUT_NS: u64 = 600_000_000_000; // 10 minutes
    let now = ic_cdk::api::time();
    let expired_claims: Vec<(u64, crate::state::BotClaim)> = read_state(|s| {
        s.bot_claims
            .iter()
            .filter(|(_, claim)| now.saturating_sub(claim.claimed_at) >= BOT_CLAIM_TIMEOUT_NS)
            .map(|(vid, claim)| (*vid, claim.clone()))
            .collect()
    });

    for (vault_id, claim) in &expired_claims {
        let required = claim.collateral_amount;

        if !bot_claim_return_is_releasable(claim) {
            log!(
                INFO,
                "[CL-02] auto-cancel deferred for vault #{}: exact gross return proof or conserving outbound terms missing for claim generation {}",
                vault_id,
                claim.generation
            );
            mutate_state(|s| {
                if s.bot_claims.get(vault_id).is_some_and(|active| {
                    active.generation == claim.generation
                        && !bot_claim_return_is_releasable(active)
                }) {
                    crate::event::record_bot_claim_reconciliation_needed(
                        s, *vault_id, 0, required,
                    );
                }
            });
            continue;
        }

        // The proof was checked against native query_blocks or ICRC-3 and
        // persisted by bot_record_collateral_return_proof. It is bound to this claim
        // generation and must still match before cleanup.
        let still_same_claim = read_state(|s| {
            s.bot_claims.get(vault_id).is_some_and(|active| {
                active.generation == claim.generation
                    && active.collateral_return_proof == claim.collateral_return_proof
                    && bot_claim_return_is_releasable(active)
            })
        });
        if !still_same_claim {
            continue;
        }
        log!(
            INFO,
            "[check_vaults] Auto-cancelling returned bot claim for vault #{} (claimed {}s ago, verified exact collateral return)",
            vault_id,
            (now - claim.claimed_at) / 1_000_000_000
        );

        mutate_state(|s| {
            // Re-check the generation and proof before clearing state so a
            // newer claim can never be released by stale return evidence.
            if !s.bot_claims.get(vault_id).is_some_and(|active| {
                active.generation == claim.generation
                    && active.collateral_return_proof == claim.collateral_return_proof
                    && bot_claim_return_is_releasable(active)
            }) {
                return;
            }
            if let Some(vault) = s.vault_id_to_vaults.get_mut(vault_id) {
                vault.bot_processing = false;
            }
            s.bot_budget_remaining_e8s += claim.debt_amount;
            s.bot_claims.remove(vault_id);
        });
    }

    // Wave-10 LIQ-008: short-circuit the auto-publishing path when the
    // mass-liquidation circuit breaker is tripped. Bot-claim auto-cancel
    // above runs unconditionally because it is hygiene, not auto-publishing.
    // Manual liquidation endpoints (`liquidate_vault`, `liquidate_vault_partial`,
    // `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`,
    // `liquidate_vault_debt_already_burned`) do not consult the breaker.
    if read_state(|s| s.liquidation_breaker_tripped) {
        log!(
            INFO,
            "[LIQ-008] check_vaults skipping notify (breaker tripped). Manual liquidation remains available."
        );
        return;
    }

    let dummy_rate = read_state(|s| {
        s.last_icp_rate.unwrap_or_else(|| {
            log!(
                INFO,
                "[check_vaults] No ICP rate available, using default rate"
            );
            UsdIcp::from(dec!(1.0))
        })
    });

    // Only identify unhealthy vaults but don't liquidate them
    //
    // Wave-8b LIQ-002: walk `vault_cr_index` ascending so the bot / stability
    // pool receive worst-CR vaults first. The list of underwater vaults is
    // unchanged; the order is now sorted by CR ascending. This matches the
    // server-side band-gate behavior — the bot/pool see the same vault the
    // band gate would accept first.
    //
    // Wave-9c DOS-005: bound the walk to the at-risk band on most ticks.
    // `advance_check_vaults_tick` returns true on the Nth tick (default
    // every 12 = once per hour at the 5-min cadence), making that tick a
    // full sweep. This is the safety belt for cross-collateral CR-key
    // drift: a vault whose key is stale-above-threshold is missed by
    // band-only ticks but caught by the next full sweep. Tunable via
    // `set_check_vaults_alert_band_bps` and
    // `set_check_vaults_full_sweep_every_n_ticks`.
    let do_full_sweep = mutate_state(|s| s.advance_check_vaults_tick());
    let scan = read_state(|s| s.scan_unhealthy_vaults(dummy_rate, do_full_sweep));
    log!(
        INFO,
        "[check_vaults] {} tick: visited {} vault(s), threshold_key={}, found {} unhealthy",
        if scan.was_full_sweep {
            "full-sweep"
        } else {
            "band-only"
        },
        scan.vaults_visited,
        scan.threshold_key,
        scan.unhealthy_vaults.len(),
    );
    let unhealthy_vaults = scan.unhealthy_vaults;

    // 2026-05-18 follow-up: prune routing state unconditionally on every
    // tick — even quiet ones — so a vault that was SP-attempted during a
    // prior unhealthy episode and has since recovered does not stay
    // blacklisted forever. Pre-fix this cleanup was inline INSIDE the
    // `if !unhealthy_vaults.is_empty()` branch and was skipped on quiet
    // ticks; the 5/17 incident exposed the consequence (vaults
    // #149/#179/#182/#183 stuck on the SP blacklist until manual
    // liquidation). See `prune_recovered_routing_state` doc.
    let now = ic_cdk::api::time();
    let bot_timeout_ns: u64 = 300_000_000_000; // 5 min
    let scan_unhealthy_ids: std::collections::BTreeSet<u64> =
        unhealthy_vaults.iter().map(|v| v.vault_id).collect();
    mutate_state(|s| {
        prune_recovered_routing_state(s, &scan_unhealthy_ids, now, bot_timeout_ns);
    });

    // Log unhealthy vaults but don't liquidate them
    if !unhealthy_vaults.is_empty() {
        log!(
            INFO,
            "[check_vaults] Found {} liquidatable vaults. Waiting for external liquidators.",
            unhealthy_vaults.len()
        );

        // Log detailed information about each unhealthy vault
        for vault in &unhealthy_vaults {
            let (ratio, min_ratio) = read_state(|s| {
                (
                    compute_collateral_ratio(vault, dummy_rate, s),
                    s.get_min_liquidation_ratio_for(&vault.collateral_type),
                )
            });
            log!(
                INFO,
                "[check_vaults] Liquidatable vault #{}: owner={}, borrowed={}, collateral={}, ratio={:.2}%, min_ratio={:.2}%",
                vault.vault_id,
                vault.owner,
                vault.borrowed_icusd_amount,
                vault.collateral_amount,
                ratio.to_f64() * 100.0,
                min_ratio.to_f64() * 100.0
            );
        }

        // Build enriched notification payload
        let vault_notifications: Vec<LiquidatableVaultInfo> = read_state(|s| {
            unhealthy_vaults
                .iter()
                .map(|v| {
                    let collateral_price_usd = s
                        .get_collateral_price_decimal(&v.collateral_type)
                        .map(|p| UsdIcp::from(p))
                        .unwrap_or(UsdIcp::from(rust_decimal::Decimal::ZERO));
                    let optimal_liq = s.recommended_liquidation_amount_for(v, collateral_price_usd);
                    LiquidatableVaultInfo {
                        vault_id: v.vault_id,
                        collateral_type: v.collateral_type,
                        debt_amount: v.borrowed_icusd_amount.to_u64(),
                        collateral_amount: v.collateral_amount,
                        recommended_liquidation_amount: optimal_liq.to_u64(),
                        collateral_price_e8s: collateral_price_usd.to_e8s(),
                    }
                })
                .collect()
        });

        // ── Priority-ordered liquidation cascade ──
        // 1. Bot gets first shot at vaults with bot-eligible collateral
        // 2. Stability pool handles: non-bot-eligible immediately + bot-eligible after timeout
        // 3. Manual liquidation is always available as last resort (via get_liquidatable_vaults)
        //
        // `now` and `bot_timeout_ns` were established above the `if` block
        // for the unconditional `prune_recovered_routing_state` call;
        // re-used here for the cascade decisions.

        let (bot_canister, pool_canister) =
            read_state(|s| (s.liquidation_bot_principal, s.stability_pool_canister));

        let mut for_bot: Vec<LiquidatableVaultInfo> = Vec::new();
        let mut for_pool: Vec<LiquidatableVaultInfo> = Vec::new();

        for vault_info in &vault_notifications {
            // `vault_routable_to_bot` enforces both the operator allowlist and
            // the custody fence (native-XRP is claim-based and must never go
            // to the bot; it falls through to the SP's absorb path).
            let bot_eligible = bot_canister.is_some()
                && read_state(|s| s.vault_routable_to_bot(&vault_info.collateral_type));

            let sp_already_tried =
                read_state(|s| s.sp_attempted_vaults.contains(&vault_info.vault_id));

            if sp_already_tried {
                // SP already had its shot → manual only, skip entirely
                continue;
            }

            if !bot_eligible {
                // Not bot-eligible → stability pool (one shot)
                for_pool.push(vault_info.clone());
            } else {
                // Bot-eligible: check if we already sent it and it timed out
                let pending_since =
                    read_state(|s| s.bot_pending_vaults.get(&vault_info.vault_id).copied());
                match pending_since {
                    None => {
                        // First time seeing this vault → send to bot
                        for_bot.push(vault_info.clone());
                    }
                    Some(ts) if now.saturating_sub(ts) >= bot_timeout_ns => {
                        // Bot had its chance and didn't liquidate → fallback to pool (one shot)
                        log!(
                            INFO,
                            "[check_vaults] Bot timeout for vault #{}, falling back to stability pool",
                            vault_info.vault_id
                        );
                        for_pool.push(vault_info.clone());
                    }
                    Some(_) => {
                        // Still within bot's window → re-send to bot
                        for_bot.push(vault_info.clone());
                    }
                }
            }
        }

        // Update tracking state.
        //
        // The `prune_recovered_routing_state` call above already retained
        // only the entries whose vault is still in the scan's
        // `unhealthy_ids` set. The remaining work here is to insert the
        // newly-routed bot vaults so the timeout window starts ticking.
        // The SP-attempted insertion is done inside the spawn's Ok arm via
        // `record_sp_notification_result` (Wave-14a CDP-10), not here.
        let bot_vault_ids: Vec<u64> = for_bot.iter().map(|v| v.vault_id).collect();
        let pool_vault_ids: Vec<u64> = for_pool.iter().map(|v| v.vault_id).collect();

        mutate_state(|s| {
            for vid in &bot_vault_ids {
                s.bot_pending_vaults.entry(*vid).or_insert(now);
            }
        });

        // Push to bot (fire-and-forget with error logging)
        if let Some(bot) = bot_canister {
            if !for_bot.is_empty() {
                let count = for_bot.len();
                ic_cdk::spawn(async move {
                    let result: Result<(), _> =
                        ic_cdk::call(bot, "notify_liquidatable_vaults", (for_bot,)).await;
                    if let Err((code, msg)) = result {
                        log!(
                            INFO,
                            "[check_vaults] ERROR: bot notification failed: {:?} {}",
                            code,
                            msg
                        );
                    }
                });
                log!(
                    INFO,
                    "[check_vaults] Sent {} bot-eligible vaults to bot {}",
                    count,
                    bot
                );
            }
        }

        // Push to stability pool (fire-and-forget; Wave-14a CDP-10 routes the
        // result through `record_sp_notification_result` so the SP-attempted
        // marker is only set when the call actually delivered.)
        if let Some(pool) = pool_canister {
            if !for_pool.is_empty() {
                let count = for_pool.len();
                let dispatched_ids = pool_vault_ids.clone();
                ic_cdk::spawn(async move {
                    let result: Result<(), _> =
                        ic_cdk::call(pool, "notify_liquidatable_vaults", (for_pool,)).await;
                    let normalized: Result<(), (i32, String)> =
                        result.map_err(|(code, msg)| (code as i32, msg));
                    if let Err((code, msg)) = &normalized {
                        log!(
                            INFO,
                            "[check_vaults] ERROR: stability pool notification failed: {} {}",
                            code,
                            msg
                        );
                    }
                    let event = mutate_state(|s| {
                        record_sp_notification_result(s, dispatched_ids, normalized)
                    });
                    if let Some(ev) = event {
                        crate::storage::record_event(&ev);
                    }
                });
                log!(
                    INFO,
                    "[check_vaults] Sent {} vaults to stability pool {} (non-bot-eligible or bot timeout)",
                    count,
                    pool
                );
            }
        }
    } else {
        log!(
            DEBUG,
            "[check_vaults] All vaults are healthy at the current ICP rate: {}",
            dummy_rate.to_f64()
        );
    }

    // No longer calling record_liquidate_vault to trigger automatic liquidations
}

/// Compute collateral ratio for a vault using per-collateral price and decimals.
/// Returns Ratio::ZERO when price or config is unavailable — callers must
/// independently check `last_price.is_some()` before performing operations.
pub fn compute_collateral_ratio(vault: &Vault, _rate: UsdIcp, state: &state::State) -> Ratio {
    if vault.borrowed_icusd_amount == 0 {
        return Ratio::from(Decimal::MAX);
    }
    let margin_value: ICUSD =
        if let Some(config) = state.get_collateral_config(&vault.collateral_type) {
            if let Some(price) = config.last_price {
                let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
                numeric::collateral_usd_value(vault.collateral_amount, price_dec, config.decimals)
            } else {
                // No price available — return zero ratio (conservative / safe direction).
                // Operations must independently check last_price.is_some() and error out.
                return Ratio::from(Decimal::ZERO);
            }
        } else {
            // No config — return zero ratio. This vault's collateral type is unknown.
            return Ratio::from(Decimal::ZERO);
        };
    margin_value / vault.borrowed_icusd_amount
}

fn note_pending_payout_failure(
    row: &mut crate::state::PendingMarginTransfer,
    error: &crate::management::DurableTransferError,
) {
    row.in_flight = false;
    match error {
        crate::management::DurableTransferError::LedgerNoEffect(TransferError::TooOld) => {
            row.held_for_manual_retry = true;
            row.reconciliation_required = true;
            row.too_old_confirmed = true;
        }
        crate::management::DurableTransferError::LedgerNoEffect(TransferError::BadFee {
            ..
        }) => {
            // The original net amount and attempt identity are immutable. A changed
            // fee needs owner review; changing amounts under the old nonce is unsafe.
            row.held_for_manual_retry = true;
        }
        crate::management::DurableTransferError::AmbiguousCall { .. }
        | crate::management::DurableTransferError::AmbiguousResponse(_) => {
            // The ledger may have committed before the reply was lost. Keep the
            // same attempt held until exact reconciliation evidence is supplied.
            row.held_for_manual_retry = true;
            row.reconciliation_required = true;
        }
        crate::management::DurableTransferError::LedgerNoEffect(_) => {
            row.retry_count = row.retry_count.saturating_add(1);
            if row.retry_count >= MAX_PENDING_RETRIES {
                row.held_for_manual_retry = true;
            }
        }
    }
}

fn pending_payout_is_dispatchable(transfer: &crate::state::PendingMarginTransfer) -> bool {
    !transfer.held_for_manual_retry
        && !transfer.reconciliation_required
        && !transfer.in_flight
        && transfer.op_nonce != 0
        && transfer.ledger.is_some()
        && transfer.transfer_amount_raw.is_some()
}

fn reserve_payout_journal_mismatch_requires_hold(
    transfer: &crate::state::PendingMarginTransfer,
    journal_matches: bool,
) -> bool {
    pending_payout_is_dispatchable(transfer) && !journal_matches
}

pub async fn process_one_pending_payout(operation_id: u128) {
    let transfer = mutate_state(|s| {
        let Some((_, transfer)) = s.get_pending_payout(operation_id) else {
            return None;
        };
        if !pending_payout_is_dispatchable(&transfer) {
            return None;
        }
        if let Some(key) = s.three_usd_reserve_payout_operation_keys.get(&operation_id) {
            let journal_matches = s.three_usd_reserve_collateral_payouts.get(key).is_some_and(|payout| {
                let attempt = if payout.op_nonce == transfer.op_nonce {
                    Some((payout.fee_arg_e8s, payout.candidate_block_index))
                } else {
                    payout.rearmed_attempts.iter()
                        .find(|attempt| attempt.op_nonce == transfer.op_nonce)
                        .map(|attempt| (attempt.fee_arg_e8s, attempt.candidate_block_index))
                };
                payout.operation_id == operation_id
                    && payout.ledger == transfer.ledger.unwrap_or(Principal::anonymous())
                    && payout.destination.owner == transfer.owner
                    && payout.destination.subaccount.is_none()
                    && payout.net_e8s == transfer.transfer_amount_raw.unwrap_or_default()
                    && payout.gross_e8s == transfer.margin.to_u64()
                    && attempt.is_some_and(|(fee_arg, candidate)| {
                        fee_arg == Some(payout.expected_fee_e8s) && candidate.is_none()
                    })
            });
            if reserve_payout_journal_mismatch_requires_hold(&transfer, journal_matches) {
                s.mutate_pending_payout(operation_id, |row| {
                    row.in_flight = false;
                    row.held_for_manual_retry = true;
                    row.reconciliation_required = true;
                });
                return None;
            }
        }
        if transfer.retry_count >= MAX_PENDING_RETRIES {
            s.mutate_pending_payout(operation_id, |row| row.held_for_manual_retry = true);
            return None;
        }
        s.mutate_pending_payout(operation_id, |row| row.in_flight = true);
        Some(transfer)
    });
    let Some(transfer) = transfer else {
        return;
    };
    if !crate::payout_history::capture_dispatch_boundary(operation_id, transfer).await {
        crate::payout_history::hold_failed_dispatch_preflight(operation_id, transfer);
        return;
    }
    let ledger = transfer.ledger.expect("validated payout ledger");
    let amount = transfer
        .transfer_amount_raw
        .expect("validated payout amount");
    let reserve_fee_arg = read_state(|state| {
        state.three_usd_reserve_payout_operation_keys
            .get(&operation_id)
            .and_then(|key| state.three_usd_reserve_collateral_payouts.get(key))
            .and_then(|payout| {
                if payout.op_nonce == transfer.op_nonce {
                    Some(payout.fee_arg_e8s)
                } else {
                    payout.rearmed_attempts.iter()
                        .find(|attempt| attempt.op_nonce == transfer.op_nonce)
                        .map(|attempt| attempt.fee_arg_e8s)
                }
            })
    });
    let result = crate::management::transfer_collateral_with_nonce_and_fee_status(
        amount,
        reserve_fee_arg.flatten(),
        transfer.owner,
        ledger,
        transfer.op_nonce,
    )
    .await;
    match result {
        Ok(block_index) => {
            // Reserve-linked payouts keep their exact tuple and the ledger's
            // returned index in the private replay journal before any proof
            // await or public event removes the generic pending row.
            let reserve_candidate = mutate_state(|state| {
                let Some(key) = state.three_usd_reserve_payout_operation_keys
                    .get(&operation_id).cloned() else { return None };
                let Some(saved) = state.three_usd_reserve_collateral_payouts.get(&key) else {
                    return None;
                };
                let mut payout = saved.clone();
                if payout.op_nonce != transfer.op_nonce {
                    let Some(attempt) = payout.rearmed_attempts.iter()
                        .find(|attempt| attempt.op_nonce == transfer.op_nonce) else { return None };
                    payout.op_nonce = attempt.op_nonce;
                    payout.memo = attempt.memo;
                    payout.created_at_time_ns = attempt.created_at_time_ns;
                    payout.fee_arg_e8s = attempt.fee_arg_e8s;
                    payout.candidate_block_index = attempt.candidate_block_index;
                    payout.observed_fee_e8s = attempt.observed_fee_e8s;
                }
                if payout.operation_id != operation_id || payout.ledger != ledger
                    || payout.destination.owner != transfer.owner
                    || payout.net_e8s != amount || payout.op_nonce != transfer.op_nonce
                {
                    return None;
                }
                if !crate::event::record_three_usd_reserve_payout_candidate(
                    state, key.clone(), operation_id, transfer.op_nonce, block_index,
                ) {
                    return None;
                }
                Some((key, payout.clone()))
            });
            let reserve_fee_observation = if let Some((key, payout)) = reserve_candidate {
                match verify_three_usd_reserve_payout_fee(&payout, block_index).await {
                    Ok(actual_fee_e8s) => Some((key, actual_fee_e8s)),
                    Err(reason) => {
                        mutate_state(|state| {
                            state.mutate_pending_payout(operation_id, |row| {
                                row.in_flight = false;
                                row.held_for_manual_retry = true;
                                row.reconciliation_required = true;
                            });
                        });
                        log!(INFO, "3USD reserve payout {} candidate {} remains held pending direct fee evidence: {}", operation_id, block_index, reason);
                        return;
                    }
                }
            } else if read_state(|state| state.three_usd_reserve_payout_operation_keys.contains_key(&operation_id)) {
                mutate_state(|state| {
                    state.mutate_pending_payout(operation_id, |row| {
                        row.in_flight = false;
                        row.held_for_manual_retry = true;
                        row.reconciliation_required = true;
                    });
                });
                log!(INFO, "3USD reserve payout {} returned a block but its exact durable tuple did not match; payout remains held", operation_id);
                return;
            } else {
                None
            };
            mutate_state(|s| match transfer.payout_kind {
                crate::state::PendingPayoutKind::Redemption => {
                    let burn_index = s
                        .pending_payout_index
                        .get(&operation_id)
                        .and_then(|locator| locator.redemption_block_index);
                    if let Some(burn_index) = burn_index {
                        crate::event::record_redemption_transfered(
                            s,
                            burn_index,
                            operation_id,
                            block_index,
                        );
                    }
                }
                kind => {
                    if let Some((key, actual_fee_e8s)) = reserve_fee_observation {
                        if !crate::event::record_three_usd_reserve_payout_fee_observed(
                            s, key, operation_id, transfer.op_nonce, block_index, actual_fee_e8s,
                        ) {
                            s.mutate_pending_payout(operation_id, |row| {
                                row.in_flight = false;
                                row.held_for_manual_retry = true;
                                row.reconciliation_required = true;
                            });
                            return;
                        }
                        let expected_fee_e8s = s.three_usd_reserve_payout_operation_keys
                            .get(&operation_id)
                            .and_then(|key| s.three_usd_reserve_collateral_payouts.get(key))
                            .map(|payout| payout.expected_fee_e8s);
                        if !three_usd_reserve_fee_matches(expected_fee_e8s, actual_fee_e8s) {
                            s.mutate_pending_payout(operation_id, |row| {
                                row.in_flight = false;
                                row.held_for_manual_retry = true;
                                row.reconciliation_required = true;
                            });
                            log!(INFO, "3USD reserve payout {} has observed fee {} differing from pinned expected fee {:?}; completion remains held", operation_id, actual_fee_e8s, expected_fee_e8s);
                            return;
                        }
                    }
                    crate::event::record_margin_transfer(
                        s, transfer.vault_id, transfer.owner, operation_id, kind, block_index,
                    )
                }
            });
        }
        Err(error) => {
            if matches!(
                &error,
                crate::management::DurableTransferError::LedgerNoEffect(TransferError::TooOld)
            ) {
                mutate_state(|s| {
                    crate::event::record_pending_payout_too_old(s, operation_id, transfer.op_nonce);
                });
            } else {
                mutate_state(|s| {
                    if matches!(
                        &error,
                        crate::management::DurableTransferError::AmbiguousCall { .. }
                            | crate::management::DurableTransferError::AmbiguousResponse(_)
                    ) {
                        crate::event::record_pending_payout_ambiguous_outcome(
                            s,
                            operation_id,
                            transfer.op_nonce,
                        );
                    } else {
                        s.mutate_pending_payout(operation_id, |row| {
                            note_pending_payout_failure(row, &error);
                        });
                    }
                });
            }
            if let crate::management::DurableTransferError::LedgerNoEffect(
                TransferError::BadFee { expected_fee },
            ) = error
            {
                if let Ok(fee) = expected_fee.0.try_into() {
                    mutate_state(|s| {
                        if let Some(config) = s.get_collateral_config_mut(&transfer.collateral_type)
                        {
                            config.ledger_fee = fee;
                        }
                    });
                }
            }
        }
    }
}

fn three_usd_reserve_fee_matches(expected_fee_e8s: Option<u64>, observed_fee_e8s: u64) -> bool {
    expected_fee_e8s == Some(observed_fee_e8s)
}

#[cfg(test)]
mod three_usd_reserve_fee_tests {
    use super::three_usd_reserve_fee_matches;

    #[test]
    fn only_exact_observed_fee_allows_payout_completion() {
        assert!(three_usd_reserve_fee_matches(Some(10), 10));
        assert!(!three_usd_reserve_fee_matches(Some(10), 12));
        assert!(!three_usd_reserve_fee_matches(None, 10));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreeUsdReserveCandidateRecoveryClaim {
    key: crate::state::ThreeUsdReserveIngressKey,
    operation_id: u128,
    attempt_nonce: u128,
    block_index: u64,
    payout: crate::state::ThreeUsdReserveCollateralPayout,
}

fn three_usd_reserve_payout_attempt(
    payout: &crate::state::ThreeUsdReserveCollateralPayout,
    attempt_nonce: u128,
) -> Option<crate::state::ThreeUsdReserveCollateralPayout> {
    let mut attempt_payout = payout.clone();
    if payout.op_nonce == attempt_nonce {
        return Some(attempt_payout);
    }
    let attempt = payout.rearmed_attempts.iter().find(|attempt| attempt.op_nonce == attempt_nonce)?;
    attempt_payout.op_nonce = attempt.op_nonce;
    attempt_payout.memo = attempt.memo;
    attempt_payout.created_at_time_ns = attempt.created_at_time_ns;
    attempt_payout.fee_arg_e8s = attempt.fee_arg_e8s;
    attempt_payout.candidate_block_index = attempt.candidate_block_index;
    attempt_payout.observed_fee_e8s = attempt.observed_fee_e8s;
    Some(attempt_payout)
}

fn has_three_usd_reserve_candidate(
    state: &crate::state::State,
    operation_id: u128,
) -> bool {
    state.three_usd_reserve_payout_operation_keys
        .get(&operation_id)
        .and_then(|key| state.three_usd_reserve_collateral_payouts.get(key))
        .is_some_and(|payout| {
            payout.candidate_block_index.is_some()
                || payout.rearmed_attempts.iter().any(|attempt| attempt.candidate_block_index.is_some())
        })
}

fn claim_three_usd_reserve_candidate_recovery(
    state: &mut crate::state::State,
    owner: Principal,
    backend: Principal,
    operation_id: u128,
) -> Option<ThreeUsdReserveCandidateRecoveryClaim> {
    let key = state.three_usd_reserve_payout_operation_keys.get(&operation_id)?.clone();
    let (_, transfer) = state.get_pending_payout(operation_id)?;
    let payout = state.three_usd_reserve_collateral_payouts.get(&key)?;
    let mut candidates = Vec::new();
    if let Some(block_index) = payout.candidate_block_index {
        candidates.push((payout.op_nonce, block_index));
    }
    candidates.extend(payout.rearmed_attempts.iter().filter_map(|attempt| {
        attempt.candidate_block_index.map(|block_index| (attempt.op_nonce, block_index))
    }));
    if candidates.len() != 1 || candidates[0].0 != transfer.op_nonce
        || transfer.operation_id != operation_id
        || transfer.owner != owner
        || owner != key.stability_pool
        || transfer.vault_id != key.vault_id
        || transfer.payout_kind != crate::state::PendingPayoutKind::Margin
        || !transfer.held_for_manual_retry
        || !transfer.reconciliation_required
        || transfer.in_flight
        || transfer.ledger != Some(payout.ledger)
        || transfer.collateral_type != payout.collateral_type
        || transfer.margin.to_u64() != payout.gross_e8s
        || transfer.transfer_amount_raw != Some(payout.net_e8s)
        || payout.operation_id != operation_id
        || payout.op_nonce != operation_id
        || payout.ledger == Principal::anonymous()
        || payout.source.owner != backend
        || payout.source.subaccount.is_some()
        || payout.destination.owner != owner
        || payout.destination.subaccount.is_some()
        || payout.gross_e8s.checked_sub(payout.net_e8s) != Some(payout.expected_fee_e8s)
        || payout.fee_arg_e8s != Some(payout.expected_fee_e8s)
        || payout.rearmed_attempts.len() > crate::state::MAX_THREE_USD_RESERVE_PAYOUT_ATTEMPTS
    {
        return None;
    }
    let (attempt_nonce, block_index) = candidates[0];
    let attempt_payout = three_usd_reserve_payout_attempt(payout, attempt_nonce)?;
    if attempt_payout.candidate_block_index != Some(block_index)
        || attempt_payout.fee_arg_e8s != Some(attempt_payout.expected_fee_e8s)
        || attempt_payout.memo.as_slice()
            != crate::management::nonce_to_memo(attempt_nonce).0.as_slice()
        || attempt_payout.created_at_time_ns
            != crate::management::nonce_to_created_at_time(attempt_nonce)
    {
        return None;
    }
    state.mutate_pending_payout(operation_id, |row| row.in_flight = true);
    Some(ThreeUsdReserveCandidateRecoveryClaim {
        key,
        operation_id,
        attempt_nonce,
        block_index,
        payout: attempt_payout,
    })
}

fn finish_three_usd_reserve_candidate_recovery(
    state: &mut crate::state::State,
    claim: &ThreeUsdReserveCandidateRecoveryClaim,
    observed_fee_e8s: Option<u64>,
    completed_at_ns: u64,
) -> bool {
    let same_claim = state.three_usd_reserve_payout_operation_keys.get(&claim.operation_id)
        == Some(&claim.key)
        && state.get_pending_payout(claim.operation_id).is_some_and(|(_, transfer)| {
            transfer.operation_id == claim.operation_id
                && transfer.owner == claim.payout.destination.owner
                && transfer.vault_id == claim.key.vault_id
                && transfer.payout_kind == crate::state::PendingPayoutKind::Margin
                && transfer.ledger == Some(claim.payout.ledger)
                && transfer.collateral_type == claim.payout.collateral_type
                && transfer.margin.to_u64() == claim.payout.gross_e8s
                && transfer.transfer_amount_raw == Some(claim.payout.net_e8s)
                && transfer.op_nonce == claim.attempt_nonce
                && transfer.held_for_manual_retry
                && transfer.reconciliation_required
                && transfer.in_flight
        })
        && state.three_usd_reserve_collateral_payouts.get(&claim.key)
            .and_then(|payout| three_usd_reserve_payout_attempt(payout, claim.attempt_nonce))
            .is_some_and(|payout| payout == claim.payout)
        && claim.payout.candidate_block_index == Some(claim.block_index);
    if !same_claim {
        return false;
    }
    let hold = |state: &mut crate::state::State| {
        state.mutate_pending_payout(claim.operation_id, |row| {
            row.in_flight = false;
            row.held_for_manual_retry = true;
            row.reconciliation_required = true;
        });
    };
    let Some(observed_fee_e8s) = observed_fee_e8s else {
        hold(state);
        return false;
    };
    if let Some(previous_fee) = claim.payout.observed_fee_e8s {
        if previous_fee != observed_fee_e8s {
            hold(state);
            return false;
        }
    } else if !crate::event::record_three_usd_reserve_payout_fee_observed(
        state,
        claim.key.clone(),
        claim.operation_id,
        claim.attempt_nonce,
        claim.block_index,
        observed_fee_e8s,
    ) {
        hold(state);
        return false;
    }
    if !three_usd_reserve_fee_matches(Some(claim.payout.expected_fee_e8s), observed_fee_e8s) {
        hold(state);
        return false;
    }
    crate::event::record_margin_transfer_at(
        state,
        claim.key.vault_id,
        claim.payout.destination.owner,
        claim.operation_id,
        crate::state::PendingPayoutKind::Margin,
        claim.block_index,
        completed_at_ns,
    );
    true
}

/// Retry only proof for an already persisted 3USD reserve payout candidate.
/// `None` means this operation has no persisted candidate and may continue
/// through the generic recovery flow. `Some(false)` is held without resend.
pub async fn recover_three_usd_reserve_payout_candidate(
    operation_id: u128,
    owner: Principal,
) -> Option<bool> {
    let reserve_linked = crate::state::read_state(|state| {
        state.three_usd_reserve_payout_operation_keys.contains_key(&operation_id)
    });
    if reserve_linked && !crate::state::read_state(|state| has_three_usd_reserve_candidate(state, operation_id)) {
        match crate::payout_history::advance_owner_reserve_candidate_discovery(operation_id, owner)
            .await
        {
            Ok(crate::payout_history::ReserveCandidateDiscoveryProgress::CandidateRecorded) => {}
            Ok(crate::payout_history::ReserveCandidateDiscoveryProgress::Scanning)
            | Ok(crate::payout_history::ReserveCandidateDiscoveryProgress::Held)
            | Err(_) => return Some(false),
        }
    }
    let claim_result = crate::state::mutate_state(|state| {
        if !has_three_usd_reserve_candidate(state, operation_id) {
            return None;
        }
        Some(claim_three_usd_reserve_candidate_recovery(
            state,
            owner,
            ic_cdk::id(),
            operation_id,
        ))
    });
    let claim = match claim_result {
        None => return None,
        Some(None) => return Some(false),
        Some(Some(claim)) => claim,
    };
    let observed_fee_e8s = verify_three_usd_reserve_payout_fee(&claim.payout, claim.block_index)
        .await
        .ok();
    let completed_at_ns = ic_cdk::api::time();
    Some(crate::state::mutate_state(|state| {
        finish_three_usd_reserve_candidate_recovery(state, &claim, observed_fee_e8s, completed_at_ns)
    }))
}

#[cfg(test)]
mod three_usd_reserve_candidate_recovery_tests {
    use super::*;
    use crate::state::{
        PendingMarginTransfer, PendingPayoutKind, State, ThreeUsdReserveCollateralPayout,
        ThreeUsdReserveIngressKey,
    };
    use icrc_ledger_types::icrc1::account::Account;

    fn principal(seed: u8) -> Principal {
        Principal::self_authenticating([seed; 32])
    }

    fn fixture() -> (State, Principal, Principal, ThreeUsdReserveIngressKey, u128) {
        let backend = principal(51);
        let pool = principal(52);
        let ledger = principal(53);
        let operation_id = 54;
        let key = ThreeUsdReserveIngressKey { stability_pool: pool, vault_id: 7, absorb_id: 8 };
        let memo: [u8; 16] = crate::management::nonce_to_memo(operation_id).0.as_slice()
            .try_into().unwrap();
        let payout = ThreeUsdReserveCollateralPayout {
            operation_id,
            op_nonce: operation_id,
            collateral_type: ledger,
            ledger,
            source: Account { owner: backend, subaccount: None },
            destination: Account { owner: pool, subaccount: None },
            gross_e8s: 110,
            net_e8s: 100,
            expected_fee_e8s: 10,
            memo,
            created_at_time_ns: crate::management::nonce_to_created_at_time(operation_id),
            fee_arg_e8s: Some(10),
            candidate_block_index: Some(12),
            observed_fee_e8s: None,
            rearmed_attempts: Vec::new(),
        };
        let transfer = PendingMarginTransfer {
            vault_id: 7,
            operation_id,
            payout_kind: PendingPayoutKind::Margin,
            owner: pool,
            margin: crate::numeric::ICP::new(110),
            collateral_type: ledger,
            retry_count: 0,
            op_nonce: operation_id,
            ledger: Some(ledger),
            transfer_amount_raw: Some(100),
            held_for_manual_retry: true,
            reconciliation_required: true,
            in_flight: false,
            too_old_confirmed: false,
            history_start_index: None,
            rearm_schema_version: 1,
            history_scan: None,
            history_candidate_seen: false,
            no_effect_proof: None,
            history_log_length: None,
            history_cursor: 0,
            min_net_collateral_raw: None,
        };
        let mut state = State::default();
        state.three_usd_reserve_collateral_payouts.insert(key.clone(), payout);
        state.three_usd_reserve_payout_operation_keys.insert(operation_id, key.clone());
        state.insert_pending_payout(transfer);
        (state, backend, pool, key, operation_id)
    }

    #[test]
    fn exact_pinned_fee_completes_only_the_persisted_candidate() {
        let (mut state, backend, pool, key, operation_id) = fixture();
        let claim = claim_three_usd_reserve_candidate_recovery(&mut state, pool, backend, operation_id)
            .expect("held single candidate should be claimable");
        assert_eq!(claim.block_index, 12);
        assert!(finish_three_usd_reserve_candidate_recovery(&mut state, &claim, Some(10), 100));
        assert!(state.get_pending_payout(operation_id).is_none());
        assert_eq!(
            state.three_usd_reserve_collateral_payouts.get(&key).unwrap().observed_fee_e8s,
            Some(10),
        );
    }

    #[test]
    fn fee_mismatch_is_journaled_and_stays_held() {
        let (mut state, backend, pool, key, operation_id) = fixture();
        let claim = claim_three_usd_reserve_candidate_recovery(&mut state, pool, backend, operation_id)
            .expect("held single candidate should be claimable");
        assert!(!finish_three_usd_reserve_candidate_recovery(&mut state, &claim, Some(12), 100));
        let (_, transfer) = state.get_pending_payout(operation_id).unwrap();
        assert!(transfer.held_for_manual_retry);
        assert!(transfer.reconciliation_required);
        assert!(!transfer.in_flight);
        assert_eq!(
            state.three_usd_reserve_collateral_payouts.get(&key).unwrap().observed_fee_e8s,
            Some(12),
        );
    }

    #[test]
    fn stale_proof_callback_cannot_complete_a_changed_pending_tuple() {
        let (mut state, backend, pool, _, operation_id) = fixture();
        let claim = claim_three_usd_reserve_candidate_recovery(&mut state, pool, backend, operation_id)
            .expect("held single candidate should be claimable");
        state.mutate_pending_payout(operation_id, |row| row.transfer_amount_raw = Some(99));
        let before = state.get_pending_payout(operation_id).unwrap();
        assert!(!finish_three_usd_reserve_candidate_recovery(&mut state, &claim, Some(10), 100));
        assert_eq!(state.get_pending_payout(operation_id), Some(before));
    }

    #[test]
    fn failed_block_read_releases_only_the_in_memory_claim_and_keeps_payout_held() {
        let (mut state, backend, pool, _, operation_id) = fixture();
        let claim = claim_three_usd_reserve_candidate_recovery(&mut state, pool, backend, operation_id)
            .expect("held single candidate should be claimable");
        assert!(!finish_three_usd_reserve_candidate_recovery(&mut state, &claim, None, 100));
        let (_, transfer) = state.get_pending_payout(operation_id).unwrap();
        assert!(transfer.held_for_manual_retry);
        assert!(transfer.reconciliation_required);
        assert!(!transfer.in_flight);
    }

    #[test]
    fn generic_timer_does_not_clear_an_owner_proof_claim_as_journal_mismatch() {
        let (mut state, backend, pool, _, operation_id) = fixture();
        let _claim = claim_three_usd_reserve_candidate_recovery(&mut state, pool, backend, operation_id)
            .expect("held single candidate should be claimable");
        let (_, transfer) = state.get_pending_payout(operation_id).unwrap();
        assert!(!reserve_payout_journal_mismatch_requires_hold(&transfer, false));
        assert!(transfer.in_flight);
        assert!(transfer.held_for_manual_retry);
    }
}

/// Read back a reserve payout candidate and verify its exact transfer tuple.
/// The observed fee is evidence only; it is never added to the recipient
/// amount. Archive callback-only blocks remain held; this backend candidate
/// is not itself the authenticated inclusion proof required by the SP.
async fn verify_three_usd_reserve_payout_fee(
    payout: &crate::state::ThreeUsdReserveCollateralPayout,
    block_index: u64,
) -> Result<u64, String> {
    if crate::native_icp_proof::is_native_icp_ledger(payout.ledger) {
        let block = crate::native_icp_proof::query_block(payout.ledger, block_index).await?;
        return crate::native_icp_proof::verify_transfer_block(
            &block,
            payout.source.owner,
            payout.destination.owner,
            payout.net_e8s,
            &payout.memo,
            payout.created_at_time_ns,
        );
    }
    // This backend candidate is intentionally direct-ledger only. Archive
    // callbacks do not authenticate membership for Stability Pool settlement.
    let request = vec![icrc_ledger_types::icrc3::blocks::GetBlocksRequest {
        start: candid::Nat::from(block_index),
        length: candid::Nat::from(1u64),
    }];
    let response: Result<(
        icrc_ledger_types::icrc3::blocks::GetBlocksResult,
    ), _> = ic_cdk::call(payout.ledger, "icrc3_get_blocks", (request,)).await;
    let (response,) = response.map_err(|(code, message)| {
        format!("direct icrc3_get_blocks failed: {code:?} {message}")
    })?;
    if !response.archived_blocks.is_empty()
        || response.blocks.len() != 1
        || response.blocks[0].id.0.to_u64() != Some(block_index)
    {
        return Err("exact payout block is not served directly by the ledger".into());
    }
    let block = crate::icrc3_proof::decode_block(&response.blocks[0].block)?;
    crate::icrc3_proof::validate_icrc3_transfer_block(
        &block,
        Some(payout.source.clone()),
        payout.destination.clone(),
        payout.net_e8s,
        Some(&payout.memo),
        Some(payout.created_at_time_ns),
    )?;
    block.fee.ok_or_else(|| "direct ICRC-3 transfer block omits fee".to_string())
}

pub async fn process_pending_transfer() {
    let _guard = match crate::guard::TimerLogicGuard::new() {
        Some(guard) => guard,
        None => {
            log!(INFO, "[process_pending_transfer] double entry.");
            return;
        }
    };

    // Process a bounded round-robin slice through immutable operation receipts.
    // Retry caps hold receipts in place; advancing the persisted cursor prevents
    // a held low-ID prefix from starving later retryable payouts.
    let payout_ids = read_state(|s| s.next_pending_payout_batch(MAX_PENDING_PAYOUTS_PER_TICK));
    for operation_id in payout_ids {
        process_one_pending_payout(operation_id).await;
        mutate_state(|s| s.advance_pending_payout_scan_cursor(operation_id));
    }

    // Wave-4 ICC-007: durable refund queue from `redeem_reserves` double-failures.
    // Each entry is keyed by the original burn block index (unique). We drive the
    // retry through `transfer_icusd_with_nonce` so the icUSD ledger deduplicates
    // if a previous attempt's reply was lost.
    let pending_refunds = read_state(|s| {
        s.pending_refunds
            .iter()
            .filter(|(_, refund)| pending_refund_is_automatically_retryable(refund.retry_count))
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<(u64, crate::state::PendingRefund)>>()
    });

    for (icusd_block_index, refund) in pending_refunds {
        match crate::management::transfer_icusd_with_nonce(
            crate::numeric::ICUSD::new(refund.amount_e8s),
            refund.user,
            refund.op_nonce,
        )
        .await
        {
            Ok(block_index) => {
                log!(INFO,
                    "[refunding] icUSD refund settled for {} (burn block {}, refund block {}, amount {})",
                    refund.user, icusd_block_index, block_index, refund.amount_e8s
                );
                mutate_state(|s| {
                    s.pending_refunds.remove(&icusd_block_index);
                });
            }
            Err(error) => {
                log!(
                    INFO,
                    "[refunding] icUSD refund failed for {} (burn block {}): {}. Will retry.",
                    refund.user,
                    icusd_block_index,
                    error
                );
                if let TransferError::BadFee { expected_fee } = error {
                    // Refresh fee cache; do NOT increment retry count on BadFee.
                    if let Ok(expected_fee_u64) = expected_fee.0.clone().try_into() {
                        let icusd_ledger = read_state(|s| s.icusd_ledger_principal);
                        crate::management::set_cached_fee(icusd_ledger, expected_fee_u64);
                    }
                } else {
                    let retries = mutate_state(|s| {
                        if let Some(r) = s.pending_refunds.get_mut(&icusd_block_index) {
                            r.retry_count = r.retry_count.saturating_add(1);
                            r.retry_count
                        } else {
                            0
                        }
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(
                            INFO,
                            "[refunding] CRITICAL: holding durable icUSD refund claim for {} (burn block {}) \
                             after {} automatic retries. Amount: {}. Owner can inspect it; manual recovery is required.",
                            refund.user,
                            icusd_block_index,
                            retries,
                            refund.amount_e8s
                        );
                    }
                }
            }
        }
    }

    // Durable queue for stranded 3USD reserve refunds. V2 child rows have a
    // journal that binds their retry and receipt lifecycle. Legacy hashed-
    // reserve rows instead fence before their first ledger await and remain
    // held after any ambiguous outcome. Parentless default-account rows lack
    // the V2 journal and remain available for reconciliation only.
    let pending_3usd_refunds = read_state(|s| {
        s.pending_3usd_refunds
            .iter()
            .filter(|(_, refund)| pending_3usd_refund_can_be_retried(s, refund))
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<(u128, crate::state::PendingThreeUsdRefund)>>()
    });

    for (nonce_key, refund) in pending_3usd_refunds {
        let _legacy_reserve_fee_guard = if refund.source
            == crate::state::ThreeUsdRefundSource::LegacyHashedReserve
            && refund.legacy_dispatch_retryable
        {
            match crate::management::ThreeUsdReserveIngressAdmissionGuard::try_acquire() {
                Some(guard) => Some(guard),
                None => continue,
            }
        } else {
            None
        };
        let _default_account_guard = if refund.source == crate::state::ThreeUsdRefundSource::DefaultAccount
            && refund.parent_absorb_id.is_some()
        {
            match crate::management::ThreeUsdReserveIngressAdmissionGuard::try_acquire() {
                Some(guard) => Some(guard),
                None => continue,
            }
        } else {
            None
        };
        let destination = icrc_ledger_types::icrc1::account::Account {
            owner: refund.stability_pool,
            subaccount: None,
        };
        let mut dispatched_amount = None;
        let mut dispatched_fee = None;
        let result = match refund.source {
            // Preserve the historic source and transfer arguments exactly for
            // rows decoded from old snapshots. Old rows are never automatically
            // dispatched; newly created rows use the pinned full-principal path.
            crate::state::ThreeUsdRefundSource::LegacyHashedReserve => {
                dispatch_legacy_three_usd_refund(nonce_key, refund, destination.clone()).await
            }
            crate::state::ThreeUsdRefundSource::DefaultAccount => {
                // V2 refund rows promise a net credit to the SP. The protocol
                // sends that exact amount and pays the ledger fee from its own
                // default-account liquidity. Legacy rows keep their historical
                // net-of-fee behavior and persisted dispatch tuple.
                let protocol_pays_fee = refund.parent_absorb_id.is_some();
                if protocol_pays_fee {
                    // A saved block index means the transfer already returned
                    // successfully. Re-verify that exact receipt after restart;
                    // never submit the transfer again.
                    if let (Some(amount), Some(0), Some(block_index)) = (
                        refund.dispatch_amount_e8s,
                        refund.dispatch_fee_e8s,
                        refund.dispatch_block_index,
                    ) {
                        dispatched_amount = Some(amount);
                        dispatched_fee = Some(0);
                        Ok(block_index)
                    } else if refund.dispatch_submitted {
                        // A prior await may have committed while its reply was
                        // lost. Keep the liability durable and require exact
                        // status/receipt reconciliation before any re-arm.
                        Err(TransferError::GenericError {
                            error_code: candid::Nat::from(0u8),
                            message: "3USD refund dispatch outcome is ambiguous; manual reconciliation required".into(),
                        })
                    } else {
                        let fee_ok = matches!(crate::management::refresh_fee_cache(refund.ledger).await, Ok(0));
                        let balance_ok = if fee_ok {
                            let balance = crate::management::get_balance_of(
                                icrc_ledger_types::icrc1::account::Account { owner: ic_cdk::id(), subaccount: None },
                                refund.ledger,
                            ).await.ok();
                            let required = crate::management::three_usd_default_account_required_balance(refund.ledger);
                            matches!((balance, required), (Some(balance), Some(required)) if u128::from(balance) >= required)
                        } else {
                            false
                        };
                        if !balance_ok {
                            mutate_state(|s| {
                                if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                    row.retry_count = MAX_PENDING_RETRIES;
                                }
                            });
                            Err(TransferError::GenericError {
                                error_code: candid::Nat::from(0u8),
                                message: "3USD refund held: fee or default-account solvency is unverified".into(),
                            })
                        } else {
                            // Persist the immutable transfer tuple and the
                            // submitted marker before crossing the ledger await.
                            let persisted = mutate_state(|s| {
                                if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                    if row.parent_absorb_id == refund.parent_absorb_id
                                        && !row.dispatch_submitted
                                        && row.dispatch_amount_e8s.is_none()
                                        && row.dispatch_fee_e8s.is_none()
                                    {
                                        row.dispatch_amount_e8s = Some(row.amount_e8s);
                                        row.dispatch_fee_e8s = Some(0);
                                        row.dispatch_submitted = true;
                                        return Some(row.amount_e8s);
                                    }
                                }
                                None
                            });
                            if let Some(amount) = persisted {
                                dispatched_amount = Some(amount);
                                dispatched_fee = Some(0);
                                let submitted = crate::management::transfer_idempotent_pinned_fee(
                                    refund.ledger, None, destination.clone(), amount as u128,
                                    refund.op_nonce, 0,
                                ).await;
                                if let Ok(block_index) = submitted {
                                    // Save the returned receipt location before
                                    // any further await or proof decoding.
                                    mutate_state(|s| {
                                        if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                            if row.dispatch_submitted
                                                && row.dispatch_amount_e8s == Some(amount)
                                                && row.dispatch_fee_e8s == Some(0)
                                            {
                                                row.dispatch_block_index = Some(block_index);
                                            }
                                        }
                                    });
                                    Ok(block_index)
                                } else {
                                    submitted
                                }
                            } else {
                                Err(TransferError::GenericError {
                                    error_code: candid::Nat::from(0u8),
                                    message: "3USD refund dispatch tuple changed before submission".into(),
                                })
                            }
                        }
                    }
                } else {
                let pinned = match (refund.dispatch_amount_e8s, refund.dispatch_fee_e8s) {
                    (Some(amount), Some(fee)) => Some((amount, fee)),
                    (None, None) => match crate::management::get_or_refresh_fee(refund.ledger).await {
                        Ok(fee) if refund.amount_e8s > fee => {
                            let amount = refund.amount_e8s - fee;
                            mutate_state(|s| {
                                if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                    if row.source == crate::state::ThreeUsdRefundSource::DefaultAccount
                                        && row.dispatch_amount_e8s.is_none() {
                                        row.dispatch_amount_e8s = Some(amount);
                                        row.dispatch_fee_e8s = Some(fee);
                                    }
                                }
                            });
                            Some((amount, fee))
                        }
                        Ok(_) | Err(_) => None,
                    },
                    _ => None,
                };
                match pinned {
                    Some((amount, fee)) => {
                        dispatched_amount = Some(amount);
                        dispatched_fee = Some(fee);
                        crate::management::transfer_idempotent(
                            refund.ledger, None, destination.clone(), amount as u128,
                            refund.op_nonce, None,
                        ).await
                    },
                    None => Err(TransferError::GenericError {
                        error_code: candid::Nat::from(0u8),
                        message: "default-account refund is below fee or fee is unavailable".into(),
                    }),
                }
                }
            }
        };
        match result {
            Ok(block_index) => {
                if refund.source == crate::state::ThreeUsdRefundSource::LegacyHashedReserve {
                    let saved = mutate_state(|s| {
                        let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) else {
                            return None;
                        };
                        let expected_submitted = crate::state::PendingThreeUsdRefund {
                            dispatch_submitted: true,
                            dispatch_amount_e8s: if refund.legacy_dispatch_retryable {
                                Some(refund.amount_e8s)
                            } else {
                                refund.dispatch_amount_e8s
                            },
                            dispatch_fee_e8s: if refund.legacy_dispatch_retryable {
                                row.dispatch_fee_e8s
                            } else {
                                refund.dispatch_fee_e8s
                            },
                            ..refund
                        };
                        if refund.dispatch_submitted
                            || refund.dispatch_block_index.is_some()
                            || (refund.legacy_dispatch_retryable && row.dispatch_fee_e8s.is_none())
                            || *row != expected_submitted
                        {
                            return None;
                        }
                        row.dispatch_block_index = Some(block_index);
                        Some(*row)
                    });
                    if let Some(saved_row) = saved {
                        let proof = crate::icrc3_proof::fetch_icrc3_block_direct(
                            refund.ledger,
                            block_index,
                        )
                        .await;
                        let receipt = verify_legacy_three_usd_refund_receipt(
                            &saved_row,
                            ic_cdk::id(),
                            block_index,
                            proof,
                        );
                        match receipt {
                            Ok(receipt) => {
                                let reconciled = mutate_state(|s| {
                                    commit_legacy_three_usd_refund_receipt(
                                        s,
                                        nonce_key,
                                        saved_row,
                                        &receipt,
                                            ic_cdk::api::time(),
                                        ic_cdk::id(),
                                    )
                                });
                                if reconciled {
                                    log!(INFO,
                                        "[refunding] 3USD legacy refund block {} has an exact direct-ledger receipt; durable liability tombstoned",
                                        block_index
                                    );
                                } else {
                                    log!(INFO,
                                        "[refunding] 3USD legacy refund block {} verified, but durable row changed before resolution; retaining liability",
                                        block_index
                                    );
                                }
                            }
                            Err(reason) => log!(INFO,
                                "[refunding] 3USD legacy refund block {} lacks exact direct-ledger proof; retaining liability: {}",
                                block_index, reason
                            ),
                        }
                    } else {
                        log!(INFO,
                            "[refunding] 3USD legacy refund returned block {}, but durable row changed; preserving liability for reconciliation",
                            block_index
                        );
                    }
                    continue;
                }
                let verified_refund = if refund.source == crate::state::ThreeUsdRefundSource::DefaultAccount {
                    let verified = async {
                        let amount = dispatched_amount.ok_or_else(|| "default refund amount was not pinned".to_string())?;
                        let block = crate::icrc3_proof::fetch_icrc3_block(refund.ledger, block_index).await?;
                        let charged_fee = block.fee.ok_or_else(|| "refund block omits the charged fee".to_string())?;
                        let protocol_pays_fee = refund.parent_absorb_id.is_some();
                        let credited = if protocol_pays_fee { amount } else { amount.checked_add(charged_fee).unwrap_or_default() };
                        if credited != refund.amount_e8s {
                            return Err("verified refund net credit does not restore the journaled obligation".to_string());
                        }
                        let memo: [u8; 16] = crate::management::nonce_to_memo(refund.op_nonce).0.as_slice()
                            .try_into().map_err(|_| "refund memo is not 16 bytes".to_string())?;
                        let tuple = crate::state::ThreeUsdReserveRefundTuple {
                            source_owner: ic_cdk::id(),
                            source_subaccount: None,
                            destination: destination.clone(),
                            amount_e8s: amount,
                            charged_fee_e8s: charged_fee,
                            fee_e8s: dispatched_fee,
                            memo,
                            created_at_time_ns: crate::management::nonce_to_created_at_time(refund.op_nonce),
                        };
                        crate::icrc3_proof::validate_three_usd_reserve_refund_block(&block, &tuple)?;
                        Ok::<_, String>(crate::state::ThreeUsdReserveRefundReceipt { block_index, tuple })
                    }.await;
                    match verified {
                        Ok(receipt) => Some(receipt),
                        Err(reason) => {
                            log!(INFO,
                                "[refunding] 3USD default-source transfer returned block {} but exact ICRC-3 proof failed; retaining durable refund for reconciliation: {}",
                                block_index, reason
                            );
                            mutate_state(|s| {
                                if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                    row.retry_count = row.retry_count.saturating_add(1);
                                }
                            });
                            continue;
                        }
                    }
                } else {
                    None
                };
                log!(INFO,
                    "[refunding] 3USD reserve refund settled for SP {} (vault {}, refund block {}, amount {})",
                    refund.stability_pool, refund.vault_id, block_index, refund.amount_e8s
                );
                mutate_state(|s| {
                    if refund.source == crate::state::ThreeUsdRefundSource::DefaultAccount {
                        if let Some(parent_absorb_id) = refund.parent_absorb_id {
                            let key = crate::state::ThreeUsdReserveIngressKey {
                                stability_pool: refund.stability_pool,
                                vault_id: refund.vault_id,
                                absorb_id: parent_absorb_id,
                            };
                            if let Some(journal) = s.three_usd_reserve_ingress_journals.get_mut(&key) {
                                if let Some(child) = journal.refund.as_mut() {
                                    if child.op_nonce == refund.op_nonce {
                                        child.settled_receipt = verified_refund.clone();
                                    }
                                }
                            }
                        }
                    }
                    s.pending_3usd_refunds.remove(&nonce_key);
                    if let Some(parent_absorb_id) = refund.parent_absorb_id {
                        let key = crate::state::ThreeUsdReserveIngressKey {
                            stability_pool: refund.stability_pool,
                            vault_id: refund.vault_id,
                            absorb_id: parent_absorb_id,
                        };
                        if let Some(journal) = s.three_usd_reserve_ingress_journals.get_mut(&key) {
                            journal.protocol_refund_fee_reserve_e8s = 0;
                        }
                    }
                });
            }
            Err(error) => {
                log!(
                    INFO,
                    "[refunding] 3USD reserve refund failed for SP {} (vault {}): {:?}. Retaining durable liability.",
                    refund.stability_pool,
                    refund.vault_id,
                    error
                );
                if refund.source == crate::state::ThreeUsdRefundSource::LegacyHashedReserve
                    && matches!(&error, TransferError::BadFee { .. })
                {
                    mutate_state(|s| {
                        release_legacy_3usd_refund_after_bad_fee(s, nonce_key, refund)
                    });
                }
                if refund.parent_absorb_id.is_some() {
                    if matches!(&error, TransferError::BadFee { .. }) {
                        // BadFee proves the pinned transfer had no effect. Clear
                        // the fence only through exact CAS against the submitted
                        // tuple and its unresolved parent child; the helper caps
                        // the row until a developer rearms the zero-fee rail.
                        let released = mutate_state(|s| {
                            crate::management::release_v2_default_account_refund_after_bad_fee(
                                s, nonce_key, refund,
                            )
                        });
                        if !released {
                            // Preserve a fail-closed cap if the row or parent
                            // changed while the transfer call was in flight.
                            mutate_state(|s| {
                                if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                    row.retry_count = MAX_PENDING_RETRIES;
                                }
                            });
                        }
                    } else {
                        // Every non-typed response may have committed before a
                        // lost reply. Keep the submitted tuple fenced and stop
                        // timer polling for operator reconciliation.
                        mutate_state(|s| {
                            if let Some(row) = s.pending_3usd_refunds.get_mut(&nonce_key) {
                                row.retry_count = MAX_PENDING_RETRIES;
                            }
                        });
                    }
                }
                if let TransferError::BadFee { expected_fee } = error {
                    // Refresh fee cache; do NOT increment retry count on BadFee.
                    if let Ok(expected_fee_u64) = expected_fee.0.clone().try_into() {
                        crate::management::set_cached_fee(refund.ledger, expected_fee_u64);
                    }
                } else {
                    let retries = mutate_state(|s| {
                        s.note_pending_3usd_refund_failure(nonce_key).unwrap_or(0)
                    });
                    if retries >= MAX_PENDING_RETRIES {
                        log!(
                            INFO,
                            "[refunding] CRITICAL: holding 3USD reserve refund for SP {} (vault {}) \
                             after {} retries. Amount: {}. Manual reconciliation required.",
                            refund.stability_pool,
                            refund.vault_id,
                            retries,
                            refund.amount_e8s
                        );
                        // Keep every capped obligation durable for reconciliation.
                    }
                }
            }
        }
    }

    // Schedule another run if needed, but with better timing
    if read_state(|s| {
        s.pending_payout_index.keys().any(|operation_id| {
            s.get_pending_payout(*operation_id)
                .is_some_and(|(_, transfer)| {
                    !transfer.held_for_manual_retry
                        && !transfer.reconciliation_required
                        && !transfer.in_flight
                        && transfer.retry_count < MAX_PENDING_RETRIES
                })
        }) || s
            .pending_refunds
            .values()
            .any(|refund| pending_refund_is_automatically_retryable(refund.retry_count))
            || s.pending_3usd_refunds
                .values()
                .any(|refund| pending_3usd_refund_can_be_retried(s, refund))
    }) {
        // Schedule another check in 5 seconds
        log!(
            INFO,
            "[process_pending_transfer] Scheduling another transfer attempt in 5 seconds"
        );
        ic_cdk_timers::set_timer(std::time::Duration::from_secs(5), || {
            ic_cdk::spawn(crate::process_pending_transfer())
        });
    } else {
        log!(INFO, "[process_pending_transfer] No more pending transfers");
    }
}
