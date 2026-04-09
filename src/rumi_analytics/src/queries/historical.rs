//! Read-only paginated readers over StableLogs. Pure functions, no state mutation.

use crate::storage;
use crate::types::{RangeQuery, TvlSeriesResponse, VaultSeriesResponse, StabilitySeriesResponse, HolderSeriesResponse};

pub fn get_tvl_series(query: RangeQuery) -> TvlSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    let rows = storage::daily_tvl::range(from, to, limit);

    // If we returned exactly `limit` rows, more may exist: hand back a
    // continuation cursor pointing one nanosecond past the last returned row.
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    TvlSeriesResponse { rows, next_from_ts }
}

pub fn get_vault_series(query: RangeQuery) -> VaultSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::daily_vaults::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    VaultSeriesResponse { rows, next_from_ts }
}

pub fn get_stability_series(query: RangeQuery) -> StabilitySeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::daily_stability::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    StabilitySeriesResponse { rows, next_from_ts }
}

pub fn get_holder_series(query: RangeQuery, token: candid::Principal) -> HolderSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    let icusd_ledger = crate::state::read_state(|s| s.sources.icusd_ledger);
    let rows = if token == icusd_ledger {
        storage::holders::daily_holders_icusd::range(from, to, limit)
    } else {
        storage::holders::daily_holders_3usd::range(from, to, limit)
    };

    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    HolderSeriesResponse { rows, next_from_ts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_query_defaults() {
        let q = RangeQuery::default();
        assert_eq!(q.resolved_limit(), 500);
        assert_eq!(q.resolved_from(), 0);
        assert_eq!(q.resolved_to(), u64::MAX);
    }

    #[test]
    fn range_query_limit_capped() {
        let q = RangeQuery {
            limit: Some(99_999),
            ..Default::default()
        };
        assert_eq!(q.resolved_limit(), 2000);
    }
}
