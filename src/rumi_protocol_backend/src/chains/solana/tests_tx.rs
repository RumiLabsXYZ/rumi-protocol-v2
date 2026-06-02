use super::tx::*;
use solana_message::Hash;
use solana_pubkey::Pubkey;

// ─── compact-u16 (short_vec length prefix) ───────────────────────────────────
// Vectors are the canonical ones asserted in solana-short-vec's own test suite
// (`assert_len_encoding`), so this proves our hand-encoder is byte-identical to
// the encoding `Message::serialize()` would emit.

#[test]
fn compact_u16_matches_canonical_vectors() {
    assert_eq!(encode_compact_u16(0x00), vec![0x00]);
    assert_eq!(encode_compact_u16(0x7f), vec![0x7f]);
    assert_eq!(encode_compact_u16(0x80), vec![0x80, 0x01]);
    assert_eq!(encode_compact_u16(0xff), vec![0xff, 0x01]);
    assert_eq!(encode_compact_u16(0x100), vec![0x80, 0x02]);
    // 0x4000 is the lower 3-byte boundary: the exact value where the SECOND
    // continuation byte first appears (0x3fff still fits in two bytes).
    assert_eq!(encode_compact_u16(0x4000), vec![0x80, 0x80, 0x01]);
    assert_eq!(encode_compact_u16(0x7fff), vec![0xff, 0xff, 0x01]);
    assert_eq!(encode_compact_u16(0xffff), vec![0xff, 0xff, 0x03]);
}

#[test]
fn compact_u16_count_one_is_single_byte() {
    // The single-signature count, which assemble_wire_tx prepends.
    assert_eq!(encode_compact_u16(1), vec![0x01]);
}

// ─── assemble_wire_tx (Step 1) ───────────────────────────────────────────────

#[test]
fn wire_tx_length_and_sig_count_byte() {
    let sig = [9u8; 64];
    let message_bytes = vec![0xAA; 100];
    let wire = assemble_wire_tx(sig, &message_bytes);
    // [compact-u16 sig count = 1][64 sig bytes][message bytes].
    assert_eq!(wire.len(), 1 + 64 + message_bytes.len());
    assert_eq!(wire[0], 1, "first byte is the compact-u16 signature count (1)");
    assert_eq!(&wire[1..65], &sig, "signature follows the count");
    assert_eq!(&wire[65..], &message_bytes[..], "message follows the signature");
}

#[test]
fn wire_tx_empty_message() {
    let sig = [0u8; 64];
    let wire = assemble_wire_tx(sig, &[]);
    assert_eq!(wire.len(), 1 + 64);
    assert_eq!(wire[0], 1);
}

// ─── system_transfer_instruction (hand-encoded data layout) ──────────────────

#[test]
fn transfer_instruction_data_is_tag2_plus_lamports_le() {
    let from = Pubkey::new_from_array([1u8; 32]);
    let to = Pubkey::new_from_array([2u8; 32]);
    let ix = system_transfer_instruction(&from, &to, 0x0102030405060708);

    // program_id is the System Program (32 zero bytes).
    assert_eq!(ix.program_id, Pubkey::new_from_array([0u8; 32]));
    // data = [tag u32 LE = 2][lamports u64 LE]. 12 bytes total.
    assert_eq!(
        ix.data,
        vec![
            0x02, 0x00, 0x00, 0x00, // tag = 2 (Transfer), u32 LE
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // lamports u64 LE
        ]
    );
    // Two accounts: from (signer, writable), to (non-signer, writable).
    assert_eq!(ix.accounts.len(), 2);
    assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
    assert!(!ix.accounts[1].is_signer && ix.accounts[1].is_writable);
    assert_eq!(ix.accounts[0].pubkey, from);
    assert_eq!(ix.accounts[1].pubkey, to);
}

// ─── build_transfer_message (Step 2) ─────────────────────────────────────────

#[test]
fn transfer_message_has_one_instruction_and_correct_fee_payer() {
    let from = Pubkey::new_from_array([1u8; 32]);
    let to = Pubkey::new_from_array([2u8; 32]);
    let blockhash = Hash::new_from_array([3u8; 32]);

    let msg = build_transfer_message(&from, &to, 1_000_000, blockhash);

    assert_eq!(msg.instructions.len(), 1, "exactly one transfer instruction");
    // The fee payer is account_keys[0] and must equal `from`.
    assert_eq!(msg.account_keys[0], from, "fee payer is the `from` account");
    // from + to + System Program id => 3 distinct account keys.
    assert!(
        msg.account_keys.contains(&to),
        "recipient must appear in account_keys"
    );
    // The signer (fee payer) is the single required signature.
    assert_eq!(msg.header.num_required_signatures, 1);
    // Blockhash round-trips into the message.
    assert_eq!(msg.recent_blockhash.to_bytes(), [3u8; 32]);
}

// ─── serialize_legacy_message (wire-format consistency) ──────────────────────

#[test]
fn serialized_message_layout_is_well_formed() {
    let from = Pubkey::new_from_array([1u8; 32]);
    let to = Pubkey::new_from_array([2u8; 32]);
    let blockhash = Hash::new_from_array([4u8; 32]);
    let msg = build_transfer_message(&from, &to, 42, blockhash);

    let bytes = serialize_legacy_message(&msg);

    // Header is the first three bytes, num_required_signatures == 1.
    assert_eq!(bytes[0], 1, "num_required_signatures");
    // account_keys length prefix is at offset 3; a System Transfer has 3 keys
    // (from, to, System Program), which fits in one compact-u16 byte.
    let n_keys = bytes[3];
    assert_eq!(n_keys, msg.account_keys.len() as u8);
    // The fee payer (account_keys[0]) is serialized immediately after the prefix.
    assert_eq!(&bytes[4..36], from.as_ref(), "first account key is the fee payer");
    // The recent blockhash appears after all account keys.
    let bh_offset = 4 + (n_keys as usize) * 32;
    assert_eq!(&bytes[bh_offset..bh_offset + 32], &[4u8; 32], "recent blockhash");

    // Total length is internally consistent: re-serializing is deterministic.
    assert_eq!(serialize_legacy_message(&msg), bytes);
}

#[test]
fn serialized_message_emits_two_byte_length_prefix_for_large_instruction_data() {
    // The well-formed test above only exercises 1-byte compact-u16 prefixes (a
    // 3-key System Transfer with 12 bytes of data). Here we force the multi-byte
    // length path through serialize_legacy_message by hand-building a Message with
    // a single CompiledInstruction whose data is 200 bytes long. 200 (0xc8)
    // exceeds 127, so its short_vec length prefix is the 2-byte [0xc8, 0x01].
    use solana_message::compiled_instruction::CompiledInstruction;
    use solana_message::{Message, MessageHeader};

    // Anchor: confirm the expected 2-byte prefix for length 200 up front.
    assert_eq!(encode_compact_u16(200), vec![0xc8, 0x01]);

    let program = Pubkey::new_from_array([5u8; 32]);
    let data = vec![0xABu8; 200];
    let ix = CompiledInstruction {
        program_id_index: 0,
        accounts: vec![], // empty accounts -> single 0x00 length byte, keeps offsets simple
        data: data.clone(),
    };
    let msg = Message {
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        },
        account_keys: vec![program], // one key (the program); 1 fits in a single prefix byte
        recent_blockhash: Hash::new_from_array([6u8; 32]),
        instructions: vec![ix],
    };

    let bytes = serialize_legacy_message(&msg);

    // Walk the layout to the data-length prefix offset:
    //   [0..3]   header (3 bytes)
    //   [3]      account_keys len prefix (1 key -> single byte 0x01)
    //   [4..36]  the one account key (32 bytes)
    //   [36..68] recent_blockhash (32 bytes)
    //   [68]     instructions len prefix (1 ix -> single byte 0x01)
    //   [69]     program_id_index (u8)
    //   [70]     accounts len prefix (0 accounts -> single byte 0x00)
    //   [71..73] data len prefix -> the 2-byte [0xc8, 0x01] we are proving
    //   [73..]   the 200 data bytes
    let data_len_prefix_off = 3 + 1 + 32 + 32 + 1 + 1 + 1;
    assert_eq!(
        &bytes[data_len_prefix_off..data_len_prefix_off + 2],
        &[0xc8, 0x01],
        "200-byte instruction data must serialize a 2-byte compact-u16 length prefix"
    );
    // The data bytes follow the 2-byte prefix verbatim.
    let data_off = data_len_prefix_off + 2;
    assert_eq!(&bytes[data_off..data_off + 200], &data[..], "data follows its prefix");
    // And the message ends exactly there (no trailing bytes).
    assert_eq!(bytes.len(), data_off + 200, "no trailing bytes after the instruction data");
}

// ─── Task 2: program-id constants ────────────────────────────────────────────
// Each parsed program id must round-trip back to its known base58 string. This
// guards against a typo in the hand-copied literal (a wrong program id silently
// produces wrong instructions/PDAs that only fail on-chain).

#[test]
fn program_ids_roundtrip_to_known_base58() {
    assert_eq!(
        bs58::encode(token_program_id().as_ref()).into_string(),
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "SPL Token program id"
    );
    assert_eq!(
        bs58::encode(ata_program_id().as_ref()).into_string(),
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
        "Associated Token Account program id"
    );
    // The System Program is the all-zero address (base58 "111...111", 32 ones).
    assert_eq!(
        bs58::encode(system_program_id().as_ref()).into_string(),
        "11111111111111111111111111111111",
        "System program id"
    );
    assert_eq!(system_program_id().as_ref(), &[0u8; 32], "System program is 32 zero bytes");
}

// ─── Task 2 Step 1: derive_ata (PDA known-answer + invariants) ───────────────
//
// Known-answer vector sourced from the canonical SPL Associated Token Account
// test suite (solana-program/associated-token-account, pinocchio test
// `create_with_args_accepts_canonical_bump_hint`):
//   https://github.com/solana-program/associated-token-account/blob/main/pinocchio/program/tests/bump.rs
// That suite fixes mint = 8N6gdBxJaZUG9cBnSSaHDsx7vMeQ4VR1LmCmk9SCu38s and wallet
// 1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG with the standard SPL Token program,
// and asserts the canonical bump == 255. The resulting ATA address below
// (3kP4RCoX8u1PhyiUkuNpFfbUmFF32ZC9uscL7H84Xs2u) was cross-derived two independent
// ways from that exact (owner, token_program, mint, ATA program) tuple (once with
// the official @solana/kit `@solana/addresses` getProgramDerivedAddress, and once
// with a hand-rolled reference impl of the PDA algorithm), both yielding bump 255,
// matching the SPL suite's asserted canonical bump.
const ATA_OWNER_B58: &str = "1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG";
const ATA_MINT_B58: &str = "8N6gdBxJaZUG9cBnSSaHDsx7vMeQ4VR1LmCmk9SCu38s";
const ATA_EXPECTED_B58: &str = "3kP4RCoX8u1PhyiUkuNpFfbUmFF32ZC9uscL7H84Xs2u";

/// Decode a base58 Solana address literal into a `Pubkey` (test helper).
fn pk(b58: &str) -> Pubkey {
    let bytes = bs58::decode(b58).into_vec().expect("valid base58");
    let arr: [u8; 32] = bytes.as_slice().try_into().expect("32-byte address");
    Pubkey::new_from_array(arr)
}

#[test]
fn derive_ata_matches_known_answer_vector() {
    let owner = pk(ATA_OWNER_B58);
    let mint = pk(ATA_MINT_B58);
    let ata = derive_ata(&owner, &mint);
    assert_eq!(
        bs58::encode(ata.as_ref()).into_string(),
        ATA_EXPECTED_B58,
        "derived ATA must match the canonical SPL-sourced known-answer vector"
    );
}

#[test]
fn derive_ata_structural_invariants() {
    let owner = pk(ATA_OWNER_B58);
    let mint = pk(ATA_MINT_B58);
    let ata = derive_ata(&owner, &mint);

    // Deterministic: two derivations agree.
    assert_eq!(ata, derive_ata(&owner, &mint), "derivation is deterministic");
    // A PDA is never the owner (or the mint).
    assert_ne!(ata, owner, "ATA must differ from the owner");
    assert_ne!(ata, mint, "ATA must differ from the mint");
    // find_program_address succeeded -> the result is off the ed25519 curve
    // (PDAs have no private key). Available now that curve25519 is enabled.
    assert!(!ata.is_on_curve(), "ATA (a PDA) must be off-curve");

    // Independent re-derivation: recompute the address from the canonical bump
    // (255 for this vector, per the SPL suite) via create_program_address and
    // confirm it equals find_program_address's result.
    let recomputed = Pubkey::create_program_address(
        &[owner.as_ref(), token_program_id().as_ref(), mint.as_ref(), &[255u8]],
        &ata_program_id(),
    )
    .expect("canonical bump 255 yields a valid off-curve PDA");
    assert_eq!(recomputed, ata, "create_program_address(bump=255) matches find_program_address");
}

// ─── Task 2 Step 2: mint_to_ix (SPL Token MintTo) ────────────────────────────

#[test]
fn mint_to_ix_data_is_tag7_plus_amount_le() {
    let mint = Pubkey::new_from_array([1u8; 32]);
    let dest_ata = Pubkey::new_from_array([2u8; 32]);
    let authority = Pubkey::new_from_array([3u8; 32]);
    let ix = mint_to_ix(&mint, &dest_ata, &authority, 0x0102030405060708);

    // program is the SPL Token program.
    assert_eq!(ix.program_id, token_program_id(), "MintTo runs on the Token program");
    // data = [discriminant 7][amount u64 LE]. 9 bytes total.
    assert_eq!(
        ix.data,
        vec![
            0x07, // MintTo discriminant
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // amount u64 LE
        ],
        "data is the MintTo tag (7) followed by the amount little-endian"
    );
    assert_eq!(ix.data.len(), 9);

    // accounts: [mint (writable, non-signer), dest_ata (writable, non-signer),
    //            authority (read-only, signer)].
    assert_eq!(ix.accounts.len(), 3);
    assert_eq!(ix.accounts[0].pubkey, mint);
    assert!(ix.accounts[0].is_writable && !ix.accounts[0].is_signer, "mint: writable, not signer");
    assert_eq!(ix.accounts[1].pubkey, dest_ata);
    assert!(ix.accounts[1].is_writable && !ix.accounts[1].is_signer, "dest_ata: writable, not signer");
    assert_eq!(ix.accounts[2].pubkey, authority);
    assert!(!ix.accounts[2].is_writable && ix.accounts[2].is_signer, "authority: read-only signer");
}

// ─── Task 2 Step 3: create_ata_idempotent_ix (ATA CreateIdempotent) ──────────

#[test]
fn create_ata_idempotent_ix_data_and_account_layout() {
    let funder = Pubkey::new_from_array([9u8; 32]);
    let owner = pk(ATA_OWNER_B58);
    let mint = pk(ATA_MINT_B58);
    let ix = create_ata_idempotent_ix(&funder, &owner, &mint);

    // program is the Associated Token Account program.
    assert_eq!(ix.program_id, ata_program_id(), "runs on the ATA program");
    // data is the single CreateIdempotent discriminant byte (1).
    assert_eq!(ix.data, vec![1u8], "CreateIdempotent discriminant");

    // accounts: [funder (w, signer), ata (w), owner (ro), mint (ro),
    //            system (ro), token program (ro)].
    assert_eq!(ix.accounts.len(), 6);

    assert_eq!(ix.accounts[0].pubkey, funder);
    assert!(ix.accounts[0].is_writable && ix.accounts[0].is_signer, "funder: writable signer");

    // The derived ATA appears at index 1 (writable, non-signer).
    assert_eq!(ix.accounts[1].pubkey, derive_ata(&owner, &mint), "index 1 is the derived ATA");
    assert!(ix.accounts[1].is_writable && !ix.accounts[1].is_signer, "ata: writable, not signer");

    assert_eq!(ix.accounts[2].pubkey, owner);
    assert!(!ix.accounts[2].is_writable && !ix.accounts[2].is_signer, "owner: read-only");

    assert_eq!(ix.accounts[3].pubkey, mint);
    assert!(!ix.accounts[3].is_writable && !ix.accounts[3].is_signer, "mint: read-only");

    assert_eq!(ix.accounts[4].pubkey, system_program_id());
    assert!(!ix.accounts[4].is_writable && !ix.accounts[4].is_signer, "system program: read-only");

    assert_eq!(ix.accounts[5].pubkey, token_program_id());
    assert!(!ix.accounts[5].is_writable && !ix.accounts[5].is_signer, "token program: read-only");
}

// ─── Task 2 Step 4: build_mint_message ───────────────────────────────────────

#[test]
fn build_mint_message_has_two_instructions_and_correct_programs() {
    let authority = pk(ATA_OWNER_B58); // any pubkey works as the fee payer/authority
    let mint = pk(ATA_MINT_B58);
    let recipient_owner = Pubkey::new_from_array([7u8; 32]);
    let blockhash = Hash::new_from_array([8u8; 32]);

    let msg = build_mint_message(&authority, &mint, &recipient_owner, 1_000_000, blockhash);

    // Two instructions: [0] ATA create-idempotent, [1] MintTo.
    assert_eq!(msg.instructions.len(), 2, "create-ATA then MintTo");

    // Resolve each compiled instruction's program id back through account_keys.
    let prog0 = msg.account_keys[msg.instructions[0].program_id_index as usize];
    let prog1 = msg.account_keys[msg.instructions[1].program_id_index as usize];
    assert_eq!(prog0, ata_program_id(), "first instruction runs on the ATA program");
    assert_eq!(prog1, token_program_id(), "second instruction runs on the Token program");

    // Fee payer is the authority (account_keys[0]) and is the single signer.
    assert_eq!(msg.account_keys[0], authority, "fee payer is the authority");
    assert_eq!(msg.header.num_required_signatures, 1, "only the authority signs");

    // The MintTo data carries the requested amount (1_000_000) little-endian.
    let mint_to_data = &msg.instructions[1].data;
    assert_eq!(mint_to_data[0], 7u8, "second instruction is MintTo (tag 7)");
    assert_eq!(
        &mint_to_data[1..9],
        &1_000_000u64.to_le_bytes(),
        "MintTo amount round-trips into the message"
    );

    // Blockhash round-trips.
    assert_eq!(msg.recent_blockhash.to_bytes(), [8u8; 32]);
}
