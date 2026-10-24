//! Bounded ckSOL browser deposit relay.
//!
//! The ckSOL minter requires an inter-canister call with cycles.  Ingress
//! callers cannot attach those cycles to a call made by the backend, so this
//! module provides the fixed minter transport for caller-funded processing. The
//! accepted deposit id is retained until the minter reports a terminal status;
//! transport uncertainty is fail-closed and never retried automatically.

use candid::{CandidType, Deserialize, Nat, Principal};
use crate::state::{mutate_state, read_state, CkSolRelayAttempt, CkSolRelayStatus};

pub const CKSOL_LEDGER_CANISTER: &str = "ls5lp-lqaaa-aaaar-qb5oa-cai";
pub const CKSOL_MINTER_CANISTER: &str = "lh22c-kyaaa-aaaar-qb5nq-cai";
pub const CKSOL_DECIMALS: u8 = 9;
pub const CKSOL_RELAY_CALL_CYCLES: u128 = 1_000_000_000_000;
pub const CKSOL_RELAY_FEE_CYCLES: u128 = 45_000_000_000;
pub const CKSOL_RELAY_CANISTER_RESERVE: u128 = 10_000_000_000_000;
pub const CKSOL_RELAY_BUDGET_CAP: u64 = 100_000_000_000_000;
pub const CKSOL_RELAY_OWNER_CAP: usize = 1_000;
pub const CKSOL_REJECTED_RETRY_COOLDOWN_NS: u64 = 60 * 1_000_000_000;

/// Convert an exact native-token minimum between decimal precisions without
/// truncating a safety floor. Down-conversions round up, while up-conversions
/// are exact powers of ten and checked for overflow.
pub fn convert_minimum_deposit(
    amount: u64,
    from_decimals: u8,
    to_decimals: u8,
) -> Result<u64, String> {
    if from_decimals == to_decimals {
        return Ok(amount);
    }
    if from_decimals > to_decimals {
        let factor = 10u64
            .checked_pow(u32::from(from_decimals - to_decimals))
            .ok_or_else(|| "decimal conversion factor overflow".to_string())?;
        return Ok(amount
            .checked_add(factor - 1)
            .ok_or_else(|| "minimum collateral deposit conversion overflow".to_string())?
            / factor);
    }
    let factor = 10u64
        .checked_pow(u32::from(to_decimals - from_decimals))
        .ok_or_else(|| "decimal conversion factor overflow".to_string())?;
    amount
        .checked_mul(factor)
        .ok_or_else(|| "minimum collateral deposit conversion overflow".to_string())
}

pub fn ledger_principal() -> Principal {
    Principal::from_text(CKSOL_LEDGER_CANISTER).expect("ckSOL ledger principal is valid")
}

fn minter_principal() -> Principal {
    Principal::from_text(CKSOL_MINTER_CANISTER).expect("ckSOL minter principal is valid")
}

#[derive(CandidType, Deserialize, Debug)]
struct DepositSolArgs {
    owner: Option<Principal>,
    subaccount: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Debug)]
struct InsufficientCyclesError {
    expected: Nat,
    received: Nat,
}

#[derive(CandidType, Deserialize, Debug)]
struct ValueTooSmallError {
    balance: u64,
    minimum_deposit_amount: u64,
}

#[derive(CandidType, Deserialize, Debug)]
enum DepositSolError {
    InsufficientCycles(InsufficientCyclesError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    ValueTooSmall(ValueTooSmallError),
    Quarantined { deposit_id: u64 },
}

#[derive(CandidType, Deserialize, Debug)]
enum DepositSolStatus {
    NotFound,
    Queued {
        sweepable_amount: u64,
    },
    Swept {
        signature: String,
    },
    Finalized {
        signature: String,
    },
    Minted {
        block_index: u64,
        minted_amount: u64,
    },
    Dropped {
        signature: String,
    },
    Quarantined {
        signature: String,
    },
}

fn minter_error_message(error: &DepositSolError) -> String {
    match error {
        DepositSolError::InsufficientCycles(e) => {
            format!(
                "ckSOL minter requires {} cycles (received {})",
                e.expected, e.received
            )
        }
        DepositSolError::TemporarilyUnavailable(message) => {
            format!("ckSOL minter temporarily unavailable: {message}")
        }
        DepositSolError::AlreadyProcessing => "ckSOL minter already processing this account".into(),
        DepositSolError::ValueTooSmall(e) => format!(
            "ckSOL deposit address balance {} is below minimum {}",
            e.balance, e.minimum_deposit_amount
        ),
        DepositSolError::Quarantined { deposit_id } => {
            format!("ckSOL minter quarantined deposit {deposit_id}")
        }
    }
}

fn developer_authorized(caller: Principal, developer: Principal) -> bool {
    caller != Principal::anonymous() && caller == developer
}

fn status_requires_recovery(status: &CkSolRelayStatus) -> bool {
    matches!(status, CkSolRelayStatus::SubmittedOrUnknown)
}

fn is_terminal(status: &DepositSolStatus) -> bool {
    matches!(
        status,
        DepositSolStatus::Minted { .. } | DepositSolStatus::Dropped { .. }
    )
}

fn status_is_unknown(status: &DepositSolStatus) -> bool {
    matches!(
        status,
        DepositSolStatus::NotFound | DepositSolStatus::Quarantined { .. }
    )
}

fn status_description(status: &DepositSolStatus) -> &'static str {
    match status {
        DepositSolStatus::NotFound => "not found",
        DepositSolStatus::Queued { .. } => "queued",
        DepositSolStatus::Swept { .. } => "swept",
        DepositSolStatus::Finalized { .. } => "finalized",
        DepositSolStatus::Minted { .. } => "minted",
        DepositSolStatus::Dropped { .. } => "dropped",
        DepositSolStatus::Quarantined { .. } => "quarantined",
    }
}

pub fn set_budget(caller: Principal, budget: u64) -> Result<(), String> {
    let developer = read_state(|state| state.developer_principal);
    if !developer_authorized(caller, developer) {
        return Err("Only the developer can set the ckSOL relay budget".into());
    }
    if budget != 0 {
        return Err("Public ckSOL sponsorship is disabled; only a zero allowance is permitted".into());
    }
    mutate_state(|state| state.cksol_relay_budget = budget);
    Ok(())
}

pub fn budget() -> u64 { 0 }

pub fn attempt(owner: Principal) -> Option<CkSolRelayAttempt> {
    read_state(|state| state.cksol_relay_attempts.get(&owner).cloned())
}

fn begin_in_flight(owner: Principal) -> Result<(), String> {
    mutate_state(|state| {
        if state.cksol_relay_in_flight.contains(&owner) {
            return Err("ckSOL deposit is already processing for this owner".into());
        }
        if state.cksol_relay_in_flight.len() >= CKSOL_RELAY_OWNER_CAP {
            return Err("ckSOL relay owner cap reached".into());
        }
        state.cksol_relay_in_flight.insert(owner);
        Ok(())
    })
}

fn end_in_flight(owner: Principal) {
    mutate_state(|state| {
        state.cksol_relay_in_flight.remove(&owner);
    });
}

fn retain_unknown(owner: Principal, started_at_ns: u64) {
    mutate_state(|state| {
        state.cksol_relay_attempts.insert(
            owner,
            CkSolRelayAttempt {
                started_at_ns,
                status: CkSolRelayStatus::SubmittedOrUnknown,
            },
        );
    });
}

fn reserve_attempt(
    state: &mut crate::state::State,
    owner: Principal,
    started_at_ns: u64,
) -> Result<(), String> {
    if state.cksol_relay_budget < CKSOL_RELAY_CALL_CYCLES as u64 {
        return Err("ckSOL relay budget is exhausted".into());
    }
    if !state.cksol_relay_attempts.contains_key(&owner) {
        reclaim_expired_rejection(state, started_at_ns);
    }
    if state.cksol_relay_attempts.len() >= CKSOL_RELAY_OWNER_CAP
        && !state.cksol_relay_attempts.contains_key(&owner)
    {
        return Err("ckSOL relay owner cap reached".into());
    }
    // This reservation is intentionally never refunded, including typed
    // minter rejections. It bounds fee spam and survives reply loss.
    state.cksol_relay_budget -= CKSOL_RELAY_CALL_CYCLES as u64;
    state.cksol_relay_attempts.insert(
        owner,
        CkSolRelayAttempt {
            started_at_ns,
            status: CkSolRelayStatus::SubmittedOrUnknown,
        },
    );
    Ok(())
}

/// Reuse a bounded slot only when a typed minter rejection and elapsed
/// cooldown prove there is no accepted upstream deposit. Paid credits are
/// stored separately and are never removed by this pruning policy.
pub(crate) fn reclaim_expired_rejection(state: &mut crate::state::State, now_ns: u64) {
    if state.cksol_relay_attempts.len() >= CKSOL_RELAY_OWNER_CAP {
        // A typed rejection proves that no upstream deposit was accepted. Once
        // its cooldown has elapsed, evict at most the oldest such row to keep
        // the bounded owner map live for new users. Unknown and accepted rows
        // are never evicted here.
        let evict = state
            .cksol_relay_attempts
            .iter()
            .filter_map(|(candidate, row)| {
                (matches!(&row.status, CkSolRelayStatus::Rejected(_))
                    && now_is_after_cooldown(row.started_at_ns, now_ns))
                .then_some((*candidate, row.started_at_ns))
            })
            .min_by_key(|(_, started_at_ns)| *started_at_ns)
            .map(|(candidate, _)| candidate);
        if let Some(candidate) = evict {
            state.cksol_relay_attempts.remove(&candidate);
        }
    }
}

fn now_is_after_cooldown(started_at_ns: u64, now_ns: u64) -> bool {
    now_ns.saturating_sub(started_at_ns) >= CKSOL_REJECTED_RETRY_COOLDOWN_NS
}

fn clear_rejected(
    state: &mut crate::state::State,
    owner: Principal,
    now_ns: u64,
) -> Result<(), String> {
    let Some(row) = state.cksol_relay_attempts.get(&owner) else {
        return Err("no retained ckSOL attempt exists for this owner".into());
    };
    if let CkSolRelayStatus::Rejected(reason) = &row.status {
        let age = now_ns.saturating_sub(row.started_at_ns);
        if age < CKSOL_REJECTED_RETRY_COOLDOWN_NS {
            return Err(format!("ckSOL retry cooldown active: {reason}"));
        }
        state.cksol_relay_attempts.remove(&owner);
        return Ok(());
    }
    Err("only a definitively rejected ckSOL attempt can be cleared directly".into())
}

pub fn bind_observed_id(
    caller: Principal,
    owner: Principal,
    deposit_id: u64,
) -> Result<(), String> {
    if !developer_authorized(caller, read_state(|state| state.developer_principal)) {
        return Err("Only the developer can recover a ckSOL deposit id".into());
    }
    if owner == Principal::anonymous() {
        return Err("Cannot recover an anonymous ckSOL owner".into());
    }
    mutate_state(|state| match state.cksol_relay_attempts.get_mut(&owner) {
        Some(row) if row.status == CkSolRelayStatus::SubmittedOrUnknown => {
            row.status = CkSolRelayStatus::Queued(deposit_id);
            Ok(())
        }
        Some(_) => Err("ckSOL owner does not have an unknown attempt".into()),
        None => Err("no retained ckSOL attempt exists for this owner".into()),
    })
}

/// Start one fixed ckSOL minter deposit for the ingress caller.
pub async fn start(_owner: Principal) -> Result<u64, String> {
    Err("Rumi sponsorship is disabled. Use caller-funded ckSOL processing.".into())
}

pub(crate) async fn call_paid_minter(owner: Principal) -> (crate::cksol_paid::DepositOutcome, u128) {
    use crate::cksol_paid::DepositOutcome;
    let result: Result<(Result<u64, DepositSolError>,), _> =
        ic_cdk::api::call::call_with_payment128(minter_principal(), "deposit_sol", (DepositSolArgs {
            owner: Some(owner), subaccount: None,
        },), CKSOL_RELAY_CALL_CYCLES).await;
    let refunded = ic_cdk::api::call::msg_cycles_refunded128();
    let outcome = match result {
        Ok((Ok(id),)) => DepositOutcome::Queued(id),
        Ok((Err(error),)) => {
            let message = minter_error_message(&error);
            if matches!(error, DepositSolError::Quarantined { .. }) {
                DepositOutcome::Unknown(message)
            } else { DepositOutcome::Rejected(message) }
        }
        Err((code, message)) => DepositOutcome::Unknown(format!("Minter outcome unknown: {code:?} {message}")),
    };
    (outcome, refunded)
}

/// Reclaim a completed owner slot after independently reading the upstream
/// terminal status. The owner can never use this endpoint to discard an
/// unknown, queued, swept, or finalized deposit record.
pub async fn clear(owner: Principal) -> Result<(), String> {
    if owner == Principal::anonymous() {
        return Err("Anonymous callers cannot clear a ckSOL deposit".into());
    }
    let Some(existing) = attempt(owner) else {
        return Err("no retained ckSOL attempt exists for this owner".into());
    };
    if let CkSolRelayStatus::Rejected(_) = existing.status {
        let now = ic_cdk::api::time();
        return mutate_state(|state| clear_rejected(state, owner, now));
    }
    let CkSolRelayStatus::Queued(id) = existing.status else {
        return Err("only a queued ckSOL deposit can be cleared after terminal status".into());
    };
    begin_in_flight(owner)?;
    let status_result: Result<(DepositSolStatus,), _> =
        ic_cdk::call(minter_principal(), "deposit_sol_status", (id,)).await;
    end_in_flight(owner);
    match status_result {
        Ok((status,)) if is_terminal(&status) => {
            mutate_state(|state| {
                state.cksol_relay_attempts.remove(&owner);
            });
            Ok(())
        }
        Ok((status,)) => Err(format!(
            "ckSOL deposit {id} remains {}; retained for reconciliation",
            status_description(&status)
        )),
        Err((code, message)) => Err(format!(
            "ckSOL deposit {id} status probe failed: {code:?} {message}; retained for reconciliation"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_constants_match_live_cksol_contract() {
        assert_eq!(CKSOL_DECIMALS, 9);
        assert_eq!(CKSOL_RELAY_CALL_CYCLES, 1_000_000_000_000);
        assert_eq!(CKSOL_RELAY_FEE_CYCLES, 45_000_000_000);
        assert_eq!(CKSOL_RELAY_BUDGET_CAP, 100_000_000_000_000);
    }

    #[test]
    fn terminal_statuses_are_only_minted_or_dropped() {
        assert!(is_terminal(&DepositSolStatus::Minted {
            block_index: 1,
            minted_amount: 2,
        }));
        assert!(is_terminal(&DepositSolStatus::Dropped {
            signature: "sig".into(),
        }));
        assert!(!is_terminal(&DepositSolStatus::Quarantined {
            signature: "sig".into(),
        }));
        assert!(status_is_unknown(&DepositSolStatus::NotFound));
    }

    #[test]
    fn minimum_conversion_rounds_downscale_up() {
        assert_eq!(convert_minimum_deposit(1_000_000_000, 18, 9).unwrap(), 1);
        assert_eq!(convert_minimum_deposit(1_000_000_001, 18, 9).unwrap(), 2);
        assert_eq!(convert_minimum_deposit(2, 9, 18).unwrap(), 2_000_000_000);
    }

    #[test]
    fn old_empty_snapshot_defaults_relay_state_safely() {
        let old_snapshot: std::collections::BTreeMap<String, u8> =
            std::collections::BTreeMap::new();
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&old_snapshot, &mut bytes).expect("encode old snapshot");
        let state: crate::state::State =
            ciborium::de::from_reader(bytes.as_slice()).expect("decode old snapshot");
        assert_eq!(state.cksol_relay_budget, 0);
        assert!(state.cksol_relay_attempts.is_empty());
        assert!(state.cksol_relay_in_flight.is_empty());
    }

    #[test]
    fn budget_reservation_is_full_cost_and_is_not_refunded() {
        let owner = Principal::self_authenticating(&[1]);
        let mut state = crate::state::State::default();
        state.cksol_relay_budget = CKSOL_RELAY_CALL_CYCLES as u64;
        reserve_attempt(&mut state, owner, 7).expect("first reservation");
        assert_eq!(state.cksol_relay_budget, 0);
        assert_eq!(state.cksol_relay_attempts[&owner].started_at_ns, 7);
        state.cksol_relay_attempts.get_mut(&owner).unwrap().status =
            CkSolRelayStatus::Rejected("typed rejection".into());
        assert_eq!(state.cksol_relay_budget, 0);
    }

    #[test]
    fn owner_cap_rejects_new_owner_but_allows_existing_retry_slot() {
        let existing = Principal::self_authenticating(&[1]);
        let newcomer = Principal::self_authenticating(&[2]);
        let mut state = crate::state::State::default();
        state.cksol_relay_budget = CKSOL_RELAY_BUDGET_CAP;
        for n in 0..CKSOL_RELAY_OWNER_CAP - 1 {
            let owner_bytes = (n as u64).to_le_bytes();
            state.cksol_relay_attempts.insert(
                Principal::self_authenticating(&owner_bytes),
                CkSolRelayAttempt {
                    started_at_ns: 0,
                    status: CkSolRelayStatus::Rejected("retryable".into()),
                },
            );
        }
        state.cksol_relay_attempts.insert(
            existing,
            CkSolRelayAttempt {
                started_at_ns: 0,
                status: CkSolRelayStatus::Rejected("retryable".into()),
            },
        );
        assert!(reserve_attempt(&mut state, newcomer, 1).is_err());
        assert!(reserve_attempt(&mut state, existing, 1).is_ok());
    }

    #[test]
    fn populated_prefeature_snapshot_preserves_old_fields_and_defaults_relay_state() {
        let mut state = crate::state::State::default();
        state.frozen = true;
        state.developer_principal = Principal::self_authenticating(&[7]);
        state.cksol_relay_budget = CKSOL_RELAY_BUDGET_CAP;
        let owner = Principal::self_authenticating(&[8]);
        state.cksol_relay_attempts.insert(
            owner,
            CkSolRelayAttempt {
                started_at_ns: 11,
                status: CkSolRelayStatus::Queued(12),
            },
        );
        state.cksol_relay_in_flight.insert(owner);

        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&state, &mut encoded).expect("encode state");
        let value: ciborium::Value =
            ciborium::de::from_reader(encoded.as_slice()).expect("decode state map");
        let mut entries = match value {
            ciborium::Value::Map(entries) => entries,
            other => panic!("expected state map, got {other:?}"),
        };
        entries.retain(|(key, _)| {
            !matches!(key, ciborium::Value::Text(name)
                if name == "cksol_relay_budget"
                    || name == "cksol_relay_attempts"
                    || name == "cksol_relay_in_flight")
        });
        let mut legacy = Vec::new();
        ciborium::ser::into_writer(&ciborium::Value::Map(entries), &mut legacy)
            .expect("encode prefeature snapshot");
        let restored: crate::state::State =
            ciborium::de::from_reader(legacy.as_slice()).expect("decode prefeature snapshot");
        assert!(restored.frozen);
        assert_eq!(restored.developer_principal, state.developer_principal);
        assert_eq!(restored.cksol_relay_budget, 0);
        assert!(restored.cksol_relay_attempts.is_empty());
        assert!(restored.cksol_relay_in_flight.is_empty());

        let mut current = Vec::new();
        ciborium::ser::into_writer(&state, &mut current).expect("encode current snapshot");
        let roundtrip: crate::state::State =
            ciborium::de::from_reader(current.as_slice()).expect("decode current snapshot");
        assert_eq!(roundtrip.cksol_relay_budget, CKSOL_RELAY_BUDGET_CAP);
        assert_eq!(
            roundtrip.cksol_relay_attempts[&owner].status,
            CkSolRelayStatus::Queued(12)
        );
        assert!(roundtrip.cksol_relay_in_flight.is_empty());
    }

    #[test]
    fn rejected_owner_slot_can_be_reclaimed_after_cooldown() {
        let newcomer = Principal::self_authenticating(&[0x55]);
        let mut state = crate::state::State::default();
        for n in 0..CKSOL_RELAY_OWNER_CAP {
            let owner_bytes = (n as u64 + 1_000).to_le_bytes();
            state.cksol_relay_attempts.insert(
                Principal::self_authenticating(&owner_bytes),
                CkSolRelayAttempt {
                    started_at_ns: 1,
                    status: CkSolRelayStatus::Rejected("value too small".into()),
                },
            );
        }
        let reclaim = *state.cksol_relay_attempts.keys().next().unwrap();
        assert!(reserve_attempt(&mut state, newcomer, 2).is_err());
        clear_rejected(&mut state, reclaim, CKSOL_REJECTED_RETRY_COOLDOWN_NS + 1)
            .expect("cooled-down rejection is safe to clear");
        state.cksol_relay_budget = CKSOL_RELAY_CALL_CYCLES as u64;
        reserve_attempt(&mut state, newcomer, 3).expect("new owner admitted after reclaim");
    }

    #[test]
    fn cap_eviction_reclaims_only_old_rejections() {
        let newcomer = Principal::self_authenticating(&[0x66]);
        let mut rejected = crate::state::State::default();
        rejected.cksol_relay_budget = CKSOL_RELAY_CALL_CYCLES as u64;
        for n in 0..CKSOL_RELAY_OWNER_CAP {
            rejected.cksol_relay_attempts.insert(
                Principal::self_authenticating(&(n as u64 + 20_000).to_le_bytes()),
                CkSolRelayAttempt {
                    started_at_ns: n as u64,
                    status: CkSolRelayStatus::Rejected("typed rejection".into()),
                },
            );
        }
        reserve_attempt(
            &mut rejected,
            newcomer,
            CKSOL_REJECTED_RETRY_COOLDOWN_NS + 1,
        )
        .expect("old rejected row may be evicted");
        assert_eq!(rejected.cksol_relay_attempts.len(), CKSOL_RELAY_OWNER_CAP);

        let mut accepted_or_unknown = crate::state::State::default();
        accepted_or_unknown.cksol_relay_budget = CKSOL_RELAY_CALL_CYCLES as u64;
        for n in 0..CKSOL_RELAY_OWNER_CAP {
            accepted_or_unknown.cksol_relay_attempts.insert(
                Principal::self_authenticating(&(n as u64 + 30_000).to_le_bytes()),
                CkSolRelayAttempt {
                    started_at_ns: 0,
                    status: if n % 2 == 0 {
                        CkSolRelayStatus::SubmittedOrUnknown
                    } else {
                        CkSolRelayStatus::Queued(n as u64)
                    },
                },
            );
        }
        assert!(reserve_attempt(
            &mut accepted_or_unknown,
            newcomer,
            CKSOL_REJECTED_RETRY_COOLDOWN_NS + 1,
        )
        .is_err());
        assert_eq!(
            accepted_or_unknown.cksol_relay_attempts.len(),
            CKSOL_RELAY_OWNER_CAP
        );
    }

    #[test]
    fn anonymous_is_never_developer_and_unknown_blocks_retry() {
        let developer = Principal::self_authenticating(&[9]);
        assert!(!developer_authorized(Principal::anonymous(), developer));
        assert!(developer_authorized(developer, developer));
        assert!(status_requires_recovery(
            &CkSolRelayStatus::SubmittedOrUnknown
        ));
        assert!(!status_requires_recovery(&CkSolRelayStatus::Rejected(
            "retry".into()
        )));
    }
}
