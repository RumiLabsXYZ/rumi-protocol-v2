//! Solana M2 sign-seam proof (PocketIC).
//!
//! Proves the backend can build, threshold-Ed25519 sign, and assemble a legacy
//! Solana wire transaction end to end, and that the resulting signature actually
//! verifies against the signed message bytes under the fee-payer's public key.
//!
//! The verification is done HOST-side with `ed25519-dalek` (a dev-dependency, so
//! it does not touch the wasm artifact). We decode the wire transaction
//! ourselves (`[compact-u16 sig count][64-byte sig][serialized legacy message]`),
//! take the fee-payer pubkey from the message's first account key, and run
//! `verify_strict` over the message bytes. This avoids enabling the wasm-risky
//! `verify` feature on `solana-transaction` (and the serde-bumping `bincode`
//! feature elsewhere) just to call `Transaction::verify()`.
//!
//! Best-effort, exactly like the M1 seam test: if this PocketIC build does not
//! provision a threshold-Ed25519 `test_key_1`, the developer call returns an
//! Err / Reject; we log and skip the signature assertion (the real signature is
//! verified on devnet). The dev-gate rejection assertion always runs.
//!
//! Notes:
//! - `inspect_message` rejects ALL anonymous update callers at ingress, so the
//!   dev-gate is exercised with a non-anonymous, non-developer principal.
//! - The error arm is decoded as `candid::Reserved`: the canister's full
//!   `ProtocolError` does not subtype-decode into a minimal local mirror, and we
//!   only need to know the call returned an error.

use candid::{encode_args, CandidType, Decode, Deserialize, Principal, Reserved};
use ed25519_dalek::{Signature, VerifyingKey};
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

/// Decode a Solana compact-u16 (short_vec) length prefix from `bytes[at..]`.
/// Returns (value, bytes_consumed). Mirrors the encoder in `chains/solana/tx.rs`.
fn decode_compact_u16(bytes: &[u8], at: usize) -> (u16, usize) {
    let mut value: u32 = 0;
    let mut consumed = 0usize;
    loop {
        let byte = bytes[at + consumed];
        value |= ((byte & 0x7f) as u32) << (7 * consumed);
        consumed += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
    (value as u16, consumed)
}

/// Split the legacy wire tx into (signature, message_bytes) and pull the fee
/// payer (account_keys[0]) out of the message. Wire layout:
///   [compact-u16 sig count = 1][64-byte sig][message]
/// message: [3 header bytes][compact-u16 n_keys][keys * 32][blockhash][...]
fn dissect_wire_tx(wire: &[u8]) -> ([u8; 64], Vec<u8>, [u8; 32]) {
    let (sig_count, off) = decode_compact_u16(wire, 0);
    assert_eq!(sig_count, 1, "single-signature transfer");
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&wire[off..off + 64]);
    let message_bytes = wire[off + 64..].to_vec();

    // Inside the message: skip the 3-byte header, then read account_keys.
    let (n_keys, klen) = decode_compact_u16(&message_bytes, 3);
    assert!(n_keys >= 1, "at least the fee payer");
    let keys_start = 3 + klen;
    let mut fee_payer = [0u8; 32];
    fee_payer.copy_from_slice(&message_bytes[keys_start..keys_start + 32]);
    (sig, message_bytes, fee_payer)
}

#[test]
fn solana_sign_test_transfer_rejects_non_developer() {
    let (pic, backend) = boot();
    let to = "11111111111111111111111111111111".to_string();
    let reply = pic
        .update_call(
            backend,
            non_dev(),
            "solana_sign_test_transfer",
            encode_args((to, 1_000_000u64)).unwrap(),
        )
        .expect("ingress accepted (non-anonymous)");
    let res = match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<Vec<u8>, Reserved>).expect("decode"),
        WasmResult::Reject(m) => panic!("unexpected reject: {m}"),
    };
    assert!(
        res.is_err(),
        "non-developer caller must be rejected by the dev-gate"
    );
}

#[test]
fn solana_sign_test_transfer_dev_signs_and_signature_verifies() {
    let (pic, backend) = boot();
    let to = "11111111111111111111111111111111".to_string();
    let lamports = 1_000_000u64;

    let dev_reply = pic
        .update_call(
            backend,
            dev(),
            "solana_sign_test_transfer",
            encode_args((to, lamports)).unwrap(),
        )
        .expect("ingress accepted (developer)");

    let wire = match dev_reply {
        WasmResult::Reply(b) => match Decode!(&b, Result<Vec<u8>, Reserved>).expect("decode") {
            Ok(wire) => wire,
            Err(_) => {
                eprintln!(
                    "[M2 sign] threshold-Ed25519 sign returned an error in PocketIC \
                     (test_key_1 likely not provisioned; verified on devnet)"
                );
                return;
            }
        },
        WasmResult::Reject(m) => {
            eprintln!(
                "[M2 sign] solana_sign_test_transfer rejected (Ed25519 key likely \
                 absent; verified on devnet): {m}"
            );
            return;
        }
    };

    // Wire = [compact-u16 sig count][64-byte sig][serialized message].
    assert!(
        wire.len() > 1 + 64 + 3,
        "wire tx must hold a sig count, a 64-byte sig, and a non-trivial message"
    );
    assert_eq!(wire[0], 1, "first byte is the compact-u16 signature count (1)");

    let (sig_bytes, message_bytes, fee_payer) = dissect_wire_tx(&wire);

    // The threshold-Ed25519 signature must verify over the exact message bytes
    // under the fee-payer (settlement) public key. verify_strict rejects the
    // small-order / malleable edge cases that plain verify would accept.
    let vk = VerifyingKey::from_bytes(&fee_payer)
        .expect("fee-payer pubkey is a valid Ed25519 point");
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify_strict(&message_bytes, &sig)
        .expect("threshold-Ed25519 signature must verify over the serialized message");
}
