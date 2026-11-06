//! BB-09b-3: integration proofs for the round 2 adversarial review fixes of the ICP icUSD <->
//! Base icUSD escrow bridge.
//!
//! * A. Mint gas: the default limit (300,000) covers the most expensive mint with a real
//!   margin, the mint preflight carries that exact limit, and an out-of-gas simulation is a
//!   recoverable hold instead of a paid on-chain revert.
//! * B. `inspect_message`: anonymous and unauthorized ingress is refused BEFORE execution.
//! * C. `admin_set_rpc_services` records the controller that called it.
//! * D. EIP-55: a mixed-case recipient must match its checksum.
//!
//! Every scenario runs the real bridge wasm in PocketIC against the real ICRC ledger wasm and
//! a local anvil node running the real `BridgedIcUSD` bytecode, with the bridge's Base
//! transactions signed by its own threshold-ECDSA minter key. Unless a test says otherwise
//! the three provider URLs are a pure passthrough to that one anvil, so the evidence is REAL
//! local EVM execution. Tests that install a fault hook say so and label what is MOCKED.
//!
//! Build once, then run:
//!
//! ```text
//! cargo build --offline --release --target wasm32-unknown-unknown -p rumi_icusd_bridge -p evm_rpc_relay
//! POCKET_IC_BIN=$PWD/pocket-ic cargo test --offline -p rumi_icusd_bridge --test bridge_round2_pic -- --test-threads=1 --nocapture
//! ```

mod support;

use candid::Principal;
use serde_json::{json, Value};
use support::anvil::parse_hex_u64;
use support::bridge::*;
use support::contract::{decode_u128, SEL_MINT};
use support::outcalls::{FaultAction, PROVIDER_A, PROVIDER_B, PROVIDER_C};

const PROVIDER_D: &str = "https://provider-d.test/";
const PROVIDER_E: &str = "https://provider-e.test/";

fn e8(n: u64) -> u64 {
    n * E8
}

/// A recipient whose 20 bytes are all non-zero: the most expensive `balanceOf` write.
const DENSE_RECIPIENT: &str = "0xffffffffffffffffffffffffffffffffffffff01";

/// `eth_getBlockByNumber("latest").timestamp` on anvil.
fn latest_block_timestamp(w: &BridgeWorld) -> u64 {
    parse_hex_u64(&w.anvil.rpc("eth_getBlockByNumber", json!(["latest", false]))["timestamp"])
}

/// A no-argument uint view of the bound contract (for example `mintWindowStart()`).
fn contract_uint(w: &BridgeWorld, sig: &str) -> u128 {
    let data = hex::decode(selector_hex(sig).trim_start_matches("0x")).unwrap();
    decode_u128(&w.anvil.eth_call(&w.token.address, &data))
}

/// EIP-55 checksum rendering of a 40-character lowercase hex address (no prefix), written
/// here independently of the canister: keccak256 of the lowercase hex text, uppercase a
/// letter when its hash nibble is 8 or more.
fn eip55(lower40: &str) -> String {
    assert_eq!(lower40.len(), 40);
    assert_eq!(lower40, lower40.to_ascii_lowercase());
    let digest = keccak(lower40.as_bytes());
    let mut out = String::from("0x");
    for (i, c) in lower40.chars().enumerate() {
        let nibble = if i % 2 == 0 { digest[i / 2] >> 4 } else { digest[i / 2] & 0x0f };
        out.push(if c.is_ascii_alphabetic() && nibble >= 8 { c.to_ascii_uppercase() } else { c });
    }
    out
}

fn consent_request(args: &BridgeToBaseArgs) -> ConsentMessageRequest {
    ConsentMessageRequest {
        method: "bridge_to_base".to_string(),
        arg: candid::encode_one(args).expect("encode bridge_to_base args"),
        user_preferences: ConsentMessageSpec {
            metadata: ConsentMessageMetadata { language: "en".to_string(), utc_offset_minutes: None },
            device_spec: Some(DeviceSpec::GenericDisplay),
        },
    }
}

/// `icrc21_canister_call_consent_message` for `bridge_to_base(args)` as `sender`.
fn consent_for(
    w: &mut BridgeWorld,
    sender: Principal,
    args: &BridgeToBaseArgs,
) -> Result<Result<ConsentInfo, Icrc21Error>, CallFailure> {
    w.try_update(sender, "icrc21_canister_call_consent_message", (consent_request(args),))
}

fn services(urls: &[&str]) -> Vec<RpcServiceConfig> {
    urls.iter().map(|u| RpcServiceConfig::Custom { url: u.to_string() }).collect()
}

/// An unbound world: quick to build, enough for admin and ingress scenarios.
fn quick_world() -> BridgeWorld {
    BridgeWorld::setup(SetupOptions { bind_contract: false, unpause: false, ..SetupOptions::default() })
}

// ---------------------------------------------------------------------------
// A. Mint gas
// ---------------------------------------------------------------------------

/// REAL: the whole scenario is local EVM execution on anvil (contract bytecode, gas metering,
/// storage cold/warm costs, the contract's own mint window logic) and the bridge's real
/// tECDSA-signed mint. The only simulated thing is time: `evm_increaseTime` moves anvil's
/// clock past the contract's first mint window, which is what makes the first mint reset the
/// window (the storage-expensive path).
#[test]
fn first_mint_after_the_contract_window_elapsed_to_a_dense_recipient_succeeds_within_gas_margin() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let user = w.user;
    let info = w.bridge_info();
    let limit = info.gas.mint_gas_limit;
    assert_eq!(limit, 300_000, "the default mint gas limit: {:?}", info.gas);
    let binding = info.contract.clone().expect("bound contract");

    // Worst case storage conditions: the very first mint on the contract...
    assert_eq!(w.base_total_supply(), 0, "totalSupply is 0: this is the first mint ever");
    assert_eq!(w.minter_nonce(), 0);
    // ...after the contract's first mint window has elapsed (anvil clock moved forward).
    let window_start = contract_uint(&w, "mintWindowStart()") as u64;
    let window_seconds = binding.mint_window_seconds;
    assert_eq!(contract_uint(&w, "mintWindowSeconds()") as u64, window_seconds);
    w.anvil.rpc("evm_increaseTime", json!([window_seconds + 3_600]));
    w.mine_finalize();
    let now_chain = latest_block_timestamp(&w);
    assert!(
        now_chain >= window_start + window_seconds,
        "the mint window must have elapsed: block time {now_chain}, window ends {}",
        window_start + window_seconds
    );

    // A recipient whose 20 bytes are all non-zero (lowercase, so no checksum applies).
    assert!(hex::decode(&DENSE_RECIPIENT[2..]).unwrap().iter().all(|b| *b != 0), "20 non-zero bytes");
    let d = w.approve_and_deposit(user, e8(100), DENSE_RECIPIENT);
    assert_eq!(d.status, DepositStatus::Escrowed, "{d:?}");
    let d = w.wait_mint_included(d.id);
    assert_eq!(d.mint_tx_hashes.len(), 1, "exactly one signed attempt (no revert, no replacement)");
    let tx_hash = d.mint_tx_hashes[0].clone();

    // The mint transaction really executed on anvil.
    let rc = w.receipt(&tx_hash).expect("mint receipt on anvil");
    assert_eq!(rc["status"], "0x1", "mint tx succeeded on anvil: {rc}");
    let gas_used = parse_hex_u64(&rc["gasUsed"]);
    let tx = w.anvil.rpc("eth_getTransactionByHash", json!([tx_hash]));
    let tx_gas = parse_hex_u64(&tx["gas"]);
    println!(
        "MEASURED first mint after window to a dense recipient: gasUsed={gas_used}, tx gas limit={tx_gas} ({:.1}% of the limit)",
        100.0 * gas_used as f64 / tx_gas as f64
    );
    assert_eq!(tx_gas, limit, "the signed transaction carries exactly the configured limit");
    assert!(
        (gas_used as f64) < 0.8 * limit as f64,
        "gasUsed {gas_used} must stay below 80% of the {limit} limit"
    );
    assert!(gas_used > 120_000, "the cold-storage worst case should be exercised, got {gas_used}");
    let (_mint_id, to, amount) = w.bridge_mint_of(&tx_hash);
    assert_eq!((to.as_str(), amount), (DENSE_RECIPIENT, e8(100) as u128));

    // The preflight simulation carried the same limit (REAL: logged from the live requests the
    // providers received).
    let sim_gas: Vec<String> = w
        .router
        .log()
        .iter()
        .flat_map(|r| match &r.request {
            Value::Array(items) => items.clone(),
            v => vec![v.clone()],
        })
        .filter(|req| req["method"] == "eth_call")
        .filter(|req| {
            req["params"][0]["data"].as_str().map(|d| d.starts_with(&format!("0x{}", hex::encode(SEL_MINT)))).unwrap_or(false)
        })
        .map(|req| req["params"][0]["gas"].as_str().unwrap_or("<missing>").to_string())
        .collect();
    assert!(!sim_gas.is_empty(), "the mint preflight ran an eth_call simulation");
    assert!(
        sim_gas.iter().all(|g| *g == format!("0x{limit:x}")),
        "every mint simulation carries gas = the configured limit (0x{limit:x}): {sim_gas:?}"
    );

    // Finality and delivery.
    w.mine_finalize();
    let d = w.wait_delivered(d.id);
    assert_eq!(d.status, DepositStatus::Delivered);
    assert_eq!(w.base_balance(DENSE_RECIPIENT), e8(100) as u128);
    assert_eq!(w.base_total_supply(), e8(100) as u128);
    let audit = w.reconcile();
    assert_eq!(audit.coverage_ok, Some(true), "{audit:?}");
    assert_eq!(audit.outstanding(), e8(100) as u128);
}

/// REAL: the setup, the escrow, the ledger pull and the later mint on anvil. MOCKED: the three
/// providers' answer to the mint simulation (`eth_call` with the mint selector) is fabricated as
/// `EVM error OutOfGas`, because the admin floor (250,000) is above the real worst case and a
/// real out-of-gas cannot be provoked through the public interface. What is proven is how the
/// canister reacts: a hold, no signed transaction, no nonce spent, an audit note, and an
/// automatic retry that mints once the (mocked) condition clears.
#[test]
fn out_of_gas_mint_simulation_holds_the_deposit_without_a_transaction_then_retries() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let user = w.user;
    let contract = w.token.address.clone();
    let mint_selector = format!("0x{}", hex::encode(SEL_MINT));
    let limit = w.bridge_info().gas.mint_gas_limit;

    let (c, sel) = (contract.clone(), mint_selector.clone());
    w.router.set_fault(move |_provider, req| {
        let is_mint_sim = req["method"] == "eth_call"
            && req["params"][0]["to"].as_str().map(|t| t.eq_ignore_ascii_case(&c)).unwrap_or(false)
            && req["params"][0]["data"].as_str().map(|d| d.starts_with(&sel)).unwrap_or(false);
        if is_mint_sim {
            FaultAction::ReplaceBody(
                json!({"jsonrpc": "2.0", "id": req["id"].clone(), "error": {"code": -32603, "message": "EVM error OutOfGas"}})
                    .to_string(),
            )
        } else {
            FaultAction::Pass
        }
    });

    let holder = w.holder(0);
    let d = w.approve_and_deposit(user, e8(10), &holder);
    assert_eq!(d.status, DepositStatus::Escrowed, "{d:?}");
    let d = w.wait_deposit(d.id, "held because the simulation ran out of gas", |d| {
        matches!(&d.status, DepositStatus::MintHeld { .. })
    });
    match &d.status {
        DepositStatus::MintHeld { reason } => {
            assert!(reason.contains("ran out of gas") && reason.contains(&limit.to_string()), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    // Nothing was signed or broadcast: no transaction, no nonce spent, nothing minted.
    assert!(d.mint_tx_hashes.is_empty() && d.mint_stage.is_none(), "{d:?}");
    assert_eq!(w.minter_nonce(), 0);
    assert_eq!(w.base_total_supply(), 0);
    let log = w.audit_log_all();
    assert!(
        log.iter().any(|e| e.deposit_id == Some(d.id) && e.note.contains("raise mint_gas_limit")),
        "the hold is audited with the operator hint"
    );
    // The escrow is intact and still counted as owed to the user.
    assert_eq!(w.escrow_balance(), e8(10) as u128);

    // The condition clears; the preflight hold retries on its own after ten minutes.
    w.router.clear_fault();
    w.advance_secs(11 * 60);
    w.wait_mint_included(d.id);
    w.mine_finalize();
    let d = w.wait_delivered(d.id);
    assert_eq!(d.mint_tx_hashes.len(), 1, "exactly one transaction, sent only after the hold cleared");
    assert_eq!(w.base_balance(&holder), e8(10) as u128);
    assert_eq!(w.minter_nonce(), 1);
}

/// REAL: the canister's own validation (no chain involved).
#[test]
fn admin_set_gas_enforces_the_new_floors_and_the_ceiling() {
    let mut w = quick_world();
    let original = w.bridge_info().gas;
    assert_eq!(original.mint_gas_limit, 300_000);
    assert!(original.admin_gas_limit >= 80_000, "{original:?}");
    let audit_len = w.audit_log_all().len();
    let set = |w: &mut BridgeWorld, mint: u64, admin: u64| -> BridgeResult<()> {
        let mut g = original.clone();
        g.mint_gas_limit = mint;
        g.admin_gas_limit = admin;
        w.admin_call("admin_set_gas", (g,))
    };

    // Below the floor (including the old 150,000 default) and above the ceiling: refused,
    // nothing stored, nothing audited.
    for bad in [0u64, 21_000, 150_000, 249_999, 1_000_001] {
        match set(&mut w, bad, original.admin_gas_limit) {
            Err(BridgeError::InvalidArgument(m)) => assert!(m.contains("mint_gas_limit"), "{bad}: {m}"),
            other => panic!("mint_gas_limit {bad} must be refused, got {other:?}"),
        }
    }
    for bad in [0u64, 21_000, 79_999, 1_000_001] {
        match set(&mut w, original.mint_gas_limit, bad) {
            Err(BridgeError::InvalidArgument(m)) => assert!(m.contains("admin_gas_limit"), "{bad}: {m}"),
            other => panic!("admin_gas_limit {bad} must be refused, got {other:?}"),
        }
    }
    assert_eq!(w.bridge_info().gas, original, "refused updates store nothing");
    assert_eq!(w.audit_log_all().len(), audit_len, "refused updates are not audited");

    // The floors themselves and the ceiling are accepted.
    assert_eq!(set(&mut w, 250_000, 80_000), Ok(()));
    let g = w.bridge_info().gas;
    assert_eq!((g.mint_gas_limit, g.admin_gas_limit), (250_000, 80_000));
    let last = w.audit_log_all().pop().expect("audit entry");
    assert!(last.note.contains("gas set by") && last.note.contains(&w.admin.to_text()), "{}", last.note);
    assert_eq!(set(&mut w, 1_000_000, 1_000_000), Ok(()));
    let g = w.bridge_info().gas;
    assert_eq!((g.mint_gas_limit, g.admin_gas_limit), (1_000_000, 1_000_000));

    // Restore.
    assert_eq!(w.admin_call::<_, BridgeResult<()>>("admin_set_gas", (original.clone(),)), Ok(()));
    assert_eq!(w.bridge_info().gas, original);
}

// ---------------------------------------------------------------------------
// B. inspect_message
// ---------------------------------------------------------------------------

/// REAL: PocketIC runs the canister's actual `inspect_message` on the ingress path. A refusal
/// surfaces as a `CanisterRejectedMessage` at submission (`CallFailure::Refused`): the update
/// method never executed. Nothing external is mocked.
#[test]
fn inspect_message_refuses_anonymous_and_unauthorized_callers_before_execution() {
    let mut w = quick_world();
    let (user, admin, anon) = (w.user, w.admin, Principal::anonymous());
    assert!(
        w.pic.get_controllers(w.bridge).contains(&anon),
        "precondition: PocketIC made the anonymous principal a controller, so the refusals below also show that a controller flag does not rescue an anonymous caller"
    );
    let hint = hex0x(&[0x11u8; 32]);
    let args = w.deposit_args(e8(2), &w.holder(0));
    // Differs from the unbound world's initial pauses (only deposits paused).
    let pause_all = Pauses { deposits: true, mints: true, releases: true, observer: true };

    let deposits_before = w.list_my_deposits(user, None, 50).len();
    let info_before = w.bridge_info();
    let audit_before = w.audit_log_all();

    // Anonymous ingress to every kind of update method is refused before execution.
    println!("refusal text: {}", w.expect_refused(anon, "bridge_to_base", (args.clone(),)));
    w.expect_refused(anon, "notify_base_bridge_out", (hint.clone(),));
    w.expect_refused(anon, "request_deposit_refund", (0u64,));
    w.expect_refused(anon, "get_base_addresses", ());
    w.expect_refused(anon, "admin_run_tick", ());
    w.expect_refused(anon, "admin_set_pauses", (pause_all.clone(),));
    w.expect_refused(anon, "set_admins", (vec![anon],));
    w.expect_refused(anon, "admin_set_rpc_services", (services(&[PROVIDER_A, PROVIDER_B, PROVIDER_C]), 2u8));

    // A signed-in caller who is not an admin cannot reach admin methods.
    w.expect_refused(user, "admin_run_tick", ());
    w.expect_refused(user, "admin_set_pauses", (pause_all.clone(),));
    w.expect_refused(user, "admin_set_gas", (info_before.gas.clone(),));
    // ...nor controller methods; and a bare admin who is not a controller cannot either.
    w.expect_refused(user, "set_admins", (vec![user],));
    w.expect_refused(admin, "set_admins", (vec![admin],));
    w.expect_refused(admin, "admin_set_rpc_services", (services(&[PROVIDER_A, PROVIDER_B, PROVIDER_C]), 2u8));
    // A query method called as an update has no ingress entry either.
    w.expect_refused(user, "get_bridge_info", ());

    // Nothing was created, changed or audited by any of those calls.
    assert_eq!(w.list_my_deposits(user, None, 50).len(), deposits_before);
    let info_after = w.bridge_info();
    assert_eq!(info_after.pauses, info_before.pauses, "no pause change got through");
    assert_eq!(info_after.gas, info_before.gas);
    assert_eq!(info_after.active_deposits, info_before.active_deposits);
    assert_eq!(info_after.observer.pending_hints.len(), info_before.observer.pending_hints.len());
    assert_eq!(info_after.rpc_service_count, info_before.rpc_service_count);
    assert_eq!(w.audit_log_all(), audit_before, "refused calls leave no audit entries");

    // The wallet standard stays open to anonymous callers: a consent message is still served.
    let consent = consent_for(&mut w, anon, &args).expect("anonymous consent call is accepted at ingress");
    match consent {
        Ok(ConsentInfo { consent_message: ConsentMessage::GenericDisplayMessage(text), .. }) => {
            let shown = eip55(&w.holder(0)[2..].to_ascii_lowercase());
            assert!(text.contains("Bridge icUSD to Base") && text.contains(&shown), "{text}")
        }
        other => panic!("expected a consent message, got {other:?}"),
    }

    // The same refused callers were not locked out of anything legitimate: an admin still acts.
    assert_eq!(w.set_pauses(pause_all.clone()), Ok(()));
    assert_eq!(w.bridge_info().pauses, pause_all);
    assert_eq!(w.set_pauses(info_before.pauses.clone()), Ok(()));
    assert_eq!(w.bridge_info().pauses, info_before.pauses);
}

/// REAL: the controller set is PocketIC's, the roles are the canister's. The ingress filter
/// lets a controller through to the admin methods, but `require_admin` is still the authority:
/// a controller who is not an admin is answered `Unauthorized` by the method itself (it was
/// accepted at ingress, so this is NOT a refusal), and a controller who is also an admin runs
/// the tick.
#[test]
fn a_controller_passes_the_ingress_filter_but_only_an_admin_runs_admin_methods() {
    let mut w = quick_world();
    let (admin, controller, user) = (w.admin, w.controller, w.user);
    assert!(w.pic.get_controllers(w.bridge).contains(&controller));
    let before = w.bridge_info().pauses;
    let pause_all = Pauses { deposits: true, mints: true, releases: true, observer: true };

    // Not an admin: accepted at ingress, refused by the method.
    let r: Result<BridgeResult<()>, CallFailure> = w.try_update(controller, "admin_run_tick", ());
    assert_eq!(r, Ok(Err(BridgeError::Unauthorized)), "accepted at ingress, refused by require_admin");
    let r: Result<BridgeResult<()>, CallFailure> = w.try_update(controller, "admin_set_pauses", (pause_all.clone(),));
    assert_eq!(r, Ok(Err(BridgeError::Unauthorized)));
    assert_eq!(w.bridge_info().pauses, before, "the pauses did not change");

    // The controller manages the admin list.
    let r: BridgeResult<()> = w.update(controller, "set_admins", (Vec::<Principal>::new(),));
    assert!(matches!(r, Err(BridgeError::InvalidArgument(_))), "{r:?}");
    let r: BridgeResult<()> = w.update(controller, "set_admins", (vec![Principal::anonymous()],));
    assert!(matches!(r, Err(BridgeError::InvalidArgument(_))), "{r:?}");
    let r: BridgeResult<()> = w.update(controller, "set_admins", (vec![admin, controller],));
    assert_eq!(r, Ok(()));
    let note = w.audit_log_all().pop().expect("audit entry").note;
    assert!(note.contains("admins set by controller") && note.contains(&controller.to_text()), "{note}");

    // Now a controller AND an admin: the tick runs. (The canister's own timer may be
    // mid-run, in which case it answers that a tick is already running; retry a few times.)
    let mut ran = false;
    for _ in 0..TICK_BUSY_RETRIES {
        let r: BridgeResult<()> = w.update(controller, "admin_run_tick", ());
        match r {
            Ok(()) => {
                ran = true;
                break;
            }
            Err(BridgeError::InvalidState(m)) if m.contains("already running") => w.advance_secs(TICK_ADVANCE_SECS),
            Err(e) => panic!("controller+admin admin_run_tick failed: {e:?}"),
        }
    }
    assert!(ran, "a controller that is also an admin can run the tick");
    // The old admin still works, a plain user still cannot reach admin methods.
    assert_eq!(w.set_pauses(before.clone()), Ok(()));
    w.expect_refused(user, "admin_run_tick", ());

    // The controller restores the admin list; it is an ordinary controller again.
    let r: BridgeResult<()> = w.update(controller, "set_admins", (vec![admin],));
    assert_eq!(r, Ok(()));
    let r: Result<BridgeResult<()>, CallFailure> = w.try_update(controller, "admin_run_tick", ());
    assert_eq!(r, Ok(Err(BridgeError::Unauthorized)), "no longer an admin");
    assert_eq!(w.set_pauses(before.clone()), Ok(()), "the original admin is intact");
}

// ---------------------------------------------------------------------------
// C. admin_set_rpc_services
// ---------------------------------------------------------------------------

/// REAL: the controller's call, the quorum chain id check against anvil, the stored set and
/// the audit entry. MOCKED (clearly bounded, in the middle part only): two of the proposed
/// providers answer `eth_chainId` with `0x1` through the fault hook, to prove a set that
/// serves the wrong network is refused and nothing is stored.
#[test]
fn admin_set_rpc_services_is_controller_only_records_the_controller_and_refuses_a_wrong_chain() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let (admin, controller) = (w.admin, w.controller);
    let abc = services(&[PROVIDER_A, PROVIDER_B, PROVIDER_C]);
    let de = services(&[PROVIDER_D, PROVIDER_E]);
    w.router.add_provider(PROVIDER_D);
    w.router.add_provider(PROVIDER_E);

    let info0 = w.bridge_info();
    assert_eq!((info0.rpc_service_count, info0.min_quorum), (3, 2));
    let audit0 = w.audit_log_all();

    // The admin (not a controller) is refused at ingress; nothing changes.
    w.expect_refused(admin, "admin_set_rpc_services", (abc.clone(), 2u8));
    let info = w.bridge_info();
    assert_eq!((info.rpc_service_count, info.min_quorum), (3, 2));
    assert_eq!(w.audit_log_all(), audit0);

    // The controller sets the very same provider set: accepted, and the audit entry names the
    // CONTROLLER (captured before the await), not the canister or the anonymous principal.
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (abc.clone(), 2u8));
    assert_eq!(r, Ok(()));
    let info = w.bridge_info();
    assert_eq!((info.rpc_service_count, info.min_quorum), (3, 2));
    let audit1 = w.audit_log_all();
    assert_eq!(audit1.len(), audit0.len() + 1, "exactly one audit entry");
    let note = audit1.last().unwrap().note.clone();
    assert!(note.contains("rpc services set by"), "{note}");
    assert!(note.contains(&controller.to_text()), "the entry names the controller: {note}");
    assert!(!note.contains(&w.bridge.to_text()) && !note.contains("2vxsx-fae"), "not the canister or anonymous: {note}");

    // A malformed proposal is refused before any outcall.
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (Vec::<RpcServiceConfig>::new(), 1u8));
    assert!(matches!(r, Err(BridgeError::InvalidArgument(_))), "{r:?}");
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (de.clone(), 3u8));
    assert!(matches!(r, Err(BridgeError::InvalidArgument(_))), "quorum above the provider count: {r:?}");

    // MOCKED: providers D and E claim to serve chain id 1 (Ethereum mainnet). The bridge is on
    // the anvil chain (31337), so the proposal is refused and nothing is stored.
    w.router.clear_log();
    w.router.set_fault(|provider, req| {
        if (provider == PROVIDER_D || provider == PROVIDER_E) && req["method"] == "eth_chainId" {
            FaultAction::ReplaceBody(json!({"jsonrpc": "2.0", "id": req["id"].clone(), "result": "0x1"}).to_string())
        } else {
            FaultAction::Pass
        }
    });
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (de.clone(), 2u8));
    w.router.clear_fault();
    match r {
        Err(BridgeError::Rpc(m)) => assert!(m.contains("31337") && m.contains("chain id"), "{m}"),
        other => panic!("a wrong-chain provider set must be refused, got {other:?}"),
    }
    assert!(
        w.router.count(PROVIDER_D, "eth_chainId") >= 1 && w.router.count(PROVIDER_E, "eth_chainId") >= 1,
        "the PROPOSED providers were asked for the chain id"
    );
    let info = w.bridge_info();
    assert_eq!((info.rpc_service_count, info.min_quorum), (3, 2), "nothing stored after a refusal");
    assert_eq!(w.audit_log_all().len(), audit1.len(), "no audit entry for a refused proposal");

    // REAL again: with honest providers the different set is accepted and stored (count 2),
    // and the controller can restore the original set.
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (de.clone(), 2u8));
    assert_eq!(r, Ok(()));
    let info = w.bridge_info();
    assert_eq!((info.rpc_service_count, info.min_quorum), (2, 2));
    let r: BridgeResult<()> = w.update(controller, "admin_set_rpc_services", (abc.clone(), 2u8));
    assert_eq!(r, Ok(()));
    let info = w.bridge_info();
    assert_eq!((info.rpc_service_count, info.min_quorum), (3, 2));
    let notes: Vec<String> = w.audit_log_all().iter().skip(audit1.len()).map(|e| e.note.clone()).collect();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes.iter().all(|n| n.contains(&controller.to_text())), "{notes:?}");
    // The restored providers work.
    w.tick();
    assert!(w.bridge_info().halted.is_none());
}

// ---------------------------------------------------------------------------
// D. EIP-55
// ---------------------------------------------------------------------------

/// REAL: validation by the canister, then a real mint to the checksummed recipient on anvil.
/// The checksum is computed here with an independent implementation checked against the EIP-55
/// test vector.
#[test]
fn eip55_mixed_case_recipients_must_match_their_checksum() {
    let mut w = BridgeWorld::setup(SetupOptions::default());
    let user = w.user;

    // The first EIP-55 test vector, and our independent checksum implementation agrees.
    let vector = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
    let lower40 = vector[2..].to_ascii_lowercase();
    assert_eq!(eip55(&lower40), vector, "the test's own EIP-55 matches the published vector");
    let lower = format!("0x{lower40}");
    let upper = format!("0x{}", lower40.to_ascii_uppercase());

    // Allowance for three accepted deposits, so a refusal can only be about the recipient.
    let amount = e8(2);
    w.approve(user, w.bridge, 3 * (amount + LEDGER_FEE) as u128);
    let user_before = w.user_balance(user);

    // Flip the case of every letter, one at a time: each is mixed case with a bad checksum.
    let letters: Vec<usize> = lower40.char_indices().filter(|(_, c)| c.is_ascii_alphabetic()).map(|(i, _)| i).collect();
    assert_eq!(letters.len(), 18, "the vector has 18 letters");
    let mut refused = 0;
    let mut first_bad = None;
    for i in letters {
        let mut body: Vec<char> = vector[2..].chars().collect();
        body[i] = if body[i].is_ascii_uppercase() { body[i].to_ascii_lowercase() } else { body[i].to_ascii_uppercase() };
        let bad: String = format!("0x{}", body.iter().collect::<String>());
        let has_lower = bad[2..].chars().any(|c| ('a'..='f').contains(&c));
        let has_upper = bad[2..].chars().any(|c| ('A'..='F').contains(&c));
        assert!(has_lower && has_upper && bad != vector, "a genuinely mixed-case mistake: {bad}");
        let args = w.deposit_args(amount, &bad);
        match w.bridge_to_base(user, args) {
            Err(BridgeError::InvalidRecipient(m)) => assert!(m.contains("checksum"), "{bad}: {m}"),
            other => panic!("{bad} (one case flip of a valid checksum) must be refused, got {other:?}"),
        }
        first_bad.get_or_insert(bad);
        refused += 1;
    }
    println!("EIP-55: {refused} single-letter case flips were all refused");

    // No record, no capacity, no funds moved by any of the refusals.
    assert!(w.list_my_deposits(user, None, 50).is_empty(), "no deposit record is created");
    let info = w.bridge_info();
    assert_eq!((info.active_deposits, info.mint_window_used_e8s), (0, 0));
    assert_eq!(w.user_balance(user), user_before);
    assert_eq!(w.escrow_balance(), 0);
    assert_eq!(w.supply_audit().buckets(), [0; 7]);

    // A wallet is not shown a consent message for the mistyped address either, but is for the
    // correct one (MOCK-free: anonymous consent call against the live canister).
    let bad_args = w.deposit_args(amount, &first_bad.unwrap());
    match consent_for(&mut w, Principal::anonymous(), &bad_args).expect("accepted at ingress") {
        Err(Icrc21Error::ConsentMessageUnavailable(_)) => {}
        other => panic!("no consent for a bad checksum, got {other:?}"),
    }
    let good_args = w.deposit_args(amount, vector);
    match consent_for(&mut w, Principal::anonymous(), &good_args).expect("accepted at ingress") {
        Ok(ConsentInfo { consent_message: ConsentMessage::GenericDisplayMessage(t), .. }) => {
            assert!(t.contains(vector), "the consent shows the EIP-55 form of the address: {t}")
        }
        other => panic!("expected a consent message for the correct checksum, got {other:?}"),
    }

    // The correctly checksummed address, the all-lowercase address and the all-uppercase address
    // are all accepted, and all three name the same account.
    let mut ids = vec![];
    for form in [vector.to_string(), lower.clone(), upper.clone()] {
        let args = w.deposit_args(amount, &form);
        let d = w.bridge_to_base(user, args).unwrap_or_else(|e| panic!("{form} must be accepted: {e:?}"));
        assert_eq!(d.status, DepositStatus::Escrowed, "{form}: {d:?}");
        assert_eq!(d.base_recipient, lower, "stored in canonical lowercase form");
        ids.push(d.id);
    }
    for id in &ids {
        w.wait_mint_included(*id);
    }
    w.mine_finalize();
    for id in &ids {
        w.wait_delivered(*id);
    }
    assert_eq!(w.base_balance(&lower), 3 * amount as u128, "all three forms credit the same account (anvil)");
    assert_eq!(w.base_total_supply(), 3 * amount as u128);
}

// ---------------------------------------------------------------------------
// E. Ingress size caps (round 3)
// ---------------------------------------------------------------------------

/// The canister's caps (`lib.rs` MAX_OPEN_INGRESS_BYTES / MAX_ADMIN_INGRESS_BYTES).
const MAX_OPEN: usize = 4 * 1024;
const MAX_ADMIN: usize = 16 * 1024;

/// A consent request for `bridge_to_base` whose Candid encoding is exactly `size` bytes.
fn consent_bytes_of_size(size: usize) -> Vec<u8> {
    let encode = |pad: usize| {
        let mut req = consent_request(&BridgeToBaseArgs {
            amount_e8s: 100_000_000,
            base_recipient: "0x00000000000000000000000000000000000000aa".to_string(),
            from_subaccount: None,
            client_request_id: serde_bytes::ByteBuf::from(vec![1u8; 16]),
            expected_deposit_fee_e8s: 0,
        });
        req.arg.resize(req.arg.len() + pad, 0);
        candid::encode_one(&req).expect("encode consent request")
    };
    let base = encode(0).len();
    assert!(base < size, "a minimal consent request is {base} bytes");
    let mut pad = size - base;
    // The arg length prefix is LEB128, so a longer arg can need one more prefix byte.
    loop {
        let bytes = encode(pad);
        match bytes.len().cmp(&size) {
            std::cmp::Ordering::Equal => return bytes,
            std::cmp::Ordering::Greater => pad -= bytes.len() - size,
            std::cmp::Ordering::Less => pad += size - bytes.len(),
        }
    }
}

/// REAL: PocketIC ingress through the canister's own `inspect_message`. Shows the caps are
/// enforced at the exact boundary before execution (so an oversized message costs the bridge
/// no ingress fee), for anonymous wallet-standard calls, user methods and admin methods, and
/// that ordinary arguments are far below them.
#[test]
fn oversized_ingress_is_refused_before_execution_at_the_exact_boundary() {
    let mut w = quick_world();
    let (user, controller, anon) = (w.user, w.controller, Principal::anonymous());
    let audit_before = w.audit_log_all();

    // Anonymous ICRC-21 consent: exactly at the cap is accepted (and answered), one byte more is refused.
    let at_cap = consent_bytes_of_size(MAX_OPEN);
    assert_eq!(at_cap.len(), MAX_OPEN);
    match w.try_update_raw(anon, "icrc21_canister_call_consent_message", at_cap) {
        Ok(_) => {}
        Err(e) => panic!("a {MAX_OPEN}-byte consent request must be accepted at ingress: {e:?}"),
    }
    let over = consent_bytes_of_size(MAX_OPEN + 1);
    assert!(matches!(w.try_update_raw(anon, "icrc21_canister_call_consent_message", over), Err(CallFailure::Refused(_))));
    let huge = consent_bytes_of_size(1024 * 1024);
    assert!(matches!(w.try_update_raw(anon, "icrc21_canister_call_consent_message", huge), Err(CallFailure::Refused(_))));

    // A signed-in user: a padded bridge_to_base is refused before execution; an ordinary one is
    // accepted at ingress (it then fails in execution because deposits are paused, which is fine).
    let mut big = w.deposit_args(e8(2), &w.holder(0));
    big.client_request_id = serde_bytes::ByteBuf::from(vec![7u8; MAX_OPEN]);
    w.expect_refused(user, "bridge_to_base", (big,));
    let normal = w.deposit_args(e8(2), &w.holder(0));
    let normal_len = candid::encode_one(&normal).unwrap().len();
    assert!(normal_len * 8 < MAX_OPEN, "an ordinary bridge_to_base argument is {normal_len} bytes");
    let accepted: Result<BridgeResult<DepositView>, CallFailure> = w.try_update(user, "bridge_to_base", (normal,));
    assert!(!matches!(accepted, Err(CallFailure::Refused(_))), "an ordinary request passes ingress: {accepted:?}");

    // A controller: an oversized provider list is refused before execution; a large but
    // in-cap one is accepted at ingress and then judged by the method itself.
    let long = |n: usize| vec![RpcServiceConfig::Custom { url: format!("https://{}.test/", "a".repeat(n)) }];
    w.expect_refused(controller, "admin_set_rpc_services", (long(MAX_ADMIN), 1u8));
    let in_cap: Result<BridgeResult<()>, CallFailure> = w.try_update(controller, "admin_set_rpc_services", (long(MAX_ADMIN / 2), 1u8));
    // The method itself refuses the 8 KiB URL (over its 256-byte URL limit) without any outcall.
    assert!(matches!(in_cap, Ok(Err(BridgeError::InvalidArgument(_)))), "an in-cap admin argument passes ingress: {in_cap:?}");

    // Nothing that was refused ran, and nothing accepted changed state.
    assert_eq!(w.audit_log_all(), audit_before, "no audit entries");
    assert!(w.list_my_deposits(user, None, 50).is_empty(), "no deposit records");
}
