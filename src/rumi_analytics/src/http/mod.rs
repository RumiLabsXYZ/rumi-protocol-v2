//! HTTP request handler. Runs in query context: never makes inter-canister
//! calls. All values are served from cached state in SlimState which the 60s
//! pull cycle keeps fresh.

mod csv;
mod metrics;

use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};

use crate::state;
use crate::storage;

pub fn http_request(req: HttpRequest) -> HttpResponse {
    let path = req.url.split('?').next().unwrap_or("");
    match path {
        "/api/supply" => supply_icusd_f64(),
        "/api/supply/raw" => supply_icusd_raw(),
        "/api/health" => health_json(),
        "/api/series/tvl" => csv_response(csv::tvl_to_csv(&storage::daily_tvl::range(0, u64::MAX, 10_000))),
        "/api/series/vaults" => csv_response(csv::vaults_to_csv(&storage::daily_vaults::range(0, u64::MAX, 10_000))),
        "/api/series/stability" => csv_response(csv::stability_to_csv(&storage::daily_stability::range(0, u64::MAX, 10_000))),
        "/api/series/swaps" => csv_response(csv::swaps_to_csv(&storage::rollups::daily_swaps::range(0, u64::MAX, 10_000))),
        "/api/series/liquidations" => csv_response(csv::liquidations_to_csv(&storage::rollups::daily_liquidations::range(0, u64::MAX, 10_000))),
        "/api/series/fees" => csv_response(csv::fees_to_csv(&storage::rollups::daily_fees::range(0, u64::MAX, 10_000))),
        "/api/series/prices" => csv_response(csv::fast_prices_to_csv(&storage::fast::fast_prices::range(0, u64::MAX, 10_000))),
        "/metrics" => {
            let body = metrics::render();
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                .with_body_and_content_length(body)
                .build()
        }
        _ => HttpResponseBuilder::not_found().build(),
    }
}

fn csv_response(body: String) -> HttpResponse {
    HttpResponseBuilder::ok()
        .header("Content-Type", "text/csv; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .with_body_and_content_length(body)
        .build()
}

fn health_json() -> HttpResponse {
    let last_daily = state::read_state(|s| s.last_daily_snapshot_ns);
    let last_pull = state::read_state(|s| s.last_pull_cycle_ns).unwrap_or(0);
    let ec = state::read_state(|s| s.error_counters.clone());
    let now = ic_cdk::api::time();

    let body = serde_json::json!({
        "status": "ok",
        "canister_time_ns": now,
        "last_daily_snapshot_ns": last_daily,
        "last_pull_cycle_ns": last_pull,
        "error_counters": {
            "backend": ec.backend,
            "icusd_ledger": ec.icusd_ledger,
            "three_pool": ec.three_pool,
            "stability_pool": ec.stability_pool,
            "amm": ec.amm,
        },
        "storage_rows": {
            "daily_tvl": storage::daily_tvl::len(),
            "daily_vaults": storage::daily_vaults::len(),
            "evt_swaps": storage::events::evt_swaps::len(),
            "evt_liquidations": storage::events::evt_liquidations::len(),
            "fast_prices": storage::fast::fast_prices::len(),
        }
    });

    HttpResponseBuilder::ok()
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .with_body_and_content_length(body.to_string())
        .build()
}

fn supply_icusd_f64() -> HttpResponse {
    let cached = state::read_state(|s| s.circulating_supply_icusd_e8s);
    match cached {
        Some(e8s) => {
            let f = (e8s as f64) / 1e8;
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain; charset=utf-8")
                .with_body_and_content_length(format!("{:.8}", f))
                .build()
        }
        None => HttpResponseBuilder::server_error("supply not yet cached").build(),
    }
}

fn supply_icusd_raw() -> HttpResponse {
    let cached = state::read_state(|s| s.circulating_supply_icusd_e8s);
    match cached {
        Some(e8s) => HttpResponseBuilder::ok()
            .header("Content-Type", "text/plain; charset=utf-8")
            .with_body_and_content_length(e8s.to_string())
            .build(),
        None => HttpResponseBuilder::server_error("supply not yet cached").build(),
    }
}
