//! Operational hardening predicates (spec Section 3). Pure + unit-tested.

/// Minimum settlement-address MON balance (e18) to allow new outbound ops.
/// Below this, refuse new ops on the chain (reads still work). 0.1 MON.
pub const HOT_WALLET_MIN_E18: u128 = 100_000_000_000_000_000; // 0.1 MON

/// An inflight op is stuck once tries >= finality_depth * 2 (min 2). EVM
/// replace-by-fee: bump gas and resubmit on the SAME nonce.
pub fn is_stuck(tries: u32, finality_depth: u32) -> bool {
    tries as u64 >= (finality_depth as u64).saturating_mul(2).max(2)
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
