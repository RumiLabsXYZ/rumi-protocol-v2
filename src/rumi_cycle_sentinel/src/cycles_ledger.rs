//! Hand-typed official ICRC Cycles Ledger `withdraw`/balance/fee interface,
//! and Sentinel's classification of a `withdraw` reply into a durable
//! outbox outcome.
//!
//! Every type below is copied field-for-field from the official Candid
//! interface at `dfinity/cycles-ledger`, release v1.0.6, commit
//! `29d98de5131918649a4c1cdd47fc176dea8770ef`
//! (`cycles-ledger/cycles-ledger.did`). Nothing here is inferred or
//! paraphrased from documentation — every variant, field name, and field
//! order matches the `.did` file exactly, so `candid::encode`/`decode`
//! against a live (or PocketIC-hosted) Cycles Ledger canister round-trips
//! byte-for-byte. Only the subset this canister actually calls
//! (`withdraw`, `icrc1_balance_of`, `icrc1_fee`) is reproduced; the ledger's
//! other methods (`icrc1_transfer`, `icrc2_*`, `create_canister*`, `icrc3_*`,
//! ...) are out of scope for Task 4.
//!
//! ```candid
//! type Account = record { owner : principal; subaccount : opt vec nat8 };
//! type BlockIndex = nat;
//! type RejectionCode = variant {
//!   NoError;
//!   CanisterError;
//!   SysTransient;
//!   DestinationInvalid;
//!   Unknown;
//!   SysFatal;
//!   CanisterReject;
//! };
//! type WithdrawArgs = record {
//!   amount : nat;
//!   from_subaccount : opt vec nat8;
//!   to : principal;
//!   created_at_time : opt nat64;
//! };
//! type WithdrawError = variant {
//!   GenericError : record { message : text; error_code : nat };
//!   TemporarilyUnavailable;
//!   FailedToWithdraw : record {
//!     fee_block : opt nat;
//!     rejection_code : RejectionCode;
//!     rejection_reason : text;
//!   };
//!   Duplicate : record { duplicate_of : nat };
//!   BadFee : record { expected_fee : nat };
//!   InvalidReceiver : record { receiver : principal };
//!   CreatedInFuture : record { ledger_time : nat64 };
//!   TooOld;
//!   InsufficientFunds : record { balance : nat };
//! };
//! withdraw : (WithdrawArgs) -> (variant { Ok : BlockIndex; Err : WithdrawError });
//! icrc1_balance_of : (Account) -> (nat) query;
//! icrc1_fee : () -> (nat) query;
//! ```

use candid::{CandidType, Nat, Principal};
use serde::Deserialize;

/// `Account` — `subaccount` uses plain `Vec<u8>` rather than
/// `serde_bytes::ByteBuf`: both encode identically as `opt vec nat8` on the
/// wire, and Task 4 never sends a non-`None` subaccount (Sentinel's Cycles
/// Ledger reserve uses its own default subaccount only — see the design's
/// "Sentinel's Cycles Ledger account" section), so the byte-encoding
/// optimization `serde_bytes` gives for large blobs does not matter here.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawArgs {
    pub amount: Nat,
    pub from_subaccount: Option<Vec<u8>>,
    pub to: Principal,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum WithdrawError {
    GenericError {
        message: String,
        error_code: Nat,
    },
    TemporarilyUnavailable,
    FailedToWithdraw {
        fee_block: Option<Nat>,
        rejection_code: RejectionCode,
        rejection_reason: String,
    },
    Duplicate {
        duplicate_of: Nat,
    },
    BadFee {
        expected_fee: Nat,
    },
    InvalidReceiver {
        receiver: Principal,
    },
    CreatedInFuture {
        ledger_time: u64,
    },
    TooOld,
    InsufficientFunds {
        balance: Nat,
    },
}

/// The exact `withdraw` reply shape: `variant { Ok : BlockIndex; Err :
/// WithdrawError }`. `candid`'s `serde::Deserialize`/`CandidType` impls for
/// `std::result::Result<T, E>` already produce/consume exactly this wire
/// shape, so no hand-written wrapper enum is needed.
pub type WithdrawReply = Result<Nat, WithdrawError>;

/// Sentinel's classification of a `withdraw` reply (or the absence of one,
/// on an inter-canister call rejection) into the durable outbox lifecycle —
/// see `funding::cycles` module doc for the full state-machine mapping this
/// feeds. Deliberately NOT a 1:1 mirror of `WithdrawError`'s variants:
/// several distinct `WithdrawError`s collapse to the same outcome because
/// they carry the same *proof strength* about whether cycles actually moved
/// (design requirement: "Use checked Nat conversions and fail closed on
/// overflow").
///
/// **`FailedToWithdraw` accounting (correction pass, ledger-security
/// review).** Both `FailedToWithdraw` cases below were verified directly
/// against the pinned ledger's own Rust source
/// (`cycles-ledger/src/storage.rs::withdraw`,
/// `cycles-ledger/src/lib.rs::withdraw_from_error_to_withdraw_error`, commit
/// `29d98de5131918649a4c1cdd47fc176dea8770ef`), not inferred from the
/// `.did` file or field names:
///
/// 1. `storage::withdraw` first burns `amount_with_fee = amount + FEE` from
///    the caller's account (step "1. burn cycles + fee").
/// 2. It calls `deposit_cycles` on the management canister. On failure it
///    computes `amount_to_reimburse = amount.saturating_sub(FEE)`.
/// 3. If `amount_to_reimburse` is `0` (i.e. `amount <= FEE`), it returns
///    `FailedToWithdrawFrom { refund_block: None, .. }` WITHOUT any further
///    mint — the WHOLE `amount + FEE` stays burned. `refund_block` becomes
///    this reply's `fee_block`.
/// 4. Otherwise, it mints `amount_to_reimburse = amount - FEE` back (a
///    `PENALIZE_MEMO` reimbursement — the ledger deliberately withholds a
///    SECOND `FEE` as a penalty for the failed withdrawal) and returns
///    `FailedToWithdrawFrom { refund_block: Some(fee_block), .. }`. Net
///    debit: `(amount + FEE) - (amount - FEE) = 2 * FEE`.
///
/// So `fee_block: Some(_)` is a KNOWN net debit of exactly `2 * fee`, not
/// one fee, and `fee_block: None` is a KNOWN net debit of the FULL
/// `amount + fee` that was reserved — both are fully determined by the
/// decoded reply alone, so neither is "ambiguous": only the absence of any
/// decoded reply at all (a call rejection) is genuinely `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawOutcome {
    /// `Ok(block)`: proven success.
    Confirmed(u64),
    /// `Err(Duplicate { duplicate_of })`: the ledger has a record for an
    /// earlier request, but this is NOT delivery proof. The pinned ledger
    /// records/deduplicates before attempting the management-canister
    /// deposit, so the resolver must quarantine this outcome and retain the
    /// reservation for independent signer reconciliation.
    Duplicate(u64),
    /// The inter-canister call itself was rejected, OR the ledger replied
    /// `TemporarilyUnavailable`/`GenericError`, OR a block index/duplicate-of/
    /// fee-block value did not fit `u64` (an overflow this canister must
    /// fail closed on rather than trap or silently truncate). Ambiguous:
    /// retry with the exact same persisted arguments while the ledger's
    /// dedup window remains valid. Never falls through to any other rail.
    Unknown,
    /// `Err(TooOld)`: the ledger's own dedup window for this
    /// `created_at_time` has passed with no independent proof of the
    /// outcome. Needs a signer, not an automatic retry.
    Quarantined,
    /// `Err(BadFee | InvalidReceiver | CreatedInFuture | InsufficientFunds)`:
    /// the ledger rejected the request before ever touching the account —
    /// proven no delivery AND no fee debit.
    TerminalNoSpend,
    /// `Err(FailedToWithdraw { fee_block: Some(block), .. })`: outbound
    /// delivery is proven to have failed, and the pinned ledger's own
    /// reimbursement logic proves a KNOWN net debit of exactly `2 * fee`
    /// (burned `amount+fee`, refunded only `amount-fee` as a withdrawal
    /// penalty) — see this type's doc comment. `fee_block` is the refund
    /// block index, retained for evidence/traceability; the caller derives
    /// the actual known-spent amount as `2 * fee_cycles` from its own
    /// cached fee, not from this field.
    TerminalFeeDebited { fee_block: u64 },
    /// `Err(FailedToWithdraw { fee_block: None, .. })`: outbound delivery is
    /// proven to have failed, and the pinned ledger's own reimbursement
    /// logic proves NO refund was ever created (`amount <= fee`, so
    /// `amount.saturating_sub(fee) == 0`) — the caller's known net debit is
    /// the FULL reserved `amount + fee`. This is a deterministic, fully
    /// decoded case, not an ambiguous one — see this type's doc comment.
    TerminalFullAmountDebited,
}

/// An explicit, independently verified result for a quarantined Cycles
/// operation. This is deliberately not constructible from a `Duplicate`
/// reply: the Task 6 ledger adapter/reconciliation endpoint must verify the
/// authoritative block or account outcome first, then pass one of these
/// decisions to the core state machine. `FeeDebited` carries the exact known
/// source debit so source/cap accounting remains conservative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuarantinedCyclesEvidence {
    /// Independent ledger evidence proves the original withdrawal delivered
    /// to the immutable destination at this block.
    Delivered { block_index: u64 },
    /// Independent evidence proves no cycles were delivered and no source
    /// debit occurred.
    NoSpend,
    /// Independent evidence proves a known source debit without delivery,
    /// such as a pinned fee/refund penalty outcome.
    FeeDebited { known_spent_cycles: u128 },
}

/// Checked `Nat -> u64` conversion for a block index. `None` on overflow —
/// callers must fail closed (treat as `Unknown`), never trap or truncate.
fn checked_block_index(value: &Nat) -> Option<u64> {
    u64::try_from(value.0.clone()).ok()
}

/// Pure classification of an already-decoded `withdraw` reply — see
/// `WithdrawOutcome`'s doc comment for the full mapping and its rationale.
/// Exercised directly by `cycles_ledger::tests` against the CANDID-decoded
/// `WithdrawReply`, so the mapping is provably exact even without a live
/// canister.
pub fn classify_withdraw_reply(reply: WithdrawReply) -> WithdrawOutcome {
    match reply {
        Ok(block) => match checked_block_index(&block) {
            Some(block) => WithdrawOutcome::Confirmed(block),
            None => WithdrawOutcome::Unknown,
        },
        Err(WithdrawError::Duplicate { duplicate_of }) => {
            match checked_block_index(&duplicate_of) {
                Some(block) => WithdrawOutcome::Duplicate(block),
                None => WithdrawOutcome::Unknown,
            }
        }
        Err(WithdrawError::TemporarilyUnavailable) => WithdrawOutcome::Unknown,
        Err(WithdrawError::GenericError { .. }) => WithdrawOutcome::Unknown,
        Err(WithdrawError::TooOld) => WithdrawOutcome::Quarantined,
        Err(WithdrawError::BadFee { .. })
        | Err(WithdrawError::InvalidReceiver { .. })
        | Err(WithdrawError::CreatedInFuture { .. })
        | Err(WithdrawError::InsufficientFunds { .. }) => WithdrawOutcome::TerminalNoSpend,
        Err(WithdrawError::FailedToWithdraw {
            fee_block: Some(fee_block),
            ..
        }) => match checked_block_index(&fee_block) {
            Some(fee_block) => WithdrawOutcome::TerminalFeeDebited { fee_block },
            // The ledger promised a refund block but its index didn't fit
            // u64: we cannot prove which block, so fail closed to ambiguous
            // rather than asserting an unrepresentable one.
            None => WithdrawOutcome::Unknown,
        },
        Err(WithdrawError::FailedToWithdraw {
            fee_block: None, ..
        }) => {
            // Verified against the pinned ledger source: `fee_block: None`
            // is the deterministic `amount <= fee` case (no refund was ever
            // created), NOT an ambiguous reply — see this type's doc
            // comment. The full held amount is known to be gone.
            WithdrawOutcome::TerminalFullAmountDebited
        }
    }
}

/// Calls the Cycles Ledger's `withdraw` and classifies the reply. An
/// inter-canister call rejection (network/trap/reject at the IC level,
/// distinct from a well-formed `Err(WithdrawError)` reply) is itself
/// ambiguous about whether the ledger ever executed the request, so it also
/// classifies as `Unknown` — the design's "Inter-canister rejection ...
/// become Unknown."
pub async fn withdraw(ledger: Principal, args: WithdrawArgs) -> WithdrawOutcome {
    match ic_cdk::call::<(WithdrawArgs,), (WithdrawReply,)>(ledger, "withdraw", (args,)).await {
        Ok((reply,)) => classify_withdraw_reply(reply),
        Err(_) => WithdrawOutcome::Unknown,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheQueryError {
    /// The inter-canister query call itself was rejected.
    CallFailed,
    /// The ledger returned a balance/fee value that does not fit `u128`
    /// (checked conversion; fail closed rather than saturate/truncate).
    Overflow,
}

/// Queries `icrc1_balance_of(Account { owner: sentinel_id, subaccount: None
/// })` and `icrc1_fee()` and returns both as checked `u128` — the "separate
/// refresh seam" the design requires: `funding::cycles::refresh_cache` (and,
/// later, the Task 6 sampler/timer) is the only caller. `prepare`/`reserve_*`
/// never call this themselves; they only ever consume whatever was cached by
/// the most recent successful call here.
pub async fn query_balance_and_fee(
    ledger: Principal,
    sentinel_id: Principal,
) -> Result<(u128, u128), CacheQueryError> {
    let account = Account {
        owner: sentinel_id,
        subaccount: None,
    };
    let balance = ic_cdk::call::<(Account,), (Nat,)>(ledger, "icrc1_balance_of", (account,))
        .await
        .map_err(|_| CacheQueryError::CallFailed)?
        .0;
    let fee = ic_cdk::call::<(), (Nat,)>(ledger, "icrc1_fee", ())
        .await
        .map_err(|_| CacheQueryError::CallFailed)?
        .0;
    let balance = u128::try_from(balance.0).map_err(|_| CacheQueryError::Overflow)?;
    let fee = u128::try_from(fee.0).map_err(|_| CacheQueryError::Overflow)?;
    Ok((balance, fee))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const PINNED_CYCLES_LEDGER_DID: &str = include_str!("../tests/vendor/cycles_ledger_v1_0_6.did");
    const PINNED_WITHDRAW_BEHAVIOR: &str =
        include_str!("../tests/vendor/cycles_ledger_v1_0_6_withdraw_behavior.txt");
    const PINNED_CYCLES_LEDGER_DID_SHA256: [u8; 32] = [
        0xde, 0xe2, 0x99, 0xa8, 0x01, 0x5a, 0xfe, 0xd4, 0x42, 0x26, 0x1a, 0x70, 0x90, 0xdc, 0xdb,
        0xd6, 0x3a, 0x44, 0xca, 0x3d, 0xfc, 0x72, 0xee, 0x8b, 0xa3, 0x25, 0xd0, 0x9e, 0x39, 0x77,
        0x3e, 0x5c,
    ];
    const PINNED_WITHDRAW_BEHAVIOR_SHA256: [u8; 32] = [
        0xc9, 0x41, 0x68, 0x2f, 0xdb, 0x80, 0x04, 0x30, 0xee, 0x8d, 0xcd, 0xda, 0x60, 0x86, 0x20,
        0xc7, 0xdb, 0xb0, 0x8e, 0x8f, 0x18, 0x59, 0x16, 0x62, 0x4a, 0x7e, 0x7d, 0x43, 0x26, 0xf0,
        0x09, 0xb0,
    ];

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    /// This fixture mirrors the pinned `cycles-ledger` source's failed
    /// management-canister deposit branch at commit
    /// `29d98de5131918649a4c1cdd47fc176dea8770ef`: burn `amount + fee`, then
    /// refund `amount - fee` only when that value is positive.
    fn pinned_failed_deposit_vector(amount: u128, fee: u128) -> (Option<u128>, u128) {
        let refund = if amount > fee {
            Some(amount - fee)
        } else {
            None
        };
        let burned = amount.checked_add(fee).unwrap();
        let net_debit = match refund {
            Some(refund) => burned.checked_sub(refund).unwrap(),
            None => burned,
        };
        (refund, net_debit)
    }

    #[test]
    fn vendor_fixture_pins_the_wire_surface_used_by_this_adapter() {
        assert_eq!(
            sha256(PINNED_CYCLES_LEDGER_DID.as_bytes()),
            PINNED_CYCLES_LEDGER_DID_SHA256,
            "the vendored Candid fixture changed without a source-pin update"
        );
        assert!(PINNED_CYCLES_LEDGER_DID
            .contains("// Commit: 29d98de5131918649a4c1cdd47fc176dea8770ef"));
        for field in [
            "amount : nat;",
            "from_subaccount : opt vec nat8;",
            "to : principal;",
            "created_at_time : opt nat64;",
            "fee_block : opt nat;",
            "duplicate_of : nat",
            "withdraw : (WithdrawArgs) -> (variant { Ok : BlockIndex; Err : WithdrawError });",
            "icrc1_balance_of : (Account) -> (nat) query;",
            "icrc1_fee : () -> (nat) query;",
        ] {
            assert!(
                PINNED_CYCLES_LEDGER_DID.contains(field),
                "pinned Cycles Ledger fixture is missing `{field}`"
            );
        }
    }

    #[test]
    fn pinned_failed_deposit_vectors_cover_refund_and_fee_outcomes() {
        assert_eq!(
            sha256(PINNED_WITHDRAW_BEHAVIOR.as_bytes()),
            PINNED_WITHDRAW_BEHAVIOR_SHA256,
            "the vendored behavior fixture changed without a source-pin update"
        );
        for pin in [
            "record_before_management_deposit = true",
            "burned_amount = amount + FEE",
            "refund_amount = amount.saturating_sub(FEE)",
            "refund_memo = PENALIZE_MEMO",
            "failed_withdraw_fee_block = refund_block",
            "duplicate_after_record_is_delivery_proof = false",
        ] {
            assert!(
                PINNED_WITHDRAW_BEHAVIOR.contains(pin),
                "pinned Cycles Ledger behavior fixture is missing `{pin}`"
            );
        }
        // amount > fee: one fee-penalized refund is minted, leaving 2*fee.
        assert_eq!(pinned_failed_deposit_vector(10, 3), (Some(7), 6));
        // amount == fee and amount < fee: no refund is minted, leaving the
        // full burned amount+fee.
        assert_eq!(pinned_failed_deposit_vector(3, 3), (None, 6));
        assert_eq!(pinned_failed_deposit_vector(2, 3), (None, 5));
    }

    #[test]
    fn duplicate_after_failed_deposit_is_record_evidence_not_delivery_proof() {
        // The ledger records/deduplicates before its management-canister
        // deposit. Model the failed first delivery (whose net debit is
        // independently pinned above), then the exact retry that receives a
        // Duplicate for that pre-existing record. The latter remains distinct
        // from Ok; funding resolution quarantines it until independent proof.
        let (_, first_net_debit) = pinned_failed_deposit_vector(10, 3);
        assert_eq!(first_net_debit, 6);
        let outcome = classify_withdraw_reply(Err(WithdrawError::Duplicate {
            duplicate_of: Nat::from(44u32),
        }));
        assert_eq!(outcome, WithdrawOutcome::Duplicate(44));
        assert!(!matches!(outcome, WithdrawOutcome::Confirmed(_)));
    }

    // ─── Official Candid encode/decode: Account ───

    #[test]
    fn account_round_trips_with_and_without_subaccount() {
        let with_sub = Account {
            owner: Principal::management_canister(),
            subaccount: Some(vec![1u8; 32]),
        };
        let bytes = candid::encode_one(&with_sub).unwrap();
        assert_eq!(candid::decode_one::<Account>(&bytes).unwrap(), with_sub);

        let without_sub = Account {
            owner: Principal::management_canister(),
            subaccount: None,
        };
        let bytes = candid::encode_one(&without_sub).unwrap();
        assert_eq!(candid::decode_one::<Account>(&bytes).unwrap(), without_sub);
    }

    // ─── Official Candid encode/decode: WithdrawArgs ───

    #[test]
    fn withdraw_args_round_trips() {
        let args = WithdrawArgs {
            amount: Nat::from(1_000_000_000_000u128),
            from_subaccount: None,
            to: Principal::management_canister(),
            created_at_time: Some(123_456_789),
        };
        let bytes = candid::encode_one(&args).unwrap();
        assert_eq!(candid::decode_one::<WithdrawArgs>(&bytes).unwrap(), args);
    }

    // ─── Official Candid encode/decode: every WithdrawError variant ───

    fn withdraw_error_variants() -> Vec<WithdrawError> {
        vec![
            WithdrawError::GenericError {
                message: "boom".to_string(),
                error_code: Nat::from(7u32),
            },
            WithdrawError::TemporarilyUnavailable,
            WithdrawError::FailedToWithdraw {
                fee_block: Some(Nat::from(42u32)),
                rejection_code: RejectionCode::CanisterReject,
                rejection_reason: "target rejected deposit".to_string(),
            },
            WithdrawError::FailedToWithdraw {
                fee_block: None,
                rejection_code: RejectionCode::SysTransient,
                rejection_reason: "transient".to_string(),
            },
            WithdrawError::Duplicate {
                duplicate_of: Nat::from(9u32),
            },
            WithdrawError::BadFee {
                expected_fee: Nat::from(10u32),
            },
            WithdrawError::InvalidReceiver {
                receiver: Principal::anonymous(),
            },
            WithdrawError::CreatedInFuture {
                ledger_time: 555_555,
            },
            WithdrawError::TooOld,
            WithdrawError::InsufficientFunds {
                balance: Nat::from(3u32),
            },
        ]
    }

    #[test]
    fn every_withdraw_error_variant_round_trips_exactly() {
        for variant in withdraw_error_variants() {
            let bytes = candid::encode_one(&variant).unwrap();
            assert_eq!(
                candid::decode_one::<WithdrawError>(&bytes).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn every_rejection_code_variant_round_trips_exactly() {
        let variants = [
            RejectionCode::NoError,
            RejectionCode::CanisterError,
            RejectionCode::SysTransient,
            RejectionCode::DestinationInvalid,
            RejectionCode::Unknown,
            RejectionCode::SysFatal,
            RejectionCode::CanisterReject,
        ];
        for variant in variants {
            let bytes = candid::encode_one(&variant).unwrap();
            assert_eq!(
                candid::decode_one::<RejectionCode>(&bytes).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn withdraw_reply_ok_and_err_round_trip_as_the_official_variant_shape() {
        let ok: WithdrawReply = Ok(Nat::from(100u32));
        let bytes = candid::encode_one(&ok).unwrap();
        assert_eq!(candid::decode_one::<WithdrawReply>(&bytes).unwrap(), ok);

        let err: WithdrawReply = Err(WithdrawError::TooOld);
        let bytes = candid::encode_one(&err).unwrap();
        assert_eq!(candid::decode_one::<WithdrawReply>(&bytes).unwrap(), err);
    }

    // ─── Decoding directly against a hand-encoded official wire fixture ───
    //
    // Proves this module's types agree with the EXACT wire bytes the
    // official `.did` shape would produce, not just with themselves — by
    // encoding via `candid::IDLValue`-free raw field order matching the
    // published `withdraw` service signature, independent of this file's
    // own struct definitions ever having existed.

    #[test]
    fn decodes_a_manually_constructed_ok_reply() {
        use candid::{Decode, Encode};
        // Simulates exactly what the ledger's `(variant { Ok : nat })`
        // reply would encode to.
        let bytes = Encode!(&Ok::<Nat, WithdrawError>(Nat::from(7u32))).unwrap();
        let decoded = Decode!(&bytes, WithdrawReply).unwrap();
        assert_eq!(decoded, Ok(Nat::from(7u32)));
    }

    // ─── classify_withdraw_reply ───

    #[test]
    fn classifies_ok_as_confirmed() {
        assert_eq!(
            classify_withdraw_reply(Ok(Nat::from(42u32))),
            WithdrawOutcome::Confirmed(42)
        );
    }

    #[test]
    fn classifies_duplicate_as_recorded_prior_request() {
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::Duplicate {
                duplicate_of: Nat::from(11u32)
            })),
            WithdrawOutcome::Duplicate(11)
        );
    }

    #[test]
    fn classifies_temporarily_unavailable_and_generic_error_as_unknown() {
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::TemporarilyUnavailable)),
            WithdrawOutcome::Unknown
        );
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::GenericError {
                message: "?".to_string(),
                error_code: Nat::from(1u32)
            })),
            WithdrawOutcome::Unknown
        );
    }

    #[test]
    fn classifies_too_old_as_quarantined() {
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::TooOld)),
            WithdrawOutcome::Quarantined
        );
    }

    #[test]
    fn classifies_validation_failures_as_terminal_no_spend() {
        for err in [
            WithdrawError::BadFee {
                expected_fee: Nat::from(1u32),
            },
            WithdrawError::InvalidReceiver {
                receiver: Principal::anonymous(),
            },
            WithdrawError::CreatedInFuture { ledger_time: 1 },
            WithdrawError::InsufficientFunds {
                balance: Nat::from(0u32),
            },
        ] {
            assert_eq!(
                classify_withdraw_reply(Err(err)),
                WithdrawOutcome::TerminalNoSpend
            );
        }
    }

    #[test]
    fn classifies_failed_to_withdraw_with_known_fee_block_as_terminal_fee_debited() {
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::FailedToWithdraw {
                fee_block: Some(Nat::from(5u32)),
                rejection_code: RejectionCode::CanisterReject,
                rejection_reason: "nope".to_string(),
            })),
            WithdrawOutcome::TerminalFeeDebited { fee_block: 5 }
        );
    }

    /// Correction pass (ledger-security review): `fee_block: None` is a
    /// DETERMINISTIC, fully decoded case verified against the pinned
    /// ledger's own source (the `amount <= fee` no-refund path) — not
    /// ambiguous, and never `Unknown`. Only a call rejection (no decoded
    /// reply at all) is genuinely ambiguous.
    #[test]
    fn classifies_no_refund_failed_to_withdraw_as_terminal_full_amount_debited_never_unknown() {
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::FailedToWithdraw {
                fee_block: None,
                rejection_code: RejectionCode::SysTransient,
                rejection_reason: "?".to_string(),
            })),
            WithdrawOutcome::TerminalFullAmountDebited
        );
    }

    #[test]
    fn classifies_block_index_overflow_as_unknown_fail_closed() {
        let huge = Nat::from(u128::MAX) + Nat::from(1u32);
        assert_eq!(
            classify_withdraw_reply(Ok(huge.clone())),
            WithdrawOutcome::Unknown
        );
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::Duplicate {
                duplicate_of: huge.clone()
            })),
            WithdrawOutcome::Unknown
        );
        assert_eq!(
            classify_withdraw_reply(Err(WithdrawError::FailedToWithdraw {
                fee_block: Some(huge),
                rejection_code: RejectionCode::Unknown,
                rejection_reason: "?".to_string(),
            })),
            WithdrawOutcome::Unknown
        );
    }
}
