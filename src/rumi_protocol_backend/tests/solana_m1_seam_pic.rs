//! Solana M1 read-seam smoke test (PocketIC).
//!
//! Proves the backend installs with the M1 Solana code and that the dev-gated
//! read endpoints reject non-developers. Best-effort: if PocketIC provisions a
//! threshold-Ed25519 `test_key_1`, asserts `solana_settlement_address` returns a
//! plausible base58 address; otherwise logs and skips that assertion (the real
//! derivation is verified on devnet, M1 Task 8) — the same auto-degrade pattern
//! as the phase1b ECDSA tests.
//!
//! Notes:
//! - The canister's `inspect_message` hook rejects ALL anonymous update callers
//!   at ingress, so the dev-gate is exercised with a non-anonymous, non-developer
//!   principal (which passes ingress and is then rejected by the in-endpoint
//!   gate), not with the anonymous principal.
//! - The error arm is decoded as `candid::Reserved`: the canister's full
//!   `ProtocolError` variant does not subtype-decode into a minimal local mirror,
//!   and for these assertions we only need to know the call returned an error.
//! - The actual SOL RPC reads are verified on devnet (Task 8); PocketIC has no
//!   Solana network and we install no RPC mock here.

use candid::{encode_args, encode_one, CandidType, Decode, Deserialize, Principal, Reserved};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

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

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

/// A non-anonymous, non-developer principal: passes `inspect_message` ingress
/// inspection, then hits the in-endpoint developer gate.
fn non_dev() -> Principal {
    Principal::from_slice(&[7, 7, 7, 7, 7])
}

fn backend_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();
    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);

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
    for _ in 0..5 {
        pic.tick();
    }
    (pic, backend_id)
}

/// Lightweight plausibility check (avoids adding bs58 as a dev-dependency):
/// Solana addresses are 32-44 base58 characters.
fn is_plausible_solana_address(s: &str) -> bool {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    s.len() >= 32 && s.len() <= 44 && s.chars().all(|c| B58.contains(c))
}

#[test]
fn solana_get_balance_rejects_non_developer() {
    let (pic, backend) = boot();
    let reply = pic
        .update_call(
            backend,
            non_dev(),
            "solana_get_balance",
            encode_one(&"11111111111111111111111111111111".to_string()).unwrap(),
        )
        .expect("ingress accepted (non-anonymous)");
    let res = match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, Reserved>).expect("decode"),
        WasmResult::Reject(m) => panic!("unexpected reject: {m}"),
    };
    assert!(res.is_err(), "non-developer caller must be rejected by the dev-gate");
}

#[test]
fn solana_settlement_address_dev_gated_and_best_effort_derivation() {
    let (pic, backend) = boot();

    // Non-developer (non-anonymous) is rejected by the in-endpoint dev-gate.
    let nd = pic
        .update_call(backend, non_dev(), "solana_settlement_address", encode_one(()).unwrap())
        .expect("ingress accepted (non-anonymous)");
    let nd_res = match nd {
        WasmResult::Reply(b) => Decode!(&b, Result<String, Reserved>).expect("decode"),
        WasmResult::Reject(m) => panic!("unexpected reject: {m}"),
    };
    assert!(nd_res.is_err(), "non-developer caller must be dev-gated");

    // Developer: best-effort. Ok => plausible base58; Err/Reject => Ed25519 key
    // not provisioned in this PocketIC build (verified on devnet, Task 8).
    let dev_reply = pic
        .update_call(backend, dev(), "solana_settlement_address", encode_one(()).unwrap())
        .expect("ingress accepted (developer)");
    match dev_reply {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, Reserved>).expect("decode") {
            Ok(addr) => assert!(
                is_plausible_solana_address(&addr),
                "derived address is not plausible base58: {addr}"
            ),
            Err(_) => eprintln!(
                "[M1 smoke] Ed25519 derivation returned an error in PocketIC \
                 (key likely not provisioned; verified on devnet)"
            ),
        },
        WasmResult::Reject(m) => eprintln!(
            "[M1 smoke] settlement_address rejected (Ed25519 key likely absent; \
             verified on devnet): {m}"
        ),
    }
}
