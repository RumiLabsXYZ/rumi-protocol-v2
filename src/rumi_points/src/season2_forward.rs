//! One-shot S2 forward transition: archive unresolved evidence, never close or
//! award its missed epoch. Only the newly observed window can accrue normally.
use crate::{accrual::SnapshotWeights, epoch, poll, snapshot_seed, state, types::*};
use candid::{CandidType, Principal};
use ic_stable_structures::{storable::Bound, StableCell, Storable};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, cell::RefCell};

const DOMAIN: &[u8] = b"rumi-s2-pending-coverage-forward-v1\0";
const MAX_PARTICIPANTS: u64 = 100;
const MAX_ARCHIVE_BYTES: usize = 1_048_576;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ForwardStatus {
    pub prepared_at_ns: u64,
    pub expires_at_ns: u64,
    pub backup_snapshot_id: Vec<u8>,
    pub archive_digest: Vec<u8>,
    pub archived_epoch: u64,
    pub archived_participants: u64,
    pub archived_snapshot_rows: u64,
    pub pending_start_ns: u64,
    /// None while prepared; actual activation time once forward scoring opens.
    pub pending_end_ns: Option<u64>,
    pub active_epoch: Option<u64>,
    pub active_start_ns: Option<u64>,
    pub active_end_ns: Option<u64>,
    pub active_seed_commit: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Archive {
    state: state::State,
    snapshots: Vec<(Principal, SnapshotWeights)>,
    principals: Vec<PrincipalState>,
    bootstrap: state::Season2ImportStatus,
    backfill: Option<state::Season2ImportStatus>,
    sources: Vec<(u8, Principal)>,
    ledgers: Vec<(u8, Principal)>,
    cursors: Vec<u64>,
    policy: FiatStablePointsPolicy,
    ordinary_ledger_len: u64,
    history_len: u64,
    revealed_seed_len: u64,
    driver_interval_secs: u64,
    poll_interval_secs: u64,
}
#[derive(Serialize, Deserialize, Clone)]
struct Record {
    archive: Archive,
    view: ForwardStatus,
}
#[derive(Serialize, Deserialize, Clone, Default)]
enum Stored {
    #[default]
    Empty,
    V1(Record),
}
fn encode<T: Serialize>(x: &T) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(x, &mut b).expect("encode S2 forward archive");
    b
}
impl Storable for Stored {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(encode(self))
    }
    fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(bytes.as_ref()).expect("decode S2 forward archive")
    }
    const BOUND: Bound = Bound::Unbounded;
}
thread_local! {
    static FORWARD: RefCell<StableCell<Stored, state::VMem>> = RefCell::new(
        StableCell::init(state::season2_forward_memory(), Stored::Empty).expect("init S2 forward archive"));
}
fn record() -> Option<Record> {
    FORWARD.with(|c| match c.borrow().get() {
        Stored::Empty => None,
        Stored::V1(r) => {
            assert_eq!(
                snapshot_seed::sha256(&[DOMAIN, &encode(&r.archive)]).to_vec(),
                r.view.archive_digest,
                "S2 archive integrity mismatch"
            );
            Some(r.clone())
        }
    })
}
fn persist(r: Record) {
    FORWARD.with(|c| {
        c.borrow_mut()
            .set(Stored::V1(r))
            .expect("persist S2 forward archive");
    });
}
pub fn status() -> Option<ForwardStatus> {
    record().map(|r| r.view)
}
fn admin(caller: Principal) -> Result<(), String> {
    if state::is_admin(caller) {
        Ok(())
    } else {
        Err("unauthorized".into())
    }
}
fn census() -> Result<Vec<PrincipalState>, String> {
    let ids = state::registered_chunk_after(None, MAX_PARTICIPANTS + 1);
    if ids.len() as u64 != state::registered_count() || ids.len() as u64 > MAX_PARTICIPANTS {
        return Err("S2 forward participant bound exceeded".into());
    }
    ids.into_iter()
        .map(|p| state::get_principal_state(&p).ok_or("registered state missing".into()))
        .collect()
}
fn check_held() -> Result<(), String> {
    let open = state::get_open_epoch().ok_or("no held S2 epoch")?;
    if !state::legacy_transition_held()
        || state::epoch_driver_enabled()
        || state::poll_enabled()
        || state::current_epoch_index() != 0
        || open.epoch_index != 0
        || !open.a_complete
        || open.b_complete
        || open.b_cursor.is_some()
        || open.close_started
        || open.close_cursor.is_some()
        || open.close_active != 0
        || open.close_points_accrued != 0
        || ic_cdk::api::time() < open.epoch_end_ns
    {
        return Err(
            "expected paused, held, expired S2 epoch0 with A complete and B/close untouched".into(),
        );
    }
    let bootstrap = state::season2_import_status().ok_or("S2 bootstrap missing")?;
    if !bootstrap.complete
        || bootstrap.credited_points != 0
        || state::season2_backfill_status().is_some()
        || state::epoch_count() != 0
        || state::revealed_seed_count() != 0
        || census()?.iter().any(|p| p.total_points != 0)
    {
        return Err(
            "expected completed zero-credit S2 bootstrap without prior awards/backfill".into(),
        );
    }
    Ok(())
}
fn live_digest() -> Result<[u8; 32], String> {
    let bytes = encode(&(
        state::with_state(Clone::clone),
        state::source_canisters(),
        state::asset_ledgers(),
        (0..4).map(state::get_cursor).collect::<Vec<_>>(),
        state::fiat_stable_points_policy(),
        state::epoch_driver_interval_secs(),
        state::poll_interval_secs(),
        census()?,
        state::point_ledger_len(),
        state::season2_import_status(),
        state::season2_backfill_status(),
        state::snapshot_buffer_entries(),
    ));
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err("S2 forward live-state byte bound exceeded".into());
    }
    Ok(snapshot_seed::sha256(&[DOMAIN, &bytes]))
}

/// Store original epoch0 evidence BEFORE held backlog replay changes positions.
/// Replays return the same receipt; they never extend the current-epoch capture deadline.
pub fn prepare(caller: Principal, backup_snapshot_id: Vec<u8>) -> Result<ForwardStatus, String> {
    admin(caller)?;
    if let Some(r) = record() {
        if r.view.backup_snapshot_id != backup_snapshot_id {
            return Err("S2 forward backup binding differs".into());
        }
        return Ok(r.view);
    }
    let _epoch = state::EpochGuard::new().ok_or("epoch transition active")?;
    let _poll = state::PollGuard::new().ok_or("points poll active")?;
    check_held()?;
    if backup_snapshot_id.is_empty() || backup_snapshot_id.len() > 256 {
        return Err("backup snapshot id required".into());
    }
    let now = ic_cdk::api::time();
    let archive = Archive {
        state: state::with_state(Clone::clone),
        snapshots: state::snapshot_buffer_entries(),
        principals: census()?,
        bootstrap: state::season2_import_status().ok_or("bootstrap missing")?,
        backfill: state::season2_backfill_status(),
        sources: state::source_canisters(),
        ledgers: state::asset_ledgers(),
        cursors: (0..4).map(state::get_cursor).collect(),
        policy: state::fiat_stable_points_policy(),
        ordinary_ledger_len: state::point_ledger_len(),
        history_len: state::epoch_count(),
        revealed_seed_len: state::revealed_seed_count(),
        driver_interval_secs: state::epoch_driver_interval_secs(),
        poll_interval_secs: state::poll_interval_secs(),
    };
    if archive.snapshots.len() as u64 > MAX_PARTICIPANTS {
        return Err("snapshot archive bound exceeded".into());
    }
    let bytes = encode(&archive);
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err("S2 archive byte bound exceeded".into());
    }
    let (_, close) = window(
        now,
        archive.state.season_start_ns,
        archive.state.season_end_ns,
        MAX_PARTICIPANTS,
        archive.driver_interval_secs,
    )?;
    let expires_at_ns = close
        .checked_sub(capture_budget(
            MAX_PARTICIPANTS,
            archive.driver_interval_secs,
        )?)
        .ok_or("capture deadline overflow")?;
    let view = ForwardStatus {
        prepared_at_ns: now,
        expires_at_ns,
        backup_snapshot_id,
        archive_digest: snapshot_seed::sha256(&[DOMAIN, &bytes]).to_vec(),
        archived_epoch: 0,
        archived_participants: archive.principals.len() as u64,
        archived_snapshot_rows: archive.snapshots.len() as u64,
        pending_start_ns: archive.state.season_start_ns,
        pending_end_ns: None,
        active_epoch: None,
        active_start_ns: None,
        active_end_ns: None,
        active_seed_commit: None,
    };
    persist(Record {
        archive,
        view: view.clone(),
    });
    Ok(view)
}

fn capture_budget(count: u64, interval: u64) -> Result<u64, String> {
    count
        .div_ceil(epoch::CAPTURE_CHUNK)
        .saturating_mul(2)
        .saturating_add(6)
        .checked_mul(interval)
        .and_then(|n| n.checked_mul(1_000_000_000))
        .ok_or("capture budget overflow".into())
}
fn window(now: u64, start: u64, end: u64, count: u64, interval: u64) -> Result<(u64, u64), String> {
    let index = now.checked_sub(start).ok_or("season not started")? / epoch::EPOCH_DURATION_NS;
    let (_, epoch_end) = epoch::epoch_bounds(index, start, end);
    let budget = capture_budget(count, interval)?;
    if index != 1 || epoch_end <= now.saturating_add(budget) {
        return Err("insufficient current-epoch capture window; remain held".into());
    }
    Ok((index, epoch_end))
}

/// A source-checked, one-shot bridge. No epoch0 summary, seed reveal or point
/// entry is synthesized. Pending coverage ends exactly where normal accrual starts.
pub async fn activate(
    caller: Principal,
    expected_archive_digest: Vec<u8>,
) -> Result<ForwardStatus, String> {
    admin(caller)?;
    let mut r = record().ok_or("prepare S2 archive first")?;
    if expected_archive_digest != r.view.archive_digest {
        return Err("archive digest mismatch".into());
    }
    if r.view.active_start_ns.is_some() {
        return Ok(r.view);
    }
    let _epoch = state::EpochGuard::new().ok_or("epoch transition active")?;
    let _poll = state::PollGuard::new().ok_or("points poll active")?;
    check_held()?;
    if state::with_state(Clone::clone) != r.archive.state
        || state::source_canisters() != r.archive.sources
        || state::asset_ledgers() != r.archive.ledgers
        || state::fiat_stable_points_policy() != r.archive.policy
        || state::snapshot_buffer_entries() != r.archive.snapshots
        || state::epoch_driver_interval_secs() != r.archive.driver_interval_secs
        || state::poll_interval_secs() != r.archive.poll_interval_secs
        || state::season2_import_status() != Some(r.archive.bootstrap.clone())
    {
        return Err("archived epoch or source/policy binding changed".into());
    }
    let before = live_digest()?;
    if ic_cdk::api::time() >= r.view.expires_at_ns {
        return Err("forward activation window expired; remain held".into());
    }
    poll::verify_held_backlog_drained().await?;
    let (entropy,) = ic_cdk::api::management_canister::main::raw_rand()
        .await
        .map_err(|e| format!("fresh entropy unavailable: {e:?}"))?;
    let entropy: [u8; 32] = entropy.try_into().map_err(|_| "invalid entropy")?;
    check_held()?;
    let now = ic_cdk::api::time();
    if now >= r.view.expires_at_ns || live_digest()? != before {
        return Err("activation state or deadline changed during awaits; remain held".into());
    }
    let (start, end) = state::season_bounds();
    let (index, close) = window(
        now,
        start,
        end,
        state::registered_count(),
        state::epoch_driver_interval_secs(),
    )?;
    let seed = snapshot_seed::sha256(&[
        DOMAIN,
        &entropy,
        &r.view.archive_digest,
        &index.to_le_bytes(),
        &now.to_le_bytes(),
        &close.to_le_bytes(),
    ]);
    state::install_reseeded_epoch_seed(seed, entropy);
    epoch::open_epoch_at(index, now, close, None)
        .unwrap_or_else(|e| ic_cdk::trap(&format!("forward open failed: {e:?}")));
    state::with_state_mut(|s| s.current_epoch_index = index);
    state::set_legacy_transition_held(false);
    state::set_legacy_reseed_pending(false);
    state::set_poll_enabled(caller, true)
        .unwrap_or_else(|e| ic_cdk::trap(&format!("enable poll: {e:?}")));
    state::set_epoch_driver_enabled(caller, true)
        .unwrap_or_else(|e| ic_cdk::trap(&format!("enable driver: {e:?}")));
    r.view.pending_end_ns = Some(now);
    r.view.active_epoch = Some(index);
    r.view.active_start_ns = Some(now);
    r.view.active_end_ns = Some(close);
    r.view.active_seed_commit = Some(snapshot_seed::commitment(&seed).to_vec());
    persist(r.clone());
    Ok(r.view)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forward_window_uses_current_calendar_epoch_and_refuses_short_tail() {
        let week = epoch::EPOCH_DURATION_NS;
        assert_eq!(window(week + 1, 0, u64::MAX, 15, 300), Ok((1, 2 * week)));
        assert!(window(2 * week - 1, 0, u64::MAX, 15, 300).is_err());
        assert!(window(1, 0, u64::MAX, 15, 300).is_err());
        assert!(window(2 * week + 1, 0, u64::MAX, 15, 300).is_err());
        assert!(window(2 * week + 1, 0, 2 * week, 15, 300).is_err());
    }
}
