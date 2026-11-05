//! Durable stablecoin withdrawal delivery. Pending rows are liabilities already
//! removed from spendable depositor books, separate from collateral claims.
use crate::state::{mutate_state, read_state, StabilityPoolState};
use crate::types::{
    PendingOutboundPayout, PendingOutboundPayoutStatus, PoolEventType, StabilityPoolError,
};
use candid::Principal;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{TransferArg, TransferError},
};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

const MAX_PENDING_WITHDRAWALS: usize = 10_000;
thread_local! {
    static ACTIVE: RefCell<BTreeSet<(Principal, Principal)>> = RefCell::new(BTreeSet::new());
}

struct Guard((Principal, Principal));
impl Guard {
    fn reserve(owner: Principal, ledger: Principal) -> Result<Self, StabilityPoolError> {
        let key = (owner, ledger);
        ACTIVE.with(|active| {
            if active.borrow_mut().insert(key) {
                Ok(Self(key))
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        })
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

fn unresolved(reason: impl Into<String>) -> StabilityPoolError {
    StabilityPoolError::LedgerTransferFailed {
        reason: reason.into(),
    }
}

fn memo(owner: Principal, ledger: Principal, timestamp: u64) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(b"rumi-sp-stable-withdrawal-v1\0");
    digest.update((owner.as_slice().len() as u64).to_be_bytes());
    digest.update(owner.as_slice());
    digest.update((ledger.as_slice().len() as u64).to_be_bytes());
    digest.update(ledger.as_slice());
    digest.update(timestamp.to_be_bytes());
    digest.finalize().to_vec()
}

fn same_attempt(saved: &PendingOutboundPayout, expected: &PendingOutboundPayout) -> bool {
    saved.dispatch_in_flight
        && saved.dispatch_generation == expected.dispatch_generation
        && saved.gross_amount == expected.gross_amount
        && saved.transfer_amount == expected.transfer_amount
        && saved.transfer_fee == expected.transfer_fee
        && saved.recipient == expected.recipient
        && saved.from_subaccount == expected.from_subaccount
        && saved.transfer_memo == expected.transfer_memo
        && saved.transfer_created_at_time_ns == expected.transfer_created_at_time_ns
}

impl StabilityPoolState {
    pub fn pending_withdrawal(
        &self,
        owner: Principal,
        ledger: Principal,
    ) -> Option<PendingOutboundPayout> {
        self.pending_withdrawals
            .as_ref()?
            .get(&(owner, ledger))
            .cloned()
    }

    pub fn pending_withdrawals_for(&self, owner: &Principal) -> Vec<PendingOutboundPayoutStatus> {
        self.pending_withdrawals
            .iter()
            .flat_map(|rows| rows.iter())
            .filter(|((caller, _), _)| caller == owner)
            .map(|((_, ledger), payout)| PendingOutboundPayoutStatus {
                ledger: *ledger,
                gross_amount: payout.gross_amount,
                transfer_amount: payout.transfer_amount,
                transfer_fee: payout.transfer_fee,
                recipient: payout.recipient,
                from_subaccount: payout.from_subaccount.map(|v| v.to_vec()),
                transfer_memo: payout.transfer_memo.clone(),
                transfer_created_at_time_ns: payout.transfer_created_at_time_ns,
                dispatch_in_flight: payout.dispatch_in_flight,
                ambiguous_seen: payout.ambiguous_seen,
                last_error: payout.last_error.clone(),
                reconciliation_attempts: payout.reconciliation_attempts,
                last_reconciliation_at_ns: payout.last_reconciliation_at_ns,
                candidate_block_index: payout.candidate_block_index,
                candidate_block_index_raw: payout.candidate_block_index_raw.clone(),
            })
            .collect()
    }

    /// All validation precedes the debit and journal write. No await occurs
    /// between them, so both commit or roll back in the same IC message.
    pub(crate) fn reserve_withdrawal(
        &mut self,
        owner: Principal,
        ledger: Principal,
        gross: u64,
        fee: u64,
        live_balance: u64,
        now: u64,
    ) -> Result<PendingOutboundPayout, StabilityPoolError> {
        if self.configuration.emergency_pause {
            return Err(StabilityPoolError::EmergencyPaused);
        }
        if owner == Principal::anonymous() {
            return Err(StabilityPoolError::Unauthorized);
        }
        if !self.stablecoin_registry.contains_key(&ledger) {
            return Err(StabilityPoolError::TokenNotAccepted { ledger });
        }
        let rows = self.pending_withdrawals.as_ref();
        if rows.is_some_and(|rows| {
            rows.contains_key(&(owner, ledger)) || rows.len() >= MAX_PENDING_WITHDRAWALS
        }) {
            return Err(StabilityPoolError::SystemBusy);
        }
        if gross <= fee {
            return Err(StabilityPoolError::AmountTooLow {
                minimum_e8s: fee.saturating_add(1),
            });
        }
        let balance = self
            .deposits
            .get(&owner)
            .ok_or(StabilityPoolError::NoPositionFound)?
            .stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0);
        if balance < gross {
            return Err(StabilityPoolError::InsufficientBalance {
                token: ledger,
                required: gross,
                available: balance,
            });
        }
        let total = self
            .total_stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0);
        if total < gross || live_balance < total {
            return Err(StabilityPoolError::InsufficientPoolBalance);
        }
        let timestamp = self.allocate_outbound_payout_timestamp(now)?;
        let payout = PendingOutboundPayout {
            gross_amount: gross,
            transfer_amount: gross - fee,
            transfer_fee: fee,
            recipient: Account {
                owner,
                subaccount: None,
            },
            from_subaccount: None,
            transfer_memo: memo(owner, ledger, timestamp),
            transfer_created_at_time_ns: timestamp,
            dispatch_in_flight: true,
            ambiguous_seen: false,
            last_error: None,
            dispatch_generation: 1,
            reconciliation_attempts: 0,
            last_reconciliation_at_ns: None,
            candidate_block_index: None,
            candidate_block_index_raw: None,
        };
        let position = self
            .deposits
            .get_mut(&owner)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if balance == gross {
            position.stablecoin_balances.remove(&ledger);
        } else {
            position.stablecoin_balances.insert(ledger, balance - gross);
        }
        // Retain even an empty position until terminal outcome so rejection
        // cannot lose its opt-outs, payout addresses, or lifetime counters.
        self.total_stablecoin_balances.insert(ledger, total - gross);
        self.pending_withdrawals
            .get_or_insert_with(BTreeMap::new)
            .insert((owner, ledger), payout.clone());
        Ok(payout)
    }

    pub fn reconcile_pending_withdrawals_after_upgrade(&mut self) {
        let rows = self.pending_withdrawals.get_or_insert_with(BTreeMap::new);
        for payout in rows.values_mut() {
            if payout.dispatch_in_flight {
                payout.dispatch_in_flight = false;
                payout.ambiguous_seen = true;
                payout.last_error =
                    Some("withdrawal continuation interrupted; exact tuple retained".into());
            }
        }
    }

    /// Called only while the owner/ledger RAII guard is held. Any retained
    /// in-flight flag therefore belongs to an interrupted earlier callback.
    pub(crate) fn retry_withdrawal(
        &mut self,
        owner: Principal,
        ledger: Principal,
        gross: u64,
    ) -> Result<PendingOutboundPayout, StabilityPoolError> {
        let payout = self
            .pending_withdrawals
            .as_mut()
            .and_then(|r| r.get_mut(&(owner, ledger)))
            .ok_or(StabilityPoolError::SystemBusy)?;
        if payout.gross_amount != gross {
            return Err(StabilityPoolError::SystemBusy);
        }
        let generation = payout
            .dispatch_generation
            .checked_add(1)
            .ok_or(StabilityPoolError::SystemBusy)?;
        if payout.dispatch_in_flight {
            payout.ambiguous_seen = true;
        }
        payout.dispatch_in_flight = true;
        payout.dispatch_generation = generation;
        Ok(payout.clone())
    }

    pub(crate) fn hold_withdrawal(
        &mut self,
        owner: Principal,
        ledger: Principal,
        expected: &PendingOutboundPayout,
        reason: String,
        candidate: Option<(Option<u64>, String)>,
    ) -> bool {
        let Some(payout) = self
            .pending_withdrawals
            .as_mut()
            .and_then(|r| r.get_mut(&(owner, ledger)))
        else {
            return false;
        };
        if !same_attempt(payout, expected) {
            return false;
        }
        if let Some((index, raw)) = candidate {
            if payout
                .candidate_block_index_raw
                .as_ref()
                .is_some_and(|saved| saved != &raw)
            {
                payout.last_error =
                    Some("conflicting withdrawal receipt candidates; tuple retained".into());
                payout.dispatch_in_flight = false;
                payout.ambiguous_seen = true;
                return false;
            }
            payout.candidate_block_index = index;
            payout.candidate_block_index_raw = Some(raw);
        }
        payout.dispatch_in_flight = false;
        payout.ambiguous_seen = true;
        payout.last_error = Some(reason.chars().take(512).collect());
        true
    }

    pub(crate) fn finish_withdrawal(
        &mut self,
        owner: Principal,
        ledger: Principal,
        expected: &PendingOutboundPayout,
        now: u64,
    ) -> bool {
        let Some(saved) = self
            .pending_withdrawals
            .as_ref()
            .and_then(|r| r.get(&(owner, ledger)))
        else {
            return false;
        };
        if !same_attempt(saved, expected) {
            return false;
        }
        self.pending_withdrawals
            .as_mut()
            .unwrap()
            .remove(&(owner, ledger));
        // Another asset withdrawal/claim or incoming deposit may still need
        // this original position's metadata when its callback resumes.
        let has_other_pending = self.pending_withdrawals.as_ref().is_some_and(|rows| {
            rows.keys()
                .any(|(pending_owner, _)| *pending_owner == owner)
        }) || self.pending_outbound_payouts.as_ref().is_some_and(|rows| {
            rows.keys()
                .any(|(pending_owner, _)| *pending_owner == owner)
        }) || self
            .pending_deposit_intents
            .as_ref()
            .is_some_and(|rows| rows.contains_key(&owner));
        if !has_other_pending && self.deposits.get(&owner).is_some_and(|p| p.is_empty()) {
            self.deposits.remove(&owner);
        }
        self.push_event_at(
            owner,
            PoolEventType::Withdraw {
                token_ledger: ledger,
                amount: expected.gross_amount,
            },
            now,
        );
        true
    }

    /// A definitive response proves no effect only for the first attempt.
    /// Once any dispatch was ambiguous, later errors never restore credit.
    pub(crate) fn reject_withdrawal(
        &mut self,
        owner: Principal,
        ledger: Principal,
        expected: &PendingOutboundPayout,
    ) -> Result<bool, StabilityPoolError> {
        let Some(saved) = self
            .pending_withdrawals
            .as_ref()
            .and_then(|r| r.get(&(owner, ledger)))
        else {
            return Ok(false);
        };
        if !same_attempt(saved, expected) {
            return Ok(false);
        }
        if saved.ambiguous_seen {
            return Ok(false);
        }
        let position = self
            .deposits
            .get_mut(&owner)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let balance = position
            .stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0)
            .checked_add(expected.gross_amount)
            .ok_or(StabilityPoolError::SystemBusy)?;
        let total = self
            .total_stablecoin_balances
            .get(&ledger)
            .copied()
            .unwrap_or(0)
            .checked_add(expected.gross_amount)
            .ok_or(StabilityPoolError::SystemBusy)?;
        position.stablecoin_balances.insert(ledger, balance);
        self.total_stablecoin_balances.insert(ledger, total);
        self.pending_withdrawals
            .as_mut()
            .unwrap()
            .remove(&(owner, ledger));
        Ok(true)
    }
}

pub async fn withdraw(ledger: Principal, gross: u64) -> Result<(), StabilityPoolError> {
    // Reject a ledger already reserved by an absorb before inspecting caller
    // or making any ledger call, matching the existing balance-operation gate.
    crate::ensure_pool_token_balance_mutation_allowed(&[ledger])?;
    let owner = ic_cdk::caller();
    if owner == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _guard = Guard::reserve(owner, ledger)?;
    crate::ensure_pool_token_balance_mutation_allowed(&[ledger])?;
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    let pending = read_state(|s| s.pending_withdrawal(owner, ledger));
    let payout = if pending.is_some() {
        mutate_state(|s| s.retry_withdrawal(owner, ledger, gross))?
    } else {
        if !read_state(|s| s.stablecoin_registry.contains_key(&ledger)) {
            return Err(StabilityPoolError::TokenNotAccepted { ledger });
        }
        let fee = crate::deposits::current_ledger_transfer_fee(ledger).await?;
        let live = crate::deposits::ledger_pool_balance(ledger)
            .await
            .ok_or_else(|| {
                unresolved("current ledger balance unavailable; withdrawal unchanged")
            })?;
        crate::ensure_pool_token_balance_mutation_allowed(&[ledger])?;
        mutate_state(|s| {
            s.reserve_withdrawal(owner, ledger, gross, fee, live, ic_cdk::api::time())
        })?
    };
    let _balance_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
    dispatch(owner, ledger, payout).await
}

async fn prove_and_finish(
    owner: Principal,
    ledger: Principal,
    payout: &PendingOutboundPayout,
    block: u64,
) -> Result<(), StabilityPoolError> {
    if let Err(reason) =
        crate::deposits::verify_collateral_payout_block(ledger, block, payout).await
    {
        mutate_state(|s| {
            s.hold_withdrawal(
                owner,
                ledger,
                payout,
                reason.clone(),
                // A failed proof does not authenticate this candidate. Keep
                // any previously bound receipt, but never pin user input.
                None,
            )
        });
        return Err(unresolved(reason));
    }
    if !mutate_state(|s| s.finish_withdrawal(owner, ledger, payout, ic_cdk::api::time())) {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

/// Only the immediate authenticated transfer callback calls this helper.
/// Historical candidate indices and manual reconciliation cannot use it.
fn finish_confirmed_callback(
    state: &mut StabilityPoolState,
    owner: Principal,
    ledger: Principal,
    payout: &PendingOutboundPayout,
    reply: &Result<candid::Nat, TransferError>,
    now: u64,
) -> bool {
    if !matches!(reply, Ok(_) | Err(TransferError::Duplicate { .. })) {
        return false;
    }
    state.finish_withdrawal(owner, ledger, payout, now)
}

async fn dispatch(
    owner: Principal,
    ledger: Principal,
    payout: PendingOutboundPayout,
) -> Result<(), StabilityPoolError> {
    if payout.candidate_block_index_raw.is_some() {
        return match payout.candidate_block_index {
            Some(block) => prove_and_finish(owner, ledger, &payout, block).await,
            None => {
                mutate_state(|s| {
                    s.hold_withdrawal(
                        owner,
                        ledger,
                        &payout,
                        "receipt index exceeds supported range; tuple retained".into(),
                        None,
                    )
                });
                Err(unresolved(
                    "withdrawal receipt index exceeds supported range",
                ))
            }
        };
    }
    let args = TransferArg {
        from_subaccount: payout.from_subaccount,
        to: payout.recipient,
        amount: payout.transfer_amount.into(),
        fee: Some(payout.transfer_fee.into()),
        memo: Some(payout.transfer_memo.clone().into()),
        created_at_time: Some(payout.transfer_created_at_time_ns),
    };
    let result: Result<(Result<candid::Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;
    match result {
        Ok((reply @ Ok(_),)) | Ok((reply @ Err(TransferError::Duplicate { .. }),)) => {
            // This is an authenticated reply from the configured ledger to
            // the exact transfer dispatched above. ICRC-1 Ok/Duplicate is
            // authoritative payment evidence; requiring a second history API
            // here would strand withdrawals on otherwise compatible ledgers.
            // Cached/user-supplied candidates still use prove_and_finish.
            if mutate_state(|s| {
                finish_confirmed_callback(s, owner, ledger, &payout, &reply, ic_cdk::api::time())
            }) {
                Ok(())
            } else {
                Err(StabilityPoolError::SystemBusy)
            }
        }
        Ok((Err(error),))
            if matches!(
                &error,
                TransferError::BadFee { .. }
                    | TransferError::BadBurn { .. }
                    | TransferError::InsufficientFunds { .. }
                    | TransferError::CreatedInFuture { .. }
                    | TransferError::TooOld
            ) =>
        {
            if mutate_state(|s| s.reject_withdrawal(owner, ledger, &payout))? {
                Err(unresolved(format!(
                    "withdrawal rejected without effect: {error:?}"
                )))
            } else {
                mutate_state(|s| {
                    s.hold_withdrawal(
                        owner,
                        ledger,
                        &payout,
                        format!("later rejection after ambiguity: {error:?}"),
                        None,
                    )
                });
                Err(unresolved(
                    "withdrawal remains held after an earlier ambiguous attempt",
                ))
            }
        }
        Ok((Err(error),)) => {
            mutate_state(|s| {
                s.hold_withdrawal(
                    owner,
                    ledger,
                    &payout,
                    format!("unresolved ledger result: {error:?}"),
                    None,
                )
            });
            Err(unresolved(
                "withdrawal outcome unresolved; exact tuple and debit retained",
            ))
        }
        Err(error) => {
            mutate_state(|s| {
                s.hold_withdrawal(
                    owner,
                    ledger,
                    &payout,
                    format!("transfer reply unavailable: {error:?}"),
                    None,
                )
            });
            Err(unresolved(
                "withdrawal transfer reply unavailable; exact tuple and debit retained",
            ))
        }
    }
}

pub async fn reconcile(
    ledger: Principal,
    owner: Principal,
    block: u64,
) -> Result<u64, StabilityPoolError> {
    let caller = ic_cdk::caller();
    if caller != owner && !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    let _guard = Guard::reserve(owner, ledger)?;
    crate::ensure_pool_token_balance_mutation_allowed(&[ledger])?;
    let original = read_state(|s| s.pending_withdrawal(owner, ledger))
        .ok_or(StabilityPoolError::SystemBusy)?;
    if original.candidate_block_index.is_some_and(|v| v != block)
        || (original.candidate_block_index_raw.is_some()
            && original.candidate_block_index.is_none())
    {
        return Err(unresolved(
            "candidate conflicts with the ledger-reported withdrawal receipt",
        ));
    }
    let payout = mutate_state(|s| {
        let mut attempt = s.retry_withdrawal(owner, ledger, original.gross_amount)?;
        let row = s
            .pending_withdrawals
            .as_mut()
            .unwrap()
            .get_mut(&(owner, ledger))
            .unwrap();
        row.reconciliation_attempts = row.reconciliation_attempts.saturating_add(1);
        row.last_reconciliation_at_ns = Some(ic_cdk::api::time());
        attempt.reconciliation_attempts = row.reconciliation_attempts;
        attempt.last_reconciliation_at_ns = row.last_reconciliation_at_ns;
        Ok::<_, StabilityPoolError>(attempt)
    })?;
    let _balance_guard = crate::pool_guard::PoolBalanceAsyncGuard::new();
    prove_and_finish(owner, ledger, &payout, block).await?;
    Ok(payout.transfer_amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StablecoinConfig;
    fn setup() -> (StabilityPoolState, Principal, Principal) {
        let owner = Principal::self_authenticating([1]);
        let ledger = Principal::self_authenticating([2]);
        let mut state = StabilityPoolState::default();
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ledger,
            symbol: "icUSD".into(),
            decimals: 8,
            priority: 0,
            is_active: true,
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.add_deposit_at(owner, ledger, 1_000, 1);
        (state, owner, ledger)
    }
    #[test]
    fn withdrawal_ambiguous_retry_and_upgrade_keep_exact_tuple_and_single_debit() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert!(s.hold_withdrawal(owner, ledger, &p, "reply lost".into(), None));
        let bytes = candid::encode_one(&s).unwrap();
        let mut s: StabilityPoolState = candid::decode_one(&bytes).unwrap();
        s.reconcile_pending_withdrawals_after_upgrade();
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        assert_eq!(
            retry.transfer_created_at_time_ns,
            p.transfer_created_at_time_ns
        );
        assert_eq!(retry.transfer_memo, p.transfer_memo);
        assert_eq!(retry.transfer_fee, 10);
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert!(!s.reject_withdrawal(owner, ledger, &retry).unwrap());
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert!(s.finish_withdrawal(owner, ledger, &retry, 20));
        assert!(!s.finish_withdrawal(owner, ledger, &retry, 21));
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert_eq!(
            s.pool_events
                .as_ref()
                .unwrap()
                .iter()
                .filter(|e| matches!(e.event_type, PoolEventType::Withdraw { .. }))
                .count(),
            1
        );
    }
    #[test]
    fn withdrawal_no_effect_restores_once_and_preserves_later_credit() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 1_000, 10, 1_000, 5)
            .unwrap();
        assert!(s.deposits.contains_key(&owner));
        s.add_deposit_at(owner, ledger, 25, 6);
        assert!(s.reject_withdrawal(owner, ledger, &p).unwrap());
        assert!(!s.reject_withdrawal(owner, ledger, &p).unwrap());
        assert_eq!(s.total_stablecoin_balances[&ledger], 1_025);
        assert_eq!(s.deposits[&owner].stablecoin_balances[&ledger], 1_025);
    }
    #[test]
    fn withdrawal_rejects_changed_payload_and_interrupted_dispatch_never_restores() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        assert!(s.retry_withdrawal(owner, ledger, 601).is_err());
        s.reconcile_pending_withdrawals_after_upgrade();
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        assert!(retry.ambiguous_seen);
        assert!(!s.reject_withdrawal(owner, ledger, &retry).unwrap());
        assert!(!s.finish_withdrawal(owner, ledger, &p, 10));
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
    }
    #[test]
    fn withdrawal_shortfall_and_fee_checks_leave_books_unchanged() {
        let (mut s, owner, ledger) = setup();
        assert!(s
            .reserve_withdrawal(owner, ledger, 1_000, 10, 999, 5)
            .is_err());
        assert!(s
            .reserve_withdrawal(owner, ledger, 10, 10, 1_000, 5)
            .is_err());
        assert_eq!(s.total_stablecoin_balances[&ledger], 1_000);
        assert!(s.pending_withdrawal(owner, ledger).is_none());
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 11, 1_000, 5)
            .unwrap();
        assert_eq!(p.transfer_amount + p.transfer_fee, p.gross_amount);
        assert_eq!(p.transfer_amount, 589);
    }
    #[test]
    fn withdrawal_candidates_are_immutable_and_status_is_owner_scoped() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        assert!(s.hold_withdrawal(
            owner,
            ledger,
            &p,
            "verify receipt".into(),
            Some((Some(7), "7".into()))
        ));
        assert!(s
            .pending_withdrawals_for(&Principal::anonymous())
            .is_empty());
        assert_eq!(s.pending_withdrawals_for(&owner).len(), 1);
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        assert_eq!(retry.candidate_block_index, Some(7));
        assert!(!s.hold_withdrawal(
            owner,
            ledger,
            &retry,
            "conflict".into(),
            Some((Some(8), "8".into()))
        ));
        let held = s.pending_withdrawal(owner, ledger).unwrap();
        assert_eq!(held.candidate_block_index, Some(7));
        assert_eq!(held.candidate_block_index_raw.as_deref(), Some("7"));
        assert!(held.ambiguous_seen);
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
    }

    #[test]
    fn withdrawal_cross_asset_completion_preserves_metadata_for_other_rejection() {
        let (mut s, owner, ledger) = setup();
        let other = Principal::self_authenticating([3]);
        s.register_stablecoin(StablecoinConfig {
            ledger_id: other,
            symbol: "OTHER".into(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
        s.add_deposit_at(owner, other, 2_000, 1);
        let excluded = Principal::self_authenticating([4]);
        s.deposits
            .get_mut(&owner)
            .unwrap()
            .opted_out_collateral
            .insert(excluded);
        s.deposits
            .get_mut(&owner)
            .unwrap()
            .total_interest_earned_e8s = Some(77);
        let original = s.deposits[&owner].clone();
        let first = s
            .reserve_withdrawal(owner, ledger, 1_000, 10, 1_000, 5)
            .unwrap();
        let second = s
            .reserve_withdrawal(owner, other, 2_000, 10, 2_000, 6)
            .unwrap();
        assert!(s.deposits[&owner].is_empty());
        assert!(s.finish_withdrawal(owner, ledger, &first, 7));
        assert!(s.deposits.contains_key(&owner));
        assert!(s.reject_withdrawal(owner, other, &second).unwrap());
        assert_eq!(s.deposits[&owner].stablecoin_balances[&other], 2_000);
        assert_eq!(
            s.deposits[&owner].opted_out_collateral,
            original.opted_out_collateral
        );
        assert_eq!(s.deposits[&owner].total_interest_earned_e8s, Some(77));
        assert_eq!(s.total_stablecoin_balances[&ledger], 0);
        assert_eq!(s.total_stablecoin_balances[&other], 2_000);
    }

    #[test]
    fn withdrawal_confirmed_callback_completes_without_history_but_unknown_stays_held() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        let unknown = Err(TransferError::GenericError {
            error_code: 99u64.into(),
            message: "unknown".into(),
        });
        assert!(!finish_confirmed_callback(
            &mut s, owner, ledger, &p, &unknown, 6
        ));
        assert!(s.hold_withdrawal(owner, ledger, &p, "unknown reply".into(), None));
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        assert!(!finish_confirmed_callback(
            &mut s,
            owner,
            ledger,
            &retry,
            &Err(TransferError::TooOld),
            7
        ));
        assert!(!s.reject_withdrawal(owner, ledger, &retry).unwrap());
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert!(s.pending_withdrawal(owner, ledger).is_some());
        let paid = Ok(candid::Nat::from(123u64));
        assert!(!finish_confirmed_callback(
            &mut s, owner, ledger, &p, &paid, 8
        ));
        assert!(finish_confirmed_callback(
            &mut s, owner, ledger, &retry, &paid, 9
        ));
        assert!(!finish_confirmed_callback(
            &mut s, owner, ledger, &retry, &paid, 10
        ));
        assert!(s.pending_withdrawal(owner, ledger).is_none());
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
        assert_eq!(
            s.pool_events
                .as_ref()
                .unwrap()
                .iter()
                .filter(|e| matches!(e.event_type, PoolEventType::Withdraw { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn withdrawal_authenticated_duplicate_callback_completes_same_tuple_once() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        assert!(s.hold_withdrawal(owner, ledger, &p, "transfer reply lost".into(), None));
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        assert_eq!(
            retry.transfer_created_at_time_ns,
            p.transfer_created_at_time_ns
        );
        assert_eq!(retry.transfer_memo, p.transfer_memo);
        let duplicate = Err(TransferError::Duplicate {
            duplicate_of: candid::Nat::from(123u64),
        });
        assert!(finish_confirmed_callback(
            &mut s, owner, ledger, &retry, &duplicate, 9
        ));
        assert!(!finish_confirmed_callback(
            &mut s, owner, ledger, &retry, &duplicate, 10
        ));
        assert_eq!(s.total_stablecoin_balances[&ledger], 400);
    }

    #[test]
    fn withdrawal_failed_proof_does_not_pin_or_replace_a_candidate() {
        let (mut s, owner, ledger) = setup();
        let p = s
            .reserve_withdrawal(owner, ledger, 600, 10, 1_000, 5)
            .unwrap();
        // An unproved, caller-suggested block is deliberately not attached.
        assert!(s.hold_withdrawal(owner, ledger, &p, "wrong user candidate proof".into(), None));
        let held = s.pending_withdrawal(owner, ledger).unwrap();
        assert!(held.candidate_block_index.is_none());
        assert!(held.candidate_block_index_raw.is_none());
        let retry = s.retry_withdrawal(owner, ledger, 600).unwrap();
        // A later correct authenticated reply is still able to settle it.
        assert!(finish_confirmed_callback(
            &mut s,
            owner,
            ledger,
            &retry,
            &Ok(candid::Nat::from(77u64)),
            8
        ));

        let p = s
            .reserve_withdrawal(owner, ledger, 200, 10, 400, 9)
            .unwrap();
        assert!(s.hold_withdrawal(
            owner,
            ledger,
            &p,
            "bound historical candidate".into(),
            Some((Some(88), "88".into()))
        ));
        let retry = s.retry_withdrawal(owner, ledger, 200).unwrap();
        assert!(s.hold_withdrawal(owner, ledger, &retry, "proof unavailable".into(), None));
        let held = s.pending_withdrawal(owner, ledger).unwrap();
        assert_eq!(held.candidate_block_index, Some(88));
        assert_eq!(held.candidate_block_index_raw.as_deref(), Some("88"));
    }

    #[test]
    fn withdrawal_guard_serializes_owner_ledger_retries() {
        let owner = Principal::self_authenticating([7]);
        let ledger = Principal::self_authenticating([8]);
        let guard = Guard::reserve(owner, ledger).unwrap();
        assert!(Guard::reserve(owner, ledger).is_err());
        drop(guard);
        assert!(Guard::reserve(owner, ledger).is_ok());
    }
}
