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
//!  1. Helper records only vault ids with returned SP attempt results. An
//!     empty or truncated success reply leaves omitted vaults retryable.
//!  2. Helper records a transport `Err` by leaving `sp_attempted_vaults`
//!     unchanged and emitting `Event::StabilityPoolCallFailed`.
//!  3. The retain loop still cleans entries for vaults that have become
//!     healthy.

use candid::Principal;

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::record_sp_notification_result_at;
use rumi_protocol_backend::state::State;
use rumi_protocol_backend::{InitArg, SpNotificationResult};

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

fn attempt(vault_id: u64, success: bool) -> SpNotificationResult {
    SpNotificationResult { vault_id, success }
}

#[test]
fn cdp_10_ok_marks_only_returned_attempts() {
    let mut state = fresh_state();
    assert!(state.sp_attempted_vaults.is_empty());

    let dispatched = vec![7u64, 8u64, 9u64];
    let event = record_sp_notification_result_at(
        &mut state,
        dispatched,
        Ok(vec![attempt(7, true), attempt(9, false), attempt(99, true)]),
        TEST_NOW_NS,
    );

    assert_eq!(state.sp_attempted_vaults, [7, 9].into_iter().collect());
    assert!(
        event.is_none(),
        "no event should be emitted on Ok; got {:?}",
        event
    );
}

#[test]
fn cdp_10_empty_pool_reply_keeps_dispatched_vaults_retryable() {
    let mut state = fresh_state();
    let event = record_sp_notification_result_at(&mut state, vec![7, 8], Ok(vec![]), TEST_NOW_NS);
    assert!(state.sp_attempted_vaults.is_empty());
    assert!(event.is_none());
}

#[test]
fn cdp_10_decodes_full_pool_reply_without_discarding_attempt_ids() {
    #[derive(candid::CandidType)]
    struct FullPoolResult {
        vault_id: u64,
        stables_consumed: std::collections::BTreeMap<Principal, u64>,
        collateral_gained: u64,
        collateral_type: Principal,
        success: bool,
        error_message: Option<String>,
    }

    let empty = candid::encode_args((Vec::<FullPoolResult>::new(),)).unwrap();
    let (empty_decoded,): (Vec<SpNotificationResult>,) = candid::decode_args(&empty).unwrap();
    assert!(empty_decoded.is_empty());

    let reply = candid::encode_args((vec![FullPoolResult {
        vault_id: 42,
        stables_consumed: std::collections::BTreeMap::new(),
        collateral_gained: 0,
        collateral_type: Principal::from_slice(&[10]),
        success: false,
        error_message: Some("held".to_string()),
    }],))
    .unwrap();
    let (decoded,): (Vec<SpNotificationResult>,) = candid::decode_args(&reply).unwrap();
    assert_eq!(decoded, vec![attempt(42, false)]);
}

#[test]
fn cdp_10_transport_err_does_not_blacklist() {
    let mut state = fresh_state();

    let dispatched = vec![42u64, 43u64];
    let err: Result<Vec<SpNotificationResult>, (i32, String)> =
        Err((500, "queue full".to_string()));

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
    let _ = record_sp_notification_result_at(
        &mut state,
        vec![100u64],
        Ok(vec![attempt(100, true)]),
        TEST_NOW_NS,
    );
    assert!(state.sp_attempted_vaults.contains(&100u64));
}

#[test]
fn cdp_10_empty_dispatch_is_noop() {
    let mut state = fresh_state();
    let event = record_sp_notification_result_at(&mut state, vec![], Ok(vec![]), TEST_NOW_NS);
    assert!(state.sp_attempted_vaults.is_empty());
    assert!(event.is_none());
}
