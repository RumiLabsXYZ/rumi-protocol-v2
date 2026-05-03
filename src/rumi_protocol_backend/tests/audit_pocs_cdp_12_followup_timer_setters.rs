//! Wave-14b CDP-12 follow-up: developer-gated setters that let ops tune
//! the three timer cadences in place without a canister upgrade.
//!
//! State carries the cadence values (seconds, u64). Pre-Wave-14b
//! snapshots that don't have these fields hydrate to the production
//! defaults (300, 60, 300) via `#[serde(default = ...)]`.

use candid::Principal;

use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;

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
fn cdp_12_default_intervals_match_consts() {
    let s = fresh_state();
    assert_eq!(s.xrc_fetch_interval_secs, 300, "Timer A default mirrors xrc::FETCHING_ICP_RATE_INTERVAL");
    assert_eq!(s.interest_treasury_tick_interval_secs, 60, "Timer B default mirrors xrc::INTEREST_AND_TREASURY_TICK_INTERVAL");
    assert_eq!(s.vault_check_tick_interval_secs, 300, "Timer C default mirrors xrc::VAULT_CHECK_TICK_INTERVAL");
}

#[test]
fn cdp_12_intervals_are_independently_settable() {
    let mut s = fresh_state();

    s.xrc_fetch_interval_secs = 120;
    s.interest_treasury_tick_interval_secs = 30;
    s.vault_check_tick_interval_secs = 600;

    assert_eq!(s.xrc_fetch_interval_secs, 120);
    assert_eq!(s.interest_treasury_tick_interval_secs, 30);
    assert_eq!(s.vault_check_tick_interval_secs, 600);
}

#[test]
fn cdp_12_legacy_snapshot_decodes_to_defaults() {
    // Simulate a pre-Wave-14b snapshot decode by serializing a State
    // without these fields (using serde_json's default-on-missing behavior
    // through the State struct's #[serde(default = ...)] attributes).
    let s = fresh_state();
    let json = serde_json::to_string(&s).expect("State serializes");
    // Strip the timer fields to simulate a legacy blob, then re-decode.
    // Easier: round-trip through a value that doesn't include them.
    let mut value: serde_json::Value =
        serde_json::from_str(&json).expect("State JSON parses");
    if let Some(obj) = value.as_object_mut() {
        obj.remove("xrc_fetch_interval_secs");
        obj.remove("interest_treasury_tick_interval_secs");
        obj.remove("vault_check_tick_interval_secs");
    }
    let stripped = serde_json::to_string(&value).expect("stripped JSON");
    let decoded: State = serde_json::from_str(&stripped).expect("legacy decode");

    assert_eq!(decoded.xrc_fetch_interval_secs, 300);
    assert_eq!(decoded.interest_treasury_tick_interval_secs, 60);
    assert_eq!(decoded.vault_check_tick_interval_secs, 300);
}
