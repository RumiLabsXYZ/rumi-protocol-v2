//! Operational hardening predicates (spec Section 3). Pure + unit-tested.

/// Minimum settlement-address MON balance (e18) to allow new outbound ops.
/// Below this, refuse new ops on the chain (reads still work). 0.1 MON.
pub const HOT_WALLET_MIN_E18: u128 = 100_000_000_000_000_000; // 0.1 MON

/// An inflight op is stuck once tries >= finality_depth * 2 (min 2). EVM
/// replace-by-fee: bump gas and resubmit on the SAME nonce.
pub fn is_stuck(tries: u32, finality_depth: u32) -> bool {
    tries as u64 >= (finality_depth as u64).saturating_mul(2).max(2)
}

/// Decide what a NOT-MINED confirm tick does to an inflight op. The first
/// element is the new `tries` value (always advanced by 1, saturating, so the
/// stuck threshold can actually be crossed — the prior code never advanced
/// `tries` on a plain not-mined tick, so replace-by-fee never fired). The second
/// is whether to replace-by-fee THIS tick: only when the advanced count is stuck
/// AND we recorded the original submit nonce (`has_submit_nonce`) — resubmitting
/// without the stored nonce would risk a fresh-nonce second mint.
pub fn on_not_mined_tick(tries: u32, finality_depth: u32, has_submit_nonce: bool) -> (u32, bool) {
    let new_tries = tries.saturating_add(1);
    let resubmit = has_submit_nonce && is_stuck(new_tries, finality_depth);
    (new_tries, resubmit)
}

/// Default extra margin (secs) added to a swap's on-chain `deadline_secs` before
/// the IC gives up on a never-mined LiquidationSwap. MUST exceed chain finality so
/// a mined-then-reverted swap (deadline expiry) is observed on-chain BEFORE the IC
/// timeout fires — else the SP could absorb debt the swap later settles (spec §4.8
/// double-spend). 10 minutes is comfortably > eSpace finality.
pub const SWAP_CONFIRM_FINALITY_MARGIN_SECS: u64 = 600;

/// Net-new confirm-timeout for the LiquidationSwap kind (findings #12/#22): a
/// never-mined swap (dropped from the mempool, no receipt ever) must transition
/// to `Failed` so it cannot wedge the vault marker + reserved collateral forever
/// (swaps are EXCLUDED from replace-by-fee, so without this they sit Inflight
/// indefinitely). Returns true once `now - inflight_since > deadline + margin`.
pub fn swap_confirm_timed_out(
    inflight_since_ns: u64,
    now_ns: u64,
    deadline_secs: u64,
    finality_margin_secs: u64,
) -> bool {
    let timeout_ns = deadline_secs
        .saturating_add(finality_margin_secs)
        .saturating_mul(1_000_000_000);
    now_ns.saturating_sub(inflight_since_ns) > timeout_ns
}

/// Consecutive observer ticks a finalized-block regression must persist before
/// it is treated as a real reorg (vs a transient single-provider RPC lag).
/// fetch_block_numbers is un-quorumed (one provider at a time), so a single
/// stale read must NOT permanently halt the chain.
pub const REORG_CONFIRM_TICKS: u32 = 3;

/// Decide the reorg-suspicion streak transition for one observer tick.
/// `suspected` is the `is_reorg(..)` result this tick. Returns
/// `(new_streak, should_halt)`: a suspect tick advances the streak and halts
/// once it reaches `REORG_CONFIRM_TICKS`; a non-suspect tick resets to 0.
pub fn on_reorg_tick(streak: u32, suspected: bool) -> (u32, bool) {
    if suspected {
        let s = streak.saturating_add(1);
        (s, s >= REORG_CONFIRM_TICKS)
    } else {
        (0, false)
    }
}

/// A reorg deeper than finality: the newly-observed finalized block is LOWER
/// than the previously-observed one by MORE than finality_depth.
pub fn is_reorg(prev_observed: u64, now_observed: u64, finality_depth: u32) -> bool {
    now_observed < prev_observed && (prev_observed - now_observed) > finality_depth as u64
}

/// Gas gate: settlement address has at least the minimum MON.
pub fn hot_wallet_ok(balance_e18: u128) -> bool {
    balance_e18 >= HOT_WALLET_MIN_E18
}

/// Bump EIP-1559 fees by 25% (EVM RBF floor is +10%; 25% is a safe margin).
pub fn bump_gas(prio: u128, max_fee: u128) -> (u128, u128) {
    (prio.saturating_mul(125) / 100, max_fee.saturating_mul(125) / 100)
}

// ─── Inflight guard self-heal ────────────────────────────────────────────────
//
// On the IC, a trap in a post-await continuation does NOT run `Drop`, so a
// per-chain inflight-guard entry can stick forever if the holder trapped.  To
// self-heal, each guard stores the timestamp it was acquired at.  A later tick
// whose `inflight_should_acquire` check finds the entry older than
// `INFLIGHT_STALE_NS` reclaims it (the previous holder must have trapped; a
// live slow tick's dedup/idempotency makes an accidental concurrent observer
// supply-safe anyway).

/// Stale threshold for per-chain inflight guard entries (in nanoseconds).
///
/// A healthy observer/settlement tick completes in seconds; the timers fire
/// every ~30 s. If an in-flight entry is older than this, the previous holder
/// must have trapped in a post-await continuation (`Drop` never ran), so a
/// later tick reclaims it. Set well above the worst-case legit tick (several
/// sequential RPC outcalls) to avoid reclaiming a merely-slow tick (which
/// dedup/idempotency would make safe anyway). 10 min mirrors the existing
/// stale-operation threshold convention.
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
