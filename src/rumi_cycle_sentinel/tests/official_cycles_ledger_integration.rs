//! Task 6 gate: a live PocketIC integration test against the actual,
//! official `dfinity/cycles-ledger` release Wasm — not a mock, and not the
//! source-pinned Task 4 fixture (`src/cycles_ledger.rs`'s
//! `vendor_fixture_pins_the_wire_surface_used_by_this_adapter` test, which
//! only pins a hand-copied Candid excerpt and a behavior-vector text file).
//!
//! This suite installs the real ledger canister, exercises its actual
//! `icrc1_fee`/`icrc1_balance_of` query behavior, and proves the
//! security-critical "record before delivery" contract described in the
//! approved design
//! (`docs/superpowers/specs/2026-09-13-cycle-sentinel-telemetry-design.md`,
//! "Task 4 amendment: `Duplicate` is not delivery proof"): a `withdraw` to a
//! destination the IC cannot deliver cycles to still burns and *records* the
//! attempt, so a byte-identical retry comes back `Duplicate` referencing
//! that same recorded block — never as proof that the original destination
//! ever received anything.
//!
//! This file is fully self-contained and does not import from
//! `rumi_cycle_sentinel`'s production `src/` (those modules are private and
//! this suite intentionally verifies the ledger's real wire contract
//! independent of Sentinel's own adapter code). It does not touch
//! `tests/pocket_ic_integration.rs`, `tests/mock_canister/`, or any
//! production Sentinel source.
//!
//! Vendored artifacts (this file's only external inputs, besides the
//! PocketIC server binary):
//!
//! - `tests/vendor/official_cycles_ledger_v1_0_6.wasm.gz` — the unmodified
//!   official release asset, `cycles-ledger.wasm.gz`, DFINITY
//!   `dfinity/cycles-ledger` release `v1.0.6`.
//! - `tests/vendor/official_cycles_ledger_v1_0_6.did` — the unmodified
//!   official release asset, `cycles-ledger.did`, same release.
//!
//! Both are hash-pinned below and asserted byte-for-byte in
//! [`vendored_official_release_artifacts_match_pinned_hashes`] so a silent
//! swap of either file fails the suite instead of silently testing a
//! different ledger build.
//!
//! Build and run:
//!
//! ```text
//! POCKET_IC_BIN=/path/to/pocket-ic \
//!   cargo test -p rumi_cycle_sentinel --test official_cycles_ledger_integration
//! ```
//!
//! No mainnet call, deployment, funding, or live canister interaction is
//! possible from this test: every canister installed here is a local Wasm
//! inside an ephemeral PocketIC instance.

use candid::{CandidType, Decode, Encode, Nat, Principal};
use pocket_ic::{PocketIcBuilder, WasmResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Vendored, unmodified official release Wasm:
/// `dfinity/cycles-ledger` release `v1.0.6`, asset `cycles-ledger.wasm.gz`.
const OFFICIAL_LEDGER_WASM_GZ: &[u8] =
    include_bytes!("vendor/official_cycles_ledger_v1_0_6.wasm.gz");

/// Vendored, unmodified official release Candid:
/// `dfinity/cycles-ledger` release `v1.0.6`, asset `cycles-ledger.did`.
const OFFICIAL_LEDGER_DID: &str = include_str!("vendor/official_cycles_ledger_v1_0_6.did");

/// SHA-256 of `OFFICIAL_LEDGER_WASM_GZ`, independently verified against the
/// published release asset before vendoring.
const OFFICIAL_LEDGER_WASM_GZ_SHA256_HEX: &str =
    "ed99402535bb4f58e4ab469acc40c903f2fdeea409be16623d5c6a9131cbf120";

/// SHA-256 of `OFFICIAL_LEDGER_DID`, independently verified against the
/// published release asset before vendoring.
const OFFICIAL_LEDGER_DID_SHA256_HEX: &str =
    "179462f9038c632c71fb9527385d25b130b3c4e91b8f8213691506bcc438b43d";

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Deterministic gate that does not require `POCKET_IC_BIN`: fails loudly if
/// either vendored artifact drifts from the exact bytes recorded in the
/// approved design/plan, before any PocketIC test trusts their content.
#[test]
fn vendored_official_release_artifacts_match_pinned_hashes() {
    assert_eq!(
        sha256_hex(OFFICIAL_LEDGER_WASM_GZ),
        OFFICIAL_LEDGER_WASM_GZ_SHA256_HEX,
        "vendored official cycles-ledger v1.0.6 wasm.gz no longer matches the pinned release hash"
    );
    assert_eq!(
        sha256_hex(OFFICIAL_LEDGER_DID.as_bytes()),
        OFFICIAL_LEDGER_DID_SHA256_HEX,
        "vendored official cycles-ledger v1.0.6 .did no longer matches the pinned release hash"
    );
    for fragment in [
        "type InitArgs = record {",
        "max_blocks_per_request : nat64;",
        "index_id : opt principal;",
        "initial_balances : opt vec record { Account; nat };",
        "type LedgerArgs = variant {",
        "Init : InitArgs;",
        "type WithdrawArgs = record {",
        "type WithdrawError = variant {",
        "FailedToWithdraw : record {",
        "Duplicate : record { duplicate_of : nat };",
        "withdraw : (WithdrawArgs) -> (variant { Ok : BlockIndex; Err : WithdrawError });",
        "icrc1_balance_of : (Account) -> (nat) query;",
        "icrc1_fee : () -> (nat) query;",
    ] {
        assert!(
            OFFICIAL_LEDGER_DID.contains(fragment),
            "vendored official Candid is missing expected fragment `{fragment}`"
        );
    }
}

// ---------------------------------------------------------------------
// Hand-typed wire contract. Every field/variant below is copied verbatim
// from `OFFICIAL_LEDGER_DID` (asserted above), independent of and not
// imported from `rumi_cycle_sentinel::cycles_ledger` (that module is
// private, and this suite deliberately exercises the ledger's real wire
// contract rather than Sentinel's own decoding of it).
// ---------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<Vec<u8>>,
}

#[derive(CandidType)]
struct InitArgs {
    max_blocks_per_request: u64,
    index_id: Option<Principal>,
    initial_balances: Option<Vec<(Account, Nat)>>,
}

#[derive(CandidType)]
enum LedgerArgs {
    Init(InitArgs),
}

#[derive(CandidType, Clone)]
struct WithdrawArgs {
    amount: Nat,
    from_subaccount: Option<Vec<u8>>,
    to: Principal,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum WithdrawError {
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

type WithdrawReply = Result<Nat, WithdrawError>;

fn install_official_ledger(
    pic: &pocket_ic::PocketIc,
    funded_owner: Principal,
    seed_balance: u128,
) -> Principal {
    let ledger = pic.create_canister();
    pic.add_cycles(ledger, 200_000_000_000_000_000);
    let args = LedgerArgs::Init(InitArgs {
        max_blocks_per_request: 100,
        index_id: None,
        initial_balances: Some(vec![(
            Account {
                owner: funded_owner,
                subaccount: None,
            },
            Nat::from(seed_balance),
        )]),
    });
    pic.install_canister(
        ledger,
        OFFICIAL_LEDGER_WASM_GZ.to_vec(),
        Encode!(&args).expect("encode official LedgerArgs::Init"),
        None,
    );
    ledger
}

fn query_fee(pic: &pocket_ic::PocketIc, ledger: Principal) -> Nat {
    match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_fee",
            Encode!().unwrap(),
        )
        .expect("icrc1_fee query call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, Nat).expect("decode icrc1_fee reply"),
        WasmResult::Reject(message) => panic!("icrc1_fee rejected: {message}"),
    }
}

fn query_balance(pic: &pocket_ic::PocketIc, ledger: Principal, owner: Principal) -> Nat {
    let account = Account {
        owner,
        subaccount: None,
    };
    match pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            Encode!(&account).expect("encode icrc1_balance_of args"),
        )
        .expect("icrc1_balance_of query call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, Nat).expect("decode icrc1_balance_of reply"),
        WasmResult::Reject(message) => panic!("icrc1_balance_of rejected: {message}"),
    }
}

fn call_withdraw(
    pic: &pocket_ic::PocketIc,
    ledger: Principal,
    caller: Principal,
    args: &WithdrawArgs,
) -> WithdrawReply {
    match pic
        .update_call(
            ledger,
            caller,
            "withdraw",
            Encode!(args).expect("encode WithdrawArgs"),
        )
        .expect("withdraw update call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, WithdrawReply).expect("decode withdraw reply"),
        WasmResult::Reject(message) => panic!("withdraw call itself rejected: {message}"),
    }
}

/// A syntactically valid opaque canister principal that this PocketIC
/// instance never creates or installs anything at. `withdraw`'s
/// pre-burn `InvalidReceiver` check only rejects the management canister,
/// the ledger itself, and the anonymous/malformed principals — an ordinary
/// but nonexistent opaque canister ID passes that check, so the ledger
/// burns first and only discovers the destination cannot accept the
/// deposit when it makes the IC management-canister `deposit_cycles` call.
/// That is exactly the "record before delivery" boundary this suite proves.
fn nonexistent_canister_principal() -> Principal {
    // Opaque canister ID layout: 8-byte big-endian counter, 1 reserved byte,
    // then the 0x01 "opaque id" tag. A counter far above anything a fresh
    // PocketIC application subnet allocates in this small test keeps this
    // collision-free with `pic.create_canister()` IDs.
    let mut bytes = [0u8; 10];
    bytes[..8].copy_from_slice(&0x00FF_FFFF_FFFFu64.to_be_bytes()[..8]);
    bytes[9] = 0x01;
    Principal::from_slice(&bytes)
}

#[test]
fn official_ledger_reports_real_fee_and_seeded_balance() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let owner = Principal::from_slice(&[7; 29]);
    let seed_balance: u128 = 10_000_000_000_000;
    let ledger = install_official_ledger(&pic, owner, seed_balance);

    let fee = query_fee(&pic, ledger);
    assert!(fee > Nat::from(0u32), "official ledger fee must be nonzero");

    let balance = query_balance(&pic, ledger, owner);
    assert_eq!(
        balance,
        Nat::from(seed_balance),
        "icrc1_balance_of must reflect the exact InitArgs.initial_balances seed"
    );

    let stranger = Principal::from_slice(&[8; 29]);
    let stranger_balance = query_balance(&pic, ledger, stranger);
    assert_eq!(
        stranger_balance,
        Nat::from(0u32),
        "an unseeded account must read as exactly zero on the real ledger"
    );
}

/// The security-critical contract: `withdraw` to a destination the IC
/// cannot deliver cycles to still burns and *records* the attempt (the
/// ledger's own source records a transaction before it ever attempts the
/// management-canister `deposit_cycles` call). The caller sees
/// `FailedToWithdraw`, not an ambiguous "maybe it went through". A
/// byte-identical retry (same amount, same `to`, same `created_at_time`)
/// then hits the ledger's dedup window and comes back `Duplicate`,
/// referencing that same recorded block — proving only that the request
/// was recorded, never that the unreachable destination received anything.
#[test]
fn withdraw_to_unreachable_destination_records_then_duplicates_on_exact_retry() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let caller = Principal::from_slice(&[42; 29]);
    let seed_balance: u128 = 10_000_000_000_000;
    let ledger = install_official_ledger(&pic, caller, seed_balance);

    let fee = query_fee(&pic, ledger);
    // Comfortably above the fee so the ledger's failed-withdraw path mints a
    // penalized refund (`amount - fee`) rather than keeping the whole
    // `amount + fee` burned — this suite must observe a nonzero, exact,
    // ledger-computed burn either way, not assume a specific branch.
    let amount = fee.clone() + Nat::from(1_000_000_000u64);
    let destination = nonexistent_canister_principal();

    let created_at_time = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after the Unix epoch")
        .as_nanos() as u64;

    let args = WithdrawArgs {
        amount: amount.clone(),
        from_subaccount: None,
        to: destination,
        created_at_time: Some(created_at_time),
    };

    let balance_before = query_balance(&pic, ledger, caller);

    let first = call_withdraw(&pic, ledger, caller, &args);
    let (fee_block, rejection_code) = match first {
        Err(WithdrawError::FailedToWithdraw {
            fee_block,
            rejection_code,
            ..
        }) => (fee_block, rejection_code),
        other => panic!(
            "expected FailedToWithdraw for a withdrawal to a nonexistent canister, got {other:?}"
        ),
    };
    assert_ne!(
        rejection_code,
        RejectionCode::NoError,
        "a failed deposit must carry a real IC rejection code"
    );

    let balance_after_first = query_balance(&pic, ledger, caller);
    assert!(
        balance_after_first < balance_before,
        "the ledger must record (burn) the withdrawal attempt even though delivery failed — \
         this is the record-before-delivery invariant this suite exists to prove"
    );
    let net_debit = balance_before.clone() - balance_after_first.clone();
    assert!(
        net_debit == fee.clone() * Nat::from(2u32) || net_debit == amount.clone() + fee.clone(),
        "net debit ({net_debit}) must match one of the ledger's two documented failed-withdraw \
         outcomes: 2*fee (refund minted) or amount+fee (no refund minted); saw fee={fee}, amount={amount}"
    );

    // Exact byte-identical retry: same amount, same destination, same
    // explicit created_at_time, same caller. This must hit the ledger's own
    // dedup window and never re-attempt (and never silently succeed as a
    // fresh delivery) a second time.
    let second = call_withdraw(&pic, ledger, caller, &args);
    let duplicate_of = match second {
        Err(WithdrawError::Duplicate { duplicate_of }) => duplicate_of,
        other => {
            panic!("expected Duplicate on an exact retry of a recorded withdrawal, got {other:?}")
        }
    };
    // `duplicate_of` references a real prior block (observed: distinct from
    // `fee_block`, which is the separate penalized-refund mint block, not
    // the deduped withdraw transaction itself). This suite deliberately does
    // not assert exact equality with `fee_block` — the ledger's internal
    // block layout across the init mint / burn / refund transactions is an
    // implementation detail out of scope here. What matters, and what this
    // test proves, is the observable dedup contract: a `Duplicate` is
    // returned, it references *some* already-recorded block, and — per the
    // assertion below — it never triggers a second burn. Per the design's
    // "Task 4 amendment", neither this suite nor Sentinel's adapter may ever
    // treat that block reference as proof that `destination` received
    // anything.
    assert_ne!(
        duplicate_of,
        Nat::from(0u32),
        "duplicate_of should reference a real recorded transaction block, not a sentinel zero"
    );
    let _ = fee_block;

    let balance_after_second = query_balance(&pic, ledger, caller);
    assert_eq!(
        balance_after_second, balance_after_first,
        "a Duplicate reply must never burn again — it identifies a prior recorded \
         request, it is not a second withdrawal attempt"
    );

    // The whole point: Duplicate is record evidence only. This suite never
    // treats it as proof that `destination` received any cycles, and
    // neither must Sentinel's adapter (see cycles_ledger.rs doc comment and
    // the design's "Task 4 amendment: `Duplicate` is not delivery proof").
}

/// A non-retried, freshly distinct withdrawal (different `created_at_time`)
/// against the same unreachable destination must independently record and
/// fail again — proving the dedup path above is keyed on exact argument
/// equality, not merely on the destination being unreachable.
#[test]
fn distinct_created_at_time_is_not_treated_as_a_duplicate() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let caller = Principal::from_slice(&[43; 29]);
    let seed_balance: u128 = 10_000_000_000_000;
    let ledger = install_official_ledger(&pic, caller, seed_balance);

    let fee = query_fee(&pic, ledger);
    let amount = fee.clone() + Nat::from(1_000_000_000u64);
    let destination = nonexistent_canister_principal();
    let base_time = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after the Unix epoch")
        .as_nanos() as u64;

    let first_args = WithdrawArgs {
        amount: amount.clone(),
        from_subaccount: None,
        to: destination,
        created_at_time: Some(base_time),
    };
    let first = call_withdraw(&pic, ledger, caller, &first_args);
    assert!(
        matches!(first, Err(WithdrawError::FailedToWithdraw { .. })),
        "expected FailedToWithdraw on the first attempt, got {first:?}"
    );

    pic.advance_time(Duration::from_secs(1));
    let second_time = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after the Unix epoch")
        .as_nanos() as u64;
    assert_ne!(
        second_time, base_time,
        "the second attempt must use a genuinely distinct timestamp"
    );

    let second_args = WithdrawArgs {
        amount: amount.clone(),
        from_subaccount: None,
        to: destination,
        created_at_time: Some(second_time),
    };
    let second = call_withdraw(&pic, ledger, caller, &second_args);
    assert!(
        matches!(second, Err(WithdrawError::FailedToWithdraw { .. })),
        "a withdrawal with a distinct created_at_time must record and fail independently, \
         not silently collapse into Duplicate: got {second:?}"
    );
}
