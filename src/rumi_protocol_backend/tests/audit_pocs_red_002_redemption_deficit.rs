//! Wave-9 RED-002: redemption-side bad debt must accrue to the
//! Wave-8e deficit account.
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/raw-pass-results/redemption-peg.json`
//!     finding RED-002.
//!
//! # What the bug was
//!
//! `redeem_collateral` and `redeem_reserves` (vault-spillover branch)
//! both delegate to `event::record_redemption_on_vaults`, which calls
//! `state.redeem_on_vaults` to walk the cr-index ascending and deduct
//! collateral per vault. When a vault's collateral runs short of the
//! redeemer's claim at oracle price, the
//! `vault.collateral_amount.saturating_sub(...)` clip silently absorbs
//! the remainder. Pre-fix, the redeemer was paid from
//! `pending_redemption_transfer` for the full claim while only the
//! capped amount was deducted from vaults; the difference was
//! socialized invisibly.
//!
//! Liquidation shortfalls were already routed through
//! `State::accrue_deficit_shortfall` (Wave-8e LIQ-005). RED-002 extends
//! that machinery to redemption: the redeemer-claim minus actual
//! collateral seized at oracle price is now accrued to
//! `protocol_deficit_icusd` and emitted as `Event::DeficitAccrued` with
//! the new `DeficitSource::Redemption { redeemer }` source variant.
//!
//! # How this file tests the fix
//!
//! Unit tests drive the production codepath in two layers:
//!   * State + math: `state.redeem_on_vaults` returns the per-vault
//!     breakdown with actual (post-saturation) `collateral_seized`,
//!     and `event::compute_redemption_shortfall` derives the icUSD
//!     shortfall from that breakdown. Both are state-side helpers
//!     used inside `record_redemption_on_vaults` on a live canister.
//!   * State mutation: `state.accrue_deficit_shortfall` increments
//!     `protocol_deficit_icusd`. The composed
//!     `accrue_redemption_shortfall_at` helper additionally records
//!     a `DeficitAccrued` event with the new `Redemption` source
//!     variant; the event-recording leg is exercised by the existing
//!     PocketIC suite (`record_event` reads `ic_cdk::api::time()`
//!     which panics outside a canister, so we don't drive it here).
//!
//! Three behavioural scenarios + one event-shape fence + one structural
//! fence:
//!   * Scenario A (TDD red-green) — underwater vault: redeem against a
//!     vault whose collateral value at oracle price is far below its
//!     debt. Assert that the shortfall predicate returns the missing
//!     icUSD AND that `state.accrue_deficit_shortfall` increments
//!     `protocol_deficit_icusd` by exactly that amount.
//!   * Scenario B (regression fence) — solvent vault: redeem against a
//!     healthy vault. Assert that the shortfall predicate returns 0
//!     (no false-positive accrual).
//!   * Scenario C (TDD red-green) — `DeficitSource::Redemption` is a
//!     distinct variant carrying the redeemer principal, AND
//!     `record_redemption_on_vaults` constructs that variant rather
//!     than reusing `Liquidation`. Structural fence reads
//!     `event.rs` directly to pin the wiring.
//!   * `red_002_deficit_accrued_event_with_liquidation_source_round_trips`
//!     — pin the extended `Event::DeficitAccrued` CBOR shape (the
//!     legacy back-compat case is covered by
//!     `red_002_legacy_deficit_accrued_event_decodes_with_source_none`).

use candid::Principal;
use rumi_protocol_backend::event::{
    compute_redemption_shortfall, DeficitSource, Event, VaultRedemption,
};
use rumi_protocol_backend::numeric::{ICUSD, UsdIcp};
use rumi_protocol_backend::state::{CollateralStatus, State};
use rumi_protocol_backend::vault::Vault;
use rumi_protocol_backend::InitArg;
use std::path::PathBuf;

const DEFAULT_DECIMALS: u8 = 8;

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

fn icp_ct(state: &State) -> Principal {
    state.icp_collateral_type()
}

fn open_test_vault(state: &mut State, vault_id: u64, debt_e8s: u64, collateral_e8s: u64) {
    let ct = icp_ct(state);
    state.open_vault(Vault {
        owner: Principal::from_slice(&[1]),
        vault_id,
        collateral_amount: collateral_e8s,
        borrowed_icusd_amount: ICUSD::new(debt_e8s),
        collateral_type: ct,
        last_accrual_time: 0,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });
}

fn set_icp_price(state: &mut State, price_usd: f64) {
    let ct = icp_ct(state);
    if let Some(c) = state.collateral_configs.get_mut(&ct) {
        c.last_price = Some(price_usd);
        c.last_price_timestamp = Some(1);
        c.status = CollateralStatus::Active;
    }
}

fn redeem_then_accrue(
    state: &mut State,
    target: ICUSD,
    price_usd: f64,
    decimals: u8,
) -> (Vec<VaultRedemption>, ICUSD) {
    let ct = icp_ct(state);
    let ct_price = UsdIcp::from(rust_decimal::Decimal::from_f64_retain(price_usd).unwrap());
    let vault_redemptions = state.redeem_on_vaults(target, ct_price, &ct);
    let shortfall = compute_redemption_shortfall(
        target,
        &vault_redemptions,
        rust_decimal::Decimal::from_f64_retain(price_usd).unwrap(),
        decimals,
    );
    // Mirror the production helper's state mutation. The composed
    // `accrue_redemption_shortfall_at` additionally records a
    // `DeficitAccrued` event; we skip that here because `record_event`
    // pulls `ic_cdk::api::time()` which panics outside a canister
    // context (a pre-existing limitation that already affects the
    // LIQ-005 audit POCs that touch the event log).
    if shortfall.0 > 0 {
        state.accrue_deficit_shortfall(shortfall);
    }
    (vault_redemptions, shortfall)
}

// ─── Behavioural scenarios ───

#[test]
fn red_002_scenario_a_underwater_vault_accrues_redemption_deficit() {
    // Setup: 1 vault with 10 icUSD debt and only 1 ICP collateral at
    // $5/ICP. Collateral value = $5, debt = $10 → CR = 0.5 (severely
    // underwater). Redeeming the full 10 icUSD against this vault
    // means the redeemer's claim ($10) exceeds what vault collateral
    // can cover at oracle price ($5).
    let mut state = fresh_state();
    let debt_e8s: u64 = 1_000_000_000; // 10 icUSD
    let collateral_e8s: u64 = 100_000_000; // 1 ICP
    open_test_vault(&mut state, 1, debt_e8s, collateral_e8s);
    set_icp_price(&mut state, 5.0);

    let pre_deficit = state.protocol_deficit_icusd;

    let (breakdown, shortfall) = redeem_then_accrue(
        &mut state,
        ICUSD::new(debt_e8s),
        5.0,
        DEFAULT_DECIMALS,
    );

    // Sanity on the per-vault breakdown: collateral_seized is now the
    // *actual* amount post-saturation, not the requested amount.
    assert_eq!(breakdown.len(), 1);
    assert_eq!(breakdown[0].vault_id, 1);
    assert_eq!(breakdown[0].icusd_redeemed_e8s, debt_e8s);
    assert_eq!(
        breakdown[0].collateral_seized, collateral_e8s,
        "post-fix VaultRedemption.collateral_seized must be the \
         actual seized amount, not the requested amount"
    );

    // Predicate: redeemer claim ($10) - actual collateral seized at oracle
    //         = 10 icUSD - 1 ICP * $5 = 5 icUSD = 500_000_000 e8s.
    let expected_shortfall = ICUSD::new(500_000_000);
    assert_eq!(
        shortfall, expected_shortfall,
        "compute_redemption_shortfall must return {} e8s for an \
         underwater redemption (got {})",
        expected_shortfall.to_u64(),
        shortfall.to_u64(),
    );

    // State mutation: protocol_deficit_icusd is incremented by the shortfall.
    let new_deficit = state.protocol_deficit_icusd - pre_deficit;
    assert_eq!(
        new_deficit, expected_shortfall,
        "underwater redemption must accrue {} e8s to protocol_deficit_icusd \
         (got {})",
        expected_shortfall.to_u64(),
        new_deficit.to_u64(),
    );
}

#[test]
fn red_002_scenario_b_solvent_redemption_no_deficit() {
    // Setup: 1 solvent vault with 10 icUSD debt and 5 ICP collateral
    // at $5/ICP. Collateral value = $25, plenty to cover the full
    // redemption at oracle price. No shortfall expected.
    let mut state = fresh_state();
    let debt_e8s: u64 = 1_000_000_000; // 10 icUSD
    let collateral_e8s: u64 = 500_000_000; // 5 ICP
    open_test_vault(&mut state, 1, debt_e8s, collateral_e8s);
    set_icp_price(&mut state, 5.0);

    let pre_deficit = state.protocol_deficit_icusd;

    let (_breakdown, shortfall) = redeem_then_accrue(
        &mut state,
        ICUSD::new(debt_e8s),
        5.0,
        DEFAULT_DECIMALS,
    );

    assert_eq!(
        shortfall,
        ICUSD::new(0),
        "solvent redemption must compute zero shortfall (got {} e8s)",
        shortfall.to_u64(),
    );
    let new_deficit = state.protocol_deficit_icusd - pre_deficit;
    assert_eq!(
        new_deficit,
        ICUSD::new(0),
        "solvent redemption must not accrue any deficit (got {} e8s)",
        new_deficit.to_u64(),
    );
}

#[test]
fn red_002_scenario_c_event_carries_redemption_source_variant() {
    // Two-part fence:
    //   1. `DeficitSource::Redemption` is a distinct variant carrying
    //      the redeemer principal — exercised at the type level here.
    //   2. `record_redemption_on_vaults` (event.rs) constructs that
    //      variant on the deficit-accrual leg, NOT
    //      `DeficitSource::Liquidation`. Structural fence reads the
    //      event.rs source directly and asserts the wiring.
    //
    // Layered this way because the integrated event-emission path
    // (`record_event` → `ic_cdk::api::time()`) cannot be exercised in
    // a unit test environment.

    // Part 1: variant fence.
    let redeemer = Principal::from_slice(&[42]);
    let source = DeficitSource::Redemption { redeemer };
    match source.clone() {
        DeficitSource::Redemption { redeemer: r } => {
            assert_eq!(
                r, redeemer,
                "DeficitSource::Redemption must carry the redeemer principal so \
                 explorer can attribute deficit growth back to the user that \
                 triggered the underwater redemption"
            );
        }
        DeficitSource::Liquidation { .. } => {
            panic!("clone of Redemption produced Liquidation");
        }
    }

    // Part 2: structural wiring fence.
    let event_rs_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/event.rs");
    let event_rs = std::fs::read_to_string(&event_rs_path)
        .unwrap_or_else(|e| panic!("read {}: {}", event_rs_path.display(), e));

    // The fix wires `record_redemption_on_vaults` → the new
    // `accrue_redemption_shortfall_at` helper, which in turn calls
    // `record_deficit_accrued` with `DeficitSource::Redemption`.
    // Pin both call sites textually so refactoring this path can't
    // silently regress to passing `Liquidation` (or no source at all).
    let helper_decl_idx = event_rs
        .find("pub fn accrue_redemption_shortfall_at(")
        .expect(
            "event.rs must define `accrue_redemption_shortfall_at` — the pure \
             helper that drives the Wave-9 RED-002 redemption shortfall accrual",
        );
    let helper_body = &event_rs[helper_decl_idx..];
    assert!(
        helper_body.contains("DeficitSource::Redemption"),
        "accrue_redemption_shortfall_at must construct \
         DeficitSource::Redemption when accruing the redemption-side \
         shortfall (audit RED-002). Reusing Liquidation would mis-attribute \
         deficit growth in the explorer."
    );

    let entry_decl_idx = event_rs
        .find("pub fn record_redemption_on_vaults(")
        .expect("event.rs must define `record_redemption_on_vaults`");
    let entry_to_helper_slice = &event_rs[entry_decl_idx..];
    assert!(
        entry_to_helper_slice.contains("accrue_redemption_shortfall_at"),
        "record_redemption_on_vaults must invoke \
         accrue_redemption_shortfall_at so every redemption that walks the \
         vault cr-index has its shortfall (if any) routed into the deficit \
         account (audit RED-002)."
    );
}

// ─── Event-shape round-trip fences ───
//
// The standalone `DeficitSource::Redemption` variant carries a
// `candid::Principal`, whose serde Deserialize impl asserts a Candid
// context and panics under ciborium ("Not called by Candid"). We
// therefore round-trip the LIQUIDATION variant only — that's enough
// to pin the enum's CBOR tagging and, more importantly, the
// extended `Event::DeficitAccrued` shape carrying the new `source`
// field. The Redemption variant's wire format is exercised
// end-to-end by the PocketIC redemption suite.

#[test]
fn red_002_deficit_accrued_event_with_liquidation_source_round_trips() {
    let e = Event::DeficitAccrued {
        vault_id: 7,
        amount: ICUSD::new(500_000_000),
        new_deficit: ICUSD::new(500_000_000),
        timestamp: 1_700_000_000_000_000_000,
        source: Some(DeficitSource::Liquidation { vault_id: 7 }),
    };
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&e, &mut bytes).expect("encode");
    let decoded: Event =
        ciborium::de::from_reader(bytes.as_slice()).expect("decode");
    assert_eq!(decoded, e);
}

#[test]
fn red_002_legacy_deficit_accrued_event_decodes_with_source_none() {
    // Pre-Wave-9 events were serialized without the `source` field. We
    // simulate that by encoding a current event with source = None
    // (`skip_serializing_if = "Option::is_none"` drops the field on
    // the wire) and asserting it decodes with `source = None` again.
    // Round-tripping `None` is exactly the back-compat case: pre-Wave-9
    // canister event-log entries lack the field on disk, and serde's
    // `default` annotation backfills `None` on read.
    let e = Event::DeficitAccrued {
        vault_id: 1,
        amount: ICUSD::new(100),
        new_deficit: ICUSD::new(100),
        timestamp: 1_700_000_000_000_000_000,
        source: None,
    };
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&e, &mut bytes).expect("encode");
    let decoded: Event = ciborium::de::from_reader(bytes.as_slice()).expect(
        "legacy DeficitAccrued (no source) must decode under serde(default)",
    );

    match decoded {
        Event::DeficitAccrued { source, vault_id, .. } => {
            assert!(
                source.is_none(),
                "legacy DeficitAccrued must decode with source = None; got {:?}",
                source,
            );
            assert_eq!(vault_id, 1);
        }
        other => panic!("decoded into wrong variant: {:?}", other),
    }
}
