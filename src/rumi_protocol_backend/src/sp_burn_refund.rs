//! Exact compensation for a Stability Pool icUSD burn that cannot be
//! absorbed. A confirmed burn is the sole authority for reimbursement: this
//! path never changes vault debt, collateral, foreign-chain supply, or SP
//! depositor accounting.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::Memo;
use icrc_ledger_types::icrc3::blocks::{
    ArchivedBlocks, BlockWithId, GetBlocksRequest, GetBlocksResult,
};
use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::icrc3_proof::{ProofExpectations, SpProofLedger, SpWritedownProof};
use crate::state::{
    mutate_state, read_state, StoredSpBurnRefund, StoredSpBurnRefundHistoryScan,
    StoredSpBurnRefundNoEffectEvidence, MAX_SP_BURN_REFUND_ATTEMPTS,
};
use crate::ProtocolError;

const REFUND_MEMO_PREFIX: &[u8; 8] = b"RSPRFND:";
// Keep each recovery history request bounded; archive callbacks may prove a
// positive match, while larger or incompletely archived histories stay held.
const MAX_REFUND_HISTORY_BLOCKS: u64 = 64;
const MAX_REFUND_ARCHIVE_CALLBACKS: usize = 32;
const MAX_REFUND_ARCHIVE_RANGES_PER_CALLBACK: usize = 32;

thread_local! {
    /// Runtime-only exclusion for the proof-specific async saga. The durable
    /// obligation is in State; an upgrade clears this guard and retries the
    /// same ledger tuple.
    static REFUNDS_IN_FLIGHT: RefCell<BTreeSet<(SpProofLedger, u64)>> =
        RefCell::new(BTreeSet::new());
}

struct RefundGuard((SpProofLedger, u64));

impl RefundGuard {
    fn acquire(key: (SpProofLedger, u64)) -> Result<Self, ProtocolError> {
        let inserted = REFUNDS_IN_FLIGHT.with(|locks| locks.borrow_mut().insert(key));
        if inserted {
            Ok(Self(key))
        } else {
            Err(ProtocolError::AlreadyProcessing)
        }
    }
}

impl Drop for RefundGuard {
    fn drop(&mut self) {
        REFUNDS_IN_FLIGHT.with(|locks| {
            locks.borrow_mut().remove(&self.0);
        });
    }
}

/// Durable receipt returned by both mint and reconciliation. `recipient` is
/// the original authenticated Stability Pool principal; it is always paid to
/// that canister's default account.
#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SpBurnRefundReceipt {
    pub vault_id: u64,
    pub amount_e8s: u64,
    pub ledger: Principal,
    pub recipient: Principal,
    pub burn_block_index: u64,
    pub refund_block_index: u64,
    pub refund_created_at_time: u64,
    pub refund_memo: Vec<u8>,
}

/// Compensate an exact, authenticated SP burn when absorption did not win the
/// proof's terminal-outcome race. If a previous call already journaled the
/// obligation, retries reuse its exact ledger, memo, timestamp, and amount.
pub async fn refund_stability_pool_burn(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
) -> Result<SpBurnRefundReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    authorize_sp(caller)?;
    validate_burn_request(vault_id, amount_e8s, &proof)?;
    let key = (proof.ledger_kind, proof.block_index);
    let _guard = RefundGuard::acquire(key)?;

    if let Some(record) = existing_matching_refund(key, caller, vault_id, amount_e8s, &proof)? {
        if let Some(block) = record.refund_block_index {
            return Ok(receipt_from_record(&record, block));
        }
        return execute_stored_refund(key, record).await;
    }

    let ledger = read_state(|s| s.icusd_ledger_principal);
    if ledger == Principal::anonymous() {
        return Err(ProtocolError::GenericError(
            "icUSD ledger is not configured".into(),
        ));
    }
    if proof_was_consumed(key) {
        return Err(consumed_proof_error(key));
    }

    let expectations = ProofExpectations {
        ledger_kind: SpProofLedger::IcusdBurn,
        expected_amount_e8s: amount_e8s,
        sp_principal: caller,
        reserves_account: default_account(ic_cdk::id()),
        vault_id_memo: vault_id,
    };
    crate::icrc3_proof::fetch_and_validate_block(ledger, proof.block_index, &expectations)
        .await
        .map_err(|err| {
            ProtocolError::GenericError(format!("SP burn proof verification failed: {err}"))
        })?;

    // The ledger's configured minting account is the backend's default
    // account. Check this live before creating a compensation obligation.
    verify_mint_authority(ledger).await?;

    // Recheck all state-dependent authority and proof arbitration after both
    // awaits. The record and consumed tombstone are committed atomically before
    // the first mint attempt.
    let record = mutate_state(|s| {
        if s.stability_pool_canister != Some(caller) {
            return Err(ProtocolError::GenericError(
                "Stability Pool registration changed while verifying burn".into(),
            ));
        }
        if s.icusd_ledger_principal != ledger {
            return Err(ProtocolError::GenericError(
                "icUSD ledger configuration changed while verifying burn".into(),
            ));
        }
        if let Some(record) = s.sp_burn_refunds_by_proof.get(&key) {
            if refund_matches(record, caller, vault_id, amount_e8s, &proof, ledger) {
                return Ok(record.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP burn proof already has a different compensation request".into(),
            ));
        }
        if s.consumed_writedown_proofs.contains(&key) {
            return Err(consumed_proof_error(key));
        }
        let op_nonce = s.next_op_nonce();
        let mut refund_memo = Vec::with_capacity(24);
        refund_memo.extend_from_slice(REFUND_MEMO_PREFIX);
        refund_memo.extend_from_slice(&proof.block_index.to_be_bytes());
        refund_memo.extend_from_slice(&vault_id.to_be_bytes());
        let record = StoredSpBurnRefund {
            caller,
            vault_id,
            amount_e8s,
            ledger,
            burn_block_index: proof.block_index,
            op_nonce,
            refund_created_at_time: crate::management::nonce_to_created_at_time(op_nonce),
            refund_memo,
            refund_block_index: None,
            attempt_history: Vec::new(),
            history_scan: None,
            no_effect_evidence: None,
            attempt_no_effect_evidence: Vec::new(),
        };
        if !s.consumed_writedown_proofs.insert(key) {
            return Err(consumed_proof_error(key));
        }
        s.sp_burn_refunds_by_proof.insert(key, record.clone());
        Ok(record)
    })?;

    if let Some(block) = record.refund_block_index {
        return Ok(receipt_from_record(&record, block));
    }
    execute_stored_refund(key, record).await
}

/// Reconcile a previously journaled compensation against a ledger block.
/// This only records proof of an existing mint; it never issues a new transfer.
pub async fn reconcile_stability_pool_burn_refund(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
    refund_block_index: u64,
) -> Result<SpBurnRefundReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    authorize_sp(caller)?;
    validate_burn_request(vault_id, amount_e8s, &proof)?;
    let key = (proof.ledger_kind, proof.block_index);
    let record = existing_matching_refund(key, caller, vault_id, amount_e8s, &proof)?
        .ok_or_else(|| ProtocolError::GenericError("no journaled SP burn refund exists".into()))?;
    if let Some(existing_block) = record.refund_block_index {
        if existing_block != refund_block_index {
            return Err(ProtocolError::GenericError(
                "SP burn refund is already confirmed at a different block".into(),
            ));
        }
        return Ok(receipt_from_record(&record, existing_block));
    }
    let _guard = RefundGuard::acquire(key)?;
    crate::icrc3_proof::verify_icrc3_transfer_block(
        record.ledger,
        refund_block_index,
        None,
        default_account(record.caller),
        record.amount_e8s,
        Some(&record.refund_memo),
        Some(record.refund_created_at_time),
    )
    .await
    .map_err(|err| {
        ProtocolError::GenericError(format!(
            "SP refund block does not match the current persisted transfer identity: {err}"
        ))
    })?;

    let confirmed = mutate_state(|s| {
        if s.stability_pool_canister != Some(caller) || s.icusd_ledger_principal != record.ledger {
            return Err(ProtocolError::GenericError(
                "Stability Pool or icUSD ledger configuration changed during reconciliation".into(),
            ));
        }
        let current = s.sp_burn_refunds_by_proof.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("SP burn refund journal entry disappeared".into())
        })?;
        if !refund_matches(current, caller, vault_id, amount_e8s, &proof, record.ledger) {
            return Err(ProtocolError::GenericError(
                "SP burn refund journal changed during reconciliation".into(),
            ));
        }
        if let Some(existing) = current.refund_block_index {
            if existing != refund_block_index {
                return Err(ProtocolError::GenericError(
                    "SP burn refund is already confirmed at a different block".into(),
                ));
            }
        } else {
            current.refund_block_index = Some(refund_block_index);
        }
        Ok(current.clone())
    })?;
    Ok(receipt_from_record(&confirmed, refund_block_index))
}

/// Advance one bounded page of the persisted ledger-prefix scan. A positive
/// exact match records the receipt. An empty prefix is only a snapshot: the
/// original request may still commit after the snapshot, so it cannot justify
/// changing transfer identity without a ledger-specific finality guarantee.
pub async fn reconcile_stability_pool_burn_refund_from_history(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
) -> Result<SpBurnRefundReceipt, ProtocolError> {
    let caller = ic_cdk::api::caller();
    authorize_sp(caller)?;
    validate_burn_request(vault_id, amount_e8s, &proof)?;
    let key = (proof.ledger_kind, proof.block_index);
    let _guard = RefundGuard::acquire(key)?;
    let record = existing_matching_refund(key, caller, vault_id, amount_e8s, &proof)?
        .ok_or_else(|| ProtocolError::GenericError("no journaled SP burn refund exists".into()))?;
    if let Some(block) = record.refund_block_index {
        return Ok(receipt_from_record(&record, block));
    }

    let (record, scan) = advance_refund_history_scan(key, record).await?;
    let refund_block_index = match scan {
        RefundHistoryScan::Found(found) => {
            if !refund_match_is_current_attempt(&record, &found) {
                return Err(ProtocolError::GenericError(
                    "ledger history matched a prior refund tuple with persisted no-effect evidence; outcome remains held".into(),
                ));
            }
            found.0
        }
        RefundHistoryScan::ConflictingMatches => {
            return Err(ProtocolError::GenericError(
                "multiple ledger blocks match the SP refund history; the outcome remains held"
                    .into(),
            ));
        }
        RefundHistoryScan::CompleteAbsent => {
            return Err(ProtocolError::GenericError(
                "complete ledger-prefix snapshot contains no matching refund, but the original transfer may still commit; outcome remains held".into(),
            ));
        }
        RefundHistoryScan::CoveredAbsent => {
            let cursor = record
                .history_scan
                .as_ref()
                .map(|scan| scan.next_index)
                .unwrap_or(0);
            let total = record
                .history_scan
                .as_ref()
                .map(|scan| scan.snapshot_log_length)
                .unwrap_or(0);
            return Err(ProtocolError::GenericError(format!(
                "SP refund prefix scan covered through index {cursor} of {total}; call again to continue; no identity was changed"
            )));
        }
        RefundHistoryScan::Incomplete => {
            return Err(ProtocolError::GenericError(
                "SP refund history page is missing, malformed, or unavailable; its cursor was not advanced and the obligation remains held".into(),
            ));
        }
    };
    crate::icrc3_proof::verify_icrc3_transfer_block(
        record.ledger,
        refund_block_index,
        None,
        default_account(record.caller),
        record.amount_e8s,
        Some(&record.refund_memo),
        Some(record.refund_created_at_time),
    )
    .await
    .map_err(|err| {
        ProtocolError::GenericError(format!(
            "historical SP refund block failed exact verification: {err}"
        ))
    })?;

    let confirmed = mutate_state(|s| {
        if s.stability_pool_canister != Some(caller) || s.icusd_ledger_principal != record.ledger {
            return Err(ProtocolError::GenericError(
                "Stability Pool or icUSD ledger configuration changed during reconciliation".into(),
            ));
        }
        let current = s.sp_burn_refunds_by_proof.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("SP burn refund journal entry disappeared".into())
        })?;
        if current != &record {
            if current.refund_block_index == Some(refund_block_index) {
                return Ok(current.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP burn refund journal changed during history reconciliation".into(),
            ));
        }
        current.refund_block_index = Some(refund_block_index);
        Ok(current.clone())
    })?;
    Ok(receipt_from_record(&confirmed, refund_block_index))
}

fn authorize_sp(caller: Principal) -> Result<(), ProtocolError> {
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    if !read_state(|s| s.stability_pool_canister == Some(caller)) {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".into(),
        ));
    }
    if caller == ic_cdk::id() {
        return Err(ProtocolError::GenericError(
            "Stability Pool cannot be the icUSD minting canister".into(),
        ));
    }
    Ok(())
}

fn validate_burn_request(
    vault_id: u64,
    amount_e8s: u64,
    proof: &SpWritedownProof,
) -> Result<(), ProtocolError> {
    if amount_e8s == 0 {
        return Err(ProtocolError::AmountTooLow { minimum_amount: 1 });
    }
    if proof.ledger_kind != SpProofLedger::IcusdBurn || proof.vault_id_memo != vault_id {
        return Err(ProtocolError::GenericError(
            "compensation requires an icUSD burn proof bound to this vault".into(),
        ));
    }
    Ok(())
}

fn proof_was_consumed(key: (SpProofLedger, u64)) -> bool {
    read_state(|s| s.consumed_writedown_proofs.contains(&key))
}

fn consumed_proof_error(key: (SpProofLedger, u64)) -> ProtocolError {
    ProtocolError::GenericError(format!(
        "SP writedown proof replay rejected: ({:?}, block {}) already consumed",
        key.0, key.1
    ))
}

fn existing_matching_refund(
    key: (SpProofLedger, u64),
    caller: Principal,
    vault_id: u64,
    amount_e8s: u64,
    proof: &SpWritedownProof,
) -> Result<Option<StoredSpBurnRefund>, ProtocolError> {
    read_state(|s| match s.sp_burn_refunds_by_proof.get(&key) {
        Some(record)
            if refund_matches(
                record,
                caller,
                vault_id,
                amount_e8s,
                proof,
                s.icusd_ledger_principal,
            ) =>
        {
            Ok(Some(record.clone()))
        }
        Some(_) => Err(ProtocolError::GenericError(
            "SP burn proof already has a different compensation request".into(),
        )),
        None if s.consumed_writedown_proofs.contains(&key) => Err(consumed_proof_error(key)),
        None => Ok(None),
    })
}

fn refund_matches(
    record: &StoredSpBurnRefund,
    caller: Principal,
    vault_id: u64,
    amount_e8s: u64,
    proof: &SpWritedownProof,
    ledger: Principal,
) -> bool {
    proof.ledger_kind == SpProofLedger::IcusdBurn
        && proof.vault_id_memo == vault_id
        && record.caller == caller
        && record.vault_id == vault_id
        && record.amount_e8s == amount_e8s
        && record.ledger == ledger
        && record.burn_block_index == proof.block_index
}

async fn verify_mint_authority(ledger: Principal) -> Result<(), ProtocolError> {
    let result: Result<(Option<Account>,), _> =
        ic_cdk::call(ledger, "icrc1_minting_account", ()).await;
    let (minting_account,) = result.map_err(|(code, msg)| {
        ProtocolError::GenericError(format!(
            "could not verify icUSD minting account ({:?}): {}",
            code, msg
        ))
    })?;
    if !minting_account
        .as_ref()
        .is_some_and(|account| is_default_account(account, ic_cdk::id()))
    {
        return Err(ProtocolError::GenericError(
            "icUSD ledger minting account is not the backend default account".into(),
        ));
    }
    Ok(())
}

async fn execute_stored_refund(
    key: (SpProofLedger, u64),
    mut record: StoredSpBurnRefund,
) -> Result<SpBurnRefundReceipt, ProtocolError> {
    if let Some(block) = record.refund_block_index {
        return Ok(receipt_from_record(&record, block));
    }
    verify_mint_authority(record.ledger).await?;
    // Recheck registration, ledger identity and the exact journal tuple after
    // the authority query, before making the external minting call.
    if !read_state(|s| {
        s.stability_pool_canister == Some(record.caller)
            && s.icusd_ledger_principal == record.ledger
            && s.sp_burn_refunds_by_proof.get(&key) == Some(&record)
            && s.consumed_writedown_proofs.contains(&key)
    }) {
        return Err(ProtocolError::GenericError(
            "SP refund authorization or journal changed before mint".into(),
        ));
    }
    let block = loop {
        let transfer = crate::management::transfer_idempotent(
            record.ledger,
            None,
            default_account(record.caller),
            record.amount_e8s as u128,
            record.op_nonce,
            Some(Memo::from(record.refund_memo.clone())),
        )
        .await;
        match transfer {
            Ok(block) => break block,
            Err(icrc_ledger_types::icrc1::transfer::TransferError::TooOld) => {
                match recover_too_old_refund(key, record).await? {
                    TooOldRecovery::Confirmed(receipt) => return Ok(receipt),
                    TooOldRecovery::Retry(updated) => record = updated,
                }
            }
            Err(error) => return Err(ProtocolError::TransferError(error)),
        }
    };

    // A returned block index (including Duplicate's original index) is not a
    // payment receipt until the exact mint block is read back and validated.
    crate::icrc3_proof::verify_icrc3_transfer_block(
        record.ledger,
        block,
        None,
        default_account(record.caller),
        record.amount_e8s,
        Some(&record.refund_memo),
        Some(record.refund_created_at_time),
    )
    .await
    .map_err(|err| {
        ProtocolError::GenericError(format!("SP refund receipt verification failed: {err}"))
    })?;

    let confirmed = mutate_state(|s| {
        if s.stability_pool_canister != Some(record.caller)
            || s.icusd_ledger_principal != record.ledger
        {
            return Err(ProtocolError::GenericError(
                "Stability Pool or icUSD ledger configuration changed after mint".into(),
            ));
        }
        let current = s.sp_burn_refunds_by_proof.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("SP burn refund journal entry disappeared".into())
        })?;
        if current != &record {
            if current.refund_block_index == Some(block) {
                return Ok(current.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP burn refund journal changed after mint".into(),
            ));
        }
        current.refund_block_index = Some(block);
        Ok(current.clone())
    })?;
    Ok(receipt_from_record(&confirmed, block))
}

fn default_account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn is_default_account(account: &Account, owner: Principal) -> bool {
    account.owner == owner
        && account
            .subaccount
            .as_ref()
            .map_or(true, |subaccount| *subaccount == [0; 32])
}

fn receipt_from_record(
    record: &StoredSpBurnRefund,
    refund_block_index: u64,
) -> SpBurnRefundReceipt {
    SpBurnRefundReceipt {
        vault_id: record.vault_id,
        amount_e8s: record.amount_e8s,
        ledger: record.ledger,
        recipient: record.caller,
        burn_block_index: record.burn_block_index,
        refund_block_index,
        refund_created_at_time: record.refund_created_at_time,
        refund_memo: record.refund_memo.clone(),
    }
}

enum TooOldRecovery {
    Confirmed(SpBurnRefundReceipt),
    Retry(StoredSpBurnRefund),
}

type RefundHistoryMatch = (u64, u128, u64, Vec<u8>);

enum RefundHistoryScan {
    Found(RefundHistoryMatch),
    /// A page is fully covered, but this is not yet the complete prefix.
    CoveredAbsent,
    CompleteAbsent,
    Incomplete,
    ConflictingMatches,
}

/// A typed `TooOld` from the pinned official icUSD ledger is a no-effect
/// rejection for that call. Together with the complete ledger prefix read
/// after the rejection, it also closes the delayed-call race: ledger time is
/// monotonic, and any not-yet-executed request with this expired tuple will be
/// rejected before transaction application. Scan one bounded page per update.
async fn recover_too_old_refund(
    key: (SpProofLedger, u64),
    record: StoredSpBurnRefund,
) -> Result<TooOldRecovery, ProtocolError> {
    let (record, scan) = advance_refund_history_scan(key, record).await?;
    let block = match scan {
        RefundHistoryScan::Found(found) => {
            if !refund_match_is_current_attempt(&record, &found) {
                return Err(ProtocolError::GenericError(
                    "ledger history matched a prior SP refund tuple already tombstoned absent; payment outcome is inconsistent and the obligation remains held".into(),
                ));
            }
            found.0
        }
        RefundHistoryScan::ConflictingMatches => {
            return Err(ProtocolError::GenericError(
                "multiple ledger blocks match persisted SP refund attempts; payment outcome is inconsistent and the obligation remains held".into(),
            ));
        }
        RefundHistoryScan::CompleteAbsent => {
            let expired = mark_refund_too_old_rejected(key, record)?;
            return Ok(TooOldRecovery::Retry(rotate_refund_identity(
                key,
                expired,
                ic_cdk::api::time(),
            )?));
        }
        RefundHistoryScan::CoveredAbsent => {
            let cursor = record
                .history_scan
                .as_ref()
                .map(|scan| scan.next_index)
                .unwrap_or(0);
            let total = record
                .history_scan
                .as_ref()
                .map(|scan| scan.snapshot_log_length)
                .unwrap_or(0);
            return Err(ProtocolError::GenericError(format!(
                "SP refund returned TooOld; complete prefix scan advanced through {cursor} of {total} blocks; retry the same operation to continue"
            )));
        }
        RefundHistoryScan::Incomplete => {
            return Err(ProtocolError::GenericError(
                "SP refund returned TooOld; history page is incomplete or unavailable, so its cursor was not advanced and the original transfer outcome remains unresolved".into(),
            ));
        }
    };
    crate::icrc3_proof::verify_icrc3_transfer_block(
        record.ledger,
        block,
        None,
        default_account(record.caller),
        record.amount_e8s,
        Some(&record.refund_memo),
        Some(record.refund_created_at_time),
    )
    .await
    .map_err(|err| {
        ProtocolError::GenericError(format!(
            "historical SP refund block failed exact verification: {err}"
        ))
    })?;
    let confirmed = mutate_state(|s| {
        if s.stability_pool_canister != Some(record.caller)
            || s.icusd_ledger_principal != record.ledger
        {
            return Err(ProtocolError::GenericError(
                "Stability Pool or icUSD ledger configuration changed during refund recovery"
                    .into(),
            ));
        }
        let current = s.sp_burn_refunds_by_proof.get_mut(&key).ok_or_else(|| {
            ProtocolError::GenericError("SP burn refund journal entry disappeared".into())
        })?;
        if current != &record {
            if current.refund_block_index == Some(block) {
                return Ok(current.clone());
            }
            return Err(ProtocolError::GenericError(
                "SP burn refund journal changed during historical recovery".into(),
            ));
        }
        current.refund_block_index = Some(block);
        Ok(current.clone())
    })?;
    Ok(TooOldRecovery::Confirmed(receipt_from_record(
        &confirmed, block,
    )))
}

fn rotate_refund_identity(
    key: (SpProofLedger, u64),
    record: StoredSpBurnRefund,
    now: u64,
) -> Result<StoredSpBurnRefund, ProtocolError> {
    let evidence = record.no_effect_evidence.as_ref().ok_or_else(|| {
        ProtocolError::GenericError("SP refund identity rotation requires persisted no-effect evidence".into())
    })?;
    if !evidence.too_old_rejected || !no_effect_evidence_matches(&record, evidence) {
        return Err(ProtocolError::GenericError(
            "SP refund identity rotation requires TooOld from the pinned ledger and complete exact-history absence".into(),
        ));
    }
    if refund_retry_cap_exhausted(&record) {
        return Err(ProtocolError::GenericError(
            "SP refund retry identity cap reached after confirmed no-effect; obligation remains held for operator review".into(),
        ));
    }
    mutate_state(|s| {
        if s.stability_pool_canister != Some(record.caller)
            || s.icusd_ledger_principal != record.ledger
            || s.sp_burn_refunds_by_proof.get(&key) != Some(&record)
        {
            return Err(ProtocolError::GenericError(
                "SP refund journal or ledger configuration changed before safe identity rotation".into(),
            ));
        }
        let next_nonce = s.next_op_nonce_at(now);
        let next_timestamp = crate::management::nonce_to_created_at_time(next_nonce);
        if next_timestamp <= record.refund_created_at_time {
            return Err(ProtocolError::GenericError(
                "backend clock has not advanced beyond the expired refund tuple; obligation remains held".into(),
            ));
        }
        let attempt_number = u64::try_from(record.attempt_history.len() + 1).map_err(|_| {
            ProtocolError::GenericError("SP refund attempt number exceeds u64".into())
        })?;
        let mut next_memo = Vec::with_capacity(32);
        next_memo.extend_from_slice(REFUND_MEMO_PREFIX);
        next_memo.extend_from_slice(&record.burn_block_index.to_be_bytes());
        next_memo.extend_from_slice(&record.vault_id.to_be_bytes());
        next_memo.extend_from_slice(&attempt_number.to_be_bytes());

        let current = s
            .sp_burn_refunds_by_proof
            .get_mut(&key)
            .expect("checked above");
        current.attempt_history.push((
            current.op_nonce,
            current.refund_created_at_time,
            current.refund_memo.clone(),
        ));
        current.attempt_no_effect_evidence.push(evidence.clone());
        current.op_nonce = next_nonce;
        current.refund_created_at_time = next_timestamp;
        current.refund_memo = next_memo;
        current.refund_block_index = None;
        current.history_scan = None;
        current.no_effect_evidence = None;
        Ok(current.clone())
    })
}

/// Bind a typed TooOld response to the already persisted complete-prefix
/// absence. This helper is only called from the exact transfer response path.
fn mark_refund_too_old_rejected(
    key: (SpProofLedger, u64),
    record: StoredSpBurnRefund,
) -> Result<StoredSpBurnRefund, ProtocolError> {
    let expected_evidence = record.no_effect_evidence.as_ref().ok_or_else(|| {
        ProtocolError::GenericError(
            "complete exact-history absence evidence is missing after TooOld".into(),
        )
    })?;
    if !no_effect_evidence_matches(&record, expected_evidence) {
        return Err(ProtocolError::GenericError(
            "SP refund history evidence does not bind the expired tuple".into(),
        ));
    }
    mutate_state(|s| {
        if s.icusd_ledger_principal != record.ledger
            || s.sp_burn_refunds_by_proof.get(&key) != Some(&record)
        {
            return Err(ProtocolError::GenericError(
                "SP refund journal changed before TooOld evidence was committed".into(),
            ));
        }
        let current = s
            .sp_burn_refunds_by_proof
            .get_mut(&key)
            .expect("checked above");
        let evidence = current.no_effect_evidence.as_mut().ok_or_else(|| {
            ProtocolError::GenericError(
                "complete exact-history absence evidence is missing after TooOld".into(),
            )
        })?;
        if evidence != expected_evidence {
            return Err(ProtocolError::GenericError("SP refund history evidence changed before TooOld was recorded".into()));
        }
        evidence.too_old_rejected = true;
        Ok(current.clone())
    })
}

fn refund_retry_cap_exhausted(record: &StoredSpBurnRefund) -> bool {
    record.attempt_history.len() >= MAX_SP_BURN_REFUND_ATTEMPTS
}

fn no_effect_evidence_matches(
    record: &StoredSpBurnRefund,
    evidence: &StoredSpBurnRefundNoEffectEvidence,
) -> bool {
    evidence.ledger == record.ledger
        && evidence.recipient == record.caller
        && evidence.amount_e8s == record.amount_e8s
        && evidence.op_nonce == record.op_nonce
        && evidence.created_at_time == record.refund_created_at_time
        && evidence.memo == record.refund_memo
}

/// Scan at most one bounded page and persist its prefix cursor only after the
/// ledger and every advertised archive callback have supplied exact coverage.
/// This keeps recovery resumable across upgrades without treating a tail or a
/// missing archive range as evidence of no effect.
async fn advance_refund_history_scan(
    key: (SpProofLedger, u64),
    mut record: StoredSpBurnRefund,
) -> Result<(StoredSpBurnRefund, RefundHistoryScan), ProtocolError> {
    if record
        .no_effect_evidence
        .as_ref()
        .is_some_and(|evidence| no_effect_evidence_matches(&record, evidence))
    {
        return Ok((record, RefundHistoryScan::CompleteAbsent));
    }
    if record.no_effect_evidence.is_some() {
        return Err(ProtocolError::GenericError(
            "SP refund no-effect evidence is malformed or belongs to another tuple".into(),
        ));
    }

    let scan = match record.history_scan.clone() {
        Some(scan)
            if scan.ledger == record.ledger
                && scan.recipient == record.caller
                && scan.amount_e8s == record.amount_e8s
                && scan.op_nonce == record.op_nonce
                && scan.created_at_time == record.refund_created_at_time
                && scan.memo == record.refund_memo
                && scan.next_index <= scan.snapshot_log_length =>
        {
            scan
        }
        Some(_) => {
            return Err(ProtocolError::GenericError(
                "SP refund history cursor does not bind the current transfer tuple".into(),
            ));
        }
        None => {
            let probe = vec![GetBlocksRequest {
                start: Nat::from(0u64),
                length: Nat::from(1u64),
            }];
            let (head,): (GetBlocksResult,) = ic_cdk::call(
                record.ledger,
                "icrc3_get_blocks",
                (probe,),
            )
            .await
            .map_err(|(code, message)| {
                ProtocolError::GenericError(format!(
                    "could not establish icUSD ledger history length after TooOld ({code:?}): {message}"
                ))
            })?;
            let snapshot_log_length = head.log_length.0.to_u64().ok_or_else(|| {
                ProtocolError::GenericError("icUSD ledger log length exceeds u64".into())
            })?;
            let scan = StoredSpBurnRefundHistoryScan {
                ledger: record.ledger,
                recipient: record.caller,
                amount_e8s: record.amount_e8s,
                op_nonce: record.op_nonce,
                created_at_time: record.refund_created_at_time,
                memo: record.refund_memo.clone(),
                snapshot_log_length,
                next_index: 0,
            };
            record = mutate_state(|s| {
                if s.icusd_ledger_principal != record.ledger
                    || s.sp_burn_refunds_by_proof.get(&key) != Some(&record)
                {
                    return Err(ProtocolError::GenericError(
                        "SP refund journal changed while establishing ledger history length".into(),
                    ));
                }
                let current = s
                    .sp_burn_refunds_by_proof
                    .get_mut(&key)
                    .expect("checked above");
                current.history_scan = Some(scan.clone());
                Ok(current.clone())
            })?;
            scan
        }
    };

    if scan.snapshot_log_length == 0 {
        let evidence = StoredSpBurnRefundNoEffectEvidence {
            ledger: record.ledger,
            recipient: record.caller,
            amount_e8s: record.amount_e8s,
            op_nonce: record.op_nonce,
            created_at_time: record.refund_created_at_time,
            memo: record.refund_memo.clone(),
            log_length: 0,
            too_old_rejected: false,
        };
        let updated = mutate_state(|s| {
            if s.icusd_ledger_principal != record.ledger
                || s.sp_burn_refunds_by_proof.get(&key) != Some(&record)
            {
                return Err(ProtocolError::GenericError(
                    "SP refund journal changed before empty-history proof was committed".into(),
                ));
            }
            let current = s
                .sp_burn_refunds_by_proof
                .get_mut(&key)
                .expect("checked above");
            current.history_scan = None;
            current.no_effect_evidence = Some(evidence);
            Ok(current.clone())
        })?;
        return Ok((updated, RefundHistoryScan::CompleteAbsent));
    }

    if scan.next_index == scan.snapshot_log_length {
        return Err(ProtocolError::GenericError(
            "SP refund history cursor reached its snapshot without committed no-effect evidence"
                .into(),
        ));
    }
    let start = scan.next_index;
    let end = scan
        .snapshot_log_length
        .min(start.saturating_add(MAX_REFUND_HISTORY_BLOCKS));
    let request = vec![GetBlocksRequest {
        start: Nat::from(start),
        length: Nat::from(end - start),
    }];
    let (history,): (GetBlocksResult,) =
        ic_cdk::call(record.ledger, "icrc3_get_blocks", (request,))
            .await
            .map_err(|(code, message)| {
                ProtocolError::GenericError(format!(
                    "could not read icUSD history page [{start}, {end}) ({code:?}): {message}"
                ))
            })?;
    let outcome = scan_refund_history_with_archives(
        &record,
        start,
        end,
        history,
        |archive_id, method, requests| async move {
            let result: Result<(GetBlocksResult,), _> =
                ic_cdk::call(archive_id, &method, (requests,)).await;
            result
                .map(|(response,)| response)
                .map_err(|(code, message)| {
                    format!(
                        "ICRC-3 archive call to {} failed: {:?} {}",
                        archive_id, code, message
                    )
                })
        },
    )
    .await?;

    let RefundHistoryScan::CoveredAbsent = outcome else {
        return Ok((record, outcome));
    };
    let final_page = end == scan.snapshot_log_length;
    let updated = mutate_state(|s| {
        if s.icusd_ledger_principal != record.ledger
            || s.sp_burn_refunds_by_proof.get(&key) != Some(&record)
        {
            return Err(ProtocolError::GenericError(
                "SP refund journal changed before history page progress was committed".into(),
            ));
        }
        let current = s
            .sp_burn_refunds_by_proof
            .get_mut(&key)
            .expect("checked above");
        if final_page {
            current.history_scan = None;
            current.no_effect_evidence = Some(StoredSpBurnRefundNoEffectEvidence {
                ledger: record.ledger,
                recipient: record.caller,
                amount_e8s: record.amount_e8s,
                op_nonce: record.op_nonce,
                created_at_time: record.refund_created_at_time,
                memo: record.refund_memo.clone(),
                log_length: scan.snapshot_log_length,
                too_old_rejected: false,
            });
        } else {
            let mut next_scan = scan.clone();
            next_scan.next_index = end;
            current.history_scan = Some(next_scan);
        }
        Ok(current.clone())
    })?;
    Ok((
        updated,
        if final_page {
            RefundHistoryScan::CompleteAbsent
        } else {
            RefundHistoryScan::CoveredAbsent
        },
    ))
}

/// Scan the bounded ledger response, following only archive callbacks advertised by
/// that response. Archive results are useful for finding a positive exact match;
/// missing, malformed, or unavailable ranges always make absence inconclusive.
async fn scan_refund_history_with_archives<F, Fut>(
    record: &StoredSpBurnRefund,
    start: u64,
    end: u64,
    history: GetBlocksResult,
    mut fetch_archive: F,
) -> Result<RefundHistoryScan, ProtocolError>
where
    F: FnMut(Principal, String, Vec<GetBlocksRequest>) -> Fut,
    Fut: std::future::Future<Output = Result<GetBlocksResult, String>>,
{
    let length = end
        .checked_sub(start)
        .ok_or_else(|| ProtocolError::GenericError("invalid bounded icUSD history range".into()))?;
    if length > MAX_REFUND_HISTORY_BLOCKS
        || history.log_length.0.to_u64().map_or(true, |n| n < end)
        || history.blocks.len() > MAX_REFUND_HISTORY_BLOCKS as usize
        || history.archived_blocks.len() > MAX_REFUND_ARCHIVE_CALLBACKS
    {
        return Ok(RefundHistoryScan::Incomplete);
    }

    let mut complete = true;
    let mut indexes = std::collections::BTreeSet::new();
    let mut found = Vec::<RefundHistoryMatch>::new();
    let mut direct_blocks = Vec::new();
    for block in &history.blocks {
        let Some(index) = block.id.0.to_u64() else {
            complete = false;
            continue;
        };
        if index < start || index >= end || !indexes.insert(index) {
            complete = false;
            continue;
        }
        direct_blocks.push((index, block.clone()));
    }
    for (index, block) in direct_blocks {
        match refund_identity_for_block(record, &block.block) {
            Ok(Some(identity)) => found.push((index, identity.0, identity.1, identity.2)),
            Ok(None) => {}
            Err(_) => complete = false,
        }
    }

    let mut callback_count = 0usize;
    let mut callback_blocks = 0u64;
    let mut archive_results = Vec::<(Vec<GetBlocksRequest>, GetBlocksResult)>::new();
    for archive in &history.archived_blocks {
        if archive.args.len() > MAX_REFUND_ARCHIVE_RANGES_PER_CALLBACK {
            complete = false;
            continue;
        }
        let requests = clipped_archive_requests(archive, start, end);
        if requests.is_empty() {
            continue;
        }
        if requests.len() > MAX_REFUND_ARCHIVE_RANGES_PER_CALLBACK {
            complete = false;
            continue;
        }
        callback_count += 1;
        let requested_count = requests.iter().try_fold(0u64, |sum, request| {
            request.length.0.to_u64().and_then(|n| sum.checked_add(n))
        });
        let Some(requested_count) = requested_count else {
            complete = false;
            continue;
        };
        callback_blocks = callback_blocks.saturating_add(requested_count);
        if callback_count > MAX_REFUND_ARCHIVE_CALLBACKS
            || callback_blocks > MAX_REFUND_HISTORY_BLOCKS
        {
            complete = false;
            break;
        }
        match fetch_archive(
            archive.callback.canister_id,
            archive.callback.method.clone(),
            requests.clone(),
        )
        .await
        {
            Ok(response) => archive_results.push((requests, response)),
            Err(_) => complete = false,
        }
    }

    for (requests, response) in archive_results {
        // Archive-to-archive recursion would make work and trust depth unclear;
        // the exact-block verifier remains available for a known block index.
        // This scanner follows only the ledger-advertised first-level callback.
        // A nested archive descriptor means this callback did not return a
        // self-contained range; do not accept even an otherwise matching block
        // from that malformed/partial response.
        if !response.archived_blocks.is_empty() {
            complete = false;
            continue;
        }
        let mut response_indexes = std::collections::BTreeSet::new();
        let mut valid_blocks: Vec<(u64, BlockWithId)> = Vec::new();
        let requested_count = requests.iter().try_fold(0u64, |sum, request| {
            request.length.0.to_u64().and_then(|n| sum.checked_add(n))
        });
        // Archive `log_length` is local to that archive canister, while block
        // IDs in its response remain ledger-global. Comparing it with `end`
        // (the ledger-global page endpoint) rejects valid archived prefixes.
        // Exact request membership and contiguous ID coverage below are the
        // evidence used for this bounded page instead.
        let mut response_valid = requested_count
            .is_some_and(|count| response.blocks.len() as u64 <= count)
            && response.blocks.len() <= MAX_REFUND_HISTORY_BLOCKS as usize;
        for block in response.blocks {
            let Some(index) = block.id.0.to_u64() else {
                response_valid = false;
                continue;
            };
            if index < start
                || index >= end
                || !requests
                    .iter()
                    .any(|request| request_covers_refund_index(request, index))
                || !response_indexes.insert(index)
            {
                response_valid = false;
                continue;
            }
            valid_blocks.push((index, block));
        }
        if !response_valid {
            complete = false;
            continue;
        }
        for (index, block) in valid_blocks {
            if !indexes.insert(index) {
                complete = false;
                continue;
            }
            match refund_identity_for_block(record, &block.block) {
                Ok(Some(identity)) => found.push((index, identity.0, identity.1, identity.2)),
                Ok(None) => {}
                Err(_) => complete = false,
            }
        }
        if requests.iter().any(|request| {
            let Some(request_start) = request.start.0.to_u64() else {
                return true;
            };
            let Some(request_length) = request.length.0.to_u64() else {
                return true;
            };
            (request_start..request_start.saturating_add(request_length))
                .any(|index| !indexes.contains(&index))
        }) {
            complete = false;
        }
    }

    let contiguous =
        indexes.len() as u64 == length && (start..end).all(|index| indexes.contains(&index));
    complete &= contiguous;
    if found.len() > 1 {
        return Ok(RefundHistoryScan::ConflictingMatches);
    }
    if let Some(found) = found.pop() {
        return Ok(RefundHistoryScan::Found(found));
    }
    Ok(if complete {
        RefundHistoryScan::CoveredAbsent
    } else {
        RefundHistoryScan::Incomplete
    })
}

fn clipped_archive_requests(
    archive: &ArchivedBlocks,
    start: u64,
    end: u64,
) -> Vec<GetBlocksRequest> {
    let mut ranges = Vec::new();
    for advertised in &archive.args {
        let Some(advertised_start) = advertised.start.0.to_u64() else {
            continue;
        };
        let Some(advertised_length) = advertised.length.0.to_u64() else {
            continue;
        };
        let Some(advertised_end) = advertised_start.checked_add(advertised_length) else {
            continue;
        };
        let clipped_start = start.max(advertised_start);
        let clipped_end = end.min(advertised_end);
        if clipped_start < clipped_end {
            ranges.push(GetBlocksRequest {
                start: Nat::from(clipped_start),
                length: Nat::from(clipped_end - clipped_start),
            });
        }
    }
    ranges
}

fn request_covers_refund_index(request: &GetBlocksRequest, index: u64) -> bool {
    let Some(start) = request.start.0.to_u64() else {
        return false;
    };
    let Some(length) = request.length.0.to_u64() else {
        return false;
    };
    start <= index && start.checked_add(length).is_some_and(|end| index < end)
}

fn refund_match_is_current_attempt(
    record: &StoredSpBurnRefund,
    matched: &RefundHistoryMatch,
) -> bool {
    matched.1 == record.op_nonce
        && matched.2 == record.refund_created_at_time
        && matched.3 == record.refund_memo
}

#[cfg(test)]
fn refund_block_matches(
    record: &StoredSpBurnRefund,
    block: &ICRC3Value,
) -> Result<bool, ProtocolError> {
    Ok(refund_identity_for_block(record, block)?.is_some())
}

fn refund_identity_for_block(
    record: &StoredSpBurnRefund,
    block: &ICRC3Value,
) -> Result<Option<(u128, u64, Vec<u8>)>, ProtocolError> {
    let decoded = crate::icrc3_proof::decode_block(block).map_err(|err| {
        ProtocolError::GenericError(format!(
            "could not decode bounded refund history block: {err}"
        ))
    })?;
    // This is a negative-proof scanner: an operation from an extension we do
    // not understand cannot be counted as evidence that the original mint was
    // absent. The shared decoder normalizes any numeric btype prefix (useful
    // for ICRC-1/2 transfers), so check the raw type here before classifying
    // unknown blocks as harmless non-mints.
    let known_block_type = match block {
        ICRC3Value::Map(fields) => match fields.get("btype") {
            Some(ICRC3Value::Text(btype)) => matches!(
                btype.as_str(),
                "1mint" | "1burn" | "1xfer" | "2xfer" | "2approve"
            ),
            Some(_) => false,
            // The bundled official icUSD ledger can encode a recognized
            // operation in tx.op without a top-level btype.
            None => matches!(fields.get("tx"), Some(ICRC3Value::Map(tx))
                if matches!(tx.get("op"), Some(ICRC3Value::Text(op))
                    if matches!(op.as_str(), "mint" | "burn" | "xfer" | "approve"))),
        },
        _ => false,
    };
    if !known_block_type {
        return Err(ProtocolError::GenericError(
            "unknown icUSD ledger block type; history absence is inconclusive".into(),
        ));
    }
    if decoded.op != "mint"
        || decoded.from.is_some()
        || decoded.to.as_ref() != Some(&default_account(record.caller))
        || decoded.amount != record.amount_e8s as u128
    {
        return Ok(None);
    }
    let Some(memo) = decoded.memo else {
        return Ok(None);
    };
    let Some(created_at_time) = decoded.created_at_time else {
        return Ok(None);
    };
    if memo == record.refund_memo && created_at_time == record.refund_created_at_time {
        return Ok(Some((record.op_nonce, created_at_time, memo)));
    }
    for (nonce, timestamp, historical_memo) in &record.attempt_history {
        if memo == *historical_memo && created_at_time == *timestamp {
            return Ok(Some((*nonce, *timestamp, memo)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use icrc_ledger_types::icrc3::archive::QueryArchiveFn;

    fn proof(vault_id_memo: u64) -> SpWritedownProof {
        SpWritedownProof {
            block_index: 91,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo,
        }
    }

    fn stored(caller: Principal, ledger: Principal, vault_id: u64) -> StoredSpBurnRefund {
        StoredSpBurnRefund {
            caller,
            vault_id,
            amount_e8s: 700_000_000,
            ledger,
            burn_block_index: 91,
            op_nonce: (5_000u128 << 64) | 33,
            refund_created_at_time: 5_000,
            refund_memo: memo_for_test(91, vault_id),
            refund_block_index: None,
            attempt_history: Vec::new(),
            history_scan: None,
            no_effect_evidence: None,
            attempt_no_effect_evidence: Vec::new(),
        }
    }

    fn memo_for_test(block: u64, vault_id: u64) -> Vec<u8> {
        [
            REFUND_MEMO_PREFIX.as_slice(),
            &block.to_be_bytes(),
            &vault_id.to_be_bytes(),
        ]
        .concat()
    }

    fn mint_block(record: &StoredSpBurnRefund, created_at_time: u64) -> ICRC3Value {
        use candid::Nat;
        use serde_bytes::ByteBuf;

        let mut tx = std::collections::BTreeMap::new();
        tx.insert("op".into(), ICRC3Value::Text("mint".into()));
        tx.insert(
            "to".into(),
            ICRC3Value::Array(vec![ICRC3Value::Blob(ByteBuf::from(
                record.caller.as_slice().to_vec(),
            ))]),
        );
        tx.insert("amt".into(), ICRC3Value::Nat(Nat::from(record.amount_e8s)));
        tx.insert(
            "memo".into(),
            ICRC3Value::Blob(ByteBuf::from(record.refund_memo.clone())),
        );
        tx.insert("ts".into(), ICRC3Value::Nat(Nat::from(created_at_time)));
        let mut block = std::collections::BTreeMap::new();
        block.insert("btype".into(), ICRC3Value::Text("1mint".into()));
        block.insert("tx".into(), ICRC3Value::Map(tx));
        ICRC3Value::Map(block)
    }

    fn history(
        log_length: u64,
        blocks: Vec<BlockWithId>,
        archived_blocks: Vec<ArchivedBlocks>,
    ) -> GetBlocksResult {
        GetBlocksResult {
            log_length: Nat::from(log_length),
            blocks,
            archived_blocks,
        }
    }

    fn archive_descriptor(start: u64, length: u64, archive: Principal) -> ArchivedBlocks {
        ArchivedBlocks {
            args: vec![GetBlocksRequest {
                start: Nat::from(start),
                length: Nat::from(length),
            }],
            callback: QueryArchiveFn::new(archive, "archive_blocks_v2"),
        }
    }

    fn empty_block(index: u64) -> BlockWithId {
        BlockWithId {
            id: Nat::from(index),
            block: ICRC3Value::Map(std::collections::BTreeMap::new()),
        }
    }

    fn nonmatching_block(record: &StoredSpBurnRefund, index: u64) -> BlockWithId {
        BlockWithId {
            id: Nat::from(index),
            block: mint_block(record, record.refund_created_at_time.saturating_add(1)),
        }
    }

    #[test]
    fn historical_refund_recovery_matches_the_full_persisted_operation() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        assert!(
            refund_block_matches(&record, &mint_block(&record, record.refund_created_at_time))
                .unwrap()
        );
        assert!(!refund_block_matches(
            &record,
            &mint_block(&record, record.refund_created_at_time + 1)
        )
        .unwrap());
        let mut wrong_memo = record.clone();
        wrong_memo.refund_memo.push(0);
        assert!(!refund_block_matches(
            &wrong_memo,
            &mint_block(&record, record.refund_created_at_time)
        )
        .unwrap());

        let mut retried = record.clone();
        let old_timestamp = record.refund_created_at_time;
        let old_nonce = record.op_nonce;
        retried
            .attempt_history
            .push((old_nonce, old_timestamp, record.refund_memo.clone()));
        retried.refund_created_at_time = old_timestamp + 1;
        retried.op_nonce = ((old_timestamp + 1) as u128) << 64;
        assert!(refund_block_matches(&retried, &mint_block(&record, old_timestamp)).unwrap());
    }

    #[test]
    fn four_prior_retry_identities_allow_five_total_and_block_a_sixth() {
        let mut record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        assert!(!refund_retry_cap_exhausted(&record));
        for index in 0..MAX_SP_BURN_REFUND_ATTEMPTS {
            let timestamp = record.refund_created_at_time + index as u64 + 1;
            let mut memo = record.refund_memo.clone();
            memo[8] ^= (index + 1) as u8;
            record.attempt_history.push((
                ((timestamp as u128) << 64) | index as u128,
                timestamp,
                memo,
            ));
            assert_eq!(
                refund_retry_cap_exhausted(&record),
                index + 1 == MAX_SP_BURN_REFUND_ATTEMPTS
            );
        }
        assert_eq!(record.attempt_history.len(), MAX_SP_BURN_REFUND_ATTEMPTS);
        assert!(refund_retry_cap_exhausted(&record));
    }

    #[test]
    fn bounded_history_recovers_exact_refund_from_advertised_archive_callback() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let archive = Principal::from_slice(&[8]);
        let record_for_archive = record.clone();
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            4,
            history(
                4,
                vec![
                    BlockWithId {
                        id: Nat::from(0u64),
                        block: mint_block(&record, record.refund_created_at_time + 1),
                    },
                    BlockWithId {
                        id: Nat::from(2u64),
                        block: mint_block(&record, record.refund_created_at_time + 1),
                    },
                    BlockWithId {
                        id: Nat::from(3u64),
                        block: mint_block(&record, record.refund_created_at_time + 1),
                    },
                ],
                vec![archive_descriptor(1, 1, archive)],
            ),
            move |target, method, requests| {
                let archive_record = record_for_archive.clone();
                async move {
                    assert_eq!(target, archive);
                    assert_eq!(method, "archive_blocks_v2");
                    assert_eq!(
                        requests,
                        vec![GetBlocksRequest {
                            start: Nat::from(1u64),
                            length: Nat::from(1u64),
                        }]
                    );
                    Ok(history(
                        2,
                        vec![BlockWithId {
                            id: Nat::from(1u64),
                            block: mint_block(
                                &archive_record,
                                archive_record.refund_created_at_time,
                            ),
                        }],
                        vec![],
                    ))
                }
            },
        ));
        assert!(matches!(
            result.unwrap(),
            RefundHistoryScan::Found((1, _, _, _))
        ));
    }

    #[test]
    fn incomplete_or_wrong_archive_responses_never_prove_absence() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let archive = Principal::from_slice(&[8]);
        let run = |callback_result: Result<GetBlocksResult, String>| {
            futures::executor::block_on(scan_refund_history_with_archives(
                &record,
                0,
                2,
                history(
                    2,
                    vec![empty_block(0)],
                    vec![archive_descriptor(1, 1, archive)],
                ),
                move |target, method, requests| {
                    assert_eq!(target, archive);
                    assert_eq!(method, "archive_blocks_v2");
                    assert_eq!(requests.len(), 1);
                    let callback_result = callback_result.clone();
                    async move { callback_result }
                },
            ))
            .unwrap()
        };

        assert!(matches!(
            run(Ok(history(2, vec![empty_block(0)], vec![]))),
            RefundHistoryScan::Incomplete
        ));

        assert!(matches!(
            run(Err("archive unavailable".into())),
            RefundHistoryScan::Incomplete
        ));

        assert!(matches!(
            run(Ok(history(2, vec![], vec![]))),
            RefundHistoryScan::Incomplete
        ));

        let malformed_callback = Ok(history(
            2,
            vec![
                BlockWithId {
                    id: Nat::from(1u64),
                    block: mint_block(&record, record.refund_created_at_time),
                },
                empty_block(0),
            ],
            vec![],
        ));
        assert!(matches!(
            run(malformed_callback),
            RefundHistoryScan::Incomplete
        ));
    }

    #[test]
    fn exact_match_in_nested_archive_response_is_rejected() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let archive = Principal::from_slice(&[8]);
        let nested_archive = Principal::from_slice(&[9]);
        let record_for_archive = record.clone();
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            2,
            history(
                2,
                vec![empty_block(0)],
                vec![archive_descriptor(1, 1, archive)],
            ),
            move |_, _, _| {
                let record = record_for_archive.clone();
                async move {
                    Ok(history(
                        2,
                        vec![BlockWithId {
                            id: Nat::from(1u64),
                            block: mint_block(&record, record.refund_created_at_time),
                        }],
                        vec![archive_descriptor(1, 1, nested_archive)],
                    ))
                }
            },
        ));
        assert!(matches!(result.unwrap(), RefundHistoryScan::Incomplete));
    }

    #[test]
    fn distinct_direct_and_archived_matches_are_reported_as_conflicting() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let archive = Principal::from_slice(&[8]);
        let archive_record = record.clone();
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            2,
            history(
                2,
                vec![BlockWithId {
                    id: Nat::from(0u64),
                    block: mint_block(&record, record.refund_created_at_time),
                }],
                vec![archive_descriptor(1, 1, archive)],
            ),
            move |_, _, _| {
                let archive_record = archive_record.clone();
                async move {
                    Ok(history(
                        2,
                        vec![BlockWithId {
                            id: Nat::from(1u64),
                            block: mint_block(
                                &archive_record,
                                archive_record.refund_created_at_time,
                            ),
                        }],
                        vec![],
                    ))
                }
            },
        ));
        assert!(matches!(
            result.unwrap(),
            RefundHistoryScan::ConflictingMatches
        ));
    }

    #[test]
    fn oversized_history_request_is_incomplete_without_archive_calls() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let total = MAX_REFUND_HISTORY_BLOCKS + 1;
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            total,
            history(total, vec![], vec![]),
            |_, _, _| async { panic!("oversized history must not dispatch archive callbacks") },
        ));
        assert!(matches!(result.unwrap(), RefundHistoryScan::Incomplete));
    }

    #[test]
    fn prefix_pages_cover_history_beyond_2048_and_find_an_older_positive_match() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let page_end = MAX_REFUND_HISTORY_BLOCKS;
        let first_page = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            page_end,
            history(
                page_end + 2,
                (0..page_end)
                    .map(|index| nonmatching_block(&record, index))
                    .collect(),
                vec![],
            ),
            |_, _, _| async { panic!("no archive callback was advertised") },
        ));
        assert!(matches!(
            first_page.unwrap(),
            RefundHistoryScan::CoveredAbsent
        ));

        let older_match = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            page_end,
            page_end + 2,
            history(
                page_end + 2,
                vec![
                    BlockWithId {
                        id: Nat::from(page_end),
                        block: mint_block(&record, record.refund_created_at_time),
                    },
                    nonmatching_block(&record, page_end + 1),
                ],
                vec![],
            ),
            |_, _, _| async { panic!("no archive callback was advertised") },
        ));
        let RefundHistoryScan::Found(found) = older_match.unwrap() else {
            panic!("expected an exact match in the next page")
        };
        assert_eq!(found.0, page_end);
        assert!(refund_match_is_current_attempt(&record, &found));
    }

    #[test]
    fn no_effect_evidence_binds_every_transfer_identity_field() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let evidence = StoredSpBurnRefundNoEffectEvidence {
            ledger: record.ledger,
            recipient: record.caller,
            amount_e8s: record.amount_e8s,
            op_nonce: record.op_nonce,
            created_at_time: record.refund_created_at_time,
            memo: record.refund_memo.clone(),
            log_length: 9,
            too_old_rejected: false,
        };
        assert!(no_effect_evidence_matches(&record, &evidence));
        let mut wrong = evidence.clone();
        wrong.ledger = Principal::from_slice(&[9]);
        assert!(!no_effect_evidence_matches(&record, &wrong));
        let mut wrong_timestamp = evidence;
        wrong_timestamp.created_at_time += 1;
        assert!(!no_effect_evidence_matches(&record, &wrong_timestamp));
        let mut wrong_recipient = StoredSpBurnRefundNoEffectEvidence {
            ledger: record.ledger,
            recipient: Principal::from_slice(&[9]),
            amount_e8s: record.amount_e8s,
            op_nonce: record.op_nonce,
            created_at_time: record.refund_created_at_time,
            memo: record.refund_memo.clone(),
            log_length: 9,
            too_old_rejected: false,
        };
        assert!(!no_effect_evidence_matches(&record, &wrong_recipient));
        wrong_recipient.recipient = record.caller;
        wrong_recipient.amount_e8s += 1;
        assert!(!no_effect_evidence_matches(&record, &wrong_recipient));
    }

    #[test]
    fn empty_snapshot_alone_cannot_rotate_but_typed_too_old_and_complete_prefix_can() {
        let caller = Principal::from_slice(&[2]);
        let ledger = Principal::from_slice(&[3]);
        let key = (SpProofLedger::IcusdBurn, 91);
        let mut record = stored(caller, ledger, 44);
        let mut state = crate::state::State::default();
        state.stability_pool_canister = Some(caller);
        state.icusd_ledger_principal = ledger;
        state.consumed_writedown_proofs.insert(key);
        state.sp_burn_refunds_by_proof.insert(key, record.clone());
        crate::state::replace_state(state);

        assert!(rotate_refund_identity(key, record.clone(), record.refund_created_at_time + 1).is_err());
        assert_eq!(
            read_state(|s| s.sp_burn_refunds_by_proof[&key].clone()),
            record
        );

        record.no_effect_evidence = Some(StoredSpBurnRefundNoEffectEvidence {
            ledger,
            recipient: caller,
            amount_e8s: record.amount_e8s,
            op_nonce: record.op_nonce,
            created_at_time: record.refund_created_at_time,
            memo: record.refund_memo.clone(),
            log_length: 0,
            too_old_rejected: false,
        });
        assert!(rotate_refund_identity(key, record.clone(), record.refund_created_at_time + 1).is_err());
        crate::state::mutate_state(|s| {
            s.sp_burn_refunds_by_proof.insert(key, record.clone());
        });
        let exact_too_old = mark_refund_too_old_rejected(key, record.clone()).unwrap();
        let rotated = rotate_refund_identity(
            key,
            exact_too_old.clone(),
            record.refund_created_at_time + 1,
        )
        .unwrap();
        assert_eq!(rotated.attempt_history.len(), 1);
        assert_eq!(rotated.attempt_history[0].0, record.op_nonce);
        assert_eq!(rotated.attempt_history[0].1, record.refund_created_at_time);
        assert_eq!(rotated.attempt_history[0].2, record.refund_memo);
        assert_eq!(rotated.attempt_no_effect_evidence.len(), 1);
        assert!(rotated.attempt_no_effect_evidence[0].too_old_rejected);
        assert!(rotated.refund_created_at_time > record.refund_created_at_time);
        assert_eq!(rotated.refund_memo.len(), 32);

        // The previous identity is tombstoned and remains part of history
        // matching, so a late indexed mint is still recognized as an outcome.
        let old_match = (
            0,
            record.op_nonce,
            record.refund_created_at_time,
            record.refund_memo.clone(),
        );
        assert!(!refund_match_is_current_attempt(&rotated, &old_match));

        record = rotated;
        for index in 0..MAX_SP_BURN_REFUND_ATTEMPTS {
            let timestamp = record.refund_created_at_time + index as u64 + 10;
            let nonce = ((timestamp as u128) << 64) | index as u128;
            let mut memo = record.refund_memo.clone();
            memo[8] ^= (index + 1) as u8;
            record.attempt_history.push((nonce, timestamp, memo));
        }
        crate::state::mutate_state(|s| {
            s.sp_burn_refunds_by_proof.insert(key, record.clone());
        });
        assert!(refund_retry_cap_exhausted(&record));
        assert!(rotate_refund_identity(key, record.clone(), record.refund_created_at_time + 1).is_err());
        assert_eq!(
            read_state(|s| s.sp_burn_refunds_by_proof[&key].clone()),
            record
        );
    }

    #[test]
    fn oversized_response_shapes_fail_closed_before_archive_dispatch() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let archive = Principal::from_slice(&[8]);
        let oversized_direct = history(
            1,
            (0..=MAX_REFUND_HISTORY_BLOCKS).map(empty_block).collect(),
            vec![],
        );
        let direct_result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            1,
            oversized_direct,
            |_, _, _| async { panic!("oversized direct response must not dispatch callbacks") },
        ));
        assert!(matches!(
            direct_result.unwrap(),
            RefundHistoryScan::Incomplete
        ));

        let mut oversized_descriptor = archive_descriptor(0, 1, archive);
        oversized_descriptor.args = (0..=MAX_REFUND_ARCHIVE_RANGES_PER_CALLBACK)
            .map(|_| GetBlocksRequest {
                start: Nat::from(0u64),
                length: Nat::from(1u64),
            })
            .collect();
        let descriptor_result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            1,
            history(1, vec![], vec![oversized_descriptor]),
            |_, _, _| async { panic!("oversized descriptor must not dispatch callback") },
        ));
        assert!(matches!(
            descriptor_result.unwrap(),
            RefundHistoryScan::Incomplete
        ));
    }

    #[test]
    fn history_match_from_prior_tombstone_is_not_current_attempt_evidence() {
        let mut record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let old = (
            record.op_nonce,
            record.refund_created_at_time,
            record.refund_memo.clone(),
        );
        record.attempt_history.push(old.clone());
        record.op_nonce += 1 << 64;
        record.refund_created_at_time += 1;
        let old_match = (7, old.0, old.1, old.2);
        let current_match = (
            8,
            record.op_nonce,
            record.refund_created_at_time,
            record.refund_memo.clone(),
        );
        assert!(!refund_match_is_current_attempt(&record, &old_match));
        assert!(refund_match_is_current_attempt(&record, &current_match));
    }

    #[test]
    fn positive_current_match_is_usable_from_tail_while_absence_stays_incomplete() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let start = 10_000;
        let end = start + 2;
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            start,
            end,
            history(
                end,
                vec![BlockWithId {
                    id: Nat::from(start + 1),
                    block: mint_block(&record, record.refund_created_at_time),
                }],
                vec![],
            ),
            |_, _, _| async { panic!("no archive callback was advertised") },
        ));
        let RefundHistoryScan::Found(found) = result.unwrap() else {
            panic!("an exact positive current-tuple block is useful in a bounded tail")
        };
        assert_eq!(found.0, start + 1);
        assert!(refund_match_is_current_attempt(&record, &found));

        let absent = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            start,
            end,
            history(
                end,
                vec![
                    nonmatching_block(&record, start),
                    nonmatching_block(&record, start + 1),
                ],
                vec![],
            ),
            |_, _, _| async { panic!("no archive callback was advertised") },
        ));
        assert!(matches!(absent.unwrap(), RefundHistoryScan::CoveredAbsent));
    }

    #[test]
    fn unknown_block_type_cannot_be_skipped_as_no_effect_evidence() {
        let record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        let mut block = mint_block(&record, record.refund_created_at_time);
        let ICRC3Value::Map(fields) = &mut block else {
            panic!("test block is a map")
        };
        fields.insert("btype".into(), ICRC3Value::Text("9mint".into()));

        assert!(refund_identity_for_block(&record, &block).is_err());
        let result = futures::executor::block_on(scan_refund_history_with_archives(
            &record,
            0,
            1,
            history(
                1,
                vec![BlockWithId {
                    id: Nat::from(0u64),
                    block,
                }],
                vec![],
            ),
            |_, _, _| async { panic!("no archive callback was advertised") },
        ))
        .expect("unknown types are an incomplete scan result, not a call error");
        assert!(matches!(result, RefundHistoryScan::Incomplete));

        let mut untyped = mint_block(&record, record.refund_created_at_time);
        let ICRC3Value::Map(fields) = &mut untyped else {
            panic!("test block is a map")
        };
        fields.remove("btype");
        if let Some(ICRC3Value::Map(tx)) = fields.get_mut("tx") {
            tx.insert("op".into(), ICRC3Value::Text("mint".into()));
        }
        assert!(refund_identity_for_block(&record, &untyped).is_ok());
        let ICRC3Value::Map(fields) = &mut untyped else {
            panic!("test block is a map")
        };
        if let Some(ICRC3Value::Map(tx)) = fields.get_mut("tx") {
            tx.insert("op".into(), ICRC3Value::Text("MINT".into()));
        }
        assert!(refund_identity_for_block(&record, &untyped).is_err());
    }

    #[test]
    fn exhausted_attempt_history_still_allows_positive_current_receipt_only() {
        let mut record = stored(Principal::from_slice(&[2]), Principal::from_slice(&[3]), 44);
        for index in 0..MAX_SP_BURN_REFUND_ATTEMPTS {
            let timestamp = record.refund_created_at_time + index as u64 + 1;
            record.attempt_history.push((
                ((timestamp as u128) << 64) | index as u128,
                timestamp,
                [record.refund_memo.as_slice(), &[index as u8]].concat(),
            ));
        }
        assert!(refund_retry_cap_exhausted(&record));
        let current = (
            11,
            record.op_nonce,
            record.refund_created_at_time,
            record.refund_memo.clone(),
        );
        assert!(refund_match_is_current_attempt(&record, &current));
        let old = (
            10,
            record.attempt_history[0].0,
            record.attempt_history[0].1,
            record.attempt_history[0].2.clone(),
        );
        assert!(!refund_match_is_current_attempt(&record, &old));
    }

    #[test]
    fn refund_identity_binds_sp_vault_amount_ledger_and_burn_proof() {
        let caller = Principal::from_slice(&[2]);
        let ledger = Principal::from_slice(&[3]);
        let record = stored(caller, ledger, 44);
        assert!(refund_matches(
            &record,
            caller,
            44,
            700_000_000,
            &proof(44),
            ledger
        ));
        assert!(!refund_matches(
            &record,
            Principal::from_slice(&[4]),
            44,
            700_000_000,
            &proof(44),
            ledger
        ));
        assert!(!refund_matches(
            &record,
            caller,
            45,
            700_000_000,
            &proof(45),
            ledger
        ));
        assert!(!refund_matches(
            &record,
            caller,
            44,
            700_000_001,
            &proof(44),
            ledger
        ));
        assert!(!refund_matches(
            &record,
            caller,
            44,
            700_000_000,
            &proof(44),
            Principal::from_slice(&[5])
        ));
    }

    #[test]
    fn receipt_uses_original_pool_and_the_fixed_pending_transfer_tuple() {
        let caller = Principal::from_slice(&[2]);
        let record = stored(caller, Principal::from_slice(&[3]), 44);
        let receipt = receipt_from_record(&record, 102);
        assert_eq!(receipt.recipient, caller);
        assert_eq!(receipt.vault_id, 44);
        assert_eq!(receipt.amount_e8s, 700_000_000);
        assert_eq!(receipt.burn_block_index, 91);
        assert_eq!(receipt.refund_block_index, 102);
        assert_eq!(receipt.refund_created_at_time, 5_000);
        assert_eq!(receipt.refund_memo, record.refund_memo);
    }
}
