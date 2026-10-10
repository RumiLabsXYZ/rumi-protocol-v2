use candid::{decode_one, encode_args, encode_one, CandidType, Nat, Principal};
use icrc_ledger_types::icrc3::blocks::{GetBlocksRequest, GetBlocksResult};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_amm::{
    state::{InboundOperationPhase, InboundOperationStatus, OutboundPayout, OutboundPayoutStatus},
    types::*,
};
use std::time::Duration;

fn amm_test_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

#[derive(CandidType, Clone, Debug)]
struct LedgerAccount {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

fn reply<T: candid::CandidType + for<'de> candid::Deserialize<'de>>(result: WasmResult) -> T {
    match result {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode canister reply"),
        WasmResult::Reject(reason) => panic!("canister rejected: {reason}"),
    }
}

fn setup() -> (
    PocketIc,
    Principal,
    Principal,
    Principal,
    Principal,
    String,
    [u8; 32],
    [u8; 32],
) {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let user = Principal::self_authenticating(&[71, 72, 73]);
    let token_a = pic.create_canister();
    let token_b = pic.create_canister();
    for ledger in [token_a, token_b] {
        pic.add_cycles(ledger, 2_000_000_000_000);
        pic.install_canister(ledger, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "set_fee",
            encode_one(Nat::from(10u128)).unwrap(),
        )
        .unwrap();
    }
    let amm = pic.create_canister_with_settings(Some(admin), None);
    pic.add_cycles(amm, 2_000_000_000_000);
    pic.install_canister(
        amm,
        amm_test_wasm(),
        encode_one(AmmInitArgs { admin }).unwrap(),
        Some(admin),
    );

    for ledger in [token_a, token_b] {
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "mint",
            encode_args((
                LedgerAccount {
                    owner: user,
                    subaccount: None,
                },
                Nat::from(10_000_000u128),
            ))
            .unwrap(),
        )
        .unwrap();
        let approve = flaky_types::FlakyApproveArgs {
            from_subaccount: None,
            spender: LedgerAccount {
                owner: amm,
                subaccount: None,
            },
            amount: Nat::from(u128::MAX),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            memo: None,
            created_at_time: None,
        };
        // Flaky ledger's local Candid shape is matched below without depending
        // on its crate's private test-only Rust types.
        pic.update_call(ledger, user, "icrc2_approve", encode_one(approve).unwrap())
            .unwrap();
    }

    let create = CreatePoolArgs {
        token_a,
        token_b,
        fee_bps: 30,
        curve: CurveType::ConstantProduct,
    };
    let created: Result<String, AmmError> = reply(
        pic.update_call(amm, admin, "create_pool", encode_one(create).unwrap())
            .unwrap(),
    );
    let pool_id = created.expect("create test pool");
    let seed: Result<([u8; 32], [u8; 32]), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_pool",
            encode_args((pool_id.clone(), 1_000_000u128, 1_000_000u128)).unwrap(),
        )
        .unwrap(),
    );
    let (raw_sub_a, raw_sub_b) = seed.expect("seed test reserves");
    let pool_info: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool_id.clone()).unwrap())
            .unwrap(),
    );
    let pool_info = pool_info.expect("test pool exists");
    let (sub_a, sub_b) = if pool_info.token_a == token_a {
        (raw_sub_a, raw_sub_b)
    } else {
        (raw_sub_b, raw_sub_a)
    };
    for (ledger, subaccount) in [(token_a, sub_a), (token_b, sub_b)] {
        pic.update_call(
            ledger,
            Principal::anonymous(),
            "mint",
            encode_args((
                LedgerAccount {
                    owner: amm,
                    subaccount: Some(subaccount),
                },
                Nat::from(2_000_000u128),
            ))
            .unwrap(),
        )
        .unwrap();
    }
    (pic, amm, token_a, token_b, user, pool_id, sub_a, sub_b)
}

// Keep this fixture type local: the ledger crate does not expose its control
// argument types as a library API.
mod flaky_types {
    use candid::{CandidType, Nat};
    #[derive(CandidType)]
    pub struct FlakyApproveArgs {
        pub from_subaccount: Option<[u8; 32]>,
        pub spender: super::LedgerAccount,
        pub amount: Nat,
        pub expected_allowance: Option<Nat>,
        pub expires_at: Option<u64>,
        pub fee: Option<Nat>,
        pub memo: Option<Vec<u8>>,
        pub created_at_time: Option<u64>,
    }
}

fn set_fault_count(pic: &PocketIc, ledger: Principal, method: &str, count: u32) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        method,
        encode_one(count).unwrap(),
    )
    .unwrap();
}

fn set_transfer_failure(pic: &PocketIc, ledger: Principal, fail: bool) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fail_transfers",
        encode_one(fail).unwrap(),
    )
    .unwrap();
}

fn set_fee_query_failure(pic: &PocketIc, ledger: Principal, fail: bool) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fail_fee_query",
        encode_one(fail).unwrap(),
    )
    .unwrap();
}

fn set_ledger_fee(pic: &PocketIc, ledger: Principal, fee: u128) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fee",
        encode_one(Nat::from(fee)).unwrap(),
    )
    .unwrap();
}

fn status(
    pic: &PocketIc,
    amm: Principal,
    user: Principal,
    request_id: &[u8],
) -> InboundOperationStatus {
    let result: Result<InboundOperationStatus, AmmError> = reply(
        pic.query_call(
            amm,
            user,
            "get_inbound_operation",
            encode_one(request_id.to_vec()).unwrap(),
        )
        .unwrap(),
    );
    result.expect("operation status").clone()
}

fn balance(pic: &PocketIc, ledger: Principal, account: LedgerAccount) -> u128 {
    let result: Nat = reply(
        pic.query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account).unwrap(),
        )
        .unwrap(),
    );
    result.0.try_into().unwrap()
}

fn payout_status(pic: &PocketIc, amm: Principal, caller: Principal, id: u64) -> OutboundPayout {
    let result: Result<OutboundPayout, AmmError> = reply(
        pic.query_call(amm, caller, "get_outbound_payout_status", encode_one(id).unwrap())
            .unwrap(),
    );
    result.expect("authorized payout status")
}

fn last_block_index(pic: &PocketIc, ledger: Principal) -> u64 {
    let result: GetBlocksResult = reply(
        pic.query_call(
            ledger,
            Principal::anonymous(),
            "icrc3_get_blocks",
            encode_one(vec![GetBlocksRequest {
                start: Nat::from(0u8),
                length: Nat::from(100u8),
            }])
            .unwrap(),
        )
        .unwrap(),
    );
    let height: u64 = result.log_length.0.try_into().expect("block height fits u64");
    height - 1
}

fn call_swap(
    pic: &PocketIc,
    amm: Principal,
    user: Principal,
    id: &[u8],
    pool: &str,
    token: Principal,
    amount: u128,
) -> Result<SwapResult, AmmError> {
    reply(
        pic.update_call(
            amm,
            user,
            "swap_v2",
            encode_args((id.to_vec(), pool.to_string(), token, amount, 1u128)).unwrap(),
        )
        .unwrap(),
    )
}

fn request_id(sequence: u64, salt: u8) -> Vec<u8> {
    let mut id = vec![salt; 32];
    id[..8].copy_from_slice(&sequence.to_be_bytes());
    id
}

#[test]
fn unavailable_output_fee_fails_before_request_reservation_or_input_debit() {
    let (pic, amm, token_a, token_b, user, pool, _sub_a, _sub_b) = setup();
    let id = request_id(1, 0x50);
    let before = balance(
        &pic,
        token_a,
        LedgerAccount {
            owner: user,
            subaccount: None,
        },
    );
    set_fee_query_failure(&pic, token_b, true);
    assert!(matches!(
        call_swap(&pic, amm, user, &id, &pool, token_a, 10_000),
        Err(AmmError::InvalidInput { .. })
    ));
    assert_eq!(
        balance(
            &pic,
            token_a,
            LedgerAccount {
                owner: user,
                subaccount: None,
            },
        ),
        before,
        "failed strict fee preflight cannot debit input"
    );
    let next: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            user,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(next.unwrap(), 1, "preflight failure does not consume ID");
}

#[test]
fn partial_remove_payout_keeps_atomic_reserve_event() {
    let (pic, amm, _token_a, _token_b, _user, pool, _sub_a, _sub_b) = setup();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let info: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool.clone()).unwrap())
            .unwrap(),
    );
    let token_b = info.expect("pool exists").token_b;
    // Fixture seeding assigns 1,000 LP shares to the admin. Make B fail so A
    // can succeed first and the second leg remains held.
    set_transfer_failure(&pic, token_b, true);
    let result: Result<(u128, u128), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "remove_liquidity",
            encode_args((pool.clone(), 100u128, 1u128, 1u128)).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(result, Err(AmmError::TransferFailed { .. })));

    let events: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_liquidity_events",
            encode_args((0u64, 10u64)).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(events.len(), 1, "accounting event survives partial delivery");
    assert!(matches!(events[0].action, AmmLiquidityAction::RemoveLiquidity));
    assert_eq!(events[0].amount_a, 100_000);
    assert_eq!(events[0].amount_b, 100_000);
    assert_eq!(events[0].lp_shares, 100);

    let info: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool).unwrap())
            .unwrap(),
    );
    let info = info.expect("pool remains queryable");
    assert_eq!(info.reserve_a + info.reserve_b, 1_800_000);
    assert!(info.paused, "pool remains fenced around held payout");

    let unresolved: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "get_unresolved_outbound_payouts",
            encode_args((0u64, 100u64)).unwrap(),
        )
        .unwrap(),
    );
    let payout = unresolved
        .expect("admin can inspect held outbound rows")
        .into_iter()
        .find(|row| row.operation_id.starts_with("remove_liquidity_b:"))
        .expect("B liability is retained");
    let pruned: Result<(), AmmError> = reply(
        pic.update_call(amm, admin, "pocketic_prune_accounting_events", encode_args(()).unwrap())
            .unwrap(),
    );
    pruned.expect("test-only event-ring pruning");
    let events: Vec<AmmLiquidityEvent> = reply(
        pic.query_call(amm, admin, "get_amm_liquidity_events", encode_args((0u64, 10u64)).unwrap())
            .unwrap(),
    );
    assert!(events.is_empty(), "event history no longer contains the accounting proof");
    set_transfer_failure(&pic, token_b, false);
    let recovered: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "recover_outbound_payout",
            encode_args((payout.id, None::<u64>)).unwrap(),
        )
        .unwrap(),
    );
    recovered.expect("committed B leg replays its exact tuple within ledger window");
    assert!(balance(&pic, token_b, LedgerAccount { owner: admin, subaccount: None }) > 0);
    let remaining: Result<Vec<OutboundPayout>, AmmError> = reply(
        pic.query_call(amm, admin, "get_unresolved_outbound_payouts", encode_args((0u64, 100u64)).unwrap())
            .unwrap(),
    );
    assert!(remaining.unwrap().is_empty(), "only the recovered B row is retired");
}

#[test]
fn partial_admin_fee_withdrawal_records_event_and_retains_other_leg_liability() {
    let (pic, amm, token_a, token_b, _user, pool, sub_a, sub_b) = setup();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let info: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool.clone()).unwrap())
            .unwrap(),
    );
    let info = info.expect("pool exists");
    let pool_sub_a = if info.token_a == token_a { sub_a } else { sub_b };
    let pool_sub_b = if info.token_b == token_b { sub_b } else { sub_a };
    set_ledger_fee(&pic, info.token_a, 123);
    let seeded: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_protocol_fees",
            encode_args((pool.clone(), 500u128, 700u128)).unwrap(),
        )
        .unwrap(),
    );
    seeded.expect("seed protocol-fee balances");
    // B lands but its reply is lost, leaving a genuine aged ambiguity for the
    // direct-block route after the admin event ring is cleared below.
    set_fault_count(&pic, info.token_b, "set_phantom_failures", 1);

    let result: Result<(u128, u128), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "withdraw_protocol_fees",
            encode_one(pool.clone()).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(result, Err(AmmError::TransferFailed { .. })));

    let events: Vec<AmmAdminEvent> = reply(
        pic.query_call(
            amm,
            admin,
            "get_amm_admin_events",
            encode_args((0u64, 10u64)).unwrap(),
        )
        .unwrap(),
    );
    let event = events
        .iter()
        .find(|event| matches!(event.action, AmmAdminAction::WithdrawProtocolFees { .. }))
        .expect("fee-to-liability accounting event survives partial payout");
    assert!(matches!(
        event.action,
        AmmAdminAction::WithdrawProtocolFees {
            amount_a: 500,
            amount_b: 700,
            ..
        }
    ));

    let payouts: Vec<rumi_amm::state::OutboundPayout> = reply(
        pic.query_call(
            amm,
            admin,
            "pocketic_get_outbound_payouts",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(payouts.len(), 1, "successful A row retires; B liability remains");
    assert_eq!(payouts[0].ledger, info.token_b);
    assert_eq!(payouts[0].from_subaccount, Some(pool_sub_b));
    assert_eq!(payouts[0].gross_amount, 700);
    assert_eq!(payouts[0].status, OutboundPayoutStatus::Ambiguous);
    assert!(payouts[0].operation_id.starts_with("protocol_fee_b:"));
    let payout_block = last_block_index(&pic, info.token_b);
    assert_eq!(
        balance(
            &pic,
            info.token_a,
            LedgerAccount {
                owner: admin,
                subaccount: None,
            }
        ),
        377,
        "A leg paid net of ledger fee"
    );
    assert_eq!(
        balance(
            &pic,
            info.token_a,
            LedgerAccount {
                owner: amm,
                subaccount: Some(pool_sub_a),
            }
        ),
        1_999_500
    );

    let reseeded: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_protocol_fees",
            encode_args((pool.clone(), 100u128, 200u128)).unwrap(),
        )
        .unwrap(),
    );
    reseeded.expect("simulate fees accruing after earlier withdrawal");
    let repeated: Result<(u128, u128), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "withdraw_protocol_fees",
            encode_one(pool.clone()).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(repeated, Err(AmmError::PoolBusy)));
    let retained: Result<(u128, u128), AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "pocketic_get_protocol_fees",
            encode_one(pool).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(retained.unwrap(), (100, 200));

    let pruned: Result<(), AmmError> = reply(
        pic.update_call(amm, admin, "pocketic_prune_accounting_events", encode_args(()).unwrap())
            .unwrap(),
    );
    pruned.expect("test-only event-ring pruning");
    let events: Vec<AmmAdminEvent> = reply(
        pic.query_call(amm, admin, "get_amm_admin_events", encode_args((0u64, 10u64)).unwrap())
            .unwrap(),
    );
    assert!(events.is_empty(), "event history no longer contains the accounting proof");
    pic.advance_time(Duration::from_secs(86_401));
    pic.tick();
    let recovered: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "recover_outbound_payout",
            encode_args((payouts[0].id, Some(payout_block))).unwrap(),
        )
        .unwrap(),
    );
    recovered.expect("committed B fee leg proves its exact direct block after window expiry");
    assert_eq!(
        balance(&pic, info.token_b, LedgerAccount { owner: admin, subaccount: None }),
        690,
        "recovery sends net of the pinned fee exactly once"
    );
}

#[test]
fn strict_fee_query_failure_precedes_exit_and_admin_accounting_changes() {
    let (pic, amm, _token_a, _token_b, _user, pool, _sub_a, _sub_b) = setup();
    let admin = Principal::self_authenticating(&[81, 82, 83]);
    let info: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool.clone()).unwrap())
            .unwrap(),
    );
    let before = info.expect("pool exists");
    let seeded: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "pocketic_seed_protocol_fees",
            encode_args((pool.clone(), 500u128, 700u128)).unwrap(),
        )
        .unwrap(),
    );
    seeded.expect("seed protocol-fee balances");
    set_fee_query_failure(&pic, before.token_a, true);

    let remove: Result<(u128, u128), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "remove_liquidity",
            encode_args((pool.clone(), 100u128, 1u128, 1u128)).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(remove, Err(AmmError::TransferFailed { .. })));
    let after: Option<PoolInfo> = reply(
        pic.query_call(amm, admin, "get_pool", encode_one(pool.clone()).unwrap())
            .unwrap(),
    );
    let after = after.expect("pool remains");
    assert_eq!(after.reserve_a, before.reserve_a);
    assert_eq!(after.reserve_b, before.reserve_b);
    assert_eq!(after.total_lp_shares, before.total_lp_shares);

    let withdraw: Result<(u128, u128), AmmError> = reply(
        pic.update_call(
            amm,
            admin,
            "withdraw_protocol_fees",
            encode_one(pool.clone()).unwrap(),
        )
        .unwrap(),
    );
    assert!(matches!(withdraw, Err(AmmError::TransferFailed { .. })));
    let fees: Result<(u128, u128), AmmError> = reply(
        pic.query_call(
            amm,
            admin,
            "pocketic_get_protocol_fees",
            encode_one(pool.clone()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(fees.unwrap(), (500, 700));
    let payouts: Vec<rumi_amm::state::OutboundPayout> = reply(
        pic.query_call(
            amm,
            admin,
            "pocketic_get_outbound_payouts",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert!(payouts.is_empty(), "fee-query failure creates no liability row");
}

#[test]
fn effect_then_error_replays_exact_input_once_and_survives_upgrade() {
    let (pic, amm, token_a, token_b, user, pool, sub_a, sub_b) = setup();
    let id = request_id(1, 0x51);
    let before_in = balance(
        &pic,
        token_a,
        LedgerAccount {
            owner: user,
            subaccount: None,
        },
    );
    let before_out = balance(
        &pic,
        token_b,
        LedgerAccount {
            owner: user,
            subaccount: None,
        },
    );
    set_fault_count(&pic, token_a, "set_phantom_failures", 1);

    let first = call_swap(&pic, amm, user, &id, &pool, token_a, 10_000);
    assert!(matches!(first, Err(AmmError::TransferFailed { .. })));
    let pending = status(&pic, amm, user, &id);
    assert!(matches!(
        pending.operation.phase,
        InboundOperationPhase::Held
    ));
    assert_eq!(
        balance(
            &pic,
            token_a,
            LedgerAccount {
                owner: user,
                subaccount: None
            }
        ),
        before_in - 10_010
    );
    assert_eq!(
        balance(
            &pic,
            token_a,
            LedgerAccount {
                owner: amm,
                subaccount: Some(sub_a)
            }
        ),
        2_010_000
    );

    pic.upgrade_canister(
        amm,
        amm_test_wasm(),
        encode_one(AmmInitArgs {
            admin: Principal::self_authenticating(&[81, 82, 83]),
        })
        .unwrap(),
        Some(Principal::self_authenticating(&[81, 82, 83])),
    )
    .expect("upgrade preserves held inbound state");
    let next_sequence: Result<u64, AmmError> = reply(
        pic.query_call(
            amm,
            user,
            "get_next_inbound_sequence",
            encode_args(()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        next_sequence.unwrap(),
        2,
        "global high-water survives upgrade"
    );
    let recovered = call_swap(&pic, amm, user, &id, &pool, token_a, 10_000)
        .expect("same-ID exact replay recovers Duplicate receipt and completes");
    assert!(matches!(
        status(&pic, amm, user, &id).operation.phase,
        InboundOperationPhase::Completed
    ));
    assert_eq!(
        balance(
            &pic,
            token_a,
            LedgerAccount {
                owner: user,
                subaccount: None
            }
        ),
        before_in - 10_010
    );
    assert_eq!(
        balance(
            &pic,
            token_b,
            LedgerAccount {
                owner: user,
                subaccount: None
            }
        ),
        before_out + recovered.amount_out
    );
    assert_eq!(
        balance(
            &pic,
            token_b,
            LedgerAccount {
                owner: amm,
                subaccount: Some(sub_b)
            }
        ),
        2_000_000 - recovered.amount_out - 10
    );

    let settled = call_swap(&pic, amm, user, &id, &pool, token_a, 10_000).expect("terminal replay");
    assert_eq!(settled.amount_out, recovered.amount_out);
    assert!(recovered.amount_out > 0);
    assert_eq!(
        balance(
            &pic,
            token_b,
            LedgerAccount {
                owner: user,
                subaccount: None
            }
        ),
        before_out + recovered.amount_out
    );
}

#[test]
fn output_payout_liability_survives_upgrade_without_double_accounting() {
    let (pic, amm, token_a, token_b, user, pool, _sub_a, sub_b) = setup();
    let id = request_id(1, 0x52);
    let before_out = balance(
        &pic,
        token_b,
        LedgerAccount {
            owner: user,
            subaccount: None,
        },
    );
    set_transfer_failure(&pic, token_b, true);

    let first = call_swap(&pic, amm, user, &id, &pool, token_a, 10_000);
    assert!(matches!(first, Err(AmmError::TransferFailed { .. })));
    let held = status(&pic, amm, user, &id);
    assert!(matches!(
        held.operation.phase,
        InboundOperationPhase::OutputPending
    ));
    assert!(held.operation.output_payout_id.is_some());
    assert!(matches!(
        held.linked_payout_status,
        Some(OutboundPayoutStatus::Ambiguous)
    ));
    assert!(matches!(get_pool_paused(&pic, amm, &pool), true));

    pic.upgrade_canister(
        amm,
        amm_test_wasm(),
        encode_one(AmmInitArgs {
            admin: Principal::self_authenticating(&[81, 82, 83]),
        })
        .unwrap(),
        Some(Principal::self_authenticating(&[81, 82, 83])),
    )
    .expect("upgrade preserves output liability and pool fence");
    set_transfer_failure(&pic, token_b, false);
    let recovered = call_swap(&pic, amm, user, &id, &pool, token_a, 10_000)
        .expect("same request resumes saved outbound identity");
    assert!(matches!(
        status(&pic, amm, user, &id).operation.phase,
        InboundOperationPhase::Completed
    ));
    assert_eq!(
        balance(
            &pic,
            token_b,
            LedgerAccount {
                owner: user,
                subaccount: None
            }
        ),
        before_out + recovered.amount_out
    );
    assert_eq!(
        balance(
            &pic,
            token_b,
            LedgerAccount {
                owner: amm,
                subaccount: Some(sub_b)
            }
        ),
        2_000_000 - recovered.amount_out - 10
    );
    let status = status(&pic, amm, user, &id);
    assert!(status.operation.output_payout_id.is_some());
    assert_eq!(status.operation.output_ledger_fee, Some(10));
    assert_eq!(
        status.linked_payout_status, None,
        "settled payout row may be retired after the operation keeps its terminal result"
    );
    assert_eq!(
        status.operation.result_amount.unwrap() - recovered.amount_out,
        10
    );
}

#[test]
fn aged_output_liability_requires_exact_direct_block_and_finalizes_once() {
    let (pic, amm, token_a, token_b, user, pool, _sub_a, _sub_b) = setup();
    let id = request_id(1, 0x53);
    let before_output = balance(&pic, token_b, LedgerAccount { owner: user, subaccount: None });
    set_fault_count(&pic, token_b, "set_phantom_failures", 1);
    assert!(matches!(
        call_swap(&pic, amm, user, &id, &pool, token_a, 10_000),
        Err(AmmError::TransferFailed { .. })
    ));
    let held = status(&pic, amm, user, &id);
    assert!(matches!(held.operation.phase, InboundOperationPhase::OutputPending));
    let payout_id = held.operation.output_payout_id.expect("linked payout id");
    let payout = payout_status(&pic, amm, user, payout_id);
    assert_eq!(payout.status, OutboundPayoutStatus::Ambiguous);
    assert_eq!(
        balance(&pic, token_b, LedgerAccount { owner: user, subaccount: None }),
        before_output + payout.net_amount,
        "phantom error represents an effect followed by a lost reply"
    );
    let exact_block = last_block_index(&pic, token_b);
    pic.advance_time(Duration::from_secs(86_401));
    pic.tick();

    let wrong: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            user,
            "recover_outbound_payout",
            encode_args((payout_id, Some(exact_block + 1))).unwrap(),
        )
        .unwrap(),
    );
    assert!(wrong.is_err(), "a missing/wrong block is not evidence of no effect");
    assert_eq!(
        payout_status(&pic, amm, user, payout_id).status,
        OutboundPayoutStatus::Ambiguous,
        "wrong evidence cannot retire the liability"
    );

    let recovered: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            user,
            "recover_outbound_payout",
            encode_args((payout_id, Some(exact_block))).unwrap(),
        )
        .unwrap(),
    );
    recovered.expect("exact direct block finalizes the held swap payout");
    assert!(matches!(
        status(&pic, amm, user, &id).operation.phase,
        InboundOperationPhase::Completed
    ));
    assert_eq!(
        balance(&pic, token_b, LedgerAccount { owner: user, subaccount: None }),
        before_output + payout.net_amount,
        "finalization does not transfer twice"
    );
    let replay: Result<(), AmmError> = reply(
        pic.update_call(
            amm,
            user,
            "recover_outbound_payout",
            encode_args((payout_id, Some(exact_block))).unwrap(),
        )
        .unwrap(),
    );
    assert!(replay.is_err(), "the once-only payout row has been retired");
}

fn get_pool_paused(pic: &PocketIc, amm: Principal, pool: &str) -> bool {
    let info: Option<PoolInfo> = reply(
        pic.query_call(
            amm,
            Principal::anonymous(),
            "get_pool",
            encode_one(pool.to_string()).unwrap(),
        )
        .unwrap(),
    );
    info.expect("pool exists").paused
}
