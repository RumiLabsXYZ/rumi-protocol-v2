//! Solana durable-nonce bootstrap blockhash-override proof (PocketIC).
//!
//! Proves the playbook #4 escape hatch end to end: `getLatestBlockhash` returns a
//! value that changes EVERY SLOT, so the DFINITY sol-rpc canister's multi-provider
//! consensus chronically returns `#Inconsistent`, and the canister-side auto-fetch
//! (Equality consensus) rejects it. On real devnet/mainnet the no-arg bootstrap
//! therefore RELIABLY FAILS; the operator must supply a fresh finalized blockhash
//! that is fed straight into the create-nonce tx, bypassing consensus.
//!
//! The mock here is taught to model the per-slot divergence the real cluster shows
//! (which a `Consistent(Ok)` mock cannot): `set_latest_blockhash_inconsistent(true)`
//! makes its `getLatestBlockhash` return `MultiRequestResult::Inconsistent(..)`.
//! With the nonce account NOT yet initialized (`set_nonce_exists(false)`, so the
//! idempotency check falls through to the create path), the test asserts:
//!   1. `solana_bootstrap_nonce(None)`              -> Err  (auto-fetch hits the
//!                                                     chronic `#Inconsistent`;
//!                                                     proves the bug on the
//!                                                     consensus path).
//!   2. `solana_bootstrap_nonce(Some(valid b58))`   -> Ok   (override is fed in,
//!                                                     bypassing the broken fetch;
//!                                                     proves the #4 escape hatch).
//!   3. `solana_bootstrap_nonce(Some(not-base58))`  -> Err  (boundary validation of
//!                                                     the operator-supplied value).
//!
//! ## tEd25519-in-PocketIC: full vs gated
//!
//! The bootstrap derives the settlement + nonce addresses and multi-signs via the
//! management-canister threshold-Schnorr Ed25519 API (`test_key_1`). We boot with
//! `.with_ii_subnet().with_application_subnet()` and probe via
//! `solana_settlement_address`: Ok => the FULL block runs (all three asserts);
//! Err/Reject => `test_key_1` is unprovisioned, so we log and skip the
//! signing-dependent block (the same auto-degrade split as solana_m2_pic.rs /
//! solana_m2_sign_pic.rs). The invalid-blockhash assert reaches the bootstrap
//! (which derives addresses) too, so the whole FULL block degrades together.
//!
//! Notes:
//! - `inspect_message` rejects ALL anonymous update callers at ingress, so the
//!   dev-gate is exercised with a non-anonymous, non-developer principal.
//! - The error arm is decoded as `candid::Reserved`: the backend's rich
//!   `ProtocolError` does not subtype-decode into a minimal local mirror, and we
//!   only need Ok vs Err here.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Principal, Reserved};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

// ─── Locally-mirrored backend types (shapes mirror src/.../*.rs exactly) ─────

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
}

// ─── Constants ───────────────────────────────────────────────────────────────

/// Solana internal chain key (SLIP-44 coin type); must match
/// `chains::solana::config::SOLANA_CHAIN_ID`.
const SOLANA_CHAIN_ID: u32 = 501;

/// A valid 32-byte base58 value used as the operator-supplied fresh finalized
/// blockhash for the override. It only has to base58-decode to 32 bytes (the same
/// shape a Solana blockhash has); the mock's `sendTransaction` accepts the
/// resulting create-nonce tx. (Distinct, recognizable, and not the all-zeros
/// 32-`1`s value, so it reads like a real blockhash.)
const OVERRIDE_BLOCKHASH_B58: &str = "GfVcyD4kkTrj4bKc7WA9sZCin9JDbdT4Zibj1zzGEYpc";

/// A clearly-invalid blockhash: `!` is outside the base58 alphabet, so
/// `decode_solana_address` rejects it before it could be fed into a tx.
const INVALID_BLOCKHASH: &str = "not-valid-base58-!!!";

// ─── PocketIC call helpers ───────────────────────────────────────────────────

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn mock_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/sol_rpc_mock.wasm").to_vec()
}

/// The developer principal the backend is init'd with. Every admin/gated call is
/// made as this principal (anonymous update callers are rejected by
/// `inspect_message`).
fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

/// A non-anonymous, non-developer principal: passes `inspect_message` ingress
/// inspection, then hits the in-endpoint developer gate.
fn non_dev() -> Principal {
    Principal::from_slice(&[7, 7, 7, 7, 7])
}

/// Update call as the developer principal; returns the raw reply blob.
fn update_dev(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, dev(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

/// Update call as an arbitrary caller (the mock's ungated test-control endpoints).
fn update_any(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, Principal::anonymous(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

/// Decode a `Result<(), ProtocolError>` reply, treating the error arm as
/// `Reserved` (the backend's rich `ProtocolError` does not subtype-decode into a
/// minimal local enum; mirrors solana_m2_pic.rs). A `Reject` is surfaced as an
/// `Err` so callers that only care about Ok-vs-not still see the failure.
fn decode_unit_result(reply: WasmResult, method: &str) -> Result<(), String> {
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), Reserved>)
            .unwrap_or_else(|e| panic!("decode {} Result: {}", method, e))
            .map_err(|_| format!("{method} returned Err")),
        WasmResult::Reject(msg) => Err(format!("{method} rejected: {msg}")),
    }
}

/// Call `solana_bootstrap_nonce(opt text)` as the developer and return Ok/Err
/// (the error detail is irrelevant; both Err and Reject collapse to Err).
fn bootstrap(pic: &PocketIc, backend: Principal, blockhash: Option<String>) -> Result<(), String> {
    decode_unit_result(
        update_dev(
            pic,
            backend,
            "solana_bootstrap_nonce",
            Encode!(&blockhash).unwrap(),
        ),
        "solana_bootstrap_nonce",
    )
}

// ─── Boot: II subnet (for tEd25519) + application subnet (backend + mock) ─────

fn boot() -> (PocketIc, Principal, Principal) {
    // An II subnet is required for PocketIC to provision threshold-Schnorr keys;
    // the canisters live on the application subnet and route Schnorr requests to
    // the II subnet. If this build cannot provision `test_key_1`, the Ed25519
    // probe below degrades the test to its gated subset.
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);
    let mock_id = pic.create_canister();
    pic.add_cycles(mock_id, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: dev(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    // Mock takes no init arg.
    pic.install_canister(mock_id, mock_wasm(), Encode!().expect("encode mock init"), None);

    for _ in 0..5 {
        pic.tick();
    }

    (pic, backend_id, mock_id)
}

/// Lightweight base58 plausibility check (avoids adding bs58 as a dev-dependency),
/// mirroring solana_m2_pic.rs: Solana addresses are 32-44 base58 chars.
fn is_plausible_solana_address(s: &str) -> bool {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    s.len() >= 32 && s.len() <= 44 && s.chars().all(|c| B58.contains(c))
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn solana_bootstrap_blockhash_override_beats_inconsistent_consensus() {
    let (pic, backend, mock) = boot();

    // The dev-gate is exercised regardless of Ed25519 availability: a
    // non-developer caller is rejected before any derivation/RPC happens.
    assert!(
        bootstrap_as_non_dev(&pic, backend).is_err(),
        "non-developer caller must be rejected by the dev-gate"
    );

    // Point the backend's SOL RPC wrapper at the mock.
    decode_unit_result(
        update_dev(&pic, backend, "set_sol_rpc_principal", Encode!(&mock).unwrap()),
        "set_sol_rpc_principal",
    )
    .expect("set_sol_rpc_principal");

    // Register the Solana chain (501). The bootstrap itself does not read the
    // chain config (it only builds derivation paths from the chain id), but we
    // register it to mirror the proven solana_m2_pic.rs harness.
    let reg = RegisterChainArg {
        chain_id: ChainId(SOLANA_CHAIN_ID),
        display_name: "SolanaDevnet".to_string(),
        rpc_endpoints: vec!["https://mock-solana-rpc.invalid".to_string()],
        finality_depth: 0,
        gas_strategy: GasStrategy::SolanaPriorityFee {
            lamports_per_cu_ceiling: 10_000,
        },
        chain_native_decimals: 9,
    };
    decode_unit_result(
        update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");

    // ── Ed25519 probe: decide full vs gated ──────────────────────────────────
    // solana_settlement_address derives the settlement (mint-authority) address
    // via the management-canister threshold-Schnorr API. If PocketIC cannot
    // provision `test_key_1`, this errors and we skip the signing-dependent block.
    let ed25519_available = match update_dev(
        &pic,
        backend,
        "solana_settlement_address",
        encode_one(()).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, Reserved>) {
            Ok(Ok(addr)) => {
                assert!(
                    is_plausible_solana_address(&addr),
                    "settlement address is plausible base58: {addr}"
                );
                eprintln!("[solana bootstrap] Ed25519 AVAILABLE; settlement address = {addr}; running FULL block");
                true
            }
            Ok(Err(_)) => {
                eprintln!("[solana bootstrap] Ed25519 UNAVAILABLE (solana_settlement_address returned Err); running GATED subset");
                false
            }
            Err(decode_err) => {
                eprintln!("[solana bootstrap] solana_settlement_address decode error ({decode_err}); running GATED subset");
                false
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[solana bootstrap] Ed25519 UNAVAILABLE (solana_settlement_address rejected: {msg}); running GATED subset");
            false
        }
    };

    if !ed25519_available {
        // GATED SUBSET BOUNDARY: the bootstrap derives addresses (and the FULL
        // block signs) via threshold Ed25519, which is the wall here. The dev-gate
        // rejection above and the override decode logic are exercised on an
        // Ed25519-capable PocketIC; this auto-upgrades to the full block there.
        eprintln!("[solana bootstrap] GATED subset: threshold Ed25519 unavailable; dev-gate asserted, returning");
        return;
    }

    // ════════════════════════════════════════════════════════════════════════
    // FULL BLOCK (Ed25519 available)
    // ════════════════════════════════════════════════════════════════════════

    // Pre-condition the mock for an ACTUAL create (not the idempotent no-op):
    //   (a) the nonce account is NOT yet initialized, so `get_durable_nonce`
    //       returns Err and the bootstrap falls through to the create path;
    //   (b) `getLatestBlockhash` is INCONSISTENT, modeling playbook #4 (the
    //       per-slot value multi-provider consensus cannot agree on).
    update_any(&pic, mock, "set_nonce_exists", Encode!(&false).unwrap());
    update_any(
        &pic,
        mock,
        "set_latest_blockhash_inconsistent",
        Encode!(&true).unwrap(),
    );

    // (1) None => auto-fetch path. `get_latest_blockhash` hits the chronic
    //     `#Inconsistent` (Equality consensus rejects it) and the bootstrap fails
    //     BEFORE signing. This is the production bug the override exists to dodge.
    let none_result = bootstrap(&pic, backend, None);
    assert!(
        none_result.is_err(),
        "bootstrap(None) must FAIL: the consensus auto-fetch of the per-slot \
         getLatestBlockhash returns #Inconsistent on real clusters (playbook #4). \
         got: {none_result:?}"
    );

    // (2) Some(valid blockhash) => the override is fed straight into the
    //     create-nonce tx, bypassing the broken consensus fetch; the tx is then
    //     multi-signed (threshold Ed25519) and broadcast (the mock's
    //     sendTransaction returns Ok). This is the #4 escape hatch working.
    let ok_result = bootstrap(&pic, backend, Some(OVERRIDE_BLOCKHASH_B58.to_string()));
    assert!(
        ok_result.is_ok(),
        "bootstrap(Some(valid blockhash)) must SUCCEED: the override bypasses the \
         broken consensus fetch and the create-nonce tx signs + broadcasts. \
         got: {ok_result:?}"
    );

    // (3) Some(not-base58) => boundary validation. The operator-supplied value is
    //     decoded via decode_solana_address (32-byte base58) and a malformed value
    //     is rejected with a clear error before reaching the tx.
    let invalid_result = bootstrap(&pic, backend, Some(INVALID_BLOCKHASH.to_string()));
    assert!(
        invalid_result.is_err(),
        "bootstrap(Some(not-base58)) must FAIL: a non-32-byte/non-base58 \
         operator-supplied blockhash is rejected at validation. got: {invalid_result:?}"
    );

    // (4) Causal clincher: flip the inconsistency OFF (the nonce is STILL
    //     uninitialized via set_nonce_exists(false) above, so the bootstrap still
    //     falls through to the create path), and the SAME None auto-fetch now
    //     SUCCEEDS. This proves the (1) failure was CAUSED by the modeled
    //     #Inconsistent, not by signing, address derivation, or the dev gate (all
    //     of which are identical across every call in this block). Without this,
    //     a green (1) could in principle mask an unrelated failure cause.
    update_any(
        &pic,
        mock,
        "set_latest_blockhash_inconsistent",
        Encode!(&false).unwrap(),
    );
    let none_recovers = bootstrap(&pic, backend, None);
    assert!(
        none_recovers.is_ok(),
        "bootstrap(None) must SUCCEED once getLatestBlockhash is consistent again: \
         this proves the (1) failure was caused by #Inconsistent (not signing / \
         derivation / the dev gate). got: {none_recovers:?}"
    );

    eprintln!(
        "[solana bootstrap] FULL block PASSED: None auto-fetch FAILS under modeled \
         #Inconsistent then RECOVERS when consistent (causal proof); Some(valid) \
         override SUCCEEDS (signs + broadcasts); Some(invalid) rejected at \
         validation (playbook #4 escape hatch proven)"
    );
}

/// Call `solana_bootstrap_nonce(None)` as a non-developer principal (used only for
/// the dev-gate assertion). Returns the decoded Ok/Err.
fn bootstrap_as_non_dev(pic: &PocketIc, backend: Principal) -> Result<(), String> {
    let reply = pic
        .update_call(
            backend,
            non_dev(),
            "solana_bootstrap_nonce",
            Encode!(&Option::<String>::None).unwrap(),
        )
        .expect("ingress accepted (non-anonymous)");
    decode_unit_result(reply, "solana_bootstrap_nonce")
}
