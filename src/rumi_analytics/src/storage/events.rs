//! EVT_* normalized event log types and StableLog instances.

use candid::{CandidType, Decode, Encode, Principal};
use ic_stable_structures::storable::{Bound, Storable};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::RefCell;
use super::{get_memory, Memory, MEM_ADD_MARGIN_EVENT_IDS};
use super::{
    MEM_EVT_LIQUIDATIONS_IDX, MEM_EVT_LIQUIDATIONS_DATA,
    MEM_EVT_SWAPS_IDX, MEM_EVT_SWAPS_DATA,
    MEM_EVT_LIQUIDITY_IDX, MEM_EVT_LIQUIDITY_DATA,
    MEM_EVT_VAULTS_IDX, MEM_EVT_VAULTS_DATA,
    MEM_EVT_STABILITY_IDX, MEM_EVT_STABILITY_DATA,
    MEM_EVT_ADMIN_IDX, MEM_EVT_ADMIN_DATA,
    MEM_EVT_AMM_LIQUIDITY_IDX, MEM_EVT_AMM_LIQUIDITY_DATA,
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
    /// Collateral added to an existing vault (e.g. AddMarginToVault). Stored
    /// rows decoded from a pre-2026-04-30 schema never carry this variant; the
    /// timeline reconstructor treats unknown variants as no-op so legacy logs
    /// still decode after upgrade.
    CollateralDeposited,
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

/// AMM Add/RemoveLiquidity event mirror, used to reconstruct per-(principal,
/// pool_id) LP-share timelines for portfolio valuation.
#[derive(CandidType, Clone, Debug, Serialize, Deserialize)]
pub struct AnalyticsAmmLiquidityEvent {
    pub timestamp_ns: u64,
    pub source_event_id: u64,
    pub caller: Principal,
    pub pool_id: String,
    pub action: LiquidityAction,
    pub lp_shares: u64,
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
storable_candid!(AnalyticsAmmLiquidityEvent);

// --- StableLog instances ---

thread_local! {
    /// Sparse index of backend AddMargin events already represented in the
    /// vault event log. Backfill and live tailing share this dedupe gate.
    static ADD_MARGIN_EVENT_IDS: RefCell<ic_stable_structures::StableBTreeMap<u64, u8, Memory>> =
        RefCell::new(ic_stable_structures::StableBTreeMap::init(get_memory(MEM_ADD_MARGIN_EVENT_IDS)));

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

    static EVT_AMM_LIQUIDITY_LOG: RefCell<ic_stable_structures::StableLog<AnalyticsAmmLiquidityEvent, Memory, Memory>> =
        RefCell::new({
            ic_stable_structures::StableLog::init(
                get_memory(MEM_EVT_AMM_LIQUIDITY_IDX),
                get_memory(MEM_EVT_AMM_LIQUIDITY_DATA),
            ).expect("init EVT_AMM_LIQUIDITY log")
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
                    if limit == 0 {
                        return;
                    }
                    for i in 0..n {
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= from_ts {
                                if row.timestamp_ns < to_ts {
                                    out.push(row);
                                    if out.len() >= limit {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                });
                out
            }

            /// Return the most recently appended rows in `[from_ts, to_ts)`,
            /// preserving their chronological timestamp order. Event source
            /// timestamps may be delayed or out of order, so this deliberately
            /// scans the bounded tail rather than binary-searching by timestamp.
            /// If more than `limit` matching rows exist, older rows are omitted.
            pub fn tail_range(from_ts: u64, to_ts: u64, limit: usize) -> Vec<$row_type> {
                let mut out = Vec::new();
                $log.with(|log| {
                    let log = log.borrow();
                    let mut i = log.len();
                    while i > 0 && out.len() < limit {
                        i -= 1;
                        if let Some(row) = log.get(i) {
                            if row.timestamp_ns >= from_ts && row.timestamp_ns < to_ts {
                                out.push(row);
                            }
                        }
                    }
                });
                out.reverse();
                out.sort_by_key(|row| row.timestamp_ns);
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
evt_accessors!(evt_amm_liquidity, EVT_AMM_LIQUIDITY_LOG, AnalyticsAmmLiquidityEvent);

/// Append one AddMargin row for each backend source event. The index claim and
/// StableLog append are synchronous, so callbacks cannot interleave between
/// them; a trap rolls both stable writes back with the message.
pub fn push_add_margin_if_new(row: AnalyticsVaultEvent) -> bool {
    if row.event_kind != VaultEventKind::CollateralDeposited {
        return false;
    }
    let event_id = row.source_event_id;
    let inserted = ADD_MARGIN_EVENT_IDS.with(|ids| {
        let mut ids = ids.borrow_mut();
        if ids.contains_key(&event_id) {
            false
        } else {
            ids.insert(event_id, 1);
            true
        }
    });
    if inserted {
        evt_vaults::push(row);
    }
    inserted
}

/// Index a bounded range of existing vault rows. Returns the next log index.
pub fn index_existing_add_margin_rows(from: u64, limit: u64) -> u64 {
    let end = from.saturating_add(limit).min(evt_vaults::len());
    for index in from..end {
        if let Some(row) = evt_vaults::get(index) {
            if row.event_kind == VaultEventKind::CollateralDeposited {
                ADD_MARGIN_EVENT_IDS.with(|ids| {
                    ids.borrow_mut().insert(row.source_event_id, 1);
                });
            }
        }
    }
    end
}

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
    fn tail_range_uses_recent_rows_and_returns_timestamp_order() {
        let first_id = u64::MAX - 2;
        let second_id = u64::MAX - 1;
        let third_id = u64::MAX;
        for (timestamp_ns, source_event_id) in [
            (u64::MAX - 100, first_id),
            (u64::MAX - 300, second_id),
            (u64::MAX - 200, third_id),
        ] {
            evt_stability::push(AnalyticsStabilityEvent {
                timestamp_ns,
                source_event_id,
                caller: Principal::anonymous(),
                action: StabilityAction::Deposit,
                amount: 1,
            });
        }

        // A time-range read still finds the earlier-timestamp event appended
        // after a later one; the tail read keeps the final two appended rows,
        // then restores chronological timestamp order.
        let ranged = evt_stability::range(u64::MAX - 250, u64::MAX, 10);
        assert_eq!(
            ranged
                .iter()
                .map(|row| row.source_event_id)
                .collect::<Vec<_>>(),
            vec![first_id, third_id]
        );

        let rows = evt_stability::tail_range(u64::MAX - 500, u64::MAX, 2);
        assert_eq!(
            rows.iter().map(|row| row.source_event_id).collect::<Vec<_>>(),
            vec![second_id, third_id]
        );
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

    #[test]
    fn add_margin_append_is_idempotent_across_live_and_backfill_paths() {
        let event_id = u64::MAX - 17;
        let initial_len = evt_vaults::len();
        let row = || AnalyticsVaultEvent {
            timestamp_ns: 9,
            source_event_id: event_id,
            vault_id: 44,
            owner: Principal::from_slice(&[4]),
            event_kind: VaultEventKind::CollateralDeposited,
            collateral_type: Principal::anonymous(),
            amount: 123,
            fee_amount: None,
        };

        assert!(push_add_margin_if_new(row()));
        assert!(!push_add_margin_if_new(row()));
        assert_eq!(evt_vaults::len(), initial_len + 1);

        // Simulate a pre-index persisted row and ensure bounded index warming
        // makes a subsequent backfill retry a no-op.
        let legacy_id = u64::MAX - 18;
        let mut legacy_row = row();
        legacy_row.source_event_id = legacy_id;
        evt_vaults::push(legacy_row);
        let legacy_position = evt_vaults::len() - 1;
        assert_eq!(index_existing_add_margin_rows(legacy_position, 1), evt_vaults::len());
        let mut retry = row();
        retry.source_event_id = legacy_id;
        assert!(!push_add_margin_if_new(retry));
        assert_eq!(evt_vaults::len(), initial_len + 2);
    }
}
