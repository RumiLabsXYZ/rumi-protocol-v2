//! Solana transaction primitives, message builders and the return-shape checker
//! (`chain-solana.md` sections 2.1 to 2.4). No Solana SDK crate is used: the wire layout is
//! hand-encoded from its documented format and pinned by golden vectors that were produced
//! and cross-checked independently (see `golden.rs`).
//!
//! * this file: program ids, base58, compact-u16, program-derived and associated token
//!   addresses (token program as a parameter), instruction encoders, legacy/v0 message
//!   compile, serialize and strict decode, wire assembly and signature ordering;
//! * [`builders`]: the canister-built mint, burn and refund messages and the reference
//!   (user-built) return message;
//! * [`shape`]: the pure function that decides whether a decoded message has exactly the
//!   shape of a valid user return.
//!
//! Provenance of the copied helpers: compact-u16, legacy serialization, wire assembly,
//! signature ordering and the nonce and ATA instruction layouts come from
//! `rumi_protocol_backend/src/chains/solana/tx.rs`, rewritten without `solana-*` types. The
//! backend's unchecked `MintTo` (tag 7) is deliberately not copied: every token instruction
//! here is a `*Checked` one with `decimals == 8`.

use std::collections::BTreeMap;

pub mod builders;
pub mod shape;

#[cfg(test)]
mod golden;

/// A Solana address (32 bytes, base58 in text).
pub type Pubkey = [u8; 32];
/// An Ed25519 signature (64 bytes, base58 in text; the first one is the transaction id).
pub type Signature = [u8; 64];

// ---------------------------------------------------------------------------
// Program ids (F: S14 section 1 and S4/S13 in chain-solana.md). The byte arrays are checked
// against their base58 literals by `tests::program_ids_match_their_base58_literals`.
// ---------------------------------------------------------------------------

/// System Program: 32 zero bytes (`1111...1111`).
pub const SYSTEM_PROGRAM_ID: Pubkey = [0u8; 32];
pub const SYSTEM_PROGRAM_B58: &str = "11111111111111111111111111111111";

pub const TOKEN_PROGRAM_B58: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN_PROGRAM_ID: Pubkey = [
    0x06, 0xdd, 0xf6, 0xe1, 0xd7, 0x65, 0xa1, 0x93, 0xd9, 0xcb, 0xe1, 0x46, 0xce, 0xeb, 0x79, 0xac,
    0x1c, 0xb4, 0x85, 0xed, 0x5f, 0x5b, 0x37, 0x91, 0x3a, 0x8c, 0xf5, 0x85, 0x7e, 0xff, 0x00, 0xa9,
];

pub const TOKEN_2022_PROGRAM_B58: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const TOKEN_2022_PROGRAM_ID: Pubkey = [
    0x06, 0xdd, 0xf6, 0xe1, 0xee, 0x75, 0x8f, 0xde, 0x18, 0x42, 0x5d, 0xbc, 0xe4, 0x6c, 0xcd, 0xda,
    0xb6, 0x1a, 0xfc, 0x4d, 0x83, 0xb9, 0x0d, 0x27, 0xfe, 0xbd, 0xf9, 0x28, 0xd8, 0xa1, 0x8b, 0xfc,
];

pub const ATA_PROGRAM_B58: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub const ATA_PROGRAM_ID: Pubkey = [
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83,
    0x0b, 0x5a, 0x13, 0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
];

/// Memo v3. **U (blocker `solana-memo-program-version-unconfirmed`):** the note's source S4
/// also lists v1 (`Memo1Uhk...`) and v4 (`Memo4c2p...`) and does not say which is deployed on
/// mainnet-beta, so every builder and the shape checker take the memo program id as a
/// parameter (a profile field). This constant is the profile default only, never a fact.
pub const MEMO_PROGRAM_V3_B58: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
pub const MEMO_PROGRAM_V3_ID: Pubkey = [
    0x05, 0x4a, 0x53, 0x5a, 0x99, 0x29, 0x21, 0x06, 0x4d, 0x24, 0xe8, 0x71, 0x60, 0xda, 0x38, 0x7c,
    0x7c, 0x35, 0xb5, 0xdd, 0xbc, 0x92, 0xbb, 0x81, 0xe4, 0x1f, 0xa8, 0x40, 0x41, 0x05, 0x44, 0x8d,
];

pub const COMPUTE_BUDGET_PROGRAM_B58: &str = "ComputeBudget111111111111111111111111111111";
pub const COMPUTE_BUDGET_PROGRAM_ID: Pubkey = [
    0x03, 0x06, 0x46, 0x6f, 0xe5, 0x21, 0x17, 0x32, 0xff, 0xec, 0xad, 0xba, 0x72, 0xc3, 0x9b, 0xe7,
    0xbc, 0x8c, 0xe5, 0xbb, 0xc5, 0xf7, 0x12, 0x6b, 0x2c, 0x43, 0x9b, 0x3a, 0x40, 0x00, 0x00, 0x00,
];

/// RecentBlockhashes sysvar, read-only account of `AdvanceNonceAccount`.
pub const RECENT_BLOCKHASHES_SYSVAR_B58: &str = "SysvarRecentB1ockHashes11111111111111111111";
pub const RECENT_BLOCKHASHES_SYSVAR_ID: Pubkey = [
    0x06, 0xa7, 0xd5, 0x17, 0x19, 0x2c, 0x56, 0x8e, 0xe0, 0x8a, 0x84, 0x5f, 0x73, 0xd2, 0x97, 0x88,
    0xcf, 0x03, 0x5c, 0x31, 0x45, 0xb2, 0x1a, 0xb3, 0x44, 0xd8, 0x06, 0x2e, 0xa9, 0x40, 0x00, 0x00,
];

/// Decimals of the escrow mint: fixed at `InitializeMint2`, 1 base unit = 1 e8s.
pub const TOKEN_DECIMALS: u8 = 8;

/// Token-program instruction tags (F, S3; shared by both token programs).
pub const TAG_TRANSFER: u8 = 3;
pub const TAG_MINT_TO: u8 = 7;
pub const TAG_BURN: u8 = 8;
pub const TAG_TRANSFER_CHECKED: u8 = 12;
pub const TAG_MINT_TO_CHECKED: u8 = 14;
pub const TAG_BURN_CHECKED: u8 = 15;
/// S3 lists a `Batch` tag. **U:** whether it is deployed; it is rejected either way.
pub const TAG_BATCH: u8 = 255;
/// ATA program `CreateIdempotent` (Create = 0, CreateIdempotent = 1).
pub const ATA_CREATE_IDEMPOTENT_TAG: u8 = 1;
/// `SystemInstruction::AdvanceNonceAccount`, bincode `u32` little-endian.
pub const SYSTEM_ADVANCE_NONCE_TAG: u32 = 4;

/// Largest legal Solana transaction (packet) in bytes.
pub const MAX_TRANSACTION_BYTES: usize = 1232;

/// The two allowed values of the profile's `token_program` field (note section 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, candid::CandidType)]
pub enum TokenProgram {
    /// Classic SPL Token (`Tokenkeg...`).
    Classic,
    /// Token-2022 (`TokenzQd...`); the note's recommendation.
    Token2022,
}

impl TokenProgram {
    pub fn id(&self) -> Pubkey {
        match self {
            TokenProgram::Classic => TOKEN_PROGRAM_ID,
            TokenProgram::Token2022 => TOKEN_2022_PROGRAM_ID,
        }
    }

    pub fn from_id(id: &Pubkey) -> Option<TokenProgram> {
        if *id == TOKEN_PROGRAM_ID {
            Some(TokenProgram::Classic)
        } else if *id == TOKEN_2022_PROGRAM_ID {
            Some(TokenProgram::Token2022)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxError {
    Base58(String),
    WrongLength { what: &'static str, expected: usize, found: usize },
    /// A compact-u16 is truncated, longer than 3 bytes or above `u16::MAX`.
    CompactU16,
    TooManyAccounts,
    TooManyInstructions,
    DataTooLong,
    PacketTooLarge { size: usize },
    Truncated,
    TrailingBytes,
    /// A version byte other than legacy or v0.
    UnsupportedVersion(u8),
    /// A v0 message with address-table lookups (not accepted in v1).
    AddressTableLookups,
    BadHeader(&'static str),
    BadProgramIndex,
    BadAccountIndex,
    DuplicateAccountKey,
    ZeroSignatures,
    SignatureCountMismatch { header: usize, wire: usize },
    /// No bump seed gave an off-curve address (probability about 2^-256).
    NoViablePda,
    InvalidAmount(&'static str),
    InvalidRecipient(&'static str),
    InvalidProfile(&'static str),
    MissingSignature(Pubkey),
    /// A memo longer than the memo program or packet can carry.
    MemoTooLong,
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// ---------------------------------------------------------------------------
// Base58
// ---------------------------------------------------------------------------

pub fn pubkey_to_base58(key: &Pubkey) -> String {
    bs58::encode(key).into_string()
}

pub fn signature_to_base58(sig: &Signature) -> String {
    bs58::encode(sig).into_string()
}

/// Decode a base58 address. Rejects anything that is not exactly 32 bytes.
pub fn pubkey_from_base58(text: &str) -> Result<Pubkey, TxError> {
    let bytes = bs58::decode(text).into_vec().map_err(|e| TxError::Base58(e.to_string()))?;
    Pubkey::try_from(bytes.as_slice()).map_err(|_| TxError::WrongLength { what: "address", expected: 32, found: bytes.len() })
}

/// Decode a base58 signature. Rejects anything that is not exactly 64 bytes.
pub fn signature_from_base58(text: &str) -> Result<Signature, TxError> {
    let bytes = bs58::decode(text).into_vec().map_err(|e| TxError::Base58(e.to_string()))?;
    Signature::try_from(bytes.as_slice()).map_err(|_| TxError::WrongLength { what: "signature", expected: 64, found: bytes.len() })
}

// ---------------------------------------------------------------------------
// Program-derived and associated token addresses
// ---------------------------------------------------------------------------

/// Whether `bytes` decompress to a point on the ed25519 curve. The same call Solana's
/// `bytes_are_curve_point` makes (`CompressedEdwardsY::decompress`).
pub fn is_on_curve(bytes: &Pubkey) -> bool {
    curve25519_dalek::edwards::CompressedEdwardsY(*bytes).decompress().is_some()
}

const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

/// `find_program_address`: try bump seeds 255 down to 0 and return the first
/// `sha256(seeds || bump || program_id || "ProgramDerivedAddress")` that is off the curve.
pub fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Option<(Pubkey, u8)> {
    use sha2::{Digest as _, Sha256};
    for bump in (0..=255u8).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program_id);
        h.update(PDA_MARKER);
        let address: Pubkey = h.finalize().into();
        if !is_on_curve(&address) {
            return Some((address, bump));
        }
    }
    None
}

/// The associated token account of `(wallet, token_program, mint)`. The token program id is
/// a seed, so classic and Token-2022 give different addresses for the same wallet and mint.
pub fn derive_ata(wallet: &Pubkey, token_program: &Pubkey, mint: &Pubkey) -> Result<Pubkey, TxError> {
    find_program_address(&[wallet.as_slice(), token_program.as_slice(), mint.as_slice()], &ATA_PROGRAM_ID)
        .map(|(address, _bump)| address)
        .ok_or(TxError::NoViablePda)
}

// ---------------------------------------------------------------------------
// compact-u16
// ---------------------------------------------------------------------------

/// Solana compact-u16 (short_vec length): little-endian base-128, 1 to 3 bytes.
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
            return out;
        }
    }
}

/// Decode a compact-u16 from the front of `bytes`: `(value, bytes_consumed)`. Rejects a
/// truncated varint, a 4th byte, a value above `u16::MAX`, and any non-canonical encoding
/// (a longer encoding of a value that fits in fewer bytes), as the runtime does.
pub fn decode_compact_u16(bytes: &[u8]) -> Result<(u16, usize), TxError> {
    let mut value: u32 = 0;
    for (i, &byte) in bytes.iter().enumerate().take(3) {
        value |= ((byte & 0x7f) as u32) << (i * 7);
        if byte & 0x80 == 0 {
            if value > u16::MAX as u32 {
                return Err(TxError::CompactU16);
            }
            // Canonical: the last byte of a multi-byte encoding is never zero.
            if i > 0 && byte == 0 {
                return Err(TxError::CompactU16);
            }
            return Ok((value as u16, i + 1));
        }
    }
    Err(TxError::CompactU16)
}

fn compact_len(len: usize, err: TxError) -> Result<Vec<u8>, TxError> {
    u16::try_from(len).map(encode_compact_u16).map_err(|_| err)
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn writable(pubkey: Pubkey, is_signer: bool) -> Self {
        AccountMeta { pubkey, is_signer, is_writable: true }
    }

    pub fn readonly(pubkey: Pubkey, is_signer: bool) -> Self {
        AccountMeta { pubkey, is_signer, is_writable: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

/// `System::AdvanceNonceAccount`. Accounts: nonce (writable), RecentBlockhashes sysvar
/// (read-only), authority (read-only signer). Data: the `u32` little-endian tag 4. It must
/// be the first instruction of a durable-nonce transaction.
pub fn advance_nonce_instruction(nonce: &Pubkey, authority: &Pubkey) -> Instruction {
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable(*nonce, false),
            AccountMeta::readonly(RECENT_BLOCKHASHES_SYSVAR_ID, false),
            AccountMeta::readonly(*authority, true),
        ],
        data: SYSTEM_ADVANCE_NONCE_TAG.to_le_bytes().to_vec(),
    }
}

/// ATA `CreateIdempotent`. Accounts: funder (writable signer), the ATA (writable), the
/// wallet, the mint, the System Program and the token program (all read-only). Data: `[1]`.
/// The ATA address is derived here from `(wallet, token_program, mint)`.
pub fn create_ata_idempotent_instruction(
    funder: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Result<Instruction, TxError> {
    let ata = derive_ata(wallet, token_program, mint)?;
    Ok(Instruction {
        program_id: ATA_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable(*funder, true),
            AccountMeta::writable(ata, false),
            AccountMeta::readonly(*wallet, false),
            AccountMeta::readonly(*mint, false),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::readonly(*token_program, false),
        ],
        data: vec![ATA_CREATE_IDEMPOTENT_TAG],
    })
}

fn checked_data(tag: u8, amount: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(10);
    data.push(tag);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(TOKEN_DECIMALS);
    data
}

/// `MintToChecked`. Accounts: mint (writable), destination token account (writable), mint
/// authority (read-only signer). Data: `[14, amount u64 LE, 8]`.
pub fn mint_to_checked_instruction(
    token_program: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    mint_authority: &Pubkey,
    amount: u64,
) -> Instruction {
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::writable(*mint, false),
            AccountMeta::writable(*destination, false),
            AccountMeta::readonly(*mint_authority, true),
        ],
        data: checked_data(TAG_MINT_TO_CHECKED, amount),
    }
}

/// `BurnChecked`. Accounts: token account (writable), mint (writable), owner (read-only
/// signer). Data: `[15, amount u64 LE, 8]`.
pub fn burn_checked_instruction(
    token_program: &Pubkey,
    token_account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) -> Instruction {
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::writable(*token_account, false),
            AccountMeta::writable(*mint, false),
            AccountMeta::readonly(*owner, true),
        ],
        data: checked_data(TAG_BURN_CHECKED, amount),
    }
}

/// `TransferChecked`. Accounts: source (writable), mint (read-only), destination (writable),
/// authority (read-only signer); exactly four, no multisig tail. Data: `[12, amount u64 LE, 8]`.
pub fn transfer_checked_instruction(
    token_program: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::writable(*source, false),
            AccountMeta::readonly(*mint, false),
            AccountMeta::writable(*destination, false),
            AccountMeta::readonly(*authority, true),
        ],
        data: checked_data(TAG_TRANSFER_CHECKED, amount),
    }
}

/// A Memo instruction with **zero account metas** (the note's shape).
pub fn memo_instruction(memo_program: &Pubkey, text: &[u8]) -> Instruction {
    Instruction { program_id: *memo_program, accounts: vec![], data: text.to_vec() }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageVersion {
    Legacy,
    /// v0 without address-table lookups (the only v0 form this crate builds or accepts).
    V0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageHeader {
    pub num_required_signatures: u8,
    pub num_readonly_signed_accounts: u8,
    pub num_readonly_unsigned_accounts: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub version: MessageVersion,
    pub header: MessageHeader,
    pub account_keys: Vec<Pubkey>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledInstruction>,
}

impl Message {
    /// The keys that must sign, in signature order: `account_keys[..num_required_signatures]`.
    pub fn required_signers(&self) -> &[Pubkey] {
        let n = (self.header.num_required_signatures as usize).min(self.account_keys.len());
        &self.account_keys[..n]
    }

    /// Whether the key at `index` is a signer.
    pub fn is_signer_index(&self, index: usize) -> bool {
        index < self.header.num_required_signatures as usize
    }

    /// The program id of a compiled instruction.
    pub fn program_id(&self, ix: &CompiledInstruction) -> Option<&Pubkey> {
        self.account_keys.get(ix.program_id_index as usize)
    }

    /// The account keys of a compiled instruction, in instruction order.
    pub fn instruction_accounts(&self, ix: &CompiledInstruction) -> Option<Vec<Pubkey>> {
        ix.accounts.iter().map(|&i| self.account_keys.get(i as usize).copied()).collect()
    }
}

/// Compile instructions into a message the way the Solana SDK's `Message::new_with_blockhash`
/// does: the fee payer first, then writable signers, read-only signers, writable non-signers
/// and read-only non-signers, each group sorted ascending by key bytes. Pinned to the SDK
/// ordering so a message built here is byte-identical to one the SDK would build from the same
/// instructions (the future SDK golden test, note section 8, can use these vectors unchanged).
pub fn compile_message(
    version: MessageVersion,
    instructions: &[Instruction],
    payer: &Pubkey,
    recent_blockhash: [u8; 32],
) -> Result<Message, TxError> {
    #[derive(Default)]
    struct Meta {
        signer: bool,
        writable: bool,
    }
    let mut metas: BTreeMap<Pubkey, Meta> = BTreeMap::new();
    for ix in instructions {
        metas.entry(ix.program_id).or_default();
        for a in &ix.accounts {
            let m = metas.entry(a.pubkey).or_default();
            m.signer |= a.is_signer;
            m.writable |= a.is_writable;
        }
    }
    metas.remove(payer);

    let mut writable_signers = vec![*payer];
    let (mut readonly_signers, mut writable_others, mut readonly_others) = (vec![], vec![], vec![]);
    // BTreeMap iteration is ascending by key bytes, which is the SDK's within-group order.
    for (key, m) in &metas {
        match (m.signer, m.writable) {
            (true, true) => writable_signers.push(*key),
            (true, false) => readonly_signers.push(*key),
            (false, true) => writable_others.push(*key),
            (false, false) => readonly_others.push(*key),
        }
    }
    let to_u8 = |n: usize| u8::try_from(n).map_err(|_| TxError::TooManyAccounts);
    let header = MessageHeader {
        num_required_signatures: to_u8(writable_signers.len() + readonly_signers.len())?,
        num_readonly_signed_accounts: to_u8(readonly_signers.len())?,
        num_readonly_unsigned_accounts: to_u8(readonly_others.len())?,
    };
    let mut account_keys = writable_signers;
    account_keys.extend(readonly_signers);
    account_keys.extend(writable_others);
    account_keys.extend(readonly_others);
    if account_keys.len() > 256 {
        return Err(TxError::TooManyAccounts);
    }

    let index_of: BTreeMap<Pubkey, u8> =
        account_keys.iter().enumerate().filter_map(|(i, k)| u8::try_from(i).ok().map(|i| (*k, i))).collect();
    let mut compiled = Vec::with_capacity(instructions.len());
    for ix in instructions {
        let program_id_index = *index_of.get(&ix.program_id).ok_or(TxError::BadProgramIndex)?;
        let accounts =
            ix.accounts.iter().map(|a| index_of.get(&a.pubkey).copied().ok_or(TxError::BadAccountIndex)).collect::<Result<Vec<u8>, _>>()?;
        compiled.push(CompiledInstruction { program_id_index, accounts, data: ix.data.clone() });
    }
    Ok(Message { version, header, account_keys, recent_blockhash, instructions: compiled })
}

/// Serialize a message to its wire bytes (the bytes that are signed).
///
/// Legacy: `[header 3][compact keys][keys*32][blockhash 32][compact ix count]` then per
/// instruction `[program index][compact n][account indexes][compact len][data]`.
/// v0 adds the prefix byte `0x80` and ends with a compact-u16 zero (no address-table lookups).
pub fn serialize_message(m: &Message) -> Result<Vec<u8>, TxError> {
    let mut out = Vec::new();
    if m.version == MessageVersion::V0 {
        out.push(0x80);
    }
    out.push(m.header.num_required_signatures);
    out.push(m.header.num_readonly_signed_accounts);
    out.push(m.header.num_readonly_unsigned_accounts);
    out.extend(compact_len(m.account_keys.len(), TxError::TooManyAccounts)?);
    for k in &m.account_keys {
        out.extend_from_slice(k);
    }
    out.extend_from_slice(&m.recent_blockhash);
    out.extend(compact_len(m.instructions.len(), TxError::TooManyInstructions)?);
    for ix in &m.instructions {
        out.push(ix.program_id_index);
        out.extend(compact_len(ix.accounts.len(), TxError::TooManyAccounts)?);
        out.extend_from_slice(&ix.accounts);
        out.extend(compact_len(ix.data.len(), TxError::DataTooLong)?);
        out.extend_from_slice(&ix.data);
    }
    if m.version == MessageVersion::V0 {
        out.push(0x00);
    }
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], TxError> {
        let end = self.pos.checked_add(n).ok_or(TxError::Truncated)?;
        let s = self.bytes.get(self.pos..end).ok_or(TxError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, TxError> {
        self.take(1)?.first().copied().ok_or(TxError::Truncated)
    }

    fn compact(&mut self) -> Result<usize, TxError> {
        let rest = self.bytes.get(self.pos..).ok_or(TxError::Truncated)?;
        let (value, used) = decode_compact_u16(rest)?;
        self.pos += used;
        Ok(value as usize)
    }

    fn array32(&mut self) -> Result<[u8; 32], TxError> {
        let s = self.take(32)?;
        <[u8; 32]>::try_from(s).map_err(|_| TxError::Truncated)
    }
}

/// Strictly decode message bytes (legacy or v0 **without** address-table lookups). Rejects
/// trailing bytes, an unknown version, a malformed header, duplicate account keys, an
/// out-of-range or fee-payer program index, and any account index out of range.
pub fn decode_message(bytes: &[u8]) -> Result<Message, TxError> {
    let mut r = Reader { bytes, pos: 0 };
    let first = r.u8()?;
    let (version, num_required_signatures) = if first & 0x80 != 0 {
        let v = first & 0x7f;
        if v != 0 {
            return Err(TxError::UnsupportedVersion(v));
        }
        (MessageVersion::V0, r.u8()?)
    } else {
        (MessageVersion::Legacy, first)
    };
    let header = MessageHeader {
        num_required_signatures,
        num_readonly_signed_accounts: r.u8()?,
        num_readonly_unsigned_accounts: r.u8()?,
    };
    if header.num_required_signatures == 0 {
        return Err(TxError::BadHeader("no required signature"));
    }
    if header.num_readonly_signed_accounts >= header.num_required_signatures {
        return Err(TxError::BadHeader("the fee payer must be a writable signer"));
    }
    let key_count = r.compact()?;
    if key_count > 256 {
        return Err(TxError::TooManyAccounts);
    }
    if (header.num_required_signatures as usize) + (header.num_readonly_unsigned_accounts as usize) > key_count {
        return Err(TxError::BadHeader("header counts exceed the account keys"));
    }
    let mut account_keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        let k = r.array32()?;
        if account_keys.contains(&k) {
            return Err(TxError::DuplicateAccountKey);
        }
        account_keys.push(k);
    }
    let recent_blockhash = r.array32()?;
    let ix_count = r.compact()?;
    let mut instructions = Vec::with_capacity(ix_count.min(64));
    for _ in 0..ix_count {
        let program_id_index = r.u8()?;
        // Index 0 is the fee payer, which can never be a program.
        if program_id_index == 0 || (program_id_index as usize) >= key_count {
            return Err(TxError::BadProgramIndex);
        }
        let n = r.compact()?;
        let accounts = r.take(n)?.to_vec();
        if accounts.iter().any(|&i| (i as usize) >= key_count) {
            return Err(TxError::BadAccountIndex);
        }
        let len = r.compact()?;
        let data = r.take(len)?.to_vec();
        instructions.push(CompiledInstruction { program_id_index, accounts, data });
    }
    if version == MessageVersion::V0 {
        let lookups = r.compact()?;
        if lookups != 0 {
            return Err(TxError::AddressTableLookups);
        }
    }
    if r.pos != bytes.len() {
        return Err(TxError::TrailingBytes);
    }
    Ok(Message { version, header, account_keys, recent_blockhash, instructions })
}

/// The first instruction as `AdvanceNonceAccount`, if it is one: `(nonce account, authority)`.
/// A durable-nonce transaction is recognized by this leading instruction, and its
/// `recent_blockhash` must equal the stored nonce value (note section 4).
pub fn leading_advance_nonce(m: &Message) -> Option<(Pubkey, Pubkey)> {
    let ix = m.instructions.first()?;
    if m.program_id(ix)? != &SYSTEM_PROGRAM_ID || ix.data != SYSTEM_ADVANCE_NONCE_TAG.to_le_bytes() {
        return None;
    }
    let accounts = m.instruction_accounts(ix)?;
    let authority_index = *ix.accounts.get(2)?;
    match accounts.as_slice() {
        [nonce, sysvar, authority] if *sysvar == RECENT_BLOCKHASHES_SYSVAR_ID && m.is_signer_index(authority_index as usize) => {
            Some((*nonce, *authority))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Wire transactions and signatures
// ---------------------------------------------------------------------------

/// `[compact-u16 signature count][signature * 64][message]`. The signatures must already be in
/// the message's required-signer order (see [`order_signatures_by_signer`]).
pub fn assemble_wire_tx(signatures: &[Signature], message_bytes: &[u8]) -> Result<Vec<u8>, TxError> {
    let mut out = Vec::with_capacity(3 + signatures.len() * 64 + message_bytes.len());
    out.extend(compact_len(signatures.len(), TxError::TooManyAccounts)?);
    for s in signatures {
        out.extend_from_slice(s);
    }
    out.extend_from_slice(message_bytes);
    Ok(out)
}

/// Split a wire transaction into its signatures and its message bytes.
pub fn split_wire_transaction(wire: &[u8]) -> Result<(Vec<Signature>, &[u8]), TxError> {
    let (count, used) = decode_compact_u16(wire)?;
    let count = count as usize;
    let sigs_end = count.checked_mul(64).and_then(|n| n.checked_add(used)).ok_or(TxError::Truncated)?;
    let sig_bytes = wire.get(used..sigs_end).ok_or(TxError::Truncated)?;
    let signatures: Vec<Signature> = sig_bytes.chunks_exact(64).filter_map(|c| Signature::try_from(c).ok()).collect();
    let message = wire.get(sigs_end..).ok_or(TxError::Truncated)?;
    Ok((signatures, message))
}

/// Decode a wire transaction (as `getTransaction` returns it in base64): the signatures, the
/// message, and the exact message bytes. The signature count must equal the header's.
pub fn decode_wire_transaction(wire: &[u8]) -> Result<(Vec<Signature>, Message, &[u8]), TxError> {
    let (signatures, message_bytes) = split_wire_transaction(wire)?;
    let message = decode_message(message_bytes)?;
    if signatures.len() != message.header.num_required_signatures as usize {
        return Err(TxError::SignatureCountMismatch {
            header: message.header.num_required_signatures as usize,
            wire: signatures.len(),
        });
    }
    Ok((signatures, message, message_bytes))
}

/// The transaction id: the base58 of the FIRST signature of a wire transaction. Deterministic
/// from the signed bytes, so it can be computed locally before broadcast.
pub fn first_signature_base58(wire: &[u8]) -> Result<String, TxError> {
    let (signatures, _) = split_wire_transaction(wire)?;
    signatures.first().map(signature_to_base58).ok_or(TxError::ZeroSignatures)
}

/// Resolve signatures for a message's required signers, in `account_keys[..n]` order. `signers`
/// maps each signing key to its signature (order irrelevant). Errors naming the first required
/// signer with no signature.
pub fn order_signatures_by_signer(message: &Message, signers: &[(Pubkey, Signature)]) -> Result<Vec<Signature>, TxError> {
    message
        .required_signers()
        .iter()
        .map(|key| signers.iter().find(|(pk, _)| pk == key).map(|(_, sig)| *sig).ok_or(TxError::MissingSignature(*key)))
        .collect()
}

/// Reject a transaction that cannot fit one packet once it carries `signature_count` signatures.
pub fn check_packet_size(message_bytes: &[u8], signature_count: usize) -> Result<(), TxError> {
    let size = 1 + signature_count * 64 + message_bytes.len();
    if size > MAX_TRANSACTION_BYTES {
        return Err(TxError::PacketTooLarge { size });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_ids_match_their_base58_literals() {
        for (b58, id) in [
            (SYSTEM_PROGRAM_B58, SYSTEM_PROGRAM_ID),
            (TOKEN_PROGRAM_B58, TOKEN_PROGRAM_ID),
            (TOKEN_2022_PROGRAM_B58, TOKEN_2022_PROGRAM_ID),
            (ATA_PROGRAM_B58, ATA_PROGRAM_ID),
            (MEMO_PROGRAM_V3_B58, MEMO_PROGRAM_V3_ID),
            (COMPUTE_BUDGET_PROGRAM_B58, COMPUTE_BUDGET_PROGRAM_ID),
            (RECENT_BLOCKHASHES_SYSVAR_B58, RECENT_BLOCKHASHES_SYSVAR_ID),
        ] {
            assert_eq!(pubkey_from_base58(b58).unwrap(), id, "{b58}");
            assert_eq!(pubkey_to_base58(&id), b58);
        }
    }

    #[test]
    fn token_program_enum_round_trips_and_has_exactly_two_values() {
        assert_eq!(TokenProgram::Classic.id(), TOKEN_PROGRAM_ID);
        assert_eq!(TokenProgram::Token2022.id(), TOKEN_2022_PROGRAM_ID);
        assert_eq!(TokenProgram::from_id(&TOKEN_PROGRAM_ID), Some(TokenProgram::Classic));
        assert_eq!(TokenProgram::from_id(&TOKEN_2022_PROGRAM_ID), Some(TokenProgram::Token2022));
        assert_eq!(TokenProgram::from_id(&ATA_PROGRAM_ID), None);
        assert_eq!(TokenProgram::from_id(&SYSTEM_PROGRAM_ID), None);
    }

    #[test]
    fn base58_rejects_wrong_lengths_and_alphabet() {
        assert!(pubkey_from_base58("11111111111111111111111111111112").is_ok());
        assert!(matches!(pubkey_from_base58("1111"), Err(TxError::WrongLength { expected: 32, .. })));
        assert!(matches!(pubkey_from_base58("0OIl"), Err(TxError::Base58(_))));
        assert!(matches!(signature_from_base58(TOKEN_PROGRAM_B58), Err(TxError::WrongLength { expected: 64, .. })));
        let sig = [0xabu8; 64];
        assert_eq!(signature_from_base58(&signature_to_base58(&sig)).unwrap(), sig);
    }

    /// Known answer from the SPL ATA test suite (also pinned in the backend's `tests_tx.rs`
    /// and reproduced by the independent Python reference): classic token program,
    /// canonical bump 255.
    #[test]
    fn derive_ata_matches_the_spl_known_answer() {
        let owner = pubkey_from_base58("1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG").unwrap();
        let mint = pubkey_from_base58("8N6gdBxJaZUG9cBnSSaHDsx7vMeQ4VR1LmCmk9SCu38s").unwrap();
        let ata = derive_ata(&owner, &TOKEN_PROGRAM_ID, &mint).unwrap();
        assert_eq!(pubkey_to_base58(&ata), "3kP4RCoX8u1PhyiUkuNpFfbUmFF32ZC9uscL7H84Xs2u");
        let (again, bump) = find_program_address(&[&owner, &TOKEN_PROGRAM_ID, &mint], &ATA_PROGRAM_ID).unwrap();
        assert_eq!((again, bump), (ata, 255));
        assert!(!is_on_curve(&ata), "an ATA is a PDA, so it is off the curve");
    }

    #[test]
    fn the_token_program_is_a_seed_of_the_ata() {
        let wallet = [0x77u8; 32];
        let mint = [0x44u8; 32];
        let classic = derive_ata(&wallet, &TOKEN_PROGRAM_ID, &mint).unwrap();
        let t22 = derive_ata(&wallet, &TOKEN_2022_PROGRAM_ID, &mint).unwrap();
        assert_ne!(classic, t22);
        assert_ne!(classic, derive_ata(&[0x78u8; 32], &TOKEN_PROGRAM_ID, &mint).unwrap());
        assert_ne!(classic, derive_ata(&wallet, &TOKEN_PROGRAM_ID, &[0x45u8; 32]).unwrap());
    }

    #[test]
    fn is_on_curve_agrees_with_known_points() {
        // The ed25519 base point (compressed y = 4/5) is on the curve.
        let mut base = [0x66u8; 32];
        base[0] = 0x58;
        assert!(is_on_curve(&base));
        // The System Program key (all zeros, y = 0) is on the curve too; a PDA never is.
        assert!(is_on_curve(&[0u8; 32]));
    }

    #[test]
    fn compact_u16_round_trips_and_rejects_bad_encodings() {
        for v in [0u16, 1, 127, 128, 255, 300, 16_383, 16_384, 65_535] {
            let enc = encode_compact_u16(v);
            assert_eq!(decode_compact_u16(&enc), Ok((v, enc.len())), "{v}");
        }
        assert_eq!(encode_compact_u16(127), vec![0x7f]);
        assert_eq!(encode_compact_u16(128), vec![0x80, 0x01]);
        assert_eq!(encode_compact_u16(65_535), vec![0xff, 0xff, 0x03]);
        assert_eq!(decode_compact_u16(&[0x80]), Err(TxError::CompactU16), "truncated");
        assert_eq!(decode_compact_u16(&[]), Err(TxError::CompactU16));
        assert_eq!(decode_compact_u16(&[0xff, 0xff, 0x04]), Err(TxError::CompactU16), "above u16");
        assert_eq!(decode_compact_u16(&[0x81, 0x80, 0x80, 0x00]), Err(TxError::CompactU16), "4 bytes");
        assert_eq!(decode_compact_u16(&[0x80, 0x00]), Err(TxError::CompactU16), "non-canonical zero");
        assert_eq!(decode_compact_u16(&[0xff, 0x00]), Err(TxError::CompactU16), "non-canonical 127");
    }

    #[test]
    fn instruction_encoders_use_the_checked_tags_and_eight_decimals() {
        let tp = TOKEN_2022_PROGRAM_ID;
        let (a, b, c, d) = ([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);
        let amount = 0x0102_0304_0506_0708u64;
        let mint_to = mint_to_checked_instruction(&tp, &a, &b, &c, amount);
        assert_eq!(mint_to.data, vec![14, 8, 7, 6, 5, 4, 3, 2, 1, 8]);
        let burn = burn_checked_instruction(&tp, &a, &b, &c, amount);
        assert_eq!(burn.data, vec![15, 8, 7, 6, 5, 4, 3, 2, 1, 8]);
        let transfer = transfer_checked_instruction(&tp, &a, &b, &c, &d, amount);
        assert_eq!(transfer.data, vec![12, 8, 7, 6, 5, 4, 3, 2, 1, 8]);
        assert_eq!(transfer.accounts.len(), 4);
        // Account roles: (pubkey, signer, writable).
        let roles = |ix: &Instruction| ix.accounts.iter().map(|m| (m.pubkey[0], m.is_signer, m.is_writable)).collect::<Vec<_>>();
        assert_eq!(roles(&mint_to), vec![(1, false, true), (2, false, true), (3, true, false)]);
        assert_eq!(roles(&burn), vec![(1, false, true), (2, false, true), (3, true, false)]);
        assert_eq!(roles(&transfer), vec![(1, false, true), (2, false, false), (3, false, true), (4, true, false)]);
        for ix in [&mint_to, &burn, &transfer] {
            assert_eq!(ix.program_id, tp);
            assert!(![TAG_TRANSFER, TAG_MINT_TO, TAG_BURN, TAG_BATCH].contains(&ix.data[0]), "never an unchecked tag");
        }
    }

    #[test]
    fn advance_nonce_and_create_ata_layouts() {
        let (nonce, authority) = ([5u8; 32], [6u8; 32]);
        let adv = advance_nonce_instruction(&nonce, &authority);
        assert_eq!(adv.program_id, SYSTEM_PROGRAM_ID);
        assert_eq!(adv.data, vec![4, 0, 0, 0]);
        assert_eq!(
            adv.accounts,
            vec![
                AccountMeta::writable(nonce, false),
                AccountMeta::readonly(RECENT_BLOCKHASHES_SYSVAR_ID, false),
                AccountMeta::readonly(authority, true)
            ]
        );
        let funder = [7u8; 32];
        let wallet = [8u8; 32];
        let mint = [9u8; 32];
        let create = create_ata_idempotent_instruction(&funder, &wallet, &mint, &TOKEN_PROGRAM_ID).unwrap();
        assert_eq!(create.program_id, ATA_PROGRAM_ID);
        assert_eq!(create.data, vec![1]);
        let ata = derive_ata(&wallet, &TOKEN_PROGRAM_ID, &mint).unwrap();
        assert_eq!(
            create.accounts,
            vec![
                AccountMeta::writable(funder, true),
                AccountMeta::writable(ata, false),
                AccountMeta::readonly(wallet, false),
                AccountMeta::readonly(mint, false),
                AccountMeta::readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::readonly(TOKEN_PROGRAM_ID, false),
            ]
        );
    }

    fn sample_instructions() -> Vec<Instruction> {
        vec![
            advance_nonce_instruction(&[0x55; 32], &[0x11; 32]),
            memo_instruction(&MEMO_PROGRAM_V3_ID, b"hi"),
        ]
    }

    #[test]
    fn compile_orders_keys_like_the_sdk_and_round_trips() {
        let msg = compile_message(MessageVersion::Legacy, &sample_instructions(), &[0x11; 32], [0x66; 32]).unwrap();
        // payer (writable signer), nonce (writable), then read-only ascending by bytes:
        // system (00..), memo (05 4a..), recent blockhashes (06 a7..).
        assert_eq!(
            msg.account_keys,
            vec![[0x11; 32], [0x55; 32], SYSTEM_PROGRAM_ID, MEMO_PROGRAM_V3_ID, RECENT_BLOCKHASHES_SYSVAR_ID]
        );
        assert_eq!(msg.header, MessageHeader { num_required_signatures: 1, num_readonly_signed_accounts: 0, num_readonly_unsigned_accounts: 3 });
        let bytes = serialize_message(&msg).unwrap();
        assert_eq!(decode_message(&bytes).unwrap(), msg);
        assert_eq!(leading_advance_nonce(&msg), Some(([0x55; 32], [0x11; 32])));
    }

    #[test]
    fn a_payer_that_is_also_an_instruction_account_is_not_duplicated() {
        let msg = compile_message(MessageVersion::Legacy, &sample_instructions(), &[0x11; 32], [0; 32]).unwrap();
        assert_eq!(msg.account_keys.iter().filter(|k| **k == [0x11; 32]).count(), 1);
        assert_eq!(msg.account_keys[0], [0x11; 32]);
    }

    #[test]
    fn v0_without_lookups_round_trips_and_adds_the_prefix_and_trailer() {
        let v0 = compile_message(MessageVersion::V0, &sample_instructions(), &[0x11; 32], [0x66; 32]).unwrap();
        let legacy = compile_message(MessageVersion::Legacy, &sample_instructions(), &[0x11; 32], [0x66; 32]).unwrap();
        let (b0, bl) = (serialize_message(&v0).unwrap(), serialize_message(&legacy).unwrap());
        assert_eq!(b0[0], 0x80);
        assert_eq!(&b0[1..b0.len() - 1], &bl[..]);
        assert_eq!(*b0.last().unwrap(), 0);
        assert_eq!(decode_message(&b0).unwrap(), v0);
    }

    #[test]
    fn decode_rejects_malformed_messages() {
        let msg = compile_message(MessageVersion::Legacy, &sample_instructions(), &[0x11; 32], [0x66; 32]).unwrap();
        let good = serialize_message(&msg).unwrap();
        assert!(decode_message(&good).is_ok());

        // Trailing byte.
        let mut t = good.clone();
        t.push(0);
        assert_eq!(decode_message(&t), Err(TxError::TrailingBytes));
        // Truncated at every length.
        for n in 0..good.len() {
            assert!(decode_message(&good[..n]).is_err(), "prefix of {n} bytes decoded");
        }
        // Unknown version byte.
        let mut v1 = good.clone();
        v1.insert(0, 0x81);
        assert_eq!(decode_message(&v1), Err(TxError::UnsupportedVersion(1)));
        // v0 with a non-empty lookup table list.
        let mut v0 = serialize_message(&compile_message(MessageVersion::V0, &sample_instructions(), &[0x11; 32], [0x66; 32]).unwrap()).unwrap();
        *v0.last_mut().unwrap() = 1;
        assert_eq!(decode_message(&v0), Err(TxError::AddressTableLookups));
        // Zero required signatures.
        let mut z = good.clone();
        z[0] = 0;
        assert!(matches!(decode_message(&z), Err(TxError::BadHeader(_))));
        // Duplicate account key.
        let mut d = msg.clone();
        d.account_keys[1] = d.account_keys[0];
        assert_eq!(decode_message(&serialize_message(&d).unwrap()), Err(TxError::DuplicateAccountKey));
        // Program index 0 (the fee payer) and out-of-range indexes.
        let mut p0 = msg.clone();
        p0.instructions[0].program_id_index = 0;
        assert_eq!(decode_message(&serialize_message(&p0).unwrap()), Err(TxError::BadProgramIndex));
        let mut pr = msg.clone();
        pr.instructions[0].program_id_index = 99;
        assert_eq!(decode_message(&serialize_message(&pr).unwrap()), Err(TxError::BadProgramIndex));
        let mut ar = msg.clone();
        ar.instructions[0].accounts[0] = 99;
        assert_eq!(decode_message(&serialize_message(&ar).unwrap()), Err(TxError::BadAccountIndex));
    }

    #[test]
    fn long_instruction_data_uses_a_two_byte_length_prefix() {
        let ix = Instruction { program_id: MEMO_PROGRAM_V3_ID, accounts: vec![], data: vec![0xab; 200] };
        let msg = compile_message(MessageVersion::Legacy, &[ix], &[0x11; 32], [0; 32]).unwrap();
        let bytes = serialize_message(&msg).unwrap();
        // header 3 + count 1 + 2 keys * 32 + blockhash 32 + ix count 1 + program index 1 + accounts count 1
        let off = 3 + 1 + 64 + 32 + 1 + 1 + 1;
        assert_eq!(&bytes[off..off + 2], &[0xc8, 0x01]);
        assert_eq!(bytes.len(), off + 2 + 200);
        assert_eq!(decode_message(&bytes).unwrap(), msg);
    }

    #[test]
    fn wire_transaction_round_trip_signature_order_and_id() {
        // Two required signers: payer 0x11 (writable) and a read-only signer 0x22.
        let ixs = vec![
            advance_nonce_instruction(&[0x55; 32], &[0x11; 32]),
            burn_checked_instruction(&TOKEN_PROGRAM_ID, &[0x99; 32], &[0x44; 32], &[0x22; 32], 5),
        ];
        let msg = compile_message(MessageVersion::Legacy, &ixs, &[0x11; 32], [0x66; 32]).unwrap();
        assert_eq!(msg.required_signers(), &[[0x11; 32], [0x22; 32]]);
        let sig_payer = [0xa1u8; 64];
        let sig_owner = [0xb2u8; 64];
        // Supplied out of order on purpose.
        let ordered = order_signatures_by_signer(&msg, &[([0x22; 32], sig_owner), ([0x11; 32], sig_payer)]).unwrap();
        assert_eq!(ordered, vec![sig_payer, sig_owner]);
        assert_eq!(
            order_signatures_by_signer(&msg, &[([0x11; 32], sig_payer)]),
            Err(TxError::MissingSignature([0x22; 32]))
        );
        let bytes = serialize_message(&msg).unwrap();
        let wire = assemble_wire_tx(&ordered, &bytes).unwrap();
        assert_eq!(wire[0], 2);
        let (sigs, back, mbytes) = decode_wire_transaction(&wire).unwrap();
        assert_eq!(sigs, ordered);
        assert_eq!(back, msg);
        assert_eq!(mbytes, &bytes[..]);
        assert_eq!(first_signature_base58(&wire).unwrap(), signature_to_base58(&sig_payer));
        // A wire transaction with the wrong signature count is rejected.
        let one = assemble_wire_tx(&ordered[..1], &bytes).unwrap();
        assert_eq!(decode_wire_transaction(&one), Err(TxError::SignatureCountMismatch { header: 2, wire: 1 }));
        assert_eq!(first_signature_base58(&[0]), Err(TxError::ZeroSignatures));
        assert!(first_signature_base58(&[1, 2, 3]).is_err());
    }

    #[test]
    fn packet_size_check() {
        assert!(check_packet_size(&vec![0; 1100], 2).is_ok());
        assert_eq!(check_packet_size(&vec![0; 1200], 2), Err(TxError::PacketTooLarge { size: 1329 }));
        assert!(check_packet_size(&vec![0; 1167], 1).is_ok());
        assert!(check_packet_size(&vec![0; 1168], 1).is_err());
    }
}
