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

/// RecentBlockhashes sysvar id (base58). Referenced (read-only) by both the
/// AdvanceNonceAccount and InitializeNonceAccount System instructions.
const RECENT_BLOCKHASHES_SYSVAR_B58: &str = "SysvarRecentB1ockHashes11111111111111111111";
/// Rent sysvar id (base58). Referenced (read-only) by InitializeNonceAccount.
const RENT_SYSVAR_B58: &str = "SysvarRent111111111111111111111111111111111";

/// Serialized size of a System nonce account, the `space` allocated when creating
/// one (matches `solana_system_interface`'s `NONCE_STATE_SIZE`, verified against
/// the crate source).
pub const NONCE_STATE_SIZE: u64 = 80;

/// Bincode enum discriminants for the System nonce instructions. Solana
/// serializes the `SystemInstruction` enum tag as a u32 little-endian (confirmed
/// via `Instruction::new_with_bincode` -> `bincode::serialize`, whose default
/// config uses a 4-byte LE variant index for ALL variants). The enum order is
/// CreateAccount=0, Assign=1, Transfer=2, CreateAccountWithSeed=3,
/// AdvanceNonceAccount=4, WithdrawNonceAccount=5, InitializeNonceAccount=6, ...
const SYSTEM_INSTRUCTION_CREATE_ACCOUNT_TAG: u32 = 0;
const SYSTEM_INSTRUCTION_ADVANCE_NONCE_TAG: u32 = 4;
const SYSTEM_INSTRUCTION_INITIALIZE_NONCE_TAG: u32 = 6;

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

/// The RecentBlockhashes sysvar id as a `Pubkey`.
pub fn recent_blockhashes_sysvar_id() -> Pubkey {
    pubkey_from_base58(RECENT_BLOCKHASHES_SYSVAR_B58)
}

/// The Rent sysvar id as a `Pubkey`.
pub fn rent_sysvar_id() -> Pubkey {
    pubkey_from_base58(RENT_SYSVAR_B58)
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

/// Assemble a legacy wire transaction with one or more signatures:
/// `[compact-u16 sig count][sig0][sig1]...[message]`. The single-signature case
/// is byte-identical to `assemble_wire_tx`.
///
/// The signatures MUST be supplied in the SAME ORDER as the message's required
/// signers, i.e. `account_keys[0..num_required_signatures]` (the on-chain runtime
/// matches signature `i` against signer key `i`). Callers that sign with threshold
/// Ed25519 must therefore order the `sigs` to match the compiled message's signer
/// keys; see `bootstrap_nonce_account` for the create-nonce two-signer ordering.
pub fn assemble_wire_tx_multi(sigs: &[[u8; 64]], message_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + sigs.len() * 64 + message_bytes.len());
    out.extend_from_slice(&encode_compact_u16(sigs.len() as u16));
    for sig in sigs {
        out.extend_from_slice(sig);
    }
    out.extend_from_slice(message_bytes);
    out
}

/// Decode a Solana compact-u16 (ShortU16 / short_vec length prefix) from the
/// front of `bytes`, returning `(value, bytes_consumed)`. The inverse of
/// `encode_compact_u16`: a little-endian base-128 varint, 1-3 bytes, where each
/// byte carries 7 value bits and the high bit flags continuation.
///
/// Errs (never panics) on a truncated varint (continuation bit set on the last
/// available byte) or a value that would not fit in a u16 (a 4th byte, or a 3rd
/// byte with bits above the 16th). Pure + unit-tested.
pub fn decode_compact_u16(bytes: &[u8]) -> Result<(u16, usize), String> {
    let mut value: u32 = 0;
    for (i, &byte) in bytes.iter().enumerate().take(3) {
        let part = (byte & 0x7f) as u32;
        value |= part << (i * 7);
        if byte & 0x80 == 0 {
            // Terminal byte. Reject a value that does not round-trip into a u16
            // (e.g. a 3rd byte carrying bits above bit 15).
            if value > u16::MAX as u32 {
                return Err(format!("compact-u16 value {value} exceeds u16"));
            }
            return Ok((value as u16, i + 1));
        }
    }
    Err("compact-u16 is truncated or longer than 3 bytes".to_string())
}

/// Extract the transaction signature from a legacy wire transaction and return
/// it base58-encoded.
///
/// A Solana transaction's signature is its FIRST signature (the fee payer's),
/// which is DETERMINISTIC from the signed message bytes. This parses the leading
/// compact-u16 signature count, takes the first 64 signature bytes, and
/// base58-encodes them, yielding the exact string `sendTransaction` returns for
/// the same wire bytes.
///
/// The Task-8 settlement worker computes this LOCALLY from the bytes the adapter
/// produced (before broadcasting) so it can track the op by its deterministic
/// signature regardless of whether the `sendTransaction` outcall returns Ok or a
/// "maybe-sent" Err. Because a durable-nonce tx advances the nonce exactly once
/// on success, re-broadcasting these SAME bytes is idempotent, so confirming by
/// this fixed signature (never re-signing with a fresh nonce) cannot double-mint.
///
/// Errs (never panics) on an empty buffer, a zero signature count (no signature
/// to track), or a buffer too short to hold the first 64-byte signature.
pub fn first_signature_base58(wire_tx: &[u8]) -> Result<String, String> {
    let (count, consumed) = decode_compact_u16(wire_tx)?;
    if count == 0 {
        return Err("wire tx has zero signatures".to_string());
    }
    let sig_end = consumed
        .checked_add(64)
        .ok_or_else(|| "signature offset overflow".to_string())?;
    let sig = wire_tx
        .get(consumed..sig_end)
        .ok_or_else(|| format!("wire tx too short for a 64-byte signature: len {}", wire_tx.len()))?;
    Ok(bs58::encode(sig).into_string())
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

// ─── Durable nonce: System instruction builders (Task 3) ─────────────────────

/// Hand-encode a `SystemProgram::AdvanceNonceAccount` instruction.
///
/// Consumes the stored durable nonce and replaces it with its successor. Must be
/// the FIRST instruction of any nonce-backed transaction (the runtime recognizes
/// a durable-nonce tx by this leading instruction). Matches the gated
/// `solana_system_interface::instruction::advance_nonce_account`.
///
/// Layout (AdvanceNonceAccount is fieldless, so the data is just the u32-LE
/// discriminant 4):
///   accounts: [ (nonce, writable, non-signer),
///               (RecentBlockhashes sysvar, read-only, non-signer),
///               (authority, read-only, signer) ]
///   data:     [ tag: u32 LE = 4 ]   (4 bytes)
pub fn advance_nonce_instruction(nonce: &Pubkey, authority: &Pubkey) -> Instruction {
    Instruction {
        program_id: system_program_id(),
        accounts: vec![
            AccountMeta::new(*nonce, false),
            AccountMeta::new_readonly(recent_blockhashes_sysvar_id(), false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data: SYSTEM_INSTRUCTION_ADVANCE_NONCE_TAG.to_le_bytes().to_vec(),
    }
}

/// Hand-encode a `SystemProgram::CreateAccount` instruction (System variant 0).
///
/// Matches the gated `solana_system_interface::instruction::create_account`: both
/// `from` and the `new` account are writable signers (the new account must sign
/// because its key authorizes its own creation).
///
/// Layout:
///   accounts: [ (from, writable, signer), (new, writable, signer) ]
///   data:     [ tag: u32 LE = 0 ][ lamports u64 LE ][ space u64 LE ][ owner 32 ] (52 bytes)
pub fn create_account_instruction(
    from: &Pubkey,
    new: &Pubkey,
    lamports: u64,
    space: u64,
    owner: &Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(52);
    data.extend_from_slice(&SYSTEM_INSTRUCTION_CREATE_ACCOUNT_TAG.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: system_program_id(),
        accounts: vec![
            AccountMeta::new(*from, true),
            AccountMeta::new(*new, true),
        ],
        data,
    }
}

/// Hand-encode a `SystemProgram::InitializeNonceAccount` instruction (variant 6).
///
/// Drives an Uninitialized nonce account to Initialized, setting its authority.
/// Matches the gated `solana_system_interface::instruction`'s use inside
/// `create_nonce_account`. No signer is required (the authority is a data field,
/// not a signer), enabling derived nonce account addresses.
///
/// Layout:
///   accounts: [ (nonce, writable, non-signer),
///               (RecentBlockhashes sysvar, read-only), (Rent sysvar, read-only) ]
///   data:     [ tag: u32 LE = 6 ][ authority 32 bytes ]   (36 bytes)
pub fn initialize_nonce_instruction(nonce: &Pubkey, authority: &Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&SYSTEM_INSTRUCTION_INITIALIZE_NONCE_TAG.to_le_bytes());
    data.extend_from_slice(authority.as_ref());
    Instruction {
        program_id: system_program_id(),
        accounts: vec![
            AccountMeta::new(*nonce, false),
            AccountMeta::new_readonly(recent_blockhashes_sysvar_id(), false),
            AccountMeta::new_readonly(rent_sysvar_id(), false),
        ],
        data,
    }
}

/// Build the two-instruction message that creates AND initializes a durable nonce
/// account: `[create_account(from, nonce, lamports, 80, system_program),
/// initialize_nonce(nonce, authority)]`. Mirrors the gated
/// `create_nonce_account`. The fee payer is `from`; `recent_blockhash` is a REAL
/// network blockhash (the nonce does not exist yet, so it cannot self-reference).
///
/// This message has TWO required signers: the fee payer (`from`) and the new
/// nonce account (`nonce`), both writable signers in `create_account`. Account
/// compilation and header math are delegated to `Message::new_with_blockhash`.
pub fn build_create_nonce_account_message(
    from: &Pubkey,
    nonce: &Pubkey,
    authority: &Pubkey,
    lamports: u64,
    recent_blockhash: Hash,
) -> Message {
    let create = create_account_instruction(from, nonce, lamports, NONCE_STATE_SIZE, &system_program_id());
    let init = initialize_nonce_instruction(nonce, authority);
    Message::new_with_blockhash(&[create, init], Some(from), &recent_blockhash)
}

/// Build a durable-nonce-backed System Transfer message: the FIRST instruction is
/// `advance_nonce_account(nonce, from)` and the `recent_blockhash` is the durable
/// nonce (`durable_nonce`), not a network blockhash. `from` is the fee payer, the
/// nonce authority, AND the transfer source (all the same settlement key here).
///
/// The advance-nonce instruction is simply the first element of the slice passed
/// to `Message::new_with_blockhash`, so account ordering / header math stays
/// canonical.
pub fn build_transfer_message_with_nonce(
    from: &Pubkey,
    to: &Pubkey,
    lamports: u64,
    nonce: &Pubkey,
    durable_nonce: Hash,
) -> Message {
    let advance = advance_nonce_instruction(nonce, from);
    let transfer = system_transfer_instruction(from, to, lamports);
    Message::new_with_blockhash(&[advance, transfer], Some(from), &durable_nonce)
}

/// Build a durable-nonce-backed mint message: the FIRST instruction is
/// `advance_nonce_account(nonce, authority)`, then the unchanged
/// create-ATA-idempotent and MintTo instructions, with `recent_blockhash` set to
/// the durable nonce. `authority` is the fee payer, the mint authority, AND the
/// nonce authority (the single settlement key).
pub fn build_mint_message_with_nonce(
    authority: &Pubkey,
    mint: &Pubkey,
    recipient_owner: &Pubkey,
    amount: u64,
    nonce: &Pubkey,
    durable_nonce: Hash,
) -> Message {
    let advance = advance_nonce_instruction(nonce, authority);
    let dest_ata = derive_ata(recipient_owner, mint);
    let create_ata = create_ata_idempotent_ix(authority, recipient_owner, mint);
    let mint_to = mint_to_ix(mint, &dest_ata, authority, amount);
    Message::new_with_blockhash(&[advance, create_ata, mint_to], Some(authority), &durable_nonce)
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

// ─── Durable nonce: bootstrap (multi-signature create + initialize) ──────────

/// Rent-exempt lamports for an 80-byte nonce account on Solana. At the standard
/// rent rate this is 1_447_680 lamports (~0.00144768 SOL), computed as
/// (128 account-overhead + 80 data) bytes * 3480 lamports/byte-year * 2.0 years
/// (the rent-exemption threshold). The account keeps this balance for its
/// lifetime; the nonce only needs creating once per settlement key. (Devnet uses
/// the same rent parameters as mainnet.)
pub const NONCE_ACCOUNT_RENT_LAMPORTS: u64 = 1_447_680;

/// Resolve the ordered list of signatures for a message's required signers.
///
/// The on-chain runtime matches signature `i` against `account_keys[i]` for the
/// first `num_required_signatures` keys, so the wire tx must carry signatures in
/// that exact key order. `signers` maps each derived signer pubkey to its already
/// computed 64-byte signature (order irrelevant). Returns the signatures ordered
/// to match `account_keys[0..num_required_signatures]`, or an Err naming the first
/// required-signer key that has no provided signature (a derivation-path / key
/// mismatch bug). Pure (no async, no I/O) so the ordering logic is unit-tested.
pub fn order_signatures_by_signer(
    message: &Message,
    signers: &[(Pubkey, [u8; 64])],
) -> Result<Vec<[u8; 64]>, String> {
    let n = message.header.num_required_signatures as usize;
    let mut ordered = Vec::with_capacity(n);
    for key in &message.account_keys[0..n] {
        let sig = signers
            .iter()
            .find(|(pk, _)| pk == key)
            .map(|(_, sig)| *sig)
            .ok_or_else(|| {
                format!(
                    "no signature provided for required signer {}",
                    bs58::encode(key.as_ref()).into_string()
                )
            })?;
        ordered.push(sig);
    }
    Ok(ordered)
}

/// Idempotently bootstrap the settlement key's durable nonce account on the given
/// chain. If the nonce account already holds an Initialized durable nonce, returns
/// `Ok(())` without sending anything. Otherwise it:
///   1. derives the settlement (fee payer + nonce authority) and nonce addresses
///      (two distinct threshold-Ed25519 paths),
///   2. obtains a REAL recent blockhash (the nonce cannot self-reference yet) -
///      either the operator-supplied `blockhash_override` or, if none, the
///      consensus-dependent `sol_rpc::get_latest_blockhash` auto-fetch,
///   3. builds the 2-instruction create+initialize message,
///   4. MULTI-SIGNS it: signs the serialized bytes once per derivation path, then
///      orders the two signatures to match the message's required-signer keys, and
///   5. broadcasts via `sendTransaction`.
///
/// ## The `blockhash_override` escape hatch (playbook #4)
///
/// `getLatestBlockhash` returns a value that changes EVERY SLOT, so the DFINITY
/// sol-rpc canister's multi-provider consensus almost never agrees on it -> it
/// chronically returns `#Inconsistent`, which `get_latest_blockhash` (Equality
/// consensus) rejects as an error. On real devnet/mainnet the `None` auto-fetch
/// therefore RELIABLY FAILS, so the operator must supply a single fresh finalized
/// blockhash via `blockhash_override`, which is fed straight into the create-nonce
/// transaction here - bypassing canister-side multi-provider consensus for a value
/// that cannot reach it. Blockhashes expire (~60s), so the override must be fetched
/// and passed in the same shell, promptly.
///
/// `None` (auto-fetch) is correct only where multi-provider consensus on
/// `getLatestBlockhash` IS possible - i.e. the PocketIC mock (which returns a
/// single `Consistent(Ok)` response) and any consensus-capable environment. It is
/// also retained as the documented fallback. The override is the production path.
///
/// The operator runs this once per settlement key. The full create+sign+broadcast
/// round trip is exercised end-to-end only against the Task 9 mock / live devnet;
/// the pure pieces (message construction, signer ordering, multi-sig assembly) are
/// unit-tested, and the override-vs-auto-fetch split is proven in
/// `tests/solana_bootstrap_pic.rs` (override succeeds where the auto-fetch hits a
/// modeled `#Inconsistent`).
pub async fn bootstrap_nonce_account(
    chain: crate::chains::config::ChainId,
    blockhash_override: Option<Hash>,
) -> Result<(), String> {
    use super::sol_rpc;

    // Derive both addresses (path + pubkey). settlement = fee payer + authority.
    let settlement_path = ted25519::settlement_derivation_path(chain);
    let (settlement_pk_bytes, _settlement_addr) =
        ted25519::derive_solana_address(settlement_path.clone()).await?;
    let nonce_path = ted25519::nonce_derivation_path(chain);
    let (nonce_pk_bytes, nonce_addr) =
        ted25519::derive_solana_address(nonce_path.clone()).await?;

    // Idempotency: if the nonce already reads back as Initialized, we are done.
    if sol_rpc::get_durable_nonce(&nonce_addr).await.is_ok() {
        return Ok(());
    }

    let settlement = pubkey_from_bytes(&settlement_pk_bytes, "settlement")?;
    let nonce = pubkey_from_bytes(&nonce_pk_bytes, "nonce")?;

    // The create+initialize tx uses a REAL recent blockhash (the nonce does not
    // exist yet, so it has no durable value to reference).
    let recent_blockhash = match blockhash_override {
        // Operator-supplied fresh finalized blockhash (playbook #4 escape hatch):
        // fed straight in, bypassing canister-side multi-provider consensus.
        Some(bh) => bh,
        // Consensus path: works in PocketIC / consensus-capable environments;
        // chronically `#Inconsistent` (and thus fails) on real clusters.
        None => sol_rpc::get_latest_blockhash().await?,
    };

    let message = build_create_nonce_account_message(
        &settlement,
        &nonce,
        &settlement, // settlement is also the nonce authority
        NONCE_ACCOUNT_RENT_LAMPORTS,
        recent_blockhash,
    );
    let message_bytes = serialize_legacy_message(&message);

    // Multi-sign: sign the SAME serialized bytes with each required signer's path.
    // The two required signers are the fee payer (settlement) and the new nonce
    // account; both sign the identical message. We sign per distinct path, then
    // order the signatures to match account_keys[0..num_required_signatures].
    let settlement_sig = sign_64(message_bytes.clone(), settlement_path).await?;
    let nonce_sig = sign_64(message_bytes.clone(), nonce_path).await?;
    let signers = [(settlement, settlement_sig), (nonce, nonce_sig)];
    let ordered = order_signatures_by_signer(&message, &signers)?;

    let wire = assemble_wire_tx_multi(&ordered, &message_bytes);
    sol_rpc::send_transaction(&wire).await?;
    Ok(())
}

/// Decode a 32-byte slice into a `Pubkey`, erroring (rather than panicking) on a
/// wrong length. `label` names the key for the error message.
fn pubkey_from_bytes(bytes: &[u8], label: &str) -> Result<Pubkey, String> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{label} pubkey must be 32 bytes, got {}", bytes.len()))?;
    Ok(Pubkey::new_from_array(arr))
}

/// Sign `message` at `path` via threshold Ed25519 and return the 64-byte
/// signature as a fixed array (the management call already guarantees length 64,
/// but we re-check defensively).
async fn sign_64(message: Vec<u8>, path: Vec<Vec<u8>>) -> Result<[u8; 64], String> {
    let sig = ted25519::sign_message(message, path).await?;
    sig.as_slice()
        .try_into()
        .map_err(|_| format!("expected 64-byte Ed25519 signature, got {}", sig.len()))
}
