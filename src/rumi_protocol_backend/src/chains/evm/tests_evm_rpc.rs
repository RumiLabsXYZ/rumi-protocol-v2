use super::evm_rpc::{parse_hex_quantity, BurnLog};

// ─── Candid round-trip test: IcError with real RejectionCode ─────────────────
//
// This test proves that the production `RequestResult` type can decode a
// REAL-WIRE-SHAPED IcError where `code` is a Candid variant (`RejectionCode`),
// not a u32.
//
// "Real shape" types are defined independently here to mirror the live .did
// exactly.  We encode with them, then decode with the production type.
// With `code: u32` the Decode! call returns Err (decode trap averted by
// candid::Decode! returning Result).  After changing `code` to `RejectionCode`
// it must pass.

#[cfg(test)]
mod ic_error_round_trip {
    use candid::{CandidType, Deserialize};
    use candid::{Encode, Decode};

    // ── "Real" (wire-shape) types — independent mirror of the live .did ──────

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealRejectionCode {
        NoError,
        CanisterError,
        SysTransient,
        DestinationInvalid,
        Unknown,
        SysFatal,
        CanisterReject,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    struct RealIcError {
        pub code: RealRejectionCode,
        pub message: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    struct RealInvalidHttpJsonRpcRecord {
        pub status: u16,
        pub body: String,
        #[serde(rename = "parsingError")]
        pub parsing_error: Option<String>,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealHttpOutcallError {
        IcError(RealIcError),
        InvalidHttpJsonRpcResponse(RealInvalidHttpJsonRpcRecord),
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    struct RealJsonRpcError {
        pub code: i64,
        pub message: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    struct RealTooFewCyclesRecord {
        pub expected: candid::Nat,
        pub received: candid::Nat,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealProviderError {
        TooFewCycles(RealTooFewCyclesRecord),
        MissingRequiredProvider,
        ProviderNotFound,
        NoPermission,
        InvalidRpcConfig(String),
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealValidationError {
        Custom(String),
        InvalidHex(String),
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealRpcError {
        JsonRpcError(RealJsonRpcError),
        ProviderError(RealProviderError),
        ValidationError(RealValidationError),
        HttpOutcallError(RealHttpOutcallError),
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealRequestResult {
        Ok(String),
        Err(RealRpcError),
    }

    // ── Production types (import from the module under test) ─────────────────
    use super::super::evm_rpc::{
        HttpOutcallError, IcErrorRecord, RejectionCode, RpcError, RequestResult,
    };

    /// Encode a real-wire-shaped `IcError { code: SysTransient }` and decode
    /// it as the production `RequestResult`.  With `code: u32` this decode
    /// fails; with `code: RejectionCode` it must pass and round-trip correctly.
    #[test]
    fn ic_error_rejection_code_round_trips() {
        let wire = RealRequestResult::Err(RealRpcError::HttpOutcallError(
            RealHttpOutcallError::IcError(RealIcError {
                code: RealRejectionCode::SysTransient,
                message: "no consensus".to_string(),
            }),
        ));

        let bytes = Encode!(&wire).expect("encode real RequestResult");

        let decoded = Decode!(&bytes, RequestResult)
            .expect("decode production RequestResult from real-wire bytes");

        let expected = RequestResult::Err(RpcError::HttpOutcallError(
            HttpOutcallError::IcError(IcErrorRecord {
                code: RejectionCode::SysTransient,
                message: "no consensus".to_string(),
            }),
        ));

        assert_eq!(decoded, expected);
    }
}

// ─── Candid round-trip test: typed eth_getBlockByNumber result ───────────────
//
// Proves (a) the production typed-method mirror types match the live .did and
// (b) the minimal production `Block { number }` decodes a RICHER wire `Block`
// (record subtyping: a reader with FEWER fields decodes a record with more,
// ignoring the extras).  We define independent "real shape" types here with
// MANY more fields than just `number` (hash, timestamp, miner, ...), encode a
// `Consistent(Ok(RealBlock{...}))`, then `Decode!` into the PRODUCTION
// `MultiGetBlockByNumberResult` and assert the decoded `number` survives.
//
// This must fail to compile/decode before the production types exist and pass
// after.

#[cfg(test)]
mod block_by_number_round_trip {
    use candid::{CandidType, Deserialize};
    use candid::{Encode, Decode};

    // ── "Real" (wire-shape) types — independent mirror of the live .did ──────
    //
    // Reuse the RpcError tree shape from the Layer-1 test family; here we only
    // need the Ok arm, so a minimal RealRpcError stand-in suffices for the
    // variant's type to line up.

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    struct RealJsonRpcError {
        pub code: i64,
        pub message: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
    enum RealRpcError {
        JsonRpcError(RealJsonRpcError),
    }

    /// A FULL `Block` record (many more fields than the production `Block`,
    /// which carries only `number`).  Field order follows the live .did so the
    /// candid hashes match; only `number` is asserted post-decode.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    struct RealBlock {
        pub miner: String,
        #[serde(rename = "totalDifficulty")]
        pub total_difficulty: Option<candid::Nat>,
        #[serde(rename = "receiptsRoot")]
        pub receipts_root: String,
        #[serde(rename = "stateRoot")]
        pub state_root: String,
        pub hash: String,
        pub difficulty: Option<candid::Nat>,
        pub size: candid::Nat,
        pub uncles: Vec<String>,
        #[serde(rename = "baseFeePerGas")]
        pub base_fee_per_gas: Option<candid::Nat>,
        #[serde(rename = "extraData")]
        pub extra_data: String,
        #[serde(rename = "transactionsRoot")]
        pub transactions_root: Option<String>,
        #[serde(rename = "sha3Uncles")]
        pub sha3_uncles: String,
        pub nonce: candid::Nat,
        pub number: candid::Nat,
        pub timestamp: candid::Nat,
        pub transactions: Vec<String>,
        #[serde(rename = "gasLimit")]
        pub gas_limit: candid::Nat,
        #[serde(rename = "logsBloom")]
        pub logs_bloom: String,
        #[serde(rename = "parentHash")]
        pub parent_hash: String,
        #[serde(rename = "gasUsed")]
        pub gas_used: candid::Nat,
        #[serde(rename = "mixHash")]
        pub mix_hash: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    enum RealGetBlockByNumberResult {
        Ok(RealBlock),
        Err(RealRpcError),
    }

    // The `Inconsistent` arm references the singular RpcService; since this
    // test only ever encodes `Consistent`, a minimal stand-in keeps the
    // variant's candid type well-formed without re-mirroring all of RpcService.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    struct RealRpcApi {
        pub url: String,
        pub headers: Option<Vec<RealHttpHeader>>,
    }
    #[derive(CandidType, Deserialize, Clone, Debug)]
    struct RealHttpHeader {
        pub name: String,
        pub value: String,
    }
    #[derive(CandidType, Deserialize, Clone, Debug)]
    enum RealRpcService {
        Custom(RealRpcApi),
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    enum RealMultiGetBlockByNumberResult {
        Consistent(RealGetBlockByNumberResult),
        Inconsistent(Vec<(RealRpcService, RealGetBlockByNumberResult)>),
    }

    // ── Production types (import from the module under test) ─────────────────
    use super::super::evm_rpc::{
        Block, GetBlockByNumberResult, MultiGetBlockByNumberResult,
    };

    /// Encode a real-wire-shaped `Consistent(Ok(RealBlock{ number: 12345, ...}))`
    /// (richer than production `Block`) and decode it into the production
    /// `MultiGetBlockByNumberResult`.  Asserts the decoded `number` matches —
    /// proving the mirror is correct AND record-subtyping drops the extra
    /// fields cleanly.
    #[test]
    fn typed_block_result_round_trips_with_subtyping() {
        let wire = RealMultiGetBlockByNumberResult::Consistent(
            RealGetBlockByNumberResult::Ok(RealBlock {
                miner: "0xabc".to_string(),
                total_difficulty: Some(candid::Nat::from(0u64)),
                receipts_root: "0xrr".to_string(),
                state_root: "0xsr".to_string(),
                hash: "0xdeadbeef".to_string(),
                difficulty: None,
                size: candid::Nat::from(1u64),
                uncles: vec![],
                base_fee_per_gas: Some(candid::Nat::from(7u64)),
                extra_data: "0x".to_string(),
                transactions_root: Some("0xtr".to_string()),
                sha3_uncles: "0xsu".to_string(),
                nonce: candid::Nat::from(0u64),
                number: candid::Nat::from(12_345u64),
                timestamp: candid::Nat::from(1_700_000_000u64),
                transactions: vec!["0xtx1".to_string()],
                gas_limit: candid::Nat::from(30_000_000u64),
                logs_bloom: "0x00".to_string(),
                parent_hash: "0xparent".to_string(),
                gas_used: candid::Nat::from(21_000u64),
                mix_hash: "0xmix".to_string(),
            }),
        );

        let bytes = Encode!(&wire).expect("encode real MultiGetBlockByNumberResult");

        let decoded = Decode!(&bytes, MultiGetBlockByNumberResult)
            .expect("decode production MultiGetBlockByNumberResult from richer wire bytes");

        match decoded {
            MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Ok(Block {
                number,
            })) => {
                assert_eq!(number, candid::Nat::from(12_345u64));
            }
            other => panic!("expected Consistent(Ok(Block)), got {:?}", other),
        }
    }
}

#[test]
fn parses_hex_quantity() {
    assert_eq!(parse_hex_quantity("0x0").unwrap(), 0u128);
    assert_eq!(parse_hex_quantity("0x10").unwrap(), 16u128);
    assert_eq!(parse_hex_quantity("0x2540be400").unwrap(), 10_000_000_000u128); // 100 icUSD @ 8dp
    assert!(parse_hex_quantity("not-hex").is_err());
}

#[test]
fn decodes_burn_log() {
    // Burn(uint256 vault_id, address burner, uint256 amount):
    //   topics[0] = keccak("Burn(uint256,address,uint256)")
    //   topics[1] = vault_id (indexed), topics[2] = burner (indexed), data = amount
    let topic0 = super::evm_rpc::BURN_EVENT_TOPIC0.to_string();
    let vault_id_topic = format!("0x{:064x}", 7u64);
    let burner_topic = format!("0x{:064x}", 0u8);
    let amount_data = format!("0x{:064x}", 10_000_000_000u128);
    let log = BurnLog::from_raw(&[topic0, vault_id_topic, burner_topic], &amount_data, "0xtxhash", 110)
        .expect("decode burn");
    assert_eq!(log.vault_id, 7);
    assert_eq!(log.amount_e8s, 10_000_000_000);
    assert_eq!(log.block_number, 110);
}

#[test]
fn burn_log_with_burner_decodes_indexed_burner_address() {
    let topics = vec![
        super::evm_rpc::BURN_EVENT_TOPIC0.to_string(),
        format!("0x{:064x}", 7u64),
        "0x0000000000000000000000001234567890abcdef1234567890abcdef12345678"
            .to_string(),
    ];
    let burn = super::evm_rpc::decode_burn_log_with_burner(
        &topics,
        &format!("0x{:064x}", 40_000_000u128),
        "0xabc",
        99,
    )
    .expect("decode burn with burner");
    assert_eq!(burn.vault_id, 7);
    assert_eq!(burn.amount_e8s, 40_000_000);
    assert_eq!(burn.burner, "0x1234567890abcdef1234567890abcdef12345678");
}

#[test]
fn rejects_log_with_wrong_topic0() {
    let res = BurnLog::from_raw(
        &["0xdeadbeef".into(), format!("0x{:064x}", 1u64), format!("0x{:064x}", 0u8)],
        &format!("0x{:064x}", 1u128), "0xtx", 1);
    assert!(res.is_err());
}

#[test]
fn receipt_with_logs_parses_status_block_and_logs() {
    // A minimal eth_getTransactionReceipt JSON with one log.
    let json = r#"{"jsonrpc":"2.0","id":1,"result":{
        "status":"0x1","blockNumber":"0x10",
        "logs":[{"address":"0xCAFE","topics":["0xTOPIC0","0xVAULT","0xBURNER"],
                 "data":"0x2a","logIndex":"0x3"}]}}"#;
    let parsed = super::evm_rpc::parse_receipt_with_logs(json).expect("parse");
    let r = parsed.expect("receipt present");
    assert!(r.success);
    assert_eq!(r.block_number, 16);
    assert_eq!(r.logs.len(), 1);
    let (addr, topics, data, idx) = &r.logs[0];
    assert_eq!(addr, "0xcafe"); // lowercased
    assert_eq!(topics[0], "0xTOPIC0");
    assert_eq!(data, "0x2a");
    assert_eq!(*idx, 3);
}

#[test]
fn parse_eth_call_u128_decodes_padded_word() {
    use crate::chains::monad::evm_rpc::parse_eth_call_u128;
    // A 32-byte ABI word for 1_000_000, left-padded to 64 hex chars.
    let word = format!("0x{:064x}", 1_000_000u128);
    assert_eq!(parse_eth_call_u128(&word).unwrap(), 1_000_000u128);
    // Zero supply.
    assert_eq!(parse_eth_call_u128(&format!("0x{:064x}", 0u128)).unwrap(), 0u128);
    // Empty result ("0x") -> error, NOT 0.
    assert!(parse_eth_call_u128("0x").is_err());
    // Non-hex -> error.
    assert!(parse_eth_call_u128("0xzz").is_err());
    // A value exceeding u128 (full 32-byte max) -> error rather than silent wrap.
    let too_big = format!("0x{}", "f".repeat(64));
    assert!(parse_eth_call_u128(&too_big).is_err());
}

// ─── Increment 3 / Task 4: DEX getReserves parse + Transfer-log decode ───
#[test]
fn parse_two_uint112_splits_getreserves() {
    use super::evm_rpc::parse_two_uint112;
    // ABI return (uint112, uint112, uint32) = THREE full 32-byte words (each value
    // left-padded), 192 hex chars. parse reads word0 + word1.
    let hex = format!("0x{:064x}{:064x}{:064x}", 1_000_000u128, 90_900u128, 12_345u128);
    assert_eq!(parse_two_uint112(&hex).unwrap(), (1_000_000, 90_900));
    // Too-short result fails closed.
    assert!(parse_two_uint112("0x1234").is_err());
}

#[test]
fn transfer_log_decodes_to_and_amount() {
    use super::evm_rpc::{TransferLog, TRANSFER_EVENT_TOPIC0};
    let to = "0x000000000000000000000000000000000000c0de";
    let topics = vec![
        TRANSFER_EVENT_TOPIC0.to_string(),
        format!("0x{:064x}", 1u128), // from (indexed, padded)
        format!("0x000000000000000000000000{}", to.trim_start_matches("0x")), // to (indexed, padded)
    ];
    let data = format!("0x{:064x}", 5_000_000u128);
    let t = TransferLog::from_raw(&topics, &data).unwrap();
    assert_eq!(t.to.to_lowercase(), to.to_lowercase());
    assert_eq!(t.amount, 5_000_000);
    // Wrong topic0 -> Err.
    let mut bad = topics.clone();
    bad[0] = format!("0x{}", "0".repeat(64));
    assert!(TransferLog::from_raw(&bad, &data).is_err());
}
