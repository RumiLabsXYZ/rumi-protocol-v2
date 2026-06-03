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
use sha2::{Digest, Sha256};

/// SHA-256 over the concatenation of `parts`. The commit-reveal chain and the
/// epoch-summary hash both use this; kept here so the hashing convention has one
/// home.
pub(crate) fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// The on-chain commit `H0 = sha256(S0)` for a secret seed `S0`. The operator
/// computes this OFF-CHAIN from their secret `S0` and passes it as
/// `InitArgs.snapshot_seed_commit` at init; `start_season` later verifies
/// `sha256(S0) == H0`. Public so tooling (and the E2E) can derive the commit.
pub fn commitment(seed: &[u8; 32]) -> [u8; 32] {
    sha256(&[seed])
}

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
    seed: &[u8; 32],
    epoch_start_ns: u64,
    epoch_end_ns: u64,
) -> (u64, u64) {
    let half = epoch_end_ns.saturating_sub(epoch_start_ns) / 2;
    if half == 0 {
        // Degenerate (empty) epoch: nothing to spread the snapshots across.
        // Collapse both to the start and avoid the modulo-by-zero panic.
        return (epoch_start_ns, epoch_start_ns);
    }
    // First 8 bytes place snapshot A somewhere in the first half of the epoch;
    // the next 8 place snapshot B in the second half. The split guarantees A
    // precedes B with a real gap (spike 0.3).
    let a_offset = u64::from_le_bytes(seed[0..8].try_into().unwrap()) % half;
    let b_offset = u64::from_le_bytes(seed[8..16].try_into().unwrap()) % half;
    (epoch_start_ns + a_offset, epoch_start_ns + half + b_offset)
}

/// Owns the commit-reveal chain across epochs. PHASE 5: full implementation per
/// the spike's `start_epoch` / `close_epoch` outline. Scaffolded here so the
/// epoch driver can be written against a stable interface.
pub struct SeedManager;

impl SeedManager {
    /// Resolve this epoch's seed (the explicit arg for epoch 0, else the
    /// pre-loaded `current_seed` from the prior close), verify it against the
    /// pending commit (when committed), stash it as the open epoch's seed, and
    /// return the two snapshot times. `CommitMismatch` halts on tampering;
    /// `MissingSeed` if neither an arg nor a pre-loaded seed is available.
    pub fn start_epoch(
        singleton: &mut SnapshotSeedSingleton,
        epoch_start_ns: u64,
        epoch_end_ns: u64,
        seed_for_this_epoch: Option<[u8; 32]>,
    ) -> Result<(u64, u64), SeedError> {
        let seed = seed_for_this_epoch
            .or(singleton.current_seed)
            .ok_or(SeedError::MissingSeed)?;
        if singleton.is_committed() && sha256(&[&seed]) != singleton.pending_commit {
            // The seed for this epoch must hash to the commit locked in by the
            // previous reveal (or the init H0). A mismatch means corruption or
            // tampering; the driver must halt, not silently re-roll the times.
            return Err(SeedError::CommitMismatch);
        }
        singleton.current_seed = Some(seed);
        Ok(derive_snapshot_times(&seed, epoch_start_ns, epoch_end_ns))
    }

    /// Reveal the open epoch's seed as a `RevealedSeed` (the caller appends it to
    /// the audit log), then pre-load the next epoch's seed
    /// `next = sha256(seed || summary_hash)` and its commit `sha256(next)`. The
    /// snapshot times and reveal timestamp are passed in (kept by the driver in
    /// `State.open_epoch`) so the singleton needs no extra fields.
    pub fn close_epoch(
        singleton: &mut SnapshotSeedSingleton,
        epoch_index: u64,
        snapshot_a_ns: u64,
        snapshot_b_ns: u64,
        now_ns: u64,
        epoch_summary_hash: [u8; 32],
    ) -> Result<RevealedSeed, SeedError> {
        let seed = singleton.current_seed.ok_or(SeedError::MissingSeed)?;
        let revealed = RevealedSeed {
            epoch_index,
            seed,
            snapshot_time_a_ns: snapshot_a_ns,
            snapshot_time_b_ns: snapshot_b_ns,
            revealed_at_ns: now_ns,
        };
        // Chain the next epoch's seed off this one plus the epoch's summary, then
        // lock its commit. The reveal of THIS seed (returned to the caller) fixes
        // the next commit, so the team cannot retroactively change future times.
        let next = sha256(&[&seed, &epoch_summary_hash]);
        singleton.pending_commit = sha256(&[&next]);
        singleton.current_seed = Some(next);
        Ok(revealed)
    }
}

#[cfg(test)]
mod derive_tests {
    use super::*;

    /// One week in nanoseconds, matching `epoch::EPOCH_DURATION_NS`.
    const WK: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

    #[test]
    fn derive_is_deterministic() {
        let seed = [7u8; 32];
        let first = derive_snapshot_times(&seed, 1_000, 1_000 + WK);
        let again = derive_snapshot_times(&seed, 1_000, 1_000 + WK);
        assert_eq!(first, again);
    }

    #[test]
    fn derive_zero_seed_hits_start_and_midpoint() {
        // seed[0..8] == 0 -> a offset 0 -> a == start.
        // seed[8..16] == 0 -> b offset 0 -> b == start + half (the midpoint).
        let start = 1_000u64;
        let (a, b) = derive_snapshot_times(&[0u8; 32], start, start + WK);
        assert_eq!(a, start);
        assert_eq!(b, start + WK / 2);
    }

    #[test]
    fn derive_keeps_a_in_first_half_b_in_second() {
        let start = 500u64;
        let end = start + WK;
        let half = WK / 2;
        // An arbitrary seed with distinct low/high bytes.
        let mut seed = [0u8; 32];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(37).wrapping_add(3);
        }
        let (a, b) = derive_snapshot_times(&seed, start, end);
        assert!(start <= a && a < start + half, "a={a} must be in the first half");
        assert!(start + half <= b && b < end, "b={b} must be in the second half");
        assert!(a < b, "snapshot A must precede snapshot B");
    }

    #[test]
    fn derive_different_seeds_produce_different_times() {
        let start = 0u64;
        let zero = derive_snapshot_times(&[0u8; 32], start, start + WK);
        let mut other = [0u8; 32];
        other[0] = 0xAB; // perturb the A offset
        other[8] = 0xCD; // perturb the B offset
        let perturbed = derive_snapshot_times(&other, start, start + WK);
        assert_ne!(zero, perturbed);
    }

    #[test]
    fn derive_zero_duration_does_not_panic() {
        // half == 0 must not panic on the modulo; both snapshots collapse to start.
        let (a, b) = derive_snapshot_times(&[9u8; 32], 1_234, 1_234);
        assert_eq!((a, b), (1_234, 1_234));
    }
}

#[cfg(test)]
mod seed_manager_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Independent sha256 (computed straight from the `sha2` crate) so the tests
    /// verify the manager's hashing against a reference, not against itself.
    fn h(parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for p in parts {
            hasher.update(p);
        }
        hasher.finalize().into()
    }

    const WK: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

    /// A singleton committed to `seed` (H0 = sha256(seed)), no open seed yet.
    fn committed(seed: &[u8; 32]) -> SnapshotSeedSingleton {
        SnapshotSeedSingleton {
            pending_commit: h(&[seed]),
            current_seed: None,
        }
    }

    #[test]
    fn start_epoch_returns_derived_times_and_stashes_seed() {
        let s0 = [3u8; 32];
        let mut sing = committed(&s0);
        let times = SeedManager::start_epoch(&mut sing, 1_000, 1_000 + WK, Some(s0)).unwrap();
        assert_eq!(times, derive_snapshot_times(&s0, 1_000, 1_000 + WK));
        assert_eq!(sing.current_seed, Some(s0));
    }

    #[test]
    fn start_epoch_rejects_commit_mismatch() {
        let s0 = [3u8; 32];
        let mut sing = committed(&s0); // commit is sha256(s0)
        let wrong = [4u8; 32];
        let err = SeedManager::start_epoch(&mut sing, 0, WK, Some(wrong)).unwrap_err();
        assert_eq!(err, SeedError::CommitMismatch);
        assert_eq!(sing.current_seed, None); // unchanged on rejection
    }

    #[test]
    fn start_epoch_missing_seed_errors() {
        let mut sing = committed(&[3u8; 32]); // committed, but no current_seed and no arg
        let err = SeedManager::start_epoch(&mut sing, 0, WK, None).unwrap_err();
        assert_eq!(err, SeedError::MissingSeed);
    }

    #[test]
    fn start_epoch_uses_current_seed_when_arg_is_none() {
        // Epoch >= 1: the prior close pre-loaded current_seed and its commit.
        let seed1 = [11u8; 32];
        let mut sing = SnapshotSeedSingleton {
            pending_commit: h(&[&seed1]),
            current_seed: Some(seed1),
        };
        let times = SeedManager::start_epoch(&mut sing, 0, WK, None).unwrap();
        assert_eq!(times, derive_snapshot_times(&seed1, 0, WK));
        assert_eq!(sing.current_seed, Some(seed1));
    }

    #[test]
    fn start_epoch_uncommitted_accepts_any_seed() {
        // pending_commit all-zero => not committed => no verification gate.
        let mut sing = SnapshotSeedSingleton::default();
        assert!(!sing.is_committed());
        let any = [42u8; 32];
        assert!(SeedManager::start_epoch(&mut sing, 0, WK, Some(any)).is_ok());
        assert_eq!(sing.current_seed, Some(any));
    }

    #[test]
    fn close_epoch_reveals_seed_and_preloads_next_commit() {
        let s0 = [3u8; 32];
        let mut sing = committed(&s0);
        let (a, b) = SeedManager::start_epoch(&mut sing, 0, WK, Some(s0)).unwrap();
        let summary = [9u8; 32];

        let revealed = SeedManager::close_epoch(&mut sing, 0, a, b, 777, summary).unwrap();
        assert_eq!(revealed.epoch_index, 0);
        assert_eq!(revealed.seed, s0);
        assert_eq!(revealed.snapshot_time_a_ns, a);
        assert_eq!(revealed.snapshot_time_b_ns, b);
        assert_eq!(revealed.revealed_at_ns, 777);

        // Next epoch's seed is sha256(seed || summary); its commit is sha256(next).
        let next = h(&[&s0, &summary]);
        assert_eq!(sing.current_seed, Some(next));
        assert_eq!(sing.pending_commit, h(&[&next]));
    }

    #[test]
    fn close_without_open_seed_errors() {
        let mut sing = committed(&[3u8; 32]); // current_seed is None
        let err = SeedManager::close_epoch(&mut sing, 0, 1, 2, 3, [0u8; 32]).unwrap_err();
        assert_eq!(err, SeedError::MissingSeed);
    }

    #[test]
    fn hash_chain_holds_across_three_epochs() {
        let s0 = [5u8; 32];
        let mut sing = committed(&s0);

        // Epoch 0: provided S0.
        let (a0, b0) = SeedManager::start_epoch(&mut sing, 0, WK, Some(s0)).unwrap();
        SeedManager::close_epoch(&mut sing, 0, a0, b0, 1, [1u8; 32]).unwrap();

        // Epoch 1: derived seed, no arg; the pre-loaded commit must verify.
        let (a1, b1) = SeedManager::start_epoch(&mut sing, WK, 2 * WK, None)
            .expect("epoch 1 seed must satisfy the pending commit");
        SeedManager::close_epoch(&mut sing, 1, a1, b1, 2, [2u8; 32]).unwrap();

        // Epoch 2: same, chain still intact.
        SeedManager::start_epoch(&mut sing, 2 * WK, 3 * WK, None)
            .expect("epoch 2 seed must satisfy the pending commit");
    }
}
