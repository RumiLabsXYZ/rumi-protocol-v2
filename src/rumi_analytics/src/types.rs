//! Shared candid types for analytics queries and responses.

use candid::CandidType;
use serde::Deserialize;

use crate::storage::DailyTvlRow;

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
