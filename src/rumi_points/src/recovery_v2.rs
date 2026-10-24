//! Explicit late recovery for the held October epoch. Missing intervals are
//! evidence for reconciliation, never synthetic captures or zero summaries.
use crate::{epoch, snapshot_seed::sha256, state};
use candid::{CandidType, Principal};
use ic_stable_structures::{storable::Bound, StableCell, Storable};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, cell::RefCell};

const DOMAIN: &[u8] = b"rumi-points-late-recovery-bridge-v2\0";
pub const MAX_PARTICIPANTS: u64 = 100;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RecoveryCoverage {
    pub authorized_at_ns: u64,
    pub predecessor_epoch: u64,
    pub season_start_ns: u64,
    pub season_end_ns: u64,
    /// Epoch18 is reconciled as a whole, offset by its real delayed-B credits.
    pub missed_start_ns: u64,
    /// None until the SAME message that opens the real normal epoch.
    pub normal_epoch: Option<u64>,
    pub normal_start_ns: Option<u64>,
    pub normal_end_ns: Option<u64>,
    pub predecessor_summary_digest: Option<[u8; 32]>,
    pub predecessor_next_commit: Option<[u8; 32]>,
    pub bridge_digest: Option<[u8; 32]>,
    pub normal_seed_commit: Option<[u8; 32]>,
    /// Opening observations only; staging refreshes eligibility after source catch-up.
    pub participants: Vec<Principal>,
}

#[derive(Serialize, Deserialize, Clone)]
enum Stored {
    V1(Option<RecoveryCoverage>),
}
impl Storable for Stored {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut b = Vec::new();
        ciborium::ser::into_writer(self, &mut b).expect("encode recovery coverage");
        Cow::Owned(b)
    }
    fn from_bytes(b: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(b.as_ref()).expect("decode recovery coverage")
    }
    const BOUND: Bound = Bound::Unbounded;
}
thread_local! {
    static COVERAGE:RefCell<StableCell<Stored,state::VMem>>=RefCell::new(
        StableCell::init(state::late_recovery_memory(),Stored::V1(None)).expect("init recovery coverage"));
}
pub fn coverage() -> Option<RecoveryCoverage> {
    COVERAGE.with(|c| match c.borrow().get() {
        Stored::V1(r) => r.clone(),
    })
}
fn persist(r: RecoveryCoverage) {
    COVERAGE.with(|c| {
        c.borrow_mut()
            .set(Stored::V1(Some(r)))
            .expect("persist recovery coverage");
    });
}
pub(crate) fn census() -> Result<Vec<Principal>, String> {
    let p = state::registered_chunk_after(None, MAX_PARTICIPANTS + 1);
    if p.len() as u64 > MAX_PARTICIPANTS || p.len() as u64 != state::registered_count() {
        return Err("late recovery participant census exceeds its reviewed bound".into());
    }
    Ok(p)
}
pub fn authorize(now: u64, missed_start_ns: u64) -> Result<(), String> {
    if coverage().is_some() {
        return Err("late recovery already authorized".into());
    }
    let (season_start_ns, season_end_ns) = state::season_bounds();
    let participants = census()?;
    persist(RecoveryCoverage {
        authorized_at_ns: now,
        predecessor_epoch: 18,
        season_start_ns,
        season_end_ns,
        missed_start_ns,
        normal_epoch: None,
        normal_start_ns: None,
        normal_end_ns: None,
        predecessor_summary_digest: None,
        predecessor_next_commit: None,
        bridge_digest: None,
        normal_seed_commit: None,
        participants,
    });
    Ok(())
}
/// Budget is a selection rule, not proof that all external sources stay available.
/// Captures retain their retry cursors if a source subsequently fails.
pub fn choose_epoch(
    now: u64,
    season_start: u64,
    season_end: u64,
) -> Result<(u64, u64, u64), String> {
    let budget = state::registered_count()
        .div_ceil(epoch::CAPTURE_CHUNK)
        .saturating_mul(2)
        .saturating_add(6)
        .checked_mul(state::epoch_driver_interval_secs())
        .and_then(|x| x.checked_mul(1_000_000_000))
        .ok_or("normal recovery capture budget overflow")?;
    let index = now.saturating_sub(season_start) / epoch::EPOCH_DURATION_NS;
    let mut index = index.max(19);
    let (mut start, mut end) = epoch::epoch_bounds(index, season_start, season_end);
    if end <= now.saturating_add(budget) {
        index = index
            .checked_add(1)
            .ok_or("normal recovery epoch overflow")?;
        (start, end) = epoch::epoch_bounds(index, season_start, season_end);
    }
    if start >= end || end <= now.saturating_add(budget) {
        return Err("no viable in-season normal recovery window".into());
    }
    Ok((index, start, end))
}
/// Called synchronously with the REAL close18. The preloaded private successor
/// seed remains committed; no fake seed or summary is created for skipped IDs.
pub fn record_close(summary_digest: [u8; 32]) {
    if let Some(mut r) = coverage() {
        if r.normal_start_ns.is_none() {
            assert!(
                r.predecessor_summary_digest.is_none(),
                "recovery close already recorded"
            );
            r.predecessor_summary_digest = Some(summary_digest);
            r.predecessor_next_commit = Some(state::get_pending_commit());
            persist(r);
        }
    }
}
pub fn pending_open() -> bool {
    coverage()
        .is_some_and(|r| r.normal_start_ns.is_none() && r.predecessor_summary_digest.is_some())
}

/// No await and no fresh entropy draw: the closing entropy is still secret.
/// An open, its bridge, census and exact coverage cutoff commit atomically.
pub fn open_pending(now: u64) -> Result<(), String> {
    let mut r = coverage().ok_or("late recovery missing")?;
    if r.normal_start_ns.is_some() {
        return Ok(());
    }
    if state::get_open_epoch().is_some() || state::current_epoch_index() != 19 {
        return Err("late recovery expected closed18 and unopened19".into());
    }
    if state::season_bounds() != (r.season_start_ns, r.season_end_ns) {
        return Err("season bounds changed during late recovery".into());
    }
    let summary = r
        .predecessor_summary_digest
        .ok_or("real close18 is incomplete")?;
    let prior_commit = r
        .predecessor_next_commit
        .ok_or("predecessor seed commitment missing")?;
    if state::get_pending_commit() != prior_commit {
        return Err("predecessor seed commitment changed".into());
    }
    let (index, start, end) = choose_epoch(now, r.season_start_ns, r.season_end_ns)?;
    if now < start {
        return Ok(());
    }
    let seed = state::current_epoch_seed().ok_or("predecessor private seed missing")?;
    let entropy = state::current_epoch_entropy().ok_or("predecessor private entropy missing")?;
    if crate::snapshot_seed::commitment(&seed) != prior_commit {
        return Err("private seed does not match predecessor commit".into());
    }
    r.participants = census()?;
    let bridge = sha256(&[
        DOMAIN,
        &r.predecessor_epoch.to_le_bytes(),
        &summary,
        &prior_commit,
        &r.missed_start_ns.to_le_bytes(),
        &index.to_le_bytes(),
        &now.to_le_bytes(),
        &end.to_le_bytes(),
    ]);
    let normal_seed = sha256(&[DOMAIN, &seed, &bridge]);
    state::install_reseeded_epoch_seed(normal_seed, entropy);
    // Any unexpected error traps so the preloaded commit cannot partially change.
    epoch::open_epoch_at(index, now, end, None)
        .unwrap_or_else(|e| ic_cdk::trap(&format!("late recovery opening failed: {e:?}")));
    state::with_state_mut(|s| s.current_epoch_index = index);
    r.normal_epoch = Some(index);
    r.normal_start_ns = Some(now);
    r.normal_end_ns = Some(end);
    r.bridge_digest = Some(bridge);
    r.normal_seed_commit = Some(state::get_pending_commit());
    persist(r);
    Ok(())
}

pub fn fully_missed_epoch(index: u64) -> bool {
    coverage().is_some_and(|r| {
        r.normal_epoch
            .is_some_and(|target| index > 18 && index < target)
    })
}
/// All fully missed epochs plus a non-empty pre-opening prefix are affected.
/// The prefix is disjoint from normal accrual, so its normal close is not needed.
pub fn last_affected_epoch() -> Result<u64, String> {
    let r = coverage().ok_or("late recovery coverage not authorized")?;
    let index = r.normal_epoch.ok_or("normal recovery has not opened")?;
    let start = r.normal_start_ns.ok_or("normal recovery opening missing")?;
    let scheduled = epoch::epoch_bounds(index, r.season_start_ns, r.season_end_ns).0;
    Ok(if start == scheduled { index - 1 } else { index })
}
