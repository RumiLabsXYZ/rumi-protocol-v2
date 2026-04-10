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

// --- Storable impls ---

macro_rules! storable_candid {
    ($t:ty) => {
        impl Storable for $t {
            fn to_bytes(&self) -> Cow<[u8]> {
                Cow::Owned(Encode!(self).expect(concat!(stringify!($t), " encode")))
            }
            fn from_bytes(bytes: Cow<[u8]>) -> Self {
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
}

// --- Accessor modules ---

macro_rules! evt_accessors {
    ($mod_name:ident, $log:ident, $row_type:ty) => {
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
        };
        let bytes = evt.to_bytes();
        let decoded = AnalyticsVaultEvent::from_bytes(bytes);
        assert_eq!(decoded.vault_id, 1);
        assert!(matches!(decoded.event_kind, VaultEventKind::Opened));
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
}
