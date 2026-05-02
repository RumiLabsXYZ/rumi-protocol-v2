//! CDP-03 regression fence: a redemption against one collateral type must
//! not corrupt the redemption-fee state (`current_base_rate`,
//! `last_redemption_time`) used to price redemptions against any other
//! collateral.
//!
//! Pre-fix the redemption code wrote to the GLOBAL `s.current_base_rate`
//! and `s.last_redemption_time` even though per-collateral fields exist
//! at `s.collateral_configs.get(&ct).current_base_rate /
//! .last_redemption_time`. Result: a redemption against ckBTC moved the
//! global base rate that subsequently priced ICP redemptions, leaking
//! value across collateral types.
//!
//! Audit fence per `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. The per-collateral helper writes ONLY the targeted collateral's
//!     fields.
//!  2. It does NOT touch the global `s.current_base_rate` /
//!     `s.last_redemption_time` (they remain available as aggregate
//!     display values but are no longer mutated by the redemption path).
//!  3. Two redemptions against different collaterals each update only
//!     their own per-collateral state.

use candid::Principal;

use rumi_protocol_backend::numeric::Ratio;
use rumi_protocol_backend::record_per_collateral_redemption_fee;
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

const TEST_NOW_NS: u64 = 1_700_000_000_000_000_000;

fn fresh_state_with_two_collaterals() -> (State, Principal, Principal) {
    let icp = Principal::from_slice(&[10]);
    let other = Principal::from_text("aaaaa-aa").unwrap();
    let mut state = State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: icp,
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    let mut config = state
        .collateral_configs
        .get(&icp)
        .expect("ICP config must exist")
        .clone();
    config.ledger_canister_id = other;
    state.collateral_configs.insert(other, config);
    (state, icp, other)
}

#[test]
fn cdp_03_helper_updates_targeted_collateral_only() {
    let (mut state, icp, other) = fresh_state_with_two_collaterals();

    // Initial per-collateral values are zeros from From<InitArg>.
    assert_eq!(
        state.collateral_configs[&icp].current_base_rate,
        Ratio::from(Decimal::ZERO)
    );
    assert_eq!(state.collateral_configs[&icp].last_redemption_time, 0);
    assert_eq!(
        state.collateral_configs[&other].current_base_rate,
        Ratio::from(Decimal::ZERO)
    );
    assert_eq!(state.collateral_configs[&other].last_redemption_time, 0);

    // Apply the per-collateral fee record for ICP.
    let new_rate = Ratio::from(dec!(0.05));
    record_per_collateral_redemption_fee(&mut state, &icp, new_rate, TEST_NOW_NS);

    // ICP's per-collateral fields advanced.
    assert_eq!(state.collateral_configs[&icp].current_base_rate, new_rate);
    assert_eq!(
        state.collateral_configs[&icp].last_redemption_time,
        TEST_NOW_NS
    );

    // The OTHER collateral's per-collateral fields are unchanged.
    assert_eq!(
        state.collateral_configs[&other].current_base_rate,
        Ratio::from(Decimal::ZERO),
        "redeeming against ICP must NOT touch the other collateral's base rate"
    );
    assert_eq!(
        state.collateral_configs[&other].last_redemption_time, 0,
        "redeeming against ICP must NOT touch the other collateral's last_redemption_time"
    );
}

#[test]
fn cdp_03_helper_does_not_mutate_global_fields() {
    let (mut state, icp, _other) = fresh_state_with_two_collaterals();

    // Capture the legacy global fields before the call.
    let global_rate_before = state.current_base_rate;
    let global_time_before = state.last_redemption_time;

    record_per_collateral_redemption_fee(&mut state, &icp, Ratio::from(dec!(0.10)), TEST_NOW_NS);

    // Globals must NOT have been touched. They remain available as legacy
    // aggregate fields but are no longer mutated by the redemption path.
    assert_eq!(
        state.current_base_rate, global_rate_before,
        "legacy global current_base_rate must be untouched by per-collateral redemption fee"
    );
    assert_eq!(
        state.last_redemption_time, global_time_before,
        "legacy global last_redemption_time must be untouched by per-collateral redemption fee"
    );
}

#[test]
fn cdp_03_two_redemptions_against_different_collaterals_are_isolated() {
    let (mut state, icp, other) = fresh_state_with_two_collaterals();

    record_per_collateral_redemption_fee(&mut state, &icp, Ratio::from(dec!(0.05)), TEST_NOW_NS);
    record_per_collateral_redemption_fee(
        &mut state,
        &other,
        Ratio::from(dec!(0.10)),
        TEST_NOW_NS + 60_000_000_000,
    );

    // Each collateral has its own values; neither corrupted the other.
    assert_eq!(
        state.collateral_configs[&icp].current_base_rate,
        Ratio::from(dec!(0.05))
    );
    assert_eq!(
        state.collateral_configs[&icp].last_redemption_time,
        TEST_NOW_NS
    );
    assert_eq!(
        state.collateral_configs[&other].current_base_rate,
        Ratio::from(dec!(0.10))
    );
    assert_eq!(
        state.collateral_configs[&other].last_redemption_time,
        TEST_NOW_NS + 60_000_000_000
    );
}

#[test]
fn cdp_03_unknown_collateral_is_a_noop() {
    let (mut state, _icp, _other) = fresh_state_with_two_collaterals();
    let global_rate_before = state.current_base_rate;

    let unknown = Principal::from_text("2vxsx-fae").unwrap();
    record_per_collateral_redemption_fee(
        &mut state,
        &unknown,
        Ratio::from(dec!(0.99)),
        TEST_NOW_NS,
    );

    // Helper must silently no-op for unknown collateral types: the
    // surrounding redemption path already validated the type via
    // `get_collateral_price_decimal`, but defense in depth is cheap.
    assert!(state.collateral_configs.get(&unknown).is_none());
    assert_eq!(state.current_base_rate, global_rate_before);
}
