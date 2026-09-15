//! Governance: signer-gated multisig proposals, timelocked execution,
//! immediate pause (Task 2A), and signer-gated alarm acknowledgement
//! (Task 2B), layered on top of the `state::alarms` module (dedup,
//! auto-resolve, and the deterministic core of acknowledge) that Task 2B
//! introduces in `state.rs`.
//!
//! ## Layering
//! `state.rs` owns storage, raw CRUD, and count-bound enforcement; it never
//! checks a caller's identity. This file owns the opposite half: every
//! function that can be reached from a Candid update method starts by
//! checking [`require_signer`], and every governed mutation (register,
//! update, remove a target; change global policy; add/remove a signer;
//! change the threshold; unpause a target) goes through the
//! propose -> approve -> execute state machine below. Immediate pause is the
//! one exception the design calls out explicitly: a single signer may pause
//! a target directly, with no proposal.
//!
//! ## The `*_at(caller, now_secs)` pattern
//! Every function below that needs the calling identity or wall-clock time
//! takes it as an explicit parameter instead of reading `ic_cdk::caller()` /
//! `ic_cdk::api::time()` itself. `lib.rs` is the only place that reads those
//! from the IC execution context; every `*_at` function here is a plain,
//! deterministic Rust function callable from a host `#[test]` with no
//! `ic-cdk` execution context at all. A function that does not need the
//! caller or the clock (e.g. [`approve_proposal_at`], which never touches
//! time) simply omits that parameter — there is no value in threading an
//! unused parameter through just to match a fixed shape.
//!
//! ## Two-phase validation
//! Every `propose_*_at` function re-validates its payload against the
//! CURRENT registry/global policy before creating the proposal, purely as a
//! fast-fail courtesy to the proposer — a proposal that could never execute
//! is still allowed to exist (a signer may want to fix state and retry, or
//! simply `cancel_proposal_at`). The authoritative check is
//! [`execute_proposal_at`]'s: it re-validates fully against whatever the
//! registry/global policy/signer set actually are AT EXECUTION TIME (which,
//! after a timelock delay, may have drifted from what they were at propose
//! time), and it never applies a partial mutation — every branch of
//! [`apply_payload`] computes its new value(s) fully in memory and only
//! then makes its one synchronous stable write, so a returned error implies
//! zero mutation and the proposal is left `Open` for a signer to retry or
//! cancel.
//!
//! ## Exact-once execution
//! [`execute_proposal_at`] checks `proposal.status == Open` before doing
//! anything else and unconditionally flips it to `Executed` as the very
//! last step (after every stable write for this proposal has already
//! landed) — so a second `execute_proposal_at(id)` call on the same id
//! always returns [`GovernanceError::ProposalNotOpen`] rather than
//! re-applying the payload.
//!
//! ## Signer removal and open proposals
//! `RemoveSigner`'s own execution additionally cancels every OTHER `Open`
//! proposal that already carries an approval from the signer being removed
//! (see [`cancel_open_proposals_approved_by`]) — otherwise a proposal could
//! sit at (or above) threshold using an approval from a principal that is
//! no longer a signer, which `state::validate_whole_state`'s
//! `ProposalApprovalNotASigner` check would then reject at the next
//! `post_upgrade`. Cancelling synchronously, in the same call, keeps the
//! stable state one that always already passes that validator — not just
//! eventually, at the next upgrade.

use candid::{CandidType, Principal};
use serde::Deserialize;

use crate::state::alarms::AlarmError;
use crate::state::{self, GlobalConfig, RemoveTargetError};
use crate::types::{
    self, GlobalPolicy, GlobalPolicyArgs, GlobalPolicyError, GovernanceTimelocks, ProposalKind,
    ProposalPayload, ProposalRecord, ProposalStatus, TargetArgs, TargetPatch, TargetRecord,
    TargetRegistrationContext, TargetValidationError,
};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) enum GovernanceError {
    /// The caller is anonymous or is not a current signer. Anonymous is
    /// never a special case here: `GlobalConfig.signers` can never contain
    /// the anonymous principal (every path that can add a signer rejects
    /// it — see the `AnonymousSigner` variant below), so
    /// `state::is_signer(Principal::anonymous())` is always `false` and
    /// this one check rejects both cases uniformly.
    NotSigner,
    ProposalNotFound,
    /// The proposal is not `Open` — either already `Executed`, already
    /// `Cancelled`, or (for `execute_proposal_at`/`cancel_proposal_at`
    /// specifically) this is a replay of a call that already succeeded.
    ProposalNotOpen,
    ThresholdNotMet,
    TimelockNotElapsed,
    TargetNotFound,
    /// `UnpauseTarget` was proposed, or is being executed, against a target
    /// that is not currently paused. Checked both at propose time (a signer
    /// cannot pre-stage an unpause proposal against a healthy target before
    /// an incident occurs, which would let its timelock run out in the
    /// background and defeat the emergency pause's cooldown the instant a
    /// real pause happens) and again at execute time (the authoritative
    /// check: the target may have been unpaused by a different, already-
    /// executed `UnpauseTarget` proposal since this one was created).
    TargetNotPaused,
    InvalidTarget(TargetValidationError),
    InvalidGlobalPolicy(GlobalPolicyError),
    RemoveTargetBlocked(RemoveTargetError),
    /// The registry is at `types::MAX_TARGETS` and this would be a
    /// genuinely new target.
    TooManyTargets,
    TooManyOpenProposals,
    AnonymousSigner,
    /// The management canister principal (`aaaaa-aa`) can never originate an
    /// update call, so it can never approve or execute a proposal — adding it
    /// as a signer would silently and permanently brick governance. Checked
    /// both at propose time (fast-fail) and again in `apply_payload`'s
    /// `AddSigner` arm at execute time, mirroring `AnonymousSigner` above.
    ManagementSigner,
    DuplicateSigner(Principal),
    SignerNotFound(Principal),
    /// Removing this signer would leave zero signers.
    EmptySigners,
    ThresholdZero,
    ThresholdExceedsSigners {
        threshold: u32,
        signer_count: u32,
    },
    /// A `SetGlobalPolicy` proposal's new `global_daily_cap_cycles` would be
    /// below an existing target's own `daily_cap_cycles` — checked both at
    /// propose time (fast-fail) and again at execute time against
    /// whatever the registry actually looks like then.
    GlobalCapBelowExistingTargetCap {
        target: Principal,
    },
    /// A `SetGlobalPolicy` proposal's new `global_daily_cap_cycles` would be
    /// below an unresolved, ordinary (non-self-recovery) `FundingOperation`'s
    /// own snapshotted `daily_cap_cycles` — the same comparison
    /// `state::validate_whole_state`'s `OperationDailyCapExceedsGlobalCap`
    /// check makes. Checked at both propose and execute time so this can
    /// never write a cap that would trap the very next `post_upgrade`.
    GlobalCapBelowUnresolvedOperationCap {
        operation_id: u64,
    },
    /// A `SetGlobalPolicy` proposal's new `self_recovery_policy.daily_cap_cycles`
    /// would be below the self-recovery ledger's own live pending reservation
    /// — the same comparison `state::validate_whole_state`'s
    /// `SelfRecoveryLedgerExceedsDailyCap` check makes.
    SelfRecoveryCapBelowPendingReservation {
        operation_id: u64,
    },
    Alarm(AlarmError),
}

fn require_signer(caller: Principal) -> Result<(), GovernanceError> {
    if state::is_signer(caller) {
        Ok(())
    } else {
        Err(GovernanceError::NotSigner)
    }
}

/// The design's four timelock buckets map onto the eight proposal kinds by
/// what they govern: registry membership/shape (`RegisterTarget`,
/// `UpdateTarget`, `RemoveTarget`), spend authority (`SetGlobalPolicy`),
/// who can approve at all (`AddSigner`, `RemoveSigner`,
/// `SetSignerThreshold`), and resuming a paused target (`UnpauseTarget`).
fn required_timelock_secs(kind: ProposalKind, timelocks: &GovernanceTimelocks) -> u64 {
    match kind {
        ProposalKind::RegisterTarget | ProposalKind::UpdateTarget | ProposalKind::RemoveTarget => {
            timelocks.target_registry_secs()
        }
        ProposalKind::SetGlobalPolicy => timelocks.spend_policy_secs(),
        ProposalKind::AddSigner | ProposalKind::RemoveSigner | ProposalKind::SetSignerThreshold => {
            timelocks.signer_change_secs()
        }
        ProposalKind::UnpauseTarget => timelocks.unpause_secs(),
    }
}

/// Every current target's `daily_cap_cycles`, every unresolved ordinary
/// operation's own snapshotted `daily_cap_cycles`, and the self-recovery
/// ledger's live pending reservation must all still fit under a candidate
/// new global/self-recovery policy. Shared by `propose_set_global_policy_at`
/// (fast-fail) and `apply_payload`'s `SetGlobalPolicy` branch (the
/// authoritative check, against whatever state actually looks like at
/// execute time) — mirrors exactly the comparisons
/// `state::validate_whole_state`'s `OperationDailyCapExceedsGlobalCap` and
/// `SelfRecoveryLedgerExceedsDailyCap` checks make, so a `SetGlobalPolicy`
/// proposal can never write a policy that would trap the very next
/// `post_upgrade`.
fn check_targets_fit_global_cap(new_policy: &GlobalPolicy) -> Result<(), GovernanceError> {
    for principal in state::target_principals() {
        if let Some(record) = state::get_target(principal) {
            if record.funding_policy().daily_cap_cycles() > new_policy.global_daily_cap_cycles() {
                return Err(GovernanceError::GlobalCapBelowExistingTargetCap { target: principal });
            }
        }
    }
    for op in state::list_unresolved_ordinary_operations() {
        if op.funding_policy().daily_cap_cycles() > new_policy.global_daily_cap_cycles() {
            return Err(GovernanceError::GlobalCapBelowUnresolvedOperationCap {
                operation_id: op.id(),
            });
        }
    }
    if let Some(entry) = state::get_self_recovery_state()
        .rolling_spend()
        .pending()
        .first()
    {
        if entry.amount_cycles > new_policy.self_recovery_policy().daily_cap_cycles() {
            return Err(GovernanceError::SelfRecoveryCapBelowPendingReservation {
                operation_id: entry.operation_id,
            });
        }
    }
    Ok(())
}

fn insert_new_proposal(record: ProposalRecord) -> Result<u64, GovernanceError> {
    let id = record.id;
    state::insert_proposal(record).map_err(|_| GovernanceError::TooManyOpenProposals)?;
    Ok(id)
}

// ─────────────────────────── Propose ───────────────────────────

pub(crate) fn propose_register_target_at(
    caller: Principal,
    now_secs: u64,
    sentinel_id: Principal,
    args: TargetArgs,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let config = state::global_config();
    let existing_target_principals = state::target_principals();
    let ctx = TargetRegistrationContext {
        sentinel_id,
        existing_target_count: state::target_count() as usize,
        existing_target_principals: &existing_target_principals,
        global_policy: &config.global_policy,
    };
    TargetRecord::register(args.clone(), &ctx).map_err(GovernanceError::InvalidTarget)?;
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::RegisterTarget(args),
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_update_target_at(
    caller: Principal,
    now_secs: u64,
    principal: Principal,
    patch: TargetPatch,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let config = state::global_config();
    let existing = state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
    existing
        .apply_patch(patch.clone(), &config.global_policy)
        .map_err(GovernanceError::InvalidTarget)?;
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::UpdateTarget { principal, patch },
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_remove_target_at(
    caller: Principal,
    now_secs: u64,
    principal: Principal,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::RemoveTarget { principal },
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_set_global_policy_at(
    caller: Principal,
    now_secs: u64,
    args: GlobalPolicyArgs,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let new_policy = GlobalPolicy::validate(&args).map_err(GovernanceError::InvalidGlobalPolicy)?;
    check_targets_fit_global_cap(&new_policy)?;
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::SetGlobalPolicy(args),
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_add_signer_at(
    caller: Principal,
    now_secs: u64,
    signer: Principal,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    if signer == Principal::anonymous() {
        return Err(GovernanceError::AnonymousSigner);
    }
    if signer == Principal::management_canister() {
        return Err(GovernanceError::ManagementSigner);
    }
    let config = state::global_config();
    if config.signers.contains(&signer) {
        return Err(GovernanceError::DuplicateSigner(signer));
    }
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::AddSigner { signer },
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_remove_signer_at(
    caller: Principal,
    now_secs: u64,
    signer: Principal,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let config = state::global_config();
    if !config.signers.contains(&signer) {
        return Err(GovernanceError::SignerNotFound(signer));
    }
    let remaining = config.signers.len() - 1;
    if remaining == 0 {
        return Err(GovernanceError::EmptySigners);
    }
    if config.approval_threshold as usize > remaining {
        return Err(GovernanceError::ThresholdExceedsSigners {
            threshold: config.approval_threshold,
            signer_count: remaining as u32,
        });
    }
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::RemoveSigner { signer },
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_set_signer_threshold_at(
    caller: Principal,
    now_secs: u64,
    threshold: u32,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let config = state::global_config();
    if threshold == 0 {
        return Err(GovernanceError::ThresholdZero);
    }
    if threshold as usize > config.signers.len() {
        return Err(GovernanceError::ThresholdExceedsSigners {
            threshold,
            signer_count: config.signers.len() as u32,
        });
    }
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::SetSignerThreshold { threshold },
        caller,
        now_secs,
    ))
}

pub(crate) fn propose_unpause_target_at(
    caller: Principal,
    now_secs: u64,
    principal: Principal,
) -> Result<u64, GovernanceError> {
    require_signer(caller)?;
    let existing = state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
    if !existing.paused() {
        return Err(GovernanceError::TargetNotPaused);
    }
    let id = state::next_proposal_id();
    insert_new_proposal(ProposalRecord::new(
        id,
        ProposalPayload::UnpauseTarget { principal },
        caller,
        now_secs,
    ))
}

// ─────────────────────────── Approve / cancel ───────────────────────────

/// Idempotent: recording the same signer's approval twice returns `Ok(false)`
/// the second time and never re-applies (or applies for the first time) the
/// payload — applying is `execute_proposal_at`'s job alone.
pub(crate) fn approve_proposal_at(caller: Principal, id: u64) -> Result<bool, GovernanceError> {
    require_signer(caller)?;
    let mut proposal = state::get_proposal(id).ok_or(GovernanceError::ProposalNotFound)?;
    if proposal.status != ProposalStatus::Open {
        return Err(GovernanceError::ProposalNotOpen);
    }
    let newly_recorded = proposal.record_approval(caller);
    state::insert_proposal(proposal).expect(
        "rumi_cycle_sentinel: recording an approval on an existing proposal id never exceeds MAX_PROPOSALS",
    );
    Ok(newly_recorded)
}

pub(crate) fn cancel_proposal_at(caller: Principal, id: u64) -> Result<(), GovernanceError> {
    require_signer(caller)?;
    let mut proposal = state::get_proposal(id).ok_or(GovernanceError::ProposalNotFound)?;
    if proposal.status != ProposalStatus::Open {
        return Err(GovernanceError::ProposalNotOpen);
    }
    proposal.status = ProposalStatus::Cancelled;
    state::insert_proposal(proposal).expect(
        "rumi_cycle_sentinel: cancelling an existing proposal id never exceeds MAX_PROPOSALS",
    );
    Ok(())
}

// ─────────────────────────── Execute ───────────────────────────

pub(crate) fn execute_proposal_at(
    caller: Principal,
    now_secs: u64,
    sentinel_id: Principal,
    id: u64,
) -> Result<(), GovernanceError> {
    require_signer(caller)?;
    let proposal = state::get_proposal(id).ok_or(GovernanceError::ProposalNotFound)?;
    if proposal.status != ProposalStatus::Open {
        return Err(GovernanceError::ProposalNotOpen);
    }
    let config = state::global_config();
    if proposal.approval_count() < config.approval_threshold as usize {
        return Err(GovernanceError::ThresholdNotMet);
    }
    let required = required_timelock_secs(proposal.kind(), config.global_policy.timelocks());
    if now_secs.saturating_sub(proposal.created_at_secs) < required {
        return Err(GovernanceError::TimelockNotElapsed);
    }

    // `proposal.payload` is cloned once, by value, into `apply_payload` so
    // every match arm below owns its `TargetArgs`/`TargetPatch`/etc.
    // directly rather than needing a `.clone()` per field. `proposal`
    // itself is still needed afterward (to flip `status` and persist it),
    // so the alternative — moving `proposal.payload` out of `proposal` —
    // would require reconstructing `proposal` anyway.
    apply_payload(proposal.payload.clone(), id, sentinel_id, &config)?;

    let mut executed = proposal;
    executed.status = ProposalStatus::Executed;
    state::insert_proposal(executed).expect(
        "rumi_cycle_sentinel: marking an existing proposal executed never exceeds MAX_PROPOSALS",
    );
    Ok(())
}

/// Every branch validates fully against CURRENT state (never against
/// whatever was true at propose time) and performs one or more logical
/// stable writes, only after every check in that branch has already
/// succeeded — so a returned `Err` here is guaranteed to have mutated
/// nothing, and `execute_proposal_at` never marks the proposal `Executed`
/// in that case. (`RemoveSigner` is the one branch that writes more than
/// once: the config write, plus zero-or-more `insert_proposal` writes inside
/// `cancel_open_proposals_approved_by`. This is safe today only because IC's
/// whole-call atomicity reverts every write in this function if anything
/// after it traps, and none of `apply_payload`'s writes can themselves
/// fail — a future payload kind must not assume single-write atomicity that
/// the type system does not actually enforce.)
fn apply_payload(
    payload: ProposalPayload,
    proposal_id: u64,
    sentinel_id: Principal,
    config: &GlobalConfig,
) -> Result<(), GovernanceError> {
    match payload {
        ProposalPayload::RegisterTarget(args) => {
            let existing_target_principals = state::target_principals();
            let ctx = TargetRegistrationContext {
                sentinel_id,
                existing_target_count: state::target_count() as usize,
                existing_target_principals: &existing_target_principals,
                global_policy: &config.global_policy,
            };
            let record =
                TargetRecord::register(args, &ctx).map_err(GovernanceError::InvalidTarget)?;
            state::insert_target(record).map_err(|_| GovernanceError::TooManyTargets)?;
        }
        ProposalPayload::UpdateTarget { principal, patch } => {
            let existing = state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
            let updated = existing
                .apply_patch(patch, &config.global_policy)
                .map_err(GovernanceError::InvalidTarget)?;
            state::insert_target(updated).expect(
                "rumi_cycle_sentinel: updating an existing target principal never exceeds MAX_TARGETS",
            );
        }
        ProposalPayload::RemoveTarget { principal } => {
            if state::get_target(principal).is_none() {
                return Err(GovernanceError::TargetNotFound);
            }
            state::remove_target(principal).map_err(GovernanceError::RemoveTargetBlocked)?;
        }
        ProposalPayload::SetGlobalPolicy(args) => {
            let new_policy =
                GlobalPolicy::validate(&args).map_err(GovernanceError::InvalidGlobalPolicy)?;
            check_targets_fit_global_cap(&new_policy)?;
            state::set_global_config(GlobalConfig {
                signers: config.signers.clone(),
                approval_threshold: config.approval_threshold,
                global_policy: new_policy,
            });
            // The policy's sample interval is live configuration. Re-arm
            // after the durable write so the next callback uses the new
            // interval and public next_sample_at_secs remains truthful.
            crate::sampler::setup_timer();
        }
        ProposalPayload::AddSigner { signer } => {
            if signer == Principal::anonymous() {
                return Err(GovernanceError::AnonymousSigner);
            }
            if signer == Principal::management_canister() {
                return Err(GovernanceError::ManagementSigner);
            }
            if config.signers.contains(&signer) {
                return Err(GovernanceError::DuplicateSigner(signer));
            }
            let mut signers = config.signers.clone();
            signers.push(signer);
            state::set_global_config(GlobalConfig {
                signers,
                approval_threshold: config.approval_threshold,
                global_policy: config.global_policy.clone(),
            });
        }
        ProposalPayload::RemoveSigner { signer } => {
            if !config.signers.contains(&signer) {
                return Err(GovernanceError::SignerNotFound(signer));
            }
            let signers: Vec<Principal> = config
                .signers
                .iter()
                .copied()
                .filter(|s| *s != signer)
                .collect();
            if signers.is_empty() {
                return Err(GovernanceError::EmptySigners);
            }
            if config.approval_threshold as usize > signers.len() {
                return Err(GovernanceError::ThresholdExceedsSigners {
                    threshold: config.approval_threshold,
                    signer_count: signers.len() as u32,
                });
            }
            state::set_global_config(GlobalConfig {
                signers,
                approval_threshold: config.approval_threshold,
                global_policy: config.global_policy.clone(),
            });
            cancel_open_proposals_approved_by(signer, proposal_id);
        }
        ProposalPayload::SetSignerThreshold { threshold } => {
            if threshold == 0 {
                return Err(GovernanceError::ThresholdZero);
            }
            if threshold as usize > config.signers.len() {
                return Err(GovernanceError::ThresholdExceedsSigners {
                    threshold,
                    signer_count: config.signers.len() as u32,
                });
            }
            state::set_global_config(GlobalConfig {
                signers: config.signers.clone(),
                approval_threshold: threshold,
                global_policy: config.global_policy.clone(),
            });
        }
        ProposalPayload::UnpauseTarget { principal } => {
            let existing = state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
            if !existing.paused() {
                return Err(GovernanceError::TargetNotPaused);
            }
            state::insert_target(existing.unpause()).expect(
                "rumi_cycle_sentinel: unpausing an existing target principal never exceeds MAX_TARGETS",
            );
        }
    }
    Ok(())
}

/// Cancels every OTHER `Open` proposal (`proposal.id != exclude_proposal_id`)
/// that already carries an approval from `removed_signer`. `MAX_PROPOSALS`
/// (256) bounds the store, so one unpaginated scan is sufficient — see the
/// module doc's "Signer removal and open proposals" section for why this
/// must happen synchronously, in the same call that removes the signer.
fn cancel_open_proposals_approved_by(removed_signer: Principal, exclude_proposal_id: u64) {
    for mut proposal in state::list_proposals_after(None, types::MAX_PROPOSALS) {
        if proposal.id == exclude_proposal_id {
            continue;
        }
        if proposal.status == ProposalStatus::Open && proposal.has_approved(removed_signer) {
            proposal.status = ProposalStatus::Cancelled;
            state::insert_proposal(proposal).expect(
                "rumi_cycle_sentinel: cascading cancel of an existing proposal id never exceeds MAX_PROPOSALS",
            );
        }
    }
}

// ─────────────────────────── Immediate pause ───────────────────────────

/// Single-signer, no proposal, no threshold, no timelock — the design's one
/// explicit exception to the governed model. Unpausing is NOT the mirror of
/// this function; it only ever happens through the governed
/// `UnpauseTarget` proposal kind above.
pub(crate) fn pause_target_at(
    caller: Principal,
    principal: Principal,
) -> Result<(), GovernanceError> {
    require_signer(caller)?;
    let record = state::get_target(principal).ok_or(GovernanceError::TargetNotFound)?;
    state::insert_target(record.pause()).expect(
        "rumi_cycle_sentinel: pausing an existing target principal never exceeds MAX_TARGETS",
    );
    Ok(())
}

// ─────────────────────────── Alarm acknowledgement ───────────────────────────

/// Signer-gated only — the dedup/idempotency/rejection semantics live in
/// `state::alarms::acknowledge_at`; see that function's (and its module's)
/// doc comment for the exact contract: idempotent replay on an already
/// `Acknowledged` alarm, rejected on an already `Resolved` one.
pub(crate) fn acknowledge_alarm_at(
    caller: Principal,
    now_secs: u64,
    id: u64,
) -> Result<bool, GovernanceError> {
    require_signer(caller)?;
    state::alarms::acknowledge_at(id, now_secs).map_err(GovernanceError::Alarm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::alarms;
    use crate::types::{
        AlarmKind, Criticality, CyclesWithdrawSnapshot, Environment, FundingOperation,
        FundingRailArguments, FundingTrigger, GlobalPolicyArgs, GlobalRollingSpendState,
        GovernanceTimelocksArgs, ObservationMode, SelfRecoveryPolicyArgs, TargetFundingPolicy,
        TargetFundingPolicyArgs, TargetReservationState,
    };
    use candid::Nat;

    // ── fixtures ──

    fn sentinel_id() -> Principal {
        Principal::from_slice(&[1, 1, 1])
    }

    fn signer(seed: u8) -> Principal {
        Principal::from_slice(&[10, seed])
    }

    fn nonsigner() -> Principal {
        Principal::from_slice(&[20, 20])
    }

    fn target_principal(seed: u8) -> Principal {
        Principal::from_slice(&[30, seed])
    }

    /// All four timelock buckets get DISTINCT values so per-kind tests can
    /// prove `required_timelock_secs` actually reads the right one instead
    /// of coincidentally passing with one shared value.
    fn timelocks_args() -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: 100,
            spend_policy_secs: 200,
            signer_change_secs: 300,
            unpause_secs: 400,
        }
    }

    fn self_recovery_args() -> SelfRecoveryPolicyArgs {
        SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(1_000u64),
            daily_cap_cycles: Nat::from(100u64),
            low_balance_threshold_cycles: Nat::from(1u64),
            refill_cycles: Nat::from(10u64),
        }
    }

    fn global_policy_args(cap: u128) -> GlobalPolicyArgs {
        GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(cap),
            sample_interval_secs: 60,
            stale_after_secs: 120,
            min_icp_reserve_e8s: Nat::from(100_000_000u64),
            timelocks: timelocks_args(),
            self_recovery_policy: self_recovery_args(),
        }
    }

    /// Same as `global_policy_args`, with the self-recovery daily cap
    /// overridden — used to exercise `SelfRecoveryCapBelowPendingReservation`
    /// independently of the global/target cap fields.
    fn global_policy_args_with_self_recovery_cap(
        cap: u128,
        self_recovery_daily_cap: u128,
    ) -> GlobalPolicyArgs {
        let mut args = global_policy_args(cap);
        args.self_recovery_policy.daily_cap_cycles = Nat::from(self_recovery_daily_cap);
        args
    }

    fn init_governed(signers: Vec<Principal>, threshold: u32, cap: u128) {
        state::init(types::InitArgs {
            signers,
            approval_threshold: threshold,
            global_policy: global_policy_args(cap),
        })
        .unwrap();
    }

    fn funding_policy_args(low: u128, refill: u128, cap: u128) -> TargetFundingPolicyArgs {
        TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(low),
            refill_cycles: Nat::from(refill),
            daily_cap_cycles: Nat::from(cap),
            cooldown_secs: 60,
            burn_anomaly_limit_cycles_per_day: None,
        }
    }

    fn target_args(principal: Principal) -> TargetArgs {
        TargetArgs {
            principal,
            display_name: "svc".to_string(),
            project: "proj".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            tags: vec![],
            funding_policy: funding_policy_args(1, 10, 100),
        }
    }

    /// Proposes, approves (by every signer in `signers`), and executes a
    /// `RegisterTarget` proposal for `principal` at `now_secs` (with the
    /// timelock already elapsed by construction — see call sites), leaving
    /// the target registered. Panics on any unexpected error since this is
    /// setup, not the behavior under test.
    fn register_target(signers: &[Principal], principal: Principal, now_secs: u64) -> u64 {
        let id =
            propose_register_target_at(signers[0], now_secs, sentinel_id(), target_args(principal))
                .unwrap();
        for s in signers {
            approve_proposal_at(*s, id).unwrap();
        }
        execute_proposal_at(signers[0], now_secs + 1_000, sentinel_id(), id).unwrap();
        id
    }

    // ── require_signer: anonymous + nonsigner, one test per entry point ──

    #[test]
    fn propose_register_target_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_register_target_at(
                    caller,
                    0,
                    sentinel_id(),
                    target_args(target_principal(1))
                ),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_update_target_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_update_target_at(
                    caller,
                    0,
                    target_principal(1),
                    TargetPatch {
                        display_name: None,
                        project: None,
                        environment: None,
                        criticality: None,
                        observation_mode: None,
                        tags: None,
                        funding_policy: None,
                        enabled: None,
                        auto_topup: None,
                    }
                ),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_remove_target_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_remove_target_at(caller, 0, target_principal(1)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_set_global_policy_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_set_global_policy_at(caller, 0, global_policy_args(2_000)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_add_signer_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_add_signer_at(caller, 0, signer(2)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_remove_signer_requires_signer() {
        init_governed(vec![signer(1), signer(2)], 1, 1_000);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_remove_signer_at(caller, 0, signer(2)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_set_signer_threshold_requires_signer() {
        init_governed(vec![signer(1), signer(2)], 1, 1_000);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_set_signer_threshold_at(caller, 0, 2),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn propose_unpause_target_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                propose_unpause_target_at(caller, 0, target_principal(1)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn approve_proposal_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                approve_proposal_at(caller, id),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn execute_proposal_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                execute_proposal_at(caller, 10_000, sentinel_id(), id),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn cancel_proposal_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                cancel_proposal_at(caller, id),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn pause_target_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                pause_target_at(caller, target_principal(1)),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    #[test]
    fn acknowledge_alarm_requires_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = alarms::raise_at(Some(target_principal(1)), AlarmKind::LowBalance, 0).unwrap();
        for caller in [Principal::anonymous(), nonsigner()] {
            assert_eq!(
                acknowledge_alarm_at(caller, 0, id),
                Err(GovernanceError::NotSigner)
            );
        }
    }

    // ── propose-time validation negatives ──

    #[test]
    fn propose_register_target_rejects_duplicate() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        assert_eq!(
            propose_register_target_at(
                signer(1),
                0,
                sentinel_id(),
                target_args(target_principal(1))
            ),
            Err(GovernanceError::InvalidTarget(
                TargetValidationError::DuplicateTarget
            ))
        );
    }

    #[test]
    fn propose_register_target_rejects_reserved_principal() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_register_target_at(signer(1), 0, sentinel_id(), target_args(sentinel_id())),
            Err(GovernanceError::InvalidTarget(
                TargetValidationError::ReservedPrincipal(
                    types::ReservedPrincipalKind::SentinelSelf
                )
            ))
        );
    }

    #[test]
    fn propose_update_target_rejects_missing_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_update_target_at(
                signer(1),
                0,
                target_principal(9),
                TargetPatch {
                    display_name: None,
                    project: None,
                    environment: None,
                    criticality: None,
                    observation_mode: None,
                    tags: None,
                    funding_policy: None,
                    enabled: None,
                    auto_topup: None,
                }
            ),
            Err(GovernanceError::TargetNotFound)
        );
    }

    #[test]
    fn propose_remove_target_rejects_missing_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_remove_target_at(signer(1), 0, target_principal(9)),
            Err(GovernanceError::TargetNotFound)
        );
    }

    #[test]
    fn propose_unpause_target_rejects_missing_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_unpause_target_at(signer(1), 0, target_principal(9)),
            Err(GovernanceError::TargetNotFound)
        );
    }

    /// Governance review Blocker 1: a signer must not be able to pre-stage
    /// an `UnpauseTarget` proposal against a currently-healthy target. If
    /// this were allowed, the proposal's timelock could run out in the
    /// background while the target stays healthy, so the instant a future
    /// emergency `pause_target_at` fires, the pre-staged proposal is
    /// already executable — collapsing the unpause cooldown to zero.
    #[test]
    fn propose_unpause_target_rejects_unpaused_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        assert!(!state::get_target(target_principal(1)).unwrap().paused());
        assert_eq!(
            propose_unpause_target_at(signer(1), 0, target_principal(1)),
            Err(GovernanceError::TargetNotPaused)
        );
    }

    #[test]
    fn propose_set_global_policy_rejects_cap_below_existing_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0); // daily_cap_cycles = 100
        assert_eq!(
            propose_set_global_policy_at(signer(1), 0, global_policy_args(50)),
            Err(GovernanceError::GlobalCapBelowExistingTargetCap {
                target: target_principal(1)
            })
        );
    }

    /// Governance review forward-looking finding: `SetGlobalPolicy` must
    /// reject a candidate global cap that would leave an unresolved,
    /// ordinary (non-self-recovery) `FundingOperation`'s own snapshotted
    /// `daily_cap_cycles` unable to fit — the exact comparison
    /// `state::validate_whole_state`'s `OperationDailyCapExceedsGlobalCap`
    /// check makes. Without this, the proposal would write cleanly today and
    /// only trap at the NEXT `post_upgrade`.
    #[test]
    fn propose_set_global_policy_rejects_cap_below_unresolved_operation_cap() {
        init_governed(vec![signer(1)], 1, 1_000);
        // The target's OWN registered cap (10) comfortably fits the
        // candidate policy's 50 — only the unresolved operation's own
        // snapshotted cap (100, from `open_in_flight_operation`) should
        // trip this check, proving it is genuinely independent of the
        // existing-target-registry check above.
        let mut args = target_args(target_principal(1));
        args.funding_policy = funding_policy_args(1, 10, 10);
        let register_id = propose_register_target_at(signer(1), 0, sentinel_id(), args).unwrap();
        approve_proposal_at(signer(1), register_id).unwrap();
        execute_proposal_at(signer(1), 100, sentinel_id(), register_id).unwrap();

        let global = state::global_config().global_policy;
        let op_id = open_in_flight_operation(target_principal(1), &global, 100); // snapshot cap = 100

        assert_eq!(
            propose_set_global_policy_at(signer(1), 100, global_policy_args(50)),
            Err(GovernanceError::GlobalCapBelowUnresolvedOperationCap {
                operation_id: op_id
            })
        );
    }

    /// Same forward-looking finding as
    /// `propose_set_global_policy_rejects_cap_below_unresolved_operation_cap`,
    /// against the self-recovery ledger's own live pending reservation
    /// (`SelfRecoveryLedgerExceedsDailyCap`) instead of an ordinary
    /// operation's snapshotted cap.
    #[test]
    fn propose_set_global_policy_rejects_self_recovery_cap_below_pending_reservation() {
        init_governed(vec![signer(1)], 1, 1_000);
        let op_id = open_self_recovery_operation(sentinel_id(), 60, 0);
        let args = global_policy_args_with_self_recovery_cap(2_000, 50);
        assert_eq!(
            propose_set_global_policy_at(signer(1), 0, args),
            Err(GovernanceError::SelfRecoveryCapBelowPendingReservation {
                operation_id: op_id
            })
        );
    }

    #[test]
    fn propose_add_signer_rejects_anonymous_and_duplicate() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_add_signer_at(signer(1), 0, Principal::anonymous()),
            Err(GovernanceError::AnonymousSigner)
        );
        assert_eq!(
            propose_add_signer_at(signer(1), 0, signer(1)),
            Err(GovernanceError::DuplicateSigner(signer(1)))
        );
    }

    /// Correction pass: the management canister principal (`aaaaa-aa`) can
    /// never originate an update call, so it can never approve or execute a
    /// proposal. Rejected here at propose time (the only production path
    /// that can ever construct an `AddSigner` proposal); `apply_payload`'s
    /// `AddSigner` arm carries the identical check as defense in depth,
    /// mirroring how `AnonymousSigner` is already checked in both places.
    #[test]
    fn propose_add_signer_rejects_management_canister() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_add_signer_at(signer(1), 0, Principal::management_canister()),
            Err(GovernanceError::ManagementSigner)
        );
    }

    #[test]
    fn propose_remove_signer_rejects_nonsigner_and_last_signer() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            propose_remove_signer_at(signer(1), 0, signer(2)),
            Err(GovernanceError::SignerNotFound(signer(2)))
        );
        assert_eq!(
            propose_remove_signer_at(signer(1), 0, signer(1)),
            Err(GovernanceError::EmptySigners)
        );
    }

    #[test]
    fn propose_remove_signer_rejects_when_threshold_would_be_violated() {
        init_governed(vec![signer(1), signer(2)], 2, 1_000);
        // Removing either signer would leave 1 signer under a threshold of 2.
        assert_eq!(
            propose_remove_signer_at(signer(1), 0, signer(2)),
            Err(GovernanceError::ThresholdExceedsSigners {
                threshold: 2,
                signer_count: 1,
            })
        );
    }

    #[test]
    fn propose_set_signer_threshold_rejects_zero_and_too_high() {
        init_governed(vec![signer(1), signer(2)], 1, 1_000);
        assert_eq!(
            propose_set_signer_threshold_at(signer(1), 0, 0),
            Err(GovernanceError::ThresholdZero)
        );
        assert_eq!(
            propose_set_signer_threshold_at(signer(1), 0, 3),
            Err(GovernanceError::ThresholdExceedsSigners {
                threshold: 3,
                signer_count: 2,
            })
        );
    }

    // ── approve: idempotent, never applies ──

    #[test]
    fn approve_is_idempotent_and_never_applies() {
        init_governed(vec![signer(1), signer(2)], 2, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(3)).unwrap();
        assert_eq!(approve_proposal_at(signer(1), id), Ok(true));
        assert_eq!(approve_proposal_at(signer(1), id), Ok(false));
        // Still open, and the payload was never applied — signer(3) is not
        // yet in the config even though threshold-1-of-2 has approved.
        assert_eq!(
            state::get_proposal(id).unwrap().status,
            ProposalStatus::Open
        );
        assert!(!state::global_config().signers.contains(&signer(3)));
    }

    #[test]
    fn approve_rejects_non_open_proposal() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        execute_proposal_at(signer(1), 1_000, sentinel_id(), id).unwrap();
        assert_eq!(
            approve_proposal_at(signer(1), id),
            Err(GovernanceError::ProposalNotOpen)
        );
    }

    // ── threshold edges ──

    #[test]
    fn execute_rejects_below_threshold_and_succeeds_exactly_at_threshold() {
        init_governed(vec![signer(1), signer(2), signer(3)], 2, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(4)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), id),
            Err(GovernanceError::ThresholdNotMet)
        );
        approve_proposal_at(signer(2), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), id),
            Ok(())
        );
        assert!(state::global_config().signers.contains(&signer(4)));
    }

    /// `execute_proposal_at` reads `config.approval_threshold` fresh at
    /// execute time, not whatever the threshold was when the proposal was
    /// created or approved — a proposal sitting below the OLD threshold can
    /// become executable purely because a separate, already-executed
    /// proposal lowered the threshold in the meantime.
    #[test]
    fn execute_uses_current_threshold_not_propose_time_threshold() {
        init_governed(vec![signer(1), signer(2), signer(3)], 2, 1_000);
        let target_id = propose_add_signer_at(signer(1), 0, signer(4)).unwrap();
        approve_proposal_at(signer(1), target_id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), target_id),
            Err(GovernanceError::ThresholdNotMet)
        );

        let lower_id = propose_set_signer_threshold_at(signer(1), 0, 1).unwrap();
        approve_proposal_at(signer(1), lower_id).unwrap();
        approve_proposal_at(signer(2), lower_id).unwrap();
        execute_proposal_at(signer(1), 300, sentinel_id(), lower_id).unwrap();
        assert_eq!(state::global_config().approval_threshold, 1);

        // `target_id` still has only its original single approval, but the
        // CURRENT threshold is now 1, so it executes.
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), target_id),
            Ok(())
        );
        assert!(state::global_config().signers.contains(&signer(4)));
    }

    /// The `apply_payload::SetGlobalPolicy` branch re-checks
    /// `check_targets_fit_global_cap` against whatever the registry actually
    /// looks like AT EXECUTE TIME, not what it looked like at propose time —
    /// a target registered (and executed) after the policy proposal was
    /// created, but before it executes, can still block it.
    #[test]
    fn execute_revalidates_global_policy_against_current_registry() {
        init_governed(vec![signer(1)], 1, 1_000);
        // Passes at propose time: no targets exist yet.
        let policy_id =
            propose_set_global_policy_at(signer(1), 0, global_policy_args(500)).unwrap();
        approve_proposal_at(signer(1), policy_id).unwrap();

        // Registered and executed before the policy proposal executes, with
        // a daily cap that fits the ORIGINAL 1_000 global cap but not the
        // pending proposal's 500.
        let mut args = target_args(target_principal(1));
        args.funding_policy = funding_policy_args(1, 10, 800);
        let register_id = propose_register_target_at(signer(1), 50, sentinel_id(), args).unwrap();
        approve_proposal_at(signer(1), register_id).unwrap();
        execute_proposal_at(signer(1), 50 + 100, sentinel_id(), register_id).unwrap();

        assert_eq!(
            execute_proposal_at(signer(1), 200, sentinel_id(), policy_id),
            Err(GovernanceError::GlobalCapBelowExistingTargetCap {
                target: target_principal(1)
            })
        );
        // Not applied: proposal still Open, global cap unchanged.
        assert_eq!(
            state::get_proposal(policy_id).unwrap().status,
            ProposalStatus::Open
        );
        assert_eq!(
            state::global_config()
                .global_policy
                .global_daily_cap_cycles(),
            1_000
        );
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    /// Same execute-time-drift shape as
    /// `execute_revalidates_global_policy_against_current_registry`, against
    /// an unresolved ordinary `FundingOperation`'s own snapshotted cap
    /// instead of a registered target's live cap.
    #[test]
    fn execute_revalidates_global_policy_against_unresolved_operation_cap() {
        init_governed(vec![signer(1)], 1, 1_000);
        // Passes at propose time: no unresolved operations (or targets)
        // exist yet.
        let policy_id = propose_set_global_policy_at(signer(1), 0, global_policy_args(50)).unwrap();
        approve_proposal_at(signer(1), policy_id).unwrap();

        // A target (registered cap 10, well under 50) and an unresolved
        // operation for it (snapshotted cap 100, from
        // `open_in_flight_operation`) both appear before the policy
        // proposal executes — the operation's own cap does not fit.
        let mut args = target_args(target_principal(1));
        args.funding_policy = funding_policy_args(1, 10, 10);
        let register_id = propose_register_target_at(signer(1), 0, sentinel_id(), args).unwrap();
        approve_proposal_at(signer(1), register_id).unwrap();
        execute_proposal_at(signer(1), 100, sentinel_id(), register_id).unwrap();
        let global = state::global_config().global_policy;
        let op_id = open_in_flight_operation(target_principal(1), &global, 100);

        assert_eq!(
            execute_proposal_at(signer(1), 200, sentinel_id(), policy_id),
            Err(GovernanceError::GlobalCapBelowUnresolvedOperationCap {
                operation_id: op_id
            })
        );
        assert_eq!(
            state::get_proposal(policy_id).unwrap().status,
            ProposalStatus::Open
        );
        assert_eq!(
            state::global_config()
                .global_policy
                .global_daily_cap_cycles(),
            1_000
        );
    }

    /// Same execute-time-drift shape, against the self-recovery ledger's own
    /// live pending reservation instead of an ordinary operation's cap.
    #[test]
    fn execute_revalidates_global_policy_against_self_recovery_pending_reservation() {
        init_governed(vec![signer(1)], 1, 1_000);
        // Passes at propose time: no self-recovery reservation exists yet.
        let args = global_policy_args_with_self_recovery_cap(2_000, 50);
        let policy_id = propose_set_global_policy_at(signer(1), 0, args.clone()).unwrap();
        approve_proposal_at(signer(1), policy_id).unwrap();

        // A self-recovery reservation of 60 appears before the policy
        // proposal executes, exceeding the pending proposal's cap of 50.
        let op_id = open_self_recovery_operation(sentinel_id(), 60, 50);

        assert_eq!(
            execute_proposal_at(signer(1), 200, sentinel_id(), policy_id),
            Err(GovernanceError::SelfRecoveryCapBelowPendingReservation {
                operation_id: op_id
            })
        );
        assert_eq!(
            state::get_proposal(policy_id).unwrap().status,
            ProposalStatus::Open
        );
        assert_eq!(
            state::global_config()
                .global_policy
                .self_recovery_policy()
                .daily_cap_cycles(),
            100
        );
    }

    // ── timelock: per-kind, exact boundary ──

    #[test]
    fn execute_respects_target_registry_timelock() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_register_target_at(
            signer(1),
            0,
            sentinel_id(),
            target_args(target_principal(1)),
        )
        .unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 99, sentinel_id(), id),
            Err(GovernanceError::TimelockNotElapsed)
        );
        assert_eq!(
            execute_proposal_at(signer(1), 100, sentinel_id(), id),
            Ok(())
        );
    }

    #[test]
    fn execute_respects_spend_policy_timelock() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_set_global_policy_at(signer(1), 0, global_policy_args(2_000)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 199, sentinel_id(), id),
            Err(GovernanceError::TimelockNotElapsed)
        );
        assert_eq!(
            execute_proposal_at(signer(1), 200, sentinel_id(), id),
            Ok(())
        );
        assert_eq!(
            state::global_config()
                .global_policy
                .global_daily_cap_cycles(),
            2_000
        );
    }

    #[test]
    fn execute_respects_signer_change_timelock() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 299, sentinel_id(), id),
            Err(GovernanceError::TimelockNotElapsed)
        );
        assert_eq!(
            execute_proposal_at(signer(1), 300, sentinel_id(), id),
            Ok(())
        );
    }

    #[test]
    fn execute_respects_unpause_timelock() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        pause_target_at(signer(1), target_principal(1)).unwrap();
        assert!(state::get_target(target_principal(1)).unwrap().paused());

        let id = propose_unpause_target_at(signer(1), 2_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 2_000 + 399, sentinel_id(), id),
            Err(GovernanceError::TimelockNotElapsed)
        );
        assert_eq!(
            execute_proposal_at(signer(1), 2_000 + 400, sentinel_id(), id),
            Ok(())
        );
        assert!(!state::get_target(target_principal(1)).unwrap().paused());
    }

    /// Governance review Blocker 1's authoritative (execute-time) half: two
    /// `UnpauseTarget` proposals for the same target both pass the
    /// propose-time fast-fail (the target is paused when each is created),
    /// but the first one to execute unpauses the target — so by the time
    /// the second one's timelock has also elapsed, the authoritative check
    /// must re-observe that the target has drifted back to unpaused and
    /// reject it, mutating nothing (the proposal stays `Open`, the target's
    /// `revision` does not move past what the first execution left it at).
    #[test]
    fn execute_unpause_rejects_drift_when_already_unpaused_by_another_proposal() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        pause_target_at(signer(1), target_principal(1)).unwrap();

        let id_a = propose_unpause_target_at(signer(1), 1_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), id_a).unwrap();
        let id_b = propose_unpause_target_at(signer(1), 1_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), id_b).unwrap();

        execute_proposal_at(signer(1), 1_000 + 400, sentinel_id(), id_a).unwrap();
        assert!(!state::get_target(target_principal(1)).unwrap().paused());
        let revision_after_a = state::get_target(target_principal(1)).unwrap().revision();

        assert_eq!(
            execute_proposal_at(signer(1), 1_000 + 400, sentinel_id(), id_b),
            Err(GovernanceError::TargetNotPaused)
        );
        // No partial mutation: `id_b` stays `Open` for a signer to cancel,
        // and the target's revision is exactly what `id_a`'s execution left
        // it at — the rejected attempt wrote nothing.
        assert_eq!(
            state::get_proposal(id_b).unwrap().status,
            ProposalStatus::Open
        );
        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            revision_after_a
        );
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    // ── replay execute / cancel ──

    #[test]
    fn execute_is_exact_once() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        execute_proposal_at(signer(1), 1_000, sentinel_id(), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), id),
            Err(GovernanceError::ProposalNotOpen)
        );
        // Only one copy of signer(2), not two.
        assert_eq!(
            state::global_config()
                .signers
                .iter()
                .filter(|s| **s == signer(2))
                .count(),
            1
        );
    }

    #[test]
    fn cancel_is_exact_once_and_blocks_execute() {
        init_governed(vec![signer(1)], 1, 1_000);
        let id = propose_add_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        cancel_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            cancel_proposal_at(signer(1), id),
            Err(GovernanceError::ProposalNotOpen)
        );
        assert_eq!(
            execute_proposal_at(signer(1), 1_000, sentinel_id(), id),
            Err(GovernanceError::ProposalNotOpen)
        );
        assert!(!state::global_config().signers.contains(&signer(2)));
    }

    // ── signer removal cascades to open proposals ──

    #[test]
    fn remove_signer_cancels_other_open_proposals_it_approved() {
        // 3 signers so removing one still leaves threshold(2) <= remaining(2)
        // satisfiable — the same removal is impossible with only 2 signers.
        init_governed(vec![signer(1), signer(2), signer(3)], 2, 1_000);
        // A different, unrelated proposal that signer(2) has approved.
        let unrelated = propose_set_signer_threshold_at(signer(1), 0, 1).unwrap();
        approve_proposal_at(signer(2), unrelated).unwrap();
        assert_eq!(
            state::get_proposal(unrelated).unwrap().status,
            ProposalStatus::Open
        );

        let remove_id = propose_remove_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), remove_id).unwrap();
        approve_proposal_at(signer(3), remove_id).unwrap();
        execute_proposal_at(signer(1), 300, sentinel_id(), remove_id).unwrap();

        assert_eq!(
            state::get_proposal(unrelated).unwrap().status,
            ProposalStatus::Cancelled
        );
        assert_eq!(state::global_config().signers, vec![signer(1), signer(3)]);
    }

    #[test]
    fn remove_signer_does_not_cancel_proposals_without_that_approval() {
        init_governed(vec![signer(1), signer(2), signer(3)], 2, 1_000);
        let untouched = propose_set_signer_threshold_at(signer(1), 0, 1).unwrap();
        // Only signer(1) approved `untouched` — signer(2) never did.
        approve_proposal_at(signer(1), untouched).unwrap();

        let remove_id = propose_remove_signer_at(signer(1), 0, signer(2)).unwrap();
        approve_proposal_at(signer(1), remove_id).unwrap();
        approve_proposal_at(signer(3), remove_id).unwrap();
        execute_proposal_at(signer(1), 300, sentinel_id(), remove_id).unwrap();
        assert_eq!(
            state::get_proposal(untouched).unwrap().status,
            ProposalStatus::Open
        );
    }

    // ── revision increments ──

    #[test]
    fn revision_increments_through_register_update_pause_unpause() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            1
        );

        let patch_id = propose_update_target_at(
            signer(1),
            2_000,
            target_principal(1),
            TargetPatch {
                display_name: Some("renamed".to_string()),
                project: None,
                environment: None,
                criticality: None,
                observation_mode: None,
                tags: None,
                funding_policy: None,
                enabled: None,
                auto_topup: None,
            },
        )
        .unwrap();
        approve_proposal_at(signer(1), patch_id).unwrap();
        execute_proposal_at(signer(1), 2_000 + 100, sentinel_id(), patch_id).unwrap();
        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            2
        );
        assert_eq!(
            state::get_target(target_principal(1))
                .unwrap()
                .display_name(),
            "renamed"
        );

        pause_target_at(signer(1), target_principal(1)).unwrap();
        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            3
        );

        let unpause_id = propose_unpause_target_at(signer(1), 5_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), unpause_id).unwrap();
        execute_proposal_at(signer(1), 5_000 + 400, sentinel_id(), unpause_id).unwrap();
        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            4
        );
    }

    // ── edit/remove during an in-flight funding snapshot ──

    /// Simulates an unresolved `FundingOperation` for `target` without going
    /// through the funding module (out of scope for this task — Task 4/5
    /// own it): directly inserts the operation plus its matching per-target
    /// and global reservations, exactly the invariant
    /// `state::validate_whole_state` requires.
    fn open_in_flight_operation(target: Principal, global: &GlobalPolicy, now: u64) -> u64 {
        let op_id = 9_001;
        let funding_policy =
            TargetFundingPolicy::validate(&funding_policy_args(1, 10, 100), global).unwrap();
        let rail_arguments = FundingRailArguments::Cycles(CyclesWithdrawSnapshot {
            destination: target,
            from_subaccount: None,
            amount_cycles: 10,
            fee_cycles: 0,
            created_at_time_ns: now * 1_000_000_000,
        });
        let op = FundingOperation::open(
            op_id,
            target,
            1,
            funding_policy,
            FundingTrigger::LowBalanceAutoTopup,
            rail_arguments,
            10,
            now,
        )
        .unwrap();
        state::insert_operation(op).unwrap();
        let target_reservation = TargetReservationState::new()
            .reserve(op_id, 10, now, 86_400, 100)
            .unwrap();
        state::set_target_reservation(target, target_reservation);
        let global_reservation = GlobalRollingSpendState::new()
            .reserve(op_id, 10, now, 86_400, 1_000)
            .unwrap();
        state::set_global_rolling_spend(global_reservation);
        let source_reservation = state::get_source_reserve()
            .refresh(1_000_000, 0, now)
            .unwrap()
            .reserve_ordinary(op_id, 10, 0, now, 86_400)
            .unwrap();
        state::set_source_reserve(source_reservation);
        op_id
    }

    /// Simulates a live self-recovery pending reservation without going
    /// through the funding module (out of scope for this task): inserts a
    /// `SelfRecovery`-triggered `FundingOperation` targeting `sentinel_id`
    /// plus the matching `SELF_RECOVERY` ledger entry, exactly the invariant
    /// `state::validate_whole_state`'s self-recovery checks require.
    fn open_self_recovery_operation(sentinel_id: Principal, amount: u128, now: u64) -> u64 {
        let op_id = 9_501;
        let global = state::global_config().global_policy;
        let funding_policy =
            TargetFundingPolicy::validate(&funding_policy_args(1, 10, 100), &global).unwrap();
        let rail_arguments = FundingRailArguments::Cycles(CyclesWithdrawSnapshot {
            destination: sentinel_id,
            from_subaccount: None,
            amount_cycles: amount,
            fee_cycles: 0,
            created_at_time_ns: now * 1_000_000_000,
        });
        let op = FundingOperation::open(
            op_id,
            sentinel_id,
            0,
            funding_policy,
            FundingTrigger::SelfRecovery,
            rail_arguments,
            amount,
            now,
        )
        .unwrap();
        state::insert_operation(op).unwrap();
        let cap = global.self_recovery_policy().daily_cap_cycles();
        let self_recovery = state::get_self_recovery_state()
            .begin(op_id, amount, now, 86_400, cap)
            .unwrap();
        state::set_self_recovery_state(self_recovery);
        op_id
    }

    #[test]
    fn remove_target_fails_closed_while_operation_in_flight() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        let global = state::global_config().global_policy;
        open_in_flight_operation(target_principal(1), &global, 5_000);

        let id = propose_remove_target_at(signer(1), 5_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        assert_eq!(
            execute_proposal_at(signer(1), 5_000 + 100, sentinel_id(), id),
            Err(GovernanceError::RemoveTargetBlocked(
                RemoveTargetError::UnresolvedOperationExists
            ))
        );
        // Not applied: the proposal is still Open and the target is still there.
        assert_eq!(
            state::get_proposal(id).unwrap().status,
            ProposalStatus::Open
        );
        assert!(state::get_target(target_principal(1)).is_some());
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    #[test]
    fn update_target_succeeds_and_leaves_in_flight_snapshot_immutable() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        let global = state::global_config().global_policy;
        let op_id = open_in_flight_operation(target_principal(1), &global, 5_000);
        let snapshot_before = state::get_operation(op_id).unwrap();

        let id = propose_update_target_at(
            signer(1),
            5_000,
            target_principal(1),
            TargetPatch {
                display_name: Some("renamed".to_string()),
                project: None,
                environment: None,
                criticality: None,
                observation_mode: None,
                tags: None,
                funding_policy: None,
                enabled: None,
                auto_topup: None,
            },
        )
        .unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        execute_proposal_at(signer(1), 5_000 + 100, sentinel_id(), id).unwrap();

        assert_eq!(
            state::get_target(target_principal(1)).unwrap().revision(),
            2
        );
        // The FundingOperation's own immutable snapshot never changed.
        assert_eq!(state::get_operation(op_id).unwrap(), snapshot_before);
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    // ── immediate pause (single signer, no proposal) ──

    #[test]
    fn pause_target_is_immediate_and_single_signer() {
        init_governed(vec![signer(1), signer(2)], 2, 1_000);
        register_target(&[signer(1), signer(2)], target_principal(1), 0);
        assert_eq!(pause_target_at(signer(1), target_principal(1)), Ok(()));
        assert!(state::get_target(target_principal(1)).unwrap().paused());
    }

    #[test]
    fn pause_target_rejects_missing_target() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            pause_target_at(signer(1), target_principal(9)),
            Err(GovernanceError::TargetNotFound)
        );
    }

    // ── whole-state validation after ordinary mutations ──

    #[test]
    fn whole_state_validates_after_register_and_remove() {
        init_governed(vec![signer(1)], 1, 1_000);
        register_target(&[signer(1)], target_principal(1), 0);
        state::validate_whole_state(sentinel_id()).unwrap();

        let id = propose_remove_target_at(signer(1), 5_000, target_principal(1)).unwrap();
        approve_proposal_at(signer(1), id).unwrap();
        execute_proposal_at(signer(1), 5_000 + 100, sentinel_id(), id).unwrap();
        assert!(state::get_target(target_principal(1)).is_none());
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    #[test]
    fn whole_state_validates_after_signer_and_policy_changes() {
        init_governed(vec![signer(1), signer(2)], 2, 1_000);
        let add_id = propose_add_signer_at(signer(1), 0, signer(3)).unwrap();
        approve_proposal_at(signer(1), add_id).unwrap();
        approve_proposal_at(signer(2), add_id).unwrap();
        execute_proposal_at(signer(1), 300, sentinel_id(), add_id).unwrap();
        state::validate_whole_state(sentinel_id()).unwrap();

        let policy_id =
            propose_set_global_policy_at(signer(1), 300, global_policy_args(5_000)).unwrap();
        approve_proposal_at(signer(1), policy_id).unwrap();
        approve_proposal_at(signer(2), policy_id).unwrap();
        execute_proposal_at(signer(1), 300 + 200, sentinel_id(), policy_id).unwrap();
        state::validate_whole_state(sentinel_id()).unwrap();
    }

    // ── alarm acknowledgement (Task 2B) ──

    #[test]
    fn acknowledge_alarm_succeeds_for_signer_and_is_idempotent() {
        init_governed(vec![signer(1), signer(2)], 1, 1_000);
        let id = alarms::raise_at(Some(target_principal(1)), AlarmKind::LowBalance, 0).unwrap();
        assert_eq!(acknowledge_alarm_at(signer(1), 100, id), Ok(true));
        // Replay by the same signer, and by a different signer, is a no-op —
        // `state::alarms::acknowledge_at`'s idempotency, reached through the
        // signer-gated wrapper.
        assert_eq!(acknowledge_alarm_at(signer(1), 200, id), Ok(false));
        assert_eq!(acknowledge_alarm_at(signer(2), 200, id), Ok(false));
        assert_eq!(
            state::get_alarm(id).unwrap().acknowledged_at_secs,
            Some(100)
        );
    }

    #[test]
    fn acknowledge_alarm_rejects_missing_and_resolved() {
        init_governed(vec![signer(1)], 1, 1_000);
        assert_eq!(
            acknowledge_alarm_at(signer(1), 0, 9_999),
            Err(GovernanceError::Alarm(alarms::AlarmError::AlarmNotFound))
        );

        let id = alarms::raise_at(Some(target_principal(1)), AlarmKind::LowBalance, 0).unwrap();
        alarms::resolve_at(Some(target_principal(1)), AlarmKind::LowBalance, 50).unwrap();
        assert_eq!(
            acknowledge_alarm_at(signer(1), 100, id),
            Err(GovernanceError::Alarm(
                alarms::AlarmError::AlarmAlreadyResolved
            ))
        );
    }

    /// End-to-end alarm lifecycle through the signer-gated entry point,
    /// exercising raise -> acknowledge -> auto-resolve and confirming the
    /// accepted whole-state validator still passes after each step.
    #[test]
    fn whole_state_validates_after_alarm_lifecycle() {
        init_governed(vec![signer(1)], 1, 1_000);
        state::validate_whole_state(sentinel_id()).unwrap();

        let id = alarms::raise_at(Some(target_principal(1)), AlarmKind::LowBalance, 0).unwrap();
        state::validate_whole_state(sentinel_id()).unwrap();

        assert_eq!(acknowledge_alarm_at(signer(1), 100, id), Ok(true));
        state::validate_whole_state(sentinel_id()).unwrap();

        assert_eq!(
            alarms::resolve_at(Some(target_principal(1)), AlarmKind::LowBalance, 200),
            Some(id)
        );
        state::validate_whole_state(sentinel_id()).unwrap();
    }
}
