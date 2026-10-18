//! Additive accounting for the October 2026 points outage.
//!
//! This is a draft implementation, not an allocation policy. No approved
//! allocation digest is configured, so staging is fail-closed. The existing
//! snapshot driver, hold, seed chain and original ledger encodings are intact.
//! A future reviewed allocation must specify explicit total floors for the FULL
//! scheduled epochs 18 and 19. Reconciliation runs only after both epochs have
//! actually closed, subtracting their original credits before adding anything.
//! Missing random snapshots are never represented as observed captures.

use std::borrow::Cow;
use std::cell::RefCell;

use candid::{CandidType, Principal};
use ic_stable_structures::{log::Log as StableLog, storable::Bound, StableCell, Storable};
use serde::{Deserialize, Serialize};

use crate::state;
use crate::types::PointSource;

const FIRST_EPOCH: u64 = 18;
const LAST_EPOCH: u64 = 19;
const PARTICIPANTS: usize = 15;
const SCAN_CHUNK: u32 = 100;
const DOMAIN: &[u8] = b"rumi-points-october-2026-outage-allocation-v1\0";
const PREFIX_DOMAIN: &[u8] = b"rumi-points-october-2026-original-ledger-v1\0";

// Deliberately unset: evidence worksheets are not an approved allocation.
// Setting this requires a separately reviewed immutable allocation manifest.
const APPROVED_ALLOCATION_DIGEST: Option<[u8; 32]> = None;

/// Reference breakdown for a total epoch floor, not ordinary earning records.
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
}

/// All 30 rows are mandatory, including zero targets. The input-evidence digest
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

fn persist(progress: OutageProgress) {
    PROGRESS.with(|cell| {
        cell.borrow_mut()
            .set(StoredOutageReconciliation::V1(Some(progress)))
            .expect("persist outage reconciliation");
    });
}

pub fn allocation_digest(allocation: &OutageAllocation) -> Result<[u8; 32], String> {
    if allocation.rows.len() != PARTICIPANTS * 2 {
        return Err("allocation must contain exactly 30 ordered participant/epoch rows".into());
    }
    allocation.rows.iter().try_fold(0u128, |sum, row| {
        sum.checked_add(row.target_epoch_points)
            .ok_or("combined allocation exceeds u128")
    })?;
    let bytes = candid::encode_one(allocation).map_err(|e| format!("encode allocation: {e}"))?;
    Ok(crate::snapshot_seed::sha256(&[DOMAIN, &bytes]))
}

fn require_closed_epochs() -> Result<(), String> {
    if state::current_epoch_index() <= LAST_EPOCH {
        return Err("both affected epochs must close before reconciliation".into());
    }
    for index in FIRST_EPOCH..=LAST_EPOCH {
        let summary = state::epoch_history(index, 1);
        if summary.len() != 1 || summary[0].epoch_index != index {
            return Err("affected epoch summary is missing".into());
        }
        let (start, end) =
            crate::epoch::epoch_bounds(index, state::season_bounds().0, state::season_bounds().1);
        if summary[0].epoch_start_ns < start
            || summary[0].epoch_start_ns >= end
            || summary[0].epoch_end_ns != end
        {
            return Err("affected epoch summary has unexpected coverage".into());
        }
    }
    let policy = state::fiat_stable_points_policy();
    if policy.cutover_epoch != Some(19)
        || policy.legacy_epoch != Some(18)
        || !policy.historical_complete
        || !policy.inline_legacy_topups_complete
    {
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

/// Called only by a future reviewed admin ingress. Kept out of main.rs while
/// allocation policy, source completeness and execution disposition are open.
pub fn stage(caller: Principal, allocation: OutageAllocation) -> Result<OutageProgress, String> {
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
    require_closed_epochs()?;
    if (allocation.season_start_ns, allocation.season_end_ns) != state::season_bounds()
        || allocation.original_ledger_prefix_len != 417
        || allocation.evidence_digest == [0; 32]
        || allocation.original_ledger_prefix_digest == [0; 32]
    {
        return Err("unexpected incident, season or evidence binding".into());
    }
    let mut keys = Vec::with_capacity(allocation.rows.len());
    for row in &allocation.rows {
        if row.epoch_index < FIRST_EPOCH || row.epoch_index > LAST_EPOCH {
            return Err("allocation is restricted to epochs 18 and 19".into());
        }
        let (start, _) = crate::epoch::epoch_bounds(
            row.epoch_index,
            allocation.season_start_ns,
            allocation.season_end_ns,
        );
        let participant =
            state::get_principal_state(&row.principal).ok_or("allocation participant missing")?;
        if participant.registered_at_ns > start || state::is_excluded(&row.principal) {
            return Err("allocation includes an ineligible participant".into());
        }
        keys.push((row.epoch_index, row.principal));
    }
    if keys.windows(2).any(|pair| pair[0] >= pair[1])
        || keys[..PARTICIPANTS].iter().any(|key| key.0 != FIRST_EPOCH)
        || keys[PARTICIPANTS..].iter().any(|key| key.0 != LAST_EPOCH)
        || keys[..PARTICIPANTS]
            .iter()
            .map(|key| key.1)
            .ne(keys[PARTICIPANTS..].iter().map(|key| key.1))
    {
        return Err("allocation must cover the same 15 unique principals in both epochs, in canonical order".into());
    }
    let saved = OutageProgress {
        allocation_digest: digest,
        allocation,
        ledger_cutoff: state::point_ledger_len(),
        scanned_through: 0,
        original_prefix_digest_so_far: crate::snapshot_seed::sha256(&[PREFIX_DOMAIN]),
        original_registrations: Vec::new(),
        earned: vec![SourcePoints::default(); PARTICIPANTS * 2],
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
    require_closed_epochs()?;
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
                    let expected: Vec<_> = saved.allocation.rows[..PARTICIPANTS]
                        .iter()
                        .map(|row| row.principal)
                        .collect();
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
                if entry.source != PointSource::Registration {
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
    if state::is_excluded(&row.principal) {
        return Err("participant eligibility changed".into());
    }
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
