//! BOT-10 runtime regression: the deprecated boolean admin resolver cannot
//! settle or cancel a live bot claim without the corresponding ledger proof.
//!
//! Exercise the deployed update boundary with a real bot claim. Both boolean
//! choices must fail closed for the developer, and unauthorized callers must
//! remain unauthorized. After every call, compare the public vault fields,
//! open-claim set, reserved bot budget, and per-vault event history.

include!("common/bot_claim_fixture.rs");

#[derive(Debug, PartialEq, Eq)]
struct ObservableState {
    vaults: Vec<(u64, u64, u64, u64, Principal)>,
    active_claim_ids: Vec<u64>,
    bot_budget: (u64, u64, u64, u64),
    vault_events: Vec<(u64, rumi_protocol_backend::event::Event)>,
}

fn observable_state(fixture: &Fixture) -> ObservableState {
    let vaults: Vec<rumi_protocol_backend::vault::CandidVault> = match fixture
        .pic
        .query_call(
            fixture.protocol_id,
            Principal::anonymous(),
            "get_vaults",
            encode_args((Some(fixture.test_user),)).unwrap(),
        )
        .expect("get_vaults query failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode get_vaults"),
        WasmResult::Reject(message) => panic!("get_vaults rejected: {message}"),
    };
    let vaults = vaults
        .into_iter()
        .map(|vault| {
            (
                vault.vault_id,
                vault.borrowed_icusd_amount,
                vault.collateral_amount,
                vault.accrued_interest,
                vault.collateral_type,
            )
        })
        .collect();

    let vault_events = match fixture
        .pic
        .query_call(
            fixture.protocol_id,
            Principal::anonymous(),
            "get_vault_history",
            encode_args((fixture.vault_id,)).unwrap(),
        )
        .expect("get_vault_history query failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode get_vault_history"),
        WasmResult::Reject(message) => panic!("get_vault_history rejected: {message}"),
    };

    let stats = get_bot_stats(&fixture.pic, fixture.protocol_id);
    ObservableState {
        vaults,
        active_claim_ids: get_active_claim_ids(&fixture.pic, fixture.protocol_id),
        bot_budget: (
            stats.budget_total_e8s,
            stats.budget_remaining_e8s,
            stats.budget_start_timestamp,
            stats.total_debt_covered_e8s,
        ),
        vault_events,
    }
}

fn resolve_stuck_claim(
    fixture: &Fixture,
    caller: Principal,
    apply_debt_reduction: bool,
) -> Result<(), ProtocolError> {
    match fixture
        .pic
        .update_call(
            fixture.protocol_id,
            caller,
            "admin_resolve_stuck_claim",
            encode_args((fixture.vault_id, apply_debt_reduction)).unwrap(),
        )
        .expect("admin_resolve_stuck_claim call failed")
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode admin resolver result"),
        WasmResult::Reject(message) => panic!("admin resolver rejected: {message}"),
    }
}

#[test]
fn bot_010_proofless_admin_resolution_preserves_live_claim_state() {
    let fixture = setup_fixture();
    let (_budget_before_claim, _claim_timestamp, _claim) = seed_bot_claim(&fixture);
    let claimed_state = observable_state(&fixture);
    assert!(claimed_state.active_claim_ids.contains(&fixture.vault_id));
    assert!(claimed_state
        .vaults
        .iter()
        .any(|vault| vault.0 == fixture.vault_id));

    for apply_debt_reduction in [false, true] {
        for (caller, expected_message) in [
            (
                fixture.developer,
                "Unsafe stuck-claim recovery disabled",
            ),
            (fixture.test_user, "Unauthorized: developer only"),
        ] {
            let before = observable_state(&fixture);
            assert_eq!(before, claimed_state, "state changed before resolver call");

            let error = resolve_stuck_claim(&fixture, caller, apply_debt_reduction)
                .expect_err("proofless compatibility resolver must fail closed");
            match error {
                ProtocolError::GenericError(message) => {
                    assert!(
                        message.contains(expected_message),
                        "unexpected error for caller {caller} and apply_debt_reduction={apply_debt_reduction}: {message}"
                    );
                }
                other => panic!("unexpected resolver error: {other:?}"),
            }

            assert_eq!(
                observable_state(&fixture),
                before,
                "proofless resolver mutated claim, vault, bot budget, or vault events for caller {caller} and apply_debt_reduction={apply_debt_reduction}"
            );
        }
    }
}
