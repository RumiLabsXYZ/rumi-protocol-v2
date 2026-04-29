//! EVT_* normalized event log types and StableLog instances.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_EVT_LIQUIDATIONS_IDX, MEM_EVT_LIQUIDATIONS_DATA,
    MEM_EVT_SWAPS_IDX, MEM_EVT_SWAPS_DATA,
    MEM_EVT_LIQUIDITY_IDX, MEM_EVT_LIQUIDITY_DATA,
    MEM_EVT_VAULTS_IDX, MEM_EVT_VAULTS_DATA,
    MEM_EVT_STABILITY_IDX, MEM_EVT_STABILITY_DATA,
    MEM_EVT_ADMIN_IDX, MEM_EVT_ADMIN_DATA,
};

// --- Enum types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LiquidationKind {
    Full,
    Partial,
    Redistribution,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum VaultEventKind {
    Opened,
    Borrowed,
    Repaid,
    CollateralWithdrawn,
    PartialCollateralWithdrawn,
    WithdrawAndClose,
    Closed,
    DustForgiven,
    Redeemed,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SwapSource {
    ThreePool,
    Amm,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LiquidityAction {
    Add,
    Remove,
    RemoveOneCoin,
    Donate,
}

/// Stability pool activity kind. Deposit/Withdraw affect principal balance;
/// ClaimReturns is a yield claim (ICP) that doesn't change icUSD position.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum StabilityAction {
    Deposit,
    Withdraw,
    ClaimReturns,
}

// --- Event row types ---

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsLiquidationEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub vault_id: u64,
    pub collateral_type: Principal,
    pub collateral_amount: u64,
    pub debt_amount: u64,
    pub liquidation_kind: LiquidationKind,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsVaultEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub vault_id: u64,
    pub owner: Principal,
    pub event_kind: VaultEventKind,
    pub collateral_type: Principal,
    pub amount: u64,
    /// Fee paid on this event in icUSD e8s. Populated as `Some(fee)` for
    /// Borrowed and Redeemed; `None` for other event kinds and for events
    /// stored before round 1 introduced this field. Optional so candid
    /// subtyping accepts decoding pre-round-1 stable storage entries that
    /// lack the field entirely.
    #[serde(default)]
    pub fee_amount: Option<u64>,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsSwapEvent {
    pub timestamp_ns: u64,
    pub source: SwapSource,
    pub source_event_id: u64,
    pub caller: Principal,
    pub token_in: Principal,
    pub token_out: Principal,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee: u64,
}

#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsLiquidityEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub caller: Principal,
    pub action: LiquidityAction,
    pub amounts: Vec<u64>,
    pub lp_amount: u64,
    pub coin_index: Option<u8>,
    pub fee: Option<u64>,
}

/// Mirror of backend stability-pool participation events (provide/withdraw/
/// claim). Sourced from rumi_protocol_backend via the backend-event tailer.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsStabilityEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub caller: Principal,
    pub action: StabilityAction,
    pub amount: u64,
}

/// Mirror of backend admin/setter events. Only label + timestamp are kept so
/// the log stays small; admin events are rare (a handful per week).
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsAdminEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub label: String,
}

// --- Storable impls ---

macro_rules! storable_candid {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<'_, [u8]> {
                Cow::Owned(Encode!(self).expect(concat!(stringify!($t), " encode")))
            }
            fn from_bytes(bytes: Cow<'_, [u8]>) -> Self {
                Decode!(bytes.as_ref(), Self).expect(concat!(stringify!($t), " decode"))
            }
            const BOUND: Bound = Bound::Unbounded;
        }
    };
}

storable_candid!(AnalyticsLiquidationEvent);
storable_candid!(AnalyticsVaultEvent);
storable_candid!(AnalyticsSwapEvent);
storable_candid!(AnalyticsLiquidityEvent);
storable_candid!(AnalyticsStabilityEvent);
storable_candid!(AnalyticsAdminEvent);

// --- StableLog instances ---

thread_local! {
    static EVT_LIQUIDATIONS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsLiquidationEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_LIQUIDATIONS_IDX),
                get_memory(MEM_EVT_LIQUIDATIONS_DATA),
            ).expect("init EVT_LIQUIDATIONS log")
        });

    static EVT_SWAPS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsSwapEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_SWAPS_IDX),
                get_memory(MEM_EVT_SWAPS_DATA),
            ).expect("init EVT_SWAPS log")
        });

    static EVT_LIQUIDITY_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsLiquidityEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_LIQUIDITY_IDX),
                get_memory(MEM_EVT_LIQUIDITY_DATA),
            ).expect("init EVT_LIQUIDITY log")
        });

    static EVT_VAULTS_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsVaultEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_VAULTS_IDX),
                get_memory(MEM_EVT_VAULTS_DATA),
            ).expect("init EVT_VAULTS log")
        });

    static EVT_STABILITY_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsStabilityEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_STABILITY_IDX),
                get_memory(MEM_EVT_STABILITY_DATA),
            ).expect("init EVT_STABILITY log")
        });

    static EVT_ADMIN_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsAdminEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_ADMIN_IDX),
                get_memory(MEM_EVT_ADMIN_DATA),
            ).expect("init EVT_ADMIN log")
        });
}

// --- Accessor modules ---

macro_rules! evt_accessors {
    ($mod_name:ident, $log:ident, $row_type:ty) => {
        #[allow(dead_code)]
        pub mod $mod_name {
            use super::*;

            pub fn push(row: $row_type) {
                $log.with(|log| {
                    log.borrow_mut().append(&row).expect(concat!("append ", stringify!($mod_name)));
                });
            }

            pub fn len() -> u64 {
                $log.with(|log| log.borrow().len())
            }

            pub fn get(index: u64) -> Option<$row_type> {
                $log.with(|log| log.borrow().get(index))
            }

            pub fn range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<$row_type> {
                let mut out = Vec::new();
                $log.with(|log| {
                    let log = log.borrow();
                    let n = log.len();
                    for i in 0..n {
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= to_ts {
                                break;
                            }
                            if row.timestamp_ns >= from_ts {
                                out.push(row);
                                if out.len() >= limit {
                                    break;
                                }
                            }
                        }
                    }
                });
                out
            }
        }
    };
}

evt_accessors!(evt_liquidations, EVT_LIQUIDATIONS_LOG, AnalyticsLiquidationEvent);
evt_accessors!(evt_swaps, EVT_SWAPS_LOG, AnalyticsSwapEvent);
evt_accessors!(evt_liquidity, EVT_LIQUIDITY_LOG, AnalyticsLiquidityEvent);
evt_accessors!(evt_vaults, EVT_VAULTS_LOG, AnalyticsVaultEvent);
evt_accessors!(evt_stability, EVT_STABILITY_LOG, AnalyticsStabilityEvent);
evt_accessors!(evt_admin, EVT_ADMIN_LOG, AnalyticsAdminEvent);

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    #[test]
    fn liquidation_event_roundtrip() {
        let evt = AnalyticsLiquidationEvent {
            timestamp_ns: 1_000_000,
            source_event_id: 42,
            vault_id: 7,
            collateral_type: Principal::anonymous(),
            collateral_amount: 500_000_000,
            debt_amount: 100_000_000,
            liquidation_kind: LiquidationKind::Full,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsLiquidationEvent::from_bytes(bytes);
        assert_eq!(decoded.vault_id, 7);
        assert_eq!(decoded.collateral_amount, 500_000_000);
    }

    #[test]
    fn swap_event_roundtrip() {
        let evt = AnalyticsSwapEvent {
            timestamp_ns: 2_000_000,
            source: SwapSource::ThreePool,
            source_event_id: 10,
            caller: Principal::anonymous(),
            token_in: Principal::anonymous(),
            token_out: Principal::anonymous(),
            amount_in: 1_000_000,
            amount_out: 999_000,
            fee: 1_000,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsSwapEvent::from_bytes(bytes);
        assert_eq!(decoded.amount_in, 1_000_000);
        assert!(matches!(decoded.source, SwapSource::ThreePool));
    }

    #[test]
    fn vault_event_roundtrip() {
        let evt = AnalyticsVaultEvent {
            timestamp_ns: 3_000_000,
            source_event_id: 5,
            vault_id: 1,
            owner: Principal::anonymous(),
            event_kind: VaultEventKind::Opened,
            collateral_type: Principal::anonymous(),
            amount: 10_000_000_000,
            fee_amount: None,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsVaultEvent::from_bytes(bytes);
        assert_eq!(decoded.vault_id, 1);
        assert!(matches!(decoded.event_kind, VaultEventKind::Opened));
        assert_eq!(decoded.fee_amount, None);
    }

    /// Pre-round-1 events were encoded by an `AnalyticsVaultEvent` whose
    /// struct lacked `fee_amount` entirely. The first round-2 deploy traps
    /// when `fee_amount` is required (`u64`) because candid subtyping
    /// rejects "field missing" against a non-optional schema. This test
    /// pins down the fix: a legacy-shaped struct round-trips into the
    /// current one with `fee_amount = None`.
    #[test]
    fn vault_event_decodes_pre_round1_legacy_shape() {
        #[derive(candid::CandidType, serde::Deserialize)]
        struct LegacyVaultEvent {
            timestamp_ns: u64,
            source_event_id: u64,
            vault_id: u64,
            owner: Principal,
            event_kind: VaultEventKind,
            collateral_type: Principal,
            amount: u64,
        }
        let legacy = LegacyVaultEvent {
            timestamp_ns: 3_000_000,
            source_event_id: 5,
            vault_id: 42,
            owner: Principal::anonymous(),
            event_kind: VaultEventKind::Borrowed,
            collateral_type: Principal::anonymous(),
            amount: 1_000_000_000,
        };
        let bytes = candid::Encode!(&legacy).expect("encode legacy");
        let decoded = AnalyticsVaultEvent::from_bytes(std::borrow::Cow::Owned(bytes));
        assert_eq!(decoded.vault_id, 42);
        assert!(matches!(decoded.event_kind, VaultEventKind::Borrowed));
        assert_eq!(decoded.fee_amount, None);
    }

    #[test]
    fn liquidity_event_roundtrip() {
        let evt = AnalyticsLiquidityEvent {
            timestamp_ns: 4_000_000,
            source_event_id: 20,
            caller: Principal::anonymous(),
            action: LiquidityAction::Add,
            amounts: vec![100, 200, 300],
            lp_amount: 500,
            coin_index: None,
            fee: Some(5),
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsLiquidityEvent::from_bytes(bytes);
        assert_eq!(decoded.amounts, vec![100, 200, 300]);
        assert_eq!(decoded.fee, Some(5));
    }

    #[test]
    fn stability_event_roundtrip() {
        let evt = AnalyticsStabilityEvent {
            timestamp_ns: 5_000_000,
            source_event_id: 31,
            caller: Principal::anonymous(),
            action: StabilityAction::Deposit,
            amount: 123_456_789,
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsStabilityEvent::from_bytes(bytes);
        assert_eq!(decoded.amount, 123_456_789);
        assert!(matches!(decoded.action, StabilityAction::Deposit));
    }

    #[test]
    fn admin_event_roundtrip() {
        let evt = AnalyticsAdminEvent {
            timestamp_ns: 6_000_000,
            source_event_id: 77,
            label: "SetBorrowingFee".to_string(),
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsAdminEvent::from_bytes(bytes);
        assert_eq!(decoded.label, "SetBorrowingFee");
        assert_eq!(decoded.source_event_id, 77);
    }
}
