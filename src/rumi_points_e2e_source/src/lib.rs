//! Test-only mock of `rumi_protocol_backend.get_events_forward_filtered` for the
//! rumi_points ingestion E2E. Its response types match the backend `.did`
//! structurally, so rumi_points' real poll path (inter-canister call -> candid
//! decode -> normalize -> auto-register -> cursor advance) is exercised exactly as
//! it would be against the real backend. The 95-variant superset-decode case is
//! covered separately by the canary tests in `source_types.rs`.
//!
//! Behavior: the first forward window (start == 0) returns one synthetic
//! `borrow_from_vault` for `ryjl3-tyaaa-aaaaa-aaaba-cai` at an in-season
//! timestamp, with `next_start = 1, reached_end = true`. Subsequent windows are
//! empty (caught up).

use candid::{CandidType, Principal};
use serde::Deserialize;
use std::cell::RefCell;

/// The subset of the backend's `EventTypeFilter` that rumi_points sends. We only
/// need to DECODE it (the mock ignores the filter), and the values rumi_points
/// sends are all among these.
#[derive(CandidType, Deserialize, Clone)]
enum EventTypeFilter {
    OpenVault,
    CloseVault,
    Borrow,
    Repay,
    Liquidation,
    PartialLiquidation,
    Redemption,
    StabilityPoolDeposit,
    StabilityPoolWithdraw,
}

/// A faithful subset of the backend `Event` (snake_case candid labels via serde
/// rename, matching the real enum). Only the variants this mock emits.
#[derive(CandidType, Deserialize, Clone)]
enum Event {
    #[serde(rename = "borrow_from_vault")]
    BorrowFromVault {
        block_index: u64,
        vault_id: u64,
        timestamp: Option<u64>,
        fee_amount: u64,
        caller: Option<Principal>,
        borrowed_amount: u64,
    },
    #[serde(rename = "close_vault")]
    CloseVault {
        vault_id: u64,
        block_index: Option<u64>,
        timestamp: Option<u64>,
    },
}

#[derive(CandidType, Deserialize, Clone)]
struct ForwardFilteredEventsResponse {
    events: Vec<(u64, Event)>,
    next_start: u64,
    reached_end: bool,
}

/// Within the rumi_points default season window (June 1 .. Aug 31 2026).
const IN_SEASON_TS_NS: u64 = 1_780_300_000_000_000_000;

#[ic_cdk::query]
fn get_events_forward_filtered(
    start: u64,
    _max_scan: u64,
    _types: Option<Vec<EventTypeFilter>>,
) -> ForwardFilteredEventsResponse {
    if start == 0 {
        let caller = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
        ForwardFilteredEventsResponse {
            events: vec![(
                0,
                Event::BorrowFromVault {
                    block_index: 0,
                    vault_id: 1,
                    timestamp: Some(IN_SEASON_TS_NS),
                    fee_amount: 0,
                    caller: Some(caller),
                    borrowed_amount: 1_000,
                },
            )],
            next_start: 1,
            reached_end: true,
        }
    } else {
        ForwardFilteredEventsResponse {
            events: vec![],
            next_start: start,
            reached_end: true,
        }
    }
}

// ── Balance-query mocks (Phase 5 accrual E2E) ───────────────────────────────
// The snapshot capture queries all four sources for a principal's current
// balances. This single mock answers every one. Only the vault debt is settable
// (so the test can vary a position between snapshots to exercise the min()); the
// other venues return empty, isolating the accrual to a single contribution.

thread_local! {
    /// Test-controlled `(owner, debt_e8s)` returned by `get_vaults`.
    static VAULT_DEBT: RefCell<Option<(Principal, u64)>> = const { RefCell::new(None) };
}

/// Superset of the backend `CandidVault` (rumi_points mirrors only `borrowed_icusd_amount`).
#[derive(CandidType, Deserialize, Clone)]
struct CandidVault {
    owner: Principal,
    vault_id: u64,
    borrowed_icusd_amount: u64,
}

#[derive(CandidType, Deserialize, Clone)]
struct ProtocolStatus {
    last_icp_rate: f64,
    last_icp_timestamp: u64,
}

#[derive(CandidType, Deserialize, Clone)]
struct PoolStatus {
    balances: [u128; 3],
    lp_total_supply: u128,
    virtual_price: u128,
}

#[derive(CandidType, Deserialize, Clone)]
struct Account {
    owner: Principal,
    subaccount: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone)]
struct UserStabilityPosition {
    stablecoin_balances: Vec<(Principal, u64)>,
}

#[derive(CandidType, Deserialize, Clone)]
struct PoolInfo {
    pool_id: String,
    token_a: Principal,
    token_b: Principal,
    reserve_a: u128,
    reserve_b: u128,
    total_lp_shares: u128,
}

/// Test control: set the vault debt returned for `owner`.
#[ic_cdk::update]
fn set_vault_debt(owner: Principal, debt: u64) {
    VAULT_DEBT.with(|v| *v.borrow_mut() = Some((owner, debt)));
}

#[ic_cdk::query]
fn get_vaults(target: Option<Principal>) -> Vec<CandidVault> {
    let stored = VAULT_DEBT.with(|v| v.borrow().clone());
    match (stored, target) {
        (Some((owner, debt)), Some(t)) if owner == t => vec![CandidVault {
            owner,
            vault_id: 1,
            borrowed_icusd_amount: debt,
        }],
        _ => vec![],
    }
}

#[ic_cdk::query]
fn get_protocol_status() -> ProtocolStatus {
    ProtocolStatus { last_icp_rate: 1.0, last_icp_timestamp: 0 }
}

#[ic_cdk::query]
fn get_pool_status() -> PoolStatus {
    PoolStatus {
        balances: [0, 0, 0],
        lp_total_supply: 0,
        virtual_price: 1_000_000_000_000_000_000, // vp 1.0
    }
}

#[ic_cdk::query]
fn icrc1_balance_of(_account: Account) -> candid::Nat {
    candid::Nat::from(0u64)
}

#[ic_cdk::query]
fn get_user_position(_target: Option<Principal>) -> Option<UserStabilityPosition> {
    None
}

#[ic_cdk::query]
fn get_pools() -> Vec<PoolInfo> {
    vec![]
}

#[ic_cdk::query]
fn get_lp_balance(_pool_id: String, _user: Principal) -> u128 {
    0
}
