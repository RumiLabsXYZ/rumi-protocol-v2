// ICRC-3 transaction log query endpoints for the 3USD LP token.
//
// Enables the DFINITY ic-icrc1-index-ng canister to index all LP token
// transactions (mint, burn, transfer, approve) by polling icrc3_get_blocks.

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc3::archive::QueryArchiveFn;
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

/// Candid `func` reference — archives are not used, but the callback still
/// needs the standard function-reference type for interface compatibility.
pub type ArchivedBlocksCallback = QueryArchiveFn<Vec<GetBlocksArgs>, GetBlocksResult>;

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
            memo, created_at_time,
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
            // ICRC-3 puts the transfer's caller-supplied idempotency metadata
            // inside `tx`. Omit absent values so blocks created before this
            // field was introduced re-encode byte-for-byte identically.
            if let Some(memo) = memo {
                fields.push(("memo".to_string(), Icrc3Value::Blob(memo.clone())));
            }
            if let Some(created_at_time) = created_at_time {
                fields.push((
                    "ts".to_string(),
                    Icrc3Value::Nat(Nat::from(*created_at_time)),
                ));
            }
            ("1xfer", fields)
        }
        Icrc3Transaction::Approve {
            from, spender, amount, expires_at,
            from_subaccount, spender_subaccount,
            memo, created_at_time,
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
            if let Some(memo) = memo {
                fields.push(("memo".to_string(), Icrc3Value::Blob(memo.clone())));
            }
            if let Some(created_at_time) = created_at_time {
                fields.push((
                    "ts".to_string(),
                    Icrc3Value::Nat(Nat::from(*created_at_time)),
                ));
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

#[cfg(test)]
mod transaction_metadata_tests {
    use super::*;

    #[derive(CandidType, Deserialize)]
    enum LegacyTransaction {
        Transfer {
            from: Principal,
            to: Principal,
            amount: u128,
            spender: Option<Principal>,
            from_subaccount: Option<Vec<u8>>,
            to_subaccount: Option<Vec<u8>>,
            spender_subaccount: Option<Vec<u8>>,
        },
    }

    #[derive(CandidType, Deserialize)]
    struct LegacyBlock {
        id: u64,
        timestamp: u64,
        tx: LegacyTransaction,
    }

    #[test]
    fn legacy_candid_transaction_decodes_and_keeps_legacy_encoding() {
        let legacy = LegacyBlock {
            id: 7,
            timestamp: 99,
            tx: LegacyTransaction::Transfer {
                from: Principal::anonymous(),
                to: Principal::management_canister(),
                amount: 11,
                spender: None,
                from_subaccount: None,
                to_subaccount: None,
                spender_subaccount: None,
            },
        };
        let bytes = candid::encode_one(&legacy).unwrap();
        let decoded: Icrc3Block = candid::decode_one(&bytes).unwrap();
        match &decoded.tx {
            Icrc3Transaction::Transfer { memo, created_at_time, .. } => {
                assert!(memo.is_none());
                assert!(created_at_time.is_none());
            }
            other => panic!("expected legacy transfer, got {other:?}"),
        }

        let legacy_projection = match legacy.tx {
            LegacyTransaction::Transfer {
                from,
                to,
                amount,
                spender,
                from_subaccount,
                to_subaccount,
                spender_subaccount,
            } => Icrc3Transaction::Transfer {
                from,
                to,
                amount,
                spender,
                from_subaccount,
                to_subaccount,
                spender_subaccount,
                memo: None,
                created_at_time: None,
            },
        };
        let expected = Icrc3Block { id: 7, timestamp: 99, tx: legacy_projection };
        assert_eq!(
            encode_block_with_phash(&decoded, None),
            encode_block_with_phash(&expected, None),
            "missing metadata must leave the historical block value unchanged",
        );
    }
}


// ─── Query implementations ───

/// Maximum number of blocks returned across all `GetBlocksArgs` ranges in a
/// single `icrc3_get_blocks` call. Without this, a caller could pass many
/// ranges (or repeat `{0, log_length}`) and force an unbounded reply plus a
/// per-block SHA-256 over the whole log. The ic-icrc1-index-ng paginates well
/// below this, so it is invisible to honest callers. Audit 2026-06-05 (SAT-006).
pub const MAX_GET_BLOCKS_RESPONSE: u64 = 2_000;

pub fn icrc3_get_blocks(args: Vec<GetBlocksArgs>) -> GetBlocksResult {
    let log_length = crate::storage::blocks::len();

    let mut result_blocks = Vec::new();
    let mut remaining = MAX_GET_BLOCKS_RESPONSE;
    for arg in &args {
        if remaining == 0 {
            break;
        }
        let start = nat_to_u64(&arg.start);
        // Cap this range's length to the global response budget so the total
        // number of blocks (and SHA-256 hashes) stays bounded across all args.
        let length = nat_to_u64(&arg.length).min(remaining);

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
        remaining = remaining.saturating_sub(blocks.len() as u64);
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

#[cfg(test)]
mod transfer_metadata_tests {
    use super::{account_to_value, encode_block_with_phash, Icrc3Value};
    use crate::types::{Icrc3Block, Icrc3Transaction};
    use candid::{Nat, Principal};

    fn account(owner: u8) -> Principal { Principal::from_slice(&[owner]) }

    fn tx_fields(value: Icrc3Value) -> Vec<(String, Icrc3Value)> {
        let Icrc3Value::Map(block) = value else { panic!("block must be a map") };
        let Some((_, Icrc3Value::Map(tx))) = block.into_iter().find(|(key, _)| key == "tx") else {
            panic!("block must contain tx map")
        };
        tx
    }

    #[test]
    fn transfer_from_encoder_emits_exact_memo_and_transaction_time() {
        let block = Icrc3Block {
            id: 9,
            timestamp: 900,
            tx: Icrc3Transaction::Transfer {
                from: account(1), to: account(2), amount: 33,
                spender: Some(account(3)), from_subaccount: None,
                to_subaccount: None, spender_subaccount: None,
                memo: Some(vec![4; 16]), created_at_time: Some(123_456),
            },
        };
        let fields = tx_fields(encode_block_with_phash(&block, None));
        assert!(fields.contains(&("memo".into(), Icrc3Value::Blob(vec![4; 16]))));
        assert!(fields.contains(&("ts".into(), Icrc3Value::Nat(Nat::from(123_456u64)))));
        assert!(fields.contains(&("spender".into(), account_to_value(account(3), None))));
    }

    #[test]
    fn direct_owner_transfer_metadata_is_emitted_but_none_stays_legacy_shaped() {
        let metadata_transfer = Icrc3Block {
            id: 1,
            timestamp: 901,
            tx: Icrc3Transaction::Transfer {
                from: account(1), to: account(2), amount: 7,
                spender: None, from_subaccount: None, to_subaccount: None,
                spender_subaccount: None, memo: Some(vec![8]), created_at_time: Some(45),
            },
        };
        let tx = tx_fields(encode_block_with_phash(&metadata_transfer, None));
        assert!(tx.iter().any(|(key, _)| key == "memo"));
        assert!(tx.iter().any(|(key, _)| key == "ts"));

        let legacy_shaped = Icrc3Block {
            id: 0, timestamp: 902,
            tx: Icrc3Transaction::Transfer {
                from: account(1), to: account(2), amount: 7,
                spender: None, from_subaccount: None, to_subaccount: None,
                spender_subaccount: None, memo: None, created_at_time: None,
            },
        };
        let tx = tx_fields(encode_block_with_phash(&legacy_shaped, None));
        assert!(!tx.iter().any(|(key, _)| key == "memo" || key == "ts"));
    }

    #[test]
    fn old_transfer_snapshot_decodes_and_keeps_the_pre_metadata_block_hash() {
        #[derive(serde::Serialize)]
        struct LegacyBlock { id: u64, timestamp: u64, tx: LegacyTransaction }
        #[derive(serde::Serialize)]
        enum LegacyTransaction {
            Transfer {
                from: Principal, to: Principal, amount: u128, spender: Option<Principal>,
                from_subaccount: Option<Vec<u8>>, to_subaccount: Option<Vec<u8>>,
                spender_subaccount: Option<Vec<u8>>,
            },
        }
        let legacy = LegacyBlock {
            id: 0, timestamp: 777,
            tx: LegacyTransaction::Transfer {
                from: account(1), to: account(2), amount: 66, spender: Some(account(3)),
                from_subaccount: None, to_subaccount: None, spender_subaccount: None,
            },
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
        let restored: Icrc3Block = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        let Icrc3Transaction::Transfer { memo, created_at_time, .. } = &restored.tx else {
            panic!("legacy transfer variant must decode")
        };
        assert_eq!(memo, &None);
        assert_eq!(created_at_time, &None);

        let legacy_encoded = Icrc3Value::Map(vec![
            ("ts".into(), Icrc3Value::Nat(Nat::from(777u64))),
            ("fee".into(), Icrc3Value::Nat(Nat::from(0u64))),
            ("tx".into(), Icrc3Value::Map(vec![
                ("op".into(), Icrc3Value::Text("xfer".into())),
                ("from".into(), account_to_value(account(1), None)),
                ("to".into(), account_to_value(account(2), None)),
                ("amt".into(), Icrc3Value::Nat(Nat::from(66u64))),
                ("spender".into(), account_to_value(account(3), None)),
            ])),
        ]);
        let encoded_after_upgrade = encode_block_with_phash(&restored, None);
        assert_eq!(crate::certification::hash_value(&encoded_after_upgrade), crate::certification::hash_value(&legacy_encoded));
    }
}
