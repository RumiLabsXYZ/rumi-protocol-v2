//! CL-10 end-to-end backend interest mint recovery against the real SP.
//!
//! The test commits an icUSD mint while returning a simulated lost reply,
//! proves the backend retains its exact mint outbox row, then lets the backend
//! retry the same ledger tuple and deliver the receipt to the production SP.

include!("common/bot_claim_fixture.rs");

use std::{fs, path::PathBuf};

#[derive(CandidType, Deserialize)]
struct SpInitArgs {
    protocol_canister_id: Principal,
    authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Deserialize)]
struct StablecoinConfigWire {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    priority: u8,
    is_active: bool,
    transfer_fee: Option<u64>,
    is_lp_token: Option<bool>,
    underlying_pool: Option<Principal>,
}

#[derive(CandidType, Deserialize)]
struct InterestRecipientWire {
    destination: String,
    bps: u64,
}

#[derive(CandidType, Deserialize)]
struct PendingInterestMintWire {
    operation_nonce: u128,
    ledger: Principal,
    pool: Principal,
    amount_e8s: u64,
    collateral_type: Principal,
    memo: Vec<u8>,
    created_at_time_ns: u64,
    phase: InterestMintPhaseWire,
    attempts: u8,
    last_attempt_at_ns: u64,
}

#[derive(CandidType, Deserialize)]
enum InterestMintPhaseWire {
    MintPending,
    NotificationPending { mint_block: u64 },
    Held { reason: String },
}

fn artifact_from_env(var: &str, fallback: &str) -> Vec<u8> {
    let path = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fallback));
    fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn cl10_backend_wasm() -> Vec<u8> {
    artifact_from_env(
        "RUMI_CL10_BACKEND_WASM",
        "../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm",
    )
}

fn cl10_sp_wasm() -> Vec<u8> {
    artifact_from_env(
        "RUMI_CL10_SP_WASM",
        "../../../target/wasm32-unknown-unknown/release/stability_pool.wasm",
    )
}

fn cl10_fixture() -> (Fixture, Principal, Principal) {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let test_user = Principal::self_authenticating(b"cl10-interest-user");
    let developer = Principal::self_authenticating(b"cl10-interest-developer");
    let treasury = Principal::self_authenticating(b"cl10-interest-treasury");
    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set backend controllers");
    let bot_id = pic.create_canister();
    pic.add_cycles(bot_id, 2_000_000_000_000);

    let icp_ledger = deploy_native_icp_ledger(&pic, test_user, 1_000_000_000_000, developer, 1_000);
    let icusd_ledger = pic.create_canister();
    pic.add_cycles(icusd_ledger, 2_000_000_000_000);
    pic.install_canister(
        icusd_ledger,
        flaky_ledger_wasm(),
        encode_one(()).unwrap(),
        None,
    );
    let ckusdc_ledger = pic.create_canister();
    pic.add_cycles(ckusdc_ledger, 2_000_000_000_000);
    pic.install_canister(
        ckusdc_ledger,
        flaky_ledger_wasm(),
        encode_one(()).unwrap(),
        None,
    );
    let xrc_id = pic.create_canister();
    pic.add_cycles(xrc_id, 1_000_000_000_000);
    pic.install_canister(xrc_id, xrc_wasm(), prepare_mock_xrc(), None);
    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let init = ProtocolArgVariant::Init(ProtocolInitArg {
        xrc_principal: xrc_id,
        icusd_ledger_principal: icusd_ledger,
        icp_ledger_principal: icp_ledger,
        ckusdc_ledger_principal: Some(ckusdc_ledger),
        fee_e8s: 10_000,
        developer_principal: developer,
    });
    pic.install_canister(
        protocol_id,
        cl10_backend_wasm(),
        encode_one(init).unwrap(),
        None,
    );
    let mint_setup = pic
        .update_call(
            icusd_ledger,
            Principal::anonymous(),
            "set_minting_account",
            encode_one(Some(account(protocol_id))).unwrap(),
        )
        .expect("set icUSD minter");
    match mint_setup {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect("decode minter setup"),
        WasmResult::Reject(message) => panic!("set_minting_account rejected: {message}"),
    }

    pic.install_canister(
        bot_id,
        liquidation_bot_wasm(),
        encode_one(BotInitArgs {
            config: BotConfig {
                backend_principal: protocol_id,
                treasury_principal: treasury,
                admin: developer,
                max_slippage_bps: 200,
                icp_ledger,
                ckusdc_ledger,
                icpswap_pool: xrc_id,
                icpswap_zero_for_one: Some(true),
                icp_fee_e8s: Some(10_000),
                ckusdc_fee_e6: Some(0),
                three_pool_principal: None,
                kong_swap_principal: None,
                ckusdt_ledger: None,
                icusd_ledger: Some(icusd_ledger),
            },
        })
        .unwrap(),
        None,
    );
    set_xrc_fetch_interval(&pic, protocol_id, developer, 60);
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }
    for (method, args) in [
        (
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        ),
        (
            "set_rate_curve_markers",
            encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
        ),
        ("set_borrowing_fee", encode_args((0.0f64,)).unwrap()),
        (
            "set_interest_rate",
            encode_args((icp_ledger, 0.1f64)).unwrap(),
        ),
        ("set_treasury_principal", encode_args((treasury,)).unwrap()),
    ] {
        let result = pic
            .update_call(protocol_id, developer, method, args)
            .unwrap();
        match result {
            WasmResult::Reply(bytes) => {
                decode_one::<Result<(), ProtocolError>>(&bytes)
                    .unwrap_or_else(|error| panic!("decode {method}: {error}"))
                    .unwrap_or_else(|error| panic!("{method} failed: {error:?}"));
            }
            WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
        }
    }

    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 50_000_000_000);
    let vault_id = open_vault_v2_call(&pic, protocol_id, test_user, 1, 5_000_000_000);
    let borrowed = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((VaultArg {
                vault_id,
                amount: 10_000_000_000u64,
            },))
            .unwrap(),
        )
        .unwrap();
    match borrowed {
        WasmResult::Reply(bytes) => {
            decode_one::<Result<SuccessWithFee, ProtocolError>>(&bytes)
                .expect("decode borrow")
                .expect("borrow succeeds");
        }
        WasmResult::Reject(message) => panic!("borrow rejected: {message}"),
    }

    let fixture = Fixture {
        pic,
        protocol_id,
        bot_id,
        icp_ledger,
        icusd_ledger,
        ckusdc_ledger,
        xrc_id,
        developer,
        test_user,
        vault_id,
        second_vault_id: 0,
    };

    let sp = fixture.pic.create_canister();
    fixture.pic.add_cycles(sp, 2_000_000_000_000);
    fixture.pic.install_canister(
        sp,
        cl10_sp_wasm(),
        encode_one(SpInitArgs {
            protocol_canister_id: protocol_id,
            authorized_admins: vec![developer],
        })
        .unwrap(),
        None,
    );
    let register = fixture
        .pic
        .update_call(
            sp,
            developer,
            "register_stablecoin",
            encode_one(StablecoinConfigWire {
                ledger_id: icusd_ledger,
                symbol: "icUSD".into(),
                decimals: 8,
                priority: 1,
                is_active: true,
                transfer_fee: Some(0),
                is_lp_token: None,
                underlying_pool: None,
            })
            .unwrap(),
        )
        .unwrap();
    match register {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolErrorWire>>(&bytes)
            .expect("decode SP ledger registration")
            .expect("register icUSD"),
        WasmResult::Reject(message) => panic!("register_stablecoin rejected: {message}"),
    }

    for (method, args) in [
        ("set_stability_pool_principal", encode_one(sp).unwrap()),
        (
            "set_interest_split",
            encode_one(vec![InterestRecipientWire {
                destination: "stability_pool".into(),
                bps: 10_000,
            }])
            .unwrap(),
        ),
    ] {
        let result = fixture
            .pic
            .update_call(protocol_id, developer, method, args)
            .unwrap();
        match result {
            WasmResult::Reply(bytes) => {
                let _: Result<(), ProtocolError> = decode_one(&bytes).expect("decode {method}");
            }
            WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
        }
    }

    let deposit_user = Principal::self_authenticating(b"cl10-interest-depositor");
    call_unit(
        &fixture.pic,
        icusd_ledger,
        Principal::anonymous(),
        "mint",
        encode_args((account(deposit_user), Nat::from(1_000_000_000u64))).unwrap(),
    );
    icrc2_approve_call(&fixture.pic, icusd_ledger, deposit_user, sp, 500_000_000);
    let deposit = fixture
        .pic
        .update_call(
            sp,
            deposit_user,
            "deposit",
            encode_args((icusd_ledger, 500_000_000u64)).unwrap(),
        )
        .unwrap();
    match deposit {
        WasmResult::Reply(bytes) => decode_one::<Result<(), StabilityPoolErrorWire>>(&bytes)
            .expect("decode SP deposit")
            .expect("deposit icUSD"),
        WasmResult::Reject(message) => panic!("SP deposit rejected: {message}"),
    }
    (fixture, sp, deposit_user)
}

#[derive(CandidType, Deserialize, Debug)]
enum StabilityPoolErrorWire {
    #[serde(rename = "Unauthorized")]
    Unauthorized,
}

fn call_unit(pic: &PocketIc, canister: Principal, caller: Principal, method: &str, args: Vec<u8>) {
    match pic
        .update_call(canister, caller, method, args)
        .expect(method)
    {
        WasmResult::Reply(bytes) => decode_one::<()>(&bytes).expect(method),
        WasmResult::Reject(message) => panic!("{method} rejected: {message}"),
    }
}

#[test]
fn cl10_interest_mint_reply_loss_retries_once_and_reaches_real_sp() {
    let (f, sp, depositor) = cl10_fixture();

    // Keep Timer B from harvesting this vault first. Repayment accrues interest
    // synchronously and sends its earned share through the real backend outbox.
    let long_timer = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_interest_treasury_tick_interval_secs",
            encode_one(32_000_000u64).unwrap(),
        )
        .expect("delay Timer B until after repayment");
    match long_timer {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .expect("decode long timer interval")
            .expect("set long timer interval"),
        WasmResult::Reject(message) => panic!("set long timer interval rejected: {message}"),
    }
    f.pic.advance_time(Duration::from_secs(31_536_000));
    call_unit(
        &f.pic,
        f.icusd_ledger,
        Principal::anonymous(),
        "mint",
        encode_args((account(f.test_user), Nat::from(10_100_000_000u64))).unwrap(),
    );
    icrc2_approve_call(
        &f.pic,
        f.icusd_ledger,
        f.test_user,
        f.protocol_id,
        10_100_000_000,
    );
    call_unit(
        &f.pic,
        f.icusd_ledger,
        Principal::anonymous(),
        "set_phantom_mint_failures",
        encode_one(1u32).unwrap(),
    );
    let repayment = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "repay_to_vault_v2",
            encode_args((
                1u128,
                VaultArg {
                    vault_id: f.vault_id,
                    amount: 10_000_000_000,
                },
            ))
            .unwrap(),
        )
        .expect("repay through the backend interest path");
    match repayment {
        WasmResult::Reply(bytes) => {
            let status = decode_one::<
                Result<rumi_protocol_backend::RepaymentV2StatusView, ProtocolError>,
            >(&bytes)
            .expect("decode repayment")
            .expect("repayment succeeds despite its mint's lost reply");
            assert!(
                matches!(
                    status.phase,
                    rumi_protocol_backend::RepaymentV2Phase::RepayCommitted
                        | rumi_protocol_backend::RepaymentV2Phase::Complete
                ),
                "mint-only fault must not affect the repayment transfer_from: {status:?}"
            );
        }
        WasmResult::Reject(message) => panic!("repayment rejected: {message}"),
    }

    // Repayment commits its interest share into the pending pool accumulator;
    // Timer B flushes that amount into the mint outbox. Trigger only one pass
    // so the deliberately lost first mint reply remains observable.
    let timer_update = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_interest_treasury_tick_interval_secs",
            encode_one(1u64).unwrap(),
        )
        .expect("set first flush timer interval");
    match timer_update {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .expect("decode timer interval update")
            .expect("set timer interval"),
        WasmResult::Reject(message) => panic!("set timer interval rejected: {message}"),
    }
    f.pic.advance_time(Duration::from_secs(2));
    f.pic.tick();

    let pending: Result<Vec<PendingInterestMintWire>, ProtocolError> = match f
        .pic
        .query_call(
            f.protocol_id,
            f.developer,
            "list_pending_sp_interest_mints",
            encode_args((None::<u128>, 10u16)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode pending mint inventory"),
        WasmResult::Reject(message) => panic!("pending mint query rejected: {message}"),
    };
    let pending = pending.expect("controller pending mint query");
    assert_eq!(
        pending.len(),
        1,
        "lost mint reply must retain its durable row"
    );
    assert!(matches!(
        &pending[0].phase,
        InterestMintPhaseWire::MintPending
    ));
    assert_eq!(pending[0].attempts, 1);
    assert_eq!(pending[0].pool, sp);
    assert!(pending[0].amount_e8s > 0);
    assert_eq!(
        icrc1_balance_of_call(&f.pic, f.icusd_ledger, sp),
        500_000_000 + pending[0].amount_e8s,
        "fault must commit exactly one mint on top of the initial SP stake"
    );

    // The retry interval is bounded; Timer B drains the same durable tuple.
    let retry_timer_update = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "set_interest_treasury_tick_interval_secs",
            encode_one(1u64).unwrap(),
        )
        .expect("set retry timer interval");
    match retry_timer_update {
        WasmResult::Reply(bytes) => decode_one::<Result<(), ProtocolError>>(&bytes)
            .expect("decode timer interval update")
            .expect("set timer interval"),
        WasmResult::Reject(message) => panic!("set timer interval rejected: {message}"),
    }
    f.pic.advance_time(Duration::from_secs(30));
    for _ in 0..10 {
        f.pic.tick();
    }
    let pending_after: Result<Vec<PendingInterestMintWire>, ProtocolError> = match f
        .pic
        .query_call(
            f.protocol_id,
            f.developer,
            "list_pending_sp_interest_mints",
            encode_args((None::<u128>, 10u16)).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode retried mint inventory"),
        WasmResult::Reject(message) => panic!("retried mint query rejected: {message}"),
    };
    assert!(pending_after
        .expect("controller pending mint query")
        .is_empty());
    assert_eq!(
        icrc1_balance_of_call(&f.pic, f.icusd_ledger, sp),
        500_000_000 + pending[0].amount_e8s
    );
    assert_eq!(
        icrc1_balance_of_call(&f.pic, f.icusd_ledger, depositor),
        500_000_000
    );
    let notification_count: u64 = match f
        .pic
        .query_call(
            f.protocol_id,
            f.developer,
            "get_pending_stability_pool_interest_notification_count",
            encode_args(()).unwrap(),
        )
        .unwrap()
    {
        WasmResult::Reply(bytes) => decode_one(&bytes).expect("decode notification count"),
        WasmResult::Reject(message) => panic!("notification count rejected: {message}"),
    };
    assert_eq!(
        notification_count, 0,
        "real SP acknowledged the mint receipt"
    );
}
