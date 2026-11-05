//! Private issuer-history observer and the wallet-hint path (Task 5).
//!
//! Two doors, ONE consumer. A wallet-reported hash (`observe_xrpl_repayment`) and the
//! paged `account_tx` scan of the issuer both end in
//! [`super::repay::apply_verified_return`], which deduplicates on
//! `(network, issuer, tx hash)`: the same return found twice applies once.
//!
//! The observer is idle-free. It exists only while a repayment intent is open
//! ([`XrplIcusdState::observer_has_work`]); with none, a pass returns BEFORE any
//! outcall and the tick stops. The durable cursor survives the quiet period, so a
//! later intent resumes exactly where history was last fully processed (an unknown
//! externally initiated return is discovered then, not instantly).
//!
//! Safety rules:
//! * the cursor advances only after EVERY transaction of the page was durably
//!   handled; a history gap, a provider disagreement or a capacity limit leaves it
//!   where it was and has no financial effect;
//! * each provider's `complete_ledgers` is verified inside `fetch_issuer_history`
//!   before a page can exist at all;
//! * a page is a DISCOVERY fact: every payment of our asset is re-confirmed through
//!   the quorum receipt lookup before any value moves;
//! * batches are bounded ([`PAGE_LIMIT`] transactions per pass).

use std::cell::Cell;

use candid::Principal;

use super::repay::{self, ReturnOutcome};
use super::types::*;
use crate::chains::xrp::issued_amount::IssuedAsset;
use crate::chains::xrp::issued_rpc::{
    self, normalize_tx_hash, HistoryProgress, IssuedRpcError, IssuerHistoryCursor,
    IssuerHistoryPage, ParsedTx, RawAmount, ReceiptError, ReceiptLookup,
};
use crate::chains::xrp::network::XrplNetwork;
use crate::state::{mutate_state, read_state};

/// Transactions processed per observer pass (and the `account_tx` page size).
pub const PAGE_LIMIT: u32 = 50;

thread_local! {
    static OBSERVER_BUSY: Cell<bool> = const { Cell::new(false) };
}

#[must_use]
struct ObserverGuard;

impl ObserverGuard {
    fn acquire() -> Option<Self> {
        if OBSERVER_BUSY.with(|b| b.replace(true)) {
            None
        } else {
            Some(ObserverGuard)
        }
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        OBSERVER_BUSY.with(|b| b.set(false));
    }
}

// ----------------------------------------------------------------------------------
// Transport seam (production = HTTPS outcalls; tests inject outcomes)
// ----------------------------------------------------------------------------------

#[allow(async_fn_in_trait)]
pub trait ObserverTransport {
    /// Newest validated ledger index (one `account_info` quorum read).
    async fn validated_tip(&self, network: XrplNetwork, issuer: &str) -> Result<u32, IssuedRpcError>;
    /// One bounded page of issuer history for the cursor's fixed interval. Providers'
    /// `complete_ledgers` are checked here; an uncovered interval is `HistoryGap`.
    async fn history(
        &self,
        network: XrplNetwork,
        issuer: &str,
        cursor: &IssuerHistoryCursor,
        limit: u32,
    ) -> Result<IssuerHistoryPage, IssuedRpcError>;
    /// Quorum-verified receipt of one transaction.
    async fn receipt(
        &self,
        network: XrplNetwork,
        tx_hash: &str,
        asset: &IssuedAsset,
    ) -> Result<ReceiptLookup, IssuedRpcError>;
}

pub struct IcTransport;

impl ObserverTransport for IcTransport {
    async fn validated_tip(&self, network: XrplNetwork, issuer: &str) -> Result<u32, IssuedRpcError> {
        crate::chains::xrp::issuer_rpc::fetch_issuer_account(network, issuer)
            .await
            .map(|i| i.ledger_index)
    }
    async fn history(
        &self,
        network: XrplNetwork,
        issuer: &str,
        cursor: &IssuerHistoryCursor,
        limit: u32,
    ) -> Result<IssuerHistoryPage, IssuedRpcError> {
        issued_rpc::fetch_issuer_history(network, issuer, cursor, limit).await
    }
    async fn receipt(
        &self,
        network: XrplNetwork,
        tx_hash: &str,
        asset: &IssuedAsset,
    ) -> Result<ReceiptLookup, IssuedRpcError> {
        issued_rpc::fetch_issued_receipt(network, tx_hash, asset).await
    }
}

// ----------------------------------------------------------------------------------
// Page classification and application (pure)
// ----------------------------------------------------------------------------------

/// What the page's transactions need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PagePlan {
    /// Payments of OUR asset to the issuer that are not yet consumed: confirm each.
    pub to_verify: Vec<String>,
    /// Issued-currency payments of another asset (evidence only; no value, no effect).
    pub foreign_asset: Vec<(String, u32, String, Option<u32>)>,
}

fn is_our_asset(amount: &RawAmount, asset: &IssuedAsset) -> Option<bool> {
    match amount {
        RawAmount::Issued {
            currency, issuer, ..
        } => Some(asset.check_identity(currency, issuer).is_ok()),
        _ => None,
    }
}

/// Classify one history page. Only successful `Payment`s to the issuer from another
/// account matter. Native XRP (funding) is expected traffic and ignored silently.
pub fn plan_page(
    st: &super::state::XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    asset: &IssuedAsset,
    page: &IssuerHistoryPage,
) -> PagePlan {
    let mut plan = PagePlan::default();
    let mut seen = std::collections::BTreeSet::new();
    for tx in &page.txs {
        if !relevant(tx, issuer) {
            continue;
        }
        // Below the dedup floor everything was already settled by the scan (see
        // `retention`): the record may be compacted, the receipt is never new.
        if tx.ledger_index < st.dedup_floor_ledger {
            continue;
        }
        // `delivered` is the truth for what moved; fall back to the nominal amount
        // only to decide whether it is worth a receipt lookup.
        let ours = is_our_asset(&tx.delivered, asset).or_else(|| is_our_asset(&tx.amount, asset));
        match ours {
            Some(true) => {
                let key = repay::processed_key(network, issuer, &tx.hash);
                if !st.processed_returns.contains_key(&key) && seen.insert(key) {
                    plan.to_verify.push(tx.hash.clone());
                }
            }
            Some(false) => plan.foreign_asset.push((
                tx.hash.clone(),
                tx.ledger_index,
                tx.account.clone(),
                tx.destination_tag,
            )),
            None => {}
        }
    }
    plan
}

fn relevant(tx: &ParsedTx, issuer: &str) -> bool {
    tx.tx_type == "Payment"
        && tx.engine_result == "tesSUCCESS"
        && tx.destination.as_deref() == Some(issuer)
        && tx.account != issuer
}

/// How one page ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageResult {
    /// Operations that gained a verified return (to settle now).
    pub affected: Vec<OperationId>,
    /// Every transaction of the page was durably handled: the cursor may advance.
    pub complete: bool,
    pub note: Option<String>,
}

/// Apply fetched receipts and move the durable cursor if (and only if) the whole page
/// was handled. `fetched` is `(hash, lookup)` for each hash of `plan.to_verify` that
/// was reached.
pub fn apply_page(
    st: &mut super::state::XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    cursor: &IssuerHistoryCursor,
    page: &IssuerHistoryPage,
    plan: &PagePlan,
    fetched: Vec<(String, Result<ReceiptLookup, IssuedRpcError>)>,
    now_ns: u64,
) -> PageResult {
    let mut affected = Vec::new();
    let mut complete = fetched.len() >= plan.to_verify.len();
    let mut note = None;
    for (hash, rec) in fetched {
        match rec {
            Ok(ReceiptLookup::Verified(r)) => {
                match repay::apply_verified_return(st, network, issuer, &r, now_ns) {
                    Ok(ReturnOutcome::Attributed { op_id }) => affected.push(op_id),
                    Ok(ReturnOutcome::Merged { parent, .. }) => affected.push(parent),
                    Ok(_) => {}
                    Err(e) => {
                        complete = false;
                        note = Some(format!("return {hash} deferred: {e}"));
                    }
                }
            }
            // Validated history said it exists; a provider disagreement is retried.
            Ok(ReceiptLookup::NotFound) | Ok(ReceiptLookup::NotValidated) => {
                complete = false;
                note = Some(format!("receipt {hash} not yet confirmed by the providers"));
            }
            // A failed/non-payment/foreign receipt carries no value.
            Ok(ReceiptLookup::Failed(res)) => st.log_quarantine(QuarantineRecord {
                tx_hash: hash,
                ledger_index: None,
                sender: None,
                destination_tag: None,
                reason: format!("receipt result {res}"),
                at_ns: now_ns,
            }),
            Err(IssuedRpcError::Receipt(
                e @ (ReceiptError::WrongAsset(_)
                | ReceiptError::NotPayment(_)
                | ReceiptError::DeliveredAmountUnavailable
                | ReceiptError::InvalidAmount(_)),
            )) => st.log_quarantine(QuarantineRecord {
                tx_hash: hash,
                ledger_index: None,
                sender: None,
                destination_tag: None,
                reason: format!("unusable receipt: {e:?}"),
                at_ns: now_ns,
            }),
            Err(e) => {
                complete = false;
                note = Some(format!("receipt {hash} unavailable: {e}"));
            }
        }
    }
    if complete {
        for (hash, ledger, sender, tag) in &plan.foreign_asset {
            st.log_quarantine(QuarantineRecord {
                tx_hash: hash.to_ascii_uppercase(),
                ledger_index: Some(*ledger),
                sender: Some(sender.clone()),
                destination_tag: *tag,
                reason: "issued payment of a different currency or issuer".into(),
                at_ns: now_ns,
            });
        }
        // `progress` drives the tick back-off: a pass that only walked empty history
        // is NOT progress (the chain always advances), but a pass that found activity,
        // or that has more pages of the same interval to read, is.
        let mut made_progress = !plan.to_verify.is_empty() || !affected.is_empty();
        match cursor.advance(page) {
            HistoryProgress::Continue(c) => {
                st.observer.cursor = Some(c);
                made_progress = true;
            }
            HistoryProgress::IntervalComplete { next_ledger_min } => {
                st.observer.cursor = None;
                st.observer.next_ledger_min = Some(next_ledger_min);
            }
        }
        if made_progress {
            st.observer.progress += 1;
        }
    }
    st.observer.last_pass_ns = now_ns;
    st.observer.last_note = note.clone();
    PageResult {
        affected,
        complete,
        note,
    }
}

// ----------------------------------------------------------------------------------
// One observer pass
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassOutcome {
    /// No open intent: returned BEFORE any outcall.
    Idle,
    /// Another pass is running.
    Busy,
    /// The durable cursor is at the validated tip: nothing new.
    CaughtUp,
    /// A page was processed and the cursor moved (settle `affected`).
    Progressed { affected: Vec<OperationId>, txs: usize },
    /// A recoverable condition (history gap, provider disagreement, a return that
    /// could not be applied yet): no cursor advance, no financial effect beyond what
    /// was already durably applied.
    Recoverable(String),
}

/// Plan the next pass without any outcall. `None` = idle.
fn plan_pass(now_ns: u64) -> Option<(XrplNetwork, String, u32)> {
    mutate_state(|s| {
        repay::expire_stale_intents(&mut s.xrpl_icusd, now_ns);
        let st = &s.xrpl_icusd;
        if !st.observer_has_work() {
            return None;
        }
        let network = st.config.network?;
        let issuer = st.config.issuer_address.clone()?;
        let start = match st.config.issuer_setup {
            IssuerSetupState::Ready { ledger_index } => ledger_index.max(1),
            _ => 1,
        };
        Some((network, issuer, start))
    })
}

pub async fn observer_pass_with<T: ObserverTransport>(t: &T, now_ns: u64) -> PassOutcome {
    let Some((network, issuer, ready_ledger)) = plan_pass(now_ns) else {
        return PassOutcome::Idle;
    };
    let Some(_guard) = ObserverGuard::acquire() else {
        return PassOutcome::Busy;
    };
    let asset = match IssuedAsset::icusd(&issuer) {
        Ok(a) => a,
        Err(e) => return PassOutcome::Recoverable(format!("issuer asset: {e}")),
    };
    // Resume the in-progress interval, or open a new one up to the validated tip.
    let cursor = match read_state(|s| s.xrpl_icusd.observer.cursor.clone()) {
        Some(c) => c,
        None => {
            let tip = match t.validated_tip(network, &issuer).await {
                Ok(tip) => tip,
                Err(e) => return PassOutcome::Recoverable(format!("validated tip: {e}")),
            };
            let min = read_state(|s| s.xrpl_icusd.observer.next_ledger_min).unwrap_or(ready_ledger);
            if min > tip {
                mutate_state(|s| {
                    let st = &mut s.xrpl_icusd;
                    st.observer.last_pass_ns = now_ns;
                    // Caught up on the coverage record that asked for the scan: do
                    // not poll again until the next coverage check.
                    if let Some(c) = st.last_coverage.as_ref() {
                        st.observer.settled_coverage_ns =
                            st.observer.settled_coverage_ns.max(c.checked_at_ns);
                    }
                });
                return PassOutcome::CaughtUp;
            }
            let c = match IssuerHistoryCursor::new(min, tip) {
                Ok(c) => c,
                Err(e) => return PassOutcome::Recoverable(format!("cursor: {e}")),
            };
            mutate_state(|s| s.xrpl_icusd.observer.cursor = Some(c.clone()));
            c
        }
    };
    let page = match t.history(network, &issuer, &cursor, PAGE_LIMIT).await {
        Ok(p) => p,
        Err(e) => {
            // HistoryGap and friends: typed, recoverable, NO cursor advance.
            mutate_state(|s| {
                s.xrpl_icusd.observer.last_pass_ns = now_ns;
                s.xrpl_icusd.observer.last_note = Some(format!("history: {e}"));
            });
            return PassOutcome::Recoverable(format!("history: {e}"));
        }
    };
    let txs = page.txs.len();
    let plan = read_state(|s| plan_page(&s.xrpl_icusd, network, &issuer, &asset, &page));
    let mut fetched = Vec::new();
    for hash in &plan.to_verify {
        let r = t.receipt(network, hash, &asset).await;
        let stop = r.is_err();
        fetched.push((hash.clone(), r));
        if stop {
            break;
        }
    }
    let result = mutate_state(|s| {
        apply_page(
            &mut s.xrpl_icusd,
            network,
            &issuer,
            &cursor,
            &page,
            &plan,
            fetched,
            now_ns,
        )
    });
    if result.complete {
        PassOutcome::Progressed {
            affected: result.affected,
            txs,
        }
    } else {
        PassOutcome::Recoverable(result.note.unwrap_or_else(|| "page deferred".into()))
    }
}

/// Production pass: the tick calls this; it settles whatever it discovered.
pub async fn observer_pass() -> PassOutcome {
    let out = observer_pass_with(&IcTransport, super::now()).await;
    if let PassOutcome::Progressed { affected, .. } = &out {
        for id in affected {
            let _ = super::run_op_step(*id).await;
        }
    }
    out
}

// ----------------------------------------------------------------------------------
// Wallet hint
// ----------------------------------------------------------------------------------

/// The hint path: a wallet-reported hash is only a lookup key. The receipt is read
/// and verified by the backend, then consumed through the same
/// [`repay::apply_verified_return`] as the scan (so hint + scan apply once).
pub async fn observe_with<T: ObserverTransport>(
    t: &T,
    caller: Principal,
    arg: &ObserveRepaymentArg,
    now_ns: u64,
) -> Result<(OperationView, Vec<OperationId>), XrplIcusdError> {
    if caller == Principal::anonymous() {
        return Err(XrplIcusdError::AnonymousCaller);
    }
    let hash = normalize_tx_hash(&arg.tx_hash)
        .map_err(|_| XrplIcusdError::InvalidArgument("tx_hash must be 64 hex characters".into()))?;
    let (network, issuer) = read_state(|s| {
        let st = &s.xrpl_icusd;
        let op = st.op_for_owner(&caller, arg.operation_id)?;
        if !matches!(op.kind, OperationKind::Repayment { .. }) {
            return Err(XrplIcusdError::NotApplicable("not a repayment".into()));
        }
        st.ensure_ready()?;
        Ok((
            op.network,
            st.config
                .issuer_address
                .clone()
                .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?,
        ))
    })?;
    let consumed = read_state(|s| {
        s.xrpl_icusd
            .processed_returns
            .contains_key(&repay::processed_key(network, &issuer, &hash))
    });
    let mut affected = Vec::new();
    if !consumed
        && super::throttle_ok(
            super::THROTTLE_KIND_OBSERVE,
            &arg.operation_id.to_be_bytes(),
            now_ns,
            super::OBSERVE_GAP_NS,
        )
    {
        let asset = IssuedAsset::icusd(&issuer).map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
        let lookup = t.receipt(network, &hash, &asset).await;
        affected = mutate_state(|s| -> Result<Vec<OperationId>, XrplIcusdError> {
            let st = &mut s.xrpl_icusd;
            match lookup {
                Ok(ReceiptLookup::Verified(r)) => {
                    match repay::apply_verified_return(st, network, &issuer, &r, now_ns)? {
                        ReturnOutcome::Attributed { op_id } => Ok(vec![op_id]),
                        ReturnOutcome::Merged { parent, .. } => Ok(vec![parent]),
                        _ => Ok(vec![]),
                    }
                }
                Ok(ReceiptLookup::NotFound) | Ok(ReceiptLookup::NotValidated) => {
                    if let Ok(op) = st.op_mut(arg.operation_id) {
                        op.push_note("hinted transaction is not validated yet");
                    }
                    Ok(vec![])
                }
                Ok(ReceiptLookup::Failed(res)) => {
                    st.log_quarantine(QuarantineRecord {
                        tx_hash: hash.clone(),
                        ledger_index: None,
                        sender: None,
                        destination_tag: None,
                        reason: format!("hinted transaction result {res}"),
                        at_ns: now_ns,
                    });
                    Ok(vec![])
                }
                Err(IssuedRpcError::Receipt(e)) => {
                    st.log_quarantine(QuarantineRecord {
                        tx_hash: hash.clone(),
                        ledger_index: None,
                        sender: None,
                        destination_tag: None,
                        reason: format!("hinted transaction unusable: {e:?}"),
                        at_ns: now_ns,
                    });
                    Ok(vec![])
                }
                Err(e) => Err(XrplIcusdError::Unavailable(e.to_string())),
            }
        })?;
    }
    super::ensure_xrpl_tick_scheduled();
    Ok((super::view_for(&caller, arg.operation_id)?, affected))
}

/// `observe_xrpl_repayment` (production transport).
pub async fn observe_xrpl_repayment(
    caller: Principal,
    arg: ObserveRepaymentArg,
) -> Result<OperationView, XrplIcusdError> {
    let (view, affected) = observe_with(&IcTransport, caller, &arg, super::now()).await?;
    for id in affected {
        let _ = super::run_op_step(id).await;
    }
    Ok(super::view_for(&caller, view.operation_id).unwrap_or(view))
}

// ----------------------------------------------------------------------------------
// Controller reconciliation of quarantined value-bearing receipts
// ----------------------------------------------------------------------------------

/// Re-verified receipt vs a held record: a validated tesSUCCESS payment to the issuer
/// from the recorded sender at the recorded ledger, delivering exactly the recorded
/// amount, whose own metadata shows the issuer's obligations shrinking by exactly that
/// amount (the sum of the holders' balance changes on lines with the issuer).
fn verify_reconcile_receipt(
    r: &crate::chains::xrp::issued_rpc::VerifiedIssuedReceipt,
    issuer: &str,
    sender: &str,
    ledger_index: u32,
    amount: &WideAmount,
) -> Result<(), XrplIcusdError> {
    if !r.validated || r.engine_result != "tesSUCCESS" || r.destination != issuer {
        return Err(XrplIcusdError::InvalidArgument(
            "the re-verified receipt is not a validated tesSUCCESS payment to the issuer".into(),
        ));
    }
    if r.account != sender || r.ledger_index != ledger_index {
        return Err(XrplIcusdError::InvalidArgument(
            "the re-verified receipt differs from the quarantined record (sender or ledger)".into(),
        ));
    }
    let delivered = WideAmount::from_exact(&r.delivered_amount)
        .map_err(|_| XrplIcusdError::InvalidArgument("delivered amount is not representable".into()))?;
    if delivered != *amount {
        return Err(XrplIcusdError::InvalidArgument(
            "the re-verified delivered amount differs from the quarantined amount".into(),
        ));
    }
    // The issuer's obligations must have shrunk by exactly the delivered amount: the
    // sum of the holders' balance changes on lines with the issuer.
    let mut holder_delta = crate::chains::xrp::issued_amount::ExactIssuedAmount::ZERO;
    let mut seen = false;
    for c in &r.balance_changes {
        let holder = if c.low_account == issuer {
            &c.high_account
        } else if c.high_account == issuer {
            &c.low_account
        } else {
            continue;
        };
        if let Some(d) = c.delta_for(holder) {
            holder_delta = holder_delta
                .checked_add(&d)
                .map_err(|_| XrplIcusdError::Internal("balance change overflow".into()))?;
            seen = true;
        }
    }
    if !seen || holder_delta != r.delivered_amount.negated() {
        return Err(XrplIcusdError::InvalidArgument(
            "the receipt metadata does not show the issuer's obligations shrinking by exactly the \
             delivered amount; it stays quarantined"
                .into(),
        ));
    }
    Ok(())
}

/// Pure booking of a RE-VERIFIED receipt for a held quarantine entry. The amount is
/// never supplied by the caller: it must equal what the quarantine recorded AND what
/// the receipt's own metadata shows leaving the holders' lines of the issuer (so the
/// issuer's obligations shrank by exactly that amount). The value moves from the
/// quarantine bucket into the verified sender's refundable aggregate: `O` and the
/// liability sum are untouched (it was taken out of `O` when it was quarantined), and
/// no debt is ever credited.
pub fn apply_quarantine_reconcile(
    st: &mut super::state::XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    r: &crate::chains::xrp::issued_rpc::VerifiedIssuedReceipt,
    now_ns: u64,
) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let hash = r.hash.to_ascii_uppercase();
    let entry = st
        .quarantined_receipts
        .get(&hash)
        .cloned()
        .ok_or(XrplIcusdError::NotFound)?;
    verify_reconcile_receipt(r, issuer, &entry.sender, entry.ledger_index, &entry.amount)?;
    st.fold_unattributed(
        network,
        &entry.sender,
        &entry.amount,
        &entry.tx_hash,
        entry.ledger_index,
        "booked after controller reconciliation of a quarantined receipt",
        now_ns,
    )?;
    st.quarantined_receipts.remove(&hash);
    let key = repay::processed_key(network, issuer, &hash);
    if let Some(p) = st.processed_returns.get_mut(&key) {
        p.disposition = ReturnDisposition::Aggregated {
            reason: "reconciled quarantined receipt".into(),
        };
    }
    Ok(QuarantinedReceiptView::from(&entry))
}

/// Controller-only: re-read the quarantined transaction through the quorum lookup and
/// book it (see [`apply_quarantine_reconcile`]). There is no assertion-only path.
pub async fn reconcile_quarantined_with<T: ObserverTransport>(
    t: &T,
    tx_hash: &str,
    now_ns: u64,
) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let hash = normalize_tx_hash(tx_hash)
        .map_err(|_| XrplIcusdError::InvalidArgument("tx_hash must be 64 hex characters".into()))?;
    let (network, issuer) = read_state(|s| {
        let st = &s.xrpl_icusd;
        if !st.quarantined_receipts.contains_key(&hash.to_ascii_uppercase()) {
            return Err(XrplIcusdError::NotFound);
        }
        Ok((
            st.config
                .network
                .ok_or_else(|| XrplIcusdError::Disabled("network not configured".into()))?,
            st.config
                .issuer_address
                .clone()
                .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?,
        ))
    })?;
    let asset = IssuedAsset::icusd(&issuer).map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
    match t.receipt(network, &hash, &asset).await {
        Ok(ReceiptLookup::Verified(r)) => {
            mutate_state(|s| apply_quarantine_reconcile(&mut s.xrpl_icusd, network, &issuer, &r, now_ns))
        }
        Ok(_) => Err(XrplIcusdError::Unavailable(
            "the transaction could not be re-verified by the providers right now".into(),
        )),
        Err(e) => Err(XrplIcusdError::Unavailable(e.to_string())),
    }
}

pub async fn reconcile_quarantined_receipt(tx_hash: String) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let out = reconcile_quarantined_with(&IcTransport, &tx_hash, super::now()).await?;
    super::ensure_xrpl_tick_scheduled();
    Ok(out)
}

// ----------------------------------------------------------------------------------
// Controller reconciliation of OVERFLOW contributions
// ----------------------------------------------------------------------------------

/// Pure booking of a RE-VERIFIED receipt that joined an overflow bucket (the
/// unattributed overflow aggregate or the overflow quarantine entry). The recorded
/// per-transaction evidence is consumed exactly once: the receipt must re-verify against
/// it (sender, ledger, amount, issuer obligations shrinking by exactly that amount) and
/// then exactly that amount moves from the overflow bucket into the verified sender's
/// own attributed aggregate (refundable when it reaches the threshold). `O` and the
/// liability sum are untouched. If the sender cannot get its own slot (every slot is a
/// refundable aggregate) nothing changes and the call can be retried after refunds drain.
pub fn apply_overflow_reconcile(
    st: &mut super::state::XrplIcusdState,
    network: XrplNetwork,
    issuer: &str,
    r: &crate::chains::xrp::issued_rpc::VerifiedIssuedReceipt,
    now_ns: u64,
) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let hash = r.hash.to_ascii_uppercase();
    let ev = st
        .overflow_evidence
        .get(&hash)
        .cloned()
        .ok_or(XrplIcusdError::NotFound)?;
    if ev.network != network {
        return Err(XrplIcusdError::InvalidArgument(
            "the evidence belongs to another network".into(),
        ));
    }
    verify_reconcile_receipt(r, issuer, &ev.sender, ev.ledger_index, &ev.amount)?;
    // The bucket must still hold at least this contribution.
    let bucket_ok = match ev.kind {
        OverflowKind::Unattributed => st
            .unattributed
            .get(&super::retention::aggregate_key(network, OVERFLOW_SENDER))
            .is_some_and(|u| u.total >= ev.amount),
        OverflowKind::Quarantine => st
            .quarantined_receipts
            .get(OVERFLOW_SENDER)
            .is_some_and(|q| q.amount >= ev.amount),
    };
    if !bucket_ok {
        return Err(XrplIcusdError::Internal(
            "the overflow bucket holds less than the recorded contribution".into(),
        ));
    }
    // Book into the sender's own slot FIRST (refuses without touching state when no slot
    // is available), then take the same amount out of the bucket.
    let overflowed = st.fold_unattributed_inner(
        network,
        &ev.sender,
        &ev.amount,
        &ev.tx_hash,
        ev.ledger_index,
        "booked after controller reconciliation of an overflow receipt",
        now_ns,
        false,
    )?;
    debug_assert!(!overflowed);
    match ev.kind {
        OverflowKind::Unattributed => {
            let key = super::retention::aggregate_key(network, OVERFLOW_SENDER);
            if let Some(u) = st.unattributed.get_mut(&key) {
                u.total = u.total.checked_sub(&ev.amount).unwrap_or(WideAmount::ZERO);
                u.tx_count = u.tx_count.saturating_sub(1);
                u.updated_at_ns = now_ns;
                if u.total.is_zero() {
                    st.unattributed.remove(&key);
                }
            }
        }
        OverflowKind::Quarantine => {
            if let Some(q) = st.quarantined_receipts.get_mut(OVERFLOW_SENDER) {
                q.amount = q.amount.checked_sub(&ev.amount).unwrap_or(WideAmount::ZERO);
                if q.amount.is_zero() {
                    st.quarantined_receipts.remove(OVERFLOW_SENDER);
                }
            }
        }
    }
    st.overflow_evidence.remove(&hash);
    let key = repay::processed_key(network, issuer, &hash);
    if let Some(p) = st.processed_returns.get_mut(&key) {
        p.disposition = ReturnDisposition::Aggregated {
            reason: "reconciled overflow receipt".into(),
        };
    }
    Ok(overflow_view(&ev))
}

/// Candid view of one evidence entry (`reason` names the bucket).
pub fn overflow_view(ev: &OverflowEvidence) -> QuarantinedReceiptView {
    QuarantinedReceiptView {
        tx_hash: ev.tx_hash.clone(),
        ledger_index: ev.ledger_index,
        sender: ev.sender.clone(),
        destination_tag: ev.destination_tag,
        amount: ev.amount.to_decimal_string(),
        reason: match ev.kind {
            OverflowKind::Unattributed => "overflow: unattributed aggregate".to_string(),
            OverflowKind::Quarantine => "overflow: quarantine".to_string(),
        },
        at_ns: ev.at_ns,
    }
}

/// Controller-only: re-read the overflow receipt through the quorum lookup and move it
/// to its sender (see [`apply_overflow_reconcile`]). No assertion-only path.
pub async fn reconcile_overflow_with<T: ObserverTransport>(
    t: &T,
    tx_hash: &str,
    now_ns: u64,
) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let hash = normalize_tx_hash(tx_hash)
        .map_err(|_| XrplIcusdError::InvalidArgument("tx_hash must be 64 hex characters".into()))?;
    let (network, issuer) = read_state(|s| {
        let st = &s.xrpl_icusd;
        if !st.overflow_evidence.contains_key(&hash.to_ascii_uppercase()) {
            return Err(XrplIcusdError::NotFound);
        }
        Ok((
            st.config
                .network
                .ok_or_else(|| XrplIcusdError::Disabled("network not configured".into()))?,
            st.config
                .issuer_address
                .clone()
                .ok_or_else(|| XrplIcusdError::Disabled("issuer address not derived".into()))?,
        ))
    })?;
    let asset = IssuedAsset::icusd(&issuer).map_err(|e| XrplIcusdError::Internal(e.to_string()))?;
    match t.receipt(network, &hash, &asset).await {
        Ok(ReceiptLookup::Verified(r)) => {
            mutate_state(|s| apply_overflow_reconcile(&mut s.xrpl_icusd, network, &issuer, &r, now_ns))
        }
        Ok(_) => Err(XrplIcusdError::Unavailable(
            "the transaction could not be re-verified by the providers right now".into(),
        )),
        Err(e) => Err(XrplIcusdError::Unavailable(e.to_string())),
    }
}

pub async fn reconcile_overflow_receipt(tx_hash: String) -> Result<QuarantinedReceiptView, XrplIcusdError> {
    let out = reconcile_overflow_with(&IcTransport, &tx_hash, super::now()).await?;
    super::ensure_xrpl_tick_scheduled();
    Ok(out)
}
