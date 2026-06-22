use super::hardening::{is_stuck, is_reorg, hot_wallet_ok, bump_gas, HOT_WALLET_MIN_E18};
use super::hardening::on_not_mined_tick;
use super::hardening::{on_reorg_tick, REORG_CONFIRM_TICKS};
use super::hardening::{inflight_should_acquire, INFLIGHT_STALE_NS};

#[test]
fn detects_stuck_tx_after_threshold() {
    assert!(!is_stuck(1, 1));   // 1 try at depth 1 -> threshold is 2, not yet
    assert!(is_stuck(2, 1));    // 2 tries at depth 1 -> stuck
    assert!(is_stuck(10, 5));   // 10 >= 2*5 -> stuck
    assert!(!is_stuck(9, 5));   // 9 < 2*5
}

#[test]
fn detects_reorg_only_beyond_finality_depth() {
    assert!(is_reorg(100, 98, 1));   // regression 2 > finality_depth 1 -> reorg
    assert!(!is_reorg(100, 99, 1));  // regression 1 == finality_depth -> tolerated
    assert!(!is_reorg(100, 105, 1)); // forward progress -> never a reorg
    assert!(!is_reorg(100, 100, 1)); // no change -> not a reorg
}

#[test]
fn hot_wallet_gate_blocks_below_threshold() {
    assert!(hot_wallet_ok(HOT_WALLET_MIN_E18));
    assert!(hot_wallet_ok(HOT_WALLET_MIN_E18 + 1));
    assert!(!hot_wallet_ok(HOT_WALLET_MIN_E18 - 1));
}

#[test]
fn bump_gas_increases_fees_by_at_least_125_percent() {
    let (new_prio, new_max) = bump_gas(2_000_000_000, 50_000_000_000);
    assert!(new_prio >= 2_000_000_000 * 125 / 100);
    assert!(new_max >= 50_000_000_000 * 125 / 100);
}

#[test]
fn not_mined_tick_advances_tries_and_resubmits_at_threshold() {
    // finality_depth 1 -> stuck threshold 2. Start at tries=1 (just submitted).
    // First not-mined tick -> tries 2 -> stuck -> resubmit (have nonce).
    assert_eq!(on_not_mined_tick(1, 1, true), (2, true));
    // Without a stored nonce we must NEVER resubmit (would risk a fresh-nonce 2nd mint).
    assert_eq!(on_not_mined_tick(1, 1, false), (2, false));
    // Deeper finality: threshold 10. tries 1 -> 2, not stuck yet.
    assert_eq!(on_not_mined_tick(1, 5, true), (2, false));
    // tries 9 -> 10 == threshold -> resubmit.
    assert_eq!(on_not_mined_tick(9, 5, true), (10, true));
    // Saturates, never panics.
    assert_eq!(on_not_mined_tick(u32::MAX, 1, true), (u32::MAX, true));
}

#[test]
fn reorg_halts_only_after_consecutive_confirmations() {
    assert_eq!(REORG_CONFIRM_TICKS, 3);
    // Suspect ticks accumulate; halt only when the streak reaches K.
    assert_eq!(on_reorg_tick(0, true), (1, false));
    assert_eq!(on_reorg_tick(1, true), (2, false));
    assert_eq!(on_reorg_tick(2, true), (3, true)); // K-th consecutive -> halt
    // A non-suspect tick resets the streak (transient blip self-heals).
    assert_eq!(on_reorg_tick(2, false), (0, false));
    assert_eq!(on_reorg_tick(0, false), (0, false));
}

// ─── inflight_should_acquire (B2 hardening) ─────────────────────────────────

#[test]
fn inflight_acquire_free_entry() {
    // Chain not in the map at all (None) -> always acquire.
    assert!(inflight_should_acquire(None, 1_000_000_000_000, INFLIGHT_STALE_NS));
    assert!(inflight_should_acquire(None, 0, INFLIGHT_STALE_NS));
}

#[test]
fn inflight_acquire_fresh_entry_is_skipped() {
    // Entry acquired 10 seconds ago (well below the 10-min threshold) -> skip.
    let now_ns: u64 = 1_000_000_000_000_000_000; // 1 second in ns * 1e9 = ~31 years
    let acquired_10s_ago = now_ns - 10_000_000_000; // 10 s ago
    assert!(!inflight_should_acquire(Some(acquired_10s_ago), now_ns, INFLIGHT_STALE_NS));
}

#[test]
fn inflight_acquire_stale_entry_is_reclaimed() {
    // Entry acquired 700 s ago (> 600 s threshold) -> reclaim.
    let now_ns: u64 = 1_000_000_000_000_000_000;
    let acquired_700s_ago = now_ns - 700_000_000_000; // 700 s ago
    assert!(inflight_should_acquire(Some(acquired_700s_ago), now_ns, INFLIGHT_STALE_NS));
    // Exactly at the threshold boundary (== stale_ns) -> reclaim.
    let acquired_600s_ago = now_ns - INFLIGHT_STALE_NS;
    assert!(inflight_should_acquire(Some(acquired_600s_ago), now_ns, INFLIGHT_STALE_NS));
}

#[test]
fn inflight_acquire_future_timestamp_treated_as_fresh() {
    // A future `acquired_at` (clock skew or adversarial state) must NOT panic
    // (saturating_sub returns 0, which is < stale_ns) -> treated as fresh -> skip.
    let now_ns: u64 = 1_000_000_000_000;
    let future_timestamp = now_ns + 999_999_999_999; // well beyond now
    assert!(!inflight_should_acquire(Some(future_timestamp), now_ns, INFLIGHT_STALE_NS));
}

// ─── Increment 3 / Task 8: swap confirm-timeout (findings #12/#22) ───
#[test]
fn swap_confirm_timed_out_after_deadline_plus_finality() {
    use super::hardening::swap_confirm_timed_out;
    let t0 = 1_000_000_000u64;
    // Within deadline(180)+margin(600)=780s -> not timed out.
    assert!(!swap_confirm_timed_out(t0, t0 + 100 * 1_000_000_000, 180, 600));
    assert!(!swap_confirm_timed_out(t0, t0 + 780 * 1_000_000_000, 180, 600)); // exactly at edge: not yet (>)
    // One ns past the window -> timed out.
    assert!(swap_confirm_timed_out(t0, t0 + 780 * 1_000_000_000 + 1, 180, 600));
}
