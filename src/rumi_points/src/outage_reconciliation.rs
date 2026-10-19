//! Additive accounting for the October 2026 points outage.
//!
//! This is a draft implementation, not an allocation policy. No approved
//! allocation digest is configured, so staging is fail-closed. The existing
//! snapshot driver, hold, seed chain and original ledger encodings are intact.
//! A reviewed allocation must cover every audited participant and missed period.
//! Whole-epoch targets offset real credits after close; a sealed pre-opening
//! prefix never offsets later normal earnings. Missing epochs need gap proof.
//! Missing random snapshots are never represented as observed captures.

use std::borrow::Cow;
use std::cell::RefCell;

use candid::{CandidType, Principal};
use ic_stable_structures::{log::Log as StableLog, storable::Bound, StableCell, Storable};
use serde::{Deserialize, Serialize};

use crate::state;
use crate::types::PointSource;

const FIRST_EPOCH: u64 = 18;
const LEGACY_LAST_EPOCH: u64 = 19;
const MAX_ALLOCATION_ROWS: usize = 512;
const PARTICIPANTS: usize = 15;
const SCAN_CHUNK: u32 = 100;
const DOMAIN: &[u8] = b"rumi-points-october-2026-outage-allocation-v2\0";
const PREFIX_DOMAIN: &[u8] = b"rumi-points-october-2026-original-ledger-v1\0";

// Deliberately unset: evidence worksheets are not an approved allocation.
// Setting this requires a separately reviewed immutable allocation manifest.
const APPROVED_ALLOCATION_DIGEST: Option<[u8; 32]> = None;

/// Breakdown of ordinary earning records used to offset a total epoch floor.
/// Existing credit in any source offsets the total. The actual make-whole
/// credit is recorded with its own kind and never masquerades as a snapshot
/// award or the historical unmatched-row correction.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct SourcePoints {
    pub icusd_debt: u128,
    pub icusd_3pool: u128,
    pub ck_unmatched: u128,
    pub ck_matched: u128,
    pub ck_flat4x: u128,
    pub ck_unmatched_topup: u128,
    pub vault_repayment: u128,
    pub icusd_sp: u128,
    pub threeusd_sp: u128,
    pub amm_lp: u128,
}

impl SourcePoints {
    fn slot(&mut self, source: PointSource) -> Result<&mut u128, String> {
        match source {
            PointSource::IcUsdDebt => Ok(&mut self.icusd_debt),
            PointSource::IcUsd3Pool => Ok(&mut self.icusd_3pool),
            PointSource::CkStable3PoolUnmatched => Ok(&mut self.ck_unmatched),
            PointSource::CkStable3PoolMatched => Ok(&mut self.ck_matched),
            PointSource::CkStable3PoolFlat4x => Ok(&mut self.ck_flat4x),
            PointSource::CkStable3PoolUnmatchedTopUp => Ok(&mut self.ck_unmatched_topup),
            PointSource::VaultRepayment => Ok(&mut self.vault_repayment),
            PointSource::IcUsdStabilityPool => Ok(&mut self.icusd_sp),
            PointSource::ThreeUsdStabilityPool => Ok(&mut self.threeusd_sp),
            PointSource::AmmLp => Ok(&mut self.amm_lp),
            PointSource::Registration => {
                Err("registration is not a recoverable earning source".into())
            }
        }
    }

    fn add(&mut self, source: PointSource, points: u128) -> Result<(), String> {
        let value = self.slot(source)?;
        *value = value.checked_add(points).ok_or("source points overflow")?;
        Ok(())
    }

    fn entries(&self) -> [(PointSource, u128); 10] {
        [
            (PointSource::IcUsdDebt, self.icusd_debt),
            (PointSource::IcUsd3Pool, self.icusd_3pool),
            (PointSource::CkStable3PoolUnmatched, self.ck_unmatched),
            (PointSource::CkStable3PoolMatched, self.ck_matched),
            (PointSource::CkStable3PoolFlat4x, self.ck_flat4x),
            (
                PointSource::CkStable3PoolUnmatchedTopUp,
                self.ck_unmatched_topup,
            ),
            (PointSource::VaultRepayment, self.vault_repayment),
            (PointSource::IcUsdStabilityPool, self.icusd_sp),
            (PointSource::ThreeUsdStabilityPool, self.threeusd_sp),
            (PointSource::AmmLp, self.amm_lp),
        ]
    }

    fn total(&self) -> Result<u128, String> {
        self.entries()
            .into_iter()
            .try_fold(0u128, |sum, (_, value)| {
                sum.checked_add(value)
                    .ok_or_else(|| "allocation total overflow".to_string())
            })
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AllocationRow {
    pub epoch_index: u64,
    pub principal: Principal,
    pub target_epoch_points: u128,
    /// Both absent for a whole-epoch floor. Both present only for the exact
    /// audited pre-opening prefix of the real resumed epoch.
    #[serde(default)]
    pub coverage_start_ns: Option<u64>,
    #[serde(default)]
    pub coverage_end_ns: Option<u64>,
}

/// Every census principal/affected-period row is mandatory, including zeros. The input-evidence digest
/// binds the reviewed assumptions and source inventories, not just the numbers.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageAllocation {
    pub evidence_digest: [u8; 32],
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    pub original_ledger_prefix_len: u64,
    pub original_ledger_prefix_digest: [u8; 32],
    pub rows: Vec<AllocationRow>,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppliedRow {
    pub credited_points: u128,
    pub credit_log_offset: Option<u64>,
    pub applied_at_ns: u64,
}

/// Separate audit identity: ordinary PointEntry / PointSource remain frozen.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum OutageCreditKind {
    OutageMakeWhole,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageCredit {
    pub kind: OutageCreditKind,
    pub allocation_digest: [u8; 32],
    pub evidence_digest: [u8; 32],
    pub principal: Principal,
    pub epoch_index: u64,
    pub scheduled_period_start_ns: u64,
    pub scheduled_period_end_ns: u64,
    #[serde(default)]
    pub coverage_start_ns: Option<u64>,
    #[serde(default)]
    pub coverage_end_ns: Option<u64>,
    pub normal_ledger_cutoff: u64,
    pub normal_points_earned: u128,
    pub target_epoch_points: u128,
    pub credited_points: u128,
    pub applied_at_ns: u64,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageCreditPage {
    pub entries: Vec<OutageCredit>,
    pub next_offset: u64,
    pub reached_end: bool,
}

#[derive(Serialize, Deserialize, Clone)]
enum StoredOutageCredit {
    V1(OutageCredit),
}

impl Storable for StoredOutageCredit {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(self, &mut bytes).expect("encode outage credit");
        Cow::Owned(bytes)
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(bytes.as_ref()).expect("decode outage credit")
    }
    const BOUND: Bound = Bound::Unbounded;
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageProgress {
    pub allocation_digest: [u8; 32],
    pub allocation: OutageAllocation,
    pub ledger_cutoff: u64,
    pub scanned_through: u64,
    pub original_prefix_digest_so_far: [u8; 32],
    pub original_registrations: Vec<Principal>,
    pub earned: Vec<SourcePoints>,
    pub applied: Vec<AppliedRow>,
}

/// Public operational receipt without unpublished participant target rows.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageProgressView {
    pub allocation_digest: [u8; 32],
    pub evidence_digest: [u8; 32],
    pub original_ledger_prefix_len: u64,
    pub ledger_cutoff: u64,
    pub scanned_through: u64,
    pub rows_total: u32,
    pub rows_applied: u32,
    pub credited_points: u128,
    pub complete: bool,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OutageReconciliationStatus {
    pub configured_allocation_digest: Option<[u8; 32]>,
    pub progress: Option<OutageProgressView>,
    pub credit_log_len: u64,
    pub audit_count_matches_receipts: bool,
}

#[derive(Serialize, Deserialize, Clone)]
enum StoredOutageReconciliation {
    V1(Option<OutageProgress>),
}

impl Storable for StoredOutageReconciliation {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(self, &mut bytes).expect("encode outage reconciliation");
        Cow::Owned(bytes)
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(bytes.as_ref()).expect("decode outage reconciliation")
    }
    const BOUND: Bound = Bound::Unbounded;
}

thread_local! {
    // New MemoryIds 16/17/18 only. No previous at-rest type is rewritten.
    static PROGRESS: RefCell<StableCell<StoredOutageReconciliation, state::VMem>> =
        RefCell::new(StableCell::init(state::outage_reconciliation_memory(),
            StoredOutageReconciliation::V1(None)).expect("initialize outage reconciliation"));
}

thread_local! {
    static CREDITS: RefCell<StableLog<StoredOutageCredit, state::VMem, state::VMem>> = {
        let (index, data) = state::outage_credit_memories();
        RefCell::new(StableLog::init(index, data).expect("initialize outage credit log"))
    };
}

pub fn credit_count() -> u64 {
    CREDITS.with(|log| log.borrow().len())
}

pub fn credit_entries(offset: u64, limit: u32) -> OutageCreditPage {
    CREDITS.with(|log| {
        let log = log.borrow();
        let start = offset.min(log.len());
        let end = start
            .saturating_add(u64::from(limit.min(100)))
            .min(log.len());
        let entries = (start..end)
            .map(
                |index| match log.get(index).expect("missing outage audit row") {
                    StoredOutageCredit::V1(row) => row,
                },
            )
            .collect();
        OutageCreditPage {
            entries,
            next_offset: end,
            reached_end: end == log.len(),
        }
    })
}

/// Filtered audit view. Scan at most 100 underlying rows per call so a page
/// with no matching entries still advances its cursor. Callers must continue
/// until reached_end; entry count alone does not indicate completion.
pub fn principal_credit_entries(principal: Principal, offset: u64, limit: u32) -> OutageCreditPage {
    CREDITS.with(|log| {
        let log = log.borrow();
        let start = offset.min(log.len());
        let scan_end = start.saturating_add(100).min(log.len());
        let wanted = usize::try_from(limit.min(100)).expect("bounded page size");
        let mut next = start;
        let mut entries = Vec::new();
        while next < scan_end && entries.len() < wanted {
            let StoredOutageCredit::V1(row) = log.get(next).expect("missing outage audit row");
            next += 1;
            if row.principal == principal {
                entries.push(row);
            }
        }
        OutageCreditPage {
            entries,
            next_offset: next,
            reached_end: next == log.len(),
        }
    })
}

fn append_credit(credit: OutageCredit) -> u64 {
    CREDITS.with(|log| {
        log.borrow()
            .append(&StoredOutageCredit::V1(credit))
            .expect("append outage credit")
    })
}

pub fn progress() -> Option<OutageProgress> {
    PROGRESS.with(|cell| match cell.borrow().get() {
        StoredOutageReconciliation::V1(progress) => progress.clone(),
    })
}

fn require_current_census(saved:&OutageProgress)->Result<(),String>{
    if let Some(r)=crate::recovery_v2::coverage(){
        let cutoff=r.normal_start_ns.ok_or("normal coverage cutoff missing")?;
        let known:Vec<_>=crate::recovery_v2::census()?.into_iter().filter(|p|state::get_principal_state(p)
            .is_some_and(|s|s.registered_at_ns<cutoff)).collect();
        let planned:Vec<_>=saved.allocation.rows.iter().filter(|r|r.epoch_index==FIRST_EPOCH).map(|r|r.principal).collect();
        if known!=planned { return Err("new historical registration requires a reviewed replacement/supplemental allocation; completeness is not established".into()); }
    }
    Ok(())
}

fn progress_view(saved: &OutageProgress, count_matches: bool) -> OutageProgressView {
    OutageProgressView {
        allocation_digest: saved.allocation_digest,
        evidence_digest: saved.allocation.evidence_digest,
        original_ledger_prefix_len: saved.allocation.original_ledger_prefix_len,
        ledger_cutoff: saved.ledger_cutoff,
        scanned_through: saved.scanned_through,
        rows_total: u32::try_from(saved.allocation.rows.len()).expect("bounded allocation rows"),
        rows_applied: u32::try_from(saved.applied.len()).expect("bounded applied receipts"),
        credited_points: saved
            .applied
            .iter()
            .try_fold(0u128, |sum, row| sum.checked_add(row.credited_points))
            .expect("outage receipt sum overflow"),
        complete: saved.scanned_through == saved.ledger_cutoff
            && saved.applied.len() == saved.allocation.rows.len()
            && count_matches
            && require_current_census(saved).is_ok(),
    }
}

/// Query-only projection. It reveals no pending seed, capture times, target
/// amounts or participant rows from a staged-but-unapplied allocation.
pub fn status() -> OutageReconciliationStatus {
    let saved = progress();
    let credit_log_len = credit_count();
    let receipted_count = saved.as_ref().map_or(0, |p| {
        p.applied
            .iter()
            .filter(|row| row.credit_log_offset.is_some())
            .count() as u64
    });
    let count_matches = credit_log_len == receipted_count;
    OutageReconciliationStatus {
        configured_allocation_digest: APPROVED_ALLOCATION_DIGEST,
        progress: saved.as_ref().map(|p| progress_view(p, count_matches)),
        credit_log_len,
        audit_count_matches_receipts: count_matches,
    }
}

fn persist(progress: OutageProgress) {
    PROGRESS.with(|cell| {
        cell.borrow_mut()
            .set(StoredOutageReconciliation::V1(Some(progress)))
            .expect("persist outage reconciliation");
    });
}

pub fn allocation_digest(allocation: &OutageAllocation) -> Result<[u8; 32], String> {
    if allocation.rows.is_empty() || allocation.rows.len()>MAX_ALLOCATION_ROWS {
        return Err("allocation row count is outside its reviewed bound".into());
    }
    allocation.rows.iter().try_fold(0u128, |sum, row| {
        sum.checked_add(row.target_epoch_points)
            .ok_or("combined allocation exceeds u128")
    })?;
    let bytes = candid::encode_one(allocation).map_err(|e| format!("encode allocation: {e}"))?;
    Ok(crate::snapshot_seed::sha256(&[DOMAIN, &bytes]))
}

fn last_affected_epoch()->Result<u64,String>{
    if crate::recovery_v2::coverage().is_some(){crate::recovery_v2::last_affected_epoch()}
    else{Ok(LEGACY_LAST_EPOCH)}
}

/// Returns the actual compensated bounds and whether ordinary epoch credits
/// offset this target. Prefix compensation is disjoint from normal accrual.
fn accounting_period(row:&AllocationRow)->Result<(u64,u64,bool),String>{
    let (scheduled,end)=crate::epoch::epoch_bounds(row.epoch_index,state::season_bounds().0,state::season_bounds().1);
    if let Some(r)=crate::recovery_v2::coverage(){
        if r.normal_epoch==Some(row.epoch_index) {
            let opening=r.normal_start_ns.ok_or("normal coverage is not sealed")?;
            if row.coverage_start_ns!=Some(scheduled)||row.coverage_end_ns!=Some(opening)||opening<=scheduled||opening>=end {
                return Err("allocation prefix must exactly match the audited normal opening".into());
            }
            return Ok((scheduled,opening,false));
        }
    }
    if row.coverage_start_ns.is_some()||row.coverage_end_ns.is_some(){
        return Err("only the resumed epoch has a partial compensation interval".into());
    }
    Ok((scheduled,end,true))
}

fn require_accounting_boundary()->Result<(),String>{
    let last=last_affected_epoch()?;
    if last<FIRST_EPOCH || last-FIRST_EPOCH>=MAX_ALLOCATION_ROWS as u64 { return Err("affected epoch span exceeds reviewed bound".into()); }
    for index in FIRST_EPOCH..=last {
        if let Some(r)=crate::recovery_v2::coverage(){
            if state::season_bounds()!=(r.season_start_ns,r.season_end_ns){return Err("recovery season bounds changed".into());}
            if crate::recovery_v2::fully_missed_epoch(index){
                if state::epoch_summary_by_index(index).is_some(){return Err("missed coverage conflicts with a real summary".into());}
                continue;
            }
            if r.normal_epoch==Some(index){continue;} // disjoint prefix is sealed, normal accrual may continue
        }
        let summary=state::epoch_summary_by_index(index).ok_or("affected real epoch has not closed")?;
        let (start,end)=crate::epoch::epoch_bounds(index,state::season_bounds().0,state::season_bounds().1);
        if summary.epoch_start_ns<start||summary.epoch_start_ns>=end||summary.epoch_end_ns!=end{
            return Err("affected real epoch has unexpected coverage".into());
        }
    }
    let policy=state::fiat_stable_points_policy();
    if policy.cutover_epoch!=Some(19)||policy.legacy_epoch!=Some(18)||!policy.historical_complete||!policy.inline_legacy_topups_complete{
        return Err("fixed fiat-stable cutover or original corrections are incomplete".into());
    }
    Ok(())
}

fn approved_digest(allocation: &OutageAllocation) -> Result<[u8; 32], String> {
    let expected =
        APPROVED_ALLOCATION_DIGEST.ok_or("no reviewed outage allocation is configured")?;
    let digest = allocation_digest(allocation)?;
    if digest != expected {
        return Err("allocation does not match the reviewed digest".into());
    }
    Ok(digest)
}

/// Admin ingress cannot stage anything until a separately reviewed immutable
/// allocation digest is compiled in. The present pin remains unset.
pub async fn stage(caller: Principal, allocation: OutageAllocation) -> Result<OutageProgress, String> {
    if !state::is_admin(caller) {
        return Err("unauthorized".into());
    }
    let digest = approved_digest(&allocation)?;
    if let Some(saved) = progress() {
        return if saved.allocation_digest == digest {
            Ok(saved)
        } else {
            Err("this outage already has an immutable allocation".into())
        };
    }
    if credit_count() != 0 {
        return Err("outage credit log exists without its progress record".into());
    }
    let _epoch_guard = state::EpochGuard::new().ok_or("epoch transition active")?;
    let _poll_guard = state::PollGuard::new().ok_or("points writer active")?;
    let before=state::with_state(Clone::clone);
    let sources=state::source_canisters();let ledgers=state::asset_ledgers();
    let policy=state::fiat_stable_points_policy();
    let config=(state::epoch_driver_interval_secs(),state::poll_interval_secs(),state::poll_enabled());
    crate::poll::verify_backlog_drained().await?;
    if state::with_state(Clone::clone)!=before || state::source_canisters()!=sources || state::asset_ledgers()!=ledgers
        || state::fiat_stable_points_policy()!=policy
        || (state::epoch_driver_interval_secs(),state::poll_interval_secs(),state::poll_enabled())!=config {
        return Err("state or source bindings changed during allocation validation".into());
    }
    require_accounting_boundary()?;
    if (allocation.season_start_ns, allocation.season_end_ns) != state::season_bounds()
        || allocation.original_ledger_prefix_len != 417
        || allocation.evidence_digest == [0; 32]
        || allocation.original_ledger_prefix_digest == [0; 32]
    {
        return Err("unexpected incident, season or evidence binding".into());
    }
    let last=last_affected_epoch()?;
    let participants:Vec<Principal>=match crate::recovery_v2::coverage(){
        Some(r)=>{
            let cutoff=r.normal_start_ns.ok_or("normal opening cutoff not sealed")?;
            crate::recovery_v2::census()?.into_iter().filter(|p|state::get_principal_state(p)
                .is_some_and(|s|s.registered_at_ns<cutoff)).collect()
        },
        None=>allocation.rows.iter().filter(|r|r.epoch_index==FIRST_EPOCH).map(|r|r.principal).collect(),
    };
    if participants.is_empty() || participants.len() as u64>crate::recovery_v2::MAX_PARTICIPANTS {
        return Err("recovery participant census is empty or too large".into());
    }
    if (last-FIRST_EPOCH+1).checked_mul(participants.len() as u64).is_none_or(|n|n>MAX_ALLOCATION_ROWS as u64) { return Err("allocation census/period product exceeds reviewed row bound".into()); }
    let expected:Vec<_>=(FIRST_EPOCH..=last).flat_map(|epoch|participants.iter().map(move|p|(epoch,*p))).collect();
    let keys:Vec<_>=allocation.rows.iter().map(|r|(r.epoch_index,r.principal)).collect();
    if keys!=expected||keys.windows(2).any(|p|p[0]>=p[1]){
        return Err("allocation must cover every audited participant and affected period in canonical order".into());
    }
    for row in &allocation.rows {
        let (_,end,_)=accounting_period(row)?;
        let participant=state::get_principal_state(&row.principal).ok_or("allocation participant missing")?;
        if (participant.registered_at_ns>=end||state::is_excluded(&row.principal)) && row.target_epoch_points!=0 {
            return Err("ineligible participant/period must have a zero target".into());
        }
    }
    let allocation_row_count=allocation.rows.len();
    let saved = OutageProgress {
        allocation_digest: digest,
        allocation,
        ledger_cutoff: state::point_ledger_len(),
        scanned_through: 0,
        original_prefix_digest_so_far: crate::snapshot_seed::sha256(&[PREFIX_DOMAIN]),
        original_registrations: Vec::new(),
        earned: vec![SourcePoints::default(); allocation_row_count],
        applied: Vec::new(),
    };
    persist(saved.clone());
    Ok(saved)
}

/// Each call scans at most 100 original ledger rows OR applies one allocation
/// row. All writes in an application call are synchronous with its receipt;
/// traps roll back the additions, aggregate total and receipt together.
pub fn continue_reconciliation(caller: Principal, now_ns: u64) -> Result<OutageProgress, String> {
    if !state::is_admin(caller) {
        return Err("unauthorized".into());
    }
    APPROVED_ALLOCATION_DIGEST.ok_or("no reviewed outage allocation is configured")?;
    let mut saved = progress().ok_or("no staged outage allocation")?;
    if approved_digest(&saved.allocation)? != saved.allocation_digest {
        return Err("stored allocation binding changed".into());
    }
    let _epoch_guard = state::EpochGuard::new().ok_or("epoch transition active")?;
    let _poll_guard = state::PollGuard::new().ok_or("points writer active")?;
    require_accounting_boundary()?;
    require_current_census(&saved)?;
    let receipted_count = saved
        .applied
        .iter()
        .filter(|row| row.credit_log_offset.is_some())
        .count() as u64;
    if credit_count() != receipted_count {
        return Err("outage credit log and durable receipts disagree".into());
    }

    if state::point_ledger_len() < saved.ledger_cutoff
        || saved.ledger_cutoff < saved.allocation.original_ledger_prefix_len
    {
        return Err("frozen ledger cutoff is inconsistent".into());
    }
    if saved.scanned_through < saved.ledger_cutoff {
        let page = state::point_entries(
            saved.scanned_through,
            (saved.ledger_cutoff - saved.scanned_through).min(u64::from(SCAN_CHUNK)) as u32,
        );
        if page.next_offset <= saved.scanned_through
            || page.next_offset > saved.ledger_cutoff
            || page.entries.len() as u64 != page.next_offset - saved.scanned_through
        {
            return Err("frozen ledger scan did not advance within its bounds".into());
        }
        for (offset, entry) in (saved.scanned_through..).zip(page.entries) {
            if offset < saved.allocation.original_ledger_prefix_len {
                if entry.epoch_index >= FIRST_EPOCH {
                    return Err("original prefix already includes an affected epoch".into());
                }
                let bytes = candid::encode_one(&entry)
                    .map_err(|e| format!("encode original entry: {e}"))?;
                saved.original_prefix_digest_so_far = crate::snapshot_seed::sha256(&[
                    &saved.original_prefix_digest_so_far,
                    &offset.to_le_bytes(),
                    &bytes,
                ]);
                if entry.source == PointSource::Registration {
                    if saved.original_registrations.contains(&entry.principal)
                        || saved.original_registrations.len() >= PARTICIPANTS
                    {
                        return Err("original registrations changed".into());
                    }
                    saved.original_registrations.push(entry.principal);
                }
                if offset + 1 == saved.allocation.original_ledger_prefix_len {
                    saved.original_registrations.sort();
                    let first_epoch_start=crate::epoch::epoch_bounds(FIRST_EPOCH,saved.allocation.season_start_ns,saved.allocation.season_end_ns).0;
                    let expected:Vec<_>=saved.allocation.rows.iter().filter(|r|r.epoch_index==FIRST_EPOCH)
                        .filter(|r|state::get_principal_state(&r.principal).is_some_and(|p|p.registered_at_ns<=first_epoch_start))
                        .map(|r|r.principal).collect();
                    if saved.original_prefix_digest_so_far
                        != saved.allocation.original_ledger_prefix_digest
                        || saved.original_registrations != expected
                    {
                        return Err("original Candid ledger entries or participant set do not match the reviewed prefix".into());
                    }
                }
            }
            if let Some(index) = saved.allocation.rows.iter().position(|row| {
                row.epoch_index == entry.epoch_index && row.principal == entry.principal
            }) {
                if entry.source != PointSource::Registration && accounting_period(&saved.allocation.rows[index])?.2 {
                    saved.earned[index].add(entry.source, entry.points_delta)?;
                }
            }
        }
        saved.scanned_through = page.next_offset;
        persist(saved.clone());
        return Ok(saved);
    }
    if saved.original_prefix_digest_so_far != saved.allocation.original_ledger_prefix_digest {
        return Err("original ledger prefix verification is incomplete".into());
    }
    let index = saved.applied.len();
    if index == saved.allocation.rows.len() {
        return Ok(saved);
    }
    let row = &saved.allocation.rows[index];
    if state::is_excluded(&row.principal) && row.target_epoch_points!=0 {
        return Err("participant eligibility changed".into());
    }
    let (coverage_start,coverage_end,_)=accounting_period(row)?;
    let normal_points_earned = saved.earned[index].total()?;
    let target_epoch_points = row.target_epoch_points;
    let credited_points = target_epoch_points.saturating_sub(normal_points_earned);
    state::preflight_outage_credit(&row.principal, credited_points)?;
    let (scheduled_period_start_ns, scheduled_period_end_ns) = crate::epoch::epoch_bounds(
        row.epoch_index,
        saved.allocation.season_start_ns,
        saved.allocation.season_end_ns,
    );
    // No returned error or await follows this first write. The separate audit
    // row, aggregate increment and receipt commit or trap/roll back together.
    let credit_log_offset = if credited_points > 0 {
        Some(append_credit(OutageCredit {
            kind: OutageCreditKind::OutageMakeWhole,
            allocation_digest: saved.allocation_digest,
            evidence_digest: saved.allocation.evidence_digest,
            principal: row.principal,
            epoch_index: row.epoch_index,
            scheduled_period_start_ns,
            scheduled_period_end_ns,
            coverage_start_ns:Some(coverage_start),
            coverage_end_ns:Some(coverage_end),
            normal_ledger_cutoff: saved.ledger_cutoff,
            normal_points_earned,
            target_epoch_points,
            credited_points,
            applied_at_ns: now_ns,
        }))
    } else {
        None
    };
    state::commit_outage_credit(&row.principal, credited_points);
    saved.applied.push(AppliedRow {
        credited_points,
        credit_log_offset,
        applied_at_ns: now_ns,
    });
    persist(saved.clone());
    Ok(saved)
}
