//! Message builders (`chain-solana.md` sections 2.1 to 2.4). Every builder returns the exact
//! unsigned message bytes, which the caller persists BEFORE signing (note section 4): the
//! stored bytes are the identity of the attempt, and a consumed nonce is adopted only when the
//! landed message equals them.
//!
//! * [`build_mint_message`], 2.1: `[AdvanceNonce, ATA CreateIdempotent, MintToChecked, Memo]`;
//! * [`build_burn_message`], 2.3: `[AdvanceNonce, BurnChecked, Memo]`;
//! * [`build_refund_message`], 2.4: `[AdvanceNonce, (CreateIdempotent), Memo, TransferChecked]`;
//! * [`build_return_message`], 2.2: the **reference** shape a holder's wallet must produce
//!   (`[TransferChecked, Memo]`, user-built, normal blockhash). The bridge never builds or
//!   signs it; it exists so the shape checker and the future wallet connector (BB-39) share one
//!   golden vector.
//!
//! All messages are legacy (`MessageVersion::Legacy`), the token program is a parameter
//! (`TokenProgram`), every token instruction is a `*Checked` one with 8 decimals, and the
//! signer order follows the SDK compile order: the fee payer first, then read-only signers.

use super::*;
use crate::ids::{Hash32, KeyRole};
use crate::memo::{canister_memo, encode_return_memo, CanisterMemoKind, ReturnMemo};

/// The fixed per-instance facts every builder needs (a view of the Solana profile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxProfile {
    /// Exactly two allowed values; every ATA derivation uses its id.
    pub token_program: TokenProgram,
    /// Profile field (U: which Memo version is deployed; see [`MEMO_PROGRAM_V3_ID`]).
    pub memo_program: Pubkey,
    /// The escrow mint account (the address of the `mint-account` role key).
    pub mint: Pubkey,
    pub fee_payer: Pubkey,
    pub mint_authority: Pubkey,
    pub return_owner: Pubkey,
}

impl TxProfile {
    /// The roles must be three distinct keys, none of them the mint or a program/System id.
    pub fn validate(&self) -> Result<(), TxError> {
        let roles = [self.fee_payer, self.mint_authority, self.return_owner];
        for (i, a) in roles.iter().enumerate() {
            if roles[..i].contains(a) {
                return Err(TxError::InvalidProfile("role keys must be distinct"));
            }
            if *a == self.mint {
                return Err(TxError::InvalidProfile("a role key equals the mint"));
            }
            if is_reserved_key(a) || *a == self.memo_program {
                return Err(TxError::InvalidProfile("a role key is a program id"));
            }
        }
        if self.mint == SYSTEM_PROGRAM_ID || is_reserved_key(&self.mint) || self.mint == self.memo_program {
            return Err(TxError::InvalidProfile("the mint is a program id"));
        }
        if self.memo_program == SYSTEM_PROGRAM_ID {
            return Err(TxError::InvalidProfile("the memo program is the System Program"));
        }
        Ok(())
    }

    /// The canonical return ATA: derived from `(return-owner, token program, mint)`.
    pub fn return_ata(&self) -> Result<Pubkey, TxError> {
        derive_ata(&self.return_owner, &self.token_program.id(), &self.mint)
    }

    /// Which role a signing key plays, if any (the mint account signs only at creation).
    pub fn role_of(&self, key: &Pubkey) -> Option<KeyRole> {
        if *key == self.fee_payer {
            Some(KeyRole::FeePayer)
        } else if *key == self.mint_authority {
            Some(KeyRole::MintAuthority)
        } else if *key == self.return_owner {
            Some(KeyRole::ReturnOwner)
        } else if *key == self.mint {
            Some(KeyRole::MintAccount)
        } else {
            None
        }
    }
}

/// The System Program, the two token programs, the ATA program, ComputeBudget and the
/// RecentBlockhashes sysvar: keys no wallet or role may be.
fn is_reserved_key(k: &Pubkey) -> bool {
    [
        SYSTEM_PROGRAM_ID,
        TOKEN_PROGRAM_ID,
        TOKEN_2022_PROGRAM_ID,
        ATA_PROGRAM_ID,
        COMPUTE_BUDGET_PROGRAM_ID,
        RECENT_BLOCKHASHES_SYSVAR_ID,
    ]
    .contains(k)
}

/// A reserved durable-nonce account and its current value (which becomes the message's
/// `recent_blockhash`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonceRef {
    pub account: Pubkey,
    pub value: [u8; 32],
}

/// A built, not yet signed, transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsignedTx {
    pub message: Message,
    /// The bytes every required signer signs and the canister stores before signing.
    pub message_bytes: Vec<u8>,
}

impl UnsignedTx {
    fn from_message(message: Message) -> Result<Self, TxError> {
        let message_bytes = serialize_message(&message)?;
        check_packet_size(&message_bytes, message.header.num_required_signatures as usize)?;
        Ok(UnsignedTx { message, message_bytes })
    }

    /// The signing keys in signature order.
    pub fn required_signers(&self) -> &[Pubkey] {
        self.message.required_signers()
    }

    /// The role of each required signer, in signature order.
    pub fn signer_roles(&self, profile: &TxProfile) -> Vec<Option<KeyRole>> {
        self.required_signers().iter().map(|k| profile.role_of(k)).collect()
    }
}

fn check_nonce(profile: &TxProfile, nonce: &NonceRef) -> Result<(), TxError> {
    if nonce.account == profile.fee_payer
        || nonce.account == profile.mint_authority
        || nonce.account == profile.return_owner
        || nonce.account == profile.mint
        || is_reserved_key(&nonce.account)
    {
        return Err(TxError::InvalidProfile("the nonce account must be its own account"));
    }
    Ok(())
}

fn check_amount(amount: u64) -> Result<(), TxError> {
    if amount == 0 {
        return Err(TxError::InvalidAmount("amount must be at least 1"));
    }
    Ok(())
}

/// Wallet-side validation of a mint recipient (note 2.1): not the System Program key, the mint,
/// any of our own role keys, or a token program id. (The ATA program, ComputeBudget and the
/// sysvar are also refused: they are program keys no user wallet can be.)
pub fn validate_recipient(profile: &TxProfile, recipient: &Pubkey) -> Result<(), TxError> {
    if *recipient == SYSTEM_PROGRAM_ID {
        return Err(TxError::InvalidRecipient("the System Program key"));
    }
    if *recipient == profile.mint {
        return Err(TxError::InvalidRecipient("the mint"));
    }
    if [profile.fee_payer, profile.mint_authority, profile.return_owner].contains(recipient) {
        return Err(TxError::InvalidRecipient("one of the bridge's own role keys"));
    }
    if TokenProgram::from_id(recipient).is_some() {
        return Err(TxError::InvalidRecipient("a token program id"));
    }
    if is_reserved_key(recipient) || *recipient == profile.memo_program {
        return Err(TxError::InvalidRecipient("a program id"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 2.1 Mint
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRequest {
    pub nonce: NonceRef,
    /// The recipient WALLET (base58 32 bytes); its ATA is derived and created idempotently.
    pub recipient_wallet: Pubkey,
    pub amount: u64,
    /// Written into the memo `rumi1|m|<mint_id>`.
    pub mint_id: Hash32,
    /// Profile `per_operation_cap_e8s`: the amount must be in `[1, cap]`.
    pub per_operation_cap: u64,
}

/// `[AdvanceNonce, ATA CreateIdempotent, MintToChecked(amount, 8), Memo rumi1|m|<mint_id>]`.
/// Two required signers: `fee-payer` (writable, fee payer, funder, nonce authority) and
/// `mint-authority` (read-only). No ComputeBudget instruction in v1.
pub fn build_mint_message(profile: &TxProfile, req: &MintRequest) -> Result<UnsignedTx, TxError> {
    profile.validate()?;
    check_nonce(profile, &req.nonce)?;
    check_amount(req.amount)?;
    if req.amount > req.per_operation_cap {
        return Err(TxError::InvalidAmount("amount above the per-operation cap"));
    }
    validate_recipient(profile, &req.recipient_wallet)?;
    if req.recipient_wallet == req.nonce.account {
        return Err(TxError::InvalidRecipient("the nonce account"));
    }
    let token_program = profile.token_program.id();
    let recipient_ata = derive_ata(&req.recipient_wallet, &token_program, &profile.mint)?;
    let memo = canister_memo(CanisterMemoKind::Mint, &req.mint_id);
    let instructions = [
        advance_nonce_instruction(&req.nonce.account, &profile.fee_payer),
        create_ata_idempotent_instruction(&profile.fee_payer, &req.recipient_wallet, &profile.mint, &token_program)?,
        mint_to_checked_instruction(&token_program, &profile.mint, &recipient_ata, &profile.mint_authority, req.amount),
        memo_instruction(&profile.memo_program, memo.as_bytes()),
    ];
    let message = compile_message(MessageVersion::Legacy, &instructions, &profile.fee_payer, req.nonce.value)?;
    UnsignedTx::from_message(message)
}

// ---------------------------------------------------------------------------
// 2.3 Burn of custodied returns
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnRequest {
    /// A burn-lane nonce account (authority `fee-payer`).
    pub nonce: NonceRef,
    /// Never above the credited `RC` and never touching stray balances (the caller's rule).
    pub amount: u64,
    /// `rumi1|b|<burn_id>`; see `ids::burn_id`.
    pub burn_id: Hash32,
}

/// `[AdvanceNonce, BurnChecked(amount, 8) from the return ATA, Memo rumi1|b|<burn_id>]`.
/// Signers: `fee-payer` and `return-owner`.
pub fn build_burn_message(profile: &TxProfile, req: &BurnRequest) -> Result<UnsignedTx, TxError> {
    profile.validate()?;
    check_nonce(profile, &req.nonce)?;
    check_amount(req.amount)?;
    let token_program = profile.token_program.id();
    let return_ata = profile.return_ata()?;
    let memo = canister_memo(CanisterMemoKind::Burn, &req.burn_id);
    let instructions = [
        advance_nonce_instruction(&req.nonce.account, &profile.fee_payer),
        burn_checked_instruction(&token_program, &return_ata, &profile.mint, &profile.return_owner, req.amount),
        memo_instruction(&profile.memo_program, memo.as_bytes()),
    ];
    let message = compile_message(MessageVersion::Legacy, &instructions, &profile.fee_payer, req.nonce.value)?;
    UnsignedTx::from_message(message)
}

// ---------------------------------------------------------------------------
// 2.4 Refund of strays
// ---------------------------------------------------------------------------

/// Where a stray refund goes: the original source token account if it still exists, else the
/// verified sender's ATA, created idempotently first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefundDestination {
    ExistingAccount { token_account: Pubkey },
    CreateAta { owner: Pubkey },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefundRequest {
    pub nonce: NonceRef,
    pub destination: RefundDestination,
    pub amount: u64,
    /// `rumi1|f|<refund_id>`; see `ids::refund_id`.
    pub refund_id: Hash32,
}

/// `[AdvanceNonce, (CreateIdempotent for the sender's ATA), Memo rumi1|f|<refund_id>,
/// TransferChecked(return ATA -> destination, authority return-owner, amount, 8)]`.
///
/// The note lists `[AdvanceNonce, Memo, TransferChecked]` and adds the `CreateIdempotent` "first,
/// only if the original source account no longer exists"; BB-47 places it directly after
/// `AdvanceNonce` (the nonce advance must stay first). Signers: `fee-payer` and `return-owner`.
pub fn build_refund_message(profile: &TxProfile, req: &RefundRequest) -> Result<UnsignedTx, TxError> {
    profile.validate()?;
    check_nonce(profile, &req.nonce)?;
    check_amount(req.amount)?;
    let token_program = profile.token_program.id();
    let return_ata = profile.return_ata()?;
    let mut instructions = vec![advance_nonce_instruction(&req.nonce.account, &profile.fee_payer)];
    let destination = match &req.destination {
        RefundDestination::ExistingAccount { token_account } => {
            if *token_account == return_ata {
                return Err(TxError::InvalidRecipient("the return account itself"));
            }
            validate_recipient(profile, token_account)?;
            *token_account
        }
        RefundDestination::CreateAta { owner } => {
            validate_recipient(profile, owner)?;
            let ata = derive_ata(owner, &token_program, &profile.mint)?;
            if ata == return_ata {
                return Err(TxError::InvalidRecipient("the return account itself"));
            }
            instructions.push(create_ata_idempotent_instruction(&profile.fee_payer, owner, &profile.mint, &token_program)?);
            ata
        }
    };
    let memo = canister_memo(CanisterMemoKind::Refund, &req.refund_id);
    instructions.push(memo_instruction(&profile.memo_program, memo.as_bytes()));
    instructions.push(transfer_checked_instruction(
        &token_program,
        &return_ata,
        &profile.mint,
        &destination,
        &profile.return_owner,
        req.amount,
    ));
    let message = compile_message(MessageVersion::Legacy, &instructions, &profile.fee_payer, req.nonce.value)?;
    UnsignedTx::from_message(message)
}

// ---------------------------------------------------------------------------
// 2.2 Reference user return
// ---------------------------------------------------------------------------

/// `ComputeBudget::SetComputeUnitLimit(units)`: tag 2 then `u32` LE, no accounts. Wallets add
/// instructions like this; the shape checker accepts zero to two ComputeBudget instructions.
pub fn compute_unit_limit_instruction(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction { program_id: COMPUTE_BUDGET_PROGRAM_ID, accounts: vec![], data }
}

/// `ComputeBudget::SetComputeUnitPrice(micro_lamports)`: tag 3 then `u64` LE, no accounts.
pub fn compute_unit_price_instruction(micro_lamports: u64) -> Instruction {
    let mut data = vec![3u8];
    data.extend_from_slice(&micro_lamports.to_le_bytes());
    Instruction { program_id: COMPUTE_BUDGET_PROGRAM_ID, accounts: vec![], data }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnRequest {
    /// The holder: fee payer and the single signing authority of the transfer.
    pub holder: Pubkey,
    pub source_token_account: Pubkey,
    pub amount: u64,
    pub memo: ReturnMemo,
    /// An ordinary recent blockhash (a holder transaction does not use a durable nonce).
    pub recent_blockhash: [u8; 32],
    /// Optional wallet-style ComputeBudget instructions placed first (at most two are valid).
    pub compute_budget: Vec<Instruction>,
}

/// `[(ComputeBudget)*, TransferChecked(source -> return ATA, authority holder, amount, 8),
/// Memo rumi1|r|...]`, legacy. REFERENCE ONLY: the holder signs this, not the bridge.
pub fn build_return_message(profile: &TxProfile, req: &ReturnRequest) -> Result<UnsignedTx, TxError> {
    profile.validate()?;
    check_amount(req.amount)?;
    if [profile.fee_payer, profile.mint_authority, profile.return_owner].contains(&req.holder) {
        return Err(TxError::InvalidRecipient("a holder cannot be one of the bridge's role keys"));
    }
    let token_program = profile.token_program.id();
    let return_ata = profile.return_ata()?;
    let memo = encode_return_memo(&req.memo);
    let mut instructions = req.compute_budget.clone();
    instructions.push(transfer_checked_instruction(
        &token_program,
        &req.source_token_account,
        &profile.mint,
        &return_ata,
        &req.holder,
        req.amount,
    ));
    instructions.push(memo_instruction(&profile.memo_program, memo.as_bytes()));
    let message = compile_message(MessageVersion::Legacy, &instructions, &req.holder, req.recent_blockhash)?;
    UnsignedTx::from_message(message)
}
