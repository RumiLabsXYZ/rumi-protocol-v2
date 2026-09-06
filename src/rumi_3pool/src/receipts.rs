//! Caller-scoped, never-evicted swap attempts. No API replays a retained attempt.
//! The stable fence is deliberately not an admin-clearable boolean: a lost
//! callback requires evidence-backed recovery before reserves may move again.
use crate::storage;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_RECEIPTS: u64 = 10_000;
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwapRequestV1 {
    pub intent_id: Vec<u8>,
    pub i: u8,
    pub j: u8,
    pub dx: u128,
    pub min_dy: u128,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SwapReceiptStatusV1 {
    Prepared,
    InputSubmitted,
    OutputSubmitted,
    RefundSubmitted,
    Completed,
    Refunded,
    Failed,
    Unresolved,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SwapTransferStatusV1 {
    Submitted,
    Confirmed,
    Rejected,
    Unresolved,
    SkippedDust,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwapTransferV1 {
    pub ledger: Principal,
    pub from: Account,
    pub to: Account,
    /// Credited amount; fee is paid in addition by from.
    pub amount: u128,
    pub fee: u128,
    pub created_at_time: u64,
    pub memo: Vec<u8>,
    pub block_index: Option<Nat>,
    pub status: SwapTransferStatusV1,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwapReceiptV1 {
    pub version: u16,
    pub owner: Principal,
    pub request: SwapRequestV1,
    pub status: SwapReceiptStatusV1,
    pub input: Option<SwapTransferV1>,
    pub output: Option<SwapTransferV1>,
    pub refund: Option<SwapTransferV1>,
    pub pool_fee: Option<u128>,
    pub gross_output: Option<u128>,
    pub error: Option<String>,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SwapReceiptErrorV1 {
    InvalidIntentId,
    IntentConflict,
    CapacityExceeded,
    Unauthorized,
    InvalidRequest,
}

pub fn client_enabled(client: Principal) -> bool {
    storage::SWAP_RECEIPT_CLIENTS
        .with(|m| m.borrow().contains_key(&storage::StorablePrincipal(client)))
}
pub fn set_client(client: Principal, enabled: bool) -> Result<(), SwapReceiptErrorV1> {
    if client == Principal::anonymous() {
        return Err(SwapReceiptErrorV1::Unauthorized);
    }
    storage::SWAP_RECEIPT_CLIENTS.with(|m| {
        let mut m = m.borrow_mut();
        let key = storage::StorablePrincipal(client);
        if enabled {
            if !m.contains_key(&key) && m.len() >= 64 {
                return Err(SwapReceiptErrorV1::CapacityExceeded);
            }
            m.insert(key, storage::Unit);
        } else {
            m.remove(&key);
        }
        Ok(())
    })
}

pub fn key(owner: Principal, intent: &[u8]) -> Vec<u8> {
    let mut key = vec![owner.as_slice().len() as u8];
    key.extend_from_slice(owner.as_slice());
    key.extend_from_slice(intent);
    key
}
pub fn get(owner: Principal, intent: &[u8]) -> Option<SwapReceiptV1> {
    if intent.len() != 32 {
        return None;
    }
    storage::SWAP_RECEIPTS.with(|m| m.borrow().get(&key(owner, intent)))
}
pub fn save(receipt: &SwapReceiptV1) {
    storage::SWAP_RECEIPTS.with(|m| {
        m.borrow_mut().insert(
            key(receipt.owner, &receipt.request.intent_id),
            receipt.clone(),
        )
    });
}
pub fn reserve(
    owner: Principal,
    request: SwapRequestV1,
) -> Result<(SwapReceiptV1, bool), SwapReceiptErrorV1> {
    if owner == Principal::anonymous() {
        return Err(SwapReceiptErrorV1::Unauthorized);
    }
    if request.intent_id.len() != 32 {
        return Err(SwapReceiptErrorV1::InvalidIntentId);
    }
    if request.i >= 3 || request.j >= 3 || request.i == request.j || request.dx == 0 {
        return Err(SwapReceiptErrorV1::InvalidRequest);
    }
    if let Some(existing) = get(owner, &request.intent_id) {
        return if existing.request == request {
            Ok((existing, false))
        } else {
            Err(SwapReceiptErrorV1::IntentConflict)
        };
    }
    if storage::SWAP_RECEIPTS.with(|m| m.borrow().len()) >= MAX_RECEIPTS {
        return Err(SwapReceiptErrorV1::CapacityExceeded);
    }
    let receipt = SwapReceiptV1 {
        version: 1,
        owner,
        request,
        status: SwapReceiptStatusV1::Prepared,
        input: None,
        output: None,
        refund: None,
        pool_fee: None,
        gross_output: None,
        error: None,
    };
    save(&receipt);
    Ok((receipt, true))
}
pub fn fenced() -> bool {
    storage::SWAP_RECEIPT_FENCE.with(|c| *c.borrow().get() != 0)
}
pub(crate) fn set_fence(active: bool) {
    storage::SWAP_RECEIPT_FENCE.with(|c| {
        c.borrow_mut()
            .set(u8::from(active))
            .expect("persist swap receipt fence")
    });
}
pub fn fail(receipt: &mut SwapReceiptV1, reason: String, unresolved: bool) {
    receipt.error = Some(reason.chars().take(512).collect());
    receipt.status = if unresolved {
        SwapReceiptStatusV1::Unresolved
    } else {
        SwapReceiptStatusV1::Failed
    };
    save(receipt);
}
pub fn transfer_intent(
    receipt: &SwapReceiptV1,
    leg: u8,
    ledger: Principal,
    from: Principal,
    to: Principal,
    amount: u128,
    fee: u128,
) -> SwapTransferV1 {
    let mut digest = Sha256::new();
    digest.update(b"rumi-3pool-swap-receipt-v1");
    digest.update(key(receipt.owner, &receipt.request.intent_id));
    digest.update([leg]);
    SwapTransferV1 {
        ledger,
        from: Account {
            owner: from,
            subaccount: None,
        },
        to: Account {
            owner: to,
            subaccount: None,
        },
        amount,
        fee,
        created_at_time: ic_cdk::api::time(),
        memo: digest.finalize().to_vec(),
        block_index: None,
        status: SwapTransferStatusV1::Submitted,
    }
}

/// Explicit fees bind receipt economics. A transport rejection is ambiguous;
/// only a typed ledger error proves that this attempt did not transfer tokens.
pub async fn execute(transfer: &SwapTransferV1, pull: bool) -> Result<Nat, (bool, String)> {
    use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
    use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
    if pull {
        let args = TransferFromArgs {
            spender_subaccount: None,
            from: transfer.from,
            to: transfer.to,
            amount: Nat::from(transfer.amount),
            fee: Some(Nat::from(transfer.fee)),
            memo: Some(transfer.memo.clone().into()),
            created_at_time: Some(transfer.created_at_time),
        };
        let result: Result<(Result<Nat, TransferFromError>,), _> =
            ic_cdk::call(transfer.ledger, "icrc2_transfer_from", (args,)).await;
        match result {
            Ok((Ok(id),)) => Ok(id),
            Ok((Err(TransferFromError::Duplicate { duplicate_of }),)) => Ok(duplicate_of),
            Ok((Err(e),)) => Err((
                matches!(
                    e,
                    TransferFromError::GenericError { .. }
                        | TransferFromError::TemporarilyUnavailable
                ),
                format!("{e:?}"),
            )),
            Err(e) => Err((true, format!("{e:?}"))),
        }
    } else {
        let args = TransferArg {
            from_subaccount: None,
            to: transfer.to,
            amount: Nat::from(transfer.amount),
            fee: Some(Nat::from(transfer.fee)),
            memo: Some(transfer.memo.clone().into()),
            created_at_time: Some(transfer.created_at_time),
        };
        let result: Result<(Result<Nat, TransferError>,), _> =
            ic_cdk::call(transfer.ledger, "icrc1_transfer", (args,)).await;
        match result {
            Ok((Ok(id),)) => Ok(id),
            Ok((Err(TransferError::Duplicate { duplicate_of }),)) => Ok(duplicate_of),
            Ok((Err(e),)) => Err((
                matches!(
                    e,
                    TransferError::GenericError { .. } | TransferError::TemporarilyUnavailable
                ),
                format!("{e:?}"),
            )),
            Err(e) => Err((true, format!("{e:?}"))),
        }
    }
}

pub async fn run_leg(
    receipt: &mut SwapReceiptV1,
    leg: u8,
    transfer: SwapTransferV1,
) -> Result<(), (bool, String)> {
    receipt.status = match leg {
        0 => SwapReceiptStatusV1::InputSubmitted,
        1 => SwapReceiptStatusV1::OutputSubmitted,
        _ => SwapReceiptStatusV1::RefundSubmitted,
    };
    match leg {
        0 => receipt.input = Some(transfer.clone()),
        1 => receipt.output = Some(transfer.clone()),
        _ => receipt.refund = Some(transfer.clone()),
    }
    save(receipt); // All arguments and submission state commit before ledger call.
    let result = execute(&transfer, leg == 0).await;
    let slot = match leg {
        0 => &mut receipt.input,
        1 => &mut receipt.output,
        _ => &mut receipt.refund,
    };
    let recorded = slot.as_mut().expect("submitted leg");
    match &result {
        Ok(id) => {
            recorded.block_index = Some(id.clone());
            recorded.status = SwapTransferStatusV1::Confirmed;
        }
        Err((ambiguous, _)) => {
            recorded.status = if *ambiguous {
                SwapTransferStatusV1::Unresolved
            } else {
                SwapTransferStatusV1::Rejected
            }
        }
    }
    save(receipt);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> SwapRequestV1 {
        SwapRequestV1 {
            intent_id: vec![7; 32],
            i: 0,
            j: 1,
            dx: 1_000_000,
            min_dy: 1,
        }
    }
    #[test]
    fn duplicate_and_conflict_binding_is_caller_scoped() {
        let owner = Principal::self_authenticating(b"alice");
        let (mut r, fresh) = reserve(owner, request()).unwrap();
        assert!(fresh);
        set_fence(true);
        r.status = SwapReceiptStatusV1::InputSubmitted;
        save(&r);
        let (again, fresh) = reserve(owner, request()).unwrap();
        assert!(!fresh);
        assert_eq!(again, r);
        let mut conflict = request();
        conflict.min_dy += 1;
        assert_eq!(
            reserve(owner, conflict).unwrap_err(),
            SwapReceiptErrorV1::IntentConflict
        );
        assert!(get(Principal::self_authenticating(b"bob"), &request().intent_id).is_none());
        assert!(fenced());
        assert!(crate::pool_guard::PoolGuard::new().is_err());
        set_fence(false);
    }
    #[test]
    fn caller_and_intent_validation_precede_allocation() {
        assert_eq!(
            reserve(Principal::anonymous(), request()).unwrap_err(),
            SwapReceiptErrorV1::Unauthorized
        );
        let mut bad = request();
        bad.intent_id.push(0);
        assert_eq!(
            reserve(Principal::self_authenticating(b"a"), bad).unwrap_err(),
            SwapReceiptErrorV1::InvalidIntentId
        );
        assert_eq!(storage::SWAP_RECEIPTS.with(|m| m.borrow().len()), 0);
    }
    #[test]
    fn full_request_and_terminal_evidence_roundtrip() {
        let owner = Principal::self_authenticating(b"alice");
        let (mut r, _) = reserve(owner, request()).unwrap();
        r.status = SwapReceiptStatusV1::Completed;
        r.gross_output = Some(999);
        r.pool_fee = Some(3);
        r.input = Some(SwapTransferV1 {
            ledger: owner,
            from: Account {
                owner,
                subaccount: None,
            },
            to: Account {
                owner: Principal::management_canister(),
                subaccount: None,
            },
            amount: 100,
            fee: 10,
            created_at_time: 123,
            memo: vec![1; 32],
            block_index: Some(Nat::from(42u64)),
            status: SwapTransferStatusV1::Confirmed,
        });
        save(&r);
        assert_eq!(get(owner, &r.request.intent_id), Some(r.clone()));
        let encoded = candid::encode_one(&r).unwrap();
        let decoded: SwapReceiptV1 = candid::decode_one(&encoded).unwrap();
        assert_eq!(decoded, r);
        let (again, fresh) = reserve(owner, request()).unwrap();
        assert!(!fresh);
        assert_eq!(again, r);
    }
    #[test]
    fn client_capability_is_empty_bounded_and_revocable() {
        let owner = Principal::self_authenticating(b"alice");
        assert!(!client_enabled(owner));
        set_client(owner, true).unwrap();
        assert!(client_enabled(owner));
        set_client(owner, false).unwrap();
        assert!(!client_enabled(owner));
        for n in 0u64..64 {
            set_client(Principal::self_authenticating(&n.to_be_bytes()), true).unwrap();
        }
        assert_eq!(
            set_client(owner, true),
            Err(SwapReceiptErrorV1::CapacityExceeded)
        );
        assert_eq!(
            set_client(Principal::anonymous(), true),
            Err(SwapReceiptErrorV1::Unauthorized)
        );
    }

    #[test]
    fn capacity_never_evicts_and_existing_intent_still_reads() {
        let owner = Principal::self_authenticating(b"alice");
        let (r, _) = reserve(owner, request()).unwrap();
        for n in 1..MAX_RECEIPTS {
            let mut row = r.clone();
            row.request.intent_id[..8].copy_from_slice(&n.to_be_bytes());
            save(&row);
        }
        let mut extra = request();
        extra.intent_id = vec![255; 32];
        assert_eq!(
            reserve(owner, extra).unwrap_err(),
            SwapReceiptErrorV1::CapacityExceeded
        );
        assert!(!reserve(owner, request()).unwrap().1);
        assert!(!fenced());
    }
}
