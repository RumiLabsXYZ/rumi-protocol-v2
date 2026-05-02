//! CDP-10 regression fence: a transport `Err` from the stability_pool
//! `notify_liquidatable_vaults` call must NOT permanently blacklist the
//! affected vault from future SP attempts.
//!
//! Pre-fix, `check_vaults` synchronously inserted the vault id into
//! `sp_attempted_vaults` BEFORE spawning the inter-canister call. If the
//! spawn returned `Err` (cycle pressure, queue-full during a market
//! crash), the vault was permanently blocked from the SP and only owner
//! action could clear it. Audit fence per
//! `.claude/security-docs/2026-05-02-wave-14-avai-parity-plan.md`.
//!
//! Layered fences:
//!  1. Helper records the `Ok` result by inserting all dispatched vault
//!     ids into `sp_attempted_vaults`.
//!  2. Helper records a transport `Err` by leaving `sp_attempted_vaults`
//!     unchanged and emitting `Event::StabilityPoolCallFailed`.
//!  3. The retain loop still cleans entries for vaults that have become
//!     healthy.

use candid::Principal;

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::record_sp_notification_result_at;
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::InitArg;

const TEST_NOW_NS: u64 = 1_700_000_000_000_000_000;

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
fn cdp_10_ok_inserts_all_vault_ids() {
    let mut state = fresh_state();
    assert!(state.sp_attempted_vaults.is_empty());

    let dispatched = vec![7u64, 8u64, 9u64];
    let event = record_sp_notification_result_at(&mut state, dispatched.clone(), Ok(()), TEST_NOW_NS);

    for vid in &dispatched {
        assert!(
            state.sp_attempted_vaults.contains(vid),
            "vault {vid} must be marked SP-attempted on Ok",
        );
    }
    assert!(
        event.is_none(),
        "no event should be emitted on Ok; got {:?}",
        event
    );
}

#[test]
fn cdp_10_transport_err_does_not_blacklist() {
    let mut state = fresh_state();

    let dispatched = vec![42u64, 43u64];
    let err: Result<(), (i32, String)> = Err((500, "queue full".to_string()));

    let event = record_sp_notification_result_at(&mut state, dispatched.clone(), err, TEST_NOW_NS);

    for vid in &dispatched {
        assert!(
            !state.sp_attempted_vaults.contains(vid),
            "vault {vid} must NOT be SP-attempted on Err (this is the regression we are guarding)",
        );
    }

    let Some(Event::StabilityPoolCallFailed {
        vault_ids,
        reject_code,
        reject_message,
        timestamp: _,
    }) = event
    else {
        panic!("expected Event::StabilityPoolCallFailed on Err, got {:?}", event);
    };
    assert_eq!(vault_ids, dispatched);
    assert_eq!(reject_code, 500);
    assert_eq!(reject_message, "queue full");
}

#[test]
fn cdp_10_err_then_retry_can_succeed() {
    let mut state = fresh_state();

    // First attempt: SP transport fails. Vault NOT blacklisted.
    let _ = record_sp_notification_result_at(
        &mut state,
        vec![100u64],
        Err((500, "queue full".to_string())),
        TEST_NOW_NS,
    );
    assert!(!state.sp_attempted_vaults.contains(&100u64));

    // Next tick: SP recovers. Vault is now eligible again. Helper inserts.
    let _ = record_sp_notification_result_at(&mut state, vec![100u64], Ok(()), TEST_NOW_NS);
    assert!(state.sp_attempted_vaults.contains(&100u64));
}

#[test]
fn cdp_10_empty_dispatch_is_noop() {
    let mut state = fresh_state();
    let event = record_sp_notification_result_at(&mut state, vec![], Ok(()), TEST_NOW_NS);
    assert!(state.sp_attempted_vaults.is_empty());
    assert!(event.is_none());
}
