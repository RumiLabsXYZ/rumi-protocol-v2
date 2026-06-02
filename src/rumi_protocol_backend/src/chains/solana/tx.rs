//! Solana transaction building, signing, and wire assembly (M2 Task 1).
//!
//! Scope is deliberately narrow: build a legacy System Transfer message, sign
//! its serialized bytes with threshold Ed25519 (`ted25519::sign_message`), and
//! assemble the legacy wire transaction `[compact-u16 sig count][sigs][message]`.
//! Tasks 2-4 add SPL MintTo / ATA-create instructions, durable-nonce messages,
//! and the settlement adapter to this same module family, so the seams here stay
//! generic (a message builder, a serializer, a signer, a wire assembler).
//!
//! Serialization note: the project pins `solana-message` with
//! `default-features = false`, which leaves BOTH the `wincode` feature (gates the
//! inherent `Message::serialize()`) and the `serde` feature (gates the
//! `Serialize` derive) OFF. Enabling either pulls new crates (`wincode` /
//! `solana-short-vec`) that are absent from the committed Cargo.lock and would
//! shift resolution, which the M2 plan forbids. We therefore serialize the
//! legacy message directly from its public fields (`header`, `account_keys`,
//! `recent_blockhash`, `instructions`) using the documented legacy wire layout.
//! This is the exact byte layout `Message::serialize()` produces; the round-trip
//! is verified host-side in the PocketIC test by re-deriving the fee-payer pubkey
//! from these bytes and checking the threshold-Ed25519 signature over them.

use solana_instruction::{AccountMeta, Instruction};
use solana_message::{Hash, Message};
use solana_pubkey::Pubkey;

use super::ted25519;

/// The System Program address is 32 zero bytes (base58 `1111...1111`).
pub const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

/// Bincode enum discriminant for `SystemInstruction::Transfer` (the third
/// variant: CreateAccount=0, Assign=1, Transfer=2). Solana serializes the enum
/// tag as a u32 little-endian, so the instruction data is `[02 00 00 00]` then
/// `lamports` as a u64 little-endian (12 bytes total).
const SYSTEM_INSTRUCTION_TRANSFER_TAG: u32 = 2;

/// Encode a length as a Solana compact-u16 (a.k.a. ShortU16 / short_vec length
/// prefix): a little-endian base-128 varint, 1-3 bytes, where each byte carries
/// 7 value bits and the high bit flags continuation. Lengths that fit in 7 bits
/// (the common single-signature / few-account case) encode as a single byte.
pub fn encode_compact_u16(mut value: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(3);
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
            out.push(byte);
        } else {
            out.push(byte);
            break;
        }
    }
    out
}

/// Serialize a legacy `Message` to its canonical wire bytes from its public
/// fields. Layout (matches `solana_message::legacy::Message::serialize`):
///   [header: 3 bytes]
///   [compact-u16 account_keys.len()][account_keys * 32]
///   [recent_blockhash: 32]
///   [compact-u16 instructions.len()]
///   for each instruction:
///     [program_id_index: u8]
///     [compact-u16 accounts.len()][accounts * u8]
///     [compact-u16 data.len()][data bytes]
pub fn serialize_legacy_message(message: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    // MessageHeader (three u8s, in declaration order).
    out.push(message.header.num_required_signatures);
    out.push(message.header.num_readonly_signed_accounts);
    out.push(message.header.num_readonly_unsigned_accounts);

    // account_keys: compact-u16 length, then 32 bytes each.
    out.extend_from_slice(&encode_compact_u16(message.account_keys.len() as u16));
    for key in &message.account_keys {
        out.extend_from_slice(key.as_ref());
    }

    // recent_blockhash: 32 raw bytes.
    out.extend_from_slice(&message.recent_blockhash.to_bytes());

    // instructions: compact-u16 length, then each compiled instruction.
    out.extend_from_slice(&encode_compact_u16(message.instructions.len() as u16));
    for ix in &message.instructions {
        out.push(ix.program_id_index);
        out.extend_from_slice(&encode_compact_u16(ix.accounts.len() as u16));
        out.extend_from_slice(&ix.accounts);
        out.extend_from_slice(&encode_compact_u16(ix.data.len() as u16));
        out.extend_from_slice(&ix.data);
    }
    out
}

/// Assemble a legacy wire transaction: the compact-array of signatures (here a
/// single signature, so count = 1 encodes to the single byte 0x01) followed by
/// the 64-byte signature, then the serialized message. Layout:
/// `[compact-u16 sig count][sigs][message]`.
pub fn assemble_wire_tx(signature: [u8; 64], message_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 64 + message_bytes.len());
    // Exactly one signature; count fits in 7 bits so this is the single byte 0x01.
    out.extend_from_slice(&encode_compact_u16(1));
    out.extend_from_slice(&signature);
    out.extend_from_slice(message_bytes);
    out
}

/// Hand-encode a `SystemProgram::transfer` instruction.
///
/// `solana_system_interface::instruction::transfer` is gated behind that crate's
/// `bincode`/`wincode` feature, both of which activate an optional serde dep
/// declared at `>= 1.0.226`. This project pins serde to 1.0.217 (the IC fork's
/// `ic-types` breaks on newer serde), so enabling that feature is off the table.
/// The Transfer instruction is a stable, well-defined byte layout, so we build it
/// directly. The account-ordering / header math is still delegated to Solana's
/// own `Message::new_with_blockhash` (it is not feature-gated), so the only
/// hand-rolled surface is the 12-byte instruction data and the two AccountMetas.
///
/// Layout (matches the gated `transfer`):
///   accounts: [ (from, signer, writable), (to, non-signer, writable) ]
///   data:     [ tag: u32 LE = 2 ][ lamports: u64 LE ]
pub fn system_transfer_instruction(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&SYSTEM_INSTRUCTION_TRANSFER_TAG.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: Pubkey::new_from_array(SYSTEM_PROGRAM_ID),
        accounts: vec![
            // AccountMeta::new => writable; from is also the signer/fee payer.
            AccountMeta::new(*from, true),
            AccountMeta::new(*to, false),
        ],
        data,
    }
}

/// Build a legacy System Transfer message: a single `SystemProgram::transfer`
/// instruction moving `lamports` from `from` to `to`, with `from` as the fee
/// payer (`account_keys[0]`) and the given `recent_blockhash`. Account-key
/// compilation, signer-first ordering, and the message header are computed by
/// `Message::new_with_blockhash`.
pub fn build_transfer_message(
    from: &Pubkey,
    to: &Pubkey,
    lamports: u64,
    recent_blockhash: Hash,
) -> Message {
    let ix = system_transfer_instruction(from, to, lamports);
    Message::new_with_blockhash(&[ix], Some(from), &recent_blockhash)
}

/// Build a transfer message, serialize it, threshold-Ed25519 sign the serialized
/// bytes at `from_path`, and assemble the legacy wire transaction.
///
/// `from_pubkey` is the 32-byte Ed25519 public key of the signer (the `from`
/// account); `from_path` is its derivation path. The two MUST correspond (the
/// pubkey is derived from the same path via `ted25519::derive_solana_address`),
/// otherwise the on-chain signature check fails.
pub async fn sign_transfer(
    from_path: Vec<Vec<u8>>,
    from_pubkey: &[u8],
    to: &Pubkey,
    lamports: u64,
    blockhash: Hash,
) -> Result<Vec<u8>, String> {
    if from_pubkey.len() != 32 {
        return Err(format!(
            "from_pubkey must be a 32-byte Ed25519 key, got {}",
            from_pubkey.len()
        ));
    }
    let mut from_arr = [0u8; 32];
    from_arr.copy_from_slice(from_pubkey);
    let from = Pubkey::new_from_array(from_arr);

    let message = build_transfer_message(&from, to, lamports, blockhash);
    let message_bytes = serialize_legacy_message(&message);
    let signature = ted25519::sign_message(message_bytes.clone(), from_path).await?;
    if signature.len() != 64 {
        return Err(format!(
            "expected 64-byte Ed25519 signature, got {}",
            signature.len()
        ));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&signature);
    Ok(assemble_wire_tx(sig_arr, &message_bytes))
}
