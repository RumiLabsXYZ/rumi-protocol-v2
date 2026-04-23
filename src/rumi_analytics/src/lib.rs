//! rumi_analytics canister - Phase 1.
//! See docs/plans/2026-04-07-rumi-analytics-design.md.

use candid::Principal;

mod state;
mod http;
mod storage;
mod collectors;
mod sources;
mod queries;
mod timers;
mod tailing;
mod types;

use crate::storage::{SlimState, SourceCanisterIds};

#[derive(candid::CandidType, candid::Deserialize)]
pub struct InitArgs {
    pub admin: Principal,
    pub backend: Principal,
    pub icusd_ledger: Principal,
    pub three_pool: Principal,
    pub stability_pool: Principal,
    pub amm: Principal,
}

#[ic_cdk_macros::init]
fn init(args: InitArgs) {
    let s = SlimState {
        admin: args.admin,
        sources: SourceCanisterIds {
            backend: args.backend,
            icusd_ledger: args.icusd_ledger,
            three_pool: args.three_pool,
            stability_pool: args.stability_pool,
            amm: args.amm,
        },
        ..SlimState::default()
    };
    storage::set_slim(s);
    state::hydrate_from_slim();
    timers::setup_timers();
}

#[ic_cdk_macros::pre_upgrade]
fn pre_upgrade() {
    state::snapshot_slim_to_cell();
}

#[ic_cdk_macros::post_upgrade]
fn post_upgrade() {
    state::hydrate_from_slim();
    timers::setup_timers();
}

#[ic_cdk_macros::query]
fn ping() -> &'static str {
    "rumi_analytics ok"
}

#[ic_cdk_macros::query]
fn get_admin() -> Principal {
    state::read_state(|s| s.admin)
}

#[ic_cdk_macros::query]
fn get_tvl_series(query: types::RangeQuery) -> types::TvlSeriesResponse {
    queries::historical::get_tvl_series(query)
}

#[ic_cdk_macros::query]
fn get_vault_series(query: types::RangeQuery) -> types::VaultSeriesResponse {
    queries::historical::get_vault_series(query)
}

#[ic_cdk_macros::query]
fn get_stability_series(query: types::RangeQuery) -> types::StabilitySeriesResponse {
    queries::historical::get_stability_series(query)
}

#[ic_cdk_macros::query]
fn http_request(req: ic_canisters_http_types::HttpRequest) -> ic_canisters_http_types::HttpResponse {
    http::http_request(req)
}

#[ic_cdk_macros::query]
fn get_holder_series(query: types::RangeQuery, token: Principal) -> types::HolderSeriesResponse {
    queries::historical::get_holder_series(query, token)
}

#[ic_cdk_macros::query]
fn get_liquidation_series(query: types::RangeQuery) -> types::LiquidationSeriesResponse {
    queries::historical::get_liquidation_series(query)
}

#[ic_cdk_macros::query]
fn get_swap_series(query: types::RangeQuery) -> types::SwapSeriesResponse {
    queries::historical::get_swap_series(query)
}

#[ic_cdk_macros::query]
fn get_fee_series(query: types::RangeQuery) -> types::FeeSeriesResponse {
    queries::historical::get_fee_series(query)
}

#[ic_cdk_macros::query]
fn get_price_series(query: types::RangeQuery) -> types::PriceSeriesResponse {
    queries::historical::get_price_series(query)
}

#[ic_cdk_macros::query]
fn get_three_pool_series(query: types::RangeQuery) -> types::ThreePoolSeriesResponse {
    queries::historical::get_three_pool_series(query)
}

#[ic_cdk_macros::query]
fn get_cycle_series(query: types::RangeQuery) -> types::CycleSeriesResponse {
    queries::historical::get_cycle_series(query)
}

#[ic_cdk_macros::query]
fn get_fee_curve_series(query: types::RangeQuery) -> types::FeeCurveSeriesResponse {
    queries::historical::get_fee_curve_series(query)
}

#[ic_cdk_macros::query]
fn get_ohlc(query: types::OhlcQuery) -> types::OhlcResponse {
    queries::live::get_ohlc(query)
}

#[ic_cdk_macros::query]
fn get_twap(query: types::TwapQuery) -> types::TwapResponse {
    queries::live::get_twap(query)
}

#[ic_cdk_macros::query]
fn get_volatility(query: types::VolatilityQuery) -> types::VolatilityResponse {
    queries::live::get_volatility(query)
}

#[ic_cdk_macros::query]
fn get_peg_status() -> Option<types::PegStatus> {
    queries::live::get_peg_status()
}

#[ic_cdk_macros::query]
fn get_apys(query: types::ApyQuery) -> types::ApyResponse {
    queries::live::get_apys(query)
}

#[ic_cdk_macros::query]
fn get_protocol_summary() -> types::ProtocolSummary {
    queries::live::get_protocol_summary()
}

#[ic_cdk_macros::query]
fn get_top_holders(query: types::TopHoldersQuery) -> types::TopHoldersResponse {
    queries::live::get_top_holders(query)
}

#[ic_cdk_macros::query]
fn get_top_counterparties(query: types::TopCounterpartiesQuery) -> types::TopCounterpartiesResponse {
    queries::live::get_top_counterparties(query)
}

#[ic_cdk_macros::query]
fn get_top_sp_depositors(query: types::TopSpDepositorsQuery) -> types::TopSpDepositorsResponse {
    queries::live::get_top_sp_depositors(query)
}

#[ic_cdk_macros::query]
fn get_admin_event_breakdown(query: types::AdminEventBreakdownQuery) -> types::AdminEventBreakdownResponse {
    queries::live::get_admin_event_breakdown(query)
}

#[ic_cdk_macros::query]
fn get_trade_activity(query: types::TradeActivityQuery) -> types::TradeActivityResponse {
    queries::live::get_trade_activity(query)
}

#[ic_cdk_macros::query]
fn get_token_flow(query: types::TokenFlowQuery) -> types::TokenFlowResponse {
    queries::flow::get_token_flow(query)
}

#[ic_cdk_macros::query]
fn get_pool_routes(query: types::PoolRoutesQuery) -> types::PoolRoutesResponse {
    queries::flow::get_pool_routes(query)
}

#[ic_cdk_macros::query]
fn get_address_value_series(query: types::AddressValueSeriesQuery) -> types::AddressValueSeriesResponse {
    queries::address_value::get_address_value_series(query)
}

#[ic_cdk_macros::query]
fn get_collector_health() -> types::CollectorHealth {
    use storage::cursors;

    let cursor_names: &[(u8, &str, fn() -> u64)] = &[
        (cursors::CURSOR_ID_BACKEND_EVENTS, "backend_events", cursors::backend_events::get),
        (cursors::CURSOR_ID_3POOL_SWAPS, "3pool_swaps", cursors::three_pool_swaps::get),
        (cursors::CURSOR_ID_3POOL_LIQUIDITY, "3pool_liquidity", cursors::three_pool_liquidity::get),
        (cursors::CURSOR_ID_3POOL_BLOCKS, "3pool_blocks", cursors::three_pool_blocks::get),
        (cursors::CURSOR_ID_AMM_SWAPS, "amm_swaps", cursors::amm_swaps::get),
        (cursors::CURSOR_ID_ICUSD_BLOCKS, "icusd_blocks", cursors::icusd_blocks::get),
    ];

    let (last_success_map, last_error_map, source_count_map, backfill_icusd, backfill_3usd, last_pull_ns, error_counters, icusd_ledger, three_pool) =
        state::read_state(|s| (
            s.cursor_last_success.clone().unwrap_or_default(),
            s.cursor_last_error.clone().unwrap_or_default(),
            s.cursor_source_counts.clone().unwrap_or_default(),
            s.backfill_active_icusd.unwrap_or(false),
            s.backfill_active_3usd.unwrap_or(false),
            s.last_pull_cycle_ns.unwrap_or(0),
            s.error_counters.clone(),
            s.sources.icusd_ledger,
            s.sources.three_pool,
        ));

    let cursors: Vec<types::CursorStatus> = cursor_names.iter().map(|(id, name, get_fn)| {
        types::CursorStatus {
            name: name.to_string(),
            cursor_position: get_fn(),
            source_count: source_count_map.get(id).copied().unwrap_or(0),
            last_success_ns: last_success_map.get(id).copied().unwrap_or(0),
            last_error: last_error_map.get(id).cloned(),
        }
    }).collect();

    let mut backfill_active = Vec::new();
    if backfill_icusd { backfill_active.push(icusd_ledger); }
    if backfill_3usd { backfill_active.push(three_pool); }

    let balance_tracker_stats = vec![
        types::BalanceTrackerStats {
            token: icusd_ledger,
            holder_count: storage::balance_tracker::holder_count(storage::balance_tracker::Token::IcUsd),
            total_tracked_e8s: storage::balance_tracker::total_supply_tracked(storage::balance_tracker::Token::IcUsd),
        },
        types::BalanceTrackerStats {
            token: three_pool,
            holder_count: storage::balance_tracker::holder_count(storage::balance_tracker::Token::ThreeUsd),
            total_tracked_e8s: storage::balance_tracker::total_supply_tracked(storage::balance_tracker::Token::ThreeUsd),
        },
    ];

    types::CollectorHealth {
        cursors,
        error_counters,
        backfill_active,
        last_pull_cycle_ns: last_pull_ns,
        balance_tracker_stats,
    }
}

#[ic_cdk_macros::update]
fn start_backfill(token: Principal) -> String {
    let admin = state::read_state(|s| s.admin);
    let caller = ic_cdk::caller();
    if caller != admin {
        return format!("unauthorized: caller {} is not admin", caller);
    }

    let (icusd_ledger, three_pool) = state::read_state(|s| (s.sources.icusd_ledger, s.sources.three_pool));

    if token == icusd_ledger {
        state::mutate_state(|s| s.backfill_active_icusd = Some(true));
        "backfill started for icUSD".to_string()
    } else if token == three_pool {
        state::mutate_state(|s| s.backfill_active_3usd = Some(true));
        "backfill started for 3USD".to_string()
    } else {
        format!("unknown token: {}", token)
    }
}

ic_cdk::export_candid!();
