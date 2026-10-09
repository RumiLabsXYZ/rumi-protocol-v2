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
