//! The user-return SHAPE checker (`chain-solana.md` section 2.2): a pure function over a
//! decoded message (or its bytes) that says whether the transaction has exactly the shape of
//! a valid return, and extracts the claims the observer then verifies against the finalized
//! `getTransaction` result (token-balance deltas, `meta.err`, the source account owner).
//!
//! Accepted shape (any order, legacy or v0 **without** address-table lookups):
//!
//! * exactly one instruction to the profile token program, `TransferChecked(amount, 8)`, with
//!   exactly 4 account metas `[source, mint, destination, authority]`: the mint is the bridge
//!   mint, the destination is the canonical return ATA, the authority is a transaction signer
//!   (single signer, no multisig tail);
//! * exactly one Memo instruction (profile memo program, zero account metas) whose data is a
//!   valid return memo;
//! * zero to two ComputeBudget instructions (zero account metas; they move nothing).
//!
//! Everything else is rejected by default: any other program, any other token-program tag
//! (the unchecked `Transfer` 3, `MintTo` 7, `Burn` 8 and the `Batch` tag 255 by name), a second
//! transfer, a missing or duplicate memo. The checker passes no judgement on balances: that
//! needs the finalized transaction metadata and belongs to the observer (BB-49).

use super::*;
use crate::memo::{parse_return_memo, MemoError, ReturnMemo};
use candid::Principal;

/// What the checker needs from the profile and the instance.
#[derive(Clone, Debug)]
pub struct ReturnShapeContext<'a> {
    pub token_program: Pubkey,
    pub memo_program: Pubkey,
    pub mint: Pubkey,
    /// The canonical return ATA (see `TxProfile::return_ata`).
    pub return_ata: Pubkey,
    /// This canister's own principal (a return memo may not name it).
    pub bridge: &'a Principal,
}

/// The claims of a shape-valid return. None of them is proven yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnShape {
    pub source: Pubkey,
    /// The signing authority of the transfer; the observer must prove it owns `source`.
    pub authority: Pubkey,
    pub amount: u64,
    pub memo: ReturnMemo,
    pub compute_budget_instructions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeReject {
    /// A program that is not the token program, the memo program or ComputeBudget.
    UnknownProgram(Pubkey),
    /// The unchecked `Transfer` (3), `MintTo` (7) or `Burn` (8).
    UncheckedTokenInstruction(u8),
    /// The `Batch` tag 255.
    BatchInstruction,
    /// Any other token-program tag, or an empty instruction.
    DisallowedTokenInstruction(Option<u8>),
    NoTransfer,
    MultipleTransfers,
    /// Not exactly 4 account metas (a multisig tail adds signers after the authority).
    TransferAccountCount(usize),
    TransferDataLength(usize),
    WrongDecimals(u8),
    ZeroAmount,
    WrongMint,
    WrongDestination,
    /// The source is the return account itself.
    SourceIsDestination,
    AuthorityNotSigner,
    NoMemo,
    MultipleMemos,
    MemoHasAccounts,
    MemoInvalid(MemoError),
    TooManyComputeBudget(usize),
    ComputeBudgetHasAccounts,
    /// An account index with no key (the strict decoder already prevents this).
    MalformedIndex,
}

/// Check a decoded message.
pub fn check_return_shape(msg: &Message, ctx: &ReturnShapeContext<'_>) -> Result<ReturnShape, ShapeReject> {
    let mut transfer: Option<&CompiledInstruction> = None;
    let mut memo: Option<&CompiledInstruction> = None;
    let mut memo_count = 0usize;
    let mut transfer_count = 0usize;
    let mut compute_budget = 0usize;

    // Pass 1: classify every instruction by program, default deny.
    for ix in &msg.instructions {
        let program = msg.program_id(ix).ok_or(ShapeReject::MalformedIndex)?;
        if *program == ctx.token_program {
            match ix.data.first().copied() {
                Some(TAG_TRANSFER_CHECKED) => {
                    transfer_count += 1;
                    transfer = Some(ix);
                }
                Some(tag @ (TAG_TRANSFER | TAG_MINT_TO | TAG_BURN)) => return Err(ShapeReject::UncheckedTokenInstruction(tag)),
                Some(TAG_BATCH) => return Err(ShapeReject::BatchInstruction),
                other => return Err(ShapeReject::DisallowedTokenInstruction(other)),
            }
        } else if *program == ctx.memo_program {
            memo_count += 1;
            memo = Some(ix);
        } else if *program == COMPUTE_BUDGET_PROGRAM_ID {
            compute_budget += 1;
            if !ix.accounts.is_empty() {
                return Err(ShapeReject::ComputeBudgetHasAccounts);
            }
        } else {
            return Err(ShapeReject::UnknownProgram(*program));
        }
    }
    if transfer_count == 0 {
        return Err(ShapeReject::NoTransfer);
    }
    if transfer_count > 1 {
        return Err(ShapeReject::MultipleTransfers);
    }
    if compute_budget > 2 {
        return Err(ShapeReject::TooManyComputeBudget(compute_budget));
    }
    if memo_count == 0 {
        return Err(ShapeReject::NoMemo);
    }
    if memo_count > 1 {
        return Err(ShapeReject::MultipleMemos);
    }

    // Pass 2: the one transfer.
    let transfer = transfer.ok_or(ShapeReject::NoTransfer)?;
    if transfer.accounts.len() != 4 {
        return Err(ShapeReject::TransferAccountCount(transfer.accounts.len()));
    }
    let data: [u8; 10] = transfer
        .data
        .as_slice()
        .try_into()
        .map_err(|_| ShapeReject::TransferDataLength(transfer.data.len()))?;
    let decimals = data[9];
    if decimals != TOKEN_DECIMALS {
        return Err(ShapeReject::WrongDecimals(decimals));
    }
    let amount = u64::from_le_bytes([data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8]]);
    if amount == 0 {
        return Err(ShapeReject::ZeroAmount);
    }
    let keys = msg.instruction_accounts(transfer).ok_or(ShapeReject::MalformedIndex)?;
    let (source, mint, destination, authority) = match keys.as_slice() {
        [s, m, d, a] => (*s, *m, *d, *a),
        _ => return Err(ShapeReject::TransferAccountCount(keys.len())),
    };
    if mint != ctx.mint {
        return Err(ShapeReject::WrongMint);
    }
    if destination != ctx.return_ata {
        return Err(ShapeReject::WrongDestination);
    }
    if source == destination {
        return Err(ShapeReject::SourceIsDestination);
    }
    let authority_index = *transfer.accounts.get(3).ok_or(ShapeReject::MalformedIndex)?;
    if !msg.is_signer_index(authority_index as usize) {
        return Err(ShapeReject::AuthorityNotSigner);
    }

    // Pass 3: the memo.
    let memo = memo.ok_or(ShapeReject::NoMemo)?;
    if !memo.accounts.is_empty() {
        return Err(ShapeReject::MemoHasAccounts);
    }
    let memo = parse_return_memo(&memo.data, ctx.bridge).map_err(ShapeReject::MemoInvalid)?;

    Ok(ReturnShape { source, authority, amount, memo, compute_budget_instructions: compute_budget })
}

/// A byte-level failure or a shape rejection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeCheckError {
    /// The bytes are not a strictly valid legacy or lookup-free v0 message.
    Decode(TxError),
    Shape(ShapeReject),
}

/// Decode `message_bytes` strictly, then check the shape. A pure function on bytes.
pub fn check_return_shape_bytes(message_bytes: &[u8], ctx: &ReturnShapeContext<'_>) -> Result<ReturnShape, ShapeCheckError> {
    let msg = decode_message(message_bytes).map_err(ShapeCheckError::Decode)?;
    check_return_shape(&msg, ctx).map_err(ShapeCheckError::Shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memo::MemoError;
    use crate::tx::builders::*;

    fn bridge() -> Principal {
        Principal::from_slice(&[0xbb; 10])
    }

    fn user() -> Principal {
        Principal::from_slice(&[0x07; 10])
    }

    fn profile(tp: TokenProgram) -> TxProfile {
        TxProfile {
            token_program: tp,
            memo_program: MEMO_PROGRAM_V3_ID,
            mint: [0x44; 32],
            fee_payer: [0x11; 32],
            mint_authority: [0x22; 32],
            return_owner: [0x33; 32],
        }
    }

    const HOLDER: Pubkey = [0xa0; 32];
    const SOURCE: Pubkey = [0xa1; 32];
    const OTHER: Pubkey = [0xa2; 32];

    fn ctx<'a>(p: &TxProfile, bridge: &'a Principal) -> ReturnShapeContext<'a> {
        ReturnShapeContext {
            token_program: p.token_program.id(),
            memo_program: p.memo_program,
            mint: p.mint,
            return_ata: p.return_ata().unwrap(),
            bridge,
        }
    }

    fn memo_text(nonce: &str) -> String {
        format!("rumi1|r|{}|-|{nonce}", user().to_text())
    }

    /// The reference instructions of a valid return, for mutation.
    struct Parts {
        transfer: Instruction,
        memo: Instruction,
    }

    fn good_parts(p: &TxProfile) -> Parts {
        Parts {
            transfer: transfer_checked_instruction(
                &p.token_program.id(),
                &SOURCE,
                &p.mint,
                &p.return_ata().unwrap(),
                &HOLDER,
                5_000,
            ),
            memo: memo_instruction(&p.memo_program, memo_text("1").as_bytes()),
        }
    }

    fn message(ixs: &[Instruction]) -> Message {
        compile_message(MessageVersion::Legacy, ixs, &HOLDER, [0x77; 32]).unwrap()
    }

    fn check(p: &TxProfile, ixs: &[Instruction]) -> Result<ReturnShape, ShapeReject> {
        let b = bridge();
        check_return_shape(&message(ixs), &ctx(p, &b))
    }

    #[test]
    fn the_reference_builder_output_is_shape_valid_for_both_token_programs() {
        for tp in [TokenProgram::Classic, TokenProgram::Token2022] {
            let p = profile(tp);
            let req = ReturnRequest {
                holder: HOLDER,
                source_token_account: SOURCE,
                amount: 123_456_789,
                memo: ReturnMemo { destination: user(), subaccount: Some([0xab; 32]), client_nonce: 42 },
                recent_blockhash: [0x77; 32],
                compute_budget: vec![],
            };
            let tx = build_return_message(&p, &req).unwrap();
            let b = bridge();
            let shape = check_return_shape_bytes(&tx.message_bytes, &ctx(&p, &b)).unwrap();
            assert_eq!(shape.source, SOURCE);
            assert_eq!(shape.authority, HOLDER);
            assert_eq!(shape.amount, 123_456_789);
            assert_eq!(shape.memo, req.memo);
            assert_eq!(shape.compute_budget_instructions, 0);
        }
    }

    #[test]
    fn instruction_order_is_free_and_compute_budget_is_allowed_up_to_two() {
        let p = profile(TokenProgram::Token2022);
        let Parts { transfer, memo } = good_parts(&p);
        let cb1 = compute_unit_limit_instruction(200_000);
        let cb2 = compute_unit_price_instruction(1_000);
        for ixs in [
            vec![transfer.clone(), memo.clone()],
            vec![memo.clone(), transfer.clone()],
            vec![cb1.clone(), transfer.clone(), memo.clone()],
            vec![cb1.clone(), cb2.clone(), memo.clone(), transfer.clone()],
            vec![memo.clone(), transfer.clone(), cb2.clone()],
        ] {
            let s = check(&p, &ixs).unwrap();
            assert_eq!(s.compute_budget_instructions, ixs.len() - 2);
        }
        assert_eq!(
            check(&p, &[cb1.clone(), cb2.clone(), cb1.clone(), transfer.clone(), memo.clone()]),
            Err(ShapeReject::TooManyComputeBudget(3))
        );
        let mut with_accounts = cb1.clone();
        with_accounts.accounts.push(AccountMeta::readonly([9; 32], false));
        assert_eq!(check(&p, &[with_accounts, transfer, memo]), Err(ShapeReject::ComputeBudgetHasAccounts));
    }

    #[test]
    fn v0_without_lookups_is_accepted_and_with_lookups_is_a_decode_error() {
        let p = profile(TokenProgram::Classic);
        let Parts { transfer, memo } = good_parts(&p);
        let v0 = compile_message(MessageVersion::V0, &[transfer, memo], &HOLDER, [0x77; 32]).unwrap();
        let bytes = serialize_message(&v0).unwrap();
        let b = bridge();
        assert!(check_return_shape_bytes(&bytes, &ctx(&p, &b)).is_ok());
        let mut with_lookup = bytes.clone();
        *with_lookup.last_mut().unwrap() = 1;
        assert_eq!(
            check_return_shape_bytes(&with_lookup, &ctx(&p, &b)),
            Err(ShapeCheckError::Decode(TxError::AddressTableLookups))
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(check_return_shape_bytes(&trailing, &ctx(&p, &b)), Err(ShapeCheckError::Decode(TxError::TrailingBytes)));
    }

    fn unchecked(p: &TxProfile, tag: u8, accounts: Vec<AccountMeta>, data_len: usize) -> Instruction {
        let mut data = vec![tag];
        data.extend(std::iter::repeat_n(1u8, data_len - 1));
        Instruction { program_id: p.token_program.id(), accounts, data }
    }

    /// One row per rejection class of the note's 2.2 table and the brief's list.
    #[test]
    fn rejection_table() {
        for tp in [TokenProgram::Classic, TokenProgram::Token2022] {
            let p = profile(tp);
            let tpid = p.token_program.id();
            let ret_ata = p.return_ata().unwrap();
            let Parts { transfer, memo } = good_parts(&p);
            let other_token = if tp == TokenProgram::Classic { TOKEN_2022_PROGRAM_ID } else { TOKEN_PROGRAM_ID };

            let src_meta = || AccountMeta::writable(SOURCE, false);
            let dst_meta = || AccountMeta::writable(ret_ata, false);
            let auth_meta = || AccountMeta::readonly(HOLDER, true);
            let legacy_transfer = unchecked(&p, TAG_TRANSFER, vec![src_meta(), dst_meta(), auth_meta()], 9);
            let mint_to = unchecked(&p, TAG_MINT_TO, vec![AccountMeta::writable(p.mint, false), dst_meta(), auth_meta()], 9);
            let burn = unchecked(&p, TAG_BURN, vec![src_meta(), AccountMeta::writable(p.mint, false), auth_meta()], 9);
            let batch = unchecked(&p, TAG_BATCH, vec![], 5);
            let approve = unchecked(&p, 4, vec![src_meta(), AccountMeta::readonly(OTHER, false), auth_meta()], 9);
            let close = unchecked(&p, 9, vec![src_meta(), AccountMeta::writable(OTHER, false), auth_meta()], 1);
            let empty_token = Instruction { program_id: tpid, accounts: vec![], data: vec![] };

            let mut second_memo = memo.clone();
            second_memo.data = memo_text("2").into_bytes();
            let mut bad_memo = memo.clone();
            bad_memo.data = memo_text("01").into_bytes();
            let mut memo_with_accounts = memo.clone();
            memo_with_accounts.accounts.push(AccountMeta::readonly(OTHER, false));
            let mut wrong_program_memo = memo.clone();
            wrong_program_memo.program_id = [0x5a; 32];

            let with = |f: &dyn Fn(&mut Instruction)| {
                let mut t = transfer.clone();
                f(&mut t);
                t
            };
            let wrong_mint = with(&|t| t.accounts[1] = AccountMeta::readonly([0x45; 32], false));
            let wrong_dest = with(&|t| t.accounts[2] = AccountMeta::writable(OTHER, false));
            let self_transfer = with(&|t| t.accounts[0] = AccountMeta::writable(ret_ata, false));
            let non_signer_authority = with(&|t| t.accounts[3] = AccountMeta::readonly(OTHER, false));
            let multisig_tail = with(&|t| {
                t.accounts.push(AccountMeta::readonly(OTHER, true));
            });
            let three_accounts = with(&|t| {
                t.accounts.pop();
            });
            let wrong_decimals = with(&|t| t.data[9] = 9);
            let short_data = with(&|t| {
                t.data.pop();
            });
            let long_data = with(&|t| t.data.push(0));
            let zero_amount = with(&|t| t.data[1..9].copy_from_slice(&0u64.to_le_bytes()));

            let system_transfer = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![AccountMeta::writable(HOLDER, true), AccountMeta::writable(OTHER, false)],
                data: vec![2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
            };
            let create_ata =
                create_ata_idempotent_instruction(&HOLDER, &HOLDER, &p.mint, &tpid).unwrap();
            let other_token_transfer = Instruction { program_id: other_token, ..transfer.clone() };

            type Case = (&'static str, Vec<Instruction>, ShapeReject);
            let cases: Vec<Case> = vec![
                ("unchecked Transfer (tag 3)", vec![legacy_transfer.clone(), memo.clone()], ShapeReject::UncheckedTokenInstruction(3)),
                ("MintTo (tag 7)", vec![mint_to, transfer.clone(), memo.clone()], ShapeReject::UncheckedTokenInstruction(7)),
                ("Burn (tag 8)", vec![burn, transfer.clone(), memo.clone()], ShapeReject::UncheckedTokenInstruction(8)),
                ("Batch (tag 255)", vec![batch, transfer.clone(), memo.clone()], ShapeReject::BatchInstruction),
                ("Approve (tag 4)", vec![approve, transfer.clone(), memo.clone()], ShapeReject::DisallowedTokenInstruction(Some(4))),
                ("CloseAccount (tag 9)", vec![close, transfer.clone(), memo.clone()], ShapeReject::DisallowedTokenInstruction(Some(9))),
                ("empty token instruction", vec![empty_token, transfer.clone(), memo.clone()], ShapeReject::DisallowedTokenInstruction(None)),
                ("two transfers", vec![transfer.clone(), transfer.clone(), memo.clone()], ShapeReject::MultipleTransfers),
                ("no transfer (memo only)", vec![memo.clone()], ShapeReject::NoTransfer),
                ("missing memo", vec![transfer.clone()], ShapeReject::NoMemo),
                ("duplicate memo", vec![transfer.clone(), memo.clone(), second_memo], ShapeReject::MultipleMemos),
                ("malformed memo (leading-zero nonce)", vec![transfer.clone(), bad_memo], ShapeReject::MemoInvalid(MemoError::NonceLeadingZero)),
                ("memo with account metas", vec![transfer.clone(), memo_with_accounts], ShapeReject::MemoHasAccounts),
                ("memo on another program", vec![transfer.clone(), wrong_program_memo], ShapeReject::UnknownProgram([0x5a; 32])),
                ("wrong mint", vec![wrong_mint, memo.clone()], ShapeReject::WrongMint),
                ("wrong destination", vec![wrong_dest, memo.clone()], ShapeReject::WrongDestination),
                ("source is the return account", vec![self_transfer, memo.clone()], ShapeReject::SourceIsDestination),
                ("authority is not a signer", vec![non_signer_authority, memo.clone()], ShapeReject::AuthorityNotSigner),
                ("multisig tail (5 metas)", vec![multisig_tail, memo.clone()], ShapeReject::TransferAccountCount(5)),
                ("three metas", vec![three_accounts, memo.clone()], ShapeReject::TransferAccountCount(3)),
                ("decimals 9", vec![wrong_decimals, memo.clone()], ShapeReject::WrongDecimals(9)),
                ("short data", vec![short_data, memo.clone()], ShapeReject::TransferDataLength(9)),
                ("long data", vec![long_data, memo.clone()], ShapeReject::TransferDataLength(11)),
                ("zero amount", vec![zero_amount, memo.clone()], ShapeReject::ZeroAmount),
                ("System transfer alongside", vec![system_transfer, transfer.clone(), memo.clone()], ShapeReject::UnknownProgram(SYSTEM_PROGRAM_ID)),
                ("ATA create alongside", vec![create_ata, transfer.clone(), memo.clone()], ShapeReject::UnknownProgram(ATA_PROGRAM_ID)),
                ("the other token program", vec![other_token_transfer, memo.clone()], ShapeReject::UnknownProgram(other_token)),
            ];
            for (name, ixs, expected) in cases {
                assert_eq!(check(&p, &ixs).map(|_| ()), Err(expected), "{tp:?}: case `{name}`");
            }
            // Sanity: the unmutated pair still passes in this loop iteration.
            assert!(check(&p, &[transfer, memo]).is_ok());
        }
    }

    #[test]
    fn a_return_memo_naming_the_bridge_or_anonymous_is_rejected() {
        let p = profile(TokenProgram::Classic);
        let Parts { transfer, mut memo } = good_parts(&p);
        memo.data = format!("rumi1|r|{}|-|1", bridge().to_text()).into_bytes();
        assert_eq!(check(&p, &[transfer.clone(), memo.clone()]), Err(ShapeReject::MemoInvalid(MemoError::PrincipalIsBridge)));
        memo.data = b"rumi1|r|2vxsx-fae|-|1".to_vec();
        assert_eq!(check(&p, &[transfer, memo]), Err(ShapeReject::MemoInvalid(MemoError::PrincipalAnonymous)));
    }

    #[test]
    fn the_shape_is_decided_by_the_profile_values_not_by_defaults() {
        // A message valid for classic is not valid for a Token-2022 profile (different token
        // program id and different return ATA), and vice versa.
        let classic = profile(TokenProgram::Classic);
        let t22 = profile(TokenProgram::Token2022);
        let Parts { transfer, memo } = good_parts(&classic);
        let msg = message(&[transfer, memo]);
        let b = bridge();
        assert!(check_return_shape(&msg, &ctx(&classic, &b)).is_ok());
        assert!(matches!(check_return_shape(&msg, &ctx(&t22, &b)), Err(ShapeReject::UnknownProgram(_))));
        // A different memo program in the profile (U: which version is deployed).
        let mut other_memo = classic.clone();
        other_memo.memo_program = [0x5b; 32];
        assert!(matches!(check_return_shape(&msg, &ctx(&other_memo, &b)), Err(ShapeReject::UnknownProgram(_))));
    }
}
