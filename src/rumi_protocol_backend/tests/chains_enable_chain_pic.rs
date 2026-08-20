//! PocketIC integration test for `enable_chain`, the recovery half of the
//! `disable_chain` emergency risk stop.
//!
//! `disable_chain` blocks risk-increasing operations (new opens, additional
//! borrows), hands the chain's native pair back to manual pricing, AND drops
//! the chain from the observer/settlement worker fan-outs, so an exit enqueued
//! while it is Disabled is accepted but never broadcast. Before `enable_chain`
//! existed that was TERMINAL for a live chain: `register_chain` refuses a
//! present `chain_id` and `delete_chain` refuses a chain with any supply or any
//! vault, so a queued exit could neither complete nor be cleared. An operator
//! facing a transient incident therefore had to choose between leaving the risk
//! gate open and freezing the chain forever. This suite proves the transition
//! is real, gated, state-preserving and repeatable, through the actual Candid
//! boundary.
//!
//! Scenarios:
//!   1. `enable_chain_is_developer_gated`
//!   2. `enable_chain_rejects_unknown_and_already_registered_chains`
//!   3. `disable_then_enable_preserves_every_observable_chain_state`
//!   4. `a_signed_open_against_a_disabled_chain_does_not_consume_the_nonce`
//!   5. `risk_operations_are_restored_only_after_enable`
//!
//! Scenarios 4 and 5 drive the real EIP-712 self-serve `open_chain_vault_evm`
//! endpoint. The pre-`await` chain-status check runs BEFORE the tECDSA custody
//! derive, so the disabled-chain rejection is observable even where PocketIC
//! cannot provision a threshold key; the post-enable half degrades to an
//! ECDSA-gated subset that still proves the nonce survived (the distinguishing
//! evidence is the ABSENCE of a "bad nonce" rejection, not the open
//! succeeding).

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal};
use pocket_ic::{PocketIc, WasmResult};

use rumi_protocol_backend::chains::evm::eip712::{
    domain_separator, intent_digest, intent_struct_hash, IntentAction, VaultIntent,
};

// ─── Wire types (mirror lib.rs / chains/config.rs / chains/liquidation_config.rs) ──

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct ChainId(u32);

#[derive(CandidType, Deserialize, Clone, Debug)]
enum GasStrategy {
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
    EvmLegacy {
        gas_price_gwei_ceiling: u64,
    },
    SolanaPriorityFee {
        lamports_per_cu_ceiling: u64,
    },
    NotApplicable,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct RegisterChainArg {
    chain_id: ChainId,
    display_name: String,
    rpc_endpoints: Vec<String>,
    finality_depth: u32,
    gas_strategy: GasStrategy,
    chain_native_decimals: u8,
    min_quorum_providers: Option<u32>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct UpdateChainConfigArg {
    display_name: Option<String>,
    rpc_endpoints: Option<Vec<String>>,
    finality_depth: Option<u32>,
    gas_strategy: Option<GasStrategy>,
    min_quorum_providers: Option<Option<u32>>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum DexKind {
    UniswapV2,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
struct ChainLiquidationConfigV1 {
    dex: DexKind,
    router: String,
    factory: String,
    pair: String,
    collateral_token: String,
    settle_stable_token: String,
    slippage_cap_bps: u16,
    restore_target_cr_e4: u64,
    enabled: bool,
    max_swap_value_e8s: u128,
    max_price_age_ns: u64,
    max_dex_oracle_divergence_bps: u32,
    fee_bps: u16,
    settle_stable_decimals: u8,
    deadline_secs: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
struct ManualPriceInfo {
    price_e8: u64,
    set_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainBadDebtCircuitStatus {
    chain_id: ChainId,
    registered: bool,
    bad_debt_e8s: u128,
    threshold_e8s: Option<u128>,
    tripped: bool,
    tripped_at_ns: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    GenericError(String),
    TemporarilyUnavailable(String),
    ChainAdmin(String),
    AmountTooLow { minimum_amount: u64 },
    CallerNotOwner,
    AlreadyProcessing,
    NotLowestCR,
    SupplyInvariantHalted,
    AnonymousCallerNotAllowed,
    EvmAuth(String),
}

const CFX_MAINNET: ChainId = ChainId(1030);
const EVM_CONTRACT: &str = "0x00000000000000000000000000000000cf1c0de5";
const E18: u128 = 1_000_000_000_000_000_000;
const E8: u128 = 100_000_000;

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn developer() -> Principal {
    Principal::from_slice(&[31; 29])
}

fn outsider() -> Principal {
    Principal::from_slice(&[32; 29])
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let cid = pic.create_canister();
    pic.add_cycles(cid, 100_000_000_000_000);
    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: developer(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(cid, backend_wasm(), encode_args((init,)).expect("encode init"), None);
    for _ in 0..5 {
        pic.tick();
    }
    (pic, cid)
}

fn call_unit(
    pic: &PocketIc,
    cid: Principal,
    sender: Principal,
    method: &str,
    args: Vec<u8>,
) -> Result<(), ProtocolError> {
    let reply = pic
        .update_call(cid, sender, method, args)
        .unwrap_or_else(|e| panic!("{method} call: {e:?}"));
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<(), ProtocolError>).unwrap_or_else(|e| panic!("decode {method}: {e}"))
        }
        WasmResult::Reject(msg) => panic!("{method} rejected at transport: {msg}"),
    }
}

fn dev_unit(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) {
    call_unit(pic, cid, developer(), method, args)
        .unwrap_or_else(|e| panic!("{method} must succeed: {e:?}"));
}

fn enable_chain(pic: &PocketIc, cid: Principal, sender: Principal) -> Result<(), ProtocolError> {
    call_unit(pic, cid, sender, "enable_chain", encode_one(CFX_MAINNET).unwrap())
}

fn disable_chain(pic: &PocketIc, cid: Principal) {
    dev_unit(pic, cid, "disable_chain", encode_one(CFX_MAINNET).unwrap());
}

fn register_chain(pic: &PocketIc, cid: Principal) {
    let arg = RegisterChainArg {
        chain_id: CFX_MAINNET,
        display_name: "Conflux mainnet".to_string(),
        rpc_endpoints: vec!["https://evm.confluxrpc.com".to_string()],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    };
    dev_unit(pic, cid, "register_chain", encode_args((arg,)).unwrap());
}

fn set_price(pic: &PocketIc, cid: Principal, price_e8: u64) {
    dev_unit(
        pic,
        cid,
        "set_manual_collateral_price",
        encode_args((CFX_MAINNET, "CFX".to_string(), price_e8)).unwrap(),
    );
}

fn get_price(pic: &PocketIc, cid: Principal) -> Option<ManualPriceInfo> {
    let args = encode_args((CFX_MAINNET, "CFX".to_string())).unwrap();
    match pic
        .query_call(cid, Principal::anonymous(), "get_manual_collateral_price", args)
        .expect("get_manual_collateral_price")
    {
        WasmResult::Reply(b) => Decode!(&b, Option<ManualPriceInfo>).expect("decode price"),
        WasmResult::Reject(msg) => panic!("get_manual_collateral_price rejected: {msg}"),
    }
}

fn get_liq_config(pic: &PocketIc, cid: Principal) -> Option<ChainLiquidationConfigV1> {
    match pic
        .query_call(
            cid,
            Principal::anonymous(),
            "get_chain_liquidation_config",
            encode_one(CFX_MAINNET).unwrap(),
        )
        .expect("get_chain_liquidation_config")
    {
        WasmResult::Reply(b) => {
            Decode!(&b, Option<ChainLiquidationConfigV1>).expect("decode liq config")
        }
        WasmResult::Reject(msg) => panic!("get_chain_liquidation_config rejected: {msg}"),
    }
}

fn get_circuit_status(pic: &PocketIc, cid: Principal) -> ChainBadDebtCircuitStatus {
    match pic
        .query_call(
            cid,
            Principal::anonymous(),
            "get_chain_bad_debt_circuit_status",
            encode_one(CFX_MAINNET).unwrap(),
        )
        .expect("get_chain_bad_debt_circuit_status")
    {
        WasmResult::Reply(b) => Decode!(&b, ChainBadDebtCircuitStatus).expect("decode status"),
        WasmResult::Reject(msg) => panic!("get_chain_bad_debt_circuit_status rejected: {msg}"),
    }
}

// ─── EIP-712 signing (same fixed scalar=1 key the happy-path suite uses) ─────

fn evm_signer() -> (k256::ecdsa::SigningKey, String) {
    use k256::ecdsa::{SigningKey, VerifyingKey};
    let mut b = [0u8; 32];
    b[31] = 1;
    let sk = SigningKey::from_bytes(&b.into()).unwrap();
    let pk = VerifyingKey::from(&sk).to_encoded_point(false).as_bytes().to_vec();
    let addr = rumi_protocol_backend::chains::evm::tecdsa::evm_address_from_pubkey(&pk).unwrap();
    (sk, addr)
}

fn open_intent(owner: &str, collateral_wei: u128, debt_e8s: u128, nonce: u64) -> VaultIntent {
    VaultIntent {
        action: IntentAction::Open.as_u8(),
        chain_id: CFX_MAINNET.0 as u64,
        owner: owner.to_string(),
        vault_id: 0,
        collateral_wei,
        debt_e8s,
        recipient: owner.to_string(),
        nonce,
        deadline_secs: 9_999_999_999,
    }
}

fn sign(sk: &k256::ecdsa::SigningKey, intent: &VaultIntent) -> Vec<u8> {
    use k256::ecdsa::{RecoveryId, Signature};
    let digest = intent_digest(
        &domain_separator(intent.chain_id, EVM_CONTRACT).unwrap(),
        &intent_struct_hash(intent).unwrap(),
    );
    let (sig, rid): (Signature, RecoveryId) = sk.sign_prehash_recoverable(&digest).unwrap();
    let mut out = sig.to_bytes().to_vec();
    out.push(27 + u8::from(rid));
    out
}

/// Submit a signed OPEN as the ANONYMOUS caller (the `_evm` endpoints are
/// signature-authenticated, not caller-authenticated).
fn open_evm(
    pic: &PocketIc,
    cid: Principal,
    intent: &VaultIntent,
    sig: &[u8],
) -> Result<u64, ProtocolError> {
    match pic
        .update_call(
            cid,
            Principal::anonymous(),
            "open_chain_vault_evm",
            Encode!(intent, &sig.to_vec()).unwrap(),
        )
        .expect("open_chain_vault_evm call")
    {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>).expect("decode open_evm"),
        WasmResult::Reject(msg) => panic!("open_chain_vault_evm rejected at transport: {msg}"),
    }
}

/// Full launch-shaped setup: registered, contract bound (the actual public-open
/// gate), and a fresh native price. No liquidation config row, so the pair
/// stays manually priceable and this suite needs no XRC mock.
fn configure_launch_ready(pic: &PocketIc, cid: Principal) {
    register_chain(pic, cid);
    dev_unit(
        pic,
        cid,
        "set_chain_contract",
        encode_args((CFX_MAINNET, EVM_CONTRACT.to_string())).unwrap(),
    );
    set_price(pic, cid, 15_000_000); // $0.15 / CFX
}

#[test]
fn enable_chain_is_developer_gated() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);
    disable_chain(&pic, cid);

    match enable_chain(&pic, cid, outsider()).expect_err("a non-developer must be refused") {
        ProtocolError::ChainAdmin(msg) => assert!(msg.contains("developer"), "msg={msg}"),
        other => panic!("expected ChainAdmin(not developer), got {other:?}"),
    }
    // The refusal is inert: the chain is still Disabled, so a developer enable
    // is still the transition that matters.
    enable_chain(&pic, cid, developer()).expect("the developer may enable");
}

#[test]
fn enable_chain_rejects_unknown_and_already_registered_chains() {
    let (pic, cid) = boot();

    // Unknown chain: enable must never be able to CREATE a chain.
    match enable_chain(&pic, cid, developer()).expect_err("unknown chain must be refused") {
        ProtocolError::ChainAdmin(msg) => assert!(
            msg.contains("ChainNotRegistered"),
            "expected ChainNotRegistered, got {msg}"
        ),
        other => panic!("expected ChainAdmin, got {other:?}"),
    }
    assert!(
        !get_circuit_status(&pic, cid).registered,
        "a refused enable must not have created the chain"
    );

    // Already Registered: a redundant call is a VISIBLE error, so an operator
    // re-running the command cannot read "ok" as confirmation that a real
    // recovery just happened.
    register_chain(&pic, cid);
    match enable_chain(&pic, cid, developer()).expect_err("already-Registered must be refused") {
        ProtocolError::ChainAdmin(msg) => assert!(
            msg.contains("already Registered"),
            "expected the already-Registered message, got {msg}"
        ),
        other => panic!("expected ChainAdmin, got {other:?}"),
    }
}

#[test]
fn disable_then_enable_preserves_every_observable_chain_state() {
    let (pic, cid) = boot();
    configure_launch_ready(&pic, cid);
    dev_unit(
        &pic,
        cid,
        "set_chain_bad_debt_circuit_threshold",
        encode_args((CFX_MAINNET, Some(1_234u128))).unwrap(),
    );
    // A config edit, so the round trip has a non-default field to preserve.
    dev_unit(
        &pic,
        cid,
        "set_chain_config",
        encode_args((
            CFX_MAINNET,
            UpdateChainConfigArg {
                display_name: Some("Conflux eSpace (prod)".to_string()),
                rpc_endpoints: None,
                finality_depth: Some(500),
                gas_strategy: None,
                min_quorum_providers: Some(Some(3)),
            },
        ))
        .unwrap(),
    );

    let price_before = get_price(&pic, cid).expect("price set");
    let status_before = get_circuit_status(&pic, cid);

    disable_chain(&pic, cid);
    enable_chain(&pic, cid, developer()).expect("enable");

    assert_eq!(
        get_price(&pic, cid),
        Some(price_before),
        "the manual price AND its freshness stamp must survive the round trip"
    );
    let status_after = get_circuit_status(&pic, cid);
    assert!(status_after.registered, "the chain config entry survives");
    assert_eq!(
        status_after.threshold_e8s, status_before.threshold_e8s,
        "the bad-debt circuit threshold survives"
    );
    assert_eq!(status_after.tripped, status_before.tripped);
    assert_eq!(
        get_liq_config(&pic, cid),
        None,
        "no liquidation row was staged, and none appeared"
    );
    // The contract binding survives too: an open now fails on something OTHER
    // than "no contract set" (that error is raised before the signature is even
    // checked, so its absence is the evidence the binding is still there).
    let (sk, owner) = evm_signer();
    let intent = open_intent(&owner, 1_600 * E18, 100 * E8, 0);
    let sig = sign(&sk, &intent);
    if let Err(ProtocolError::EvmAuth(msg)) = open_evm(&pic, cid, &intent, &sig) {
        assert!(
            !msg.contains("no contract set"),
            "the bound IcUSD contract must survive disable/enable, got: {msg}"
        );
    }
}

#[test]
fn a_signed_open_against_a_disabled_chain_does_not_consume_the_nonce() {
    let (pic, cid) = boot();
    configure_launch_ready(&pic, cid);
    let (sk, owner) = evm_signer();

    disable_chain(&pic, cid);

    // The pre-`await` status check runs before the tECDSA derive, so this
    // rejection is observable regardless of whether PocketIC can provision a
    // threshold key.
    let intent = open_intent(&owner, 1_600 * E18, 100 * E8, 0);
    let sig = sign(&sk, &intent);
    match open_evm(&pic, cid, &intent, &sig).expect_err("a disabled chain must refuse an open") {
        ProtocolError::EvmAuth(msg) => assert!(
            msg.contains("ChainDisabled"),
            "expected the ChainDisabled rejection, got: {msg}"
        ),
        other => panic!("expected EvmAuth(ChainDisabled), got {other:?}"),
    }

    enable_chain(&pic, cid, developer()).expect("enable");

    // THE POINT: re-submitting the SAME intent, at the SAME nonce 0, must not
    // hit a nonce error. A consumed nonce would reject with "bad nonce"; the
    // absence of that is the evidence, whether the open then succeeds (ECDSA
    // available) or fails at the derive (ECDSA unavailable in this PocketIC).
    match open_evm(&pic, cid, &intent, &sig) {
        Ok(vault_id) => assert!(vault_id > 0, "reused nonce 0 opened vault {vault_id}"),
        Err(ProtocolError::EvmAuth(msg)) => {
            assert!(
                !msg.contains("nonce"),
                "the refused open must NOT have spent the caller's nonce, got: {msg}"
            );
            assert!(
                msg.contains("derive"),
                "the only acceptable remaining failure here is the tECDSA derive, got: {msg}"
            );
            eprintln!("[enable_chain] ECDSA unavailable; nonce-preservation proven by the absence of a nonce error");
        }
        other => panic!("unexpected open result: {other:?}"),
    }
}

#[test]
fn risk_operations_are_restored_only_after_enable() {
    let (pic, cid) = boot();
    configure_launch_ready(&pic, cid);
    let (sk, owner) = evm_signer();

    // Nonce 0 while Registered: reaches the price/derive stage (NOT the status
    // gate). Whatever it returns, it is not a ChainDisabled rejection.
    let before = open_intent(&owner, 1_600 * E18, 100 * E8, 0);
    let before_sig = sign(&sk, &before);
    if let Err(ProtocolError::EvmAuth(msg)) = open_evm(&pic, cid, &before, &before_sig) {
        assert!(
            !msg.contains("ChainDisabled"),
            "a Registered chain must not raise ChainDisabled, got: {msg}"
        );
    }

    disable_chain(&pic, cid);

    // While Disabled, every nonce is refused at the status gate.
    for nonce in 0..3u64 {
        let intent = open_intent(&owner, 1_600 * E18, 100 * E8, nonce);
        let sig = sign(&sk, &intent);
        match open_evm(&pic, cid, &intent, &sig)
            .expect_err("a disabled chain refuses every open, at any nonce")
        {
            ProtocolError::EvmAuth(msg) => assert!(
                msg.contains("ChainDisabled"),
                "nonce {nonce}: expected ChainDisabled, got {msg}"
            ),
            other => panic!("nonce {nonce}: expected EvmAuth, got {other:?}"),
        }
    }

    // Enable restores the gate. The price prerequisite is independent and still
    // has to hold: this chain has no liquidation config row, so no staleness
    // ceiling applies and the price set at configuration time is still valid.
    enable_chain(&pic, cid, developer()).expect("enable");
    assert!(get_price(&pic, cid).is_some(), "the price prerequisite is in place");

    let after = open_intent(&owner, 1_600 * E18, 100 * E8, 0);
    let after_sig = sign(&sk, &after);
    if let Err(ProtocolError::EvmAuth(msg)) = open_evm(&pic, cid, &after, &after_sig) {
        assert!(
            !msg.contains("ChainDisabled"),
            "the status gate must be reopened after enable, got: {msg}"
        );
    }

    // And the control is reversible again, not one-shot.
    disable_chain(&pic, cid);
    let again = open_intent(&owner, 1_600 * E18, 100 * E8, 1);
    let again_sig = sign(&sk, &again);
    match open_evm(&pic, cid, &again, &again_sig).expect_err("disabled again") {
        ProtocolError::EvmAuth(msg) => assert!(msg.contains("ChainDisabled"), "msg={msg}"),
        other => panic!("expected EvmAuth(ChainDisabled), got {other:?}"),
    }
}
