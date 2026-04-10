//! Shared candid types for analytics queries and responses.

use candid::CandidType;
use serde::{Deserialize, Serialize};

use crate::storage::{DailyTvlRow, DailyVaultSnapshotRow, DailyStabilityRow};

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
pub struct RangeQuery {
    pub from_ts: Option<u64>,
    pub to_ts: Option<u64>,
    pub limit: Option<u32>,
    /// Skip the first N matching rows. Phase 1 ignores this field but it's
    /// declared in the candid interface from day one so future phases can
    /// honor it without breaking the public API.
    pub offset: Option<u64>,
}

pub const DEFAULT_LIMIT: u32 = 500;
pub const MAX_LIMIT: u32 = 2000;

impl RangeQuery {
    pub fn resolved_limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT)
    }
    pub fn resolved_from(&self) -> u64 {
        self.from_ts.unwrap_or(0)
    }
    pub fn resolved_to(&self) -> u64 {
        self.to_ts.unwrap_or(u64::MAX)
    }
}

#[derive(CandidType, Clone, Debug)]
pub struct TvlSeriesResponse {
    pub rows: Vec<DailyTvlRow>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct VaultSeriesResponse {
    pub rows: Vec<DailyVaultSnapshotRow>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct StabilitySeriesResponse {
    pub rows: Vec<DailyStabilityRow>,
    pub next_from_ts: Option<u64>,
}

use candid::Principal;
use crate::storage::holders::DailyHolderRow;

#[derive(CandidType, Clone, Debug)]
pub struct HolderSeriesResponse {
    pub rows: Vec<DailyHolderRow>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct LiquidationSeriesResponse {
    pub rows: Vec<crate::storage::rollups::DailyLiquidationRollup>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct SwapSeriesResponse {
    pub rows: Vec<crate::storage::rollups::DailySwapRollup>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct FeeSeriesResponse {
    pub rows: Vec<crate::storage::rollups::DailyFeeRollup>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct PriceSeriesResponse {
    pub rows: Vec<crate::storage::fast::FastPriceSnapshot>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct ThreePoolSeriesResponse {
    pub rows: Vec<crate::storage::fast::Fast3PoolSnapshot>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct CycleSeriesResponse {
    pub rows: Vec<crate::storage::hourly::HourlyCycleSnapshot>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct FeeCurveSeriesResponse {
    pub rows: Vec<crate::storage::hourly::HourlyFeeCurveSnapshot>,
    pub next_from_ts: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct CollectorHealth {
    pub cursors: Vec<CursorStatus>,
    pub error_counters: crate::storage::ErrorCounters,
    pub backfill_active: Vec<Principal>,
    pub last_pull_cycle_ns: u64,
    pub balance_tracker_stats: Vec<BalanceTrackerStats>,
}

#[derive(CandidType, Clone, Debug)]
pub struct CursorStatus {
    pub name: String,
    pub cursor_position: u64,
    pub source_count: u64,
    pub last_success_ns: u64,
    pub last_error: Option<String>,
}

#[derive(CandidType, Clone, Debug)]
pub struct BalanceTrackerStats {
    pub token: Principal,
    pub holder_count: u64,
    pub total_tracked_e8s: u64,
}

// --- Phase 6: Live query types ---

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct OhlcQuery {
    pub collateral: Principal,
    pub bucket_secs: Option<u64>,
    pub from_ts: Option<u64>,
    pub to_ts: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(CandidType, Clone, Debug, Serialize)]
pub struct OhlcCandle {
    pub timestamp_ns: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(CandidType, Clone, Debug)]
pub struct OhlcResponse {
    pub candles: Vec<OhlcCandle>,
    pub collateral: Principal,
    pub symbol: String,
    pub bucket_secs: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TwapQuery {
    pub window_secs: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct TwapEntry {
    pub collateral: Principal,
    pub symbol: String,
    pub twap_price: f64,
    pub latest_price: f64,
    pub sample_count: u32,
}

#[derive(CandidType, Clone, Debug)]
pub struct TwapResponse {
    pub entries: Vec<TwapEntry>,
    pub window_secs: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct VolatilityQuery {
    pub collateral: Principal,
    pub window_secs: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct VolatilityResponse {
    pub collateral: Principal,
    pub symbol: String,
    pub annualized_vol_pct: f64,
    pub sample_count: u32,
    pub window_secs: u64,
}

#[derive(CandidType, Clone, Debug)]
pub struct PegStatus {
    pub timestamp_ns: u64,
    pub pool_balances: Vec<u128>,
    pub virtual_price: u128,
    pub balance_ratios: Vec<f64>,
    pub max_imbalance_pct: f64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ApyQuery {
    pub window_days: Option<u32>,
}

#[derive(CandidType, Clone, Debug)]
pub struct ApyResponse {
    pub lp_apy_pct: Option<f64>,
    pub sp_apy_pct: Option<f64>,
    pub window_days: u32,
}

#[derive(CandidType, Clone, Debug)]
pub struct ProtocolSummary {
    pub timestamp_ns: u64,
    pub total_collateral_usd_e8s: u64,
    pub total_debt_e8s: u64,
    pub system_cr_bps: u32,
    pub total_vault_count: u32,
    pub circulating_supply_icusd_e8s: Option<u128>,
    pub volume_24h_e8s: u64,
    pub swap_count_24h: u32,
    pub peg: Option<PegStatus>,
    pub lp_apy_pct: Option<f64>,
    pub sp_apy_pct: Option<f64>,
    pub prices: Vec<TwapEntry>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TradeActivityQuery {
    pub window_secs: Option<u64>,
}

#[derive(CandidType, Clone, Debug)]
pub struct TradeActivityResponse {
    pub window_secs: u64,
    pub total_swaps: u32,
    pub three_pool_swaps: u32,
    pub amm_swaps: u32,
    pub total_volume_e8s: u64,
    pub total_fees_e8s: u64,
    pub unique_traders: u32,
    pub avg_trade_size_e8s: u64,
}
