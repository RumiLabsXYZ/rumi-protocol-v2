//! Borrow into XRPL with normal ICP debt accounting (Task 4).
//!
//! Ledger-proof-first, exactly as the legacy borrow: debt is recorded only after the
//! escrow mint is PROVEN, through the SAME `BorrowFromVault` event, so replay rebuilds
//! the debt and the legacy interest/liquidation machinery applies unchanged.
//!
//! Flow of `borrow_to_xrpl`:
//! 1. every await first (trust line for the recipient and the exact asset on the
//!    configured network; the ledger tip), under the per-vault operation lock;
//! 2. ONE synchronous block ([`commit_reservation`]): re-run the factored
//!    [`admit_borrow`] against CURRENT state (a price, policy, pause, cap or CR change
//!    since the pre-checks refuses), freeze gross/fee/net/recipient/network/nonce/
//!    policy version, record `ReservationBegun` (durable headroom reservation + vault
//!    exclusion) and persist the escrow mint job (nonce stored before any await);
//! 3. the step driver proves the mint, then [`advance_borrow`] commits the frozen
//!    terms exactly once (debt event, fee routing planned once, fee-remainder mint
//!    job persisted, reservation and exclusion released in one transition) and hands
//!    the funded claim to the issuer pipeline.
//!
//! A definite mint failure releases everything with no debt. An unknown mint keeps
//! the reservation and the exclusion until reconciled.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

use super::ledger;
use super::state::{Admission, XrplIcusdState};
use super::types::*;
use crate::chains::xrp::issued_amount::IssuedAsset;
use crate::chains::xrp::issued_rpc::fetch_trust_line;
use crate::chains::xrp::network::XrplNetwork;
use crate::numeric::ICUSD;
use crate::state::{mutate_state, read_state, State};
use crate::vault::{admit_borrow, BorrowQuote, BorrowReserve};
use crate::ProtocolError;

// ----------------------------------------------------------------------------------
// Error mapping and gates
// ----------------------------------------------------------------------------------

/// Map a legacy admission error onto the lane's closed error enum (no new variants).
pub fn map_protocol_error(e: ProtocolError) -> XrplIcusdError {
    match e {
        ProtocolError::CallerNotOwner => XrplIcusdError::Unauthorized,
        ProtocolError::AnonymousCallerNotAllowed => XrplIcusdError::AnonymousCaller,
        ProtocolError::AmountTooLow { minimum_amount } => XrplIcusdError::InvalidArgument(
            format!("amount is below the minimum of {minimum_amount} e8s"),
        ),
        ProtocolError::AlreadyProcessing => {
            XrplIcusdError::Unavailable("another operation by this caller is in progress".into())
        }
        ProtocolError::TemporarilyUnavailable(m) => XrplIcusdError::Unavailable(m),
        ProtocolError::GenericError(m) => XrplIcusdError::Policy(m),
        other => XrplIcusdError::Policy(format!("{other:?}")),
    }
}

/// The emergency freeze and ReadOnly mode refuse new borrows (the legacy endpoint
/// enforces both at its entry layer).
pub fn check_borrow_gates(state: &State) -> Result<(), XrplIcusdError> {
    if state.frozen {
        return Err(XrplIcusdError::Paused(
            "the protocol is frozen; borrowing is suspended".into(),
        ));
    }
    if state.mode == crate::state::Mode::ReadOnly {
        return Err(XrplIcusdError::Paused(
            "the protocol is in read-only mode; borrowing is suspended".into(),
        ));
    }
    Ok(())
}

/// Record a lane event in the log and apply it through the SAME pure function the
/// replay path uses.
pub fn record_lane_event(state: &mut State, ev: XrplIcusdEvent) {
    let ts = ev.timestamp();
    crate::storage::record_event_at(&crate::event::Event::XrplIcusd(ev.clone()), ts);
    super::state::apply_xrpl_icusd_event(state, &ev);
}

// ----------------------------------------------------------------------------------
// Pre-checks (synchronous, before any await)
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct NewBorrow {
    pub quote: BorrowQuote,
    pub config_version: u64,
    pub issuer: String,
    pub recipient: String,
    pub network: XrplNetwork,
}

#[derive(Clone, Debug)]
pub enum Precheck {
    /// The request nonce was already accepted: idempotent.
    Existing(OperationId),
    New(Box<NewBorrow>),
}

fn request_kind(arg: &BorrowToXrplArg) -> OperationKind {
    // Only the owner-authorized parameters take part in the idempotency comparison.
    OperationKind::Borrow {
        vault_id: arg.vault_id,
        collateral_type: Principal::anonymous(),
        gross_e8s: arg.gross_e8s,
        fee_e8s: 0,
        net_e8s: 0,
        recipient: arg.recipient.clone(),
        policy_version: 0,
        fee_split: None,
    }
}

/// Dedup, readiness, v1 restrictions and a full admission quote. No state changes
/// other than the vault's interest accrual that the legacy admission also performs.
pub fn precheck(
    caller: Principal,
    arg: &BorrowToXrplArg,
    nonce: &[u8; 32],
    now_ns: u64,
) -> Result<Precheck, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let kind = request_kind(arg);
    // An already-accepted request is idempotent, even while issuance is paused.
    if let Admission::Existing(id) =
        read_state(|s| s.xrpl_icusd.admit(&caller, nonce, arg.network, &kind))?
    {
        return Ok(Precheck::Existing(id));
    }
    let (issuer, config_version) = read_state(|s| {
        let st = &s.xrpl_icusd;
        st.ensure_open_for_issuance()?;
        if Some(arg.network) != st.config.network {
            return Err(XrplIcusdError::WrongNetwork(
                "request network differs from the configured network".into(),
            ));
        }
        check_borrow_gates(s)?;
        let issuer = st
            .config
            .issuer_address
            .clone()
            .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?;
        Ok((issuer, st.config.config_version))
    })?;
    super::issuance::validate_recipient(&arg.recipient, &issuer)?;
    // v1: the caller's own native-XRP vault only.
    read_state(|s| {
        let vault = s
            .vault_id_to_vaults
            .get(&arg.vault_id)
            .ok_or_else(|| XrplIcusdError::NotFound)?;
        if vault.owner != caller {
            return Err(XrplIcusdError::Unauthorized);
        }
        let native = s
            .get_collateral_config(&vault.collateral_type)
            .is_some_and(|c| c.is_native_xrp());
        if !native {
            return Err(XrplIcusdError::NotApplicable(
                "XRPL icUSD borrowing is limited to native-XRP vaults in this version".into(),
            ));
        }
        Ok(())
    })?;
    let (quote, _) = admit_borrow(
        caller,
        arg.vault_id,
        ICUSD::new(arg.gross_e8s),
        now_ns,
        BorrowReserve::CheckOnly,
    )
    .map_err(map_protocol_error)?;
    read_state(|s| s.xrpl_icusd.check_caps(quote.net_e8s))?;
    Ok(Precheck::New(Box::new(NewBorrow {
        quote,
        config_version,
        issuer,
        recipient: arg.recipient.clone(),
        network: arg.network,
    })))
}

// ----------------------------------------------------------------------------------
// The one synchronous commit
// ----------------------------------------------------------------------------------

/// Re-validate against CURRENT state and persist the durable reservation + the escrow
/// mint job. Everything awaited has completed: this function contains no await, so
/// the admission decision and the reservation are one atomic step. Returns
/// `(operation id, created)`.
#[allow(clippy::too_many_arguments)]
pub fn commit_reservation(
    caller: Principal,
    arg: &BorrowToXrplArg,
    nonce: &[u8; 32],
    planned: &NewBorrow,
    ledger_tip: u64,
    backend: Principal,
    now_ns: u64,
) -> Result<(OperationId, bool), XrplIcusdError> {
    let kind = request_kind(arg);
    // A concurrent call with the same nonce may have won while we awaited.
    match read_state(|s| s.xrpl_icusd.admit(&caller, nonce, arg.network, &kind))? {
        Admission::Existing(id) => return Ok((id, false)),
        Admission::New => {}
    }
    read_state(|s| {
        let st = &s.xrpl_icusd;
        st.ensure_open_for_issuance()?;
        if Some(arg.network) != st.config.network {
            return Err(XrplIcusdError::WrongNetwork(
                "request network differs from the configured network".into(),
            ));
        }
        if st.config.config_version != planned.config_version {
            return Err(XrplIcusdError::Policy(
                "the issuer policy, pauses or limits changed since this request was checked; \
                 nothing was minted, retry"
                    .into(),
            ));
        }
        check_borrow_gates(s)?;
        if st.vault_exclusions.contains(&arg.vault_id) {
            return Err(XrplIcusdError::Unavailable(format!(
                "vault #{} already has an unresolved XRPL icUSD operation",
                arg.vault_id
            )));
        }
        // Stricter than the legacy borrow: ANY persisted SP absorb reservation (even
        // an expired one that the post-burn submit may still honor) blocks a new
        // XRPL reservation on the vault.
        if s.sp_xrp_absorb_preflights.contains_key(&arg.vault_id) {
            return Err(XrplIcusdError::Unavailable(format!(
                "vault #{} has a pending stability-pool absorb reservation",
                arg.vault_id
            )));
        }
        Ok(())
    })?;
    // Re-run the full admission with current inputs; any drift in the frozen terms
    // (price, fee curve, CR, caps, mode, status) refuses BEFORE any mint exists.
    let (fresh, _) = admit_borrow(
        caller,
        arg.vault_id,
        ICUSD::new(arg.gross_e8s),
        now_ns,
        BorrowReserve::CheckOnly,
    )
    .map_err(map_protocol_error)?;
    if fresh != planned.quote {
        return Err(XrplIcusdError::Policy(
            "price, fee, limits or collateral status changed since this request was checked; \
             nothing was minted, retry"
                .into(),
        ));
    }
    mutate_state(|s| {
        s.xrpl_icusd.check_caps(fresh.net_e8s)?;
        let id = s.xrpl_icusd.allocate_operation_id();
        record_lane_event(
            s,
            XrplIcusdEvent::ReservationBegun {
                op_id: id,
                owner: caller,
                request_nonce: nonce.to_vec(),
                network: arg.network,
                vault_id: arg.vault_id,
                collateral_type: fresh.collateral_type,
                gross_e8s: fresh.gross_e8s,
                fee_e8s: fresh.fee_e8s,
                net_e8s: fresh.net_e8s,
                recipient: arg.recipient.clone(),
                policy_version: planned.config_version,
                timestamp: now_ns,
            },
        );
        let op_nonce = s.next_op_nonce_at(now_ns);
        s.xrpl_icusd.add_job(
            id,
            ledger::mint_job_spec(backend, fresh.net_e8s, ledger_tip),
            op_nonce,
            now_ns,
        )?;
        let op = s.xrpl_icusd.op_mut(id)?;
        op.phase = Phase::EscrowMintPending;
        Ok((id, true))
    })
}

// ----------------------------------------------------------------------------------
// Proven mint -> debt (once) / definite failure -> release
// ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorrowAdvance {
    /// Nothing to do yet (mint unproven or unknown).
    Waiting,
    /// The frozen terms were committed: debt recorded, fee routed, claim funded.
    Committed,
    /// The mint definitively failed: reservation and exclusion released, no debt.
    Cancelled,
    /// The mint is proven but debt cannot be applied safely: everything is retained.
    Held,
    /// A replayed `DebtCommitted` operation was handed to the delivery pipeline.
    Promoted,
}

/// Advance one Borrow operation. Pure with respect to time and the IC (the caller
/// passes `now_ns`), so it is unit-testable on the host. The commit happens only in
/// the reservation phases, so it can run at most once per operation.
pub fn advance_borrow(
    state: &mut State,
    op_id: OperationId,
    now_ns: u64,
) -> Result<BorrowAdvance, XrplIcusdError> {
    let (vault_id, owner, collateral_type, gross, fee, phase, mint) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Borrow {
            vault_id,
            collateral_type,
            gross_e8s,
            fee_e8s,
            ..
        } = &op.kind
        else {
            return Ok(BorrowAdvance::Waiting);
        };
        (
            *vault_id,
            op.owner,
            *collateral_type,
            *gross_e8s,
            *fee_e8s,
            op.phase,
            op.ic_jobs
                .iter()
                .find(|j| j.purpose == IcJobPurpose::EscrowMint)
                .cloned(),
        )
    };
    match phase {
        Phase::Prepared | Phase::EscrowMintPending => {
            let Some(mint) = mint else {
                return Ok(BorrowAdvance::Waiting);
            };
            if let IcJobStatus::Failed { reason } = &mint.status {
                let reason = reason.clone();
                record_lane_event(
                    state,
                    XrplIcusdEvent::ReservationReleased {
                        op_id,
                        vault_id,
                        reason: ReleaseReason::MintFailed,
                        timestamp: now_ns,
                    },
                );
                let op = state.xrpl_icusd.op_mut(op_id)?;
                op.push_note(format!("escrow mint failed: {reason}; no debt recorded"));
                return Ok(BorrowAdvance::Cancelled);
            }
            if !mint.is_proven_done() {
                return Ok(BorrowAdvance::Waiting);
            }
            let block = mint.done_block().unwrap_or_default();
            // Debt must be applicable; the exclusion keeps the vault unchanged, so
            // this holds in practice. If it does not, retain everything (no debt, no
            // release) rather than guess.
            let applicable = state.vault_id_to_vaults.get(&vault_id).is_some_and(|v| {
                v.owner == owner
                    && v.collateral_type == collateral_type
                    && crate::numeric::checked_icusd_add(v.borrowed_icusd_amount, ICUSD::new(gross))
                        .is_some()
            });
            if !applicable {
                let op = state.xrpl_icusd.op_mut(op_id)?;
                op.push_note(
                    "escrow mint proven but debt cannot be applied safely; held for reconciliation",
                );
                return Ok(BorrowAdvance::Held);
            }
            // ---- the commit: frozen terms, once, in this synchronous block ----
            // Bring the vault's interest up to date BEFORE the new principal lands,
            // exactly as the legacy borrow does, so the new debt never inherits (or
            // skips) accrual for the time the mint was pending.
            state.accrue_single_vault(vault_id, now_ns);
            // Owner attribution is the SAVED owner, never a timer/resume caller.
            crate::event::record_borrow_from_vault_at(
                state,
                vault_id,
                ICUSD::new(gross),
                ICUSD::new(fee),
                block,
                owner,
                now_ns,
            );
            let routing = crate::treasury::plan_fee_routing_at(
                state,
                ICUSD::new(fee),
                crate::event::FeeSource::BorrowingFee,
                now_ns,
            );
            let treasury = state.treasury_principal;
            let remainder = routing.to_remainder.to_u64();
            let split = FeeSplit {
                to_deficit_e8s: routing.to_repay.to_u64(),
                to_treasury_remainder_e8s: remainder,
                treasury,
            };
            let tip = mint.ledger_tip_before.unwrap_or(0);
            let fee_nonce = (remainder > 0 && treasury.is_some()).then(|| state.next_op_nonce_at(now_ns));
            {
                let st = &mut state.xrpl_icusd;
                if let (Some(t), Some(n)) = (treasury, fee_nonce) {
                    // The fee-remainder mint is a persisted job: a retry re-sends the
                    // identical tuple and never re-runs the deficit allocation.
                    st.add_job(
                        op_id,
                        ledger::fee_mint_job_spec(
                            Account {
                                owner: t,
                                subaccount: None,
                            },
                            remainder,
                            tip,
                        ),
                        n,
                        now_ns,
                    )?;
                }
                let op = st.op_mut(op_id)?;
                if let OperationKind::Borrow { fee_split, .. } = &mut op.kind {
                    *fee_split = Some(split);
                }
                if remainder > 0 && treasury.is_none() {
                    op.push_note("treasury missing at debt commit; fee remainder held (no mint)");
                }
            }
            record_lane_event(
                state,
                XrplIcusdEvent::ReservationReleased {
                    op_id,
                    vault_id,
                    reason: ReleaseReason::DebtCommitted,
                    timestamp: now_ns,
                },
            );
            let op = state.xrpl_icusd.op_mut(op_id)?;
            op.phase = Phase::IssuePrepared;
            op.updated_at_ns = now_ns;
            Ok(BorrowAdvance::Committed)
        }
        Phase::DebtCommitted => {
            // Replay (or any path that left the claim at DebtCommitted) reaches the
            // delivery pipeline here; the debt was already recorded.
            let op = state.xrpl_icusd.op_mut(op_id)?;
            op.phase = Phase::IssuePrepared;
            op.updated_at_ns = now_ns;
            Ok(BorrowAdvance::Promoted)
        }
        _ => Ok(BorrowAdvance::Waiting),
    }
}

/// Convenience for the step driver (full-state synchronous block).
pub fn advance_borrow_now(op_id: OperationId, now_ns: u64) -> Result<BorrowAdvance, XrplIcusdError> {
    mutate_state(|s| advance_borrow(s, op_id, now_ns))
}

// ----------------------------------------------------------------------------------
// Public async entry point
// ----------------------------------------------------------------------------------

pub async fn borrow_to_xrpl(
    caller: Principal,
    arg: BorrowToXrplArg,
) -> Result<OperationView, XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let nonce = super::issuance::parse_request_nonce(&arg.request_nonce)?;
    let now = super::now();
    let planned = match precheck(caller, &arg, &nonce, now)? {
        Precheck::Existing(id) => {
            let _ = super::run_op_step(id).await;
            super::ensure_xrpl_tick_scheduled();
            return super::view_for(&caller, id);
        }
        Precheck::New(p) => *p,
    };
    if !super::throttle_ok(
        super::THROTTLE_KIND_NEW,
        caller.as_slice(),
        super::now(),
        super::NEW_REQUEST_GAP_NS,
    ) {
        return Err(XrplIcusdError::LimitExceeded(
            "too many requests; retry in a few seconds".into(),
        ));
    }
    // Serialize against every other operation by this caller and on this vault for
    // the awaits and the commit; the durable exclusion takes over afterwards.
    let created_id = {
        let _caller_guard = crate::guard::GuardPrincipal::new(
            caller,
            &format!("borrow_xrpl_vault_{}", arg.vault_id),
        )
        .map_err(|e| map_protocol_error(e.into()))?;
        let _vault_guard = crate::guard::VaultLiquidationGuard::new(arg.vault_id)
            .map_err(map_protocol_error)?;
        // ---- awaits: nothing is persisted yet ----
        let asset = IssuedAsset::icusd(&planned.issuer)
            .map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
        let proof = fetch_trust_line(planned.network, &arg.recipient, &asset)
            .await
            .map_err(|e| XrplIcusdError::Unavailable(e.to_string()))?;
        super::settlement::check_trust_line(proof.line.as_ref(), planned.quote.net_e8s)?;
        let tip = ledger::ledger_tip()
            .await
            .map_err(|e| XrplIcusdError::Unavailable(format!("ledger tip: {e}")))?;
        // ---- one synchronous commit ----
        let backend = ic_cdk::id();
        commit_reservation(caller, &arg, &nonce, &planned, tip, backend, super::now())?
    };
    super::ensure_xrpl_tick_scheduled();
    let (id, created) = created_id;
    if created {
        let _ = super::run_op_step(id).await;
    }
    super::view_for(&caller, id)
}

impl XrplIcusdState {
    /// Borrow operations whose escrow mint is unresolved (diagnostics/tests).
    pub fn unresolved_borrow_ids(&self) -> Vec<OperationId> {
        self.operations
            .values()
            .filter(|o| matches!(o.kind, OperationKind::Borrow { .. }) && o.phase.is_reservation())
            .map(|o| o.id)
            .collect()
    }
}

// ----------------------------------------------------------------------------------
// Fee-remainder mint: retry and hold (never silently dropped)
// ----------------------------------------------------------------------------------

/// What state a Borrow's fee-remainder mint is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeMintState {
    /// No remainder to mint (or not a committed Borrow).
    NotApplicable,
    /// A job is queued, in flight, unknown or done: the normal driver handles it.
    InProgress,
    /// The last job was PROVEN never executed: a fresh-nonce successor is due.
    RetryDue,
    /// The remainder is owed but no treasury was configured at the commit: held.
    HeldNoTreasury,
    /// Proven-failed attempts exhausted the automatic budget: held for the operator.
    HeldExhausted,
}

pub fn fee_mint_state(op: &Operation) -> FeeMintState {
    let OperationKind::Borrow {
        fee_split: Some(split),
        ..
    } = &op.kind
    else {
        return FeeMintState::NotApplicable;
    };
    if split.to_treasury_remainder_e8s == 0 {
        return FeeMintState::NotApplicable;
    }
    let jobs: Vec<&IcLedgerJob> = op
        .ic_jobs
        .iter()
        .filter(|j| j.purpose == IcJobPurpose::FeeMint)
        .collect();
    let Some(last) = jobs.last() else {
        return if split.treasury.is_none() {
            FeeMintState::HeldNoTreasury
        } else {
            FeeMintState::InProgress
        };
    };
    match last.status {
        IcJobStatus::Failed { .. } => {
            if jobs.len() > MAX_FEE_MINT_AUTO_RETRIES {
                FeeMintState::HeldExhausted
            } else {
                FeeMintState::RetryDue
            }
        }
        _ => FeeMintState::InProgress,
    }
}

fn add_fee_mint_successor(
    state: &mut State,
    op_id: OperationId,
    treasury: Principal,
    now_ns: u64,
) -> Result<(), XrplIcusdError> {
    let (remainder, tip) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Borrow {
            fee_split: Some(split),
            ..
        } = &op.kind
        else {
            return Err(XrplIcusdError::NotApplicable("not a committed borrow".into()));
        };
        let tip = op
            .ic_jobs
            .iter()
            .rfind(|j| j.purpose == IcJobPurpose::FeeMint)
            .or_else(|| op.ic_jobs.iter().find(|j| j.purpose == IcJobPurpose::EscrowMint))
            .and_then(|j| j.ledger_tip_before)
            .unwrap_or(0);
        (split.to_treasury_remainder_e8s, tip)
    };
    // A NEW nonce (hence a new created_at_time and memo): the previous attempt is
    // proven never executed, so a fresh one cannot double-mint.
    let nonce = state.next_op_nonce_at(now_ns);
    state.xrpl_icusd.add_job(
        op_id,
        ledger::fee_mint_job_spec(
            Account {
                owner: treasury,
                subaccount: None,
            },
            remainder,
            tip,
        ),
        nonce,
        now_ns,
    )?;
    let op = state.xrpl_icusd.op_mut(op_id)?;
    if let OperationKind::Borrow {
        fee_split: Some(split),
        ..
    } = &mut op.kind
    {
        split.treasury = Some(treasury);
    }
    Ok(())
}

/// Automatic successor of a PROVEN-failed fee-remainder mint (fresh nonce), within the
/// retry budget. Returns whether a job was added. Past the budget (or without a
/// treasury) the remainder stays held and visible (`NextAction::ContactOperator`).
pub fn retry_failed_fee_mint(
    state: &mut State,
    op_id: OperationId,
    now_ns: u64,
) -> Result<bool, XrplIcusdError> {
    let (fm, treasury) = {
        let op = state.xrpl_icusd.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let t = match &op.kind {
            OperationKind::Borrow {
                fee_split: Some(split),
                ..
            } => split.treasury,
            _ => None,
        };
        (fee_mint_state(op), t)
    };
    match (fm, treasury) {
        (FeeMintState::RetryDue, Some(t)) => {
            add_fee_mint_successor(state, op_id, t, now_ns)?;
            let op = state.xrpl_icusd.op_mut(op_id)?;
            op.push_note("fee-remainder mint proven unexecuted; retrying with a fresh nonce");
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Controller-triggered retry of a HELD fee remainder (no treasury at the commit, or
/// the automatic budget exhausted): uses the CURRENT treasury principal. Refuses while
/// any earlier job is not proven unexecuted.
pub fn operator_retry_fee_mint(
    state: &mut State,
    op_id: OperationId,
    now_ns: u64,
) -> Result<(), XrplIcusdError> {
    let fm = state
        .xrpl_icusd
        .op(op_id)
        .map(fee_mint_state)
        .ok_or(XrplIcusdError::NotFound)?;
    match fm {
        FeeMintState::HeldNoTreasury | FeeMintState::HeldExhausted | FeeMintState::RetryDue => {}
        FeeMintState::InProgress => {
            return Err(XrplIcusdError::Ambiguous(
                "a fee-remainder mint is queued or unresolved; it cannot be retried until it is \
                 proven unexecuted"
                    .into(),
            ))
        }
        FeeMintState::NotApplicable => {
            return Err(XrplIcusdError::NotApplicable(
                "this operation has no fee remainder to mint".into(),
            ))
        }
    }
    let treasury = state.treasury_principal.ok_or_else(|| {
        XrplIcusdError::Disabled("no treasury principal is configured to receive the fee".into())
    })?;
    add_fee_mint_successor(state, op_id, treasury, now_ns)?;
    let op = state.xrpl_icusd.op_mut(op_id)?;
    op.push_note("operator retried the held fee-remainder mint");
    Ok(())
}

// ----------------------------------------------------------------------------------
// Replay-rebuilt reservations: ledger-proof reconciliation
// ----------------------------------------------------------------------------------

/// Pure check + booking of a controller-supplied escrow-mint block for a
/// replay-rebuilt Borrow (`Prepared`, no jobs). Replay rebuilds the reservation and
/// the vault exclusion but cannot know whether the mint executed. The block must be
/// an escrow MINT of exactly `net_e8s` and must not already be the proven mint of
/// another operation. On success the operation carries a synthetic proven mint job
/// and moves to `EscrowMintPending`, so the normal `advance_borrow` commits the debt
/// once. No block, no resolution: absence cannot be shown from here.
pub fn apply_rebuilt_mint_proof(
    st: &mut XrplIcusdState,
    op_id: OperationId,
    block: u64,
    decoded: &crate::icrc3_proof::DecodedBlock,
    backend: Principal,
    now_ns: u64,
) -> Result<(), XrplIcusdError> {
    let net = {
        let op = st.op(op_id).ok_or(XrplIcusdError::NotFound)?;
        let OperationKind::Borrow { net_e8s, .. } = &op.kind else {
            return Err(XrplIcusdError::NotApplicable("not a borrow".into()));
        };
        if op.phase != Phase::Prepared || !op.ic_jobs.is_empty() {
            return Err(XrplIcusdError::WrongPhase(
                "only a replay-rebuilt borrow with no mint job needs mint reconciliation".into(),
            ));
        }
        *net_e8s
    };
    let escrow = escrow_account(backend);
    if decoded.op != "mint"
        || decoded.from.is_some()
        || decoded.to != Some(escrow)
        || decoded.amount != u128::from(net)
    {
        return Err(XrplIcusdError::InvalidArgument(
            "the block is not a mint of exactly the reserved net amount into the escrow account"
                .into(),
        ));
    }
    let claimed = st.operations.values().any(|o| {
        o.id != op_id
            && o.ic_jobs
                .iter()
                .any(|j| j.purpose == IcJobPurpose::EscrowMint && j.done_block() == Some(block))
    });
    if claimed {
        return Err(XrplIcusdError::InvalidArgument(
            "that mint block already belongs to another operation".into(),
        ));
    }
    let op = st.op_mut(op_id)?;
    op.ic_jobs.push(IcLedgerJob {
        purpose: IcJobPurpose::EscrowMint,
        op_nonce: 0,
        created_at_time: 0,
        from_subaccount: None,
        from_owner: None,
        to: escrow,
        amount_e8s: net,
        memo: vec![],
        reserved_fee_e8s: 0,
        ledger_tip_before: Some(block),
        status: IcJobStatus::Done { block },
        proof_verified: true,
        scan_required: false,
        scan: None,
        sends: 0,
        in_flight_since_ns: None,
    });
    op.phase = Phase::EscrowMintPending;
    op.updated_at_ns = now_ns;
    op.push_note(format!(
        "escrow mint proven by controller-supplied ledger block {block} (replay-rebuilt reservation)"
    ));
    st.live_ops.insert(op_id);
    Ok(())
}

/// Controller-only: fetch the ledger block and book it (see
/// [`apply_rebuilt_mint_proof`]), then drive the operation.
pub async fn reconcile_borrow_mint(op_id: OperationId, block: u64) -> Result<(), XrplIcusdError> {
    let ledger_id = read_state(|s| s.icusd_ledger_principal);
    let decoded = crate::icrc3_proof::fetch_icrc3_block(ledger_id, block)
        .await
        .map_err(XrplIcusdError::Unavailable)?;
    let backend = ic_cdk::id();
    mutate_state(|s| apply_rebuilt_mint_proof(&mut s.xrpl_icusd, op_id, block, &decoded, backend, super::now()))?;
    super::ensure_xrpl_tick_scheduled();
    let _ = super::run_op_step(op_id).await;
    Ok(())
}
