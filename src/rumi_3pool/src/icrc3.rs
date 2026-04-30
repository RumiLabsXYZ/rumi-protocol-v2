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
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
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

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
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

/// Encode a (principal, optional subaccount) pair as the ICRC-3 standard
/// `Account` value: `[owner_blob]` if no subaccount, `[owner_blob, sub_blob]`
/// if a subaccount is present. Blocks written before the subaccount fields
/// were added to `Icrc3Transaction` always pass `None` here, which preserves
/// their original `[owner_blob]` encoding (and therefore their hash chain).
fn account_to_value(principal: Principal, subaccount: Option<&[u8]>) -> Icrc3Value {
    let mut parts = vec![Icrc3Value::Blob(principal.as_slice().to_vec())];
    if let Some(sub) = subaccount {
        parts.push(Icrc3Value::Blob(sub.to_vec()));
    }
    Icrc3Value::Array(parts)
}

/// Encode a block as an ICRC-3 Value with optional parent hash.
/// This is the canonical encoding used for both:
///   - `icrc3_get_blocks` responses (with phash included)
///   - block hashing (representation-independent hash of this value)
pub fn encode_block_with_phash(block: &Icrc3Block, phash: Option<&[u8; 32]>) -> Icrc3Value {
    let (btype, tx_map) = match &block.tx {
        Icrc3Transaction::Mint { to, amount, to_subaccount } => (
            "1mint",
            vec![
                ("op".to_string(), Icrc3Value::Text("mint".to_string())),
                ("to".to_string(), account_to_value(*to, to_subaccount.as_deref())),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ],
        ),
        Icrc3Transaction::Burn { from, amount, from_subaccount } => (
            "1burn",
            vec![
                ("op".to_string(), Icrc3Value::Text("burn".to_string())),
                ("from".to_string(), account_to_value(*from, from_subaccount.as_deref())),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ],
        ),
        Icrc3Transaction::Transfer {
            from, to, amount, spender,
            from_subaccount, to_subaccount, spender_subaccount,
        } => {
            let mut fields = vec![
                ("op".to_string(), Icrc3Value::Text("xfer".to_string())),
                ("from".to_string(), account_to_value(*from, from_subaccount.as_deref())),
                ("to".to_string(), account_to_value(*to, to_subaccount.as_deref())),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(*amount))),
            ];
            if let Some(s) = spender {
                fields.push((
                    "spender".to_string(),
                    account_to_value(*s, spender_subaccount.as_deref()),
                ));
            }
            ("1xfer", fields)
        }
        Icrc3Transaction::Approve {
            from, spender, amount, expires_at,
            from_subaccount, spender_subaccount,
        } => {
            // Cap approve amounts to u64::MAX for index-ng compatibility.
            // The standard index-ng deserializes amounts as u64 and rejects
            // blocks with larger values. Approvals often use u128::MAX.
            let capped = std::cmp::min(*amount, u64::MAX as u128) as u64;
            let mut fields = vec![
                ("op".to_string(), Icrc3Value::Text("approve".to_string())),
                ("from".to_string(), account_to_value(*from, from_subaccount.as_deref())),
                ("spender".to_string(), account_to_value(*spender, spender_subaccount.as_deref())),
                ("amt".to_string(), Icrc3Value::Nat(Nat::from(capped))),
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
    let log_length = crate::storage::blocks::len();

    let mut result_blocks = Vec::new();
    for arg in &args {
        let start = nat_to_u64(&arg.start);
        let length = nat_to_u64(&arg.length);

        if start >= log_length {
            continue;
        }
        let end = std::cmp::min(start.saturating_add(length), log_length);
        if end <= start {
            continue;
        }

        // Parent hash for the first requested block: cached at index
        // (start - 1), or None if start == 0. Tasks 4-5 guarantee the
        // cache covers all blocks via post_upgrade backfill.
        let mut prev_hash: Option<[u8; 32]> = if start == 0 {
            None
        } else {
            Some(
                crate::storage::block_hashes::get(start - 1)
                    .expect(
                        "hash cache must cover all blocks: the post_upgrade \
                         backfill + integrity check enforces this invariant"
                    )
                    .0,
            )
        };

        // Read only the requested range from the blocks log. Encoding +
        // hashing happens once per returned block, replacing the old
        // O(end) chain rebuild.
        let blocks = crate::storage::blocks::range(start, end - start);
        for block in &blocks {
            let encoded = encode_block_with_phash(block, prev_hash.as_ref());
            // Compute the running hash so the next iteration has its parent.
            // For the LAST block in this range, prev_hash is not read again,
            // but computing it keeps the loop body symmetric and the cost is
            // a single SHA-256 over the encoded value.
            let block_hash = crate::certification::hash_value(&encoded);
            result_blocks.push(BlockWithId {
                id: Nat::from(block.id),
                block: encoded,
            });
            prev_hash = Some(block_hash);
        }
    }

    GetBlocksResult {
        log_length: Nat::from(log_length),
        blocks: result_blocks,
        archived_blocks: vec![],
    }
}

pub fn icrc3_get_archives(_args: GetArchivesArgs) -> GetArchivesResult {
    GetArchivesResult { archives: vec![] }
}

pub fn icrc3_get_tip_certificate() -> Option<Icrc3DataCertificate> {
    let last_hash = read_state(|s| s.last_block_hash)?;
    let last_index = crate::storage::blocks::len().checked_sub(1)?;
    crate::certification::get_tip_certificate(last_index, &last_hash)
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
