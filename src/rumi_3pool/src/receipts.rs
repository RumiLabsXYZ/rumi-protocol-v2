//! Caller-scoped swap attempts. Terminal rows may be pruned only after the
//! caller's durable monotonic intent sequence advances; unresolved rows remain.
//! The stable fence is deliberately not an admin-clearable boolean: a lost
//! callback requires evidence-backed recovery before reserves may move again.
use crate::storage;
use crate::icrc3::Icrc3Value;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::transfer::TransferError;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_RECEIPTS: u64 = 10_000;

/// Permanent receipt for a backend-minted donation. It is separate from swap
/// receipts and never evicted: a retry must not credit pool balances twice.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreePoolDonationReceipt {
    pub caller: Principal,
    pub op_nonce: u128,
    pub token_index: u8,
    pub amount: u128,
}

pub fn donation_key(caller: Principal, op_nonce: u128) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + caller.as_slice().len() + 16);
    key.push(caller.as_slice().len() as u8);
    key.extend_from_slice(caller.as_slice());
    key.extend_from_slice(&op_nonce.to_be_bytes());
    key
}

pub fn get_donation(caller: Principal, op_nonce: u128) -> Option<ThreePoolDonationReceipt> {
    storage::THREE_POOL_DONATION_RECEIPTS
        .with(|m| m.borrow().get(&donation_key(caller, op_nonce)))
}

pub fn save_donation(receipt: ThreePoolDonationReceipt) {
    storage::THREE_POOL_DONATION_RECEIPTS.with(|m| {
        m.borrow_mut().insert(
            donation_key(receipt.caller, receipt.op_nonce),
            receipt,
        )
    });
}
pub const MAX_ACTIVE_RECEIPTS_PER_OWNER: usize = 64;
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
pub enum IngressRequestV1 {
    AddLiquidity { amounts: [u128; 3], min_lp: u128 },
    Donate { token_index: u8, amount: u128 },
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IngressStatusV1 { Prepared, Pulling, Unresolved, Completed, Failed }
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressReceiptV1 {
    pub version: u16,
    pub owner: Principal,
    pub intent_id: Vec<u8>,
    pub request: IngressRequestV1,
    pub pulls: Vec<SwapTransferV1>,
    pub add_facts: Option<AddLiquidityFactsV1>,
    pub status: IngressStatusV1,
    pub result_lp: Option<u128>,
    pub error: Option<String>,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddLiquidityFactsV1 {
    pub lp_minted: u128,
    pub fees_native: [u128; 3],
    pub fee_bps_used: u16,
    pub imbalance_before: u64,
    pub imbalance_after: u64,
    pub is_rebalancing: bool,
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IngressReceiptErrorV1 { InvalidIntentId, StaleIntentSequence, IntentConflict, CapacityExceeded, Unauthorized, InvalidRequest, PoolLocked, ProofUnavailable, ProofMismatch }

pub fn get_ingress(owner: Principal, intent_id: &[u8]) -> Option<IngressReceiptV1> {
    if intent_id.len() != 32 { return None; }
    storage::INGRESS_RECEIPTS.with(|m| m.borrow().get(&key(owner, intent_id)))
}
pub fn save_ingress(receipt: &IngressReceiptV1) {
    storage::INGRESS_RECEIPTS.with(|m| m.borrow_mut().insert(key(receipt.owner, &receipt.intent_id), receipt.clone()));
}
pub fn reserve_ingress(owner: Principal, intent_id: Vec<u8>, request: IngressRequestV1) -> Result<(IngressReceiptV1, bool), IngressReceiptErrorV1> {
    if owner == Principal::anonymous() { return Err(IngressReceiptErrorV1::Unauthorized); }
    if intent_id.len() != 32 { return Err(IngressReceiptErrorV1::InvalidIntentId); }
    match &request {
        IngressRequestV1::AddLiquidity { amounts, .. } if amounts.iter().all(|n| *n == 0) => return Err(IngressReceiptErrorV1::InvalidRequest),
        IngressRequestV1::Donate { token_index, amount } if *token_index >= 3 || *amount == 0 => return Err(IngressReceiptErrorV1::InvalidRequest),
        _ => {}
    }
    if let Some(old) = get_ingress(owner, &intent_id) {
        return if old.request == request { Ok((old, false)) } else { Err(IngressReceiptErrorV1::IntentConflict) };
    }
    let active_for_owner = storage::INGRESS_RECEIPTS.with(|m| m.borrow().iter()
        .filter(|(_, r)| r.owner == owner && !matches!(r.status, IngressStatusV1::Completed | IngressStatusV1::Failed)).count());
    if active_for_owner >= MAX_ACTIVE_RECEIPTS_PER_OWNER { return Err(IngressReceiptErrorV1::CapacityExceeded); }
    if !storage::intent_owner_capacity_available(owner) { return Err(IngressReceiptErrorV1::CapacityExceeded); }
    if storage::INGRESS_RECEIPTS.with(|m| m.borrow().len()) >= MAX_RECEIPTS { return Err(IngressReceiptErrorV1::CapacityExceeded); }
    if !storage::accept_intent_sequence(owner, &intent_id) {
        return Err(IngressReceiptErrorV1::StaleIntentSequence);
    }
    let receipt = IngressReceiptV1 { version: 1, owner, intent_id, request, pulls: Vec::new(), add_facts: None, status: IngressStatusV1::Prepared, result_lp: None, error: None };
    save_ingress(&receipt);
    Ok((receipt, true))
}
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SwapReceiptErrorV1 {
    InvalidIntentId,
    StaleIntentSequence,
    IntentConflict,
    CapacityExceeded,
    Unauthorized,
    InvalidRequest,
    ProofUnavailable,
    ProofMismatch,
    PoolLocked,
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
    let active_for_owner = storage::SWAP_RECEIPTS.with(|m| m.borrow().iter().filter(|(_, r)|
        r.owner == owner && !matches!(r.status, SwapReceiptStatusV1::Completed | SwapReceiptStatusV1::Refunded | SwapReceiptStatusV1::Failed)).count());
    if active_for_owner >= MAX_ACTIVE_RECEIPTS_PER_OWNER { return Err(SwapReceiptErrorV1::CapacityExceeded); }
    if !storage::intent_owner_capacity_available(owner) { return Err(SwapReceiptErrorV1::CapacityExceeded); }
    if storage::SWAP_RECEIPTS.with(|m| m.borrow().len()) >= MAX_RECEIPTS {
        return Err(SwapReceiptErrorV1::CapacityExceeded);
    }
    if !storage::accept_intent_sequence(owner, &request.intent_id) {
        return Err(SwapReceiptErrorV1::StaleIntentSequence);
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
    storage::SWAP_RECEIPT_FENCE.with(|c| *c.borrow().get() != 0) || outstanding_receipt_work()
}
pub(crate) fn set_fence(active: bool) {
    // The fence is shared by all receipt-backed operations. Clearing it from
    // one completed row must not unlock the pool while another row still has
    // an unresolved transfer or unfinished accounting phase.
    let active = active || outstanding_receipt_work();
    storage::SWAP_RECEIPT_FENCE.with(|c| {
        c.borrow_mut()
            .set(u8::from(active))
            .expect("persist swap receipt fence")
    });
}

fn outstanding_receipt_work() -> bool {
    storage::SWAP_RECEIPTS.with(|m| {
        m.borrow().iter().any(|(_, receipt)| {
            !matches!(receipt.status, SwapReceiptStatusV1::Completed | SwapReceiptStatusV1::Refunded | SwapReceiptStatusV1::Failed)
                || [&receipt.input, &receipt.output, &receipt.refund].into_iter().flatten().any(|leg| {
                    matches!(leg.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved)
                })
                || (receipt.status == SwapReceiptStatusV1::Failed
                    && receipt.input.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false)
                    && !receipt.output.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false)
                    && !receipt.refund.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false))
        })
    }) || storage::INGRESS_RECEIPTS.with(|m| {
        m.borrow().iter().any(|(_, receipt)| {
            !matches!(receipt.status, IngressStatusV1::Completed | IngressStatusV1::Failed)
                || receipt.pulls.iter().any(|leg| matches!(leg.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved))
                || (receipt.status == IngressStatusV1::Failed
                    && receipt.pulls.iter().any(|leg| leg.status == SwapTransferStatusV1::Confirmed))
        })
    })
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

pub fn ingress_transfer_intent(
    owner: Principal,
    intent_id: &[u8],
    leg: u8,
    ledger: Principal,
    from: Principal,
    to: Principal,
    amount: u128,
    fee: u128,
) -> SwapTransferV1 {
    let mut digest = Sha256::new();
    digest.update(b"rumi-3pool-ingress-v1");
    digest.update(key(owner, intent_id));
    digest.update([leg]);
    SwapTransferV1 {
        ledger,
        from: Account { owner: from, subaccount: None },
        to: Account { owner: to, subaccount: None },
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
/// Fetch a caller-supplied candidate from the configured ledger or its
/// advertised archive callback and bind it to the persisted transfer tuple.
/// This runs only from update methods, so the inter-canister query executes
/// in replicated mode. Unsupported ledgers fail closed and remain held.
pub async fn matches_ledger_block(
    ledger: Principal,
    block_index: &Nat,
    expected: &SwapTransferV1,
    pull: bool,
) -> Result<bool, String> {
    use crate::icrc3::{GetBlocksArgs, GetBlocksResult};
    let index: u64 = block_index.0.clone().try_into().map_err(|_| "block index exceeds u64".to_string())?;
    let args = vec![GetBlocksArgs { start: Nat::from(index), length: Nat::from(1u8) }];
    let response: (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (args.clone(),))
        .await.map_err(|(code, message)| format!("ledger ICRC-3 query rejected: {code:?}: {message}"))?;
    if let Some(block) = response.0.blocks.iter().find(|block| block.id == *block_index) {
        return Ok(block_matches_transfer(&block.block, expected, pull, ic_cdk::id()));
    }
    for archive in response.0.archived_blocks {
        let covers = archive.args.iter().any(|arg| {
            let start: Option<u64> = arg.start.0.clone().try_into().ok();
            let length: Option<u64> = arg.length.0.clone().try_into().ok();
            matches!((start, length), (Some(s), Some(n)) if index >= s && index < s.saturating_add(n))
        });
        if !covers { continue; }
        let archived: (GetBlocksResult,) = ic_cdk::call(
            archive.callback.canister_id,
            &archive.callback.method,
            (archive.args,),
        ).await.map_err(|(code, message)| format!("ledger archive query rejected: {code:?}: {message}"))?;
        if let Some(block) = archived.0.blocks.iter().find(|block| block.id == *block_index) {
            return Ok(block_matches_transfer(&block.block, expected, pull, ic_cdk::id()));
        }
    }
    Ok(false)
}

fn block_matches_transfer(
    block: &Icrc3Value,
    expected: &SwapTransferV1,
    pull: bool,
    pool_id: Principal,
) -> bool {
    fn field<'a>(value: &'a Icrc3Value, name: &str) -> Option<&'a Icrc3Value> {
        match value { Icrc3Value::Map(fields) => fields.iter().find(|(key, _)| key == name).map(|(_, value)| value), _ => None }
    }
    fn nat(value: Option<&Icrc3Value>) -> Option<u128> {
        match value { Some(Icrc3Value::Nat(n)) => n.0.clone().try_into().ok(), _ => None }
    }
    fn blob(value: Option<&Icrc3Value>) -> Option<&[u8]> {
        match value { Some(Icrc3Value::Blob(bytes)) => Some(bytes), _ => None }
    }
    fn account_matches(value: Option<&Icrc3Value>, owner: Principal, subaccount: Option<&[u8]>) -> bool {
        let Some(Icrc3Value::Array(parts)) = value else { return false; };
        let Some(Icrc3Value::Blob(principal)) = parts.first() else { return false; };
        if principal.as_slice() != owner.as_slice() { return false; }
        match (parts.get(1), subaccount) {
            (None, None) => parts.len() == 1,
            (Some(Icrc3Value::Blob(got)), None) => parts.len() == 2 && got.iter().all(|b| *b == 0),
            (Some(Icrc3Value::Blob(got)), Some(want)) => parts.len() == 2 && got.as_slice() == want,
            _ => false,
        }
    }
    if !matches!(block, Icrc3Value::Map(_)) { return false; }
    let Some(tx) = field(block, "tx") else { return false; };
    let btype = match field(block, "btype") { Some(Icrc3Value::Text(text)) => Some(text.as_str()), _ => None };
    let op = match field(tx, "op") { Some(Icrc3Value::Text(text)) => Some(text.as_str()), _ => None };
    let expected_type = if pull { "2xfer" } else { "1xfer" };
    if btype.map(|kind| kind != expected_type).unwrap_or(op != Some("xfer")) { return false; }
    let Some(amount) = nat(field(tx, "amt")) else { return false; };
    let Some(fee) = nat(field(tx, "fee").or_else(|| field(block, "fee"))) else { return false; };
    let Some(created_at) = nat(field(tx, "ts")) else { return false; };
    let Some(memo) = blob(field(tx, "memo")) else { return false; };
    let from_ok = account_matches(field(tx, "from"), expected.from.owner, expected.from.subaccount.as_ref().map(|s| s.as_slice()));
    let to_ok = account_matches(field(tx, "to"), expected.to.owner, expected.to.subaccount.as_ref().map(|s| s.as_slice()));
    let spender_ok = if pull { account_matches(field(tx, "spender"), pool_id, None) } else { field(tx, "spender").is_none() };
    from_ok && to_ok && spender_ok && amount == expected.amount && fee == expected.fee
        && created_at == expected.created_at_time as u128 && memo == expected.memo.as_slice()
}

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
            Ok((Err(e),)) => Err((transfer_from_error_is_ambiguous(&e, false), format!("{e:?}"))),
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
            Ok((Err(e),)) => Err((transfer_error_is_ambiguous(&e, false), format!("{e:?}"))),
            Err(e) => Err((true, format!("{e:?}"))),
        }
    }
}

fn transfer_error_is_ambiguous(error: &TransferError, prior_ambiguous: bool) -> bool {
    prior_ambiguous || matches!(error,
        TransferError::TooOld | TransferError::GenericError { .. } | TransferError::TemporarilyUnavailable)
}

fn transfer_from_error_is_ambiguous(error: &TransferFromError, prior_ambiguous: bool) -> bool {
    prior_ambiguous || matches!(error,
        TransferFromError::TooOld | TransferFromError::GenericError { .. } | TransferFromError::TemporarilyUnavailable)
}

pub async fn run_leg(
    receipt: &mut SwapReceiptV1,
    leg: u8,
    proposed: SwapTransferV1,
) -> Result<(), (bool, String)> {
    let existing = match leg {
        0 => &receipt.input,
        1 => &receipt.output,
        _ => &receipt.refund,
    };
    let (transfer, prior_ambiguous) = if let Some(saved) = existing {
        if saved.ledger != proposed.ledger || saved.from != proposed.from || saved.to != proposed.to
            || saved.amount != proposed.amount || saved.fee != proposed.fee
        {
            return Err((true, "receipt leg does not match its persisted transfer tuple".to_string()));
        }
        if saved.status == SwapTransferStatusV1::Confirmed {
            return Ok(());
        }
        if saved.status == SwapTransferStatusV1::Rejected {
            return Err((false, "saved leg has a typed no-effect rejection".to_string()));
        }
        let prior_ambiguous = matches!(saved.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved);
        (saved.clone(), prior_ambiguous)
    } else {
        (proposed, false)
    };
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
            let effective_ambiguous = *ambiguous || prior_ambiguous;
            recorded.status = if effective_ambiguous {
                SwapTransferStatusV1::Unresolved
            } else {
                SwapTransferStatusV1::Rejected
            }
        }
    }
    save(receipt);
    match result {
        Ok(_) => Ok(()),
        Err((ambiguous, reason)) => Err((ambiguous || prior_ambiguous, reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn intent_id(sequence: u64) -> Vec<u8> {
        let mut id = vec![0; 32];
        id[..8].copy_from_slice(&sequence.to_be_bytes());
        id[8..].copy_from_slice(&[7; 24]);
        id
    }
    #[test]
    fn only_an_exact_icrc3_transfer_tuple_matches_a_receipt() {
        let pool = Principal::self_authenticating(b"pool");
        let user = Principal::self_authenticating(b"user");
        let expected = SwapTransferV1 {
            ledger: Principal::management_canister(),
            from: Account { owner: pool, subaccount: None },
            to: Account { owner: user, subaccount: None },
            amount: 900,
            fee: 10,
            created_at_time: 1234,
            memo: vec![8; 32],
            block_index: None,
            status: SwapTransferStatusV1::Unresolved,
        };
        let account = |p: Principal| Icrc3Value::Array(vec![Icrc3Value::Blob(p.as_slice().to_vec())]);
        let block = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("1xfer".into())),
            ("tx".into(), Icrc3Value::Map(vec![
                ("amt".into(), Icrc3Value::Nat(Nat::from(900u16))),
                ("fee".into(), Icrc3Value::Nat(Nat::from(10u8))),
                ("from".into(), account(pool)),
                ("to".into(), account(user)),
                ("memo".into(), Icrc3Value::Blob(vec![8; 32])),
                ("ts".into(), Icrc3Value::Nat(Nat::from(1234u16))),
            ])),
        ]);
        assert!(block_matches_transfer(&block, &expected, false, pool));
        let mut wrong = expected.clone();
        wrong.amount += 1;
        assert!(!block_matches_transfer(&block, &wrong, false, pool));
    }

    #[test]
    fn icrc3_transfer_from_tuple_binds_spender_and_subaccounts() {
        let pool = Principal::self_authenticating(b"pool");
        let user = Principal::self_authenticating(b"user");
        let from_subaccount = [3u8; 32];
        let pool_subaccount = [4u8; 32];
        let expected = SwapTransferV1 {
            ledger: Principal::management_canister(),
            from: Account { owner: user, subaccount: Some(from_subaccount.into()) },
            to: Account { owner: pool, subaccount: Some(pool_subaccount.into()) },
            amount: 77,
            fee: 2,
            created_at_time: 8_765,
            memo: vec![9; 32],
            block_index: None,
            status: SwapTransferStatusV1::Unresolved,
        };
        let account = |owner: Principal, subaccount: [u8; 32]| Icrc3Value::Array(vec![
            Icrc3Value::Blob(owner.as_slice().to_vec()),
            Icrc3Value::Blob(subaccount.to_vec()),
        ]);
        let tx = Icrc3Value::Map(vec![
            ("amt".into(), Icrc3Value::Nat(Nat::from(77u8))),
            ("fee".into(), Icrc3Value::Nat(Nat::from(2u8))),
            ("from".into(), account(user, from_subaccount)),
            ("to".into(), account(pool, pool_subaccount)),
            ("spender".into(), account(pool, [0; 32])),
            ("memo".into(), Icrc3Value::Blob(vec![9; 32])),
            ("ts".into(), Icrc3Value::Nat(Nat::from(8_765u16))),
        ]);
        let block = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("2xfer".into())),
            ("tx".into(), tx.clone()),
        ]);
        assert!(block_matches_transfer(&block, &expected, true, pool));

        let mut wrong_spender = tx;
        if let Icrc3Value::Map(fields) = &mut wrong_spender {
            let wrong = Principal::self_authenticating(b"other spender");
            fields.iter_mut().find(|(key, _)| key == "spender").unwrap().1 = account(wrong, [0; 32]);
        }
        let wrong_block = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("2xfer".into())),
            ("tx".into(), wrong_spender),
        ]);
        assert!(!block_matches_transfer(&wrong_block, &expected, true, pool));
    }

    #[test]
    fn too_old_and_retries_after_ambiguity_never_prove_no_effect() {
        use icrc_ledger_types::icrc1::transfer::TransferError;
        use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
        assert!(transfer_error_is_ambiguous(&TransferError::TooOld, false));
        assert!(transfer_from_error_is_ambiguous(&TransferFromError::TooOld, false));
        assert!(!transfer_error_is_ambiguous(&TransferError::BadFee { expected_fee: Nat::from(1u8) }, false));
        assert!(transfer_error_is_ambiguous(&TransferError::BadFee { expected_fee: Nat::from(1u8) }, true));
        assert!(transfer_from_error_is_ambiguous(&TransferFromError::InsufficientAllowance { allowance: Nat::from(0u8) }, true));
    }

    fn request() -> SwapRequestV1 {
        SwapRequestV1 {
            intent_id: intent_id(1),
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
        r.status = SwapReceiptStatusV1::Completed;
        save(&r);
        set_fence(false);
    }

    #[test]
    fn clearing_one_receipt_cannot_unfence_another_outstanding_receipt() {
        let first_owner = Principal::self_authenticating(b"fence owner one");
        let second_owner = Principal::self_authenticating(b"fence owner two");
        let (mut first, _) = reserve(first_owner, request()).unwrap();
        let mut second_request = request();
        second_request.intent_id = intent_id(1);
        let (mut second, _) = reserve(second_owner, second_request).unwrap();
        set_fence(true);
        first.status = SwapReceiptStatusV1::Completed;
        save(&first);
        set_fence(false);
        assert!(fenced(), "second unresolved receipt must retain global fence");
        second.status = SwapReceiptStatusV1::Completed;
        save(&second);
        set_fence(false);
        assert!(!fenced());
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
            row.owner = Principal::self_authenticating(&n.to_be_bytes());
            save(&row);
        }
        let mut extra = request();
        extra.intent_id = vec![255; 32];
        assert_eq!(
            reserve(owner, extra).unwrap_err(),
            SwapReceiptErrorV1::CapacityExceeded
        );
        assert!(!reserve(owner, request()).unwrap().1);
        assert!(fenced());
    }

    #[test]
    fn high_water_prunes_only_terminal_rows_and_rejects_delayed_duplicate() {
        let owner = Principal::self_authenticating(b"sequence-owner");
        let first = request();
        let (mut terminal, _) = reserve(owner, first.clone()).unwrap();
        terminal.status = SwapReceiptStatusV1::Completed;
        save(&terminal);

        let mut second = request();
        second.intent_id = intent_id(2);
        assert!(reserve(owner, second).unwrap().1);
        assert!(get(owner, &first.intent_id).is_none());
        assert_eq!(reserve(owner, first).unwrap_err(), SwapReceiptErrorV1::StaleIntentSequence);

        let mut unresolved = request();
        unresolved.intent_id = intent_id(3);
        let (mut unresolved_row, _) = reserve(owner, unresolved.clone()).unwrap();
        unresolved_row.status = SwapReceiptStatusV1::Unresolved;
        save(&unresolved_row);
        let mut fourth = request();
        fourth.intent_id = intent_id(4);
        assert!(reserve(owner, fourth).unwrap().1);
        assert_eq!(get(owner, &unresolved.intent_id), Some(unresolved_row));
    }
}
