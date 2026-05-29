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
