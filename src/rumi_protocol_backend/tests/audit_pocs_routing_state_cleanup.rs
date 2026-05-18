//! Routing-state cleanup fence (2026-05-18).
//!
//! # The bug
//!
//! Pre-2026-05-18, `check_vaults` ran the `sp_attempted_vaults.retain(...)`
//! and `bot_pending_vaults.retain(...)` cleanup loops INSIDE the
//! `if !unhealthy_vaults.is_empty()` branch. On any tick where no vault was
//! currently liquidatable, the cleanup was skipped entirely. The
//! `unhealthy_ids` set, the only thing those `retain` predicates compare
//! against, was also only built inside that branch.
//!
//! Consequence: a vault that was SP-attempted during one unhealthy episode
//! and then recovered above its liquidation floor stayed in
//! `sp_attempted_vaults` until *another* vault happened to become
//! liquidatable simultaneously. On a small protocol with usually 0-1
//! liquidatable vaults at a time, that simultaneity is rare, so stale
//! blacklist entries could persist indefinitely. When the recovered vault
//! later dropped below its floor again, `check_vaults` line 940 said
//! "SP already had its shot" and routed it to manual-only, denying it the
//! bot/pool retry that the design always intended.
//!
//! The 2026-05-17 incident exposed this in production: vaults #149, #179,
//! #182, #183 were SP-attempted, the band gate rejected them (`NotLowestCR`),
//! the SP transport call still returned `Ok(())` so they got blacklisted,
//! and after price recovery they stayed blacklisted because no other vault
//! was simultaneously underwater.
//!
//! # The fix
//!
//! Extract the cleanup into a pure function
//! (`prune_recovered_routing_state`) and call it from `check_vaults`
//! UNCONDITIONALLY on every tick, including ticks where no vault is
//! currently liquidatable. On a quiet tick, `unhealthy_ids` is empty, so
//! both `retain` predicates evaluate false for every entry, and both
//! collections are flushed. This is correct: if no vault is liquidatable,
//! no vault needs a routing-state entry.
//!
//! # What this fence pins
//!
//!  1. The pure helper exists with the expected signature.
//!  2. Empty `unhealthy_ids` flushes both collections in full.
//!  3. Non-empty `unhealthy_ids` retains only matching entries.
//!  4. `bot_pending_vaults` entries past `bot_timeout_ns` are dropped even
//!     when still unhealthy (handoff to SP).
//!  5. Reproduces the 5/17 incident at helper level: four vaults blacklisted,
//!     then a quiet tick flushes them.
//!  6. Idempotent: running the cleanup twice with the same args is the same
//!     as running it once.

use std::collections::{BTreeMap, BTreeSet};

use candid::Principal;

use rumi_protocol_backend::prune_recovered_routing_state;
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;

const TEST_NOW_NS: u64 = 1_700_000_000_000_000_000;
const BOT_TIMEOUT_NS: u64 = 300_000_000_000; // 5 min, matches production

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::from_slice(&[10]),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

#[test]
fn prune_empty_unhealthy_flushes_sp_attempted() {
    // The 5/17 regression scenario at helper level: blacklist has stale
    // entries from a prior episode, no vault is currently liquidatable, the
    // cleanup must flush.
    let mut state = fresh_state();
    state.sp_attempted_vaults.insert(149);
    state.sp_attempted_vaults.insert(179);
    state.sp_attempted_vaults.insert(182);
    state.sp_attempted_vaults.insert(183);

    let unhealthy: BTreeSet<u64> = BTreeSet::new();
    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);

    assert!(
        state.sp_attempted_vaults.is_empty(),
        "quiet tick must flush sp_attempted_vaults; got {:?}",
        state.sp_attempted_vaults,
    );
}

#[test]
fn prune_empty_unhealthy_flushes_bot_pending() {
    let mut state = fresh_state();
    state.bot_pending_vaults.insert(7, TEST_NOW_NS);
    state.bot_pending_vaults.insert(8, TEST_NOW_NS);

    let unhealthy: BTreeSet<u64> = BTreeSet::new();
    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);

    assert!(
        state.bot_pending_vaults.is_empty(),
        "quiet tick must flush bot_pending_vaults; got {:?}",
        state.bot_pending_vaults,
    );
}

#[test]
fn prune_partial_unhealthy_keeps_only_matching_sp_attempted() {
    let mut state = fresh_state();
    state.sp_attempted_vaults.insert(3);
    state.sp_attempted_vaults.insert(5);
    state.sp_attempted_vaults.insert(7);

    let mut unhealthy: BTreeSet<u64> = BTreeSet::new();
    unhealthy.insert(5);

    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);

    assert_eq!(
        state.sp_attempted_vaults,
        [5u64].into_iter().collect::<BTreeSet<u64>>(),
        "only vault 5 (still unhealthy) must remain blacklisted",
    );
}

#[test]
fn prune_partial_unhealthy_keeps_only_matching_bot_pending() {
    let mut state = fresh_state();
    state.bot_pending_vaults.insert(3, TEST_NOW_NS);
    state.bot_pending_vaults.insert(5, TEST_NOW_NS);
    state.bot_pending_vaults.insert(7, TEST_NOW_NS);

    let mut unhealthy: BTreeSet<u64> = BTreeSet::new();
    unhealthy.insert(5);

    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);

    let expected: BTreeMap<u64, u64> = [(5u64, TEST_NOW_NS)].into_iter().collect();
    assert_eq!(state.bot_pending_vaults, expected);
}

#[test]
fn prune_bot_pending_drops_timed_out_even_when_still_unhealthy() {
    // bot_pending_vaults serves two purposes: (1) track active bot
    // notifications, (2) hand off to SP after `bot_timeout_ns`. The cleanup
    // must drop entries whose age >= bot_timeout_ns even if the vault is
    // still unhealthy — the routing logic relies on absence-from-map to
    // decide "this one falls to the pool now."
    let mut state = fresh_state();
    let stale_ts = TEST_NOW_NS.saturating_sub(BOT_TIMEOUT_NS + 1);
    let fresh_ts = TEST_NOW_NS.saturating_sub(BOT_TIMEOUT_NS / 2);
    state.bot_pending_vaults.insert(100, stale_ts);
    state.bot_pending_vaults.insert(101, fresh_ts);

    let unhealthy: BTreeSet<u64> = [100u64, 101u64].into_iter().collect();

    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);

    assert!(
        !state.bot_pending_vaults.contains_key(&100),
        "vault 100 (timed out, still unhealthy) must be dropped so SP fallback can fire",
    );
    assert!(
        state.bot_pending_vaults.contains_key(&101),
        "vault 101 (within window, still unhealthy) must be kept",
    );
}

#[test]
fn prune_is_idempotent() {
    let mut state = fresh_state();
    state.sp_attempted_vaults.insert(3);
    state.sp_attempted_vaults.insert(5);
    state.bot_pending_vaults.insert(3, TEST_NOW_NS);

    let unhealthy: BTreeSet<u64> = [5u64].into_iter().collect();

    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);
    let after_first = (
        state.sp_attempted_vaults.clone(),
        state.bot_pending_vaults.clone(),
    );

    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);
    let after_second = (
        state.sp_attempted_vaults.clone(),
        state.bot_pending_vaults.clone(),
    );

    assert_eq!(after_first, after_second, "prune must be idempotent");
}

#[test]
fn prune_with_no_state_to_clean_is_a_noop() {
    let mut state = fresh_state();
    let unhealthy: BTreeSet<u64> = BTreeSet::new();
    prune_recovered_routing_state(&mut state, &unhealthy, TEST_NOW_NS, BOT_TIMEOUT_NS);
    assert!(state.sp_attempted_vaults.is_empty());
    assert!(state.bot_pending_vaults.is_empty());
}
