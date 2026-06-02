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
