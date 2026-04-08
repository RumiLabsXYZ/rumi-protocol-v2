//! HTTP request handler. Runs in query context: never makes inter-canister
//! calls. All values are served from cached state in SlimState which the 60s
//! pull cycle keeps fresh.

use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};

use crate::state;

pub fn http_request(req: HttpRequest) -> HttpResponse {
    let path = req.url.split('?').next().unwrap_or("");
    match path {
        "/api/supply" => supply_icusd_f64(),
        "/api/supply/raw" => supply_icusd_raw(),
        _ => HttpResponseBuilder::not_found().build(),
    }
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
