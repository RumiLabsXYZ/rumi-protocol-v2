//! Exact transfer receipt verification for the native ICP ledger.
//!
//! The native ledger exposes its legacy `query_blocks` schema and does not
//! expose ICRC-3. Keep these wire types local to that ledger path; ICRC ledgers
//! continue to use `icrc3_proof`.

use candid::{CandidType, Principal};
use serde::Deserialize;
use sha2::{Digest, Sha224};

pub const ICP_LEDGER_PRINCIPAL_TEXT: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";

pub fn is_native_icp_ledger(ledger: Principal) -> bool {
    ledger.to_text() == ICP_LEDGER_PRINCIPAL_TEXT
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub timestamp_nanos: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub memo: u64,
    pub icrc1_memo: Option<Vec<u8>>,
    pub operation: Option<Operation>,
    pub created_at_time: Timestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
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
        fee: Tokens,
        expires_at: Option<Timestamp>,
        expected_allowance: Option<Tokens>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub parent_hash: Option<Vec<u8>>,
    pub transaction: Transaction,
    pub timestamp: Timestamp,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetBlocksArgs {
    pub start: u64,
    pub length: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BlockRange {
    pub blocks: Vec<Block>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum QueryArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}

pub type QueryArchiveResult = Result<BlockRange, QueryArchiveError>;

candid::define_function!(
    pub QueryArchiveFn : (GetBlocksArgs) -> (QueryArchiveResult) query
);

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ArchivedBlocksRange {
    pub start: u64,
    pub length: u64,
    pub callback: QueryArchiveFn,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct QueryBlocksResponse {
    pub chain_length: u64,
    pub certificate: Option<Vec<u8>>,
    pub blocks: Vec<Block>,
    pub first_block_index: u64,
    pub archived_blocks: Vec<ArchivedBlocksRange>,
}

/// Fetches one ledger-global native ICP block, following only the unique
/// ledger-provided archive callback whose range covers the requested index.
///
/// The native ledger's query reply is replicated but is not a certified block
/// hash proof here. This trusts the configured canonical ledger canister's
/// `query_blocks` implementation and its advertised archive callback; it never
/// accepts caller-supplied archive locations.
pub async fn query_block(ledger: Principal, block_index: u64) -> Result<Block, String> {
    if !is_native_icp_ledger(ledger) {
        return Err("native ICP proof requested for a non-native ledger".into());
    }
    let request = GetBlocksArgs {
        start: block_index,
        length: 1,
    };
    let (response,): (QueryBlocksResponse,) =
        ic_cdk::call(ledger, "query_blocks", (request.clone(),))
            .await
            .map_err(|(code, message)| {
                format!("native ICP query_blocks failed: {code:?} {message}")
            })?;

    if response.first_block_index == block_index
        && response.blocks.len() == 1
        && response.archived_blocks.is_empty()
    {
        return response
            .blocks
            .into_iter()
            .next()
            .ok_or_else(|| "native ICP query_blocks returned no requested block".into());
    }
    if !response.blocks.is_empty() {
        return Err("native ICP query_blocks returned ambiguous block evidence".into());
    }

    let mut matching_archives = response.archived_blocks.into_iter().filter(|archive| {
        archive
            .start
            .checked_add(archive.length)
            .is_some_and(|end| {
                archive.length > 0 && block_index >= archive.start && block_index < end
            })
    });
    let archive = matching_archives.next().ok_or_else(|| {
        "native ICP block is not present in the ledger range or its archives".to_string()
    })?;
    if matching_archives.next().is_some() {
        return Err("native ICP query_blocks returned overlapping archive evidence".into());
    }

    let (result,): (QueryArchiveResult,) = ic_cdk::call(
        archive.callback.0.principal,
        &archive.callback.0.method,
        (request,),
    )
    .await
    .map_err(|(code, message)| format!("native ICP archive callback failed: {code:?} {message}"))?;
    match result {
        Ok(range) if range.blocks.len() == 1 => range
            .blocks
            .into_iter()
            .next()
            .ok_or_else(|| "native ICP archive callback returned no requested block".into()),
        Ok(_) => Err("native ICP archive callback returned an ambiguous block range".into()),
        Err(_) => Err("native ICP archive callback rejected the requested block".into()),
    }
}

/// Verifies all return/claim transfer fields represented by the native ICP
/// ledger block and returns the actual fee charged by that block.
pub fn verify_transfer_block(
    block: &Block,
    from: Principal,
    to: Principal,
    amount_e8s: u64,
    memo: &[u8],
    created_at_time_ns: u64,
) -> Result<u64, String> {
    let Some(Operation::Transfer {
        from: actual_from,
        to: actual_to,
        spender,
        amount,
        fee,
    }) = block.transaction.operation.as_ref()
    else {
        return Err("native ICP block is not an ordinary transfer".into());
    };
    if spender.is_some() {
        return Err("native ICP transfer unexpectedly names a spender".into());
    }
    if actual_from.as_slice() != account_identifier(from).as_slice() {
        return Err(
            "native ICP transfer sender does not match the expected default account".into(),
        );
    }
    if actual_to.as_slice() != account_identifier(to).as_slice() {
        return Err(
            "native ICP transfer recipient does not match the expected default account".into(),
        );
    }
    if amount.e8s != amount_e8s {
        return Err("native ICP transfer amount does not match the expected amount".into());
    }
    if block.transaction.icrc1_memo.as_deref() != Some(memo) {
        return Err("native ICP transfer ICRC-1 memo does not match".into());
    }
    if block.transaction.created_at_time.timestamp_nanos != created_at_time_ns {
        return Err("native ICP transfer created_at_time does not match".into());
    }
    Ok(fee.e8s)
}

/// Derives the legacy ICP Ledger AccountIdentifier for an owner's default
/// account: CRC32(SHA-224("\x0Aaccount-id" || principal || zero subaccount)).
pub fn account_identifier(owner: Principal) -> [u8; 32] {
    let mut hasher = Sha224::new();
    hasher.update(b"\x0Aaccount-id");
    hasher.update(owner.as_slice());
    hasher.update([0; 32]);
    let hash = hasher.finalize();
    let checksum = crc32_ieee(&hash).to_be_bytes();
    let mut identifier = [0; 32];
    identifier[..4].copy_from_slice(&checksum);
    identifier[4..].copy_from_slice(&hash);
    identifier
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{
        account_identifier, verify_transfer_block, Block, Operation, Timestamp, Tokens,
        Transaction,
    };
    use candid::Principal;

    fn transfer_block(from: Principal, to: Principal, amount: u64, fee: u64) -> Block {
        Block {
            parent_hash: None,
            transaction: Transaction {
                memo: 0,
                icrc1_memo: Some(b"claim-return".to_vec()),
                operation: Some(Operation::Transfer {
                    from: account_identifier(from).to_vec(),
                    to: account_identifier(to).to_vec(),
                    spender: None,
                    amount: Tokens { e8s: amount },
                    fee: Tokens { e8s: fee },
                }),
                created_at_time: Timestamp {
                    timestamp_nanos: 123,
                },
            },
            timestamp: Timestamp {
                timestamp_nanos: 123,
            },
        }
    }

    #[test]
    fn native_transfer_proof_checks_exact_default_account_tuple_and_actual_fee() {
        let bot = Principal::from_slice(&[1]);
        let backend = Principal::from_slice(&[2]);
        let block = transfer_block(bot, backend, 90, 10);
        assert_eq!(
            verify_transfer_block(&block, bot, backend, 90, b"claim-return", 123),
            Ok(10)
        );

        assert!(verify_transfer_block(
            &block,
            Principal::from_slice(&[3]),
            backend,
            90,
            b"claim-return",
            123
        )
        .is_err());
        assert!(verify_transfer_block(
            &block,
            bot,
            Principal::from_slice(&[3]),
            90,
            b"claim-return",
            123
        )
        .is_err());
        assert!(verify_transfer_block(&block, bot, backend, 89, b"claim-return", 123).is_err());
        assert!(verify_transfer_block(&block, bot, backend, 90, b"wrong-memo", 123).is_err());
        assert!(verify_transfer_block(&block, bot, backend, 90, b"claim-return", 124).is_err());

        let mut spender = block.clone();
        if let Some(Operation::Transfer { spender, .. }) = spender.transaction.operation.as_mut() {
            *spender = Some(vec![0x44; 32]);
        }
        assert!(verify_transfer_block(&spender, bot, backend, 90, b"claim-return", 123).is_err());
    }

}
