//! Test-only inter-canister fixtures for Cycle Sentinel's PocketIC suite.
//!
//! The fixture is deliberately one small Wasm package with a role selected at
//! install time.  It is never listed in `dfx.json`/`icp.yaml` and is not part
//! of the production canister build.  Each role implements only the wire
//! surface that the Sentinel adapter calls:
//!
//! * `SelfReport`: `cycles_status`;
//! * `Blackhole`: the pinned relay's `canister_status` projection;
//! * `CyclesLedger`: balance, fee, and official `withdraw` result variants;
//! * `IcpLedger`: ICRC-1 balance/fee/transfer and `get_blocks`;
//! * `Cmc`: conversion rate and `notify_top_up`.
//!
//! Control methods are intentionally unauthenticated because this canister
//! exists only inside a test replica.  Tests install it at fixed protocol
//! principals and use the controls to model retry, deduplication, and reply
//! outcomes without ever touching a live ledger.

use candid::{CandidType, Int, Nat, Principal};
use ic_cdk::{init, query, update};
use rumi_cycle_manager::CycleManagerCyclesStatus;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;

// ─────────────────────────── Public test configuration ───────────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockRole {
    SelfReport,
    Blackhole,
    CyclesLedger,
    IcpLedger,
    Cmc,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawMode {
    Confirmed,
    /// Records the exact withdrawal but returns an indeterminate reply on
    /// the first call. A byte-identical retry is then answered by the mock's
    /// dedup table as `Duplicate`, modeling committed delivery with reply
    /// loss without performing a second delivery.
    CommittedLostReply,
    Unknown,
    Duplicate,
    TooOld,
    TerminalNoSpend,
    FeeDebited,
    FullAmountDebited,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferMode {
    Confirmed,
    Unknown,
    Duplicate,
    TooOld,
    BadFee,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyMode {
    Completed,
    Processing,
    Refunded,
    Invalid,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MockInit {
    pub role: MockRole,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MockConfig {
    pub cycles_balance: Nat,
    pub self_report_reject: bool,
    pub self_report_healthy: bool,
    pub self_report_low_watermark: Nat,
    pub target_status: TargetStatus,
    pub ledger_balance: Nat,
    pub fee: Nat,
    pub withdraw_mode: WithdrawMode,
    pub transfer_mode: TransferMode,
    pub rate_timestamp_seconds: u64,
    pub rate_xdr_permyriad_per_icp: u64,
    pub notify_mode: NotifyMode,
    pub refund_block_index: Option<u64>,
    pub refund_block: Option<IcpLedgerValue>,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetStatusKind {
    Running,
    Stopping,
    Stopped,
    Uninstalled,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetStatus {
    pub kind: TargetStatusKind,
    pub cycles: Nat,
    pub module_hash: Option<Vec<u8>>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            cycles_balance: Nat::from(1_000_000_000_000_000u128),
            self_report_reject: false,
            self_report_healthy: true,
            self_report_low_watermark: Nat::from(1_000u64),
            target_status: TargetStatus {
                kind: TargetStatusKind::Running,
                cycles: Nat::from(1_000_000_000_000_000u128),
                module_hash: Some(vec![7; 32]),
            },
            ledger_balance: Nat::from(1_000_000_000_000_000u128),
            fee: Nat::from(1_000u64),
            withdraw_mode: WithdrawMode::Confirmed,
            transfer_mode: TransferMode::Confirmed,
            rate_timestamp_seconds: 1,
            rate_xdr_permyriad_per_icp: 10_000,
            notify_mode: NotifyMode::Completed,
            refund_block_index: None,
            refund_block: None,
        }
    }
}

#[derive(Clone, Debug)]
struct State {
    role: MockRole,
    config: MockConfig,
    next_block: u64,
    withdraw_calls: u64,
    transfer_calls: u64,
    notify_calls: u64,
    withdraw_dedup: BTreeMap<WithdrawKey, u64>,
    transfer_dedup: BTreeMap<TransferKey, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WithdrawKey {
    caller: Principal,
    amount: u128,
    from_subaccount: Option<Vec<u8>>,
    to: Principal,
    created_at_time: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TransferKey {
    caller: Principal,
    amount: u128,
    from_subaccount: Option<Vec<u8>>,
    to_owner: Principal,
    to_subaccount: Option<Vec<u8>>,
    fee: Option<u128>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|state| {
        f(state
            .borrow()
            .as_ref()
            .expect("mock canister is initialized"))
    })
}

fn with_state_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|state| {
        f(state
            .borrow_mut()
            .as_mut()
            .expect("mock canister is initialized"))
    })
}

fn require_role(expected: MockRole) {
    with_state(|state| {
        assert_eq!(
            state.role, expected,
            "fixture endpoint called for wrong role"
        )
    });
}

fn require_ledger_role() {
    with_state(|state| {
        assert!(
            matches!(state.role, MockRole::CyclesLedger | MockRole::IcpLedger),
            "fixture endpoint called for non-ledger role"
        );
    });
}

fn nat_u128(value: &Nat) -> u128 {
    value.0.clone().try_into().unwrap_or(u128::MAX)
}

#[init]
fn init(args: MockInit) {
    STATE.with(|state| {
        *state.borrow_mut() = Some(State {
            role: args.role,
            config: MockConfig::default(),
            next_block: 0,
            withdraw_calls: 0,
            transfer_calls: 0,
            notify_calls: 0,
            withdraw_dedup: BTreeMap::new(),
            transfer_dedup: BTreeMap::new(),
        });
    });
}

// ─────────────────────────── Test controls ───────────────────────────

#[update]
fn set_config(config: MockConfig) {
    with_state_mut(|state| state.config = config);
}

#[update]
fn set_withdraw_mode(mode: WithdrawMode) {
    with_state_mut(|state| state.config.withdraw_mode = mode);
}

#[update]
fn set_transfer_mode(mode: TransferMode) {
    with_state_mut(|state| state.config.transfer_mode = mode);
}

#[update]
fn set_notify_mode(mode: NotifyMode) {
    with_state_mut(|state| state.config.notify_mode = mode);
}

#[update]
fn set_refund_block(block_index: Option<u64>, block: Option<IcpLedgerValue>) {
    require_role(MockRole::IcpLedger);
    with_state_mut(|state| {
        state.config.refund_block_index = block_index;
        state.config.refund_block = block;
    });
}

#[update]
fn set_cycles_balance(balance: Nat) {
    with_state_mut(|state| state.config.cycles_balance = balance);
}

#[update]
fn set_self_report_reject(reject: bool) {
    with_state_mut(|state| state.config.self_report_reject = reject);
}

#[update]
fn set_target_status(status: TargetStatus) {
    with_state_mut(|state| state.config.target_status = status);
}

#[update]
fn set_ledger_balance(balance: Nat) {
    with_state_mut(|state| state.config.ledger_balance = balance);
}

#[update]
fn set_fee(fee: Nat) {
    with_state_mut(|state| state.config.fee = fee);
}

#[update]
fn set_rate(timestamp_seconds: u64, xdr_permyriad_per_icp: u64) {
    with_state_mut(|state| {
        state.config.rate_timestamp_seconds = timestamp_seconds;
        state.config.rate_xdr_permyriad_per_icp = xdr_permyriad_per_icp;
    });
}

#[update]
fn reset_dedup() {
    with_state_mut(|state| {
        state.withdraw_dedup.clear();
        state.transfer_dedup.clear();
    });
}

#[query]
fn withdraw_call_count() -> u64 {
    with_state(|state| state.withdraw_calls)
}

#[query]
fn transfer_call_count() -> u64 {
    with_state(|state| state.transfer_calls)
}

#[query]
fn notify_call_count() -> u64 {
    with_state(|state| state.notify_calls)
}

// ─────────────────────────── Self-report role ───────────────────────────

#[query]
fn cycles_status() -> CycleManagerCyclesStatus {
    require_role(MockRole::SelfReport);
    if with_state(|state| state.config.self_report_reject) {
        ic_cdk::trap("fixture self-report rejection");
    }
    with_state(|state| CycleManagerCyclesStatus {
        balance: state.config.cycles_balance.clone(),
        low_watermark: state.config.self_report_low_watermark.clone(),
        healthy: state.config.self_report_healthy,
        freeze_threshold_secs: 2_592_000,
        stable_memory_bytes: None,
        heap_memory_bytes: None,
        idle_burn_cycles_per_day: None,
    })
}

// ─────────────────────────── Blackhole role ───────────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlackholeRequest {
    pub canister_id: Principal,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanisterState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopping")]
    Stopping,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BlackholeSettings {
    pub controllers: Vec<Principal>,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BlackholeStatus {
    pub status: CanisterState,
    pub settings: BlackholeSettings,
    pub module_hash: Option<Vec<u8>>,
    pub cycles: Nat,
}

const PINNED_BLACKHOLE_HASH: [u8; 32] = [
    0x21, 0x0c, 0xf9, 0x41, 0xe5, 0xca, 0x77, 0xda, 0xac, 0x31, 0x4a, 0x91, 0x51, 0x74, 0x83, 0xac,
    0x17, 0x12, 0x64, 0x52, 0x7e, 0x3d, 0x0d, 0x71, 0x3b, 0x92, 0xbb, 0x95, 0x23, 0x9d, 0x7d, 0xe0,
];

#[update]
fn canister_status(request: BlackholeRequest) -> BlackholeStatus {
    require_role(MockRole::Blackhole);
    with_state(|state| {
        if request.canister_id == ic_cdk::id() {
            return BlackholeStatus {
                status: CanisterState::Running,
                settings: BlackholeSettings {
                    controllers: vec![ic_cdk::id()],
                },
                module_hash: Some(PINNED_BLACKHOLE_HASH.to_vec()),
                cycles: Nat::from(1_000_000_000_000_000u128),
            };
        }
        let target = &state.config.target_status;
        let status = match target.kind {
            TargetStatusKind::Running => CanisterState::Running,
            TargetStatusKind::Stopping => CanisterState::Stopping,
            TargetStatusKind::Stopped => CanisterState::Stopped,
            TargetStatusKind::Uninstalled => CanisterState::Running,
        };
        BlackholeStatus {
            status,
            settings: BlackholeSettings {
                controllers: vec![ic_cdk::id()],
            },
            module_hash: target.module_hash.clone(),
            cycles: target.cycles.clone(),
        }
    })
}

// ─────────────────────────── Cycles Ledger role ───────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CyclesAccount {
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

fn next_block(state: &mut State) -> Nat {
    let block = state.next_block;
    state.next_block = state.next_block.saturating_add(1);
    Nat::from(block)
}

#[update]
fn withdraw(args: WithdrawArgs) -> Result<Nat, WithdrawError> {
    require_role(MockRole::CyclesLedger);
    with_state_mut(|state| {
        state.withdraw_calls = state.withdraw_calls.saturating_add(1);
        let key = WithdrawKey {
            caller: ic_cdk::caller(),
            amount: nat_u128(&args.amount),
            from_subaccount: args.from_subaccount.clone(),
            to: args.to,
            created_at_time: args.created_at_time,
        };
        if let Some(block) = state.withdraw_dedup.get(&key) {
            return Err(WithdrawError::Duplicate {
                duplicate_of: Nat::from(*block),
            });
        }
        match state.config.withdraw_mode {
            WithdrawMode::Confirmed => {
                let block = next_block(state);
                state.withdraw_dedup.insert(key, nat_u128(&block) as u64);
                Ok(block)
            }
            WithdrawMode::CommittedLostReply => {
                let block = next_block(state);
                state.withdraw_dedup.insert(key, nat_u128(&block) as u64);
                Err(WithdrawError::TemporarilyUnavailable)
            }
            WithdrawMode::Unknown => Err(WithdrawError::TemporarilyUnavailable),
            WithdrawMode::Duplicate => Err(WithdrawError::Duplicate {
                duplicate_of: Nat::from(777u64),
            }),
            WithdrawMode::TooOld => Err(WithdrawError::TooOld),
            WithdrawMode::TerminalNoSpend => Err(WithdrawError::BadFee {
                expected_fee: state.config.fee.clone(),
            }),
            WithdrawMode::FeeDebited => Err(WithdrawError::FailedToWithdraw {
                fee_block: Some(next_block(state)),
                rejection_code: RejectionCode::CanisterReject,
                rejection_reason: "fixture fee debit".to_string(),
            }),
            WithdrawMode::FullAmountDebited => Err(WithdrawError::FailedToWithdraw {
                fee_block: None,
                rejection_code: RejectionCode::CanisterReject,
                rejection_reason: "fixture full debit".to_string(),
            }),
        }
    })
}

#[query]
fn withdraw_delivery_count() -> u64 {
    require_role(MockRole::CyclesLedger);
    with_state(|state| state.withdraw_dedup.len() as u64)
}

#[query]
fn icrc1_balance_of(_account: CyclesAccount) -> Nat {
    require_ledger_role();
    with_state(|state| state.config.ledger_balance.clone())
}

#[query]
fn icrc1_fee() -> Nat {
    require_ledger_role();
    with_state(|state| state.config.fee.clone())
}

// ─────────────────────────── ICP Ledger role ───────────────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcpAccount {
    pub owner: Principal,
    pub subaccount: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TransferArg {
    pub from_subaccount: Option<Vec<u8>>,
    pub to: IcpAccount,
    pub amount: Nat,
    pub fee: Option<Nat>,
    pub memo: Option<Vec<u8>>,
    pub created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    TemporarilyUnavailable,
    Duplicate { duplicate_of: Nat },
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetBlocksArgs {
    pub start: Nat,
    pub length: Nat,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum IcpLedgerValue {
    Blob(Vec<u8>),
    Text(String),
    Nat(Nat),
    Nat64(u64),
    Int(Int),
    Array(Vec<IcpLedgerValue>),
    Map(Vec<(String, IcpLedgerValue)>),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BlockRange {
    pub blocks: Vec<IcpLedgerValue>,
}

candid::define_function!(pub QueryBlockArchiveFn : (GetBlocksArgs) -> (BlockRange) query);

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ArchivedBlockRange {
    pub start: Nat,
    pub length: Nat,
    pub callback: QueryBlockArchiveFn,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GetBlocksResponse {
    pub first_index: Nat,
    pub chain_length: u64,
    pub certificate: Option<Vec<u8>>,
    pub blocks: Vec<IcpLedgerValue>,
    pub archived_blocks: Vec<ArchivedBlockRange>,
}

#[update]
fn icrc1_transfer(args: TransferArg) -> Result<Nat, TransferError> {
    require_role(MockRole::IcpLedger);
    with_state_mut(|state| {
        state.transfer_calls = state.transfer_calls.saturating_add(1);
        let key = TransferKey {
            caller: ic_cdk::caller(),
            amount: nat_u128(&args.amount),
            from_subaccount: args.from_subaccount.clone(),
            to_owner: args.to.owner,
            to_subaccount: args.to.subaccount.clone(),
            fee: args.fee.as_ref().map(nat_u128),
            memo: args.memo.clone(),
            created_at_time: args.created_at_time,
        };
        if let Some(block) = state.transfer_dedup.get(&key) {
            return Err(TransferError::Duplicate {
                duplicate_of: Nat::from(*block),
            });
        }
        match state.config.transfer_mode {
            TransferMode::Confirmed => {
                let block = next_block(state);
                state.transfer_dedup.insert(key, nat_u128(&block) as u64);
                Ok(block)
            }
            TransferMode::Unknown => Err(TransferError::TemporarilyUnavailable),
            TransferMode::Duplicate => Err(TransferError::Duplicate {
                duplicate_of: Nat::from(778u64),
            }),
            TransferMode::TooOld => Err(TransferError::TooOld),
            TransferMode::BadFee => Err(TransferError::BadFee {
                expected_fee: state.config.fee.clone(),
            }),
        }
    })
}

#[query]
fn get_blocks(args: GetBlocksArgs) -> GetBlocksResponse {
    require_role(MockRole::IcpLedger);
    let start = nat_u128(&args.start) as u64;
    let length = nat_u128(&args.length) as u64;
    let (chain_length, block) = with_state(|state| {
        (
            state.next_block,
            state
                .transfer_dedup
                .values()
                .find(|index| **index == start)
                .map(|_| IcpLedgerValue::Map(Vec::new())),
        )
    });
    let blocks = if length > 0 {
        let configured = with_state(|state| {
            (state.config.refund_block_index == Some(start))
                .then(|| state.config.refund_block.clone())
                .flatten()
        });
        configured.or(block).into_iter().collect()
    } else {
        Vec::new()
    };
    GetBlocksResponse {
        first_index: Nat::from(start),
        chain_length,
        certificate: None,
        blocks,
        archived_blocks: Vec::new(),
    }
}

// ─────────────────────────── CMC role ───────────────────────────

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcpXdrConversionRate {
    pub timestamp_seconds: u64,
    pub xdr_permyriad_per_icp: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcpXdrConversionRateResponse {
    pub data: IcpXdrConversionRate,
    pub hash_tree: Vec<u8>,
    pub certificate: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NotifyTopUpArg {
    pub block_index: u64,
    pub canister_id: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum NotifyError {
    Refunded {
        reason: String,
        block_index: Option<u64>,
    },
    Processing,
    TransactionTooOld(u64),
    InvalidTransaction(String),
    Other {
        error_code: u64,
        error_message: String,
    },
}

#[query]
fn get_icp_xdr_conversion_rate() -> IcpXdrConversionRateResponse {
    require_role(MockRole::Cmc);
    with_state(|state| IcpXdrConversionRateResponse {
        data: IcpXdrConversionRate {
            timestamp_seconds: state.config.rate_timestamp_seconds,
            xdr_permyriad_per_icp: state.config.rate_xdr_permyriad_per_icp,
        },
        hash_tree: Vec::new(),
        certificate: Vec::new(),
    })
}

#[update]
fn notify_top_up(args: NotifyTopUpArg) -> Result<Nat, NotifyError> {
    require_role(MockRole::Cmc);
    with_state_mut(|state| {
        state.notify_calls = state.notify_calls.saturating_add(1);
        match state.config.notify_mode {
            NotifyMode::Completed => Ok(Nat::from(5_000_000_000_000u128)),
            NotifyMode::Processing => Err(NotifyError::Processing),
            NotifyMode::Refunded => Err(NotifyError::Refunded {
                reason: format!("fixture refund for {}", args.canister_id),
                block_index: Some(args.block_index.saturating_add(1)),
            }),
            NotifyMode::Invalid => Err(NotifyError::InvalidTransaction(
                "fixture invalid transaction".to_string(),
            )),
        }
    })
}
