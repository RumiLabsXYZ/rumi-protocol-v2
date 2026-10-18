//! `XrplIcusdState` and its PURE transitions: no awaits, no `ic_cdk` calls (time and
//! nonces are parameters), so every rule here is unit-testable on the host.
//!
//! Operations and ledger/issuer jobs are snapshot state (like XRP claims). The event
//! log only carries what the fallback replay path needs to rebuild durable headroom
//! reservations, vault exclusions and interest-accrual freezes (see
//! [`apply_xrpl_icusd_event`]).

use std::collections::{BTreeMap, BTreeSet};

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};

use super::types::*;
use crate::chains::xrp::network::XrplNetwork;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct XrplIcusdState {
    pub config: XrplIcusdConfig,
    pub operations: BTreeMap<OperationId, Operation>,
    pub next_operation_id: OperationId,
    /// `(owner, request nonce) -> operation`: the same nonce never creates a second
    /// operation.
    pub request_nonce_index: BTreeMap<(Principal, [u8; 32]), OperationId>,
    /// Vaults under a durable operation exclusion (borrow mint unresolved, or an
    /// admitted repayment burn unresolved). Consulted via [`xrpl_vault_excluded`].
    pub vault_exclusions: BTreeSet<u64>,
    /// Vaults whose interest accrual/harvest is deferred while a repayment burn is
    /// unresolved (a subset of `vault_exclusions`).
    pub accrual_frozen_vaults: BTreeSet<u64>,
    /// `O`: delivered minus retired. Updated only at validated issuance (+) and
    /// verified return (-); cross-checked against the XRPL obligation snapshot.
    pub issued_outstanding: WideAmount,
    /// Single-flight issuer sequencer: the holder of the one unresolved transaction.
    pub issuer_lease: Option<LeaseHolder>,
    pub setup_attempt: Option<IssuerTxAttempt>,
    pub last_coverage: Option<CoverageRecord>,
}

/// True when a durable operation excludes `vault_id` from conflicting legacy
/// mutations. Task 4/5 wire this into the guards and redemption/SP paths.
pub fn xrpl_vault_excluded(state: &crate::state::State, vault_id: u64) -> bool {
    state.xrpl_icusd.vault_exclusions.contains(&vault_id)
}

/// True when interest accrual/harvest for the vault is deferred.
pub fn xrpl_accrual_frozen(state: &crate::state::State, vault_id: u64) -> bool {
    state.xrpl_icusd.accrual_frozen_vaults.contains(&vault_id)
}

// ----------------------------------------------------------------------------------
// Liability buckets and coverage
// ----------------------------------------------------------------------------------

/// Disjoint liability buckets, DERIVED from operations (never stored as counters,
/// except `O`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buckets {
    /// Backed delivery claims not yet issued.
    pub d: WideAmount,
    /// Outstanding issued tokens (stored counter).
    pub o: WideAmount,
    /// Verified returns awaiting an escrow burn or canonical return.
    pub r: WideAmount,
    /// Held / refund / quarantine amounts.
    pub h: WideAmount,
    /// Part of `d` whose signed issuance may already be on the XRPL.
    pub in_flight: WideAmount,
    /// IC ledger fees pending canonical returns charge on top of holder backing.
    pub ic_fee_reserve_e8s: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub ledger_index: u32,
    pub total: WideAmount,
}

#[derive(Clone, Debug)]
pub struct CoverageInput {
    pub escrow_balance_e8s: u128,
    pub snapshot: Option<SnapshotSummary>,
    /// Returns validated on the XRPL but not yet ingested here may legitimately make
    /// the snapshot LOWER than `O` by up to this amount.
    pub unprocessed_return_allowance: WideAmount,
}

fn internal(e: WideError) -> XrplIcusdError {
    XrplIcusdError::Internal(format!("liability arithmetic: {e:?}"))
}

fn wide(e8s: u64) -> WideAmount {
    WideAmount::from_e8s(e8s)
}

fn add(acc: &mut WideAmount, v: &WideAmount) -> Result<(), XrplIcusdError> {
    *acc = acc.checked_add(v).map_err(internal)?;
    Ok(())
}

/// Phases in which a Borrow's escrow funding is proven and delivery is outstanding.
fn borrow_claim_phase(p: Phase) -> bool {
    matches!(
        p,
        Phase::DebtCommitted | Phase::IssuePrepared | Phase::Signed | Phase::Submitted
    )
}

/// Phases in which an Import's escrow funding is proven and delivery is outstanding.
fn import_claim_phase(p: Phase) -> bool {
    matches!(p, Phase::IssuePrepared | Phase::Signed | Phase::Submitted)
}

impl XrplIcusdState {
    // ---- readiness / admission ----------------------------------------------------

    /// The lane answers user methods only when fully configured.
    pub fn ensure_ready(&self) -> Result<(), XrplIcusdError> {
        let c = &self.config;
        if !c.enabled {
            return Err(XrplIcusdError::Disabled("XRPL icUSD is not enabled".into()));
        }
        if c.network.is_none() {
            return Err(XrplIcusdError::Disabled("network not configured".into()));
        }
        if c.policy.is_none() {
            return Err(XrplIcusdError::Disabled("issuer policy not chosen".into()));
        }
        if !c.issuer_ready() {
            return Err(XrplIcusdError::Disabled("issuer setup not complete".into()));
        }
        if c.verified_minting_account.is_none() {
            return Err(XrplIcusdError::Disabled(
                "ledger minting account not verified".into(),
            ));
        }
        Ok(())
    }

    /// Ready AND accepting new operations.
    pub fn ensure_open_for_issuance(&self) -> Result<(), XrplIcusdError> {
        self.ensure_ready()?;
        if self.config.issuance_paused {
            return Err(XrplIcusdError::Paused("new issuance is paused".into()));
        }
        Ok(())
    }

    pub fn active_ops_for(&self, owner: &Principal) -> usize {
        self.operations
            .values()
            .filter(|o| o.owner == *owner && !o.phase.is_terminal())
            .count()
    }

    /// Dedup + limits. `Existing` means the request was already accepted (idempotent
    /// replay of the same request); a different body under the same nonce conflicts.
    pub fn admit(
        &self,
        owner: &Principal,
        request_nonce: &[u8; 32],
        network: XrplNetwork,
        kind: &OperationKind,
    ) -> Result<Admission, XrplIcusdError> {
        if let Some(id) = self.request_nonce_index.get(&(*owner, *request_nonce)) {
            let existing = self.operations.get(id).ok_or_else(|| {
                XrplIcusdError::Internal("request nonce index points at a missing operation".into())
            })?;
            if existing.network != network || !same_request(&existing.kind, kind) {
                return Err(XrplIcusdError::NonceConflict);
            }
            return Ok(Admission::Existing(*id));
        }
        if self.operations.len() >= MAX_OPERATIONS_GLOBAL {
            return Err(XrplIcusdError::LimitExceeded(
                "global operation limit reached".into(),
            ));
        }
        if self.active_ops_for(owner) >= MAX_ACTIVE_OPS_PER_OWNER {
            return Err(XrplIcusdError::LimitExceeded(format!(
                "at most {MAX_ACTIVE_OPS_PER_OWNER} active operations per owner"
            )));
        }
        Ok(Admission::New)
    }

    /// Per-operation and outstanding caps against `O + D + R + H + add`.
    pub fn check_caps(&self, add_e8s: u64) -> Result<(), XrplIcusdError> {
        let c = &self.config;
        if add_e8s == 0 {
            return Err(XrplIcusdError::InvalidArgument("amount must be > 0".into()));
        }
        if add_e8s > c.per_operation_cap_e8s {
            return Err(XrplIcusdError::LimitExceeded(format!(
                "amount exceeds the per-operation cap of {} e8s",
                c.per_operation_cap_e8s
            )));
        }
        let b = self.buckets()?;
        let mut total = b.o;
        add(&mut total, &b.d)?;
        add(&mut total, &b.r)?;
        add(&mut total, &b.h)?;
        // Borrows still awaiting their escrow mint are not in `D` yet, but their net
        // payout WILL be: count it so two concurrent borrows cannot both pass.
        add(&mut total, &wide(self.reserved_borrow_net_e8s()))?;
        add(&mut total, &wide(add_e8s))?;
        if total.ceil_e8s() > u128::from(c.outstanding_cap_e8s) {
            return Err(XrplIcusdError::LimitExceeded(format!(
                "outstanding cap of {} e8s would be exceeded",
                c.outstanding_cap_e8s
            )));
        }
        Ok(())
    }

    pub fn allocate_operation_id(&mut self) -> OperationId {
        let id = self.next_operation_id;
        self.next_operation_id += 1;
        id
    }

    /// Insert a brand-new operation under `id`. The nonce index entry and the id
    /// counter are updated together so a nonce can never map to two operations.
    pub fn insert_operation(&mut self, op: Operation) -> Result<(), XrplIcusdError> {
        if self.operations.contains_key(&op.id) {
            return Err(XrplIcusdError::Internal("operation id already used".into()));
        }
        let key = (op.owner, op.request_nonce);
        if self.request_nonce_index.contains_key(&key) {
            return Err(XrplIcusdError::NonceConflict);
        }
        self.request_nonce_index.insert(key, op.id);
        self.next_operation_id = self.next_operation_id.max(op.id + 1);
        self.operations.insert(op.id, op);
        Ok(())
    }

    pub fn op(&self, id: OperationId) -> Option<&Operation> {
        self.operations.get(&id)
    }

    /// Caller-scoped lookup: another principal's operation is `NotFound`.
    pub fn op_for_owner(
        &self,
        owner: &Principal,
        id: OperationId,
    ) -> Result<&Operation, XrplIcusdError> {
        match self.operations.get(&id) {
            Some(op) if op.owner == *owner => Ok(op),
            _ => Err(XrplIcusdError::NotFound),
        }
    }

    pub fn op_mut(&mut self, id: OperationId) -> Result<&mut Operation, XrplIcusdError> {
        self.operations
            .get_mut(&id)
            .ok_or_else(|| XrplIcusdError::Internal(format!("operation {id} missing")))
    }

    // ---- durable reservations -------------------------------------------------------

    /// Gross e8s of uncommitted XRPL borrows (phase `Prepared`/`EscrowMintPending`)
    /// against `collateral`. Legacy and XRPL admissions must both count this.
    pub fn durable_reserved_for(&self, collateral: Principal) -> u64 {
        self.operations
            .values()
            .filter(|o| o.phase.is_reservation())
            .filter_map(|o| match &o.kind {
                OperationKind::Borrow {
                    collateral_type,
                    gross_e8s,
                    ..
                } if *collateral_type == collateral => Some(*gross_e8s),
                _ => None,
            })
            .fold(0u64, |a, g| a.saturating_add(g))
    }

    /// Net payout of Borrow operations that are still durable reservations (their
    /// escrow mint is not yet proven, so they are not in bucket `D`).
    pub fn reserved_borrow_net_e8s(&self) -> u64 {
        self.operations
            .values()
            .filter(|o| o.phase.is_reservation())
            .filter_map(|o| match &o.kind {
                OperationKind::Borrow { net_e8s, .. } => Some(*net_e8s),
                _ => None,
            })
            .fold(0u64, |a, g| a.saturating_add(g))
    }

    pub fn durable_reserved_total(&self) -> u64 {
        self.operations
            .values()
            .filter(|o| o.phase.is_reservation())
            .filter_map(|o| match &o.kind {
                OperationKind::Borrow { gross_e8s, .. } => Some(*gross_e8s),
                _ => None,
            })
            .fold(0u64, |a, g| a.saturating_add(g))
    }

    // ---- liabilities -----------------------------------------------------------------

    pub fn buckets(&self) -> Result<Buckets, XrplIcusdError> {
        let mut b = Buckets {
            o: self.issued_outstanding,
            ..Default::default()
        };
        for op in self.operations.values() {
            let signed_ambiguous = op.has_signed_ambiguity();
            match &op.kind {
                OperationKind::Borrow { net_e8s, .. } if borrow_claim_phase(op.phase) => {
                    add(&mut b.d, &wide(*net_e8s))?;
                    if signed_ambiguous {
                        add(&mut b.in_flight, &wide(*net_e8s))?;
                    }
                }
                OperationKind::Import { amount_e8s, .. } if import_claim_phase(op.phase) => {
                    add(&mut b.d, &wide(*amount_e8s))?;
                    if signed_ambiguous {
                        add(&mut b.in_flight, &wide(*amount_e8s))?;
                    }
                }
                OperationKind::Repayment {
                    applied_e8s, held, ..
                } => {
                    if matches!(op.phase, Phase::ReturnVerified | Phase::EscrowBurnPending) {
                        add(&mut b.r, &wide(*applied_e8s))?;
                    }
                    if !matches!(op.phase, Phase::Cancelled | Phase::ClaimResolved) {
                        add(&mut b.h, &WideAmount::from_exact(held).map_err(internal)?)?;
                    }
                }
                OperationKind::Refund { amount, .. }
                    if matches!(op.phase, Phase::RefundPending | Phase::Held) =>
                {
                    add(&mut b.h, &WideAmount::from_exact(amount).map_err(internal)?)?;
                }
                _ => {}
            }
            for j in &op.ic_jobs {
                if j.purpose == IcJobPurpose::CanonicalReturn && !j.is_resolved() {
                    b.ic_fee_reserve_e8s += u128::from(j.reserved_fee_e8s);
                }
            }
        }
        Ok(b)
    }

    /// `E >= ceil(O + D + R + H) + buffer + pending IC fees`, and the XRPL snapshot
    /// must agree with `O` (plus in-flight issuance). New issuance is allowed only
    /// when the status is [`CoverageStatus::Covered`].
    pub fn check_coverage(&self, input: &CoverageInput) -> Result<CoverageReport, XrplIcusdError> {
        let b = self.buckets()?;
        let mut sum = b.o;
        add(&mut sum, &b.d)?;
        add(&mut sum, &b.r)?;
        add(&mut sum, &b.h)?;
        let buffer = u128::from(self.config.rounding_buffer_e8s);
        let required = sum.ceil_e8s() + buffer + b.ic_fee_reserve_e8s;
        let e = input.escrow_balance_e8s;
        let status = if e < required {
            CoverageStatus::Shortfall {
                short_e8s: required - e,
            }
        } else {
            match &input.snapshot {
                None => CoverageStatus::SnapshotMissing,
                Some(s) => {
                    let upper = b.o.checked_add(&b.in_flight).map_err(internal)?;
                    let lower = b
                        .o
                        .checked_sub(&input.unprocessed_return_allowance)
                        .unwrap_or(WideAmount::ZERO);
                    if s.total > upper {
                        CoverageStatus::SnapshotDisagrees {
                            detail: format!(
                                "XRPL obligations {} exceed recorded outstanding {} plus in-flight {}",
                                s.total.to_decimal_string(),
                                b.o.to_decimal_string(),
                                b.in_flight.to_decimal_string()
                            ),
                        }
                    } else if s.total < lower {
                        CoverageStatus::SnapshotDisagrees {
                            detail: format!(
                                "XRPL obligations {} are below recorded outstanding {}",
                                s.total.to_decimal_string(),
                                b.o.to_decimal_string()
                            ),
                        }
                    } else {
                        CoverageStatus::Covered
                    }
                }
            }
        };
        Ok(CoverageReport {
            status,
            escrow_balance_e8s: e,
            required_e8s: required,
            d_e8s: b.d.ceil_e8s(),
            o_e8s: b.o.ceil_e8s(),
            r_e8s: b.r.ceil_e8s(),
            h_e8s: b.h.ceil_e8s(),
            buffer_e8s: buffer,
            ic_fee_reserve_e8s: b.ic_fee_reserve_e8s,
            surplus_e8s: e.saturating_sub(required),
            snapshot_e8s: input.snapshot.as_ref().map(|s| s.total.ceil_e8s()),
            snapshot_ledger_index: input.snapshot.as_ref().map(|s| s.ledger_index),
            in_flight_e8s: b.in_flight.ceil_e8s(),
        })
    }

    // ---- IC ledger jobs ---------------------------------------------------------------

    /// Append a fully-specified job. The CALLER allocates `op_nonce` (via
    /// `State::next_op_nonce`) in the same synchronous block, and only then awaits.
    pub fn add_job(
        &mut self,
        op_id: OperationId,
        spec: JobSpec,
        op_nonce: u128,
        now_ns: u64,
    ) -> Result<usize, XrplIcusdError> {
        let op = self.op_mut(op_id)?;
        let job = IcLedgerJob {
            purpose: spec.purpose,
            op_nonce,
            created_at_time: crate::management::nonce_to_created_at_time(op_nonce),
            from_subaccount: spec.from_subaccount,
            from_owner: spec.from_owner,
            to: spec.to,
            amount_e8s: spec.amount_e8s,
            memo: op_nonce.to_be_bytes().to_vec(),
            reserved_fee_e8s: spec.reserved_fee_e8s,
            ledger_tip_before: spec.ledger_tip_before,
            status: IcJobStatus::Pending,
            proof_verified: false,
            scan_required: false,
            scan: None,
            sends: 0,
        };
        op.ic_jobs.push(job);
        op.updated_at_ns = now_ns;
        Ok(op.ic_jobs.len() - 1)
    }

    /// After an upgrade no call is outstanding: an `InFlight` job may have executed.
    /// Signed issuer blobs are preserved as-is (they are the recovery evidence).
    pub fn recover_after_upgrade(&mut self) {
        for op in self.operations.values_mut() {
            for j in &mut op.ic_jobs {
                if j.status == IcJobStatus::InFlight {
                    j.status = IcJobStatus::Unknown {
                        detail: "upgrade while a call was outstanding".into(),
                    };
                }
            }
        }
    }

    /// Anything for the background tick to do?
    pub fn has_work(&self) -> bool {
        self.setup_attempt
            .as_ref()
            .is_some_and(|a| a.status.holds_lease())
            || self.operations.values().any(Operation::needs_work)
    }

    // ---- reservation / exclusion events (replayed through the same functions) ----------

    /// Borrow reservation begun: the Borrow op (phase `Prepared`) is the durable
    /// reservation and the vault is excluded from conflicting mutations. Used by the
    /// live path AND by replay, so both rebuild the same state.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_reservation_begun(
        &mut self,
        op_id: u64,
        owner: Principal,
        request_nonce: &[u8],
        network: XrplNetwork,
        vault_id: u64,
        collateral_type: Principal,
        gross_e8s: u64,
        fee_e8s: u64,
        net_e8s: u64,
        recipient: &str,
        policy_version: u64,
        timestamp: u64,
    ) {
        if self.operations.contains_key(&op_id) {
            // Idempotent: still make sure the exclusion is present.
            self.vault_exclusions.insert(vault_id);
            return;
        }
        let mut nonce = [0u8; 32];
        let n = request_nonce.len().min(32);
        nonce[..n].copy_from_slice(&request_nonce[..n]);
        let op = Operation {
            id: op_id,
            owner,
            created_at_ns: timestamp,
            updated_at_ns: timestamp,
            request_nonce: nonce,
            network,
            kind: OperationKind::Borrow {
                vault_id,
                collateral_type,
                gross_e8s,
                fee_e8s,
                net_e8s,
                recipient: recipient.to_string(),
                policy_version,
                fee_split: None,
            },
            phase: Phase::Prepared,
            ic_jobs: vec![],
            xrpl_attempts: vec![],
            notes: vec![],
            resolution: None,
            attempt_budget_base: 0,
            delivery_halted: false,
        };
        self.request_nonce_index.insert((owner, nonce), op_id);
        self.next_operation_id = self.next_operation_id.max(op_id + 1);
        self.operations.insert(op_id, op);
        self.vault_exclusions.insert(vault_id);
    }

    /// Reservation released: `MintFailed` cancels the operation (no debt);
    /// `DebtCommitted` hands the operation to delivery. Either way the exclusion goes.
    pub fn apply_reservation_released(
        &mut self,
        op_id: u64,
        vault_id: u64,
        reason: ReleaseReason,
        timestamp: u64,
    ) {
        if let Some(op) = self.operations.get_mut(&op_id) {
            if op.phase.is_reservation() {
                op.phase = match reason {
                    ReleaseReason::MintFailed => Phase::Cancelled,
                    ReleaseReason::DebtCommitted => Phase::DebtCommitted,
                };
                op.updated_at_ns = timestamp;
            }
        }
        self.vault_exclusions.remove(&vault_id);
    }

    /// Repayment cutoff begun: the vault is excluded AND its interest accrual is
    /// frozen until the burn resolves.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_repay_cutoff_begun(
        &mut self,
        op_id: u64,
        owner: Principal,
        request_nonce: &[u8],
        network: XrplNetwork,
        vault_id: u64,
        payer: &str,
        destination_tag: u32,
        max_e8s: u64,
        full_close: bool,
        cutoff: CutoffSnapshot,
    ) {
        match self.operations.get_mut(&op_id) {
            Some(op) => {
                if let OperationKind::Repayment { cutoff: c, .. } = &mut op.kind {
                    *c = Some(cutoff.clone());
                }
                op.phase = Phase::EscrowBurnPending;
                op.updated_at_ns = cutoff.cutoff_ns;
            }
            None => {
                let mut nonce = [0u8; 32];
                let n = request_nonce.len().min(32);
                nonce[..n].copy_from_slice(&request_nonce[..n]);
                let ts = cutoff.cutoff_ns;
                let repay = cutoff.repay_e8s;
                let op = Operation {
                    id: op_id,
                    owner,
                    created_at_ns: ts,
                    updated_at_ns: ts,
                    request_nonce: nonce,
                    network,
                    kind: OperationKind::Repayment {
                        vault_id,
                        payer: payer.to_string(),
                        destination_tag,
                        max_e8s,
                        full_close,
                        parent: None,
                        receipts: vec![],
                        cutoff: Some(cutoff),
                        applied_e8s: repay,
                        held: crate::chains::xrp::issued_amount::ExactIssuedAmount::ZERO,
                        children: vec![],
                    },
                    phase: Phase::EscrowBurnPending,
                    ic_jobs: vec![],
                    xrpl_attempts: vec![],
                    notes: vec![],
                    resolution: None,
                    attempt_budget_base: 0,
                    delivery_halted: false,
                };
                self.request_nonce_index.insert((owner, nonce), op_id);
                self.next_operation_id = self.next_operation_id.max(op_id + 1);
                self.operations.insert(op_id, op);
            }
        }
        self.vault_exclusions.insert(vault_id);
        self.accrual_frozen_vaults.insert(vault_id);
    }

    /// Minimal set-maintenance for a settled repayment burn: lifts the freeze and the
    /// exclusion and records the phase. TASK 5 extends this (through
    /// `State::apply_xrpl_repay_settlement`) with the frozen debt/interest math and the
    /// post-cutoff `accrue_single_vault`.
    pub fn apply_repay_settled(&mut self, op_id: u64, vault_id: u64, settled_at: u64) {
        if let Some(op) = self.operations.get_mut(&op_id) {
            op.phase = Phase::DebtApplied;
            op.updated_at_ns = settled_at;
        }
        self.accrual_frozen_vaults.remove(&vault_id);
        self.vault_exclusions.remove(&vault_id);
    }

    /// Minimal set-maintenance for a proven never-executed burn: lifts the freeze and
    /// the exclusion and moves the operation to `Held`. TASK 5 extends this with
    /// `accrue_single_vault(at)` on the full still-unpaid debt and the held-receipt
    /// bookkeeping.
    pub fn apply_repay_cancelled(&mut self, op_id: u64, vault_id: u64, at: u64) {
        if let Some(op) = self.operations.get_mut(&op_id) {
            op.phase = Phase::Held;
            op.updated_at_ns = at;
        }
        self.accrual_frozen_vaults.remove(&vault_id);
        self.vault_exclusions.remove(&vault_id);
    }
}

impl Operation {
    /// Does background work remain for this operation?
    pub fn needs_work(&self) -> bool {
        let jobs_open = self
            .ic_jobs
            .iter()
            .any(|j| !j.is_resolved() || (j.done_block().is_some() && !j.proof_verified));
        // A terminal operation can still own an unresolved ledger job (a borrow's
        // fee-remainder mint outlives a quick delivery): keep driving it.
        if self.phase.is_terminal() {
            return jobs_open;
        }
        // A borrow stays active until its escrow mint is proven and the debt is
        // committed (or the mint definitively fails).
        let borrow_active = matches!(self.kind, OperationKind::Borrow { .. })
            && matches!(
                self.phase,
                Phase::Prepared | Phase::EscrowMintPending | Phase::DebtCommitted
            );
        let claim = matches!(
            self.kind,
            OperationKind::Borrow { .. } | OperationKind::Import { .. }
        );
        let delivering = claim
            && matches!(
                self.phase,
                Phase::IssuePrepared | Phase::Signed | Phase::Submitted
            )
            && !self.delivery_halted
            && self.resolution.is_none();
        // A resolved job whose phase transition has not been applied yet.
        let advance_pending = (self.phase == Phase::EscrowPullPending
            || self.resolution == Some(ClaimAction::ReceiveCanonical))
            && self.ic_jobs.iter().any(|j| j.is_resolved());
        jobs_open
            || self.unresolved_attempt().is_some()
            || delivering
            || advance_pending
            || borrow_active
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Existing(OperationId),
    New,
}

/// What a ledger job needs; the caller supplies the nonce.
#[derive(Clone, Debug)]
pub struct JobSpec {
    pub purpose: IcJobPurpose,
    pub from_subaccount: Option<[u8; 32]>,
    pub from_owner: Option<Account>,
    pub to: Account,
    pub amount_e8s: u64,
    pub reserved_fee_e8s: u64,
    pub ledger_tip_before: Option<u64>,
}

/// Two requests are "the same" for idempotency when their owner-authorized parameters
/// match (derived/frozen fields such as `fee_split` and `policy_version` excluded).
fn same_request(a: &OperationKind, b: &OperationKind) -> bool {
    match (a, b) {
        (
            OperationKind::Import {
                amount_e8s: a1,
                recipient: r1,
            },
            OperationKind::Import {
                amount_e8s: a2,
                recipient: r2,
            },
        ) => a1 == a2 && r1 == r2,
        (
            OperationKind::Borrow {
                vault_id: v1,
                gross_e8s: g1,
                recipient: r1,
                ..
            },
            OperationKind::Borrow {
                vault_id: v2,
                gross_e8s: g2,
                recipient: r2,
                ..
            },
        ) => v1 == v2 && g1 == g2 && r1 == r2,
        (
            OperationKind::Repayment {
                vault_id: v1,
                payer: p1,
                max_e8s: m1,
                full_close: f1,
                ..
            },
            OperationKind::Repayment {
                vault_id: v2,
                payer: p2,
                max_e8s: m2,
                full_close: f2,
                ..
            },
        ) => v1 == v2 && p1 == p2 && m1 == m2 && f1 == f2,
        _ => false,
    }
}

// ----------------------------------------------------------------------------------
// Views
// ----------------------------------------------------------------------------------

pub fn operation_view(op: &Operation) -> OperationView {
    let (gross, fee, net, applied) = match &op.kind {
        OperationKind::Borrow {
            gross_e8s,
            fee_e8s,
            net_e8s,
            ..
        } => (*gross_e8s, *fee_e8s, *net_e8s, 0),
        OperationKind::Import { amount_e8s, .. } => (*amount_e8s, 0, *amount_e8s, 0),
        OperationKind::Repayment {
            max_e8s,
            applied_e8s,
            ..
        } => (*max_e8s, 0, *applied_e8s, *applied_e8s),
        OperationKind::Refund { .. } => (0, 0, 0, 0),
    };
    let (observed, held, cutoff) = match &op.kind {
        OperationKind::Repayment {
            receipts,
            held,
            cutoff,
            ..
        } => {
            let mut sum = WideAmount::ZERO;
            let mut ok = true;
            for r in receipts {
                match WideAmount::from_exact(&r.delivered).and_then(|w| sum.checked_add(&w)) {
                    Ok(s) => sum = s,
                    Err(_) => ok = false,
                }
            }
            (
                (!receipts.is_empty() && ok).then(|| sum.to_decimal_string()),
                (!held.is_zero()).then(|| held.to_decimal_string()),
                cutoff.as_ref().map(|c| c.cutoff_ns),
            )
        }
        OperationKind::Refund { amount, .. } => (None, Some(amount.to_decimal_string()), None),
        _ => (None, None, None),
    };
    let blocks = op.ic_jobs.iter().filter_map(|j| j.done_block()).collect();
    let hashes = op
        .xrpl_attempts
        .iter()
        .filter_map(|a| a.tx_hash.clone())
        .collect();
    let quarantined = op
        .xrpl_attempts
        .iter()
        .any(|a| matches!(a.status, AttemptStatus::Quarantined { .. }));
    let next_action = if op.phase.is_terminal() {
        NextAction::None
    } else if quarantined {
        NextAction::ContactOperator
    } else if op.delivery_halted
        && matches!(op.phase, Phase::DebtCommitted | Phase::IssuePrepared)
    {
        NextAction::ResolveClaim
    } else {
        NextAction::Wait
    };
    OperationView {
        operation_id: op.id,
        kind: op.kind.tag(),
        vault_id: op.kind.vault_id(),
        phase: op.phase,
        network: op.network,
        recipient: op.kind.recipient().map(str::to_string),
        gross_e8s: gross,
        fee_e8s: fee,
        net_e8s: net,
        observed_exact_amount: observed,
        debt_applied_e8s: applied,
        held_exact_amount: held,
        settlement_cutoff_ns: cutoff,
        ic_ledger_blocks: blocks,
        xrpl_tx_hashes: hashes,
        next_action,
        resolution: op.resolution,
        notes: op.notes.clone(),
        created_at_ns: op.created_at_ns,
        updated_at_ns: op.updated_at_ns,
    }
}

pub fn public_config(st: &XrplIcusdState, backend: Principal) -> PublicXrplIcusdConfig {
    let c = &st.config;
    PublicXrplIcusdConfig {
        enabled: c.enabled,
        issuance_paused: c.issuance_paused,
        settlement_paused: c.settlement_paused,
        ready: st.ensure_ready().is_ok(),
        network: c.network,
        issuer_address: c.issuer_address.clone(),
        currency_hex: crate::chains::xrp::issued_amount::ICUSD_CURRENCY_HEX.to_string(),
        policy: c.policy,
        issuer_setup: c.issuer_setup.clone(),
        outstanding_cap_e8s: c.outstanding_cap_e8s,
        per_operation_cap_e8s: c.per_operation_cap_e8s,
        max_fee_drops: c.max_fee_drops,
        verified_minting_account: c.verified_minting_account,
        escrow_account: escrow_account(backend),
        config_version: c.config_version,
    }
}

/// Caller-scoped page: operations of `owner` with id greater than `cursor`, ascending,
/// at most [`MAX_PAGE`] rows.
pub fn owner_page(
    st: &XrplIcusdState,
    owner: &Principal,
    cursor: Option<u64>,
    limit: u32,
) -> OperationPage {
    let limit = limit.clamp(1, MAX_PAGE) as usize;
    let start = cursor.map(|c| c.saturating_add(1)).unwrap_or(0);
    let mut items = Vec::new();
    let mut more = false;
    for (_, op) in st.operations.range(start..) {
        if op.owner != *owner {
            continue;
        }
        if items.len() == limit {
            more = true;
            break;
        }
        items.push(operation_view(op));
    }
    let next_cursor = if more {
        items.last().map(|v| v.operation_id)
    } else {
        None
    };
    OperationPage { items, next_cursor }
}

// ----------------------------------------------------------------------------------
// Event application (live + replay share this)
// ----------------------------------------------------------------------------------

/// Apply one lane event to the full state. Replay calls this for every
/// `Event::XrplIcusd` in log order, interleaved with `Event::AccrueInterest`, so the
/// accrual freeze and the exclusions are in place BEFORE later accrual events run.
pub fn apply_xrpl_icusd_event(state: &mut crate::state::State, ev: &XrplIcusdEvent) {
    match ev {
        XrplIcusdEvent::ReservationBegun {
            op_id,
            owner,
            request_nonce,
            network,
            vault_id,
            collateral_type,
            gross_e8s,
            fee_e8s,
            net_e8s,
            recipient,
            policy_version,
            timestamp,
        } => state.xrpl_icusd.apply_reservation_begun(
            *op_id,
            *owner,
            request_nonce,
            *network,
            *vault_id,
            *collateral_type,
            *gross_e8s,
            *fee_e8s,
            *net_e8s,
            recipient,
            *policy_version,
            *timestamp,
        ),
        XrplIcusdEvent::ReservationReleased {
            op_id,
            vault_id,
            reason,
            timestamp,
        } => state
            .xrpl_icusd
            .apply_reservation_released(*op_id, *vault_id, *reason, *timestamp),
        XrplIcusdEvent::RepayCutoffBegun {
            op_id,
            owner,
            request_nonce,
            network,
            vault_id,
            payer,
            destination_tag,
            max_e8s,
            full_close,
            cutoff_ns,
            snapshot_debt_e8s,
            snapshot_accrued_e8s,
            repay_e8s,
            interest_share_e8s,
            last_accrual_time,
        } => state.xrpl_icusd.apply_repay_cutoff_begun(
            *op_id,
            *owner,
            request_nonce,
            *network,
            *vault_id,
            payer,
            *destination_tag,
            *max_e8s,
            *full_close,
            CutoffSnapshot {
                cutoff_ns: *cutoff_ns,
                snapshot_debt_e8s: *snapshot_debt_e8s,
                snapshot_accrued_e8s: *snapshot_accrued_e8s,
                repay_e8s: *repay_e8s,
                interest_share_e8s: *interest_share_e8s,
                last_accrual_time: *last_accrual_time,
            },
        ),
        XrplIcusdEvent::RepaySettled {
            op_id,
            vault_id,
            settled_at,
            ..
        } => state
            .xrpl_icusd
            .apply_repay_settled(*op_id, *vault_id, *settled_at),
        XrplIcusdEvent::RepayCancelled { op_id, vault_id, at } => {
            state.xrpl_icusd.apply_repay_cancelled(*op_id, *vault_id, *at)
        }
    }
}
