use crate::event::{
    record_claim_liquidity_returns, record_provide_liquidity, record_withdraw_liquidity,
};
use crate::guard::GuardPrincipal;
use crate::logs::INFO;
use crate::management::{mint_icusd, transfer_icp, transfer_icusd_from};
use crate::{mutate_state, read_state, ProtocolError, ICP, ICUSD, MIN_LIQUIDITY_AMOUNT};
use candid::Principal;
use ic_canister_log::log;
use icrc_ledger_types::icrc1::transfer::TransferError;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const MAX_CONCURRENT_LIQUIDITY_OPERATIONS: usize = 100;

thread_local! {
    /// Same-owner liquidity mutations share balances and must stay serialized
    /// for the entire ledger await. Unlike `GuardPrincipal`, this safety lock
    /// has no age-based takeover: an external ledger call may outlast any
    /// fixed lease while still being capable of committing.
    static LIQUIDITY_OPERATIONS: RefCell<HashMap<Principal, u128>> = RefCell::new(HashMap::new());
    static LIQUIDITY_OPERATION_NEXT_TOKEN: Cell<u128> = Cell::new(0);
}

/// A transient owner lock held across provide, withdraw, and claim ledger calls.
/// Token checks make a late Drop unable to release a different lock instance.
#[must_use]
struct LiquidityOperationGuard {
    owner: Principal,
    token: u128,
}

impl LiquidityOperationGuard {
    fn new(owner: Principal) -> Result<Self, ProtocolError> {
        LIQUIDITY_OPERATIONS.with(|active| {
            let mut active = active.borrow_mut();
            if active.contains_key(&owner) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "A liquidity operation for this owner is still in flight".to_string(),
                ));
            }
            if active.len() >= MAX_CONCURRENT_LIQUIDITY_OPERATIONS {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "Too many liquidity operations are in flight".to_string(),
                ));
            }
            let token = LIQUIDITY_OPERATION_NEXT_TOKEN
                .with(|next| {
                    let token = next.get().checked_add(1).ok_or(())?;
                    next.set(token);
                    Ok::<u128, ()>(token)
                })
                .map_err(|()| {
                    ProtocolError::TemporarilyUnavailable(
                        "Liquidity operation lock tokens are exhausted".to_string(),
                    )
                })?;
            active.insert(owner, token);
            Ok(Self { owner, token })
        })
    }
}

impl Drop for LiquidityOperationGuard {
    fn drop(&mut self) {
        LIQUIDITY_OPERATIONS.with(|active| {
            let mut active = active.borrow_mut();
            if active.get(&self.owner) == Some(&self.token) {
                active.remove(&self.owner);
            }
        });
    }
}

pub async fn provide_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "provide_liquidity")?;

    let amount: ICUSD = amount.into();

    if amount < MIN_LIQUIDITY_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64(),
        });
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            log!(INFO, "[provide_liquidity] {caller} provided {amount}",);
            mutate_state(|s| {
                record_provide_liquidity(s, amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            amount.to_u64(),
        )),
    }
}

pub async fn withdraw_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "withdraw_liquidity")?;

    let amount: ICUSD = amount.into();

    if amount < MIN_LIQUIDITY_AMOUNT {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: MIN_LIQUIDITY_AMOUNT.to_u64(),
        });
    }

    let provided_liquidity = read_state(|s| {
        s.liquidity_pool
            .get(&caller)
            .cloned()
    }).ok_or_else(|| ProtocolError::GenericError(
        "You have no provided liquidity to withdraw".to_string()
    ))?;
    if amount > provided_liquidity {
        return Err(ProtocolError::GenericError(format!(
            "cannot withdraw: {amount}, provided: {provided_liquidity}"
        )));
    }

    match mint_icusd(amount, caller).await {
        Ok(block_index) => {
            log!(INFO, "[withdraw_liquidity] {caller} withdrew {amount}",);
            mutate_state(|s| {
                record_withdraw_liquidity(s, amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_error) => Err(ProtocolError::TransferError(transfer_error)),
    }
}

pub async fn claim_liquidity_returns() -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _liquidity_guard = LiquidityOperationGuard::new(caller)?;
    let _guard_principal = GuardPrincipal::new(caller, "claim_liquidity_returns")?;

    let return_amount = read_state(|s| {
        s.liquidity_returns.get(&caller).cloned()
    }).ok_or_else(|| ProtocolError::GenericError(
        "You have no liquidity rewards to claim".to_string()
    ))?;

    match transfer_icp(return_amount, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[claim_liquidity_returns] {caller} claimed {return_amount}",
            );
            mutate_state(|s| {
                record_claim_liquidity_returns(s, return_amount, caller, block_index);
            });
            Ok(block_index)
        }
        Err(transfer_error) => {
            if let TransferError::BadFee { expected_fee } = transfer_error.clone() {
                mutate_state(|s| {
                    let expected_fee: u64 = expected_fee
                        .0
                        .try_into()
                        .expect("failed to convert Nat to u64");
                    s.icp_ledger_fee = ICP::from(expected_fee);
                });
            };
            Err(ProtocolError::TransferError(transfer_error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LiquidityOperationGuard, LIQUIDITY_OPERATIONS, MAX_CONCURRENT_LIQUIDITY_OPERATIONS,
    };
    use candid::Principal;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    #[test]
    fn owner_lock_blocks_all_same_owner_reentry_and_releases_on_drop() {
        let owner = principal(1);
        let other = principal(2);
        let first = LiquidityOperationGuard::new(owner).expect("first owner lock");
        assert!(LiquidityOperationGuard::new(owner).is_err());
        let independent = LiquidityOperationGuard::new(other).expect("independent owner lock");
        drop(independent);
        drop(first);
        assert!(LiquidityOperationGuard::new(owner).is_ok());
    }

    #[test]
    fn late_drop_cannot_clear_a_different_tokenized_owner_lock() {
        let owner = principal(3);
        let old = LiquidityOperationGuard::new(owner).expect("old owner lock");
        let successor_token = old.token.checked_add(1).expect("successor token");
        LIQUIDITY_OPERATIONS.with(|active| {
            active.borrow_mut().insert(owner, successor_token);
        });

        drop(old);

        LIQUIDITY_OPERATIONS.with(|active| {
            assert_eq!(active.borrow().get(&owner), Some(&successor_token));
            active.borrow_mut().remove(&owner);
        });
    }

    #[test]
    fn owner_lock_caps_distinct_in_flight_operations() {
        let guards: Vec<_> = (0..MAX_CONCURRENT_LIQUIDITY_OPERATIONS)
            .map(|byte| {
                LiquidityOperationGuard::new(principal(byte as u8)).expect("within operation cap")
            })
            .collect();
        assert!(LiquidityOperationGuard::new(principal(200)).is_err());
        drop(guards);
        assert!(LiquidityOperationGuard::new(principal(200)).is_ok());
    }
}
