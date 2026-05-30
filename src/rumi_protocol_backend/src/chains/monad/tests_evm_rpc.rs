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
fn rejects_log_with_wrong_topic0() {
    let res = BurnLog::from_raw(
        &["0xdeadbeef".into(), format!("0x{:064x}", 1u64), format!("0x{:064x}", 0u8)],
        &format!("0x{:064x}", 1u128), "0xtx", 1);
    assert!(res.is_err());
}
