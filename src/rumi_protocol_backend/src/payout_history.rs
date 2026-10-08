//! Conservative ICRC-3 history scans for owner-requested payout recovery.
//!
//! This scanner is used only after a typed TooOld from the pinned ledger. It
//! proves absence only over one complete, bounded snapshot prefix served
//! directly by the pinned ledger. Archived ranges remain held until archive
//! blocks can be tied to an authenticated ledger anchor. A matching loose
//! transfer tuple is always inconclusive, including when memo or
//! created_at_time is omitted by the ledger.

use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc3::blocks::{
    ArchivedBlocks, BlockWithId, GetBlocksRequest, GetBlocksResult,
};
use num_traits::ToPrimitive;
use std::collections::BTreeSet;

const PAGE_BLOCKS: u64 = 64;
const MAX_ARCHIVE_DESCRIPTORS: usize = 8;
const MAX_ARCHIVE_RANGES: usize = 64;
const MAX_REARM_HISTORY_BLOCKS: u64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutPageResult {
    CoveredAbsent,
    Candidate,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutRearmProgress {
    Scanning,
    Rearmed,
    CandidateHeld,
    UnsupportedHeld,
}

/// Capture a lower block bound before the first dispatch for this attempt.
/// If ICRC-3 is unavailable or malformed, keep normal payout processing but
/// permanently disable automatic rearm for this receipt.
pub async fn capture_dispatch_boundary(
    operation_id: u128,
    expected: crate::state::PendingMarginTransfer,
) -> bool {
    if expected.rearm_schema_version != 1 || expected.history_start_index.is_some() {
        return true;
    }
    let ledger = match expected.ledger {
        Some(ledger) => ledger,
        None => return true,
    };
    let request = vec![GetBlocksRequest {
        start: Nat::from(0u64),
        length: Nat::from(1u64),
    }];
    let result: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await;
    let log_length = result
        .ok()
        .and_then(|(response,)| response.log_length.0.to_u64());
    let Some(amount_raw) = expected.transfer_amount_raw else {
        return true;
    };
    crate::state::mutate_state(|state| {
        if !dispatch_boundary_matches(
            operation_id,
            expected,
            state
                .get_pending_payout(operation_id)
                .map(|(_, current)| current),
        ) {
            return false;
        }
        crate::event::record_pending_payout_dispatch_boundary(
            state,
            operation_id,
            expected.op_nonce,
            ledger,
            expected.owner,
            amount_raw,
            log_length,
        )
    })
}

/// Hold only the exact attempt whose preflight callback failed. A stale await
/// must not alter a row that replaced or advanced the operation while it was
/// suspended.
pub fn hold_failed_dispatch_preflight(
    operation_id: u128,
    expected: crate::state::PendingMarginTransfer,
) -> bool {
    crate::state::mutate_state(|state| {
        hold_failed_dispatch_preflight_in_state(state, operation_id, expected)
    })
}

fn hold_failed_dispatch_preflight_in_state(
    state: &mut crate::state::State,
    operation_id: u128,
    expected: crate::state::PendingMarginTransfer,
) -> bool {
    let Some((_, current)) = state.get_pending_payout(operation_id) else {
        return false;
    };
    if !preflight_failure_matches(operation_id, expected, current) {
        return false;
    }
    state.mutate_pending_payout(operation_id, |row| {
        row.in_flight = false;
        row.held_for_manual_retry = true;
        row.reconciliation_required = true;
    });
    true
}

fn preflight_failure_matches(
    operation_id: u128,
    expected: crate::state::PendingMarginTransfer,
    current: crate::state::PendingMarginTransfer,
) -> bool {
    current.operation_id == operation_id
        && expected.operation_id == operation_id
        && current.payout_kind == expected.payout_kind
        && current.owner == expected.owner
        && current.ledger == expected.ledger
        && current.transfer_amount_raw == expected.transfer_amount_raw
        && current.op_nonce == expected.op_nonce
        && current.rearm_schema_version == expected.rearm_schema_version
        && current.retry_count == expected.retry_count
        && current.no_effect_proof == expected.no_effect_proof
        && current.in_flight
}

fn dispatch_boundary_matches(
    operation_id: u128,
    expected: crate::state::PendingMarginTransfer,
    current: Option<crate::state::PendingMarginTransfer>,
) -> bool {
    current.is_some_and(|row| {
        row.operation_id == operation_id
            && expected.operation_id == operation_id
            && row.payout_kind == expected.payout_kind
            && row.owner == expected.owner
            && row.ledger == expected.ledger
            && row.transfer_amount_raw == expected.transfer_amount_raw
            && row.op_nonce == expected.op_nonce
            && row.rearm_schema_version == expected.rearm_schema_version
            && row.retry_count == expected.retry_count
            && row.no_effect_proof == expected.no_effect_proof
            && !row.held_for_manual_retry
            && !row.reconciliation_required
            && !row.too_old_confirmed
            && row.in_flight
            && row.history_start_index.is_none()
    })
}

/// Bound the next page to one snapshot and one backend message's work budget.
pub fn next_page_end(cursor: u64, log_length: u64) -> Option<u64> {
    (cursor < log_length).then(|| log_length.min(cursor.saturating_add(PAGE_BLOCKS)))
}

/// Validate exact direct-ledger coverage, then conservatively check every
/// ICRC-1/2 transfer against the payout's loose account/amount tuple. Any
/// intersecting archive delegation is inconclusive and stays held.
pub fn scan_page(
    start: u64,
    end: u64,
    snapshot_log_length: u64,
    source: Principal,
    recipient: Principal,
    amount_raw: u64,
    history: GetBlocksResult,
) -> PayoutPageResult {
    let Some(length) = end.checked_sub(start) else {
        return PayoutPageResult::Incomplete;
    };
    if length == 0
        || length > PAGE_BLOCKS
        || end > snapshot_log_length
        || history
            .log_length
            .0
            .to_u64()
            .is_none_or(|n| n < snapshot_log_length)
        || history.blocks.len() > PAGE_BLOCKS as usize
        || history.archived_blocks.len() > MAX_ARCHIVE_DESCRIPTORS
    {
        return PayoutPageResult::Incomplete;
    }

    let mut seen = BTreeSet::new();
    let mut candidate = false;
    if !inspect_blocks(
        &history.blocks,
        start,
        end,
        None,
        source,
        recipient,
        amount_raw,
        &mut seen,
        &mut candidate,
    ) {
        return PayoutPageResult::Incomplete;
    }

    // The ledger authenticates these descriptors, but this canister has no
    // cryptographic proof that a separate archive's returned blocks belong to
    // the ledger's history. Until an archive trust list or chained proof is
    // available, any requested range delegated to an archive stays held.
    for archive in &history.archived_blocks {
        if match clipped_requests(archive, start, end) {
            Ok(ranges) => !ranges.is_empty(),
            Err(()) => true,
        } {
            return PayoutPageResult::Incomplete;
        }
    }

    if seen.len() as u64 != length || (start..end).any(|i| !seen.contains(&i)) {
        return PayoutPageResult::Incomplete;
    }
    if candidate {
        PayoutPageResult::Candidate
    } else {
        PayoutPageResult::CoveredAbsent
    }
}

/// Advance one owner-authorized page, or commit a fresh nonce only after the
/// complete post-dispatch prefix has been covered with no loose tuple match.
pub async fn advance_owner_rearm(
    operation_id: u128,
    caller: Principal,
) -> Result<PayoutRearmProgress, String> {
    let now = ic_cdk::api::time();
    let initial = crate::state::read_state(|state| state.get_pending_payout(operation_id));
    let Some((_, row)) = initial else {
        return Err("pending payout no longer exists".into());
    };
    if row.owner != caller {
        return Err("only the payout owner can request history reconciliation".into());
    }
    if row.history_candidate_seen {
        return Ok(PayoutRearmProgress::CandidateHeld);
    }
    if !eligible(&row) {
        return Ok(PayoutRearmProgress::UnsupportedHeld);
    }
    if !crate::state::mutate_state(|state| state.claim_payout_history_scan_slot(now)) {
        return Ok(PayoutRearmProgress::Scanning);
    }
    let ledger = row.ledger.expect("eligible row has pinned ledger");
    let start_index = row
        .history_start_index
        .expect("eligible row has pre-dispatch log boundary");
    let scan = match row.history_scan.clone() {
        Some(scan) if scan_matches_row(&scan, operation_id, &row) => scan,
        Some(_) => return Ok(PayoutRearmProgress::UnsupportedHeld),
        None => {
            let request = vec![GetBlocksRequest {
                start: Nat::from(start_index),
                length: Nat::from(1u64),
            }];
            let result: Result<(GetBlocksResult,), _> =
                ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await;
            let Some(snapshot_log_length) = result
                .ok()
                .and_then(|(response,)| response.log_length.0.to_u64())
            else {
                return Ok(PayoutRearmProgress::UnsupportedHeld);
            };
            if snapshot_log_length < start_index {
                return Ok(PayoutRearmProgress::UnsupportedHeld);
            }
            if !history_span_within_budget(start_index, snapshot_log_length) {
                return Ok(PayoutRearmProgress::UnsupportedHeld);
            }
            let scan = crate::state::PendingPayoutHistoryScan {
                operation_id,
                payout_kind: row.payout_kind,
                ledger,
                owner: row.owner,
                amount_raw: row.transfer_amount_raw.unwrap(),
                attempt_nonce: row.op_nonce,
                start_index,
                snapshot_log_length,
                next_index: start_index,
            };
            let stored = crate::state::mutate_state(|state| {
                let valid = state
                    .get_pending_payout(operation_id)
                    .is_some_and(|(_, current)| eligible(&current) && current == row);
                if !valid {
                    return false;
                }
                state.mutate_pending_payout(operation_id, |current| {
                    current.history_log_length = Some(snapshot_log_length);
                    current.history_cursor = start_index;
                    current.history_scan = Some(scan.clone());
                });
                true
            });
            if !stored {
                return Ok(PayoutRearmProgress::UnsupportedHeld);
            }
            scan
        }
    };

    if scan.next_index == scan.snapshot_log_length {
        return commit_rearm(operation_id, caller, scan, now);
    }
    let Some(end) = next_page_end(scan.next_index, scan.snapshot_log_length) else {
        return Ok(PayoutRearmProgress::UnsupportedHeld);
    };
    let request = vec![GetBlocksRequest {
        start: Nat::from(scan.next_index),
        length: Nat::from(end - scan.next_index),
    }];
    let response: Result<(GetBlocksResult,), _> =
        ic_cdk::call(ledger, "icrc3_get_blocks", (request,)).await;
    let Ok((history,)) = response else {
        discard_incomplete_scan(operation_id, &scan);
        return Ok(PayoutRearmProgress::UnsupportedHeld);
    };
    let source = ic_cdk::id();
    let page_result = scan_page(
        scan.next_index,
        end,
        scan.snapshot_log_length,
        source,
        row.owner,
        scan.amount_raw,
        history,
    );
    match page_result {
        PayoutPageResult::Candidate => {
            crate::state::mutate_state(|state| {
                if state
                    .get_pending_payout(operation_id)
                    .is_some_and(|(_, current)| scan_matches_row(&scan, operation_id, &current))
                {
                    state.mutate_pending_payout(operation_id, |current| {
                        if current.history_scan.as_ref() == Some(&scan) {
                            current.history_candidate_seen = true;
                        }
                    });
                }
            });
            Ok(PayoutRearmProgress::CandidateHeld)
        }
        PayoutPageResult::Incomplete => {
            discard_incomplete_scan(operation_id, &scan);
            Ok(PayoutRearmProgress::UnsupportedHeld)
        }
        PayoutPageResult::CoveredAbsent => {
            let advanced = crate::state::mutate_state(|state| {
                let valid = state
                    .get_pending_payout(operation_id)
                    .is_some_and(|(_, current)| {
                        eligible(&current)
                            && scan_matches_row(&scan, operation_id, &current)
                            && current.history_scan.as_ref() == Some(&scan)
                    });
                if !valid {
                    return false;
                }
                state.mutate_pending_payout(operation_id, |current| {
                    if let Some(stored) = current.history_scan.as_mut() {
                        stored.next_index = end;
                    }
                    current.history_cursor = end;
                });
                true
            });
            if !advanced {
                return Ok(PayoutRearmProgress::UnsupportedHeld);
            }
            if end == scan.snapshot_log_length {
                let mut complete = scan;
                complete.next_index = end;
                commit_rearm(operation_id, caller, complete, now)
            } else {
                Ok(PayoutRearmProgress::Scanning)
            }
        }
    }
}

fn discard_incomplete_scan(operation_id: u128, scan: &crate::state::PendingPayoutHistoryScan) {
    crate::state::mutate_state(|state| {
        if state
            .get_pending_payout(operation_id)
            .is_some_and(|(_, current)| current.history_scan.as_ref() == Some(scan))
        {
            state.mutate_pending_payout(operation_id, |current| {
                current.history_scan = None;
                current.history_log_length = None;
                current.history_cursor = 0;
            });
        }
    });
}

fn eligible(row: &crate::state::PendingMarginTransfer) -> bool {
    row.rearm_schema_version == 1
        && row.held_for_manual_retry
        && row.reconciliation_required
        && row.too_old_confirmed
        && !row.in_flight
        && row.op_nonce != 0
        && row.history_start_index.is_some()
        && row.ledger.is_some()
        && row.transfer_amount_raw.is_some_and(|amount| amount > 0)
        && row.retry_count < crate::MAX_PENDING_RETRIES
        && row.no_effect_proof.is_none()
        && !row.history_candidate_seen
}

fn scan_matches_row(
    scan: &crate::state::PendingPayoutHistoryScan,
    operation_id: u128,
    row: &crate::state::PendingMarginTransfer,
) -> bool {
    scan.operation_id == operation_id
        && row.operation_id == operation_id
        && scan.payout_kind == row.payout_kind
        && row.ledger == Some(scan.ledger)
        && scan.owner == row.owner
        && row.transfer_amount_raw == Some(scan.amount_raw)
        && scan.attempt_nonce == row.op_nonce
        && row.history_start_index == Some(scan.start_index)
        && history_span_within_budget(scan.start_index, scan.snapshot_log_length)
        && scan.start_index <= scan.next_index
        && scan.next_index <= scan.snapshot_log_length
}

fn history_span_within_budget(start_index: u64, snapshot_log_length: u64) -> bool {
    snapshot_log_length
        .checked_sub(start_index)
        .is_some_and(|span| span <= MAX_REARM_HISTORY_BLOCKS)
}

fn commit_rearm(
    operation_id: u128,
    caller: Principal,
    scan: crate::state::PendingPayoutHistoryScan,
    now: u64,
) -> Result<PayoutRearmProgress, String> {
    let committed = crate::state::mutate_state(|state| {
        let Some((_, row)) = state.get_pending_payout(operation_id) else {
            return false;
        };
        if !eligible(&row)
            || row.owner != caller
            || !scan_matches_row(&scan, operation_id, &row)
            || scan.next_index != scan.snapshot_log_length
            || row.history_scan.as_ref() != Some(&scan)
        {
            return false;
        }
        let mut new_nonce = state.next_op_nonce_at(now);
        while new_nonce == 0 || new_nonce == row.op_nonce {
            new_nonce = state.next_op_nonce_at(now);
        }
        crate::event::record_pending_payout_rearmed(
            state,
            crate::state::PendingPayoutNoEffectProof {
                operation_id,
                payout_kind: row.payout_kind,
                ledger: scan.ledger,
                owner: row.owner,
                amount_raw: scan.amount_raw,
                old_attempt_nonce: scan.attempt_nonce,
                new_attempt_nonce: new_nonce,
                start_index: scan.start_index,
                snapshot_log_length: scan.snapshot_log_length,
                complete_prefix: true,
                verified_at_ns: now,
            },
        )
    });
    Ok(if committed {
        PayoutRearmProgress::Rearmed
    } else {
        PayoutRearmProgress::UnsupportedHeld
    })
}

fn inspect_blocks(
    blocks: &[BlockWithId],
    start: u64,
    end: u64,
    requested: Option<&[GetBlocksRequest]>,
    source: Principal,
    recipient: Principal,
    amount_raw: u64,
    seen: &mut BTreeSet<u64>,
    candidate: &mut bool,
) -> bool {
    for block in blocks {
        let Some(index) = block.id.0.to_u64() else {
            return false;
        };
        if index < start
            || index >= end
            || requested.is_some_and(|ranges| !ranges.iter().any(|r| covers(r, index)))
            || !seen.insert(index)
        {
            return false;
        }
        match transfer_candidate(&block.block, source, recipient, amount_raw) {
            Some(true) => *candidate = true,
            Some(false) => {}
            None => return false,
        }
    }
    true
}

/// `None` means the block cannot safely be classified; `Some(true)` means a
/// same-source/destination/amount transfer could be the attempted payout.
fn transfer_candidate(
    block: &ICRC3Value,
    source: Principal,
    recipient: Principal,
    amount_raw: u64,
) -> Option<bool> {
    let decoded = crate::icrc3_proof::decode_block(block).ok()?;
    let ICRC3Value::Map(fields) = block else {
        return None;
    };
    let known_type = match fields.get("btype") {
        Some(ICRC3Value::Text(kind)) => matches!(
            kind.as_str(),
            "1mint" | "1burn" | "1xfer" | "2xfer" | "1approve" | "2approve"
        ),
        Some(_) => false,
        None => matches!(decoded.op.as_str(), "mint" | "burn" | "xfer" | "approve"),
    };
    if !known_type {
        return None;
    }
    if decoded.op != "xfer" {
        return Some(false);
    }
    let from = decoded.from?;
    let to = decoded.to?;
    Some(
        is_default_account(from, source)
            && is_default_account(to, recipient)
            && decoded.amount == amount_raw as u128,
    )
}

fn is_default_account(account: Account, owner: Principal) -> bool {
    account.owner == owner
        && account
            .subaccount
            .is_none_or(|subaccount| subaccount == [0; 32])
}

fn clipped_requests(
    archive: &ArchivedBlocks,
    start: u64,
    end: u64,
) -> Result<Vec<GetBlocksRequest>, ()> {
    if archive.args.is_empty() || archive.args.len() > MAX_ARCHIVE_RANGES {
        return Err(());
    }
    let mut ranges = Vec::new();
    for advertised in &archive.args {
        let Some(a_start) = advertised.start.0.to_u64() else {
            return Err(());
        };
        let Some(a_length) = advertised.length.0.to_u64() else {
            return Err(());
        };
        let Some(a_end) = a_start.checked_add(a_length) else {
            return Err(());
        };
        let clipped_start = start.max(a_start);
        let clipped_end = end.min(a_end);
        if clipped_start < clipped_end {
            ranges.push(GetBlocksRequest {
                start: Nat::from(clipped_start),
                length: Nat::from(clipped_end - clipped_start),
            });
        }
    }
    Ok(ranges)
}

fn covers(request: &GetBlocksRequest, index: u64) -> bool {
    let (Some(start), Some(length)) = (request.start.0.to_u64(), request.length.0.to_u64()) else {
        return false;
    };
    start <= index && start.checked_add(length).is_some_and(|end| index < end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use icrc_ledger_types::icrc3::archive::QueryArchiveFn;

    fn pending_row() -> crate::state::PendingMarginTransfer {
        crate::state::PendingMarginTransfer {
            vault_id: 7,
            operation_id: 44,
            payout_kind: crate::state::PendingPayoutKind::Margin,
            owner: Principal::from_slice(&[2]),
            margin: crate::numeric::ICP::new(100),
            collateral_type: Principal::from_slice(&[3]),
            retry_count: 0,
            op_nonce: 55,
            ledger: Some(Principal::from_slice(&[4])),
            transfer_amount_raw: Some(90),
            held_for_manual_retry: false,
            reconciliation_required: false,
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
        }
    }

    fn block(index: u64, value: ICRC3Value) -> BlockWithId {
        BlockWithId {
            id: Nat::from(index),
            block: value,
        }
    }

    fn response(log_length: u64, blocks: Vec<BlockWithId>) -> GetBlocksResult {
        GetBlocksResult {
            log_length: Nat::from(log_length),
            blocks,
            archived_blocks: vec![],
        }
    }

    fn xfer(source: Principal, recipient: Principal, amount: u64) -> ICRC3Value {
        crate::icrc3_proof::make_test_transfer_block(
            Account {
                owner: source,
                subaccount: None,
            },
            Account {
                owner: recipient,
                subaccount: None,
            },
            amount,
            &[1, 2, 3],
            true,
        )
    }

    #[test]
    fn any_same_tuple_transfer_is_a_candidate_even_without_optional_identity_fields() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        let mut block = xfer(source, recipient, 55);
        let ICRC3Value::Map(fields) = &mut block else {
            panic!("test block is a map")
        };
        let Some(ICRC3Value::Map(tx)) = fields.get_mut("tx") else {
            panic!("test block has tx map")
        };
        tx.remove("memo");
        tx.remove("ts");
        assert_eq!(
            transfer_candidate(&block, source, recipient, 55),
            Some(true)
        );
    }

    #[test]
    fn unrelated_tuple_is_absent_but_unknown_block_type_is_incomplete() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        assert_eq!(
            transfer_candidate(&xfer(source, recipient, 56), source, recipient, 55),
            Some(false)
        );

        let mut unknown = xfer(source, recipient, 56);
        let ICRC3Value::Map(fields) = &mut unknown else {
            panic!("test block is a map")
        };
        fields.insert("btype".into(), ICRC3Value::Text("9xfer".into()));
        assert_eq!(transfer_candidate(&unknown, source, recipient, 55), None);
    }

    #[test]
    fn zero_subaccount_is_equivalent_to_default_account() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        let mut block = xfer(source, recipient, 55);
        let ICRC3Value::Map(fields) = &mut block else {
            panic!("test block is a map")
        };
        let Some(ICRC3Value::Map(tx)) = fields.get_mut("tx") else {
            panic!("test block has tx map")
        };
        for key in ["from", "to"] {
            let Some(ICRC3Value::Array(account)) = tx.get_mut(key) else {
                panic!("test account is an array")
            };
            account.push(ICRC3Value::Blob(vec![0; 32].into()));
        }
        assert_eq!(
            transfer_candidate(&block, source, recipient, 55),
            Some(true)
        );
    }

    #[test]
    fn page_requires_exact_contiguous_coverage() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        let absent = scan_page(
            10,
            12,
            12,
            source,
            recipient,
            55,
            response(
                12,
                vec![
                    block(10, xfer(source, recipient, 56)),
                    block(11, xfer(source, recipient, 57)),
                ],
            ),
        );
        assert_eq!(absent, PayoutPageResult::CoveredAbsent);

        let gap = scan_page(
            10,
            12,
            12,
            source,
            recipient,
            55,
            response(12, vec![block(10, xfer(source, recipient, 56))]),
        );
        assert_eq!(gap, PayoutPageResult::Incomplete);
    }

    #[test]
    fn archive_range_outside_page_does_not_block_direct_scan() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        let archive_id = Principal::from_slice(&[3]);
        let archive = ArchivedBlocks {
            args: vec![GetBlocksRequest {
                start: Nat::from(12u64),
                length: Nat::from(1u64),
            }],
            callback: QueryArchiveFn::new(archive_id, "archive_blocks_v2"),
        };
        let history = GetBlocksResult {
            log_length: Nat::from(13u64),
            blocks: vec![block(10, xfer(source, recipient, 56))],
            archived_blocks: vec![archive],
        };
        let gap = scan_page(10, 11, 11, source, recipient, 55, history);
        assert_eq!(gap, PayoutPageResult::CoveredAbsent);
    }

    #[test]
    fn forged_gap_free_archive_data_is_rejected_without_calling_archive() {
        let source = Principal::from_slice(&[1]);
        let recipient = Principal::from_slice(&[2]);
        let archived_blocks = vec![ArchivedBlocks {
            args: vec![GetBlocksRequest {
                start: Nat::from(10u64),
                length: Nat::from(1u64),
            }],
            callback: QueryArchiveFn::new(Principal::from_slice(&[9]), "get_blocks"),
        }];
        let history = GetBlocksResult {
            log_length: Nat::from(11u64),
            blocks: vec![],
            archived_blocks,
        };
        let result = scan_page(10, 11, 11, source, recipient, 55, history);
        assert_eq!(result, PayoutPageResult::Incomplete);
    }

    #[test]
    fn dispatch_preflight_revalidates_the_locked_attempt_after_await() {
        let expected = pending_row();
        let mut current = expected;
        current.in_flight = true;
        assert!(dispatch_boundary_matches(44, expected, Some(current)));

        let mut changed_nonce = current;
        changed_nonce.op_nonce += 1;
        assert!(!dispatch_boundary_matches(
            44,
            expected,
            Some(changed_nonce)
        ));
        let mut changed_ledger = current;
        changed_ledger.ledger = Some(Principal::from_slice(&[5]));
        assert!(!dispatch_boundary_matches(
            44,
            expected,
            Some(changed_ledger)
        ));
        let mut changed_owner = current;
        changed_owner.owner = Principal::from_slice(&[6]);
        assert!(!dispatch_boundary_matches(
            44,
            expected,
            Some(changed_owner)
        ));
        let mut changed_amount = current;
        changed_amount.transfer_amount_raw = Some(91);
        assert!(!dispatch_boundary_matches(
            44,
            expected,
            Some(changed_amount)
        ));
        let mut concurrently_held = current;
        concurrently_held.held_for_manual_retry = true;
        assert!(!dispatch_boundary_matches(
            44,
            expected,
            Some(concurrently_held)
        ));
        let mut unlocked = current;
        unlocked.in_flight = false;
        assert!(!dispatch_boundary_matches(44, expected, Some(unlocked)));
        assert!(!dispatch_boundary_matches(44, expected, None));
    }

    #[test]
    fn stale_preflight_failure_does_not_hold_a_replacement_attempt() {
        let expected = pending_row();
        let mut replacement = expected;
        replacement.in_flight = true;
        replacement.op_nonce += 1;
        replacement.retry_count += 1;
        let mut state = crate::state::State::default();
        state.insert_pending_payout(replacement);

        assert!(!hold_failed_dispatch_preflight_in_state(
            &mut state,
            expected.operation_id,
            expected
        ));
        let (_, current) = state.get_pending_payout(expected.operation_id).unwrap();
        assert_eq!(current.op_nonce, replacement.op_nonce);
        assert!(current.in_flight);
        assert!(!current.held_for_manual_retry);
        assert!(!current.reconciliation_required);

        let mut same_attempt = expected;
        same_attempt.in_flight = true;
        let mut state = crate::state::State::default();
        state.insert_pending_payout(same_attempt);
        assert!(hold_failed_dispatch_preflight_in_state(
            &mut state,
            expected.operation_id,
            expected
        ));
        let (_, held) = state.get_pending_payout(expected.operation_id).unwrap();
        assert!(!held.in_flight);
        assert!(held.held_for_manual_retry);
        assert!(held.reconciliation_required);
    }

    #[test]
    fn legacy_rows_and_rows_without_pre_dispatch_boundary_cannot_rearm() {
        let mut row = pending_row();
        row.held_for_manual_retry = true;
        row.reconciliation_required = true;
        row.too_old_confirmed = true;
        assert!(!eligible(&row), "missing lower boundary must fail closed");
        row.history_start_index = Some(10);
        assert!(eligible(&row));
        row.rearm_schema_version = 0;
        assert!(
            !eligible(&row),
            "legacy row cannot be opted in by migration"
        );
    }

    #[test]
    fn full_history_scan_has_a_per_attempt_work_cap() {
        assert!(history_span_within_budget(10, 10_010));
        assert!(!history_span_within_budget(10, 10_011));
        assert!(!history_span_within_budget(11, 10));
    }

    #[test]
    fn aggregate_history_scan_quota_limits_repeat_page_fetches() {
        let mut state = crate::state::State::default();
        for _ in 0..12 {
            assert!(state.claim_payout_history_scan_slot(1_000));
        }
        assert!(!state.claim_payout_history_scan_slot(1_000));
        assert!(state.claim_payout_history_scan_slot(60_000_001_000));
    }
}
