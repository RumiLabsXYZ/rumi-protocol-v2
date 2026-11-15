//! Exact protocol-funded compensation for a legacy SP chain burn whose
//! original dispatch cannot be attributed to one ledger block. This rail is
//! independent of writedown/debt settlement and never treats missing history
//! as proof that the original burn did not happen.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::Memo;
use std::cell::RefCell;
use std::collections::BTreeSet;

use crate::state::{
    mutate_state, read_state, LegacySpBurnCompensationIdentity, StoredLegacySpBurnCompensation,
};
use crate::ProtocolError;

thread_local! {
    static IN_FLIGHT: RefCell<BTreeSet<u64>> = RefCell::new(BTreeSet::new());
}

struct DispatchGuard(u64);
impl DispatchGuard {
    fn acquire(vault_id: u64) -> Result<Self, ProtocolError> {
        if IN_FLIGHT.with(|rows| rows.borrow_mut().insert(vault_id)) {
            Ok(Self(vault_id))
        } else {
            Err(ProtocolError::AlreadyProcessing)
        }
    }
}
impl Drop for DispatchGuard {
    fn drop(&mut self) {
        IN_FLIGHT.with(|rows| {
            rows.borrow_mut().remove(&self.0);
        });
    }
}

fn authorize_pool(caller: Principal) -> Result<(), ProtocolError> {
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    if !read_state(|state| state.stability_pool_canister == Some(caller)) {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".into(),
        ));
    }
    Ok(())
}

fn prepare_in_state(
    state: &mut crate::state::State,
    vault_id: u64,
    ledger: Principal,
    pool: Principal,
    protocol: Principal,
    amount_e8s: u64,
    now_ns: u64,
) -> Result<LegacySpBurnCompensationIdentity, ProtocolError> {
    if amount_e8s == 0
        || state.icusd_ledger_principal != ledger
        || state.stability_pool_canister != Some(pool)
    {
        return Err(ProtocolError::GenericError(
            "legacy compensation source differs from configured icUSD ledger or pool".into(),
        ));
    }
    if let Some(saved) = state.legacy_sp_burn_compensations_by_vault.get(&vault_id) {
        return if saved.identity.ledger == ledger
            && saved.identity.amount_e8s == amount_e8s
            && saved.identity.destination
                == (Account {
                    owner: pool,
                    subaccount: None,
                })
            && saved.identity.source
                == (Account {
                    owner: protocol,
                    subaccount: None,
                })
        {
            Ok(saved.identity.clone())
        } else {
            Err(ProtocolError::GenericError(
                "legacy compensation vault already has a conflicting exact identity".into(),
            ))
        };
    }
    let nonce = state.next_op_nonce_at(now_ns);
    let identity = LegacySpBurnCompensationIdentity {
        vault_id,
        op_nonce: nonce,
        ledger,
        source: Account {
            owner: protocol,
            subaccount: None,
        },
        destination: Account {
            owner: pool,
            subaccount: None,
        },
        amount_e8s,
        memo: crate::management::nonce_to_memo(nonce)
            .0
            .as_slice()
            .to_vec(),
        created_at_time_ns: crate::management::nonce_to_created_at_time(nonce),
    };
    state.legacy_sp_burn_compensations_by_vault.insert(
        vault_id,
        StoredLegacySpBurnCompensation {
            identity: identity.clone(),
            dispatch_attempted: false,
            block_index: None,
        },
    );
    Ok(identity)
}

fn mark_dispatch_attempted(
    state: &mut crate::state::State,
    expected: &StoredLegacySpBurnCompensation,
) -> bool {
    let Some(current) = state
        .legacy_sp_burn_compensations_by_vault
        .get_mut(&expected.identity.vault_id)
    else {
        return false;
    };
    if current != expected || current.dispatch_attempted || current.block_index.is_some() {
        return false;
    }
    current.dispatch_attempted = true;
    true
}

fn rearm_bad_fee_no_effect(
    state: &mut crate::state::State,
    expected: &StoredLegacySpBurnCompensation,
    error: &icrc_ledger_types::icrc1::transfer::TransferError,
) -> bool {
    if !matches!(
        error,
        icrc_ledger_types::icrc1::transfer::TransferError::BadFee { .. }
    ) {
        return false;
    }
    let Some(current) = state
        .legacy_sp_burn_compensations_by_vault
        .get_mut(&expected.identity.vault_id)
    else {
        return false;
    };
    if current != expected || !current.dispatch_attempted || current.block_index.is_some() {
        return false;
    }
    current.dispatch_attempted = false;
    true
}

fn record_verified_block(
    state: &mut crate::state::State,
    expected: &StoredLegacySpBurnCompensation,
    block_index: u64,
) -> bool {
    let Some(current) = state
        .legacy_sp_burn_compensations_by_vault
        .get_mut(&expected.identity.vault_id)
    else {
        return false;
    };
    if current.identity != expected.identity
        || !current.dispatch_attempted
        || current
            .block_index
            .is_some_and(|saved| saved != block_index)
    {
        return false;
    }
    current.block_index = Some(block_index);
    true
}

/// Prepare, but do not dispatch, one immutable funding identity. The only
/// amount and ledger are read from the registered SP's exact legacy intent;
/// the caller cannot select a recipient or amount.
pub async fn prepare(vault_id: u64) -> Result<LegacySpBurnCompensationIdentity, ProtocolError> {
    let caller = ic_cdk::api::caller();
    authorize_pool(caller)?;
    let (ledger, amount_e8s) = read_legacy_source(caller, vault_id).await?;
    if amount_e8s == 0 {
        return Err(ProtocolError::GenericError(
            "legacy chain burn has no positive compensation amount".into(),
        ));
    }
    let (expected_ledger, configured_pool) =
        read_state(|state| (state.icusd_ledger_principal, state.stability_pool_canister));
    if ledger != expected_ledger || configured_pool != Some(caller) {
        return Err(ProtocolError::GenericError(
            "legacy compensation source differs from configured icUSD ledger or pool".into(),
        ));
    }
    if let Some(saved) = read_state(|state| {
        state
            .legacy_sp_burn_compensations_by_vault
            .get(&vault_id)
            .cloned()
    }) {
        if saved.identity.ledger == ledger
            && saved.identity.amount_e8s == amount_e8s
            && saved.identity.destination
                == (Account {
                    owner: caller,
                    subaccount: None,
                })
            && saved.identity.source
                == (Account {
                    owner: ic_cdk::api::id(),
                    subaccount: None,
                })
        {
            return Ok(saved.identity);
        }
        return Err(ProtocolError::GenericError(
            "legacy compensation vault already has a conflicting exact identity".into(),
        ));
    }

    let identity = mutate_state(|state| {
        prepare_in_state(
            state,
            vault_id,
            ledger,
            caller,
            ic_cdk::api::id(),
            amount_e8s,
            ic_cdk::api::time(),
        )
    })?;
    Ok(identity)
}

/// Make the one protocol-funded mint attempt using the saved identity. The
/// attempted fence is durable before the ledger await. Any unresolved result
/// is left for positive direct-receipt reconciliation and is never resent.
pub async fn execute(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    authorize_pool(caller)?;
    let _guard = DispatchGuard::acquire(vault_id)?;
    let mut saved = read_state(|state| {
        state
            .legacy_sp_burn_compensations_by_vault
            .get(&vault_id)
            .cloned()
    })
    .ok_or_else(|| ProtocolError::GenericError("legacy compensation was not prepared".into()))?;
    if saved.identity.destination
        != (Account {
            owner: caller,
            subaccount: None,
        })
    {
        return Err(ProtocolError::GenericError(
            "legacy compensation destination no longer matches the registered pool".into(),
        ));
    }
    if let Some(block_index) = saved.block_index {
        return Ok(block_index);
    }
    if saved.dispatch_attempted {
        return Err(ProtocolError::GenericError(
            "legacy compensation dispatch is unresolved; only positive receipt reconciliation is allowed".into(),
        ));
    }

    let minting: Result<(Option<Account>,), _> =
        ic_cdk::call(saved.identity.ledger, "icrc1_minting_account", ()).await;
    let (minting,) = minting.map_err(|(code, message)| {
        ProtocolError::GenericError(format!(
            "could not verify compensation minting account ({code:?}): {message}"
        ))
    })?;
    if minting.as_ref() != Some(&saved.identity.source) {
        return Err(ProtocolError::GenericError(
            "configured backend account is not the icUSD minting account".into(),
        ));
    }

    let marked = mutate_state(|state| {
        if state.stability_pool_canister != Some(caller)
            || state.icusd_ledger_principal != saved.identity.ledger
        {
            return false;
        }
        let expected = saved.clone();
        let marked = mark_dispatch_attempted(state, &expected);
        if marked {
            saved.dispatch_attempted = true;
        }
        marked
    });
    if !marked {
        return Err(ProtocolError::GenericError(
            "legacy compensation changed before its one funding attempt".into(),
        ));
    }

    let transfer_result = crate::management::transfer_idempotent(
        saved.identity.ledger,
        None,
        saved.identity.destination.clone(),
        u128::from(saved.identity.amount_e8s),
        saved.identity.op_nonce,
        Some(Memo::from(saved.identity.memo.clone())),
    )
    .await;
    let block_index = match transfer_result {
        Ok(block_index) => block_index,
        Err(icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee }) => {
            let rearmed = mutate_state(|state| {
                rearm_bad_fee_no_effect(
                    state,
                    &saved,
                    &icrc_ledger_types::icrc1::transfer::TransferError::BadFee {
                        expected_fee: expected_fee.clone(),
                    },
                )
            });
            let disposition = if rearmed {
                "typed BadFee confirmed no transfer; the same saved identity is rearmed"
            } else {
                "typed BadFee received but the saved outbox changed; dispatch remains held"
            };
            return Err(ProtocolError::GenericError(format!(
                "legacy compensation {disposition}; expected fee {expected_fee}"
            )));
        }
        Err(error) => {
            return Err(ProtocolError::GenericError(format!(
                "legacy compensation funding remains held after one attempt: {error:?}"
            )))
        }
    };

    let block = crate::icrc3_proof::fetch_icrc3_block_direct(saved.identity.ledger, block_index)
        .await
        .map_err(|reason| {
            ProtocolError::GenericError(format!(
                "legacy compensation receipt is not directly provable: {reason}"
            ))
        })?;
    crate::icrc3_proof::validate_icrc3_transfer_block(
        &block,
        None,
        saved.identity.destination.clone(),
        saved.identity.amount_e8s,
        Some(&saved.identity.memo),
        Some(saved.identity.created_at_time_ns),
    )
    .map_err(|reason| {
        ProtocolError::GenericError(format!(
            "legacy compensation receipt differs from exact saved identity: {reason}"
        ))
    })?;

    let recorded = mutate_state(|state| record_verified_block(state, &saved, block_index));
    if !recorded {
        return Err(ProtocolError::GenericError(
            "legacy compensation receipt verified but its saved outbox changed".into(),
        ));
    }
    Ok(block_index)
}

async fn read_legacy_source(
    pool: Principal,
    vault_id: u64,
) -> Result<(Principal, u64), ProtocolError> {
    let result: Result<(Option<(Principal, u64)>,), _> =
        ic_cdk::call(pool, "legacy_chain_burn_compensation_source", (vault_id,)).await;
    let (source,) = result.map_err(|(code, message)| {
        ProtocolError::GenericError(format!(
            "could not verify the saved legacy SP intent ({code:?}): {message}"
        ))
    })?;
    source.ok_or_else(|| {
        ProtocolError::GenericError(
            "pool has no eligible legacy ambiguous chain intent for compensation".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        mark_dispatch_attempted, prepare_in_state, rearm_bad_fee_no_effect, record_verified_block,
    };
    use crate::state::State;
    use candid::Principal;
    use icrc_ledger_types::icrc1::transfer::TransferError;

    #[test]
    fn compensation_outbox_is_exact_idempotent_and_ambiguous_dispatch_is_one_shot() {
        let ledger = Principal::from_slice(&[1]);
        let pool = Principal::from_slice(&[2]);
        let protocol = Principal::from_slice(&[3]);
        let mut state = State::default();
        state.icusd_ledger_principal = ledger;
        state.stability_pool_canister = Some(pool);

        let first = prepare_in_state(&mut state, 77, ledger, pool, protocol, 1_000, 500)
            .expect("prepare exact compensation");
        let retry = prepare_in_state(&mut state, 77, ledger, pool, protocol, 1_000, 900)
            .expect("retry returns exact identity");
        assert_eq!(retry, first);
        assert_eq!(first.source.owner, protocol);
        assert_eq!(first.destination.owner, pool);
        assert_eq!(first.amount_e8s, 1_000);
        assert_eq!(first.memo, first.op_nonce.to_be_bytes());
        assert_eq!(
            first.created_at_time_ns,
            crate::management::nonce_to_created_at_time(first.op_nonce)
        );
        assert!(prepare_in_state(&mut state, 77, ledger, pool, protocol, 1_001, 901).is_err());
        assert!(prepare_in_state(
            &mut state,
            77,
            Principal::from_slice(&[9]),
            pool,
            protocol,
            1_000,
            901,
        )
        .is_err());

        let saved = state.legacy_sp_burn_compensations_by_vault[&77].clone();
        assert!(mark_dispatch_attempted(&mut state, &saved));
        assert!(!mark_dispatch_attempted(&mut state, &saved));
        let ambiguous = state.legacy_sp_burn_compensations_by_vault[&77].clone();
        assert!(ambiguous.dispatch_attempted);
        assert_eq!(ambiguous.block_index, None, "ambiguous result stays held");
        assert!(!mark_dispatch_attempted(&mut state, &ambiguous));

        assert!(rearm_bad_fee_no_effect(
            &mut state,
            &ambiguous,
            &TransferError::BadFee {
                expected_fee: candid::Nat::from(10u64),
            },
        ));
        let retryable = state.legacy_sp_burn_compensations_by_vault[&77].clone();
        assert!(!retryable.dispatch_attempted);
        assert_eq!(
            retryable.identity, first,
            "BadFee keeps the same exact tuple"
        );
        assert!(mark_dispatch_attempted(&mut state, &retryable));
        let ambiguous_retry = state.legacy_sp_burn_compensations_by_vault[&77].clone();
        assert!(ambiguous_retry.dispatch_attempted);
        assert!(!rearm_bad_fee_no_effect(
            &mut state,
            &ambiguous_retry,
            &TransferError::GenericError {
                error_code: candid::Nat::from(99u64),
                message: "ambiguous ledger outcome".into(),
            },
        ));
        assert_eq!(
            state.legacy_sp_burn_compensations_by_vault[&77], ambiguous_retry,
            "ambiguous outcomes remain held"
        );

        assert!(record_verified_block(&mut state, &ambiguous_retry, 44));
        assert_eq!(
            state.legacy_sp_burn_compensations_by_vault[&77].block_index,
            Some(44)
        );
        assert!(!record_verified_block(&mut state, &ambiguous_retry, 45));
    }
}
