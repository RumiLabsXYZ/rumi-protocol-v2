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
#[derive(Clone, Debug, CandidType, Deserialize, PartialEq, Eq)]
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
thread_local! {
    static FUNDING: std::cell::RefCell<Option<TransferFromArg>> = const { std::cell::RefCell::new(None) };
    static FUNDING_CALLS: Cell<u64> = const { Cell::new(0) };
    static FUNDING_COMPLETED: Cell<u64> = const { Cell::new(0) };
    static FUNDING_BALANCE: Cell<u64> = const { Cell::new(100_000_000) };
    static FUNDING_ESCROW: Cell<u64> = const { Cell::new(0) };
    static FUNDING_HOLD: Cell<bool> = const { Cell::new(false) };
}
#[ic_cdk::update]
fn set_funding_hold(v: bool) {
    FUNDING_HOLD.with(|h| h.set(v));
}
#[ic_cdk::query]
fn funding_stats() -> (u64, u64, u64, u64) {
    (
        FUNDING_CALLS.with(Cell::get),
        FUNDING_COMPLETED.with(Cell::get),
        FUNDING_BALANCE.with(Cell::get),
        FUNDING_ESCROW.with(Cell::get),
    )
}
#[ic_cdk::update]
async fn icrc2_transfer_from(args: TransferFromArg) -> Result<Nat, TransferFromError> {
    FUNDING_CALLS.with(|c| c.set(c.get() + 1));
    if let Some(previous) = FUNDING.with(|s| s.borrow().clone()) {
        return if previous == args {
            Err(TransferFromError::Duplicate {
                duplicate_of: Nat::from(1u8),
            })
        } else {
            Err(TransferFromError::GenericError {
                error_code: Nat::from(9u8),
                message: "funding tuple changed".into(),
            })
        };
    }
    let amount: u64 = args.amount.0.clone().try_into().unwrap();
    let fee: u64 = args.fee.clone().unwrap().0.try_into().unwrap();
    FUNDING_BALANCE.with(|v| v.set(v.get().checked_sub(amount + fee).unwrap()));
    FUNDING_ESCROW.with(|v| v.set(v.get() + amount));
    FUNDING.with(|v| *v.borrow_mut() = Some(args));
    if FUNDING_HOLD.with(Cell::get) {
        let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
        let _: ((),) = ic_cdk::call(minter, "hold_reply", (20u32,)).await.unwrap();
    }
    FUNDING_COMPLETED.with(|v| v.set(v.get() + 1));
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
    let amount: u64 = args.amount.0.clone().try_into().unwrap();
    let fee: u64 = args.fee.clone().unwrap().0.try_into().unwrap();
    FUNDING_ESCROW.with(|v| v.set(v.get().checked_sub(amount + fee).unwrap()));
    FUNDING_BALANCE.with(|v| v.set(v.get() + amount));
    REFUND.with(|r| *r.borrow_mut() = Some(args));
    if HOLD.with(Cell::get) {
        let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
        let _: ((),) = ic_cdk::call(minter, "hold_reply", (20u32,)).await.unwrap();
    }
    REFUND_COMPLETED.with(|c| c.set(c.get() + 1));
    Ok(Nat::from(2u8))
}

// Scripted native ICP query_blocks response. This is not an official ledger.
#[derive(CandidType, Deserialize, Clone)]
struct BlockTime {
    timestamp_nanos: u64,
}
#[derive(CandidType, Deserialize, Clone)]
struct BlockTokens {
    e8s: u64,
}
#[derive(CandidType, Deserialize, Clone)]
enum BlockOperation {
    Burn {
        from: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: BlockTokens,
    },
    Mint {
        to: Vec<u8>,
        amount: BlockTokens,
    },
    Approve {
        from: Vec<u8>,
        spender: Vec<u8>,
        allowance_e8s: i128,
        allowance: BlockTokens,
        fee: BlockTokens,
        expires_at: Option<BlockTime>,
        expected_allowance: Option<BlockTokens>,
    },
    Transfer {
        from: Vec<u8>,
        to: Vec<u8>,
        spender: Option<Vec<u8>>,
        amount: BlockTokens,
        fee: BlockTokens,
    },
}
#[derive(CandidType, Deserialize, Clone)]
struct BlockTransaction {
    memo: u64,
    icrc1_memo: Option<Vec<u8>>,
    operation: Option<BlockOperation>,
    created_at_time: BlockTime,
}
#[derive(CandidType, Deserialize, Clone)]
struct Block {
    parent_hash: Option<Vec<u8>>,
    transaction: BlockTransaction,
    timestamp: BlockTime,
}
#[derive(CandidType, Deserialize)]
struct GetBlocksArgs {
    start: u64,
    length: u64,
}
#[derive(CandidType, Deserialize)]
struct BlockRange {
    blocks: Vec<Block>,
}
#[derive(CandidType, Deserialize)]
enum QueryArchiveError {
    BadFirstBlockIndex {
        requested_index: u64,
        first_valid_index: u64,
    },
    Other {
        error_code: u64,
        error_message: String,
    },
}
candid::define_function!(ArchiveFn : (GetBlocksArgs) -> (Result<BlockRange, QueryArchiveError>) query);
#[derive(CandidType)]
struct ArchivedRange {
    start: u64,
    length: u64,
    callback: ArchiveFn,
}
#[derive(CandidType)]
struct QueryBlocksResponse {
    chain_length: u64,
    certificate: Option<Vec<u8>>,
    blocks: Vec<Block>,
    first_block_index: u64,
    archived_blocks: Vec<ArchivedRange>,
}
thread_local! { static PROOF_BLOCK: std::cell::RefCell<Option<Block>> = const { std::cell::RefCell::new(None) }; static PROOF_ARCHIVED: Cell<bool> = const { Cell::new(false) }; }
#[ic_cdk::update]
fn set_proof_block(b: Block) {
    PROOF_BLOCK.with(|v| *v.borrow_mut() = Some(b));
}
#[ic_cdk::update]
fn set_proof_archived(v: bool) {
    PROOF_ARCHIVED.with(|x| x.set(v));
}
#[ic_cdk::update]
fn query_blocks(arg: GetBlocksArgs) -> QueryBlocksResponse {
    let archived = PROOF_ARCHIVED.with(Cell::get);
    let blocks = if arg.start == 1 && !archived {
        PROOF_BLOCK.with(|v| v.borrow().clone().into_iter().collect())
    } else {
        Vec::new()
    };
    let archived_blocks = if arg.start == 1 && archived {
        vec![ArchivedRange {
            start: 1,
            length: 1,
            callback: ArchiveFn(candid::Func {
                principal: ic_cdk::id(),
                method: "archive".into(),
            }),
        }]
    } else {
        Vec::new()
    };
    QueryBlocksResponse {
        chain_length: 2,
        certificate: None,
        blocks,
        first_block_index: arg.start,
        archived_blocks,
    }
}
