//! Known-answer tests locking the hand-rolled native-SOL wire format against
//! the real `@solana/web3.js` SDK (`tools/sol-kat`, `testdata/sol_kat.json`).
//!
//! This is the single highest-leverage lock on the whole native-SOL rail: it
//! proves the byte layout `chains::sol::adapter::sign_sol_payment_from`
//! assembles (durable-nonce advance + transfer, fee payer distinct from the
//! transfer source, two-signature wire assembly) matches what a real Solana
//! client independently produces for the identical inputs.
//!
//! Regenerate the fixture per `testdata/sol_kat.json`'s own `comment` field:
//! `cd tools/sol-kat && npm install && node index.mjs > \
//!   ../../src/rumi_protocol_backend/src/chains/sol/testdata/sol_kat.json`
//!
//! If any assertion here ever disagrees with a freshly regenerated fixture,
//! OUR RUST CODE is what is wrong (a hand-rolled wire-format bug is exactly
//! what this file exists to catch) — never edit the fixture to match our
//! output.

use solana_message::{Hash, Message};
use solana_pubkey::Pubkey;

use crate::chains::solana::tx::{
    advance_nonce_instruction, assemble_wire_tx_multi, encode_compact_u16, first_signature_base58,
    order_signatures_by_signer, serialize_legacy_message, system_transfer_instruction,
};

const KAT: &str = include_str!("testdata/sol_kat.json");

fn kat() -> serde_json::Value {
    serde_json::from_str(KAT).expect("valid sol_kat.json")
}

fn pubkey_from_hex(hex_str: &str) -> Pubkey {
    let bytes = hex::decode(hex_str).expect("valid hex pubkey");
    let arr: [u8; 32] = bytes.as_slice().try_into().expect("32-byte pubkey");
    Pubkey::new_from_array(arr)
}

fn hash_from_base58(s: &str) -> Hash {
    let bytes = bs58::decode(s).into_vec().expect("valid base58 hash");
    let arr: [u8; 32] = bytes.as_slice().try_into().expect("32-byte hash");
    Hash::new_from_array(arr)
}

fn sig_from_hex(hex_str: &str) -> [u8; 64] {
    let bytes = hex::decode(hex_str).expect("valid hex signature");
    bytes.as_slice().try_into().expect("64-byte signature")
}

/// Vector 1 (design doc §10): base58 address encoding from a known 32-byte
/// Ed25519 public key. `chains::sol::ted25519::derive_sol_address` calls
/// `chains::solana::ted25519::solana_address_from_pubkey` internally to turn a
/// threshold-derived pubkey into the address handed back to callers, so that
/// is the function locked here (rather than reimplementing a bs58 encode).
#[test]
fn address_encoding_matches_web3js() {
    let v = kat();
    let pubkey_hex = v["address"]["pubkey_hex"].as_str().unwrap();
    let expected_addr = v["address"]["address_base58"].as_str().unwrap();
    let pubkey_bytes = hex::decode(pubkey_hex).unwrap();

    let got = crate::chains::solana::ted25519::solana_address_from_pubkey(&pubkey_bytes)
        .expect("solana_address_from_pubkey");
    assert_eq!(got, expected_addr, "address encoding must match web3.js");

    // Round-trip through chains::sol::address's own decoder too, since that is
    // the function every caller-supplied SOL destination is validated with.
    let decoded = super::address::decode_sol_address(expected_addr).expect("decode_sol_address");
    assert_eq!(decoded.to_vec(), pubkey_bytes, "decode must recover the same pubkey bytes");
}

/// Vector 2: legacy message serialization for a plain System-program transfer
/// (single signer, `from` is both fee payer and transfer source) — the shape
/// `chains::solana::tx::build_transfer_message` / `system_transfer_instruction`
/// produce, reused as-is by this rail.
#[test]
fn legacy_transfer_message_matches_web3js() {
    let v = kat();
    let t = &v["legacy_transfer"];
    let from = pubkey_from_hex(t["from_pubkey_hex"].as_str().unwrap());
    let to = pubkey_from_hex(t["to_pubkey_hex"].as_str().unwrap());
    let lamports = t["lamports"].as_u64().unwrap();
    let blockhash = hash_from_base58(t["recent_blockhash_base58"].as_str().unwrap());

    let ix = system_transfer_instruction(&from, &to, lamports);
    let message = Message::new_with_blockhash(&[ix], Some(&from), &blockhash);
    let bytes = serialize_legacy_message(&message);

    assert_eq!(
        hex::encode(bytes),
        t["message_hex"].as_str().unwrap(),
        "plain-transfer legacy message bytes must match web3.js"
    );
}

/// Vector 3 (design doc §5.1): durable-nonce transfer message bytes — the
/// EXACT two-signer shape `chains::sol::adapter::sign_sol_payment_from` builds:
/// `[advance_nonce_account(nonce, authority=settlement), transfer(custody->dest)]`,
/// fee payer = settlement, DISTINCT from the transfer source (custody).
#[test]
fn durable_nonce_transfer_message_matches_web3js() {
    let v = kat();
    let t = &v["durable_nonce_transfer"];
    let settlement = pubkey_from_hex(t["settlement_pubkey_hex"].as_str().unwrap());
    let custody = pubkey_from_hex(t["custody_pubkey_hex"].as_str().unwrap());
    let nonce_account = pubkey_from_hex(t["nonce_account_pubkey_hex"].as_str().unwrap());
    let destination = pubkey_from_hex(t["destination_pubkey_hex"].as_str().unwrap());
    let lamports = t["lamports"].as_u64().unwrap();
    let durable_nonce = hash_from_base58(t["durable_nonce_base58"].as_str().unwrap());

    // Mirrors sign_sol_payment_from's message composition exactly.
    let advance = advance_nonce_instruction(&nonce_account, &settlement);
    let transfer = system_transfer_instruction(&custody, &destination, lamports);
    let message = Message::new_with_blockhash(&[advance, transfer], Some(&settlement), &durable_nonce);

    assert_eq!(
        message.header.num_required_signatures as u64,
        t["num_required_signatures"].as_u64().unwrap(),
        "two required signers: settlement (fee payer) + custody (transfer source)"
    );

    let expected_signers: Vec<String> = t["required_signer_pubkeys_base58"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    let got_signers: Vec<String> = message.account_keys
        [..message.header.num_required_signatures as usize]
        .iter()
        .map(|k| bs58::encode(k.as_ref()).into_string())
        .collect();
    assert_eq!(
        got_signers, expected_signers,
        "signer order must match web3.js's compiled account_keys[0..num_required_signatures]"
    );

    let bytes = serialize_legacy_message(&message);
    assert_eq!(
        hex::encode(bytes),
        t["message_hex"].as_str().unwrap(),
        "durable-nonce transfer message bytes must match web3.js byte-for-byte"
    );
}

/// Vector 4: wire-transaction assembly with two signatures — ground truth for
/// `assemble_wire_tx_multi` + `order_signatures_by_signer`, using the SAME
/// durable-nonce message as the previous test and web3.js's own real Ed25519
/// signatures (taken as opaque 64-byte blobs; only the ASSEMBLY is under test
/// here, not signature validity, which threshold Ed25519 is not reproducible
/// off-chain for anyway).
#[test]
fn wire_tx_two_sig_assembly_matches_web3js() {
    let v = kat();
    let dn = &v["durable_nonce_transfer"];
    let settlement = pubkey_from_hex(dn["settlement_pubkey_hex"].as_str().unwrap());
    let custody = pubkey_from_hex(dn["custody_pubkey_hex"].as_str().unwrap());
    let nonce_account = pubkey_from_hex(dn["nonce_account_pubkey_hex"].as_str().unwrap());
    let destination = pubkey_from_hex(dn["destination_pubkey_hex"].as_str().unwrap());
    let lamports = dn["lamports"].as_u64().unwrap();
    let durable_nonce = hash_from_base58(dn["durable_nonce_base58"].as_str().unwrap());

    let advance = advance_nonce_instruction(&nonce_account, &settlement);
    let transfer = system_transfer_instruction(&custody, &destination, lamports);
    let message = Message::new_with_blockhash(&[advance, transfer], Some(&settlement), &durable_nonce);
    let message_bytes = serialize_legacy_message(&message);

    let wt = &v["wire_tx"];
    let settlement_sig = sig_from_hex(wt["settlement_signature_hex"].as_str().unwrap());
    let custody_sig = sig_from_hex(wt["custody_signature_hex"].as_str().unwrap());

    // order_signatures_by_signer is exactly what sign_sol_payment_from calls
    // before assemble_wire_tx_multi.
    let signers = [(settlement, settlement_sig), (custody, custody_sig)];
    let ordered = order_signatures_by_signer(&message, &signers).expect("order signatures");
    let wire = assemble_wire_tx_multi(&ordered, &message_bytes);

    assert_eq!(
        hex::encode(&wire),
        wt["wire_tx_hex"].as_str().unwrap(),
        "assembled wire tx must match web3.js's own Transaction.serialize()"
    );

    // first_signature_base58 is what sign_sol_payment_from returns as the
    // locally-computed transaction signature/id.
    let sig58 = first_signature_base58(&wire).expect("first_signature_base58");
    assert_eq!(
        sig58,
        wt["first_signature_base58"].as_str().unwrap(),
        "locally computed tx signature must match web3.js's fee-payer signature"
    );
}

/// Vector 5: compact-u16 (ShortU16 / short_vec length prefix) encoding edge
/// cases, including the 1-byte/2-byte boundary (127/128) and the 2-byte/3-byte
/// boundary (16383/16384) — ground truth extracted from the pinned
/// `@solana/web3.js` dependency's own shipped `encodeLength` (see
/// `tools/sol-kat/index.mjs`'s module doc comment for why a full-message
/// `serialize()` cannot reach the 3-byte boundary directly).
#[test]
fn compact_u16_matches_web3js() {
    let v = kat();
    for entry in v["compact_u16"].as_array().unwrap() {
        let value = entry["value"].as_u64().unwrap() as u16;
        let expected_hex = entry["encoded_hex"].as_str().unwrap();
        assert_eq!(
            hex::encode(encode_compact_u16(value)),
            expected_hex,
            "compact-u16 encoding mismatch for value={value}"
        );
    }
}
