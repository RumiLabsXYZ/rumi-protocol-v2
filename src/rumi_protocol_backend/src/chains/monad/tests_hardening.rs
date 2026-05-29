use super::hardening::{is_stuck, is_reorg, hot_wallet_ok, bump_gas, HOT_WALLET_MIN_E18};

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
