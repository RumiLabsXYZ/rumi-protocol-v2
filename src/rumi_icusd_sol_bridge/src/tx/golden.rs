//! Golden vectors for the message builders (note section 8, future test 1, brought forward).
//!
//! Each vector is the exact serialized message for fixed keys, and is checked THREE ways:
//!
//! 1. **The builder** (`build_*_message`) must produce the pinned bytes.
//! 2. **A hand-written oracle** in this file must produce the same bytes. The oracle takes an
//!    explicit key table (in the order the SDK compile rule gives, written out by hand per
//!    vector together with the group sizes), an explicit instruction table with the documented
//!    account lists and data layouts, and assembles the bytes with plain `extend_from_slice`
//!    and single-byte lengths. It shares no code with `compile_message`/`serialize_message`;
//!    the only shared inputs are the fixed keys, and the derived ATA addresses come from
//!    pinned literals, not from `derive_ata`.
//! 3. **The pinned hex** was produced by an independent implementation, the separate Python
//!    program `tools/golden_reference.py` (`hashlib.sha256`, a pure-Python base58 and a
//!    pure-Python ed25519 decompression check; it also reproduces the SPL known-answer ATA
//!    vector that the backend pins). The pinned ATA addresses come from the same program.
//!    `python3 tools/golden_reference.py --check` re-derives every constant in this file and
//!    in `ids.rs` and compares it with the Rust source text.
//!
//! The pinned values are therefore agreed by two programs written from the documented
//! formats; a change to any byte here is a change of the on-chain message.

use super::builders::*;
use super::*;
use crate::ids::{Hash32, KeyRole};
use crate::memo::ReturnMemo;
use candid::Principal;

const FEE_PAYER: Pubkey = [0x11; 32];
const MINT_AUTHORITY: Pubkey = [0x22; 32];
const RETURN_OWNER: Pubkey = [0x33; 32];
const MINT: Pubkey = [0x44; 32];
const NONCE_ACCOUNT: Pubkey = [0x55; 32];
const NONCE_VALUE: [u8; 32] = [0x66; 32];
const RECIPIENT_WALLET: Pubkey = [0x77; 32];
const SENDER_WALLET: Pubkey = [0x88; 32];
const SENDER_SOURCE: Pubkey = [0x99; 32];
const HOLDER: Pubkey = [0xa0; 32];
const HOLDER_SOURCE: Pubkey = [0xa1; 32];
const RETURN_BLOCKHASH: [u8; 32] = [0x77; 32];
const AMOUNT: u64 = 123_456_789;
const MINT_ID: Hash32 = [0xa1; 32];
const BURN_ID: Hash32 = [0xb2; 32];
const REFUND_ID: Hash32 = [0xc3; 32];
const SYSTEM: Pubkey = SYSTEM_PROGRAM_ID;
const RECENT_BH: Pubkey = RECENT_BLOCKHASHES_SYSVAR_ID;
const MEMO: Pubkey = MEMO_PROGRAM_V3_ID;

fn h32(s: &str) -> Pubkey {
    let mut out = [0u8; 32];
    hex::decode_to_slice(s, &mut out).expect("32-byte hex");
    out
}

/// Everything that differs between the two token programs.
struct Variant {
    name: &'static str,
    program: TokenProgram,
    token: Pubkey,
    recipient_ata: Pubkey,
    return_ata: Pubkey,
    sender_ata: Pubkey,
}

fn classic() -> Variant {
    Variant {
        name: "classic",
        program: TokenProgram::Classic,
        token: TOKEN_PROGRAM_ID,
        recipient_ata: h32("b50e93ae0f8cc64a9ffdd974973a49603ae90d0c4d60296a53b15d6dc3f6dd98"),
        return_ata: h32("56a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eafce708f8a"),
        sender_ata: h32("6d083824ef1c847e388ab6cf78bd8216b0611eb968810b2d0957c8e57ea33ca2"),
    }
}

fn token2022() -> Variant {
    Variant {
        name: "token2022",
        program: TokenProgram::Token2022,
        token: TOKEN_2022_PROGRAM_ID,
        recipient_ata: h32("138fa40e79302538603035ca4ebd4bd5de8034049658ad4c3213c88de03ce903"),
        return_ata: h32("9ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3bb2f5b35"),
        sender_ata: h32("4c3ac3ad6dbc463c96b6beba58dd81fe2b95eb235553c3a230a5f4e5dcc0029e"),
    }
}

fn profile(v: &Variant) -> TxProfile {
    TxProfile {
        token_program: v.program,
        memo_program: MEMO_PROGRAM_V3_ID,
        mint: MINT,
        fee_payer: FEE_PAYER,
        mint_authority: MINT_AUTHORITY,
        return_owner: RETURN_OWNER,
    }
}

fn nonce() -> NonceRef {
    NonceRef { account: NONCE_ACCOUNT, value: NONCE_VALUE }
}

/// The independent oracle. `groups` = sizes of (writable signers, read-only signers, writable
/// non-signers, read-only non-signers); each of the last three groups must be strictly
/// ascending by key bytes (the SDK order), the first group is the fee payer alone here.
type OracleIx<'a> = (&'a Pubkey, Vec<&'a Pubkey>, Vec<u8>);

fn oracle(groups: [usize; 4], keys: &[&Pubkey], blockhash: &[u8; 32], ixs: &[OracleIx<'_>]) -> Vec<u8> {
    assert_eq!(groups.iter().sum::<usize>(), keys.len(), "group sizes cover the key table");
    assert_eq!(groups[0], 1, "only the fee payer is a writable signer in these vectors");
    // Each non-payer group is strictly ascending.
    let mut start = 1;
    for size in [groups[1], groups[2], groups[3]] {
        let g = &keys[start..start + size];
        assert!(g.windows(2).all(|w| w[0] < w[1]), "group at {start} is not strictly ascending: {:?}", g.iter().map(|k| hex::encode(&k[..4])).collect::<Vec<_>>());
        start += size;
    }
    let mut out = vec![(groups[0] + groups[1]) as u8, groups[1] as u8, groups[3] as u8];
    assert!(keys.len() < 128 && ixs.len() < 128, "single-byte compact lengths");
    out.push(keys.len() as u8);
    for k in keys {
        out.extend_from_slice(*k);
    }
    out.extend_from_slice(blockhash);
    out.push(ixs.len() as u8);
    for (program, accounts, data) in ixs {
        let idx = |k: &Pubkey| keys.iter().position(|x| *x == k).expect("every referenced key is in the table") as u8;
        out.push(idx(program));
        assert!(accounts.len() < 128 && data.len() < 128);
        out.push(accounts.len() as u8);
        out.extend(accounts.iter().map(|k| idx(k)));
        out.push(data.len() as u8);
        out.extend_from_slice(data);
    }
    out
}

fn le(amount: u64) -> [u8; 8] {
    amount.to_le_bytes()
}

fn checked(tag: u8) -> Vec<u8> {
    [&[tag][..], &le(AMOUNT), &[8]].concat()
}

fn memo_bytes(kind: char, id: &Hash32) -> Vec<u8> {
    format!("rumi1|{kind}|{}", hex::encode(id)).into_bytes()
}

const ADVANCE_NONCE_DATA: [u8; 4] = [4, 0, 0, 0];

// ---------------------------------------------------------------------------
// Vector builders: (message built by the crate, message assembled by the oracle)
// ---------------------------------------------------------------------------

fn mint_vector(v: &Variant) -> (UnsignedTx, Vec<u8>) {
    let tx = build_mint_message(
        &profile(v),
        &MintRequest {
            nonce: nonce(),
            recipient_wallet: RECIPIENT_WALLET,
            amount: AMOUNT,
            mint_id: MINT_ID,
            per_operation_cap: 1_000_000_000_000,
        },
    )
    .expect("mint message");
    // Account table (SDK order): fee payer; read-only signer mint-authority; then writable
    // non-signers ascending; then read-only non-signers ascending.
    let keys: Vec<&Pubkey> = match v.program {
        // recipient ATA b50e.. sorts after the nonce account 55..
        TokenProgram::Classic => vec![
            &FEE_PAYER, &MINT_AUTHORITY, &MINT, &NONCE_ACCOUNT, &v.recipient_ata, &SYSTEM, &MEMO, &RECENT_BH, &v.token,
            &RECIPIENT_WALLET, &ATA_PROGRAM_ID,
        ],
        // recipient ATA 138f.. sorts before the mint 44..
        TokenProgram::Token2022 => vec![
            &FEE_PAYER, &MINT_AUTHORITY, &v.recipient_ata, &MINT, &NONCE_ACCOUNT, &SYSTEM, &MEMO, &RECENT_BH, &v.token,
            &RECIPIENT_WALLET, &ATA_PROGRAM_ID,
        ],
    };
    let ixs: Vec<OracleIx<'_>> = vec![
        // 1. AdvanceNonceAccount: [nonce, RecentBlockhashes, authority], data = u32 LE 4.
        (&SYSTEM, vec![&NONCE_ACCOUNT, &RECENT_BH, &FEE_PAYER], ADVANCE_NONCE_DATA.to_vec()),
        // 2. ATA CreateIdempotent: [funder, ata, wallet, mint, system, token program], data = [1].
        (&ATA_PROGRAM_ID, vec![&FEE_PAYER, &v.recipient_ata, &RECIPIENT_WALLET, &MINT, &SYSTEM, &v.token], vec![1]),
        // 3. MintToChecked: [mint, destination, authority], data = [14, amount LE, 8].
        (&v.token, vec![&MINT, &v.recipient_ata, &MINT_AUTHORITY], checked(14)),
        // 4. Memo rumi1|m|<mint_id>, no accounts.
        (&MEMO, vec![], memo_bytes('m', &MINT_ID)),
    ];
    (tx, oracle([1, 1, 3, 6], &keys, &NONCE_VALUE, &ixs))
}

fn burn_vector(v: &Variant) -> (UnsignedTx, Vec<u8>) {
    let tx = build_burn_message(&profile(v), &BurnRequest { nonce: nonce(), amount: AMOUNT, burn_id: BURN_ID }).expect("burn message");
    // Writable non-signers ascending: mint 44, nonce 55, return ATA (56a2.. or 9ab6..).
    let keys: Vec<&Pubkey> = vec![&FEE_PAYER, &RETURN_OWNER, &MINT, &NONCE_ACCOUNT, &v.return_ata, &SYSTEM, &MEMO, &RECENT_BH, &v.token];
    let ixs: Vec<OracleIx<'_>> = vec![
        (&SYSTEM, vec![&NONCE_ACCOUNT, &RECENT_BH, &FEE_PAYER], ADVANCE_NONCE_DATA.to_vec()),
        // BurnChecked: [token account, mint, owner], data = [15, amount LE, 8].
        (&v.token, vec![&v.return_ata, &MINT, &RETURN_OWNER], checked(15)),
        (&MEMO, vec![], memo_bytes('b', &BURN_ID)),
    ];
    (tx, oracle([1, 1, 3, 4], &keys, &NONCE_VALUE, &ixs))
}

fn refund_existing_vector(v: &Variant) -> (UnsignedTx, Vec<u8>) {
    let tx = build_refund_message(
        &profile(v),
        &RefundRequest {
            nonce: nonce(),
            destination: RefundDestination::ExistingAccount { token_account: SENDER_SOURCE },
            amount: AMOUNT,
            refund_id: REFUND_ID,
        },
    )
    .expect("refund message");
    // Writable non-signers ascending: nonce 55, then {return ATA, sender source 99} in byte order.
    let keys: Vec<&Pubkey> = match v.program {
        TokenProgram::Classic => vec![&FEE_PAYER, &RETURN_OWNER, &NONCE_ACCOUNT, &v.return_ata, &SENDER_SOURCE, &SYSTEM, &MEMO, &RECENT_BH, &v.token, &MINT],
        TokenProgram::Token2022 => vec![&FEE_PAYER, &RETURN_OWNER, &NONCE_ACCOUNT, &SENDER_SOURCE, &v.return_ata, &SYSTEM, &MEMO, &RECENT_BH, &v.token, &MINT],
    };
    let ixs: Vec<OracleIx<'_>> = vec![
        (&SYSTEM, vec![&NONCE_ACCOUNT, &RECENT_BH, &FEE_PAYER], ADVANCE_NONCE_DATA.to_vec()),
        (&MEMO, vec![], memo_bytes('f', &REFUND_ID)),
        // TransferChecked: [source, mint, destination, authority], data = [12, amount LE, 8].
        (&v.token, vec![&v.return_ata, &MINT, &SENDER_SOURCE, &RETURN_OWNER], checked(12)),
    ];
    (tx, oracle([1, 1, 3, 5], &keys, &NONCE_VALUE, &ixs))
}

fn refund_create_vector(v: &Variant) -> (UnsignedTx, Vec<u8>) {
    let tx = build_refund_message(
        &profile(v),
        &RefundRequest {
            nonce: nonce(),
            destination: RefundDestination::CreateAta { owner: SENDER_WALLET },
            amount: AMOUNT,
            refund_id: REFUND_ID,
        },
    )
    .expect("refund message");
    // Writable non-signers (3): {sender ATA, nonce 55, return ATA} in byte order; read-only (7).
    let keys: Vec<&Pubkey> = match v.program {
        TokenProgram::Classic => vec![
            &FEE_PAYER, &RETURN_OWNER, &NONCE_ACCOUNT, &v.return_ata, &v.sender_ata, &SYSTEM, &MEMO, &RECENT_BH, &v.token, &MINT,
            &SENDER_WALLET, &ATA_PROGRAM_ID,
        ],
        TokenProgram::Token2022 => vec![
            &FEE_PAYER, &RETURN_OWNER, &v.sender_ata, &NONCE_ACCOUNT, &v.return_ata, &SYSTEM, &MEMO, &RECENT_BH, &v.token, &MINT,
            &SENDER_WALLET, &ATA_PROGRAM_ID,
        ],
    };
    let ixs: Vec<OracleIx<'_>> = vec![
        (&SYSTEM, vec![&NONCE_ACCOUNT, &RECENT_BH, &FEE_PAYER], ADVANCE_NONCE_DATA.to_vec()),
        (&ATA_PROGRAM_ID, vec![&FEE_PAYER, &v.sender_ata, &SENDER_WALLET, &MINT, &SYSTEM, &v.token], vec![1]),
        (&MEMO, vec![], memo_bytes('f', &REFUND_ID)),
        (&v.token, vec![&v.return_ata, &MINT, &v.sender_ata, &RETURN_OWNER], checked(12)),
    ];
    (tx, oracle([1, 1, 3, 7], &keys, &NONCE_VALUE, &ixs))
}

fn user_return_memo() -> ReturnMemo {
    ReturnMemo { destination: Principal::from_slice(&[0x07; 10]), subaccount: None, client_nonce: 42 }
}

/// The principal text below was produced by the Python reference (CRC32 + base32), so a match
/// also cross-checks the principal textual encoding used for the memo.
const USER_RETURN_MEMO_TEXT: &str = "rumi1|r|jtg6r-iiha4-dqoby-ha4dq-oby|-|42";

fn return_vector(v: &Variant, with_compute_budget: bool) -> (UnsignedTx, Vec<u8>) {
    let compute_budget = if with_compute_budget {
        vec![compute_unit_limit_instruction(200_000), compute_unit_price_instruction(1_000)]
    } else {
        vec![]
    };
    let tx = build_return_message(
        &profile(v),
        &ReturnRequest {
            holder: HOLDER,
            source_token_account: HOLDER_SOURCE,
            amount: AMOUNT,
            memo: user_return_memo(),
            recent_blockhash: RETURN_BLOCKHASH,
            compute_budget,
        },
    )
    .expect("return message");
    // The holder is the only signer. Writable non-signers ascending: return ATA, holder source a1;
    // read-only ascending: [ComputeBudget 0306..], memo 05.., token 06dd.., mint 44...
    let mut ixs: Vec<OracleIx<'_>> = vec![];
    let mut keys: Vec<&Pubkey> = vec![&HOLDER, &v.return_ata, &HOLDER_SOURCE];
    let mut groups = [1, 0, 2, 3];
    let limit_data = [&[2u8][..], &200_000u32.to_le_bytes()].concat();
    let price_data = [&[3u8][..], &1_000u64.to_le_bytes()].concat();
    if with_compute_budget {
        keys.push(&COMPUTE_BUDGET_PROGRAM_ID);
        groups[3] = 4;
        ixs.push((&COMPUTE_BUDGET_PROGRAM_ID, vec![], limit_data));
        ixs.push((&COMPUTE_BUDGET_PROGRAM_ID, vec![], price_data));
    }
    keys.extend([&MEMO, &v.token, &MINT]);
    // TransferChecked: [source, mint, destination = return ATA, authority = holder].
    ixs.push((&v.token, vec![&HOLDER_SOURCE, &MINT, &v.return_ata, &HOLDER], checked(12)));
    ixs.push((&MEMO, vec![], USER_RETURN_MEMO_TEXT.as_bytes().to_vec()));
    (tx, oracle(groups, &keys, &RETURN_BLOCKHASH, &ixs))
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

fn check_vector(name: &str, built: (UnsignedTx, Vec<u8>), pinned: &str) -> UnsignedTx {
    let (tx, oracle_bytes) = built;
    assert_eq!(hex::encode(&tx.message_bytes), pinned, "{name}: builder != pinned independent reference");
    assert_eq!(tx.message_bytes, oracle_bytes, "{name}: builder != hand-written oracle");
    assert_eq!(hex::encode(&oracle_bytes), pinned, "{name}: oracle != pinned independent reference");
    // The bytes decode back to the same message, and re-serialize identically.
    let decoded = decode_message(&tx.message_bytes).expect("decodes");
    assert_eq!(decoded, tx.message, "{name}");
    assert_eq!(serialize_message(&decoded).expect("serializes"), tx.message_bytes, "{name}");
    tx
}

#[test]
fn mint_vectors() {
    for (v, pinned) in [(classic(), MINT_CLASSIC), (token2022(), MINT_T22)] {
        let tx = check_vector(&format!("mint/{}", v.name), mint_vector(&v), pinned);
        let p = profile(&v);
        let m = &tx.message;
        // Durable nonce: AdvanceNonce first, blockhash = nonce value, authority = fee payer.
        assert_eq!(leading_advance_nonce(m), Some((NONCE_ACCOUNT, FEE_PAYER)));
        assert_eq!(m.recent_blockhash, NONCE_VALUE);
        // Two signers: fee payer first, then the mint authority.
        assert_eq!(tx.required_signers(), &[FEE_PAYER, MINT_AUTHORITY]);
        assert_eq!(tx.signer_roles(&p), vec![Some(KeyRole::FeePayer), Some(KeyRole::MintAuthority)]);
        assert_eq!(m.header, MessageHeader { num_required_signatures: 2, num_readonly_signed_accounts: 1, num_readonly_unsigned_accounts: 6 });
        // Instruction programs in the documented order.
        let programs: Vec<Pubkey> = m.instructions.iter().map(|ix| *m.program_id(ix).unwrap()).collect();
        assert_eq!(programs, vec![SYSTEM, ATA_PROGRAM_ID, v.token, MEMO]);
        // Checked tag with decimals 8 and the exact amount, never tag 7.
        assert_eq!(m.instructions[2].data, [&[14u8][..], &AMOUNT.to_le_bytes(), &[8]].concat());
        // The recipient ATA derives from (wallet, THIS token program, mint).
        assert_eq!(v.recipient_ata, derive_ata(&RECIPIENT_WALLET, &v.token, &MINT).unwrap());
        // Wire assembly: signatures in signer order.
        let sigs = order_signatures_by_signer(m, &[(MINT_AUTHORITY, [2; 64]), (FEE_PAYER, [1; 64])]).unwrap();
        assert_eq!(sigs, vec![[1; 64], [2; 64]]);
        let wire = assemble_wire_tx(&sigs, &tx.message_bytes).unwrap();
        assert_eq!(first_signature_base58(&wire).unwrap(), signature_to_base58(&[1; 64]));
    }
}

#[test]
fn burn_vectors() {
    for (v, pinned) in [(classic(), BURN_CLASSIC), (token2022(), BURN_T22)] {
        let tx = check_vector(&format!("burn/{}", v.name), burn_vector(&v), pinned);
        let p = profile(&v);
        let m = &tx.message;
        assert_eq!(leading_advance_nonce(m), Some((NONCE_ACCOUNT, FEE_PAYER)));
        assert_eq!(m.recent_blockhash, NONCE_VALUE);
        assert_eq!(tx.required_signers(), &[FEE_PAYER, RETURN_OWNER]);
        assert_eq!(tx.signer_roles(&p), vec![Some(KeyRole::FeePayer), Some(KeyRole::ReturnOwner)]);
        assert_eq!(m.instructions[1].data, [&[15u8][..], &AMOUNT.to_le_bytes(), &[8]].concat());
        assert_eq!(v.return_ata, derive_ata(&RETURN_OWNER, &v.token, &MINT).unwrap());
        assert_eq!(v.return_ata, p.return_ata().unwrap());
    }
}

#[test]
fn refund_vectors() {
    for (v, existing, create) in [(classic(), REFUND_EXISTING_CLASSIC, REFUND_CREATE_ATA_CLASSIC), (token2022(), REFUND_EXISTING_T22, REFUND_CREATE_ATA_T22)] {
        let tx = check_vector(&format!("refund-existing/{}", v.name), refund_existing_vector(&v), existing);
        assert_eq!(tx.required_signers(), &[FEE_PAYER, RETURN_OWNER]);
        assert_eq!(leading_advance_nonce(&tx.message), Some((NONCE_ACCOUNT, FEE_PAYER)));
        assert_eq!(tx.message.instructions.len(), 3);
        let tx = check_vector(&format!("refund-create/{}", v.name), refund_create_vector(&v), create);
        assert_eq!(tx.required_signers(), &[FEE_PAYER, RETURN_OWNER]);
        assert_eq!(leading_advance_nonce(&tx.message), Some((NONCE_ACCOUNT, FEE_PAYER)));
        assert_eq!(tx.message.instructions.len(), 4, "the idempotent create sits right after the advance");
        assert_eq!(v.sender_ata, derive_ata(&SENDER_WALLET, &v.token, &MINT).unwrap());
    }
}

#[test]
fn return_vectors_and_their_shape() {
    use crate::tx::shape::{check_return_shape_bytes, ReturnShapeContext};
    let bridge = Principal::from_slice(&[0xbb; 10]);
    for (v, plain, with_cb) in [(classic(), RETURN_CLASSIC, RETURN_COMPUTE_BUDGET_CLASSIC), (token2022(), RETURN_T22, RETURN_COMPUTE_BUDGET_T22)] {
        let ctx = ReturnShapeContext {
            token_program: v.token,
            memo_program: MEMO,
            mint: MINT,
            return_ata: v.return_ata,
            bridge: &bridge,
        };
        let tx = check_vector(&format!("return/{}", v.name), return_vector(&v, false), plain);
        assert_eq!(tx.required_signers(), &[HOLDER]);
        let shape = check_return_shape_bytes(&tx.message_bytes, &ctx).expect("the reference return is shape-valid");
        assert_eq!((shape.source, shape.authority, shape.amount, shape.compute_budget_instructions), (HOLDER_SOURCE, HOLDER, AMOUNT, 0));
        assert_eq!(shape.memo, user_return_memo());

        let tx = check_vector(&format!("return+cb/{}", v.name), return_vector(&v, true), with_cb);
        let shape = check_return_shape_bytes(&tx.message_bytes, &ctx).expect("two ComputeBudget instructions are allowed");
        assert_eq!(shape.compute_budget_instructions, 2);
    }
}

#[test]
fn the_two_token_programs_give_different_messages() {
    let (a, _) = mint_vector(&classic());
    let (b, _) = mint_vector(&token2022());
    assert_ne!(a.message_bytes, b.message_bytes);
    assert_ne!(MINT_CLASSIC, MINT_T22);
    // Same signer set and shape, different token program and ATA seeds.
    assert_eq!(a.message.instructions.len(), b.message.instructions.len());
}

#[test]
fn builders_validate_before_building() {
    let v = token2022();
    let p = profile(&v);
    let ok = MintRequest { nonce: nonce(), recipient_wallet: RECIPIENT_WALLET, amount: AMOUNT, mint_id: MINT_ID, per_operation_cap: AMOUNT };
    assert!(build_mint_message(&p, &ok).is_ok(), "amount == cap is allowed");
    let with = |f: &dyn Fn(&mut MintRequest)| {
        let mut r = ok.clone();
        f(&mut r);
        build_mint_message(&p, &r)
    };
    assert_eq!(with(&|r| r.amount = 0), Err(TxError::InvalidAmount("amount must be at least 1")));
    assert_eq!(with(&|r| r.amount = AMOUNT + 1), Err(TxError::InvalidAmount("amount above the per-operation cap")));
    assert_eq!(with(&|r| r.per_operation_cap = 0), Err(TxError::InvalidAmount("amount above the per-operation cap")));
    // Recipients: System Program key, the mint, each role key, each token program id.
    for (bad, why) in [
        (SYSTEM_PROGRAM_ID, "the System Program key"),
        (MINT, "the mint"),
        (FEE_PAYER, "one of the bridge's own role keys"),
        (MINT_AUTHORITY, "one of the bridge's own role keys"),
        (RETURN_OWNER, "one of the bridge's own role keys"),
        (TOKEN_PROGRAM_ID, "a token program id"),
        (TOKEN_2022_PROGRAM_ID, "a token program id"),
    ] {
        assert_eq!(with(&|r| r.recipient_wallet = bad), Err(TxError::InvalidRecipient(why)), "{bad:?}");
    }
    assert_eq!(with(&|r| r.recipient_wallet = NONCE_ACCOUNT), Err(TxError::InvalidRecipient("the nonce account")));
    // The nonce account must be its own account.
    assert!(with(&|r| r.nonce.account = FEE_PAYER).is_err());
    assert!(with(&|r| r.nonce.account = RETURN_OWNER).is_err());
    assert!(with(&|r| r.nonce.account = SYSTEM_PROGRAM_ID).is_err());
    // Role keys must be distinct.
    let mut bad_profile = p.clone();
    bad_profile.mint_authority = bad_profile.fee_payer;
    assert_eq!(build_mint_message(&bad_profile, &ok), Err(TxError::InvalidProfile("role keys must be distinct")));
    let mut mint_is_role = p.clone();
    mint_is_role.mint = mint_is_role.return_owner;
    assert!(mint_is_role.validate().is_err());
    // Burn and refund amounts.
    assert!(build_burn_message(&p, &BurnRequest { nonce: nonce(), amount: 0, burn_id: BURN_ID }).is_err());
    assert!(build_refund_message(
        &p,
        &RefundRequest { nonce: nonce(), destination: RefundDestination::ExistingAccount { token_account: p.return_ata().unwrap() }, amount: 1, refund_id: REFUND_ID }
    )
    .is_err());
    assert!(build_refund_message(
        &p,
        &RefundRequest { nonce: nonce(), destination: RefundDestination::ExistingAccount { token_account: SENDER_SOURCE }, amount: 0, refund_id: REFUND_ID }
    )
    .is_err());
    assert!(build_refund_message(
        &p,
        &RefundRequest { nonce: nonce(), destination: RefundDestination::CreateAta { owner: RETURN_OWNER }, amount: 1, refund_id: REFUND_ID }
    )
    .is_err());
}

#[test]
fn a_mint_never_uses_an_unchecked_token_tag_and_the_memo_carries_the_mint_id() {
    for v in [classic(), token2022()] {
        let (tx, _) = mint_vector(&v);
        for ix in &tx.message.instructions {
            if tx.message.program_id(ix) == Some(&v.token) {
                assert!(![TAG_TRANSFER, TAG_MINT_TO, TAG_BURN, TAG_BATCH].contains(&ix.data[0]));
            }
        }
        let memo_ix = tx.message.instructions.last().unwrap();
        assert_eq!(memo_ix.data, format!("rumi1|m|{}", "a1".repeat(32)).into_bytes());
        assert!(memo_ix.accounts.is_empty(), "the memo has zero account metas");
    }
}

// ---------------------------------------------------------------------------
// Pinned message bytes (hex), produced by the independent Python reference.
// ---------------------------------------------------------------------------

const MINT_CLASSIC: &str = concat!(
    "0201060b11111111111111111111111111111111111111111111111111111111",
    "1111111122222222222222222222222222222222222222222222222222222222",
    "2222222244444444444444444444444444444444444444444444444444444444",
    "4444444455555555555555555555555555555555555555555555555555555555",
    "55555555b50e93ae0f8cc64a9ffdd974973a49603ae90d0c4d60296a53b15d6d",
    "c3f6dd9800000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a977777777777777777777777777777777777777777777777777777777",
    "777777778c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8",
    "dbe9f85966666666666666666666666666666666666666666666666666666666",
    "6666666604050303070004040000000a06000409020508010108030204010a0e",
    "15cd5b07000000000806004872756d69317c6d7c613161316131613161316131",
    "6131613161316131613161316131613161316131613161316131613161316131",
    "6131613161316131613161316131613161316131",
);

const MINT_T22: &str = concat!(
    "0201060b11111111111111111111111111111111111111111111111111111111",
    "1111111122222222222222222222222222222222222222222222222222222222",
    "22222222138fa40e79302538603035ca4ebd4bd5de8034049658ad4c3213c88d",
    "e03ce90344444444444444444444444444444444444444444444444444444444",
    "4444444455555555555555555555555555555555555555555555555555555555",
    "5555555500000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc77777777777777777777777777777777777777777777777777777777",
    "777777778c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8",
    "dbe9f85966666666666666666666666666666666666666666666666666666666",
    "6666666604050304070004040000000a06000209030508010108030302010a0e",
    "15cd5b07000000000806004872756d69317c6d7c613161316131613161316131",
    "6131613161316131613161316131613161316131613161316131613161316131",
    "6131613161316131613161316131613161316131",
);

const BURN_CLASSIC: &str = concat!(
    "0201040911111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "3333333344444444444444444444444444444444444444444444444444444444",
    "4444444455555555555555555555555555555555555555555555555555555555",
    "5555555556a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eaf",
    "ce708f8a00000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a966666666666666666666666666666666666666666666666666666666",
    "66666666030503030700040400000008030402010a0f15cd5b07000000000806",
    "004872756d69317c627c62326232623262326232623262326232623262326232",
    "6232623262326232623262326232623262326232623262326232623262326232",
    "62326232623262326232",
);

const BURN_T22: &str = concat!(
    "0201040911111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "3333333344444444444444444444444444444444444444444444444444444444",
    "4444444455555555555555555555555555555555555555555555555555555555",
    "555555559ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3",
    "bb2f5b3500000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc66666666666666666666666666666666666666666666666666666666",
    "66666666030503030700040400000008030402010a0f15cd5b07000000000806",
    "004872756d69317c627c62326232623262326232623262326232623262326232",
    "6232623262326232623262326232623262326232623262326232623262326232",
    "62326232623262326232",
);

const REFUND_EXISTING_CLASSIC: &str = concat!(
    "0201050a11111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "3333333355555555555555555555555555555555555555555555555555555555",
    "5555555556a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eaf",
    "ce708f8a99999999999999999999999999999999999999999999999999999999",
    "9999999900000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a944444444444444444444444444444444444444444444444444444444",
    "4444444466666666666666666666666666666666666666666666666666666666",
    "66666666030503020700040400000006004872756d69317c667c633363336333",
    "6333633363336333633363336333633363336333633363336333633363336333",
    "6333633363336333633363336333633363336333633363336333080403090401",
    "0a0c15cd5b070000000008",
);

const REFUND_EXISTING_T22: &str = concat!(
    "0201050a11111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "3333333355555555555555555555555555555555555555555555555555555555",
    "5555555599999999999999999999999999999999999999999999999999999999",
    "999999999ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3",
    "bb2f5b3500000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc44444444444444444444444444444444444444444444444444444444",
    "4444444466666666666666666666666666666666666666666666666666666666",
    "66666666030503020700040400000006004872756d69317c667c633363336333",
    "6333633363336333633363336333633363336333633363336333633363336333",
    "6333633363336333633363336333633363336333633363336333080404090301",
    "0a0c15cd5b070000000008",
);

const REFUND_CREATE_ATA_CLASSIC: &str = concat!(
    "0201070c11111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "3333333355555555555555555555555555555555555555555555555555555555",
    "5555555556a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eaf",
    "ce708f8a6d083824ef1c847e388ab6cf78bd8216b0611eb968810b2d0957c8e5",
    "7ea33ca200000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a944444444444444444444444444444444444444444444444444444444",
    "4444444488888888888888888888888888888888888888888888888888888888",
    "888888888c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8",
    "dbe9f85966666666666666666666666666666666666666666666666666666666",
    "6666666604050302070004040000000b0600040a090508010106004872756d69",
    "317c667c63336333633363336333633363336333633363336333633363336333",
    "6333633363336333633363336333633363336333633363336333633363336333",
    "633363330804030904010a0c15cd5b070000000008",
);

const REFUND_CREATE_ATA_T22: &str = concat!(
    "0201070c11111111111111111111111111111111111111111111111111111111",
    "1111111133333333333333333333333333333333333333333333333333333333",
    "333333334c3ac3ad6dbc463c96b6beba58dd81fe2b95eb235553c3a230a5f4e5",
    "dcc0029e55555555555555555555555555555555555555555555555555555555",
    "555555559ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3",
    "bb2f5b3500000000000000000000000000000000000000000000000000000000",
    "00000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06a7d517192c568ee08a845f73d29788cf035c3145b21ab344d8062e",
    "a940000006ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc44444444444444444444444444444444444444444444444444444444",
    "4444444488888888888888888888888888888888888888888888888888888888",
    "888888888c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8",
    "dbe9f85966666666666666666666666666666666666666666666666666666666",
    "6666666604050303070004040000000b0600020a090508010106004872756d69",
    "317c667c63336333633363336333633363336333633363336333633363336333",
    "6333633363336333633363336333633363336333633363336333633363336333",
    "633363330804040902010a0c15cd5b070000000008",
);

const RETURN_CLASSIC: &str = concat!(
    "01000306a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0",
    "a0a0a0a056a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eaf",
    "ce708f8aa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a1054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a944444444444444444444444444444444444444444444444444444444",
    "4444444477777777777777777777777777777777777777777777777777777777",
    "77777777020404020501000a0c15cd5b07000000000803002872756d69317c72",
    "7c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c34",
    "32",
);

const RETURN_T22: &str = concat!(
    "01000306a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0",
    "a0a0a0a09ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3",
    "bb2f5b35a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a1054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc44444444444444444444444444444444444444444444444444444444",
    "4444444477777777777777777777777777777777777777777777777777777777",
    "77777777020404020501000a0c15cd5b07000000000803002872756d69317c72",
    "7c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c34",
    "32",
);

const RETURN_COMPUTE_BUDGET_CLASSIC: &str = concat!(
    "01000407a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0",
    "a0a0a0a056a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eaf",
    "ce708f8aa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a10306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a",
    "40000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf585",
    "7eff00a944444444444444444444444444444444444444444444444444444444",
    "4444444477777777777777777777777777777777777777777777777777777777",
    "777777770403000502400d030003000903e8030000000000000504020601000a",
    "0c15cd5b07000000000804002872756d69317c727c6a746736722d6969686134",
    "2d64716f62792d68613464712d6f62797c2d7c3432",
);

const RETURN_COMPUTE_BUDGET_T22: &str = concat!(
    "01000407a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0",
    "a0a0a0a09ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3",
    "bb2f5b35a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a1a1a1a10306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a",
    "40000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa840",
    "4105448d06ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928",
    "d8a18bfc44444444444444444444444444444444444444444444444444444444",
    "4444444477777777777777777777777777777777777777777777777777777777",
    "777777770403000502400d030003000903e8030000000000000504020601000a",
    "0c15cd5b07000000000804002872756d69317c727c6a746736722d6969686134",
    "2d64716f62792d68613464712d6f62797c2d7c3432",
);
