// ICRC-3 transaction log query endpoints for the 3USD LP token.
//
// Enables the DFINITY ic-icrc1-index-ng canister to index all LP token
// transactions (mint, burn, transfer, approve) by polling icrc3_get_blocks.

use candid::{CandidType, Nat, Principal};
use serde::{Deserialize, Serialize};

use crate::state::read_state;
use crate::types::{Icrc3Block, Icrc3Transaction};

// ─── ICRC-3 Value (generic block encoding) ───

/// Generic value type used by ICRC-3 to encode blocks as nested maps.
/// The index-ng expects this exact Candid structure.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub enum Icrc3Value {
    Blob(Vec<u8>),
    Text(String),
    Nat(Nat),
    Int(candid::Int),
    Array(Vec<Icrc3Value>),
    Map(Vec<(String, Icrc3Value)>),
}

// ─── Request / Response types ───

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct GetBlocksArgs {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct BlockWithId {
    pub id: Nat,
    pub block: Icrc3Value,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct GetBlocksResult {
    pub log_length: Nat,
    pub blocks: Vec<BlockWithId>,
    pub archived_blocks: Vec<ArchivedBlocks>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ArchivedBlocks {
    pub args: Vec<GetBlocksArgs>,
    pub callback: ArchivedBlocksCallback,
}

/// Candid `func` reference — we never actually use archives, but the type
/// must be present in the response for Candid compatibility.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ArchivedBlocksCallback {
    pub canister_id: Principal,
    pub method: String,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct GetArchivesArgs {
    pub from: Option<Principal>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub canister_id: Principal,
    pub start: Nat,
    pub end: Nat,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct GetArchivesResult {
    pub archives: Vec<ArchiveInfo>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct Icrc3DataCertificate {
    pub certificate: Vec<u8>,
    pub hash_tree: Vec<u8>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct SupportedBlockType {
    pub block_type: String,
    pub url: String,
}

// ─── Helpers: encode blocks as ICRC3Value ───

fn account_to_value(principal: Principal) -> Icrc3Value {
    Icrc3Value::Array(vec![
        Icrc3Value::Blob(principal.as_slice().to_vec()),
    ])
}

/// Encode a block as an ICRC-3 Value with optional parent hash.
/// This is the canonical encoding used for both:
///   - `icrc3_get_blocks` responses (with phash included)
///   - block hashing (representation-independent hash of this value)
pub fn encode_block_with_phash(block: &Icrc3Block, phash: Option<&[u8; 32]>) -> Icrc3Value {
    let (btype, tx_map) = match &block.tx {
        Icrc3Transaction::Mint { to, amount } => (
            "1mint",
            vec![
                ("op".to_string(), Icrc3Value::Text("mint".to_string())),
                ("to".to_string(), account_to_value(*to)),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ],
        ),
        Icrc3Transaction::Burn { from, amount } => (
            "1burn",
            vec![
                ("op".to_string(), Icrc3Value::Text("burn".to_string())),
                ("from".to_string(), account_to_value(*from)),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ],
        ),
        Icrc3Transaction::Transfer { from, to, amount, spender } => {
            let mut fields = vec![
                ("op".to_string(), Icrc3Value::Text("xfer".to_string())),
                ("from".to_string(), account_to_value(*from)),
                ("to".to_string(), account_to_value(*to)),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ];
            if let Some(s) = spender {
                fields.push(("spender".to_string(), account_to_value(*s)));
            }
            ("1xfer", fields)
        }
        Icrc3Transaction::Approve { from, spender, amount, expires_at } => {
            let mut fields = vec![
                ("op".to_string(), Icrc3Value::Text("approve".to_string())),
                ("from".to_string(), account_to_value(*from)),
                ("spender".to_string(), account_to_value(*spender)),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ];
            // index-ng expects "expected_allowance" and "expires_at" (full names, not abbreviated)
            if let Some(exp) = expires_at {
                fields.push(("expires_at".to_string(), Icrc3Value::Nat(Nat::from(*exp))));
            }
            ("2approve", fields)
        }
    };

    let mut block_map = Vec::new();
    // phash must be present for all blocks except block 0
    if let Some(h) = phash {
        block_map.push(("phash".to_string(), Icrc3Value::Blob(h.to_vec())));
    }
    // Note: btype (ICRC-3) is intentionally omitted — the index-ng rejects unknown
    // fields and determines tx type from the "op" field inside tx instead.
    let _ = btype;
    block_map.push(("ts".to_string(), Icrc3Value::Nat(Nat::from(block.timestamp))));
    // fee at block level (required by index-ng to track ledger fee)
    block_map.push(("fee".to_string(), Icrc3Value::Nat(Nat::from(0u64))));
    block_map.push(("tx".to_string(), Icrc3Value::Map(tx_map)));

    Icrc3Value::Map(block_map)
}


// ─── Query implementations ───

pub fn icrc3_get_blocks(args: Vec<GetBlocksArgs>) -> GetBlocksResult {
    read_state(|s| {
        let all_blocks = s.blocks();
        let log_length = all_blocks.len() as u64;

        let mut result_blocks = Vec::new();
        for arg in &args {
            let start = nat_to_u64(&arg.start) as usize;
            let length = nat_to_u64(&arg.length) as usize;

            if start >= all_blocks.len() {
                continue;
            }

            let end = std::cmp::min(start + length, all_blocks.len());

            // Rebuild hash chain for the requested range so we can include phash.
            // We need to compute hashes from block 0 up to `start` to get the
            // parent hash for the first requested block.
            let mut prev_hash: Option<[u8; 32]> = None;
            for block in &all_blocks[..end] {
                let encoded = encode_block_with_phash(block, prev_hash.as_ref());
                let block_hash = crate::certification::hash_value(&encoded);
                if block.id as usize >= start {
                    result_blocks.push(BlockWithId {
                        id: Nat::from(block.id),
                        block: encoded,
                    });
                }
                prev_hash = Some(block_hash);
            }
        }

        GetBlocksResult {
            log_length: Nat::from(log_length),
            blocks: result_blocks,
            archived_blocks: vec![], // No archiving
        }
    })
}

pub fn icrc3_get_archives(_args: GetArchivesArgs) -> GetArchivesResult {
    GetArchivesResult { archives: vec![] }
}

pub fn icrc3_get_tip_certificate() -> Option<Icrc3DataCertificate> {
    read_state(|s| {
        let last_hash = s.last_block_hash.as_ref()?;
        let last_index = s.blocks().len().checked_sub(1)? as u64;
        crate::certification::get_tip_certificate(last_index, last_hash)
    })
}

pub fn icrc3_supported_block_types() -> Vec<SupportedBlockType> {
    let base_url = "https://github.com/dfinity/ICRC-1/tree/main/standards/ICRC-3";
    vec![
        SupportedBlockType {
            block_type: "1xfer".to_string(),
            url: base_url.to_string(),
        },
        SupportedBlockType {
            block_type: "2approve".to_string(),
            url: base_url.to_string(),
        },
        SupportedBlockType {
            block_type: "1mint".to_string(),
            url: base_url.to_string(),
        },
        SupportedBlockType {
            block_type: "1burn".to_string(),
            url: base_url.to_string(),
        },
    ]
}

// ─── Helpers ───

fn nat_to_u64(n: &Nat) -> u64 {
    use num_traits::cast::ToPrimitive;
    n.0.to_u64().unwrap_or(0)
}
