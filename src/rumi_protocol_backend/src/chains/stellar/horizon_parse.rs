//! Strict parsers for the small subset of Stellar Horizon JSON used by XLM
//! collateral reconciliation. These functions do not perform network I/O.

use serde_json::Value;

use super::amount::parse_xlm_to_stroops;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HorizonParseError {
    Json,
    Missing(&'static str),
    Invalid(&'static str),
    Mismatch(&'static str),
    Ambiguous,
    TruncatedPage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSnapshot {
    pub account_id: String,
    pub sequence: u64,
    pub account_last_modified_ledger: u32,
    pub native_balance_stroops: u64,
    pub subentry_count: u32,
    pub num_sponsoring: u32,
    pub num_sponsored: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerReserve {
    pub sequence: u32,
    pub base_reserve_in_stroops: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatestLedgerReserve {
    pub reserve: LedgerReserve,
    pub closed_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizedDeposit {
    pub transaction_hash: String,
    pub ledger: u32,
    /// Opaque Horizon operation identity. Do not interpret the token's bit layout.
    pub operation_id: String,
    pub destination: String,
    pub amount_stroops: u64,
    pub kind: DepositKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DepositKind {
    CreateAccount,
    NativePayment,
}

fn object(v: &Value) -> Result<&serde_json::Map<String, Value>, HorizonParseError> {
    v.as_object().ok_or(HorizonParseError::Json)
}
fn required<'a>(v: &'a Value, key: &'static str) -> Result<&'a Value, HorizonParseError> {
    object(v)?.get(key).ok_or(HorizonParseError::Missing(key))
}
fn string(v: &Value, key: &'static str) -> Result<String, HorizonParseError> {
    required(v, key)?
        .as_str()
        .map(str::to_owned)
        .ok_or(HorizonParseError::Invalid(key))
}
fn uint(v: &Value, key: &'static str) -> Result<u64, HorizonParseError> {
    required(v, key)?
        .as_u64()
        .ok_or(HorizonParseError::Invalid(key))
}
fn uint32(v: &Value, key: &'static str) -> Result<u32, HorizonParseError> {
    u32::try_from(uint(v, key)?).map_err(|_| HorizonParseError::Invalid(key))
}

fn canonical_operation_id(value: &Value, field: &'static str) -> Result<String, HorizonParseError> {
    let id = match value {
        Value::String(id) => id.clone(),
        Value::Number(number) => number
            .as_u64()
            .map(|number| number.to_string())
            .ok_or(HorizonParseError::Invalid(field))?,
        _ => return Err(HorizonParseError::Invalid(field)),
    };
    if id.is_empty()
        || id.len() > 20
        || !id.bytes().all(|byte| byte.is_ascii_digit())
        || id.starts_with('0')
        || id
            .parse::<u64>()
            .ok()
            .filter(|number| *number != 0)
            .is_none()
    {
        return Err(HorizonParseError::Invalid(field));
    }
    Ok(id)
}

/// Parse `GET /accounts/:id`. Requires the one native balance and rejects
/// duplicate native rows or non-native balance records masquerading as XLM.
pub fn parse_account(body: &[u8], expected_id: &str) -> Result<AccountSnapshot, HorizonParseError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| HorizonParseError::Json)?;
    let account_id = string(&v, "account_id")?;
    if account_id != expected_id {
        return Err(HorizonParseError::Mismatch("account_id"));
    }
    let sequence = string(&v, "sequence")?
        .parse()
        .map_err(|_| HorizonParseError::Invalid("sequence"))?;
    if sequence > i64::MAX as u64 {
        return Err(HorizonParseError::Invalid("sequence"));
    }
    let account_last_modified_ledger = uint32(&v, "account_last_modified_ledger")?;
    if account_last_modified_ledger == 0 {
        return Err(HorizonParseError::Invalid("account_last_modified_ledger"));
    }
    let balances = required(&v, "balances")?
        .as_array()
        .ok_or(HorizonParseError::Invalid("balances"))?;
    let mut native = None;
    for b in balances {
        if string(b, "asset_type")? == "native" {
            if native.is_some() {
                return Err(HorizonParseError::Ambiguous);
            }
            native = Some(
                parse_xlm_to_stroops(&string(b, "balance")?)
                    .map_err(|_| HorizonParseError::Invalid("balance"))?,
            );
        }
    }
    Ok(AccountSnapshot {
        account_id,
        sequence,
        account_last_modified_ledger,
        native_balance_stroops: native.ok_or(HorizonParseError::Missing("native balance"))?,
        subentry_count: uint32(&v, "subentry_count")?,
        num_sponsoring: uint32(&v, "num_sponsoring")?,
        num_sponsored: uint32(&v, "num_sponsored")?,
    })
}

/// Parse one `/ledgers/:sequence` object for the live reserve input. Fee budget
/// is deliberately not included: callers supply it separately.
pub fn parse_ledger_reserve(
    body: &[u8],
    expected_sequence: u32,
) -> Result<LedgerReserve, HorizonParseError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| HorizonParseError::Json)?;
    let sequence =
        u32::try_from(uint(&v, "sequence")?).map_err(|_| HorizonParseError::Invalid("sequence"))?;
    if sequence != expected_sequence {
        return Err(HorizonParseError::Mismatch("sequence"));
    }
    let base_reserve_in_stroops = u64::try_from(uint(&v, "base_reserve_in_stroops")?)
        .map_err(|_| HorizonParseError::Invalid("base_reserve_in_stroops"))?;
    if base_reserve_in_stroops == 0 {
        return Err(HorizonParseError::Invalid("base_reserve_in_stroops"));
    }
    Ok(LedgerReserve {
        sequence,
        base_reserve_in_stroops,
    })
}

/// Parse the deterministic reduction of `GET /ledgers?order=desc&limit=1`.
/// `closed_at` is retained for caller-controlled freshness policy.
pub fn parse_latest_ledger_reserve(body: &[u8]) -> Result<LatestLedgerReserve, HorizonParseError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| HorizonParseError::Json)?;
    let sequence = uint32(&v, "sequence")?;
    let base_reserve_in_stroops = uint(&v, "base_reserve_in_stroops")?;
    let closed_at = string(&v, "closed_at")?;
    if sequence == 0 || base_reserve_in_stroops == 0 || !valid_utc_timestamp(&closed_at) {
        return Err(HorizonParseError::Invalid("latest ledger"));
    }
    Ok(LatestLedgerReserve {
        reserve: LedgerReserve {
            sequence,
            base_reserve_in_stroops,
        },
        closed_at,
    })
}

fn valid_utc_timestamp(value: &str) -> bool {
    let Some(value) = value.strip_suffix('Z') else {
        return false;
    };
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if (value.contains('.') && fraction.is_empty())
        || (!fraction.is_empty() && !fraction.bytes().all(|byte| byte.is_ascii_digit()))
        || whole.len() != 19
        || whole.as_bytes().get(4) != Some(&b'-')
        || whole.as_bytes().get(7) != Some(&b'-')
        || whole.as_bytes().get(10) != Some(&b'T')
        || whole.as_bytes().get(13) != Some(&b':')
        || whole.as_bytes().get(16) != Some(&b':')
    {
        return false;
    }
    let number = |start, end| whole[start..end].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    day > 0 && day <= days && hour < 24 && minute < 60 && second <= 60
}

/// Validate a successful transaction response and return its finalized ledger.
pub fn parse_transaction(body: &[u8], expected_hash: &str) -> Result<u32, HorizonParseError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| HorizonParseError::Json)?;
    if string(&v, "hash")? != expected_hash {
        return Err(HorizonParseError::Mismatch("hash"));
    }
    if required(&v, "successful")?.as_bool() != Some(true) {
        return Err(HorizonParseError::Mismatch("successful"));
    }
    let ledger =
        u32::try_from(uint(&v, "ledger")?).map_err(|_| HorizonParseError::Invalid("ledger"))?;
    if ledger == 0 {
        return Err(HorizonParseError::Invalid("ledger"));
    }
    Ok(ledger)
}

/// Parse transaction operations collection, requiring exactly one matching
/// successful XLM deposit. The caller supplies the requested page limit; a
/// full page is rejected because it may have been truncated.
pub fn parse_deposit_operations(
    body: &[u8],
    expected_hash: &str,
    expected_ledger: u32,
    expected_destination: &str,
    expected_amount_stroops: u64,
    requested_page_limit: usize,
) -> Result<FinalizedDeposit, HorizonParseError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| HorizonParseError::Json)?;
    let records = required(required(&v, "_embedded")?, "records")?
        .as_array()
        .ok_or(HorizonParseError::Invalid("records"))?;
    if requested_page_limit == 0 {
        return Err(HorizonParseError::Invalid("requested_page_limit"));
    }
    if records.len() >= requested_page_limit {
        return Err(HorizonParseError::TruncatedPage);
    }
    let mut found = None;
    let mut operation_ids = Vec::new();
    for op in records {
        if string(op, "transaction_hash")? != expected_hash {
            return Err(HorizonParseError::Mismatch("transaction_hash"));
        }
        let fields = object(op)?;
        let paging_token = fields
            .get("paging_token")
            .map(|value| canonical_operation_id(value, "paging_token"));
        let operation_id = fields
            .get("id")
            .map(|value| canonical_operation_id(value, "id"));
        let id = match (paging_token, operation_id) {
            (Some(Ok(paging_token)), Some(Ok(operation_id))) => {
                if paging_token != operation_id {
                    return Err(HorizonParseError::Mismatch("operation id"));
                }
                paging_token
            }
            (Some(Ok(paging_token)), None) => paging_token,
            (None, Some(Ok(operation_id))) => operation_id,
            (Some(Err(error)), _) | (_, Some(Err(error))) => return Err(error),
            (None, None) => return Err(HorizonParseError::Missing("paging_token")),
        };
        if operation_ids.iter().any(|seen| seen == &id) {
            return Err(HorizonParseError::Ambiguous);
        }
        operation_ids.push(id.clone());
        if required(op, "transaction_successful")?.as_bool() != Some(true) {
            return Err(HorizonParseError::Mismatch("transaction_successful"));
        }
        let typ = string(op, "type")?;
        let (kind, destination, amount) = match typ.as_str() {
            "create_account" => (
                DepositKind::CreateAccount,
                string(op, "account")?,
                parse_xlm_to_stroops(&string(op, "starting_balance")?)
                    .map_err(|_| HorizonParseError::Invalid("starting_balance"))?,
            ),
            "payment" => {
                if string(op, "asset_type")? != "native" {
                    return Err(HorizonParseError::Mismatch("asset_type"));
                }
                (
                    DepositKind::NativePayment,
                    string(op, "to")?,
                    parse_xlm_to_stroops(&string(op, "amount")?)
                        .map_err(|_| HorizonParseError::Invalid("amount"))?,
                )
            }
            _ => continue,
        };
        if destination != expected_destination || amount != expected_amount_stroops {
            return Err(HorizonParseError::Mismatch("deposit"));
        }
        let deposit = FinalizedDeposit {
            transaction_hash: expected_hash.to_owned(),
            ledger: expected_ledger,
            operation_id: id,
            destination,
            amount_stroops: amount,
            kind,
        };
        if found.replace(deposit).is_some() {
            return Err(HorizonParseError::Ambiguous);
        }
    }
    found.ok_or(HorizonParseError::Missing("matching deposit operation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_ledger_parser_requires_bounded_valid_fields() {
        let valid = br#"{"sequence":42,"base_reserve_in_stroops":5000000,"closed_at":"2026-10-27T12:00:00Z"}"#;
        let parsed = parse_latest_ledger_reserve(valid).unwrap();
        assert_eq!(parsed.reserve.sequence, 42);
        assert_eq!(parsed.reserve.base_reserve_in_stroops, 5_000_000);
        assert_eq!(parsed.closed_at, "2026-10-27T12:00:00Z");

        for invalid in [
            br#"{"sequence":42,"base_reserve_in_stroops":5000000}"#.as_slice(),
            br#"{"sequence":"42","base_reserve_in_stroops":5000000,"closed_at":"x"}"#,
            br#"{"sequence":0,"base_reserve_in_stroops":5000000,"closed_at":"x"}"#,
            br#"{"sequence":42,"base_reserve_in_stroops":0,"closed_at":"x"}"#,
            br#"{"sequence":42,"base_reserve_in_stroops":5000000,"closed_at":"x"}"#,
            br#"{"sequence":42,"base_reserve_in_stroops":5000000,"closed_at":null}"#,
            br#"not-json"#,
        ] {
            assert!(parse_latest_ledger_reserve(invalid).is_err());
        }
    }
    const HASH: &str = "ef0fe04ac3c7de7228ca2598886059868ad05c224a041e8b2d9ee2a8a9dd6894";
    const DEST: &str = "GAYOLLLUIZE4DZMBB2ZBKGBUBZLIOYU6XFLW37GBP2VZD3ABNXCW4BVA";

    #[test]
    fn parses_account_snapshot_and_sponsorship_counts() {
        let json = br#"{"account_id":"G","sequence":"42","account_last_modified_ledger":42,"subentry_count":2,"num_sponsoring":1,"num_sponsored":3,"balances":[{"asset_type":"native","balance":"12.3400001"},{"asset_type":"credit_alphanum4","balance":"9.0"}]}"#;
        let a = parse_account(json, "G").unwrap();
        assert_eq!(
            (
                a.sequence,
                a.account_last_modified_ledger,
                a.native_balance_stroops,
                a.subentry_count,
                a.num_sponsoring,
                a.num_sponsored
            ),
            (42, 42, 123_400_001, 2, 1, 3)
        );
    }

    #[test]
    fn account_requires_positive_u32_last_modified_ledger() {
        for json in [
            br#"{"account_id":"G","sequence":"1","balances":[{"asset_type":"native","balance":"1"}]}"#.as_slice(),
            br#"{"account_id":"G","sequence":"1","account_last_modified_ledger":"42","balances":[{"asset_type":"native","balance":"1"}]}"#,
            br#"{"account_id":"G","sequence":"1","account_last_modified_ledger":0,"balances":[{"asset_type":"native","balance":"1"}]}"#,
            br#"{"account_id":"G","sequence":"1","account_last_modified_ledger":4294967296,"balances":[{"asset_type":"native","balance":"1"}]}"#,
        ] {
            assert!(parse_account(json, "G").is_err());
        }
    }

    #[test]
    fn parses_official_create_account_operation_example() {
        let json = format!(
            r#"{{"_embedded":{{"records":[{{"paging_token":"120192344791343105","transaction_hash":"{HASH}","transaction_successful":true,"type":"create_account","starting_balance":"2.0000000","account":"{DEST}"}}]}}}}"#
        );
        let d = parse_deposit_operations(json.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200)
            .unwrap();
        assert_eq!(d.kind, DepositKind::CreateAccount);
        assert_eq!(d.amount_stroops, 20_000_000);
        assert_eq!(d.operation_id, "120192344791343105");
    }

    #[test]
    fn rejects_noncanonical_operation_ids() {
        for field in ["paging_token", "id"] {
            let json = format!(
                r#"{{"_embedded":{{"records":[{{"{field}":"0123","transaction_hash":"{HASH}","transaction_successful":true,"type":"create_account","starting_balance":"2.0000000","account":"{DEST}"}}]}}}}"#
            );
            assert_eq!(
                parse_deposit_operations(json.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200),
                Err(HorizonParseError::Invalid(field))
            );
        }
    }

    #[test]
    fn rejects_conflicting_paging_token_and_operation_id() {
        let json = format!(
            r#"{{"_embedded":{{"records":[{{"paging_token":"123","id":"124","transaction_hash":"{HASH}","transaction_successful":true,"type":"create_account","starting_balance":"2.0000000","account":"{DEST}"}}]}}}}"#
        );
        assert_eq!(
            parse_deposit_operations(json.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200),
            Err(HorizonParseError::Mismatch("operation id"))
        );
    }

    #[test]
    fn rejects_duplicate_native_balance_and_non_native_payment() {
        let dup=br#"{"account_id":"G","sequence":"1","account_last_modified_ledger":1,"balances":[{"asset_type":"native","balance":"1"},{"asset_type":"native","balance":"2"}]}"#;
        assert_eq!(parse_account(dup, "G"), Err(HorizonParseError::Ambiguous));
        let json = format!(
            r#"{{"_embedded":{{"records":[{{"paging_token":"120192344791343105","transaction_hash":"{HASH}","transaction_successful":true,"type":"payment","asset_type":"credit_alphanum4","amount":"2.0000000","to":"{DEST}"}}]}}}}"#
        );
        assert_eq!(
            parse_deposit_operations(json.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200),
            Err(HorizonParseError::Mismatch("asset_type"))
        );
    }

    #[test]
    fn rejects_duplicate_matching_operation_identity() {
        let row = format!(
            r#"{{"paging_token":"123456789012345678","transaction_hash":"{HASH}","transaction_successful":true,"type":"create_account","starting_balance":"2.0000000","account":"{DEST}"}}"#
        );
        let json = format!(r#"{{"_embedded":{{"records":[{row},{row}]}}}}"#);
        assert_eq!(
            parse_deposit_operations(json.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200),
            Err(HorizonParseError::Ambiguous)
        );
    }

    #[test]
    fn rejects_full_page_and_any_mismatched_transaction_hash() {
        let row = format!(
            r#"{{"paging_token":"123456789012345678","transaction_hash":"{HASH}","transaction_successful":true,"type":"create_account","starting_balance":"2.0000000","account":"{DEST}"}}"#
        );
        let full = format!(r#"{{"_embedded":{{"records":[{row}]}}}}"#);
        assert_eq!(
            parse_deposit_operations(full.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 1),
            Err(HorizonParseError::TruncatedPage)
        );

        let other = format!(
            r#"{{"_embedded":{{"records":[{row},{{"paging_token":"2","transaction_hash":"wrong","transaction_successful":true,"type":"payment","asset_type":"native","amount":"2.0000000","to":"{DEST}"}}]}}}}"#
        );
        assert_eq!(
            parse_deposit_operations(other.as_bytes(), HASH, 27_984_461, DEST, 20_000_000, 200),
            Err(HorizonParseError::Mismatch("transaction_hash"))
        );
    }
}
