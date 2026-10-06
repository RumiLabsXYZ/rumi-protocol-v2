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
pub struct AbsenceScanV1 {
    pub fixed_tip: Nat,
    pub cursor: Nat,
    pub generation: u32,
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
    /// Ledger log length observed before this generation's first dispatch.
    #[serde(default)]
    pub history_start: Option<Nat>,
    /// Durable cursor for a fixed-tip, archive-complete absence scan.
    #[serde(default)]
    pub absence_scan: Option<AbsenceScanV1>,
    #[serde(default)]
    pub generation: Option<u32>,
    /// Number of calls persisted before dispatch for this exact tuple.
    #[serde(default)]
    pub dispatch_count: Option<u32>,
    /// Set only after an ambiguous generation was retried and received TooOld.
    #[serde(default)]
    pub too_old_after_ambiguity: Option<bool>,
    /// Hash-chain tombstone for retired exact transfer tuples.
    #[serde(default)]
    pub retired_identity_hash: Option<Vec<u8>>,
    /// Complete absence proof retired the preceding tuple and authorized this
    /// generation for its first dispatch; it has not yet had an ambiguous try.
    #[serde(default)]
    pub ready_to_dispatch: Option<bool>,
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

/// A receipt continuation may enter the pool only when it is the sole active
/// stable receipt. This is the narrow exception that lets the operation which
/// raised the fence resume without unlocking unrelated reserve mutations.
pub(crate) fn is_only_active_receipt(owner: Principal, intent_id: &[u8]) -> bool {
    let mut active_count = 0usize;
    let mut matches = false;
    storage::SWAP_RECEIPTS.with(|map| {
        for (_, receipt) in map.borrow().iter() {
            if swap_receipt_fence_active(&receipt) {
                active_count += 1;
                matches |= receipt.owner == owner && receipt.request.intent_id == intent_id;
            }
        }
    });
    storage::INGRESS_RECEIPTS.with(|map| {
        for (_, receipt) in map.borrow().iter() {
            if ingress_receipt_fence_active(&receipt) {
                active_count += 1;
                matches |= receipt.owner == owner && receipt.intent_id == intent_id;
            }
        }
    });
    active_count == 1 && matches
}

fn swap_receipt_fence_active(receipt: &SwapReceiptV1) -> bool {
    !matches!(receipt.status, SwapReceiptStatusV1::Completed | SwapReceiptStatusV1::Refunded | SwapReceiptStatusV1::Failed)
        || [&receipt.input, &receipt.output, &receipt.refund].into_iter().flatten().any(|leg| {
            matches!(leg.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved)
        })
        || (receipt.status == SwapReceiptStatusV1::Failed
            && receipt.input.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false)
            && !receipt.output.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false)
            && !receipt.refund.as_ref().map(|leg| leg.status == SwapTransferStatusV1::Confirmed).unwrap_or(false))
}

fn ingress_receipt_fence_active(receipt: &IngressReceiptV1) -> bool {
    !matches!(receipt.status, IngressStatusV1::Completed | IngressStatusV1::Failed)
        || receipt.pulls.iter().any(|leg| matches!(leg.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved))
        || (receipt.status == IngressStatusV1::Failed
            && receipt.pulls.iter().any(|leg| leg.status == SwapTransferStatusV1::Confirmed))
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
        m.borrow().iter().any(|(_, receipt)| swap_receipt_fence_active(&receipt))
    }) || storage::INGRESS_RECEIPTS.with(|m| {
        m.borrow().iter().any(|(_, receipt)| ingress_receipt_fence_active(&receipt))
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
        history_start: None,
        absence_scan: None,
        generation: Some(0),
        dispatch_count: Some(0),
        too_old_after_ambiguity: Some(false),
        retired_identity_hash: None,
        ready_to_dispatch: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferFailureClass { ProvenNoEffect, Ambiguous, TooOld }

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
        history_start: None,
        absence_scan: None,
        generation: Some(0),
        dispatch_count: Some(0),
        too_old_after_ambiguity: Some(false),
        retired_identity_hash: None,
        ready_to_dispatch: None,
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
    // Never turn a caller-supplied principal into a proof source. The exact
    // ledger is persisted in the obligation, and it must still be one of the
    // pool's configured token ledgers before making either the main-ledger
    // or archive callback call.
    if ledger != expected.ledger
        || !crate::state::read_state(|s| s.config.tokens.iter().any(|token| token.ledger_id == ledger))
    {
        return Err("proof ledger is not the persisted configured token ledger".to_string());
    }
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

/// A bounded result for one contiguous part of a fixed-tip history scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbsencePage {
    Match(Nat),
    Continue(Nat),
    Complete,
}

/// Read the authoritative log length from the configured ledger. An empty
/// ICRC-3 request returns `log_length` without transferring block data.
pub async fn ledger_log_length(ledger: Principal) -> Result<Nat, String> {
    use crate::icrc3::{GetBlocksResult};
    if !configured_token_ledger(ledger) { return Err("ledger is not configured for this pool".into()); }
    let response: (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (Vec::<crate::icrc3::GetBlocksArgs>::new(),))
        .await.map_err(|(code, message)| format!("ledger tip query rejected: {code:?}: {message}"))?;
    Ok(response.0.log_length)
}

fn configured_token_ledger(ledger: Principal) -> bool {
    crate::state::read_state(|s| s.config.tokens.iter().any(|token| token.ledger_id == ledger))
}

fn has_reviewed_ledger_lineage(_ledger: Principal) -> bool {
    // A principal alone does not pin the installed ledger implementation.
    // Keep production absence-based identity rotation disabled until admission
    // binds each ledger's actual module hash and archive schema. Positive exact
    // receipt reconciliation remains available without an absence profile.
    false
}

/// Scan at most 100 consecutive ledger indexes. Missing, duplicate, extra,
/// overlapping, or uncovered indexes are errors; callers must keep the fence
/// held and retry from the saved cursor. Archive callbacks are called exactly
/// as advertised by the configured ledger.
pub async fn scan_absence_page(
    transfer: &SwapTransferV1,
    scan: &AbsenceScanV1,
    pull: bool,
) -> Result<AbsencePage, String> {
    use crate::icrc3::{GetBlocksArgs, GetBlocksResult};
    if !cfg!(feature = "test_endpoints") && !has_reviewed_ledger_lineage(transfer.ledger) {
        return Err("absence recovery is not enabled for this ledger implementation".into());
    }
    if !configured_token_ledger(transfer.ledger) { return Err("ledger is not configured for this pool".into()); }
    let start: u64 = scan.cursor.0.clone().try_into().map_err(|_| "scan cursor exceeds u64".to_string())?;
    let tip: u64 = scan.fixed_tip.0.clone().try_into().map_err(|_| "scan tip exceeds u64".to_string())?;
    if start > tip { return Err("scan cursor exceeds fixed tip".into()); }
    if start == tip { return Ok(AbsencePage::Complete); }
    let end = start.saturating_add(100).min(tip);
    let length = end - start;
    let args = vec![GetBlocksArgs { start: Nat::from(start), length: Nat::from(length) }];
    let main: (GetBlocksResult,) = ic_cdk::call(transfer.ledger, "icrc3_get_blocks", (args,))
        .await.map_err(|(code, message)| format!("ledger scan rejected: {code:?}: {message}"))?;
    let observed_tip: u64 = main.0.log_length.0.clone().try_into().map_err(|_| "ledger log length exceeds u64".to_string())?;
    if observed_tip < tip { return Err("ledger log length regressed below fixed tip".into()); }

    let mut found = std::collections::BTreeMap::<u64, Icrc3Value>::new();
    for block in main.0.blocks {
        let id: u64 = block.id.0.try_into().map_err(|_| "ledger block id exceeds u64".to_string())?;
        if id < start || id >= end || found.insert(id, block.block).is_some() {
            return Err("ledger returned a duplicate or out-of-range block".into());
        }
    }
    let mut archive_coverage = std::collections::BTreeSet::<u64>::new();
    if main.0.archived_blocks.len() > 100 {
        return Err("ledger advertised more than 100 archive callbacks for one scan page".into());
    }
    for archive in main.0.archived_blocks {
        let mut requested = Vec::new();
        for arg in &archive.args {
            let a: u64 = arg.start.0.clone().try_into().map_err(|_| "archive start exceeds u64".to_string())?;
            let n: u64 = arg.length.0.clone().try_into().map_err(|_| "archive length exceeds u64".to_string())?;
            let a_end = a.checked_add(n).ok_or("archive range overflow")?;
            if n == 0 || a < start || a_end > end { return Err("archive range escapes requested page".into()); }
            for id in a..a_end {
                if found.contains_key(&id) || !archive_coverage.insert(id) {
                    return Err("overlapping or duplicate archive coverage".into());
                }
            }
            requested.push(arg.clone());
        }
        let archived: (GetBlocksResult,) = ic_cdk::call(
            archive.callback.canister_id,
            &archive.callback.method,
            (requested,),
        ).await.map_err(|(code, message)| format!("archive scan rejected: {code:?}: {message}"))?;
        if !archived.0.archived_blocks.is_empty() { return Err("nested archive response unsupported".into()); }
        for block in archived.0.blocks {
            let id: u64 = block.id.0.try_into().map_err(|_| "archive block id exceeds u64".to_string())?;
            if !archive_coverage.contains(&id) || found.insert(id, block.block).is_some() {
                return Err("archive returned an unrequested or duplicate block".into());
            }
        }
    }
    validate_exact_page_ids(start, end, &found.keys().copied().collect::<Vec<_>>())?;
    for (id, block) in found {
        validate_scannable_block(&block)?;
        if block_matches_transfer(&block, transfer, pull, ic_cdk::id()) {
            return Ok(AbsencePage::Match(Nat::from(id)));
        }
    }
    if end == tip { Ok(AbsencePage::Complete) } else { Ok(AbsencePage::Continue(Nat::from(end))) }
}

fn validate_exact_page_ids(start: u64, end: u64, ids: &[u64]) -> Result<(), String> {
    if end < start || ids.len() as u64 != end - start {
        return Err("ledger/archive response does not completely cover requested page".into());
    }
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    if sorted.iter().enumerate().any(|(offset, id)| *id != start + offset as u64) {
        return Err("ledger/archive response contains a gap or duplicate block id".into());
    }
    Ok(())
}

/// Validate that a block is an understood transaction variant with enough
/// structure to prove it is not the persisted transfer. Unknown variants or
/// malformed transfer records must stop an absence proof, never count as a
/// non-match.
fn validate_scannable_block(block: &Icrc3Value) -> Result<(), String> {
    fn field<'a>(value: &'a Icrc3Value, name: &str) -> Option<&'a Icrc3Value> {
        match value { Icrc3Value::Map(fields) => fields.iter().find(|(key, _)| key == name).map(|(_, value)| value), _ => None }
    }
    fn is_nat(value: Option<&Icrc3Value>) -> bool { matches!(value, Some(Icrc3Value::Nat(_))) }
    fn is_blob(value: Option<&Icrc3Value>) -> bool { matches!(value, Some(Icrc3Value::Blob(_))) }
    fn is_account(value: Option<&Icrc3Value>) -> bool {
        matches!(value, Some(Icrc3Value::Array(parts)) if (1..=2).contains(&parts.len()) && matches!(parts.first(), Some(Icrc3Value::Blob(_))) && parts.get(1).map(|v| matches!(v, Icrc3Value::Blob(bytes) if bytes.len() == 32)).unwrap_or(true))
    }
    if !matches!(block, Icrc3Value::Map(_)) { return Err("unsupported non-map ledger block".into()); }
    let tx = field(block, "tx").ok_or("ledger block lacks transaction")?;
    let btype = match field(block, "btype") {
        Some(Icrc3Value::Text(value)) => Some(value.as_str()),
        None => None,
        _ => return Err("ledger block btype has an unsupported type".into()),
    };
    let op = match field(tx, "op") { Some(Icrc3Value::Text(value)) => value.as_str(), _ => return Err("ledger block lacks transaction op".into()) };
    match (btype, op) {
        (None, "xfer") | (Some("1xfer"), "xfer") | (Some("2xfer"), "xfer") => {
            if !is_account(field(tx, "from")) || !is_account(field(tx, "to"))
                // Absence proof is only safe when every field used by the
                // exact matcher is present. Treating a missing identity field
                // as a mismatch could retire an obligation whose ledger
                // history omits metadata needed to recognize its transfer.
                || !is_nat(field(tx, "amt")) || !is_nat(field(tx, "fee").or_else(|| field(block, "fee")))
                || !is_nat(field(tx, "ts")) || !is_blob(field(tx, "memo"))
                || (btype == Some("2xfer") && !is_account(field(tx, "spender")))
            { return Err("malformed transfer block cannot support absence proof".into()); }
        }
        (None, "mint") | (Some("1mint"), "mint") => {
            if !is_account(field(tx, "to")) || !is_nat(field(tx, "amt")) { return Err("malformed mint block".into()); }
        }
        (None, "burn") | (Some("1burn"), "burn") => {
            if !is_account(field(tx, "from")) || !is_nat(field(tx, "amt")) { return Err("malformed burn block".into()); }
        }
        (None, "approve") | (Some("2approve"), "approve") => {
            if !is_account(field(tx, "from")) || !is_account(field(tx, "spender")) || !is_nat(field(tx, "amt")) { return Err("malformed approve block".into()); }
        }
        _ => return Err("unknown or inconsistent ledger block type".into()),
    }
    Ok(())
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

pub async fn execute(transfer: &SwapTransferV1, pull: bool) -> Result<Nat, (TransferFailureClass, String)> {
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
            Ok((Err(e),)) => Err((classify_transfer_from_error(&e), format!("{e:?}"))),
            Err(e) => Err((TransferFailureClass::Ambiguous, format!("{e:?}"))),
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
            Ok((Err(e),)) => Err((classify_transfer_error(&e), format!("{e:?}"))),
            Err(e) => Err((TransferFailureClass::Ambiguous, format!("{e:?}"))),
        }
    }
}

fn transfer_error_is_ambiguous(error: &TransferError, prior_ambiguous: bool) -> bool {
    prior_ambiguous || matches!(classify_transfer_error(error), TransferFailureClass::Ambiguous | TransferFailureClass::TooOld)
}

fn transfer_from_error_is_ambiguous(error: &TransferFromError, prior_ambiguous: bool) -> bool {
    prior_ambiguous || matches!(classify_transfer_from_error(error), TransferFailureClass::Ambiguous | TransferFailureClass::TooOld)
}

fn classify_transfer_error(error: &TransferError) -> TransferFailureClass {
    match error {
        TransferError::TooOld => TransferFailureClass::TooOld,
        TransferError::GenericError { .. } | TransferError::TemporarilyUnavailable => TransferFailureClass::Ambiguous,
        _ => TransferFailureClass::ProvenNoEffect,
    }
}

fn classify_transfer_from_error(error: &TransferFromError) -> TransferFailureClass {
    match error {
        TransferFromError::TooOld => TransferFailureClass::TooOld,
        TransferFromError::GenericError { .. } | TransferFromError::TemporarilyUnavailable => TransferFailureClass::Ambiguous,
        _ => TransferFailureClass::ProvenNoEffect,
    }
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
    let (mut transfer, prior_ambiguous) = if let Some(saved) = existing {
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
        let prior_ambiguous = !saved.ready_to_dispatch.unwrap_or(false)
            && matches!(saved.status, SwapTransferStatusV1::Submitted | SwapTransferStatusV1::Unresolved);
        (saved.clone(), prior_ambiguous)
    } else {
        (proposed, false)
    };
    if transfer.history_start.is_none() {
        let history_start = match ledger_log_length(transfer.ledger).await {
            Ok(length) => length,
            Err(error) => return Err((true, error)),
        };
        transfer.history_start = Some(history_start);
    }
    transfer.ready_to_dispatch = None;
    transfer.dispatch_count = Some(transfer.dispatch_count.unwrap_or(0).saturating_add(1));
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
        Err((class, _)) => {
            let effective_ambiguous = prior_ambiguous
                || matches!(class, TransferFailureClass::Ambiguous | TransferFailureClass::TooOld);
            recorded.status = if effective_ambiguous {
                SwapTransferStatusV1::Unresolved
            } else {
                SwapTransferStatusV1::Rejected
            };
            if *class == TransferFailureClass::TooOld && transfer.dispatch_count.unwrap_or(0) > 1 {
                recorded.too_old_after_ambiguity = Some(true);
            }
        }
    }
    save(receipt);
    match result {
        Ok(_) => Ok(()),
        Err((class, reason)) => Err((
            prior_ambiguous || matches!(class, TransferFailureClass::Ambiguous | TransferFailureClass::TooOld),
            reason,
        )),
    }
}

/// Begin a fixed-tip absence scan only after a later exact retry of this
/// transfer generation received typed TooOld following a prior dispatch.
pub async fn begin_absence_scan(transfer: &SwapTransferV1) -> Result<AbsenceScanV1, String> {
    if transfer.too_old_after_ambiguity != Some(true) || transfer.dispatch_count.unwrap_or(0) < 2
        || transfer.status != SwapTransferStatusV1::Unresolved
    {
        return Err("transfer has no eligible TooOld-after-ambiguity proof".into());
    }
    // Legacy receipt rows have no pre-dispatch cursor. A genesis fallback is
    // safe but may be expensive; page limits keep each update bounded and any
    // archive gap leaves the obligation held.
    let baseline = transfer.history_start.clone().unwrap_or_else(|| Nat::from(0u8));
    let fixed_tip = ledger_log_length(transfer.ledger).await?;
    if fixed_tip < baseline { return Err("ledger log length predates the pre-dispatch baseline".into()); }
    Ok(AbsenceScanV1 { fixed_tip, cursor: baseline, generation: transfer.generation.unwrap_or(0) })
}

/// Retire an absent exact tuple only after a complete fixed-tip scan. The
/// chained hash preserves a compact permanent record of each prior identity.
pub fn rotate_absent_identity(transfer: &mut SwapTransferV1, fixed_tip: Nat) -> Result<(), String> {
    rotate_absent_identity_at(transfer, fixed_tip, ic_cdk::api::time())
}

fn rotate_absent_identity_at(transfer: &mut SwapTransferV1, fixed_tip: Nat, now: u64) -> Result<(), String> {
    let completed_scan = transfer.absence_scan.as_ref().ok_or("transfer lacks a completed absence scan")?;
    let prior_generation = transfer.generation.unwrap_or(0);
    if transfer.too_old_after_ambiguity != Some(true)
        || completed_scan.fixed_tip != fixed_tip
        || completed_scan.cursor != fixed_tip
        || completed_scan.generation != prior_generation
    {
        return Err("absence scan is not complete for the current transfer generation".into());
    }
    let next_generation = prior_generation.checked_add(1).ok_or("transfer generation exhausted")?;
    let old_time = transfer.created_at_time;
    let next_time = now.max(old_time.checked_add(1).ok_or("transfer timestamp exhausted")?);
    let identity = transfer_identity_hash(transfer);
    let mut retired = Sha256::new();
    retired.update(b"rumi-3pool-retired-transfer-chain-v1");
    if let Some(previous) = transfer.retired_identity_hash.as_ref() { retired.update(previous); }
    retired.update(identity);
    let retired_hash = retired.finalize().to_vec();
    let mut memo = Sha256::new();
    memo.update(b"rumi-3pool-transfer-generation-v1");
    memo.update(&retired_hash);
    memo.update(next_generation.to_be_bytes());
    memo.update(next_time.to_be_bytes());
    transfer.memo = memo.finalize().to_vec();
    transfer.created_at_time = next_time;
    transfer.generation = Some(next_generation);
    transfer.retired_identity_hash = Some(retired_hash);
    transfer.history_start = Some(fixed_tip.clone());
    transfer.absence_scan = None;
    transfer.too_old_after_ambiguity = Some(false);
    transfer.dispatch_count = Some(0);
    transfer.block_index = None;
    transfer.status = SwapTransferStatusV1::Unresolved;
    transfer.ready_to_dispatch = Some(true);
    Ok(())
}

fn transfer_identity_hash(transfer: &SwapTransferV1) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"rumi-3pool-transfer-identity-v1");
    hash.update(transfer.ledger.as_slice());
    hash.update(transfer.from.owner.as_slice());
    hash.update(transfer.from.subaccount.as_ref().map(|bytes| bytes.as_slice()).unwrap_or(&[]));
    hash.update(transfer.to.owner.as_slice());
    hash.update(transfer.to.subaccount.as_ref().map(|bytes| bytes.as_slice()).unwrap_or(&[]));
    hash.update(transfer.amount.to_be_bytes());
    hash.update(transfer.fee.to_be_bytes());
    hash.update(transfer.created_at_time.to_be_bytes());
    hash.update(&transfer.memo);
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_absence_profiles_remain_closed_without_live_hash_attestation() {
        for ledger in [
            "t6bor-paaaa-aaaap-qrd5q-cai",
            "cngnf-vqaaa-aaaar-qag4q-cai",
            "xevnm-gaaaa-aaaar-qafnq-cai",
        ] {
            assert!(!has_reviewed_ledger_lineage(
                Principal::from_text(ledger).expect("known ledger principal")
            ));
        }
    }
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
            history_start: None, absence_scan: None, generation: Some(0),
            dispatch_count: Some(0),
            too_old_after_ambiguity: Some(false), retired_identity_hash: None,
            ready_to_dispatch: None,
        };
        let account = |p: Principal| Icrc3Value::Array(vec![Icrc3Value::Blob(p.as_slice().to_vec())]);
        let block = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("1xfer".into())),
            ("tx".into(), Icrc3Value::Map(vec![
                ("op".into(), Icrc3Value::Text("xfer".into())),
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
        assert!(validate_scannable_block(&block).is_ok());
        let mut btype_less_xfer = block.clone();
        if let Icrc3Value::Map(fields) = &mut btype_less_xfer {
            fields.retain(|(key, _)| key != "btype");
        }
        assert!(validate_scannable_block(&btype_less_xfer).is_ok());
        assert!(block_matches_transfer(&btype_less_xfer, &expected, false, pool));
        let btype_less_mint = Icrc3Value::Map(vec![
            ("tx".into(), Icrc3Value::Map(vec![
                ("op".into(), Icrc3Value::Text("mint".into())),
                ("to".into(), account(user)),
                ("amt".into(), Icrc3Value::Nat(Nat::from(5u8))),
            ])),
        ]);
        assert!(validate_scannable_block(&btype_less_mint).is_ok());
        let unknown = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("9future".into())),
            ("tx".into(), Icrc3Value::Map(vec![("op".into(), Icrc3Value::Text("future".into()))])),
        ]);
        assert!(validate_scannable_block(&unknown).is_err());
        // Same accounts and amount as the saved transfer, but no memo or
        // created-at timestamp. It cannot be treated as a non-match in an
        // absence scan because it may be an incomplete ledger encoding of
        // that exact transfer.
        let missing_identity_fields = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("1xfer".into())),
            ("tx".into(), Icrc3Value::Map(vec![
                ("op".into(), Icrc3Value::Text("xfer".into())),
                ("fee".into(), Icrc3Value::Nat(Nat::from(10u8))),
                ("from".into(), account(pool)),
                ("to".into(), account(user)),
                ("amt".into(), Icrc3Value::Nat(Nat::from(900u16))),
            ])),
        ]);
        assert!(!block_matches_transfer(&missing_identity_fields, &expected, false, pool));
        assert!(validate_scannable_block(&missing_identity_fields).is_err());
        let mut malformed = block;
        if let Icrc3Value::Map(fields) = &mut malformed {
            if let Some((_, Icrc3Value::Map(tx))) = fields.iter_mut().find(|(key, _)| key == "tx") {
                tx.iter_mut().find(|(key, _)| key == "memo").unwrap().1 = Icrc3Value::Nat(Nat::from(1u8));
            }
        }
        assert!(validate_scannable_block(&malformed).is_err());
    }

    #[test]
    fn newly_optional_receipt_fields_decode_legacy_transfer_rows() {
        #[derive(CandidType)]
        struct OldTransfer {
            ledger: Principal,
            from: Account,
            to: Account,
            amount: u128,
            fee: u128,
            created_at_time: u64,
            memo: Vec<u8>,
            block_index: Option<Nat>,
            status: SwapTransferStatusV1,
        }
        let old = OldTransfer {
            ledger: Principal::management_canister(),
            from: Account { owner: Principal::anonymous(), subaccount: None },
            to: Account { owner: Principal::management_canister(), subaccount: None },
            amount: 1,
            fee: 2,
            created_at_time: 3,
            memo: vec![4],
            block_index: None,
            status: SwapTransferStatusV1::Unresolved,
        };
        let encoded = candid::encode_one(old).unwrap();
        let decoded: SwapTransferV1 = candid::decode_one(&encoded).unwrap();
        assert_eq!(decoded.generation, None);
        assert_eq!(decoded.dispatch_count, None);
        assert_eq!(decoded.too_old_after_ambiguity, None);
        assert_eq!(decoded.history_start, None);
        assert_eq!(decoded.absence_scan, None);
    }

    #[test]
    fn completed_absence_rotation_keeps_tombstone_and_changes_exact_dedup_tuple() {
        let mut transfer = SwapTransferV1 {
            ledger: Principal::management_canister(),
            from: Account { owner: Principal::anonymous(), subaccount: None },
            to: Account { owner: Principal::self_authenticating(b"to"), subaccount: None },
            amount: 11,
            fee: 2,
            created_at_time: 50,
            memo: vec![5; 32],
            block_index: None,
            status: SwapTransferStatusV1::Unresolved,
            history_start: Some(Nat::from(9u8)),
            absence_scan: Some(AbsenceScanV1 { fixed_tip: Nat::from(20u8), cursor: Nat::from(20u8), generation: 0 }),
            generation: Some(0),
            dispatch_count: Some(2),
            too_old_after_ambiguity: Some(true),
            retired_identity_hash: None,
            ready_to_dispatch: None,
        };
        let old_time = transfer.created_at_time;
        let old_memo = transfer.memo.clone();
        rotate_absent_identity_at(&mut transfer, Nat::from(20u8), 40).unwrap();
        assert_eq!(transfer.status, SwapTransferStatusV1::Unresolved);
        assert_eq!(transfer.ready_to_dispatch, Some(true));
        assert_eq!(transfer.generation, Some(1));
        assert_eq!(transfer.created_at_time, old_time + 1);
        assert_ne!(transfer.memo, old_memo);
        assert!(transfer.retired_identity_hash.is_some());
        assert_eq!(transfer.history_start, Some(Nat::from(20u8)));
        assert_eq!(transfer.dispatch_count, Some(0));
        assert_eq!(transfer.too_old_after_ambiguity, Some(false));
        assert_eq!(transfer.absence_scan, None);
    }

    #[test]
    fn absence_page_requires_exact_contiguous_unique_ids() {
        assert!(validate_exact_page_ids(10, 13, &[10, 11, 12]).is_ok());
        assert!(validate_exact_page_ids(10, 13, &[10, 12]).is_err());
        assert!(validate_exact_page_ids(10, 13, &[10, 11, 11]).is_err());
        assert!(validate_exact_page_ids(10, 13, &[10, 11, 13]).is_err());
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
            history_start: None, absence_scan: None, generation: Some(0),
            dispatch_count: Some(0),
            too_old_after_ambiguity: Some(false), retired_identity_hash: None,
            ready_to_dispatch: None,
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
    fn payout_receipt_matches_net_ledger_amount_not_claim_debit() {
        let pool = Principal::self_authenticating(b"payout-pool");
        let claimant = Principal::self_authenticating(b"payout-user");
        let claim_debit = 910u128;
        let fee = 10u128;
        let expected = SwapTransferV1 {
            ledger: Principal::management_canister(),
            from: Account { owner: pool, subaccount: None },
            to: Account { owner: claimant, subaccount: None },
            amount: claim_debit.checked_sub(fee).unwrap(),
            fee,
            created_at_time: 55,
            memo: vec![6; 32],
            block_index: None,
            status: SwapTransferStatusV1::Unresolved,
            history_start: None, absence_scan: None, generation: Some(0),
            dispatch_count: Some(0), too_old_after_ambiguity: Some(false), retired_identity_hash: None,
            ready_to_dispatch: None,
        };
        let account = |principal: Principal| Icrc3Value::Array(vec![
            Icrc3Value::Blob(principal.as_slice().to_vec()),
        ]);
        let block = Icrc3Value::Map(vec![
            ("btype".into(), Icrc3Value::Text("1xfer".into())),
            ("tx".into(), Icrc3Value::Map(vec![
                ("amt".into(), Icrc3Value::Nat(Nat::from(900u16))),
                ("fee".into(), Icrc3Value::Nat(Nat::from(10u8))),
                ("from".into(), account(pool)),
                ("to".into(), account(claimant)),
                ("memo".into(), Icrc3Value::Blob(vec![6; 32])),
                ("ts".into(), Icrc3Value::Nat(Nat::from(55u8))),
            ])),
        ]);
        assert!(block_matches_transfer(&block, &expected, false, pool));
        let mut wrong_gross = expected;
        wrong_gross.amount = claim_debit;
        assert!(!block_matches_transfer(&block, &wrong_gross, false, pool));
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
            history_start: None, absence_scan: None, generation: Some(0),
            dispatch_count: Some(0),
            too_old_after_ambiguity: Some(false), retired_identity_hash: None,
            ready_to_dispatch: None,
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
