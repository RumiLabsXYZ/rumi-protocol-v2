//! Snapshot-timing randomization (spike 0.3, `2026-05-07-spike-0.3-snapshot-randomization.md`).
//!
//! PHASE 1 SCOPE: the seed STATE types below are real and wired into the stable
//! layout (they must survive upgrades), but the commit-reveal ALGORITHM is a
//! documented skeleton. The `derive_snapshot_times` / `SeedManager` bodies land
//! in Phase 5 alongside the weekly epoch driver. Do not implement them here.
//!
//! Mechanism (commit-reveal with hash-chained per-epoch seeds, RANDAO-style):
//!   - At init the admin commits to a secret 32-byte seed S0 by storing only
//!     `H0 = sha256(S0)` on-chain (`pending_commit`). S0 is revealed after week 1.
//!   - `seed_N = sha256(seed_{N-1} || epoch_{N-1}_summary_hash)`.
//!   - Two snapshot times per epoch are derived from `seed_N`, one in each half
//!     of the week, so they are >= a few hours apart and unpredictable in
//!     advance but verifiable after the reveal.
//! Users cannot predict snapshot times; auditors can verify them post-hoc; the
//! team cannot retroactively change them (each reveal locks the next commit).

#![allow(dead_code)] // Phase 5 surface; types are used by the stable layout now.

use candid::CandidType;
use serde::{Deserialize, Serialize};

/// In-flight seed singleton. Lives inside `state::State` (so it rides the
/// versioned State blob across upgrades). `current_seed` is held privately until
/// the epoch closes, then appended to the `RevealedSeed` audit log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct SnapshotSeedSingleton {
    /// `sha256` of the seed the NEXT epoch must reveal. `[0u8; 32]` means "not
    /// yet committed" (Phase 1 default; admin/Phase 5 sets the real H0).
    pub pending_commit: [u8; 32],
    /// Plaintext seed for the current (open) epoch, revealed when it closes.
    pub current_seed: Option<[u8; 32]>,
}

impl SnapshotSeedSingleton {
    pub fn is_committed(&self) -> bool {
        self.pending_commit != [0u8; 32]
    }
}

/// One audit-log row per closed epoch (`StableLog<RevealedSeed>`). After an epoch
/// closes anyone can recompute the snapshot times from `seed` and verify they
/// match what actually fired.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RevealedSeed {
    pub epoch_index: u64,
    pub seed: [u8; 32],
    pub snapshot_time_a_ns: u64,
    pub snapshot_time_b_ns: u64,
    pub revealed_at_ns: u64,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SeedError {
    /// `sha256(seed_for_this_epoch)` did not equal the stored `pending_commit`.
    /// Indicates state corruption or tampering; the caller must halt the epoch.
    CommitMismatch,
    /// `start_epoch` was given no seed for epoch 1 and none was derivable.
    MissingSeed,
}

/// Derive the two intra-epoch snapshot timestamps from a seed (spike 0.3).
///
/// PHASE 5 (not implemented here). The algorithm, verbatim from the spike:
/// split the epoch into halves, take `seed[0..8]` mod `half` as snapshot A's
/// offset into the first half and `seed[8..16]` mod `half` as snapshot B's
/// offset into the second half. Guarantees A precedes B with a real gap.
pub fn derive_snapshot_times(
    _seed: &[u8; 32],
    _epoch_start_ns: u64,
    _epoch_end_ns: u64,
) -> (u64, u64) {
    unimplemented!("Phase 5: implement per spike 0.3 derive_snapshot_times");
}

/// Owns the commit-reveal chain across epochs. PHASE 5: full implementation per
/// the spike's `start_epoch` / `close_epoch` outline. Scaffolded here so the
/// epoch driver can be written against a stable interface.
pub struct SeedManager;

impl SeedManager {
    /// Verify the previously stored seed matches the pending commit, derive this
    /// epoch's snapshot times, and store the (not-yet-revealed) seed.
    pub fn start_epoch(
        _singleton: &mut SnapshotSeedSingleton,
        _epoch_index: u64,
        _epoch_start_ns: u64,
        _epoch_end_ns: u64,
        _prev_epoch_summary_hash: [u8; 32],
        _seed_for_this_epoch: Option<[u8; 32]>,
    ) -> Result<(u64, u64), SeedError> {
        unimplemented!("Phase 5: implement per spike 0.3 start_epoch");
    }

    /// Reveal the current seed, append it to the audit log, and compute the next
    /// epoch's commit `H_{N+1} = sha256(seed_N || epoch_N_summary_hash)`.
    pub fn close_epoch(
        _singleton: &mut SnapshotSeedSingleton,
        _epoch_index: u64,
        _epoch_summary_hash: [u8; 32],
    ) -> Result<RevealedSeed, SeedError> {
        unimplemented!("Phase 5: implement per spike 0.3 close_epoch");
    }
}
