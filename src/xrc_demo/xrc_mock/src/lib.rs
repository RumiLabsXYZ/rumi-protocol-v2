/// Standalone XRC mock canister for pocket_ic integration tests.
///
/// Accepts init args matching the `MockXRC` struct in pocket_ic_tests.rs:
///   - rates: HashMap<String, u64>  (e.g., "ICP/USD" => 1_000_000_000 for $10.00 in e8s)
///
/// Exposes `get_exchange_rate` (as update, matching real XRC canister).
///
/// Build:
///   cargo build --target wasm32-unknown-unknown --release -p xrc-mock-canister
///   cp target/wasm32-unknown-unknown/release/xrc_mock_canister.wasm src/xrc_demo/xrc/xrc.wasm

use candid::{CandidType, Deserialize};
use ic_xrc_types::{
    ExchangeRate, ExchangeRateMetadata, GetExchangeRateRequest,
    GetExchangeRateResult,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Init payload — must match what pocket_ic_tests.rs sends via `prepare_mock_xrc()`.
#[derive(CandidType, Deserialize, Debug, Clone)]
struct MockXRC {
    rates: HashMap<String, u64>,
}

thread_local! {
    static RATES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

#[ic_cdk_macros::init]
fn init(args: MockXRC) {
    RATES.with(|r| {
        *r.borrow_mut() = args.rates;
    });
}

/// Matches the real XRC canister's interface.
/// PocketIC tests call this to get the ICP/USD price.
#[ic_cdk_macros::update]
async fn get_exchange_rate(request: GetExchangeRateRequest) -> GetExchangeRateResult {
    let base = request.base_asset.symbol.to_uppercase();
    let quote = request.quote_asset.symbol.to_uppercase();
    let key = format!("{}/{}", base, quote);

    let rate_opt = RATES.with(|r| r.borrow().get(&key).cloned());

    match rate_opt {
        Some(rate) => {
            let timestamp = ic_cdk::api::time() / 1_000_000_000;
            GetExchangeRateResult::Ok(ExchangeRate {
                base_asset: request.base_asset,
                quote_asset: request.quote_asset,
                timestamp,
                rate,
                metadata: ExchangeRateMetadata {
                    decimals: 8,
                    // Production XRC always aggregates >= 3 CEX sources for major
                    // assets. Wave-14a CDP-14 enforces this floor; mock must match.
                    base_asset_num_queried_sources: 3,
                    base_asset_num_received_rates: 3,
                    quote_asset_num_queried_sources: 3,
                    quote_asset_num_received_rates: 3,
                    standard_deviation: 0,
                    forex_timestamp: None,
                },
            })
        }
        None => GetExchangeRateResult::Err(
            ic_xrc_types::ExchangeRateError::CryptoBaseAssetNotFound,
        ),
    }
}

/// Allow tests to dynamically update rates.
#[ic_cdk_macros::update]
async fn set_exchange_rate(base: String, quote: String, rate: u64) {
    let key = format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
    RATES.with(|r| {
        r.borrow_mut().insert(key, rate);
    });
}
