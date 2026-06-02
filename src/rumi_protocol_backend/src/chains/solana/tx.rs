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

/// SPL Token program id (base58). The classic Token program, not Token-2022.
const TOKEN_PROGRAM_B58: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
/// Associated Token Account program id (base58).
const ATA_PROGRAM_B58: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// SPL Token `MintTo` instruction discriminant. SPL Token tags its instruction
/// enum with a single leading byte; `MintTo` is variant 7.
const SPL_TOKEN_MINT_TO_TAG: u8 = 7;
/// Associated Token Account `CreateIdempotent` instruction discriminant. The ATA
/// program tags its instructions with a single leading byte; `CreateIdempotent`
/// is variant 1 (Create=0, CreateIdempotent=1, RecoverNested=2).
const ATA_CREATE_IDEMPOTENT_TAG: u8 = 1;

/// Decode a base58 Solana address literal to a `Pubkey`.
///
/// Used only for the fixed program-id constants below, so a malformed literal is
/// a compile-fixed bug, not a runtime input; we panic loudly rather than thread a
/// Result through pure instruction builders. `Pubkey::from_str` is gated behind
/// solana-address's `decode` feature, which is off here, so we go through `bs58`
/// (already a direct dependency) and `new_from_array`.
fn pubkey_from_base58(b58: &str) -> Pubkey {
    let bytes = bs58::decode(b58)
        .into_vec()
        .expect("program-id literal must be valid base58");
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .expect("program-id literal must decode to 32 bytes");
    Pubkey::new_from_array(arr)
}

/// The SPL Token program id as a `Pubkey`.
pub fn token_program_id() -> Pubkey {
    pubkey_from_base58(TOKEN_PROGRAM_B58)
}

/// The Associated Token Account program id as a `Pubkey`.
pub fn ata_program_id() -> Pubkey {
    pubkey_from_base58(ATA_PROGRAM_B58)
}

/// The System Program id as a `Pubkey` (32 zero bytes).
pub fn system_program_id() -> Pubkey {
    Pubkey::new_from_array(SYSTEM_PROGRAM_ID)
}

/// Derive the Associated Token Account address for `(owner, mint)` under the
/// standard SPL Token program.
///
/// An ATA is a program-derived address (PDA) of the ATA program over the seeds
/// `[owner, token_program, mint]`. `find_program_address` walks bump seeds from
/// 255 down until the resulting address is off the ed25519 curve (a PDA has no
/// private key); it returns `(address, bump)` and we keep only the address. This
/// requires the `curve25519` feature on `solana-pubkey` (enabled in Cargo.toml)
/// for the off-curve check off-chain.
pub fn derive_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program_id().as_ref(), mint.as_ref()],
        &ata_program_id(),
    )
    .0
}

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

/// Hand-encode an SPL Token `MintTo` instruction.
///
/// The `spl-token` crate is deliberately not a dependency (it pulls
/// spl-token-2022's confidential-transfer crypto, which is wasm-risky and heavy;
/// see the note in Cargo.toml). `MintTo` has a stable, trivial byte layout, so we
/// build it directly.
///
/// Layout (matches `spl_token::instruction::mint_to`):
///   accounts: [ (mint, writable, non-signer),
///               (dest_ata, writable, non-signer),
///               (mint_authority, read-only, signer) ]
///   data:     [ tag: u8 = 7 (MintTo) ][ amount: u64 LE ]   (9 bytes)
///
/// `amount` is in the mint's base units (no decimal scaling here).
pub fn mint_to_ix(mint: &Pubkey, dest_ata: &Pubkey, authority: &Pubkey, amount: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(SPL_TOKEN_MINT_TO_TAG);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: token_program_id(),
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*dest_ata, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// Hand-encode an Associated Token Account `CreateIdempotent` instruction.
///
/// `CreateIdempotent` creates the ATA for `(owner, mint)` if it does not already
/// exist and succeeds (rather than erroring) if it does, which is exactly what a
/// mint flow wants: it can run the create unconditionally before `MintTo` without
/// a prior existence check. The ATA address is derived here from `(owner, mint)`,
/// so the caller never passes it.
///
/// Layout (matches `spl_associated_token_account::instruction::create_associated_token_account_idempotent`):
///   accounts: [ (funder, writable, signer),
///               (ata, writable, non-signer),
///               (owner, read-only, non-signer),
///               (mint, read-only, non-signer),
///               (system_program, read-only, non-signer),
///               (token_program, read-only, non-signer) ]
///   data:     [ tag: u8 = 1 (CreateIdempotent) ]   (1 byte)
pub fn create_ata_idempotent_ix(funder: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    let ata = derive_ata(owner, mint);
    Instruction {
        program_id: ata_program_id(),
        accounts: vec![
            AccountMeta::new(*funder, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program_id(), false),
            AccountMeta::new_readonly(token_program_id(), false),
        ],
        data: vec![ATA_CREATE_IDEMPOTENT_TAG],
    }
}

/// Build a legacy message that mints `amount` of `mint` to `recipient_owner`'s
/// associated token account, with `authority` as both the fee payer and the mint
/// authority.
///
/// Two instructions, in order:
///   1. `create_ata_idempotent_ix(authority, recipient_owner, mint)` ensures the
///      recipient's ATA exists (no-op if it already does), funded by `authority`.
///   2. `mint_to_ix(mint, derive_ata(recipient_owner, mint), authority, amount)`
///      mints into that ATA.
///
/// As in `build_transfer_message`, account-key compilation, signer-first ordering,
/// and the message header are delegated to the non-feature-gated
/// `Message::new_with_blockhash` so the wire layout stays canonical.
pub fn build_mint_message(
    authority: &Pubkey,
    mint: &Pubkey,
    recipient_owner: &Pubkey,
    amount: u64,
    recent_blockhash: Hash,
) -> Message {
    let dest_ata = derive_ata(recipient_owner, mint);
    let create_ata = create_ata_idempotent_ix(authority, recipient_owner, mint);
    let mint_to = mint_to_ix(mint, &dest_ata, authority, amount);
    Message::new_with_blockhash(&[create_ata, mint_to], Some(authority), &recent_blockhash)
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
