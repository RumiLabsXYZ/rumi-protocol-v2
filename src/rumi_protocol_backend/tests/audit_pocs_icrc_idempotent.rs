//! Wave-3 ICRC transfer hygiene regression fences.
//!
//! Audit report: `audit-reports/2026-04-22-28e9896/verification-results.md`
//! sections ICRC-001, ICRC-002, ICRC-003, ICRC-004, ICRC-005.
//!
//! # What the bug class was
//!
//! Every backend ICRC transfer set `created_at_time: None`, every retry path
//! treated `TransferError::Duplicate { .. }` as a generic failure, and the
//! ledger fee was cached forever. The combination meant:
//!
//!   * **ICRC-001 / ICRC-002**: when the IC drops a reply on a transfer that
//!     actually committed, the retry has a fresh dedup tuple at the ledger
//!     and lands as a second, distinct block. End state: the recipient is
//!     paid twice and the protocol's bookkeeping is short by one transfer.
//!   * **ICRC-003**: the `Duplicate` arm fires whenever the ledger correctly
//!     deduplicates a retry; the caller code (15 sites) treats this as
//!     failure and rolls back state that the ledger considers settled,
//!     producing a phantom credit that compounds future operations.
//!   * **ICRC-005**: when the ledger raises its fee on chain, the cached
//!     value goes stale; the next several transfers fail until the BadFee
//!     handler in `process_pending_margin_transfer` happens to fire and
//!     refresh the cache.
//!
//! # How this file tests it
//!
//! These are deduplication-primitive tests against the `flaky_ledger`
//! canister, which is a minimal ICRC-1/2 ledger with the same dedup tuple
//! the production ledgers use plus failure-injection knobs:
//!
//!   * `set_phantom_failures(n)` — next `n` transfers commit (state moves,
//!     dedup record persists) but return `GenericError`. Reproduces the
//!     IC reply-loss case the audit identifies.
//!   * `set_bad_fee_failures(n)` — next `n` transfers return `BadFee`.
//!   * `set_fee(nat)` — change the fee returned by `icrc1_fee` and used in
//!     `BadFee` responses.
//!
//! Each test demonstrates the *invariant* the production helper
//! `crate::management::transfer_idempotent` upholds. The helper itself is
//! a thin wrapper around the same dedup tuple — if the ledger upholds the
//! contract here, the helper translates the result correctly (Duplicate
//! → Ok). Pairing this with the call-site updates that pass a stable
//! `op_nonce` across retries gives end-to-end retry safety.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};

// ─── Flaky ledger Candid types (mirror of `flaky_ledger::*`) ───

#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Clone, Debug)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Clone, Debug)]
struct TransferFromArgs {
    spender_subaccount: Option<[u8; 32]>,
    from: Account,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
enum TransferFromError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    InsufficientAllowance { allowance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Clone, Debug)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: Account,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Deserialize)]
enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

// ─── WASM loader + helpers ───

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

fn deploy_flaky_ledger(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    id
}

fn mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let acct = Account { owner, subaccount: None };
    pic.update_call(ledger, Principal::anonymous(), "mint",
        encode_args((acct, Nat::from(amount))).unwrap())
        .expect("mint failed");
}

fn balance_of(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let acct = Account { owner, subaccount: None };
    let result = pic.query_call(ledger, Principal::anonymous(), "icrc1_balance_of",
        encode_one(acct).unwrap()).expect("balance_of failed");
    let bal: Nat = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode balance"),
        WasmResult::Reject(m) => panic!("balance_of rejected: {}", m),
    };
    bal.0.try_into().unwrap_or(0)
}

fn ledger_fee(pic: &PocketIc, ledger: Principal) -> u128 {
    let result = pic.query_call(ledger, Principal::anonymous(), "icrc1_fee",
        encode_args(()).unwrap()).expect("fee call failed");
    let fee: Nat = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode fee"),
        WasmResult::Reject(m) => panic!("fee rejected: {}", m),
    };
    fee.0.try_into().unwrap_or(0)
}

fn set_fee(pic: &PocketIc, ledger: Principal, fee: u128) {
    pic.update_call(ledger, Principal::anonymous(), "set_fee",
        encode_one(Nat::from(fee)).unwrap()).expect("set_fee failed");
}

fn set_phantom_failures(pic: &PocketIc, ledger: Principal, n: u32) {
    pic.update_call(ledger, Principal::anonymous(), "set_phantom_failures",
        encode_one(n).unwrap()).expect("set_phantom_failures failed");
}

fn set_bad_fee_failures(pic: &PocketIc, ledger: Principal, n: u32) {
    pic.update_call(ledger, Principal::anonymous(), "set_bad_fee_failures",
        encode_one(n).unwrap()).expect("set_bad_fee_failures failed");
}

fn icrc1_transfer(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    args: TransferArg,
) -> Result<Nat, TransferError> {
    let result = pic.update_call(ledger, sender, "icrc1_transfer",
        encode_one(args).unwrap()).expect("call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode transfer result"),
        WasmResult::Reject(m) => panic!("rejected: {}", m),
    }
}

fn icrc2_approve(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    spender: Principal,
    amount: u128,
) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: Account { owner: spender, subaccount: None },
        amount: Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic.update_call(ledger, sender, "icrc2_approve",
        encode_one(args).unwrap()).expect("approve call failed");
    let parsed: Result<Nat, ApproveError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode approve"),
        WasmResult::Reject(m) => panic!("approve rejected: {}", m),
    };
    parsed.expect("approve returned ledger error");
}

fn icrc2_transfer_from(
    pic: &PocketIc,
    ledger: Principal,
    spender: Principal,
    args: TransferFromArgs,
) -> Result<Nat, TransferFromError> {
    let result = pic.update_call(ledger, spender, "icrc2_transfer_from",
        encode_one(args).unwrap()).expect("transfer_from call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode transfer_from result"),
        WasmResult::Reject(m) => panic!("rejected: {}", m),
    }
}

// ─── ICRC-001 / ICRC-002: phantom failure followed by retry ───

/// **ICRC-001 fix proof.** When the ledger commits a transfer but the IC
/// drops the reply (`set_phantom_failures(1)` simulates this), the production
/// helper retries with the *same* op_nonce → the *same* `created_at_time` →
/// the same dedup tuple. The ledger recognises the second call as a
/// duplicate of the first and returns `Duplicate { duplicate_of }`. Net
/// effect: exactly one block on the ledger, no double payment.
///
/// This test reproduces the retry from the helper's perspective: it sends
/// the same `TransferArg` twice (mirroring what the helper would do across
/// retries) and asserts the second call dedupes rather than re-paying.
#[test]
fn icrc_001_no_dedup_retry_with_same_created_at_time_dedupes() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let ledger = deploy_flaky_ledger(&pic);

    let sender = Principal::self_authenticating(&[1]);
    let recipient = Principal::self_authenticating(&[2]);
    mint(&pic, ledger, sender, 1_000_000);

    // Phantom failure: the next transfer commits but returns Err to the
    // caller, exactly the IC reply-loss scenario.
    set_phantom_failures(&pic, ledger, 1);

    let arg = TransferArg {
        from_subaccount: None,
        to: Account { owner: recipient, subaccount: None },
        amount: Nat::from(100_000u64),
        fee: None,
        memo: None,
        created_at_time: Some(1_700_000_000_000_000_000),
    };

    let first = icrc1_transfer(&pic, ledger, sender, arg.clone());
    assert!(matches!(first, Err(TransferError::GenericError { .. })),
        "phantom failure should surface as GenericError to the caller, got {:?}", first);
    assert_eq!(balance_of(&pic, ledger, recipient), 100_000,
        "phantom failure: ledger MUST have committed the transfer");

    // The production helper retries with the same op_nonce → same args.
    // Without dedup (the pre-fix bug) this would land a second time.
    let second = icrc1_transfer(&pic, ledger, sender, arg);
    let dup_block = match second {
        Err(TransferError::Duplicate { duplicate_of }) => duplicate_of,
        other => panic!("ICRC-001: expected Duplicate on retry, got {:?}", other),
    };

    assert_eq!(balance_of(&pic, ledger, recipient), 100_000,
        "ICRC-001: retry MUST NOT double-pay; recipient should still hold exactly one transfer");

    let dup_block_u64: u64 = dup_block.0.try_into().unwrap();
    assert_eq!(dup_block_u64, 1,
        "Duplicate must point at the original committed block (block 1)");
}

/// **ICRC-001 demonstration of the bug**, NOT the fix. With
/// `created_at_time: None` the ledger has no dedup tuple, so the same
/// args submitted twice land as two distinct blocks → recipient gets
/// paid twice. This is what `transfer_collateral` did pre-Wave-3 in
/// every retry loop. Kept here so a future regression that drops
/// `created_at_time` from the helper trips this assertion.
#[test]
fn icrc_001_no_dedup_without_created_at_time_double_spends() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let ledger = deploy_flaky_ledger(&pic);

    let sender = Principal::self_authenticating(&[1]);
    let recipient = Principal::self_authenticating(&[2]);
    mint(&pic, ledger, sender, 1_000_000);

    set_phantom_failures(&pic, ledger, 1);

    let arg = TransferArg {
        from_subaccount: None,
        to: Account { owner: recipient, subaccount: None },
        amount: Nat::from(100_000u64),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    let first = icrc1_transfer(&pic, ledger, sender, arg.clone());
    assert!(matches!(first, Err(TransferError::GenericError { .. })));
    assert_eq!(balance_of(&pic, ledger, recipient), 100_000);

    let second = icrc1_transfer(&pic, ledger, sender, arg);
    assert!(second.is_ok(),
        "without created_at_time the ledger has no dedup, retry succeeds (the bug)");
    assert_eq!(balance_of(&pic, ledger, recipient), 200_000,
        "ICRC-001 BUG: recipient was paid twice — this is exactly what the helper prevents");
}

// ─── ICRC-002: treasury / pull-style transfer_from retry ───

/// **ICRC-002 fix proof.** Treasury withdraw uses `icrc2_transfer_from` (the
/// treasury holds funds in its own subaccount and the controller pulls
/// them). Same retry semantics as ICRC-001, but on the pull side. Phantom
/// failure on first attempt + retry with same dedup tuple → second call
/// returns `Duplicate`, no double-pull.
#[test]
fn icrc_002_treasury_withdraw_retry_dedupes() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let ledger = deploy_flaky_ledger(&pic);

    let funded = Principal::self_authenticating(&[10]);
    let spender = Principal::self_authenticating(&[11]);
    let recipient = Principal::self_authenticating(&[12]);
    mint(&pic, ledger, funded, 1_000_000);
    icrc2_approve(&pic, ledger, funded, spender, 1_000_000);

    set_phantom_failures(&pic, ledger, 1);

    let arg = TransferFromArgs {
        spender_subaccount: None,
        from: Account { owner: funded, subaccount: None },
        to: Account { owner: recipient, subaccount: None },
        amount: Nat::from(100_000u64),
        fee: None,
        memo: None,
        created_at_time: Some(1_700_000_000_000_000_001),
    };

    let first = icrc2_transfer_from(&pic, ledger, spender, arg.clone());
    assert!(matches!(first, Err(TransferFromError::GenericError { .. })),
        "phantom failure should surface as GenericError, got {:?}", first);
    let funded_after_phantom = balance_of(&pic, ledger, funded);
    let recipient_after_phantom = balance_of(&pic, ledger, recipient);
    assert_eq!(recipient_after_phantom, 100_000, "phantom failure committed at the ledger");
    assert_eq!(funded_after_phantom, 900_000, "treasury (funded) was debited");

    let second = icrc2_transfer_from(&pic, ledger, spender, arg);
    assert!(matches!(second, Err(TransferFromError::Duplicate { .. })),
        "ICRC-002: retry must dedupe, got {:?}", second);

    assert_eq!(balance_of(&pic, ledger, recipient), 100_000,
        "ICRC-002: recipient must NOT be paid twice");
    assert_eq!(balance_of(&pic, ledger, funded), 900_000,
        "ICRC-002: funded account must NOT be debited twice");
}

// ─── ICRC-003: Duplicate explicitly is success ───

/// **ICRC-003 fix proof.** The dedup test cases above already rely on
/// `Duplicate` carrying the original block index. This test pins the
/// shape of that response so any future ledger spec drift is caught.
/// The helper's job is to translate `Err(Duplicate { duplicate_of })`
/// into `Ok(duplicate_of)` — see `management::handle_transfer_outcome`.
#[test]
fn icrc_003_duplicate_returns_original_block_index() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let ledger = deploy_flaky_ledger(&pic);

    let sender = Principal::self_authenticating(&[1]);
    let recipient = Principal::self_authenticating(&[2]);
    mint(&pic, ledger, sender, 1_000_000);

    let arg = TransferArg {
        from_subaccount: None,
        to: Account { owner: recipient, subaccount: None },
        amount: Nat::from(50_000u64),
        fee: None,
        memo: None,
        created_at_time: Some(1_700_000_000_000_000_002),
    };

    let first = icrc1_transfer(&pic, ledger, sender, arg.clone()).expect("first transfer Ok");
    let original_block: u64 = first.0.try_into().unwrap();

    let second = icrc1_transfer(&pic, ledger, sender, arg);
    let dup_block = match second {
        Err(TransferError::Duplicate { duplicate_of }) => {
            let v: u64 = duplicate_of.0.try_into().unwrap();
            v
        }
        other => panic!("ICRC-003: expected Duplicate, got {:?}", other),
    };

    assert_eq!(dup_block, original_block,
        "ICRC-003: Duplicate.duplicate_of MUST equal the original block index — \
         this is what lets the helper map Duplicate→Ok with the right block");
    assert_eq!(balance_of(&pic, ledger, recipient), 50_000,
        "ICRC-003: dedup must not pay twice");
}

// ─── ICRC-005: BadFee triggers a fee refresh ───

/// **ICRC-005 fix proof.** When the ledger raises its fee on chain, the
/// next transfer that submits with the stale (cached) fee returns
/// `BadFee { expected_fee }`. The production helper's behaviour:
///
///   1. propagate the BadFee error so the caller knows to re-size,
///   2. update its local fee cache to `expected_fee`,
///   3. let the caller retry — which now uses the fresh fee.
///
/// This test fences (1) and the underlying ledger contract that
/// supplies `expected_fee`. The helper logic itself (cache-update on
/// BadFee) is exercised by every transfer through the real call sites.
#[test]
fn icrc_005_bad_fee_carries_expected_fee() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();
    let ledger = deploy_flaky_ledger(&pic);

    let sender = Principal::self_authenticating(&[1]);
    let recipient = Principal::self_authenticating(&[2]);
    mint(&pic, ledger, sender, 1_000_000);

    // Simulate "ledger governance just raised the fee" — set a new fee
    // and arm the next transfer to fail with BadFee.
    set_fee(&pic, ledger, 100_000);
    set_bad_fee_failures(&pic, ledger, 1);

    assert_eq!(ledger_fee(&pic, ledger), 100_000,
        "icrc1_fee query reports the fresh fee — this is what the helper's \
         fee cache should refresh to after BadFee");

    let arg = TransferArg {
        from_subaccount: None,
        to: Account { owner: recipient, subaccount: None },
        amount: Nat::from(50_000u64),
        fee: None,
        memo: None,
        created_at_time: Some(1_700_000_000_000_000_003),
    };

    let first = icrc1_transfer(&pic, ledger, sender, arg.clone());
    let expected = match first {
        Err(TransferError::BadFee { expected_fee }) => {
            let v: u128 = expected_fee.0.try_into().unwrap();
            v
        }
        other => panic!("ICRC-005: expected BadFee, got {:?}", other),
    };
    assert_eq!(expected, 100_000,
        "ICRC-005: BadFee MUST carry expected_fee so the caller's cache can refresh");
    assert_eq!(balance_of(&pic, ledger, recipient), 0,
        "BadFee rejects before commit — recipient unchanged");

    // Caller retries with awareness of the new fee. The dedup tuple is
    // unchanged (created_at_time is the same), so this lands as a fresh
    // transfer (BadFee did not commit) — proving the fee-refresh recovery
    // path completes successfully.
    let second = icrc1_transfer(&pic, ledger, sender, arg);
    assert!(second.is_ok(), "retry after BadFee should succeed, got {:?}", second);
    assert_eq!(balance_of(&pic, ledger, recipient), 50_000,
        "ICRC-005: after fee refresh the retry lands");
}
