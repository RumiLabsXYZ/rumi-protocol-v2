//! Read-only paginated readers over StableLogs. Pure functions, no state mutation.

use crate::storage;
use crate::types;
use crate::types::RangeQuery;

pub fn get_tvl_series(query: RangeQuery) -> types::TvlSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();

    let rows = storage::daily_tvl::range(from, to, limit);

    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };

    types::TvlSeriesResponse { rows, next_from_ts }
}

pub fn get_vault_series(query: RangeQuery) -> types::VaultSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::daily_vaults::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::VaultSeriesResponse { rows, next_from_ts }
}

pub fn get_stability_series(query: RangeQuery) -> types::StabilitySeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::daily_stability::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::StabilitySeriesResponse { rows, next_from_ts }
}

pub fn get_holder_series(query: RangeQuery, token: candid::Principal) -> types::HolderSeriesResponse {
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

    types::HolderSeriesResponse { rows, next_from_ts }
}

pub fn get_liquidation_series(query: RangeQuery) -> types::LiquidationSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::rollups::daily_liquidations::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::LiquidationSeriesResponse { rows, next_from_ts }
}

pub fn get_swap_series(query: RangeQuery) -> types::SwapSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::rollups::daily_swaps::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::SwapSeriesResponse { rows, next_from_ts }
}

pub fn get_fee_series(query: RangeQuery) -> types::FeeSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::rollups::daily_fees::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::FeeSeriesResponse { rows, next_from_ts }
}

pub fn get_price_series(query: RangeQuery) -> types::PriceSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::fast::fast_prices::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::PriceSeriesResponse { rows, next_from_ts }
}

pub fn get_three_pool_series(query: RangeQuery) -> types::ThreePoolSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::fast::fast_3pool::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::ThreePoolSeriesResponse { rows, next_from_ts }
}

pub fn get_cycle_series(query: RangeQuery) -> types::CycleSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::hourly::hourly_cycles::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::CycleSeriesResponse { rows, next_from_ts }
}

pub fn get_fee_curve_series(query: RangeQuery) -> types::FeeCurveSeriesResponse {
    let limit = query.resolved_limit() as usize;
    let from = query.resolved_from();
    let to = query.resolved_to();
    let rows = storage::hourly::hourly_fee_curve::range(from, to, limit);
    let next_from_ts = if rows.len() == limit && limit > 0 {
        rows.last().map(|r| r.timestamp_ns.saturating_add(1))
    } else {
        None
    };
    types::FeeCurveSeriesResponse { rows, next_from_ts }
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
