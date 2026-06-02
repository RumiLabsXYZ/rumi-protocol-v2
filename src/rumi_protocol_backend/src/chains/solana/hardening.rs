//! Operational hardening predicates for the Solana adapter. Pure + unit-tested.
//!
//! Mirrors the re-entrancy self-heal helper from `chains::monad::hardening`. The
//! EVM-specific pieces of the Monad module (`is_stuck` / `on_not_mined_tick` /
//! `bump_gas` replace-by-fee, and the `is_reorg` / `on_reorg_tick` reorg
//! debounce) are deliberately NOT mirrored here:
//!
//! - Solana has no nonce-bump replace-by-fee. Its stuck-tx recovery is durable
//!   nonces (a deferred Task-8 settlement seam), not gas bidding.
//! - Solana reads at the `finalized` commitment, which does not reorg, so the
//!   finalized-block-regression circuit breaker has nothing to detect.
//!
//! The per-chain inflight guard, by contrast, is chain-agnostic (it guards the
//! async observer/settlement future itself, not any chain mechanic), so it is
//! mirrored verbatim.

/// Minimum settlement (mint-authority) address SOL balance (lamports) to allow
/// new outbound ops. Below this, refuse new ops on the chain (reads still work).
/// 0.05 SOL covers a healthy margin of Solana base fees + priority fees + the
/// rent-exempt minimum a fresh ATA-create touches. The hot-wallet gas gate is
/// not wired on the observer path in this task; it is provided here so the
/// Task-8 settlement worker can gate submits without a second const bump.
pub const SOLANA_HOT_WALLET_MIN_LAMPORTS: u64 = 50_000_000; // 0.05 SOL

/// Gas gate: settlement address has at least the minimum SOL (lamports).
pub fn hot_wallet_ok(balance_lamports: u64) -> bool {
    balance_lamports >= SOLANA_HOT_WALLET_MIN_LAMPORTS
}

// ─── Inflight guard self-heal (mirrors monad::hardening) ─────────────────────
//
// On the IC, a trap in a post-await continuation does NOT run `Drop`, so a
// per-chain inflight-guard entry can stick forever if the holder trapped. To
// self-heal, each guard stores the timestamp it was acquired at. A later tick
// whose `inflight_should_acquire` check finds the entry older than
// `INFLIGHT_STALE_NS` reclaims it (the previous holder must have trapped; a
// live slow tick's idempotency makes an accidental concurrent observer
// supply-safe anyway).

/// Stale threshold for per-chain inflight guard entries (in nanoseconds).
///
/// A healthy observer/settlement tick completes in seconds; the timers fire
/// every ~5 min. If an in-flight entry is older than this, the previous holder
/// must have trapped in a post-await continuation (`Drop` never ran), so a
/// later tick reclaims it. Set well above the worst-case legit tick (several
/// sequential RPC outcalls) to avoid reclaiming a merely-slow tick. 10 min
/// mirrors the existing stale-operation threshold convention (and Monad's
/// identical const).
pub const INFLIGHT_STALE_NS: u64 = 600_000_000_000; // 10 minutes

/// Decide whether a new tick should acquire the inflight guard for a chain.
///
/// - `existing`: the timestamp stored in the guard map, or `None` if the
///   chain is not currently held.
/// - `now_ns`: `ic_cdk::api::time()` at the start of this tick.
/// - `stale_ns`: the stale threshold (normally `INFLIGHT_STALE_NS`).
///
/// Returns `true` (acquire) when:
///   - The chain is free (`None`), OR
///   - The existing entry is stale (`now_ns - acquired_at >= stale_ns`).
///
/// Returns `false` (skip) when a fresh tick holds the guard.
/// `saturating_sub` prevents a panic if `acquired_at > now_ns` (clock skew or
/// future timestamp in state); the result is 0, which is always < `stale_ns`,
/// so a spurious future timestamp is treated as "fresh" (safe: skip the tick).
pub fn inflight_should_acquire(existing: Option<u64>, now_ns: u64, stale_ns: u64) -> bool {
    match existing {
        None => true,
        Some(acquired_at) => now_ns.saturating_sub(acquired_at) >= stale_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflight_should_acquire_when_free() {
        // No entry => the chain is free => acquire.
        assert!(inflight_should_acquire(None, 1_000, INFLIGHT_STALE_NS));
    }

    #[test]
    fn inflight_should_not_acquire_when_fresh() {
        // A fresh entry (acquired just now) => a live tick holds it => skip.
        let now = 1_000_000_000_000u64;
        let acquired = now - 1; // 1 ns ago, far below the 10-min threshold
        assert!(!inflight_should_acquire(Some(acquired), now, INFLIGHT_STALE_NS));
    }

    #[test]
    fn inflight_should_acquire_when_exactly_stale() {
        // Exactly at the threshold (>= is the boundary) => reclaim.
        let acquired = 5_000_000_000_000u64;
        let now = acquired + INFLIGHT_STALE_NS;
        assert!(inflight_should_acquire(Some(acquired), now, INFLIGHT_STALE_NS));
    }

    #[test]
    fn inflight_should_acquire_when_past_stale() {
        // Well past the threshold (the trapped-holder case) => reclaim.
        let acquired = 5_000_000_000_000u64;
        let now = acquired + INFLIGHT_STALE_NS + 1;
        assert!(inflight_should_acquire(Some(acquired), now, INFLIGHT_STALE_NS));
    }

    #[test]
    fn inflight_future_timestamp_treated_as_fresh() {
        // acquired_at > now_ns (clock skew / future stamp): saturating_sub => 0,
        // which is < stale_ns, so DON'T reclaim (treat as fresh, skip the tick).
        let now = 1_000u64;
        let acquired = now + 10_000; // in the future
        assert!(!inflight_should_acquire(Some(acquired), now, INFLIGHT_STALE_NS));
    }

    #[test]
    fn hot_wallet_ok_gates_at_min() {
        assert!(!hot_wallet_ok(SOLANA_HOT_WALLET_MIN_LAMPORTS - 1));
        assert!(hot_wallet_ok(SOLANA_HOT_WALLET_MIN_LAMPORTS));
        assert!(hot_wallet_ok(SOLANA_HOT_WALLET_MIN_LAMPORTS + 1));
        assert!(!hot_wallet_ok(0));
    }
}
