use candid::{CandidType, Deserialize, Nat, Principal};
use std::cell::Cell;
thread_local! { static HOLD: Cell<bool> = const { Cell::new(false) }; static CALLS: Cell<u64> = const { Cell::new(0) }; static COMPLETED: Cell<u64> = const { Cell::new(0) }; }
#[ic_cdk::update]
fn set_hold(v: bool) {
    HOLD.with(|h| h.set(v));
}
#[ic_cdk::query]
fn calls() -> u64 {
    CALLS.with(Cell::get)
}
#[ic_cdk::query]
fn completed() -> u64 {
    COMPLETED.with(Cell::get)
}

#[derive(CandidType)]
struct Rate {
    timestamp_seconds: u64,
    xdr_permyriad_per_icp: u64,
}
#[derive(CandidType)]
struct RateResponse {
    data: Rate,
}
#[ic_cdk::update]
fn get_icp_xdr_conversion_rate() -> RateResponse {
    RateResponse {
        data: Rate {
            timestamp_seconds: ic_cdk::api::time() / 1_000_000_000,
            xdr_permyriad_per_icp: 40_000,
        },
    }
}
#[derive(CandidType, Deserialize)]
struct DepositSolArgs {
    owner: Option<Principal>,
    subaccount: Option<[u8; 32]>,
}
#[derive(CandidType, Deserialize)]
struct InsufficientCyclesError {
    expected: Nat,
    received: Nat,
}
#[derive(CandidType, Deserialize)]
struct ValueTooSmallError {
    balance: u64,
    minimum_deposit_amount: u64,
}
#[derive(CandidType, Deserialize)]
enum DepositSolError {
    InsufficientCycles(InsufficientCyclesError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    ValueTooSmall(ValueTooSmallError),
    Quarantined { deposit_id: u64 },
}
#[ic_cdk::update]
async fn deposit_sol(_args: DepositSolArgs) -> Result<u64, DepositSolError> {
    CALLS.with(|c| c.set(c.get() + 1));
    ic_cdk::api::call::msg_cycles_accept128(100_000_000_000);
    if HOLD.with(Cell::get) {
        let _: ((),) = ic_cdk::call(ic_cdk::id(), "hold_reply", (20u32,))
            .await
            .unwrap();
    }
    COMPLETED.with(|c| c.set(c.get() + 1));
    Ok(42)
}

// Keep a genuine inter-canister callback outstanding for many replica rounds.
#[ic_cdk::update]
async fn hold_reply(depth: u32) {
    if depth > 0 {
        let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
        let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
        let target = if ic_cdk::id() == cmc { minter } else { cmc };
        let _: ((),) = ic_cdk::call(target, "hold_reply", (depth - 1,))
            .await
            .unwrap();
    }
}

// A minimal local ledger for exercising an accepted refund with a delayed reply.
#[derive(Clone, Debug, CandidType, Deserialize, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}
#[derive(Clone, Debug, CandidType, Deserialize, PartialEq, Eq)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    fee: Option<Nat>,
    created_at_time: Option<u64>,
    memo: Option<Vec<u8>>,
    amount: Nat,
}
#[derive(CandidType, Deserialize)]
struct TransferFromArg {
    spender_subaccount: Option<[u8; 32]>,
    from: Account,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}
#[derive(CandidType, Deserialize)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    TemporarilyUnavailable,
    Duplicate { duplicate_of: Nat },
    GenericError { error_code: Nat, message: String },
}
#[derive(CandidType, Deserialize)]
enum TransferFromError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    InsufficientAllowance { allowance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    TemporarilyUnavailable,
    Duplicate { duplicate_of: Nat },
    GenericError { error_code: Nat, message: String },
}
thread_local! { static REFUND: std::cell::RefCell<Option<TransferArg>> = const { std::cell::RefCell::new(None) }; static REFUND_CALLS: Cell<u64> = const { Cell::new(0) }; static REFUND_COMPLETED: Cell<u64> = const { Cell::new(0) }; }
#[ic_cdk::update]
fn icrc1_fee() -> Nat {
    Nat::from(10_000u64)
}
#[ic_cdk::update]
fn icrc2_transfer_from(_args: TransferFromArg) -> Result<Nat, TransferFromError> {
    Ok(Nat::from(1u8))
}
#[ic_cdk::query]
fn refund_calls() -> u64 {
    REFUND_CALLS.with(Cell::get)
}
#[ic_cdk::query]
fn refund_completed() -> u64 {
    REFUND_COMPLETED.with(Cell::get)
}
#[ic_cdk::update]
async fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
    REFUND_CALLS.with(|c| c.set(c.get() + 1));
    let prior = REFUND.with(|r| r.borrow().clone());
    if let Some(prior) = prior {
        return if prior == args {
            Err(TransferError::Duplicate {
                duplicate_of: Nat::from(2u8),
            })
        } else {
            Err(TransferError::GenericError {
                error_code: Nat::from(9u8),
                message: "refund tuple changed".into(),
            })
        };
    }
    REFUND.with(|r| *r.borrow_mut() = Some(args));
    if HOLD.with(Cell::get) {
        let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
        let _: ((),) = ic_cdk::call(minter, "hold_reply", (20u32,)).await.unwrap();
    }
    REFUND_COMPLETED.with(|c| c.set(c.get() + 1));
    Ok(Nat::from(2u8))
}
