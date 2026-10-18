//! IC escrow-ledger jobs: mint into / burn from the dedicated escrow subaccount,
//! ICRC-2 pulls into escrow, and canonical returns out of it.
//!
//! The rules (all enforced by the pure functions below, which the async executors
//! only drive):
//!
//! * A job is created and PERSISTED (nonce, `created_at_time`, memo, accounts,
//!   amount, `ledger_tip_before`) before its first await. The executor takes only
//!   `(operation id, job index)` and loads those immutable fields from state, so a
//!   retry can never change a field and a lost reply is turned by the ledger's own
//!   dedup into `Duplicate { duplicate_of }`, which becomes `Done(block)`.
//! * Definite ledger rejections are `Failed`; transport failures and replies we
//!   could not read are `Unknown` (resent with the identical job, blocking every
//!   dependent step).
//! * When the dedup window has expired (`TooOld`) the ONLY way forward is a bounded
//!   ICRC-3 scan from `ledger_tip_before`, following archive callbacks, matching
//!   memo + `created_at_time` + amount + accounts. A nonce is never re-allocated.
//! * Mint = from the backend's default (minting) account to escrow; burn = from the
//!   escrow subaccount to the minting account. The escrow account is NEVER the
//!   minting account (`check_minting_account`).

use candid::{Nat, Principal};
use icrc_ledger_client_cdk::{CdkRuntime, ICRC1Client};
use icrc_ledger_types::icrc::generic_value::ICRC3Value;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use num_traits::ToPrimitive;

use super::state::XrplIcusdState;
use super::types::*;
use crate::icrc3_proof::{decode_block, DecodedBlock};

/// Blocks inspected per scan page, and pages per tick for one job.
pub const SCAN_PAGE_BLOCKS: u64 = 64;
pub const SCAN_PAGES_PER_STEP: u32 = 8;

// ----------------------------------------------------------------------------------
// Minting-account verification (pure)
// ----------------------------------------------------------------------------------

/// The ledger's minting account must be the backend's default account (that is what
/// makes a transfer FROM it a mint), and must differ from the escrow account (so a
/// transfer TO escrow can never be mistaken for a burn).
pub fn check_minting_account(
    reported: Option<Account>,
    backend: Principal,
) -> Result<Account, XrplIcusdError> {
    let minting = reported.ok_or_else(|| {
        XrplIcusdError::MintingAccount("the ledger reports no minting account".into())
    })?;
    let escrow = escrow_account(backend);
    if minting == escrow {
        return Err(XrplIcusdError::MintingAccount(
            "the escrow account equals the ledger minting account".into(),
        ));
    }
    if minting != backend_default_account(backend) {
        return Err(XrplIcusdError::MintingAccount(
            "the ledger minting account is not the backend default account".into(),
        ));
    }
    Ok(minting)
}

// ----------------------------------------------------------------------------------
// Job specs: the ONLY places that decide who pays whom (pure)
// ----------------------------------------------------------------------------------

use super::state::JobSpec;

/// Mint `amount_e8s` from the minting (backend default) account into escrow.
pub fn mint_job_spec(backend: Principal, amount_e8s: u64, ledger_tip: u64) -> JobSpec {
    JobSpec {
        purpose: IcJobPurpose::EscrowMint,
        from_subaccount: None,
        from_owner: None,
        to: escrow_account(backend),
        amount_e8s,
        reserved_fee_e8s: 0,
        ledger_tip_before: Some(ledger_tip),
    }
}

/// Burn `amount_e8s` from escrow into the minting account (retire backing).
pub fn burn_job_spec(backend: Principal, amount_e8s: u64, ledger_tip: u64) -> JobSpec {
    JobSpec {
        purpose: IcJobPurpose::EscrowBurn,
        from_subaccount: Some(escrow_subaccount()),
        from_owner: None,
        to: backend_default_account(backend),
        amount_e8s,
        reserved_fee_e8s: 0,
        ledger_tip_before: Some(ledger_tip),
    }
}

/// Mint the borrowing-fee remainder to `to` (never to escrow).
pub fn fee_mint_job_spec(to: Account, amount_e8s: u64, ledger_tip: u64) -> JobSpec {
    JobSpec {
        purpose: IcJobPurpose::FeeMint,
        from_subaccount: None,
        from_owner: None,
        to,
        amount_e8s,
        reserved_fee_e8s: 0,
        ledger_tip_before: Some(ledger_tip),
    }
}

// ----------------------------------------------------------------------------------
// Call-result classification (pure)
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerCallResult {
    Ok(u64),
    /// `Duplicate { duplicate_of }`: the earlier attempt landed at that block.
    Duplicate(u64),
    /// The dedup window expired: only the archive scan may resolve this job.
    TooOld,
    /// The ledger definitely did not execute it, and a resend can succeed later.
    Retryable(String),
    /// The ledger definitely rejected it.
    Rejected(String),
    /// Transport failure / unreadable reply: it MAY have executed.
    Unknown(String),
}

fn block_u64(n: &Nat) -> Option<u64> {
    n.0.to_u64()
}

pub fn classify_transfer(r: Result<Result<Nat, TransferError>, (i32, String)>) -> LedgerCallResult {
    match r {
        Ok(Ok(b)) => block_u64(&b)
            .map(LedgerCallResult::Ok)
            .unwrap_or_else(|| LedgerCallResult::Unknown("block index exceeds u64".into())),
        Ok(Err(e)) => match e {
            TransferError::Duplicate { duplicate_of } => block_u64(&duplicate_of)
                .map(LedgerCallResult::Duplicate)
                .unwrap_or_else(|| LedgerCallResult::Unknown("duplicate index exceeds u64".into())),
            TransferError::TooOld => LedgerCallResult::TooOld,
            TransferError::CreatedInFuture { .. } => {
                LedgerCallResult::Retryable("created_at_time in the ledger's future".into())
            }
            TransferError::TemporarilyUnavailable => {
                LedgerCallResult::Retryable("ledger temporarily unavailable".into())
            }
            TransferError::BadFee { expected_fee } => {
                LedgerCallResult::Retryable(format!("bad fee, expected {expected_fee}"))
            }
            TransferError::BadBurn { min_burn_amount } => {
                LedgerCallResult::Rejected(format!("bad burn, minimum {min_burn_amount}"))
            }
            TransferError::InsufficientFunds { balance } => {
                LedgerCallResult::Rejected(format!("insufficient funds, balance {balance}"))
            }
            TransferError::GenericError { message, .. } => LedgerCallResult::Rejected(message),
        },
        Err((code, msg)) => LedgerCallResult::Unknown(format!("call failed ({code}): {msg}")),
    }
}

pub fn classify_transfer_from(
    r: Result<Result<Nat, TransferFromError>, (i32, String)>,
) -> LedgerCallResult {
    match r {
        Ok(Ok(b)) => block_u64(&b)
            .map(LedgerCallResult::Ok)
            .unwrap_or_else(|| LedgerCallResult::Unknown("block index exceeds u64".into())),
        Ok(Err(e)) => match e {
            TransferFromError::Duplicate { duplicate_of } => block_u64(&duplicate_of)
                .map(LedgerCallResult::Duplicate)
                .unwrap_or_else(|| LedgerCallResult::Unknown("duplicate index exceeds u64".into())),
            TransferFromError::TooOld => LedgerCallResult::TooOld,
            TransferFromError::CreatedInFuture { .. } => {
                LedgerCallResult::Retryable("created_at_time in the ledger's future".into())
            }
            TransferFromError::TemporarilyUnavailable => {
                LedgerCallResult::Retryable("ledger temporarily unavailable".into())
            }
            TransferFromError::BadFee { expected_fee } => {
                LedgerCallResult::Retryable(format!("bad fee, expected {expected_fee}"))
            }
            TransferFromError::BadBurn { min_burn_amount } => {
                LedgerCallResult::Rejected(format!("bad burn, minimum {min_burn_amount}"))
            }
            TransferFromError::InsufficientFunds { balance } => {
                LedgerCallResult::Rejected(format!("insufficient funds, balance {balance}"))
            }
            TransferFromError::InsufficientAllowance { allowance } => {
                LedgerCallResult::Rejected(format!("insufficient allowance {allowance}"))
            }
            TransferFromError::GenericError { message, .. } => LedgerCallResult::Rejected(message),
        },
        Err((code, msg)) => LedgerCallResult::Unknown(format!("call failed ({code}): {msg}")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEffect {
    /// Ledger reply carried a block (not yet fetched back and matched).
    Done(u64),
    NeedsScan,
    Retry(String),
    Failed(String),
    Unknown(String),
}

/// Apply a classified ledger reply to a persisted job.
pub fn apply_call_result(job: &mut IcLedgerJob, r: LedgerCallResult) -> JobEffect {
    match r {
        LedgerCallResult::Ok(b) | LedgerCallResult::Duplicate(b) => {
            job.status = IcJobStatus::Done { block: b };
            job.proof_verified = false;
            JobEffect::Done(b)
        }
        LedgerCallResult::TooOld => {
            job.status = IcJobStatus::Unknown {
                detail: "dedup window expired; archive scan required".into(),
            };
            job.scan_required = true;
            JobEffect::NeedsScan
        }
        LedgerCallResult::Retryable(m) => {
            job.status = IcJobStatus::Pending;
            JobEffect::Retry(m)
        }
        LedgerCallResult::Rejected(m) => {
            job.status = IcJobStatus::Failed { reason: m.clone() };
            JobEffect::Failed(m)
        }
        LedgerCallResult::Unknown(m) => {
            job.status = IcJobStatus::Unknown { detail: m.clone() };
            JobEffect::Unknown(m)
        }
    }
}

// ----------------------------------------------------------------------------------
// Block matching (pure)
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockMatch {
    Exact,
    /// Some identifying fields match but not all: never accepted, never ignored.
    Partial(String),
    NoMatch,
}

fn expected_op(p: IcJobPurpose) -> &'static str {
    match p {
        IcJobPurpose::EscrowMint | IcJobPurpose::FeeMint => "mint",
        IcJobPurpose::EscrowBurn => "burn",
        IcJobPurpose::ImportPull | IcJobPurpose::CanonicalReturn => "xfer",
    }
}

fn opt_eq(a: &Option<Account>, b: &Account) -> bool {
    a.as_ref() == Some(b)
}

/// Does a decoded ledger block equal the persisted job tuple EXACTLY?
pub fn job_matches_block(job: &IcLedgerJob, backend: Principal, b: &DecodedBlock) -> BlockMatch {
    let memo_ok = b.memo.as_deref() == Some(job.memo.as_slice());
    let cat_ok = b.created_at_time == Some(job.created_at_time);
    let amount_ok = b.amount == u128::from(job.amount_e8s);
    let op_ok = b.op == expected_op(job.purpose);
    let escrow_src = Account {
        owner: backend,
        subaccount: job.from_subaccount,
    };
    let accounts_ok = match job.purpose {
        IcJobPurpose::EscrowMint | IcJobPurpose::FeeMint => {
            b.from.is_none() && opt_eq(&b.to, &job.to)
        }
        IcJobPurpose::EscrowBurn => {
            opt_eq(&b.from, &escrow_src)
                && (b.to.is_none() || opt_eq(&b.to, &backend_default_account(backend)))
        }
        IcJobPurpose::CanonicalReturn => {
            opt_eq(&b.from, &escrow_src) && opt_eq(&b.to, &job.to) && b.spender.is_none()
        }
        IcJobPurpose::ImportPull => {
            job.from_owner
                .as_ref()
                .is_some_and(|f| opt_eq(&b.from, f))
                && opt_eq(&b.to, &job.to)
                && opt_eq(&b.spender, &backend_default_account(backend))
        }
    };
    if op_ok && memo_ok && cat_ok && amount_ok && accounts_ok {
        return BlockMatch::Exact;
    }
    if memo_ok || (cat_ok && amount_ok) {
        return BlockMatch::Partial(format!(
            "plausible block op={} amount={} memo_ok={memo_ok} cat_ok={cat_ok} accounts_ok={accounts_ok}",
            b.op, b.amount
        ));
    }
    BlockMatch::NoMatch
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanVerdict {
    Found(u64),
    /// No block of the page relates to the job.
    Clear,
    /// Plausible-but-inexact or duplicate evidence: hold, never guess.
    Conflict(String),
}

pub fn scan_blocks(
    job: &IcLedgerJob,
    backend: Principal,
    blocks: &[(u64, DecodedBlock)],
) -> ScanVerdict {
    let mut found: Option<u64> = None;
    for (idx, b) in blocks {
        match job_matches_block(job, backend, b) {
            BlockMatch::Exact => {
                if found.replace(*idx).is_some() {
                    return ScanVerdict::Conflict("the exact tuple appears more than once".into());
                }
            }
            BlockMatch::Partial(m) => {
                return ScanVerdict::Conflict(format!("block {idx}: {m}"));
            }
            BlockMatch::NoMatch => {}
        }
    }
    match found {
        Some(i) => ScanVerdict::Found(i),
        None => ScanVerdict::Clear,
    }
}

/// Assemble the blocks for `[start, start + expected)` from direct and archive
/// answers; fail closed on any gap, duplicate, or stray id.
pub fn assemble_blocks(
    start: u64,
    expected: u64,
    parts: Vec<(u64, ICRC3Value)>,
) -> Result<Vec<(u64, ICRC3Value)>, String> {
    let mut parts = parts;
    parts.sort_by_key(|(i, _)| *i);
    if parts.len() as u64 != expected {
        return Err(format!(
            "ICRC-3 page is short or has extra blocks ({} of {expected})",
            parts.len()
        ));
    }
    for (offset, (idx, _)) in parts.iter().enumerate() {
        if *idx != start + offset as u64 {
            return Err("ICRC-3 page has a gap, duplicate, or out-of-range id".into());
        }
    }
    Ok(parts)
}

// ----------------------------------------------------------------------------------
// Pure job transitions on the lane state
// ----------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanEffect {
    Found(u64),
    /// More pages remain.
    Continue,
    /// The pinned prefix was scanned completely and the tuple is absent: the job
    /// never executed and, with the window expired, never can.
    ProvenAbsent,
    Held(String),
}

impl XrplIcusdState {
    fn job_mut(&mut self, op_id: OperationId, idx: usize) -> Result<&mut IcLedgerJob, XrplIcusdError> {
        self.op_mut(op_id)?
            .ic_jobs
            .get_mut(idx)
            .ok_or_else(|| XrplIcusdError::Internal("ledger job index out of range".into()))
    }

    pub fn job(&self, op_id: OperationId, idx: usize) -> Option<&IcLedgerJob> {
        self.op(op_id).and_then(|o| o.ic_jobs.get(idx))
    }

    /// Mark the job `InFlight` and hand out a copy of its immutable tuple. Only
    /// `Pending`/`Unknown` jobs without a pending scan can be (re)sent.
    pub fn begin_job_call(
        &mut self,
        op_id: OperationId,
        idx: usize,
    ) -> Result<IcLedgerJob, XrplIcusdError> {
        let job = self.job_mut(op_id, idx)?;
        if job.scan_required {
            return Err(XrplIcusdError::WrongPhase(
                "the dedup window expired; this job can only be resolved by scan".into(),
            ));
        }
        match job.status {
            IcJobStatus::Pending | IcJobStatus::Unknown { .. } => {}
            _ => {
                return Err(XrplIcusdError::WrongPhase(
                    "ledger job is already resolved or in flight".into(),
                ))
            }
        }
        job.status = IcJobStatus::InFlight;
        job.sends += 1;
        Ok(job.clone())
    }

    pub fn finish_job_call(
        &mut self,
        op_id: OperationId,
        idx: usize,
        r: LedgerCallResult,
        now_ns: u64,
    ) -> Result<JobEffect, XrplIcusdError> {
        let job = self.job_mut(op_id, idx)?;
        if job.status != IcJobStatus::InFlight {
            return Err(XrplIcusdError::Internal(
                "finish_job_call on a job that was not in flight".into(),
            ));
        }
        let effect = apply_call_result(job, r);
        job.in_flight_since_ns = None;
        let op = self.op_mut(op_id)?;
        op.updated_at_ns = now_ns;
        if let JobEffect::Failed(m) | JobEffect::Unknown(m) = &effect {
            op.push_note(format!("ledger job {idx}: {m}"));
        }
        Ok(effect)
    }

    /// Start (or continue) the scan: pins the end of the scan at `tip_now` the first
    /// time, which MUST be read after the `TooOld` reply.
    pub fn begin_scan(
        &mut self,
        op_id: OperationId,
        idx: usize,
        tip_now: u64,
    ) -> Result<(u64, u64), XrplIcusdError> {
        let job = self.job_mut(op_id, idx)?;
        if !job.scan_required || job.is_resolved() {
            return Err(XrplIcusdError::WrongPhase("no scan pending for this job".into()));
        }
        let Some(base) = job.ledger_tip_before else {
            return Err(XrplIcusdError::Unavailable(
                "job has no ledger_tip_before; the scan baseline is unknown".into(),
            ));
        };
        let scan = job.scan.get_or_insert(ScanProgress {
            next_block: base,
            end_block: None,
        });
        let end = *scan.end_block.get_or_insert(tip_now);
        Ok((scan.next_block, end))
    }

    pub fn finish_scan_page(
        &mut self,
        op_id: OperationId,
        idx: usize,
        page_end_exclusive: u64,
        verdict: ScanVerdict,
        now_ns: u64,
    ) -> Result<ScanEffect, XrplIcusdError> {
        let job = self.job_mut(op_id, idx)?;
        let effect = match verdict {
            ScanVerdict::Found(block) => {
                job.status = IcJobStatus::Done { block };
                // The block was decoded and matched exactly while scanning.
                job.proof_verified = true;
                job.scan_required = false;
                ScanEffect::Found(block)
            }
            ScanVerdict::Conflict(m) => {
                job.status = IcJobStatus::Unknown {
                    detail: format!("scan conflict: {m}"),
                };
                ScanEffect::Held(m)
            }
            ScanVerdict::Clear => {
                let scan = job.scan.as_mut().ok_or_else(|| {
                    XrplIcusdError::Internal("scan page without scan progress".into())
                })?;
                scan.next_block = scan.next_block.max(page_end_exclusive);
                if scan.end_block.is_some_and(|e| scan.next_block >= e) {
                    job.status = IcJobStatus::Failed {
                        reason: "dedup window expired and the archive scan proved the \
                                 transfer never executed"
                            .into(),
                    };
                    job.scan_required = false;
                    ScanEffect::ProvenAbsent
                } else {
                    ScanEffect::Continue
                }
            }
        };
        let op = self.op_mut(op_id)?;
        op.updated_at_ns = now_ns;
        if let ScanEffect::Held(m) = &effect {
            op.push_note(format!("ledger job {idx} held: {m}"));
        }
        Ok(effect)
    }

    pub fn mark_proof_verified(&mut self, op_id: OperationId, idx: usize) -> Result<(), XrplIcusdError> {
        let job = self.job_mut(op_id, idx)?;
        if job.done_block().is_none() {
            return Err(XrplIcusdError::WrongPhase("job is not done".into()));
        }
        job.proof_verified = true;
        Ok(())
    }

    /// The block the ledger named does not equal the persisted tuple. The job stays
    /// `Done` (never silently re-sent) but UNPROVEN, so no dependent step runs.
    pub fn note_proof_mismatch(&mut self, op_id: OperationId, idx: usize, detail: &str) {
        if let Ok(op) = self.op_mut(op_id) {
            op.push_note(format!("ledger job {idx} block does not match its tuple: {detail}"));
        }
    }
}

// ----------------------------------------------------------------------------------
// Async executors
// ----------------------------------------------------------------------------------

fn ledger() -> Principal {
    crate::state::read_state(|s| s.icusd_ledger_principal)
}

fn now() -> u64 {
    ic_cdk::api::time()
}

/// Exclusive ledger log length (also the baseline for `ledger_tip_before`).
pub async fn ledger_tip() -> Result<u64, String> {
    crate::icrc3_proof::icrc3_log_length(ledger()).await
}

pub async fn escrow_balance_e8s() -> Result<u128, String> {
    let acct = escrow_account(ic_cdk::id());
    let (bal,): (Nat,) = ic_cdk::call(ledger(), "icrc1_balance_of", (acct,))
        .await
        .map_err(|(c, m)| format!("icrc1_balance_of failed: {c:?} {m}"))?;
    bal.0.to_u128().ok_or_else(|| "balance exceeds u128".to_string())
}

/// Query the ledger's minting account and prove it is usable with the escrow design.
pub async fn query_minting_account() -> Result<Account, XrplIcusdError> {
    let (m,): (Option<Account>,) = ic_cdk::call(ledger(), "icrc1_minting_account", ())
        .await
        .map_err(|(c, msg)| {
            XrplIcusdError::Unavailable(format!("icrc1_minting_account failed: {c:?} {msg}"))
        })?;
    check_minting_account(m, ic_cdk::id())
}

async fn send_job(ledger: Principal, job: &IcLedgerJob) -> LedgerCallResult {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let memo = Some(Memo::from(job.memo.clone()));
    match job.purpose {
        IcJobPurpose::ImportPull => {
            let Some(from) = job.from_owner else {
                return LedgerCallResult::Rejected("pull job without a source account".into());
            };
            classify_transfer_from(
                client
                    .transfer_from(TransferFromArgs {
                        spender_subaccount: None,
                        from,
                        to: job.to,
                        amount: Nat::from(job.amount_e8s),
                        fee: None,
                        created_at_time: Some(job.created_at_time),
                        memo,
                    })
                    .await,
            )
        }
        _ => classify_transfer(
            client
                .transfer(TransferArg {
                    from_subaccount: job.from_subaccount,
                    to: job.to,
                    fee: None,
                    created_at_time: Some(job.created_at_time),
                    memo,
                    amount: Nat::from(job.amount_e8s),
                })
                .await,
        ),
    }
}

/// Run one send (or one bounded scan step) for a persisted job.
pub async fn execute_job(op_id: OperationId, idx: usize) -> Result<JobEffect, XrplIcusdError> {
    let _guard = super::OpGuard::acquire(op_id)?;
    let scan_required = crate::state::read_state(|s| {
        s.xrpl_icusd
            .job(op_id, idx)
            .map(|j| (j.scan_required, j.is_resolved()))
    });
    match scan_required {
        None => return Err(XrplIcusdError::NotFound),
        Some((_, true)) => {
            return Err(XrplIcusdError::WrongPhase("ledger job already resolved".into()))
        }
        Some((true, _)) => return scan_step_inner(op_id, idx).await,
        Some((false, _)) => {}
    }
    let ledger = ledger();
    let job = crate::state::mutate_state(|s| {
        let job = s.xrpl_icusd.begin_job_call(op_id, idx)?;
        // Stamp the start so a continuation that never runs (a trap after the await)
        // cannot leave the job `InFlight` forever: see `expire_stale_in_flight`.
        s.xrpl_icusd.job_mut(op_id, idx)?.in_flight_since_ns = Some(now());
        Ok::<_, XrplIcusdError>(job)
    })?;
    let result = send_job(ledger, &job).await;
    crate::state::mutate_state(|s| s.xrpl_icusd.finish_job_call(op_id, idx, result, now()))
}

/// Archive-following scan, bounded to [`SCAN_PAGES_PER_STEP`] pages per call.
pub async fn scan_step(op_id: OperationId, idx: usize) -> Result<JobEffect, XrplIcusdError> {
    let _guard = super::OpGuard::acquire(op_id)?;
    scan_step_inner(op_id, idx).await
}

async fn scan_step_inner(op_id: OperationId, idx: usize) -> Result<JobEffect, XrplIcusdError> {
    let ledger = ledger();
    let backend = ic_cdk::id();
    // The pin must be read AFTER the TooOld reply (it is: scan_required was set by it).
    let tip_now = ledger_tip()
        .await
        .map_err(|e| XrplIcusdError::Unavailable(format!("ledger tip: {e}")))?;
    let (mut next, end) =
        crate::state::mutate_state(|s| s.xrpl_icusd.begin_scan(op_id, idx, tip_now))?;
    let job = crate::state::read_state(|s| s.xrpl_icusd.job(op_id, idx).cloned())
        .ok_or(XrplIcusdError::NotFound)?;
    for _ in 0..SCAN_PAGES_PER_STEP {
        if next >= end {
            break;
        }
        let len = (end - next).min(SCAN_PAGE_BLOCKS);
        let blocks = fetch_decoded_range(ledger, next, len)
            .await
            .map_err(XrplIcusdError::Unavailable)?;
        let verdict = scan_blocks(&job, backend, &blocks);
        let page_end = next + len;
        let effect = crate::state::mutate_state(|s| {
            s.xrpl_icusd
                .finish_scan_page(op_id, idx, page_end, verdict, now())
        })?;
        match effect {
            ScanEffect::Found(b) => return Ok(JobEffect::Done(b)),
            ScanEffect::ProvenAbsent => {
                return Ok(JobEffect::Failed("scan proved the transfer never executed".into()))
            }
            ScanEffect::Held(m) => return Ok(JobEffect::Unknown(m)),
            ScanEffect::Continue => next = page_end,
        }
    }
    Ok(JobEffect::NeedsScan)
}

#[derive(candid::CandidType, serde::Deserialize)]
struct GetArchivesArgs {
    from: Option<Principal>,
}

#[derive(candid::CandidType, serde::Deserialize)]
struct ArchiveInfo {
    canister_id: Principal,
    #[allow(dead_code)]
    start: Nat,
    #[allow(dead_code)]
    end: Nat,
}

/// `[start, start+len)` decoded, from the ledger plus (verified) archive callbacks.
/// Fails closed on any gap, an archive the ledger itself does not list, nested
/// archives, or oversized replies.
pub async fn fetch_decoded_range(
    ledger: Principal,
    start: u64,
    len: u64,
) -> Result<Vec<(u64, DecodedBlock)>, String> {
    let req = vec![GetBlocksRequest {
        start: Nat::from(start),
        length: Nat::from(len),
    }];
    let (resp,): (GetBlocksResult,) = ic_cdk::call(ledger, "icrc3_get_blocks", (req,))
        .await
        .map_err(|(c, m)| format!("icrc3_get_blocks failed: {c:?} {m}"))?;
    let log_length = resp
        .log_length
        .0
        .to_u64()
        .ok_or("log length exceeds u64")?;
    let expected = len.min(log_length.saturating_sub(start));
    let mut parts: Vec<(u64, ICRC3Value)> = Vec::new();
    for b in resp.blocks {
        parts.push((b.id.0.to_u64().ok_or("block id exceeds u64")?, b.block));
    }
    if !resp.archived_blocks.is_empty() {
        if resp.archived_blocks.len() > 8 {
            return Err("too many archive callbacks".into());
        }
        let (archives,): (Vec<ArchiveInfo>,) =
            ic_cdk::call(ledger, "icrc3_get_archives", (GetArchivesArgs { from: None },))
                .await
                .map_err(|(c, m)| format!("icrc3_get_archives failed: {c:?} {m}"))?;
        for ab in resp.archived_blocks {
            let cb = &ab.callback;
            if cb.method.trim().is_empty() || !archives.iter().any(|a| a.canister_id == cb.canister_id) {
                return Err("archive callback is not an archive the ledger lists".into());
            }
            for a in &ab.args {
                let s = a.start.0.to_u64().ok_or("archive range start exceeds u64")?;
                let l = a.length.0.to_u64().ok_or("archive range length exceeds u64")?;
                if s < start || s.saturating_add(l) > start.saturating_add(len) {
                    return Err("archive range outside the requested page".into());
                }
            }
            let (sub,): (GetBlocksResult,) = ic_cdk::call(cb.canister_id, &cb.method, (ab.args.clone(),))
                .await
                .map_err(|(c, m)| format!("archive call failed: {c:?} {m}"))?;
            if !sub.archived_blocks.is_empty() {
                return Err("nested archive descriptors are not followed".into());
            }
            for b in sub.blocks {
                parts.push((b.id.0.to_u64().ok_or("block id exceeds u64")?, b.block));
            }
        }
    }
    let parts = assemble_blocks(start, expected, parts)?;
    parts
        .into_iter()
        .map(|(i, v)| decode_block(&v).map(|d| (i, d)))
        .collect()
}

/// Fetch the `Done` block back from the ledger and require the EXACT persisted tuple.
/// On success the job becomes `proof_verified`; dependents run only after that.
pub async fn verify_done_job(op_id: OperationId, idx: usize) -> Result<(), XrplIcusdError> {
    let (job, block) = crate::state::read_state(|s| {
        s.xrpl_icusd.job(op_id, idx).map(|j| (j.clone(), j.done_block()))
    })
    .ok_or(XrplIcusdError::NotFound)?;
    let Some(block) = block else {
        return Err(XrplIcusdError::WrongPhase("job is not done".into()));
    };
    if job.proof_verified {
        return Ok(());
    }
    let decoded = crate::icrc3_proof::fetch_icrc3_block(ledger(), block)
        .await
        .map_err(XrplIcusdError::Unavailable)?;
    match job_matches_block(&job, ic_cdk::id(), &decoded) {
        BlockMatch::Exact => crate::state::mutate_state(|s| s.xrpl_icusd.mark_proof_verified(op_id, idx)),
        other => {
            let detail = format!("{other:?}");
            crate::state::mutate_state(|s| s.xrpl_icusd.note_proof_mismatch(op_id, idx, &detail));
            Err(XrplIcusdError::Internal(format!(
                "ledger block {block} does not match the persisted tuple: {detail}"
            )))
        }
    }
}
