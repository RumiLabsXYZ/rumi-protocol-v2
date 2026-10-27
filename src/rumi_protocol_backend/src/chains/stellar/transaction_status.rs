//! Pure, fail-closed classification of Stellar transaction status evidence.
//!
//! DORMANT AND UNREGISTERED. This module is not declared in `mod.rs`, makes no
//! network calls, submits nothing, credits nothing, and mutates no state. It
//! only turns already-fetched Horizon `GET /transactions/{hash}` responses into
//! typed observations, combines up to three independent observations into a
//! quorum verdict, and maps that verdict (plus caller-supplied expiry and
//! sequence evidence) to a replacement decision.
//!
//! # Documentation consulted
//!
//! * Transaction object:
//!   <https://developers.stellar.org/docs/data/apis/horizon/api-reference/resources/transactions/object>
//!   lists `hash` (string), `ledger` (number), `successful` (boolean),
//!   `fee_charged` and `max_fee` (shown there as numbers), `source_account`
//!   (string) and `source_account_sequence` (string).
//! * Retrieve a transaction:
//!   <https://developers.stellar.org/docs/data/apis/horizon/api-reference/resources/retrieve-a-transaction>
//!   declares `fee_charged` / `max_fee` as strings in the schema but shows
//!   unquoted numbers in the example, and documents no 404 response. The two
//!   pages therefore disagree on the JSON type. Live Horizon is understood
//!   (INFERENCE, not stated in docs) to emit decimal strings of integer
//!   stroops. This module accepts BOTH a canonical decimal string and a
//!   non-negative JSON integer for `fee_charged`, and rejects anything else.
//! * Errors overview:
//!   <https://developers.stellar.org/docs/data/apis/horizon/api-reference/errors>
//!   only says 4XX-5XX statuses carry "additional information"; it does not
//!   define the problem body. The dedicated not_found page
//!   (`.../errors/http-status-codes/not-found`) returned HTTP 404 when fetched,
//!   and the Standard Status Codes page
//!   (`.../errors/http-status-codes/standard`) says only that a 404 means "The
//!   requested resource does not exist" with no JSON example. The problem+json
//!   shape used here (integer `status` of 404 and a `type` string ending in
//!   `not_found`) is therefore an INFERENCE from Horizon's problem responses
//!   (`type`, `title`, `status`, `detail`), not something the fetched docs
//!   state.
//! * Transaction result codes:
//!   <https://developers.stellar.org/docs/data/apis/horizon/api-reference/errors/result-codes/transactions>
//!   describes `tx_failed` ("none were applied") but does NOT state that failed
//!   transactions are included in ledgers or charged a fee. Fees page:
//!   <https://developers.stellar.org/docs/learn/fundamentals/fees-resource-limits-metering>
//!   defines a stroop as 1e-7 XLM and the minimum base fee as 100 stroops per
//!   operation; it only implies (for refundable Soroban budgets) that consumed
//!   resources are still paid. "A failed transaction is closed into a ledger and
//!   charges `fee_charged`" is an INFERENCE from Horizon exposing `successful ==
//!   false` together with `fee_charged` and `ledger` for such transactions.
//! * Operations and transactions:
//!   <https://developers.stellar.org/docs/learn/fundamentals/transactions/operations-and-transactions>
//!   states that a `maxTime` of 0 means no upper time bound (so an unbounded
//!   transaction may remain eligible for later transaction sets) and that a
//!   transaction's sequence number must be exactly one more than the source
//!   account's sequence number when applied; after execution the account
//!   sequence becomes the transaction's sequence.
//!
//! # Fail-closed rules
//!
//! * Only an HTTP 200 body that is a JSON object, whose `hash` equals the
//!   expected hash, with `ledger` in `1..=u32::MAX`, a JSON boolean
//!   `successful`, and a canonical non-zero `fee_charged <= i64::MAX` is an
//!   affirmative observation. Everything else is `Unavailable`.
//! * `NotFound` requires HTTP 404 plus a recognizable not_found problem body.
//! * Any conflict between affirmative observations is a quarantine and beats
//!   every other outcome.
//! * A single affirmative observation (`AffirmativeUnconfirmed`) blocks
//!   replacement even when other slots say NotFound.
//! * `NotFoundAgreed` is NOT permission to replace. Replacement additionally
//!   requires expiry evidence (agreed ledger close time strictly past the
//!   intent's bounded `maxTime` plus grace) and sequence evidence (account
//!   sequence still exactly `intent_sequence - 1`), see [`replacement_decision`].
//! * Provider independence (three distinct operators in the three slots) is a
//!   caller responsibility and is not verified here.

use serde_json::Value;

/// Why a provider response could not be turned into a definite observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    /// Status other than 200 or 404 (carries the status code).
    HttpStatus(u16),
    /// Body is not valid JSON or not a JSON object.
    MalformedBody,
    /// A required field is absent.
    MissingField(&'static str),
    /// A field is present but has the wrong type, range, or format.
    InvalidField(&'static str),
    /// Body `hash` is not the expected transaction hash.
    HashMismatch,
    /// HTTP 404 whose body is not a recognizable Horizon not_found problem.
    NotFoundBodyUnrecognized,
}

/// One provider's view of one transaction hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TxObservation {
    /// Found and `successful == true`. `hash` is canonical lowercase 64-hex.
    Applied {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    /// Found in a closed ledger with `successful == false`; a fee was charged.
    Failed {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    /// HTTP 404 with a recognizable Horizon not_found problem body.
    NotFound,
    /// Anything else. Carries no information about the transaction's fate.
    Unavailable(UnavailableReason),
}

/// Normalize a transaction hash to lowercase 64-hex, or `None` if malformed.
fn normalize_hash(hash: &str) -> Option<String> {
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}

/// Parse a canonical non-zero stroop amount no larger than `i64::MAX`.
/// Accepts digits only, no sign, no leading zeros, no whitespace.
fn parse_canonical_fee_str(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    let value: u64 = text.parse().ok()?;
    fee_in_range(value)
}

fn fee_in_range(value: u64) -> Option<u64> {
    if value == 0 || value > i64::MAX as u64 {
        None
    } else {
        Some(value)
    }
}

fn parse_found(body: &[u8], expected: &str) -> Result<TxObservation, UnavailableReason> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| UnavailableReason::MalformedBody)?;
    let object = value.as_object().ok_or(UnavailableReason::MalformedBody)?;

    match object.get("hash") {
        None => return Err(UnavailableReason::MissingField("hash")),
        Some(Value::String(actual)) => {
            if normalize_hash(actual).as_deref() != Some(expected) {
                return Err(UnavailableReason::HashMismatch);
            }
        }
        Some(_) => return Err(UnavailableReason::InvalidField("hash")),
    }

    let ledger = match object.get("ledger") {
        None => return Err(UnavailableReason::MissingField("ledger")),
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .filter(|ledger| *ledger >= 1)
            .ok_or(UnavailableReason::InvalidField("ledger"))?,
        Some(_) => return Err(UnavailableReason::InvalidField("ledger")),
    };

    let successful = match object.get("successful") {
        None => return Err(UnavailableReason::MissingField("successful")),
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(UnavailableReason::InvalidField("successful")),
    };

    let fee_charged_stroops = match object.get("fee_charged") {
        None => return Err(UnavailableReason::MissingField("fee_charged")),
        Some(Value::String(text)) => parse_canonical_fee_str(text),
        Some(Value::Number(number)) => number.as_u64().and_then(fee_in_range),
        Some(_) => None,
    }
    .ok_or(UnavailableReason::InvalidField("fee_charged"))?;

    let hash = expected.to_string();
    Ok(if successful {
        TxObservation::Applied {
            hash,
            ledger,
            fee_charged_stroops,
        }
    } else {
        TxObservation::Failed {
            hash,
            ledger,
            fee_charged_stroops,
        }
    })
}

/// INFERENCE (see module docs): a Horizon not_found problem is a JSON object
/// with integer `status == 404` and a `type` string ending in `not_found`.
fn is_not_found_problem(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let status_ok = object.get("status").and_then(Value::as_u64) == Some(404);
    let type_ok = object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.ends_with("not_found"));
    status_ok && type_ok
}

/// Classify one provider response for `GET /transactions/{expected_hash}`.
///
/// `expected_hash` must be exactly 64 hex characters (any case); callers must
/// never reach this function with a malformed hash. If they do, the result is
/// `Unavailable(InvalidField("expected_hash"))` regardless of status or body.
/// Never panics on any input.
pub fn parse_transaction_observation(
    http_status: u16,
    body: &[u8],
    expected_hash: &str,
) -> TxObservation {
    let Some(expected) = normalize_hash(expected_hash) else {
        return TxObservation::Unavailable(UnavailableReason::InvalidField("expected_hash"));
    };
    match http_status {
        200 => match parse_found(body, &expected) {
            Ok(observation) => observation,
            Err(reason) => TxObservation::Unavailable(reason),
        },
        404 => {
            if is_not_found_problem(body) {
                TxObservation::NotFound
            } else {
                TxObservation::Unavailable(UnavailableReason::NotFoundBodyUnrecognized)
            }
        }
        other => TxObservation::Unavailable(UnavailableReason::HttpStatus(other)),
    }
}

/// Why affirmative observations conflict. Ordered so that, when several pairs
/// conflict for different reasons, the greatest is reported; this keeps the
/// result independent of slot order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConflictReason {
    FeeMismatch,
    LedgerMismatch,
    AppliedAndFailed,
    HashMismatch,
}

/// Verdict of combining three slots of observations for one transaction hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatusQuorum {
    /// At least two identical Applied observations and no differing
    /// affirmative observation.
    ConfirmedApplied {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    /// At least two identical Failed observations and no differing
    /// affirmative observation.
    ConfirmedFailed {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    /// Two affirmative observations differ (or one has the wrong hash).
    Quarantine(ConflictReason),
    /// Exactly one affirmative observation and no conflict. Blocks replacement;
    /// the caller must re-observe.
    AffirmativeUnconfirmed,
    /// At least two NotFound and zero affirmative observations. NOT permission
    /// to replace.
    NotFoundAgreed,
    /// Everything else (too little information).
    Indeterminate,
}

struct Affirmative {
    failed: bool,
    hash: String,
    ledger: u32,
    fee: u64,
}

fn pair_conflict(left: &Affirmative, right: &Affirmative) -> Option<ConflictReason> {
    if left.hash != right.hash {
        Some(ConflictReason::HashMismatch)
    } else if left.failed != right.failed {
        Some(ConflictReason::AppliedAndFailed)
    } else if left.ledger != right.ledger {
        Some(ConflictReason::LedgerMismatch)
    } else if left.fee != right.fee {
        Some(ConflictReason::FeeMismatch)
    } else {
        None
    }
}

/// Combine three independent observations of `expected_hash`.
///
/// `None` slots count as no observation. The three slots must come from
/// distinct, independent providers; that is the caller's responsibility and is
/// not verified here. If `expected_hash` is not 64 hex characters the result is
/// `Indeterminate` (fail closed: it never confirms, and the decision layer
/// turns it into a hold).
pub fn transaction_status_quorum(
    slots: &[Option<TxObservation>; 3],
    expected_hash: &str,
) -> StatusQuorum {
    let Some(expected) = normalize_hash(expected_hash) else {
        return StatusQuorum::Indeterminate;
    };

    let mut affirmatives: Vec<Affirmative> = Vec::with_capacity(3);
    let mut not_found = 0usize;
    for slot in slots.iter().flatten() {
        match slot {
            TxObservation::Applied {
                hash,
                ledger,
                fee_charged_stroops,
            } => affirmatives.push(Affirmative {
                failed: false,
                hash: hash.clone(),
                ledger: *ledger,
                fee: *fee_charged_stroops,
            }),
            TxObservation::Failed {
                hash,
                ledger,
                fee_charged_stroops,
            } => affirmatives.push(Affirmative {
                failed: true,
                hash: hash.clone(),
                ledger: *ledger,
                fee: *fee_charged_stroops,
            }),
            TxObservation::NotFound => not_found += 1,
            TxObservation::Unavailable(_) => {}
        }
    }

    // Conflict beats everything. Defense in depth: a wrong-hash affirmative is
    // a conflict even if it is the only affirmative observation.
    let mut worst: Option<ConflictReason> = None;
    for item in &affirmatives {
        if item.hash != expected {
            worst = worst.max(Some(ConflictReason::HashMismatch));
        }
    }
    for left in 0..affirmatives.len() {
        for right in (left + 1)..affirmatives.len() {
            if let Some(reason) = pair_conflict(&affirmatives[left], &affirmatives[right]) {
                worst = worst.max(Some(reason));
            }
        }
    }
    if let Some(reason) = worst {
        return StatusQuorum::Quarantine(reason);
    }

    // No conflict: every affirmative observation is identical and carries the
    // expected hash.
    match affirmatives.first() {
        Some(first) if affirmatives.len() >= 2 => {
            if first.failed {
                StatusQuorum::ConfirmedFailed {
                    hash: expected,
                    ledger: first.ledger,
                    fee_charged_stroops: first.fee,
                }
            } else {
                StatusQuorum::ConfirmedApplied {
                    hash: expected,
                    ledger: first.ledger,
                    fee_charged_stroops: first.fee,
                }
            }
        }
        Some(_) => StatusQuorum::AffirmativeUnconfirmed,
        None if not_found >= 2 => StatusQuorum::NotFoundAgreed,
        None => StatusQuorum::Indeterminate,
    }
}

/// Expiry evidence for a transaction intent. The caller must derive
/// `agreed_latest_close_time_unix` from a quorum of ledger reads performed
/// AFTER the NotFound observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpiryEvidence {
    pub agreed_latest_close_time_unix: u64,
    /// The intent's transaction time-bounds `maxTime`. Zero means unbounded.
    pub intent_max_time_unix: u64,
    pub grace_secs: u64,
}

/// Sequence evidence for the intent's source account. The caller must derive
/// `agreed_account_sequence` from a quorum of account reads performed AFTER the
/// NotFound observations. A stale account read cannot prove "unchanged";
/// freshness of this evidence is the caller's responsibility (open design
/// question, not solved here).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceEvidence {
    pub agreed_account_sequence: i64,
    pub intent_sequence: i64,
}

/// Why the caller must wait and re-observe. Nothing may be replaced or settled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoldReason {
    AffirmativeUnconfirmed,
    Indeterminate,
    MissingEvidence,
    NotExpired,
    /// `intent_max_time_unix + grace_secs` overflowed `u64`.
    GraceOverflow,
}

/// Why the case must be escalated rather than retried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineReason {
    /// Affirmative observations conflict.
    StatusConflict(ConflictReason),
    /// `maxTime == 0`: per Stellar docs the transaction has no upper time bound
    /// and could still be included later, so expiry can never be proven.
    UnboundedIntent,
    /// Intent sequence below 1 (or otherwise unusable).
    InvalidIntentSequence,
    /// Account sequence is at or past the intent's sequence: the intent (or
    /// another transaction using that sequence) may have been consumed.
    /// INFERENCE from the sequence rule in the Stellar docs.
    SequenceAdvanced,
    /// Account sequence is below `intent_sequence - 1`, which a monotonic
    /// ledger state should never show for evidence of the same account.
    SequenceRegressed,
}

/// What the caller may do next. This function mutates nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplacementDecision {
    /// The transaction was applied by quorum; settle, never replace.
    Settled {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    /// The transaction failed on ledger by quorum and its fee was charged. The
    /// claim may become re-eligible after the CALLER charges
    /// `fee_charged_stroops` exactly once; nothing is charged here.
    ChargeFailedFee {
        hash: String,
        ledger: u32,
        fee_charged_stroops: u64,
    },
    Hold(HoldReason),
    Quarantine(QuarantineReason),
    /// Not-found quorum, intent provably expired, and account sequence still
    /// unconsumed: a replacement may be built and signed by the caller.
    PermitReplacement,
}

/// Map a quorum verdict plus separate expiry and sequence evidence to a
/// decision. The quorum alone never permits replacement.
///
/// The caller must obtain `expiry` and `seq` from separate quorum reads taken
/// AFTER the NotFound observations; evidence read earlier (or stale) cannot
/// prove the transaction did not land in between.
pub fn replacement_decision(
    quorum: &StatusQuorum,
    expiry: Option<&ExpiryEvidence>,
    seq: Option<&SequenceEvidence>,
) -> ReplacementDecision {
    match quorum {
        StatusQuorum::ConfirmedApplied {
            hash,
            ledger,
            fee_charged_stroops,
        } => ReplacementDecision::Settled {
            hash: hash.clone(),
            ledger: *ledger,
            fee_charged_stroops: *fee_charged_stroops,
        },
        StatusQuorum::ConfirmedFailed {
            hash,
            ledger,
            fee_charged_stroops,
        } => ReplacementDecision::ChargeFailedFee {
            hash: hash.clone(),
            ledger: *ledger,
            fee_charged_stroops: *fee_charged_stroops,
        },
        StatusQuorum::Quarantine(reason) => {
            ReplacementDecision::Quarantine(QuarantineReason::StatusConflict(*reason))
        }
        StatusQuorum::AffirmativeUnconfirmed => {
            ReplacementDecision::Hold(HoldReason::AffirmativeUnconfirmed)
        }
        StatusQuorum::Indeterminate => ReplacementDecision::Hold(HoldReason::Indeterminate),
        StatusQuorum::NotFoundAgreed => not_found_replacement(expiry, seq),
    }
}

fn not_found_replacement(
    expiry: Option<&ExpiryEvidence>,
    seq: Option<&SequenceEvidence>,
) -> ReplacementDecision {
    let (Some(expiry), Some(seq)) = (expiry, seq) else {
        return ReplacementDecision::Hold(HoldReason::MissingEvidence);
    };

    // Structural validity of the intent first: these are not "wait" cases.
    if expiry.intent_max_time_unix == 0 {
        return ReplacementDecision::Quarantine(QuarantineReason::UnboundedIntent);
    }
    let Some(previous_sequence) = seq
        .intent_sequence
        .checked_sub(1)
        .filter(|previous| *previous >= 0)
    else {
        return ReplacementDecision::Quarantine(QuarantineReason::InvalidIntentSequence);
    };

    let Some(deadline) = expiry.intent_max_time_unix.checked_add(expiry.grace_secs) else {
        return ReplacementDecision::Hold(HoldReason::GraceOverflow);
    };
    if expiry.agreed_latest_close_time_unix <= deadline {
        return ReplacementDecision::Hold(HoldReason::NotExpired);
    }

    if seq.agreed_account_sequence == previous_sequence {
        ReplacementDecision::PermitReplacement
    } else if seq.agreed_account_sequence >= seq.intent_sequence {
        ReplacementDecision::Quarantine(QuarantineReason::SequenceAdvanced)
    } else {
        ReplacementDecision::Quarantine(QuarantineReason::SequenceRegressed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hash() -> String {
        "ab".repeat(32)
    }

    fn found(successful: bool, fee: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": hash(),
            "hash": hash(),
            "ledger": 1234,
            "successful": successful,
            "fee_charged": fee,
            "max_fee": "200",
            "source_account": "GABC",
            "source_account_sequence": "99",
        }))
        .unwrap()
    }

    fn with_field(field: &str, value: Value) -> Vec<u8> {
        let mut body: Value = serde_json::from_slice(&found(true, json!("100"))).unwrap();
        body[field] = value;
        serde_json::to_vec(&body).unwrap()
    }

    fn without_field(field: &str) -> Vec<u8> {
        let mut body: Value = serde_json::from_slice(&found(true, json!("100"))).unwrap();
        body.as_object_mut().unwrap().remove(field);
        serde_json::to_vec(&body).unwrap()
    }

    fn parse200(body: &[u8]) -> TxObservation {
        parse_transaction_observation(200, body, &hash())
    }

    fn not_found_body() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "https://stellar.org/horizon-errors/not_found",
            "title": "Resource Missing",
            "status": 404,
            "detail": "The resource at the url requested was not found.",
        }))
        .unwrap()
    }

    // ---- parsing ----

    #[test]
    fn parse_applied_200() {
        assert_eq!(
            parse200(&found(true, json!("100"))),
            TxObservation::Applied {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: 100
            }
        );
    }

    #[test]
    fn parse_failed_200_charges_fee() {
        assert_eq!(
            parse200(&found(false, json!("250"))),
            TxObservation::Failed {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: 250
            }
        );
    }

    #[test]
    fn parse_accepts_integer_fee_as_docs_example_shows() {
        assert_eq!(
            parse200(&found(true, json!(100))),
            TxObservation::Applied {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: 100
            }
        );
    }

    #[test]
    fn parse_uppercase_hash_accepted_and_normalized() {
        let upper = hash().to_ascii_uppercase();
        let body = with_field("hash", json!(upper.clone()));
        assert_eq!(
            parse_transaction_observation(200, &body, &upper),
            TxObservation::Applied {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: 100
            }
        );
        // Mixed case between body and expected also matches.
        assert_eq!(
            parse_transaction_observation(200, &body, &hash()),
            TxObservation::Applied {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: 100
            }
        );
    }

    #[test]
    fn parse_hash_mismatch_unavailable() {
        let body = with_field("hash", json!("cd".repeat(32)));
        assert_eq!(
            parse200(&body),
            TxObservation::Unavailable(UnavailableReason::HashMismatch)
        );
        let short = with_field("hash", json!("abcd"));
        assert_eq!(
            parse200(&short),
            TxObservation::Unavailable(UnavailableReason::HashMismatch)
        );
        let wrong_type = with_field("hash", json!(5));
        assert_eq!(
            parse200(&wrong_type),
            TxObservation::Unavailable(UnavailableReason::InvalidField("hash"))
        );
        assert_eq!(
            parse200(&without_field("hash")),
            TxObservation::Unavailable(UnavailableReason::MissingField("hash"))
        );
    }

    #[test]
    fn parse_missing_fee_charged() {
        assert_eq!(
            parse200(&without_field("fee_charged")),
            TxObservation::Unavailable(UnavailableReason::MissingField("fee_charged"))
        );
    }

    #[test]
    fn parse_fee_zero_rejected() {
        for fee in [json!("0"), json!(0)] {
            assert_eq!(
                parse200(&found(true, fee)),
                TxObservation::Unavailable(UnavailableReason::InvalidField("fee_charged"))
            );
        }
    }

    #[test]
    fn parse_fee_leading_zero_rejected() {
        assert_eq!(
            parse200(&found(true, json!("0100"))),
            TxObservation::Unavailable(UnavailableReason::InvalidField("fee_charged"))
        );
    }

    #[test]
    fn parse_fee_above_i64_max_rejected() {
        assert_eq!(
            parse200(&found(true, json!("9223372036854775808"))),
            TxObservation::Unavailable(UnavailableReason::InvalidField("fee_charged"))
        );
        assert_eq!(
            parse200(&found(true, json!("99999999999999999999999"))),
            TxObservation::Unavailable(UnavailableReason::InvalidField("fee_charged"))
        );
        // i64::MAX itself is accepted.
        assert_eq!(
            parse200(&found(true, json!("9223372036854775807"))),
            TxObservation::Applied {
                hash: hash(),
                ledger: 1234,
                fee_charged_stroops: i64::MAX as u64
            }
        );
    }

    #[test]
    fn parse_fee_noncanonical_forms_rejected() {
        for fee in [
            json!("-100"),
            json!(-100),
            json!("+100"),
            json!(" 100"),
            json!("100 "),
            json!("1e2"),
            json!("1.0"),
            json!(100.0),
            json!(100.5),
            json!(""),
            json!(null),
            json!(true),
            json!([100]),
        ] {
            assert_eq!(
                parse200(&found(true, fee.clone())),
                TxObservation::Unavailable(UnavailableReason::InvalidField("fee_charged")),
                "fee {fee}"
            );
        }
    }

    #[test]
    fn parse_successful_must_be_bool() {
        for flag in [json!("true"), json!(1), json!(null)] {
            let body = with_field("successful", flag);
            assert_eq!(
                parse200(&body),
                TxObservation::Unavailable(UnavailableReason::InvalidField("successful"))
            );
        }
        assert_eq!(
            parse200(&without_field("successful")),
            TxObservation::Unavailable(UnavailableReason::MissingField("successful"))
        );
    }

    #[test]
    fn parse_ledger_bounds() {
        for ledger in [
            json!(0),
            json!(u64::from(u32::MAX) + 1),
            json!(-1),
            json!("5"),
            json!(5.5),
        ] {
            let body = with_field("ledger", ledger);
            assert_eq!(
                parse200(&body),
                TxObservation::Unavailable(UnavailableReason::InvalidField("ledger"))
            );
        }
        let max = with_field("ledger", json!(u32::MAX));
        assert_eq!(
            parse200(&max),
            TxObservation::Applied {
                hash: hash(),
                ledger: u32::MAX,
                fee_charged_stroops: 100
            }
        );
        assert_eq!(
            parse200(&without_field("ledger")),
            TxObservation::Unavailable(UnavailableReason::MissingField("ledger"))
        );
    }

    #[test]
    fn parse_404_not_found_problem() {
        assert_eq!(
            parse_transaction_observation(404, &not_found_body(), &hash()),
            TxObservation::NotFound
        );
    }

    #[test]
    fn parse_404_unrecognized_bodies_unavailable() {
        let unrecognized = TxObservation::Unavailable(UnavailableReason::NotFoundBodyUnrecognized);
        let bodies: Vec<Vec<u8>> = vec![
            b"not json".to_vec(),
            Vec::new(),
            b"[]".to_vec(),
            b"{}".to_vec(),
            serde_json::to_vec(&json!({"hello": "world"})).unwrap(),
            serde_json::to_vec(&json!({"type": "x/not_found", "status": 500})).unwrap(),
            serde_json::to_vec(&json!({"type": "x/not_found", "status": "404"})).unwrap(),
            serde_json::to_vec(&json!({"type": "x/not_found"})).unwrap(),
            serde_json::to_vec(&json!({"type": "x/server_error", "status": 404})).unwrap(),
            serde_json::to_vec(&json!({"type": 5, "status": 404})).unwrap(),
        ];
        for body in bodies {
            assert_eq!(
                parse_transaction_observation(404, &body, &hash()),
                unrecognized
            );
        }
    }

    #[test]
    fn parse_other_statuses_unavailable() {
        for status in [500u16, 429, 504, 400, 201, 301, 0, 999] {
            assert_eq!(
                parse_transaction_observation(status, &found(true, json!("100")), &hash()),
                TxObservation::Unavailable(UnavailableReason::HttpStatus(status))
            );
        }
    }

    #[test]
    fn parse_malformed_and_empty_body() {
        assert_eq!(
            parse200(b"{not json"),
            TxObservation::Unavailable(UnavailableReason::MalformedBody)
        );
        assert_eq!(
            parse200(b""),
            TxObservation::Unavailable(UnavailableReason::MalformedBody)
        );
        assert_eq!(
            parse200(b"[1,2,3]"),
            TxObservation::Unavailable(UnavailableReason::MalformedBody)
        );
        assert_eq!(
            parse200(b"null"),
            TxObservation::Unavailable(UnavailableReason::MalformedBody)
        );
        assert_eq!(
            parse200(&[0xff, 0xfe, 0x00]),
            TxObservation::Unavailable(UnavailableReason::MalformedBody)
        );
    }

    #[test]
    fn parse_invalid_expected_hash() {
        let invalid = TxObservation::Unavailable(UnavailableReason::InvalidField("expected_hash"));
        let bad_hashes = vec![
            String::new(),
            "abc".to_string(),
            "zz".repeat(32),
            "ab".repeat(33),
        ];
        for bad in &bad_hashes {
            assert_eq!(
                parse_transaction_observation(200, &found(true, json!("100")), bad),
                invalid
            );
            assert_eq!(
                parse_transaction_observation(404, &not_found_body(), bad),
                invalid
            );
        }
    }

    // ---- quorum ----

    fn applied(ledger: u32, fee: u64) -> Option<TxObservation> {
        Some(TxObservation::Applied {
            hash: hash(),
            ledger,
            fee_charged_stroops: fee,
        })
    }

    fn failed(ledger: u32, fee: u64) -> Option<TxObservation> {
        Some(TxObservation::Failed {
            hash: hash(),
            ledger,
            fee_charged_stroops: fee,
        })
    }

    fn nf() -> Option<TxObservation> {
        Some(TxObservation::NotFound)
    }

    fn un() -> Option<TxObservation> {
        Some(TxObservation::Unavailable(UnavailableReason::HttpStatus(
            500,
        )))
    }

    fn quorum(slots: [Option<TxObservation>; 3]) -> StatusQuorum {
        transaction_status_quorum(&slots, &hash())
    }

    fn confirmed_applied(ledger: u32, fee: u64) -> StatusQuorum {
        StatusQuorum::ConfirmedApplied {
            hash: hash(),
            ledger,
            fee_charged_stroops: fee,
        }
    }

    #[test]
    fn quorum_two_applied_confirms() {
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), None]),
            confirmed_applied(10, 100)
        );
        assert_eq!(
            quorum([applied(10, 100), un(), applied(10, 100)]),
            confirmed_applied(10, 100)
        );
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), nf()]),
            confirmed_applied(10, 100)
        );
    }

    #[test]
    fn quorum_three_applied_confirms() {
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), applied(10, 100)]),
            confirmed_applied(10, 100)
        );
    }

    #[test]
    fn quorum_two_failed_confirms() {
        assert_eq!(
            quorum([failed(11, 300), failed(11, 300), None]),
            StatusQuorum::ConfirmedFailed {
                hash: hash(),
                ledger: 11,
                fee_charged_stroops: 300
            }
        );
    }

    #[test]
    fn quorum_applied_and_failed_quarantine() {
        assert_eq!(
            quorum([applied(10, 100), failed(10, 100), None]),
            StatusQuorum::Quarantine(ConflictReason::AppliedAndFailed)
        );
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), failed(10, 100)]),
            StatusQuorum::Quarantine(ConflictReason::AppliedAndFailed)
        );
    }

    #[test]
    fn quorum_ledger_or_fee_difference_quarantine() {
        assert_eq!(
            quorum([applied(10, 100), applied(11, 100), None]),
            StatusQuorum::Quarantine(ConflictReason::LedgerMismatch)
        );
        assert_eq!(
            quorum([applied(10, 100), applied(10, 101), None]),
            StatusQuorum::Quarantine(ConflictReason::FeeMismatch)
        );
        assert_eq!(
            quorum([failed(10, 100), failed(10, 101), None]),
            StatusQuorum::Quarantine(ConflictReason::FeeMismatch)
        );
    }

    #[test]
    fn quorum_two_applied_plus_differing_applied_quarantine() {
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), applied(12, 100)]),
            StatusQuorum::Quarantine(ConflictReason::LedgerMismatch)
        );
        assert_eq!(
            quorum([applied(10, 100), applied(10, 100), applied(10, 999)]),
            StatusQuorum::Quarantine(ConflictReason::FeeMismatch)
        );
    }

    #[test]
    fn quorum_single_affirmative_blocks_even_with_not_found() {
        assert_eq!(
            quorum([applied(10, 100), nf(), nf()]),
            StatusQuorum::AffirmativeUnconfirmed
        );
        assert_eq!(
            quorum([nf(), failed(10, 100), nf()]),
            StatusQuorum::AffirmativeUnconfirmed
        );
        assert_eq!(
            quorum([un(), un(), applied(10, 100)]),
            StatusQuorum::AffirmativeUnconfirmed
        );
    }

    #[test]
    fn quorum_not_found_agreed() {
        assert_eq!(quorum([nf(), nf(), un()]), StatusQuorum::NotFoundAgreed);
        assert_eq!(quorum([nf(), nf(), None]), StatusQuorum::NotFoundAgreed);
        assert_eq!(quorum([nf(), nf(), nf()]), StatusQuorum::NotFoundAgreed);
    }

    #[test]
    fn quorum_indeterminate() {
        assert_eq!(quorum([nf(), un(), un()]), StatusQuorum::Indeterminate);
        assert_eq!(quorum([nf(), None, None]), StatusQuorum::Indeterminate);
        assert_eq!(quorum([un(), un(), un()]), StatusQuorum::Indeterminate);
        assert_eq!(quorum([None, None, None]), StatusQuorum::Indeterminate);
    }

    #[test]
    fn quorum_wrong_hash_affirmative_quarantines() {
        let wrong = Some(TxObservation::Applied {
            hash: "cd".repeat(32),
            ledger: 10,
            fee_charged_stroops: 100,
        });
        // Even alone.
        assert_eq!(
            quorum([wrong.clone(), None, None]),
            StatusQuorum::Quarantine(ConflictReason::HashMismatch)
        );
        // Even when two others agree on the expected hash.
        assert_eq!(
            quorum([wrong.clone(), applied(10, 100), applied(10, 100)]),
            StatusQuorum::Quarantine(ConflictReason::HashMismatch)
        );
        // Even when two slots agree on the wrong hash.
        assert_eq!(
            quorum([wrong.clone(), wrong, nf()]),
            StatusQuorum::Quarantine(ConflictReason::HashMismatch)
        );
    }

    #[test]
    fn quorum_uppercase_expected_hash_normalized() {
        let slots = [applied(10, 100), applied(10, 100), None];
        assert_eq!(
            transaction_status_quorum(&slots, &hash().to_ascii_uppercase()),
            confirmed_applied(10, 100)
        );
    }

    #[test]
    fn quorum_invalid_expected_hash_is_indeterminate() {
        let slots = [applied(10, 100), applied(10, 100), applied(10, 100)];
        assert_eq!(
            transaction_status_quorum(&slots, "nothex"),
            StatusQuorum::Indeterminate
        );
    }

    #[test]
    fn quorum_is_slot_order_independent() {
        let permutations = [
            [0usize, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        let cases: Vec<[Option<TxObservation>; 3]> = vec![
            [applied(10, 100), applied(10, 100), None],
            [applied(10, 100), applied(10, 100), applied(11, 100)],
            [applied(10, 100), failed(10, 100), applied(10, 100)],
            [applied(10, 100), applied(10, 101), applied(11, 100)],
            [applied(10, 100), nf(), nf()],
            [nf(), nf(), un()],
            [nf(), un(), un()],
            [failed(9, 50), failed(9, 50), nf()],
            [None, None, None],
        ];
        for case in cases {
            let baseline = quorum(case.clone());
            for order in permutations {
                let permuted = [
                    case[order[0]].clone(),
                    case[order[1]].clone(),
                    case[order[2]].clone(),
                ];
                assert_eq!(quorum(permuted), baseline, "case {case:?} order {order:?}");
            }
        }
    }

    // ---- replacement decision ----

    fn expiry(close: u64, max_time: u64, grace: u64) -> ExpiryEvidence {
        ExpiryEvidence {
            agreed_latest_close_time_unix: close,
            intent_max_time_unix: max_time,
            grace_secs: grace,
        }
    }

    fn sequence(account: i64, intent: i64) -> SequenceEvidence {
        SequenceEvidence {
            agreed_account_sequence: account,
            intent_sequence: intent,
        }
    }

    fn expired() -> ExpiryEvidence {
        expiry(1_000_101, 1_000_000, 100)
    }

    fn unchanged() -> SequenceEvidence {
        sequence(41, 42)
    }

    #[test]
    fn decision_maps_each_quorum_variant() {
        let good_expiry = expired();
        let good_seq = unchanged();
        assert_eq!(
            replacement_decision(&confirmed_applied(10, 100), None, None),
            ReplacementDecision::Settled {
                hash: hash(),
                ledger: 10,
                fee_charged_stroops: 100
            }
        );
        assert_eq!(
            replacement_decision(
                &StatusQuorum::ConfirmedFailed {
                    hash: hash(),
                    ledger: 11,
                    fee_charged_stroops: 300
                },
                Some(&good_expiry),
                Some(&good_seq)
            ),
            ReplacementDecision::ChargeFailedFee {
                hash: hash(),
                ledger: 11,
                fee_charged_stroops: 300
            }
        );
        assert_eq!(
            replacement_decision(
                &StatusQuorum::Quarantine(ConflictReason::FeeMismatch),
                Some(&good_expiry),
                Some(&good_seq)
            ),
            ReplacementDecision::Quarantine(QuarantineReason::StatusConflict(
                ConflictReason::FeeMismatch
            ))
        );
        assert_eq!(
            replacement_decision(
                &StatusQuorum::Indeterminate,
                Some(&good_expiry),
                Some(&good_seq)
            ),
            ReplacementDecision::Hold(HoldReason::Indeterminate)
        );
        assert_eq!(
            replacement_decision(
                &StatusQuorum::NotFoundAgreed,
                Some(&good_expiry),
                Some(&good_seq)
            ),
            ReplacementDecision::PermitReplacement
        );
    }

    #[test]
    fn affirmative_unconfirmed_never_permits_replacement() {
        let good_expiry = expired();
        let good_seq = unchanged();
        assert_eq!(
            replacement_decision(
                &StatusQuorum::AffirmativeUnconfirmed,
                Some(&good_expiry),
                Some(&good_seq)
            ),
            ReplacementDecision::Hold(HoldReason::AffirmativeUnconfirmed)
        );
    }

    #[test]
    fn not_found_without_evidence_holds() {
        let e = expired();
        let s = unchanged();
        let missing = ReplacementDecision::Hold(HoldReason::MissingEvidence);
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, None, None),
            missing
        );
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), None),
            missing
        );
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, None, Some(&s)),
            missing
        );
    }

    #[test]
    fn not_yet_expired_holds_at_boundary() {
        let s = unchanged();
        for close in [0u64, 999_999, 1_000_000, 1_000_100] {
            let e = expiry(close, 1_000_000, 100);
            assert_eq!(
                replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
                ReplacementDecision::Hold(HoldReason::NotExpired),
                "close {close}"
            );
        }
        let e = expiry(1_000_101, 1_000_000, 100);
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
            ReplacementDecision::PermitReplacement
        );
    }

    #[test]
    fn unbounded_intent_quarantines() {
        let e = expiry(u64::MAX, 0, 0);
        let s = unchanged();
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
            ReplacementDecision::Quarantine(QuarantineReason::UnboundedIntent)
        );
    }

    #[test]
    fn grace_overflow_holds() {
        let e = expiry(u64::MAX, u64::MAX, 1);
        let s = unchanged();
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
            ReplacementDecision::Hold(HoldReason::GraceOverflow)
        );
    }

    #[test]
    fn sequence_advanced_quarantines() {
        let e = expired();
        for account in [42i64, 43, i64::MAX] {
            let s = sequence(account, 42);
            assert_eq!(
                replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
                ReplacementDecision::Quarantine(QuarantineReason::SequenceAdvanced),
                "account {account}"
            );
        }
    }

    #[test]
    fn sequence_regressed_quarantines() {
        let e = expired();
        for account in [40i64, 0, -1, i64::MIN] {
            let s = sequence(account, 42);
            assert_eq!(
                replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
                ReplacementDecision::Quarantine(QuarantineReason::SequenceRegressed),
                "account {account}"
            );
        }
    }

    #[test]
    fn invalid_intent_sequence_does_not_panic() {
        let e = expired();
        for intent in [0i64, -1, i64::MIN] {
            let s = sequence(0, intent);
            assert_eq!(
                replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
                ReplacementDecision::Quarantine(QuarantineReason::InvalidIntentSequence),
                "intent {intent}"
            );
        }
        // Smallest valid intent sequence: account sequence 0 is "unchanged".
        let s = sequence(0, 1);
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
            ReplacementDecision::PermitReplacement
        );
        // Largest intent sequence does not overflow.
        let s = sequence(i64::MAX - 1, i64::MAX);
        assert_eq!(
            replacement_decision(&StatusQuorum::NotFoundAgreed, Some(&e), Some(&s)),
            ReplacementDecision::PermitReplacement
        );
    }
}
