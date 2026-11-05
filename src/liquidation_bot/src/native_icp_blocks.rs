//! Pure native ICP `query_blocks` decoding, range selection, and treasury
//! tuple verification shared by the bot and its official-ledger fixture.

use candid::{CandidType, Decode, Principal};
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
use serde::Deserialize;
use sha2::{Digest, Sha224};

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GetBlocksArgs {
    pub start: u64,
    pub length: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Tokens {
    pub e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Timestamp {
    pub timestamp_nanos: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Transaction {
    pub memo: u64,
    pub icrc1_memo: Option<Vec<u8>>,
    pub operation: Option<Operation>,
    pub created_at_time: Timestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Block {
    pub transaction: Transaction,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum Operation {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: Tokens,
    },
    Mint {
        to: Vec<u8>,
        amount: Tokens,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: Tokens,
        fee: Tokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: Tokens,
        expected_allowance: Option<Tokens>,
        fee: Tokens,
        expires_at: Option<Timestamp>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct BlockRange {
    pub blocks: Vec<Block>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}

pub type ArchiveResult = Result<BlockRange, ArchiveError>;

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ArchivedRange {
    pub start: u64,
    pub length: u64,
    pub callback: QueryArchiveFn<GetBlocksArgs, ArchiveResult>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct QueryBlocksResponse {
    pub chain_length: u64,
    pub certificate: Option<Vec<u8>>,
    pub blocks: Vec<Block>,
    pub first_block_index: u64,
    pub archived_blocks: Vec<ArchivedRange>,
}

#[derive(Clone, Debug)]
pub enum BlockSource {
    Direct(Block),
    Archive {
        canister_id: Principal,
        method: String,
    },
}

pub fn decode_query_blocks(bytes: &[u8]) -> Result<QueryBlocksResponse, String> {
    Decode!(bytes, QueryBlocksResponse)
        .map_err(|error| format!("could not decode native ICP query_blocks response: {error}"))
}

pub fn decode_archive_result(bytes: &[u8]) -> Result<ArchiveResult, String> {
    Decode!(bytes, ArchiveResult)
        .map_err(|error| format!("could not decode native ICP archive callback response: {error}"))
}

pub fn select_block_source(
    response: QueryBlocksResponse,
    block_index: u64,
) -> Result<BlockSource, String> {
    if response.first_block_index == block_index
        && response.blocks.len() == 1
        && response.archived_blocks.is_empty()
    {
        return Ok(BlockSource::Direct(
            response.blocks.into_iter().next().expect("length checked"),
        ));
    }
    if !response.blocks.is_empty() {
        return Err("ICP query_blocks returned ambiguous direct block evidence".into());
    }

    let mut matching_archives = response.archived_blocks.into_iter().filter(|archive| {
        archive.length > 0
            && archive
                .start
                .checked_add(archive.length)
                .is_some_and(|end| block_index >= archive.start && block_index < end)
    });
    let archive = matching_archives.next().ok_or_else(|| {
        format!("ICP block {block_index} is absent from the direct range and advertised archives")
    })?;
    if matching_archives.next().is_some() {
        return Err("ICP query_blocks returned overlapping archive evidence".into());
    }
    Ok(BlockSource::Archive {
        canister_id: archive.callback.canister_id,
        method: archive.callback.method,
    })
}

pub fn verify_treasury_transfer(
    block: &Block,
    sender: Principal,
    treasury: Principal,
    amount_e8s: u64,
    fee_e8s: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    let operation = block
        .transaction
        .operation
        .as_ref()
        .ok_or("ICP ledger block has no operation")?;
    let Operation::Transfer {
        from,
        to,
        spender,
        amount,
        fee,
    } = operation
    else {
        return Err("ICP ledger receipt block is not a transfer".into());
    };
    let sender_id = default_account_identifier(sender);
    let treasury_id = default_account_identifier(treasury);
    if spender.is_some()
        || *from != sender_id
        || *to != treasury_id
        || amount.e8s != amount_e8s
        || fee.e8s != fee_e8s
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("ICP ledger block does not match the persisted treasury transfer tuple".into());
    }
    Ok(())
}

/// Verify the exact native-ICP ICPSwap input leg for the bounded vault-158
/// recovery. ICPSwap records the memo in the native ledger's `icrc1_memo` byte
/// field; created_at_time is checked separately because the transfer did not
/// use the ICRC-1 created_at_time dedup field.
pub fn verify_swap_input_transfer(
    block: &Block,
    sender: Principal,
    pool: Principal,
    amount_e8s: u64,
    fee_e8s: u64,
    memo: &[u8],
    created_at_time: u64,
) -> Result<(), String> {
    let operation = block.transaction.operation.as_ref().ok_or("ICP ledger block has no operation")?;
    let Operation::Transfer { from, to, spender, amount, fee } = operation else {
        return Err("ICP swap input block is not a native ICP transfer".into());
    };
    if spender.as_deref() != Some(default_account_identifier(pool).as_slice())
        || *from != default_account_identifier(sender)
        || *to != default_account_identifier(pool)
        || amount.e8s != amount_e8s
        || fee.e8s != fee_e8s
        || block.transaction.memo != 0
        || block.transaction.icrc1_memo.as_deref() != Some(memo)
        || block.transaction.created_at_time.timestamp_nanos != created_at_time
    {
        return Err("native ICP swap input block does not match the pinned pool transfer".into());
    }
    Ok(())
}

pub fn default_account_identifier(owner: Principal) -> Vec<u8> {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0; 32]);
    let hash = hasher.finalize();
    let mut crc = !0u32;
    for byte in hash.iter() {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut account = Vec::with_capacity(32);
    account.extend_from_slice(&(!crc).to_be_bytes());
    account.extend_from_slice(&hash);
    account
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(input: &str) -> Vec<u8> {
        let input = input.trim();
        input.as_bytes().chunks_exact(2).map(|pair| {
            let digit = |c: u8| match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("invalid fixture hex"),
            };
            digit(pair[0]) * 16 + digit(pair[1])
        }).collect()
    }

    #[test]
    fn supplied_wrong_block_index_is_rejected() {
        let response = QueryBlocksResponse {
            chain_length: 6,
            certificate: None,
            blocks: vec![Block {
                transaction: Transaction {
                    memo: 0,
                    icrc1_memo: None,
                    operation: None,
                    created_at_time: Timestamp { timestamp_nanos: 1 },
                },
            }],
            first_block_index: 5,
            archived_blocks: vec![],
        };
        assert!(select_block_source(response, 6).is_err());
    }

    fn vault158_input_block(sender: Principal, pool: Principal, amount: u64, fee: u64, memo: Option<Vec<u8>>) -> Block {
        Block {
            transaction: Transaction {
                memo: 0,
                icrc1_memo: memo,
                operation: Some(Operation::Transfer {
                    from: default_account_identifier(sender),
                    to: default_account_identifier(pool),
                    spender: Some(default_account_identifier(pool)),
                    amount: Tokens { e8s: amount },
                    fee: Tokens { e8s: fee },
                }),
                created_at_time: Timestamp { timestamp_nanos: 1_793_306_981_079_625_901 },
            },
        }
    }

    #[test]
    fn vault158_native_swap_input_requires_exact_sender_amount_fee_memo_and_timestamp() {
        let sender = Principal::from_slice(&[1]);
        let pool = Principal::from_slice(&[2]);
        let memo = [0, 0, 0, 0, 0, 0x11, 0xbe, 0x56];
        let block = vault158_input_block(sender, pool, 226_020_933, 10_000, Some(memo.to_vec()));
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 10_000, &memo,
            1_793_306_981_079_625_901,
        ).is_ok());
        assert!(verify_swap_input_transfer(
            &block, Principal::from_slice(&[3]), pool, 226_020_933, 10_000, &memo,
            1_793_306_981_079_625_901,
        ).is_err(), "wrong sender must fail");
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_934, 10_000, &memo,
            1_793_306_981_079_625_901,
        ).is_err(), "wrong amount must fail");
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 10_001, &memo,
            1_793_306_981_079_625_901,
        ).is_err(), "wrong fee must fail");
        assert!(verify_swap_input_transfer(
            &vault158_input_block(sender, pool, 226_020_933, 10_000, Some(vec![9])),
            sender, pool, 226_020_933, 10_000, &memo, 1_793_306_981_079_625_901,
        ).is_err(), "wrong memo must fail");
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 10_000, &memo, 1,
        ).is_err(), "wrong timestamp must fail");
    }

    #[test]
    fn archived_vault158_native_receipt_fixture_matches_exact_transferfrom_block() {
        // Captured from ICP ledger query_blocks(39835236)'s advertised archive
        // callback q3fc5-haaaa-aaaaa-aaahq-cai.get_blocks on 2026-11-02.
        let response = unhex(include_str!("../tests/fixtures/vault158-icp-input-block-39835236.hex"));
        let blocks = decode_archive_result(&response).expect("captured ICP archive response decodes");
        let mut block = blocks.expect("archive response succeeds").blocks;
        assert_eq!(block.len(), 1);
        let block = block.remove(0);
        let sender = Principal::from_text("nygob-3qaaa-aaaap-qttcq-cai").unwrap();
        let pool = Principal::from_text("mohjv-bqaaa-aaaag-qjyia-cai").unwrap();
        let memo = [0, 0, 0, 0, 0, 0x11, 0xbe, 0x56];
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 10_000, &memo,
            1_793_306_981_079_625_901,
        ).is_ok());
        assert!(verify_swap_input_transfer(
            &block, Principal::anonymous(), pool, 226_020_933, 10_000, &memo,
            1_793_306_981_079_625_901,
        ).is_err(), "wrong bot must fail against captured evidence");
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 9_999, &memo,
            1_793_306_981_079_625_901,
        ).is_err(), "wrong fee must fail against captured evidence");
        assert!(verify_swap_input_transfer(
            &block, sender, pool, 226_020_933, 10_000, &[9],
            1_793_306_981_079_625_901,
        ).is_err(), "wrong memo must fail against captured evidence");
    }
}
