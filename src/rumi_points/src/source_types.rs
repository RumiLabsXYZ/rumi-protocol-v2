//! Wire mirror types for the four source canisters (Phase 2), plus the
//! `normalize_*` functions that turn a decoded source event into the normalized
//! `events::IngestedEvent`. Types here are derived VERBATIM from each source's
//! committed candid (extracted 2026-06-02). Only the fields/variants the points
//! canister needs are mirrored.
//!
//! KEY DECODE QUESTION (resolved by the canary tests below): the backend's
//! `get_events*` return type is the full ~95-variant `Event`. We only mirror the
//! ~9 variants we care about and fetch with a server-side `types` filter so only
//! those values come back. Candid's variant subtyping says a superset is NOT a
//! subtype of a subset, so whether `Decode!` accepts a subset target against a
//! superset wire type is the load-bearing assumption. The canary test pins it.

#![allow(dead_code)] // request types + normalize are consumed by the poll layer next

use candid::{CandidType, Principal};
use serde::Deserialize;

use crate::events::{IngestedEvent, IngestKind, SourceId};

fn to_amounts3(v: &[u128]) -> [u128; 3] {
    [
        v.first().copied().unwrap_or(0),
        v.get(1).copied().unwrap_or(0),
        v.get(2).copied().unwrap_or(0),
    ]
}

// ── rumi_protocol_backend ───────────────────────────────────────────────────
// The log return type is the full ~95-variant `Event`. We mirror only the 9
// variants we act on and fetch with a `types` server-side filter so only those
// VALUES come back (the first canary proves a subset target decodes them). Each
// record declares only the fields we use; candid record width-subtyping ignores
// the rest (block_index, fee_amount, icp_rate blobs, Mode, etc.).
pub mod backend {
    use super::*;

    /// Argument to `get_events` / `get_events_filtered`. We send a `types` filter
    /// limited to the variants we mirror; a subset variant value is a subtype of
    /// the backend's full `EventTypeFilter`, so the call type-checks.
    #[derive(CandidType, Clone, Debug)]
    pub struct GetEventsArg {
        pub principal: Option<Principal>,
        pub types: Option<Vec<EventTypeFilter>>,
        pub time_range: Option<EventTimeRange>,
        pub start: u64,
        pub collateral_token: Option<Principal>,
        pub length: u64,
        pub min_size_e8s: Option<u64>,
        pub admin_labels: Option<Vec<String>>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct EventTimeRange {
        pub start_ns: u64,
        pub end_ns: u64,
    }

    /// The subset of the backend's `EventTypeFilter` we request server-side.
    #[derive(CandidType, Clone, Debug)]
    pub enum EventTypeFilter {
        OpenVault,
        Borrow,
        Repay,
        CloseVault,
        Liquidation,
        PartialLiquidation,
        Redemption,
        StabilityPoolDeposit,
        StabilityPoolWithdraw,
    }

    /// The filter we send to only surface the events the points canister acts on.
    pub fn points_event_filter() -> Vec<EventTypeFilter> {
        vec![
            EventTypeFilter::OpenVault,
            EventTypeFilter::Borrow,
            EventTypeFilter::Repay,
            EventTypeFilter::CloseVault,
            EventTypeFilter::Liquidation,
            EventTypeFilter::PartialLiquidation,
            EventTypeFilter::Redemption,
        ]
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct BackendVault {
        pub owner: Principal,
        pub vault_id: u64,
        pub borrowed_icusd_amount: u64,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum BackendEvent {
        #[serde(rename = "open_vault")]
        OpenVault {
            vault: BackendVault,
            timestamp: Option<u64>,
        },
        #[serde(rename = "borrow_from_vault")]
        BorrowFromVault {
            vault_id: u64,
            caller: Option<Principal>,
            borrowed_amount: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "repay_to_vault")]
        RepayToVault {
            vault_id: u64,
            caller: Option<Principal>,
            repayed_amount: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "close_vault")]
        CloseVault {
            vault_id: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "liquidate_vault")]
        LiquidateVault {
            vault_id: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "partial_liquidate_vault")]
        PartialLiquidateVault {
            vault_id: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "redemption_on_vaults")]
        RedemptionOnVaults {
            owner: Principal,
            icusd_amount: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "provide_liquidity")]
        ProvideLiquidity {
            caller: Principal,
            amount: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "withdraw_liquidity")]
        WithdrawLiquidity {
            caller: Principal,
            amount: u64,
            timestamp: Option<u64>,
        },
    }

    pub fn normalize(id: u64, ev: BackendEvent) -> IngestedEvent {
        let (caller, ts, kind) = match ev {
            BackendEvent::OpenVault { vault, timestamp } => (
                Some(vault.owner),
                timestamp,
                IngestKind::VaultOpen {
                    vault_id: vault.vault_id,
                    borrowed_e8s: vault.borrowed_icusd_amount,
                },
            ),
            BackendEvent::BorrowFromVault { vault_id, caller, borrowed_amount, timestamp } => (
                caller,
                timestamp,
                IngestKind::VaultBorrow { vault_id, amount_e8s: borrowed_amount },
            ),
            BackendEvent::RepayToVault { vault_id, caller, repayed_amount, timestamp } => (
                caller,
                timestamp,
                // `repayment_asset` is gated: the upstream backend `RepayToVault`
                // event does not carry it yet, so it stays `None` and the 5x
                // ck-stable window is skipped until the field ships.
                IngestKind::VaultRepay {
                    vault_id,
                    amount_e8s: repayed_amount,
                    repayment_asset: None,
                },
            ),
            BackendEvent::CloseVault { vault_id, timestamp } => {
                (None, timestamp, IngestKind::VaultClose { vault_id })
            }
            BackendEvent::LiquidateVault { vault_id, timestamp } => {
                (None, timestamp, IngestKind::VaultLiquidated { vault_id })
            }
            BackendEvent::PartialLiquidateVault { vault_id, timestamp } => {
                (None, timestamp, IngestKind::VaultLiquidated { vault_id })
            }
            BackendEvent::RedemptionOnVaults { owner, icusd_amount, timestamp } => (
                Some(owner),
                timestamp,
                IngestKind::VaultRedeemed { amount_e8s: icusd_amount },
            ),
            // Legacy backend-side stability pool, superseded by the
            // rumi_stability_pool canister; ignore to avoid double-counting.
            BackendEvent::ProvideLiquidity { .. } | BackendEvent::WithdrawLiquidity { .. } => {
                (None, None, IngestKind::Other)
            }
        };
        IngestedEvent {
            source: SourceId::Backend,
            event_id: id,
            caller,
            timestamp_ns: ts.unwrap_or(0),
            kind,
        }
    }

    /// Mirror of the backend's `get_events_forward_filtered` response (added on
    /// branch feat/source-forward-event-endpoints). The poller advances its
    /// backend cursor to `next_start` (a scan position), not `max(event_id)+1`.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct ForwardFilteredEventsResponse {
        pub events: Vec<(u64, BackendEvent)>,
        pub next_start: u64,
        pub reached_end: bool,
    }

    /// Normalize a forward batch, returning the events plus `(next_start, reached_end)`.
    pub fn normalize_forward(
        resp: ForwardFilteredEventsResponse,
    ) -> (Vec<IngestedEvent>, u64, bool) {
        let events = resp
            .events
            .into_iter()
            .map(|(id, ev)| normalize(id, ev))
            .collect();
        (events, resp.next_start, resp.reached_end)
    }
}

// ── rumi_3pool ──────────────────────────────────────────────────────────────
pub mod three_pool {
    use super::*;

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
    pub enum LiquidityAction {
        AddLiquidity,
        RemoveLiquidity,
        RemoveOneCoin,
        Donate,
    }

    /// `amounts` is `[icUSD, ckUSDT, ckUSDC]`. Only the fields we use are mirrored.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct LiquidityEventV2 {
        pub id: u64,
        pub timestamp: u64,
        pub caller: Principal,
        pub action: LiquidityAction,
        pub amounts: Vec<u128>,
        pub migrated: bool,
    }

    pub fn normalize(ev: LiquidityEventV2) -> IngestedEvent {
        let amounts = to_amounts3(&ev.amounts);
        // `migrated` rows are v1->v2 backfill (pre-season); exclude from accrual.
        let kind = if ev.migrated {
            IngestKind::Other
        } else {
            match ev.action {
                LiquidityAction::AddLiquidity => IngestKind::ThreePoolAdd { amounts },
                LiquidityAction::RemoveLiquidity | LiquidityAction::RemoveOneCoin => {
                    IngestKind::ThreePoolRemove { amounts }
                }
                LiquidityAction::Donate => IngestKind::Other,
            }
        };
        IngestedEvent {
            source: SourceId::ThreePool,
            event_id: ev.id,
            caller: Some(ev.caller),
            timestamp_ns: ev.timestamp,
            kind,
        }
    }

    /// Mirror of 3pool's `get_liquidity_events_v2_forward` response (added on
    /// branch feat/source-forward-event-endpoints). Poller advances to `next_start`.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct ForwardLiquidityEventsV2 {
        pub events: Vec<(u64, LiquidityEventV2)>,
        pub next_start: u64,
        pub reached_end: bool,
    }

    pub fn normalize_forward(
        resp: ForwardLiquidityEventsV2,
    ) -> (Vec<IngestedEvent>, u64, bool) {
        // The tuple index equals ev.id here; normalize reads ev.id directly.
        let events = resp.events.into_iter().map(|(_, ev)| normalize(ev)).collect();
        (events, resp.next_start, resp.reached_end)
    }
}

// ── rumi_stability_pool ─────────────────────────────────────────────────────
// `get_pool_events` has no server-side filter, so ALL variants can appear as
// values. The second canary proves candid rejects unknown-variant values, so we
// must mirror every variant. Ones we do not act on carry an empty record payload
// (`{}`), which the wire record is a width-subtype of.
pub mod stability_pool {
    use super::*;

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum PoolEventType {
        Deposit { token_ledger: Principal, amount: u64 },
        Withdraw { token_ledger: Principal, amount: u64 },
        DepositAs3USD { token_ledger: Principal, amount_in: u64, lp_minted: u64 },
        ClaimCollateral {},
        InterestReceived {},
        OptOutCollateral {},
        OptInCollateral {},
        LiquidationNotification {},
        LiquidationExecuted {},
        StablecoinRegistered {},
        CollateralRegistered {},
        ConfigurationUpdated,
        EmergencyPauseActivated,
        OperationsResumed,
        BalanceCorrected {},
        CollateralGainCorrected {},
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct PoolEvent {
        pub id: u64,
        pub timestamp: u64,
        pub caller: Principal,
        pub event_type: PoolEventType,
    }

    pub fn normalize(ev: PoolEvent) -> IngestedEvent {
        let kind = match ev.event_type {
            PoolEventType::Deposit { token_ledger, amount } => {
                IngestKind::SpDeposit { token_ledger, amount_e8s: amount }
            }
            // Deposit-as-3USD: the input asset funded a 3USD position in the SP.
            PoolEventType::DepositAs3USD { token_ledger, amount_in, .. } => {
                IngestKind::SpDeposit { token_ledger, amount_e8s: amount_in }
            }
            PoolEventType::Withdraw { token_ledger, amount } => {
                IngestKind::SpWithdraw { token_ledger, amount_e8s: amount }
            }
            _ => IngestKind::Other,
        };
        IngestedEvent {
            source: SourceId::StabilityPool,
            event_id: ev.id,
            caller: Some(ev.caller),
            timestamp_ns: ev.timestamp,
            kind,
        }
    }
}

// ── rumi_amm ────────────────────────────────────────────────────────────────
pub mod amm {
    use super::*;

    #[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
    pub enum AmmLiquidityAction {
        AddLiquidity,
        RemoveLiquidity,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct AmmLiquidityEvent {
        pub id: u64,
        pub caller: Principal,
        pub pool_id: String,
        pub action: AmmLiquidityAction,
        pub lp_shares: u128,
        pub timestamp: u64,
    }

    pub fn normalize(ev: AmmLiquidityEvent) -> IngestedEvent {
        let kind = match ev.action {
            AmmLiquidityAction::AddLiquidity => IngestKind::AmmAddLiquidity {
                pool_id: ev.pool_id,
                lp_shares: ev.lp_shares,
            },
            AmmLiquidityAction::RemoveLiquidity => IngestKind::AmmRemoveLiquidity {
                pool_id: ev.pool_id,
                lp_shares: ev.lp_shares,
            },
        };
        IngestedEvent {
            source: SourceId::Amm,
            event_id: ev.id,
            caller: Some(ev.caller),
            timestamp_ns: ev.timestamp,
            kind,
        }
    }
}

// ── Balance / position queries (Phase 5 snapshot capture) ───────────────────
// Minimal candid mirrors of each source's READ endpoints. Record width-subtyping
// lets us declare only the fields the snapshot driver reads; everything else on
// the wire is ignored.
pub mod balances {
    use super::*;
    use std::collections::BTreeMap;

    /// `rumi_protocol_backend::get_vaults(opt principal) -> vec CandidVault`. The
    /// debt already includes accrued interest.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct CandidVault {
        pub borrowed_icusd_amount: u64,
    }

    /// `rumi_protocol_backend::get_protocol_status() -> ProtocolStatus`.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct ProtocolStatus {
        pub last_icp_rate: f64,
    }

    /// `rumi_3pool::get_pool_status() -> PoolStatus` (1e18-scaled virtual price).
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct PoolStatus {
        pub virtual_price: u128,
    }

    /// `rumi_stability_pool::get_user_position(opt principal) -> opt
    /// UserStabilityPosition`. Balances are per-ledger native decimals (icUSD and
    /// 3USD are both 8-dec), already compounded for liquidation draws.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct UserStabilityPosition {
        pub stablecoin_balances: BTreeMap<Principal, u64>,
    }

    /// `rumi_amm::get_pools() -> vec PoolInfo`. Reserves are native e8s.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct PoolInfo {
        pub pool_id: String,
        pub token_a: Principal,
        pub token_b: Principal,
        pub reserve_a: u128,
        pub reserve_b: u128,
        pub total_lp_shares: u128,
    }
}

#[cfg(test)]
mod canary {
    use candid::{CandidType, Decode, Encode};
    use serde::Deserialize;

    // Encoder side: a 3-variant "superset" (mimics the backend's big Event).
    #[derive(CandidType, Deserialize)]
    enum Superset {
        A(u64),
        B(u32),
        C, // a variant absent from the decoder's subset type
    }

    // Decoder side: a 2-variant "subset" (mimics our 9-of-95 mirror).
    #[derive(CandidType, Deserialize, Debug, PartialEq)]
    enum Subset {
        A(u64),
        B(u32),
    }

    /// The load-bearing assumption: a value whose tag IS in the subset decodes,
    /// even though the wire type table also declares the extra variant `C`.
    #[test]
    fn decode_shared_variant_from_superset_wire_type() {
        let bytes = Encode!(&Superset::A(7)).unwrap();
        let got = Decode!(&bytes, Subset).unwrap();
        assert_eq!(got, Subset::A(7));

        let bytes_b = Encode!(&Superset::B(9)).unwrap();
        let got_b = Decode!(&bytes_b, Subset).unwrap();
        assert_eq!(got_b, Subset::B(9));
    }

    /// And the inverse we rely on for ARGS: a subset value encodes and decodes
    /// against a superset target (this direction is plain subtyping and must work).
    #[test]
    fn decode_subset_value_into_superset_target() {
        let bytes = Encode!(&Subset::A(7)).unwrap();
        let got = Decode!(&bytes, Superset).unwrap();
        assert!(matches!(got, Superset::A(7)));
    }

    // Does candid honor serde's #[serde(other)] catch-all when the wire VALUE is
    // a variant absent from the target? If yes, sources with no server-side
    // filter (the stability pool) need only mirror the variants they act on plus
    // one catch-all, instead of all 16.
    #[derive(CandidType, Deserialize, Debug, PartialEq)]
    enum SubsetWithOther {
        A(u64),
        #[serde(other)]
        Unknown,
    }

    /// Pins the NEGATIVE result: candid does NOT honor `#[serde(other)]`. A wire
    /// value whose variant is absent from the target fails to decode. This is WHY
    /// unfiltered sources (the stability pool) must mirror every variant that can
    /// appear in their log, and why the backend (which CAN filter server-side)
    /// only mirrors its 9 and passes a `types` filter.
    #[test]
    fn candid_does_not_honor_serde_other_catch_all() {
        let bytes = Encode!(&Superset::C).unwrap(); // C is absent from SubsetWithOther
        let decoded = Decode!(&bytes, SubsetWithOther);
        assert!(
            decoded.is_err(),
            "if candid ever gains serde(other) support, the SP mirror can be slimmed to a subset"
        );
    }
}

#[cfg(test)]
mod balances_tests {
    use super::balances::*;
    use candid::{CandidType, Decode, Encode, Principal};
    use std::collections::BTreeMap;

    fn p(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }

    /// The subset vault decodes from a wider on-wire vault (record width subtyping).
    #[test]
    fn candid_vault_decodes_from_wider_wire() {
        #[derive(CandidType)]
        struct WireVault {
            owner: Principal,
            vault_id: u64,
            borrowed_icusd_amount: u64,
            collateral_amount: u64,
            accrued_interest: u64,
        }
        let bytes = Encode!(&vec![WireVault {
            owner: p(1),
            vault_id: 7,
            borrowed_icusd_amount: 12_345,
            collateral_amount: 9,
            accrued_interest: 3,
        }])
        .unwrap();
        let got = Decode!(&bytes, Vec<CandidVault>).unwrap();
        assert_eq!(got[0].borrowed_icusd_amount, 12_345);
    }

    #[test]
    fn protocol_status_decodes_rate_from_wider_wire() {
        #[derive(CandidType)]
        struct WireStatus {
            last_icp_rate: f64,
            last_icp_timestamp: u64,
            total_debt: u64,
        }
        let bytes = Encode!(&WireStatus {
            last_icp_rate: 5.5,
            last_icp_timestamp: 1,
            total_debt: 2,
        })
        .unwrap();
        assert_eq!(Decode!(&bytes, ProtocolStatus).unwrap().last_icp_rate, 5.5);
    }

    #[test]
    fn user_position_decodes_balance_map() {
        let mut balances = BTreeMap::new();
        balances.insert(p(2), 500u64);
        let bytes = Encode!(&Some(UserStabilityPosition { stablecoin_balances: balances })).unwrap();
        let got = Decode!(&bytes, Option<UserStabilityPosition>).unwrap().unwrap();
        assert_eq!(got.stablecoin_balances.get(&p(2)), Some(&500));
    }

    #[test]
    fn pool_info_round_trips() {
        let info = PoolInfo {
            pool_id: "a_b".into(),
            token_a: p(1),
            token_b: p(2),
            reserve_a: 100,
            reserve_b: 200,
            total_lp_shares: 300,
        };
        let got = Decode!(&Encode!(&vec![info]).unwrap(), Vec<PoolInfo>).unwrap();
        assert_eq!(got[0].reserve_a, 100);
        assert_eq!(got[0].total_lp_shares, 300);
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::*;
    use crate::events::{IngestKind, SourceId};
    use candid::{Decode, Encode};

    fn p(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }

    /// Candid round-trip through a mirror type (proves it is candid-valid and
    /// decodes the way the wire would encode it).
    fn roundtrip<T: CandidType + for<'de> Deserialize<'de>>(v: &T) -> T {
        let bytes = Encode!(v).unwrap();
        Decode!(&bytes, T).unwrap()
    }

    #[test]
    fn backend_borrow_normalizes_to_vault_borrow() {
        let ev = backend::BackendEvent::BorrowFromVault {
            vault_id: 3,
            caller: Some(p(1)),
            borrowed_amount: 500,
            timestamp: Some(123),
        };
        let out = backend::normalize(7, roundtrip(&ev));
        assert_eq!(out.source, SourceId::Backend);
        assert_eq!(out.event_id, 7);
        assert_eq!(out.caller, Some(p(1)));
        assert_eq!(out.timestamp_ns, 123);
        assert_eq!(out.kind, IngestKind::VaultBorrow { vault_id: 3, amount_e8s: 500 });
    }

    #[test]
    fn backend_open_uses_vault_owner_and_borrowed_amount() {
        let ev = backend::BackendEvent::OpenVault {
            vault: backend::BackendVault { owner: p(2), vault_id: 9, borrowed_icusd_amount: 1000 },
            timestamp: Some(50),
        };
        let out = backend::normalize(1, roundtrip(&ev));
        assert_eq!(out.caller, Some(p(2)));
        assert_eq!(out.kind, IngestKind::VaultOpen { vault_id: 9, borrowed_e8s: 1000 });
    }

    #[test]
    fn backend_close_has_no_caller() {
        let ev = backend::BackendEvent::CloseVault { vault_id: 4, timestamp: Some(1) };
        let out = backend::normalize(2, roundtrip(&ev));
        assert_eq!(out.caller, None);
        assert_eq!(out.kind, IngestKind::VaultClose { vault_id: 4 });
    }

    /// Decisive backend case: a value encoded by a SUPERSET type (an extra
    /// variant in the type table, mimicking the real ~95-variant Event) decodes
    /// into our 9-variant mirror.
    #[derive(CandidType, Deserialize)]
    enum BackendSuperset {
        #[serde(rename = "borrow_from_vault")]
        BorrowFromVault {
            vault_id: u64,
            caller: Option<Principal>,
            borrowed_amount: u64,
            timestamp: Option<u64>,
        },
        #[serde(rename = "some_variant_we_do_not_mirror")]
        SomethingElse { x: u64 },
    }

    #[test]
    fn backend_subset_decodes_value_from_superset_wire_type() {
        let bytes = Encode!(&BackendSuperset::BorrowFromVault {
            vault_id: 5,
            caller: Some(p(3)),
            borrowed_amount: 250,
            timestamp: Some(99),
        })
        .unwrap();
        let ev = Decode!(&bytes, backend::BackendEvent).unwrap();
        assert_eq!(
            backend::normalize(0, ev).kind,
            IngestKind::VaultBorrow { vault_id: 5, amount_e8s: 250 }
        );
    }

    #[test]
    fn three_pool_add_carries_amounts_and_migrated_is_excluded() {
        let ev = three_pool::LiquidityEventV2 {
            id: 2,
            timestamp: 10,
            caller: p(1),
            action: three_pool::LiquidityAction::AddLiquidity,
            amounts: vec![100, 200, 300],
            migrated: false,
        };
        assert_eq!(
            three_pool::normalize(roundtrip(&ev)).kind,
            IngestKind::ThreePoolAdd { amounts: [100, 200, 300] }
        );

        let migrated = three_pool::LiquidityEventV2 {
            id: 3,
            timestamp: 10,
            caller: p(1),
            action: three_pool::LiquidityAction::AddLiquidity,
            amounts: vec![1, 2, 3],
            migrated: true,
        };
        assert_eq!(three_pool::normalize(migrated).kind, IngestKind::Other);
    }

    #[test]
    fn sp_deposit_normalizes_and_unhandled_payload_variant_decodes() {
        let ev = stability_pool::PoolEvent {
            id: 1,
            timestamp: 5,
            caller: p(1),
            event_type: stability_pool::PoolEventType::Deposit { token_ledger: p(9), amount: 77 },
        };
        assert_eq!(
            stability_pool::normalize(roundtrip(&ev)).kind,
            IngestKind::SpDeposit { token_ledger: p(9), amount_e8s: 77 }
        );

        // A wire value of an unhandled variant that really carries a record
        // payload must still decode into our empty-payload mirror variant.
        #[derive(CandidType)]
        enum SpFull {
            LiquidationExecuted {
                vault_id: u64,
                stables_consumed_e8s: u64,
                collateral_gained: u64,
                collateral_type: Principal,
                success: bool,
            },
        }
        #[derive(CandidType)]
        struct SpEventFull {
            id: u64,
            timestamp: u64,
            caller: Principal,
            event_type: SpFull,
        }
        let bytes = Encode!(&SpEventFull {
            id: 2,
            timestamp: 6,
            caller: p(1),
            event_type: SpFull::LiquidationExecuted {
                vault_id: 1,
                stables_consumed_e8s: 1,
                collateral_gained: 1,
                collateral_type: p(2),
                success: true,
            },
        })
        .unwrap();
        let decoded = Decode!(&bytes, stability_pool::PoolEvent).unwrap();
        assert_eq!(stability_pool::normalize(decoded).kind, IngestKind::Other);
    }

    #[test]
    fn amm_add_normalizes_to_provide_liquidity() {
        let ev = amm::AmmLiquidityEvent {
            id: 1,
            caller: p(1),
            pool_id: "3usd-icp".into(),
            action: amm::AmmLiquidityAction::AddLiquidity,
            lp_shares: 42,
            timestamp: 8,
        };
        let out = amm::normalize(roundtrip(&ev));
        assert_eq!(out.caller, Some(p(1)));
        assert_eq!(
            out.kind,
            IngestKind::AmmAddLiquidity { pool_id: "3usd-icp".into(), lp_shares: 42 }
        );
    }

    #[test]
    fn backend_forward_response_roundtrips_and_carries_cursor() {
        let resp = backend::ForwardFilteredEventsResponse {
            events: vec![
                (
                    10,
                    backend::BackendEvent::BorrowFromVault {
                        vault_id: 1,
                        caller: Some(p(1)),
                        borrowed_amount: 50,
                        timestamp: Some(100),
                    },
                ),
                (
                    11,
                    backend::BackendEvent::CloseVault { vault_id: 1, timestamp: Some(100) },
                ),
            ],
            next_start: 12,
            reached_end: true,
        };
        let (events, next_start, end) = backend::normalize_forward(roundtrip(&resp));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, 10);
        assert_eq!(events[0].kind, IngestKind::VaultBorrow { vault_id: 1, amount_e8s: 50 });
        assert_eq!(events[1].kind, IngestKind::VaultClose { vault_id: 1 });
        assert_eq!(next_start, 12); // poller advances backend cursor to next_start
        assert!(end);
    }

    #[test]
    fn three_pool_forward_response_roundtrips_and_carries_cursor() {
        let resp = three_pool::ForwardLiquidityEventsV2 {
            events: vec![(
                5,
                three_pool::LiquidityEventV2 {
                    id: 5,
                    timestamp: 100,
                    caller: p(2),
                    action: three_pool::LiquidityAction::AddLiquidity,
                    amounts: vec![1, 2, 3],
                    migrated: false,
                },
            )],
            next_start: 6,
            reached_end: false,
        };
        let (events, next_start, end) = three_pool::normalize_forward(roundtrip(&resp));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, 5);
        assert_eq!(events[0].kind, IngestKind::ThreePoolAdd { amounts: [1, 2, 3] });
        assert_eq!(next_start, 6);
        assert!(!end);
    }
}
