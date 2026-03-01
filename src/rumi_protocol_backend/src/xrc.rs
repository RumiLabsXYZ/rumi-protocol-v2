use crate::logs::TRACE_XRC;
use crate::numeric::UsdIcp;  
use crate::state::{mutate_state, read_state};
use crate::Decimal;
use crate::Mode;
use ic_canister_log::log;
use ic_xrc_types::GetExchangeRateResult;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal_macros::dec;
use std::time::Duration;

/// How often to passively fetch ICP price from XRC (background polling).
/// Each XRC call costs ~1B cycles. At 60s = ~$58/month, at 300s = ~$12/month.
/// Price-sensitive operations will fetch on-demand if the cached price is >30s old,
/// so this is just a lazy background refresh for display/query purposes.
pub const FETCHING_ICP_RATE_INTERVAL: Duration = Duration::from_secs(300);

/// Maximum age (in nanoseconds) of a cached price before a price-sensitive
/// operation triggers an on-demand XRC fetch. Set to 30 seconds.
pub const PRICE_FRESHNESS_THRESHOLD_NANOS: u64 = 30 * 1_000_000_000;

pub async fn fetch_icp_rate() {
    let _guard = match crate::guard::FetchXrcGuard::new() {
        Some(guard) => guard,
        None => return,
    };

    match crate::management::fetch_icp_price().await {
        Ok(call_result) => match call_result {
            GetExchangeRateResult::Ok(exchange_rate_result) => {
                let rate = Decimal::from_u64(exchange_rate_result.rate).unwrap()
                    / Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals))
                        .unwrap();
                if rate < dec!(0.01) {  // Changed threshold for ICP
                    log!(
                        TRACE_XRC,
                        "[FetchPrice] Warning: ICP rate is below $0.01 switching to read-only at timestamp: {}",
                        exchange_rate_result.timestamp
                    );
                    mutate_state(|s| s.mode = Mode::ReadOnly);
                };
                log!(
                    TRACE_XRC,
                    "[FetchPrice] fetched new ICP rate: {rate} with timestamp: {}",
                    exchange_rate_result.timestamp
                );
                mutate_state(|s| {
                    let ts_nanos = exchange_rate_result.timestamp * 1_000_000_000;
                    let should_update = match s.last_icp_timestamp {
                        Some(last_ts) => last_ts < ts_nanos,
                        None => true,
                    };
                    if should_update {
                        s.set_icp_rate(UsdIcp::from(rate), Some(ts_nanos));
                    }
                });
            }
            GetExchangeRateResult::Err(error) => ic_canister_log::log!(
                TRACE_XRC,
                "[FetchPrice] failed to call XRC canister with error: {error:?}"
            ),
        },
        Err(error) => ic_canister_log::log!(
            TRACE_XRC,
            "[FetchPrice] failed to call XRC canister with error: {error}"
        ),
    }
    if let Some(last_icp_rate) = read_state(|s| s.last_icp_rate) {
        mutate_state(|s| s.update_total_collateral_ratio_and_mode(last_icp_rate));
    }
    if read_state(|s| s.mode != crate::Mode::ReadOnly) {
        crate::check_vaults();
    }
}

/// Ensures the price for the given collateral type is fresh enough for
/// a price-sensitive operation. ICP uses its own dedicated path; other
/// collateral types use the generic fetch_collateral_price.
pub async fn ensure_fresh_price_for(
    collateral_type: &candid::Principal,
) -> Result<(), crate::ProtocolError> {
    let icp_ledger = read_state(|s| s.icp_collateral_type());
    if *collateral_type == icp_ledger {
        ensure_fresh_price().await
    } else {
        let needs_refresh = read_state(|s| {
            match s.get_collateral_config(collateral_type) {
                Some(config) => match config.last_price_timestamp {
                    None => true,
                    Some(ts) => {
                        let age = ic_cdk::api::time().saturating_sub(ts);
                        age > PRICE_FRESHNESS_THRESHOLD_NANOS
                    }
                },
                None => true,
            }
        });

        if needs_refresh {
            log!(
                TRACE_XRC,
                "[ensure_fresh_price_for] Price stale for {}, fetching on-demand",
                collateral_type
            );
            crate::management::fetch_collateral_price(*collateral_type).await;
        }

        // Verify we have a price now
        let has_price = read_state(|s| {
            s.get_collateral_config(collateral_type)
                .and_then(|c| c.last_price)
                .is_some()
        });
        if has_price {
            Ok(())
        } else {
            Err(crate::ProtocolError::GenericError(format!(
                "No price available for collateral {}",
                collateral_type
            )))
        }
    }
}

/// Ensures the ICP price is fresh enough for a price-sensitive operation.
/// If the cached price is older than PRICE_FRESHNESS_THRESHOLD_NANOS (30s),
/// fetches a fresh price from XRC before returning.
/// Returns Ok(()) if a fresh-enough price is available, Err if fetch fails
/// and no cached price exists.
pub async fn ensure_fresh_price() -> Result<(), crate::ProtocolError> {
    let needs_refresh = read_state(|s| {
        match s.last_icp_timestamp {
            None => true, // No price at all, definitely need one
            Some(ts) => {
                let age = ic_cdk::api::time().saturating_sub(ts);
                age > PRICE_FRESHNESS_THRESHOLD_NANOS
            }
        }
    });

    if needs_refresh {
        log!(
            TRACE_XRC,
            "[ensure_fresh_price] Cached price is stale (>30s), fetching on-demand"
        );
        fetch_icp_rate().await;

        // After fetch, verify we actually have a price now
        read_state(|s| {
            s.check_price_not_too_old()
        })?;
    }

    Ok(())
}

/// Depeg thresholds: reject ckstable operations if the stablecoin price
/// is outside the safe band. Per HANDOFF.md: "reject if rate < $0.95 or > $1.05"
const DEPEG_LOWER_BOUND: Decimal = dec!(0.95);
const DEPEG_UPPER_BOUND: Decimal = dec!(1.05);

/// Maximum age for cached ckstable prices before re-fetching.
/// More lenient than ICP (60s vs 30s) since stablecoin prices move slowly.
const STABLE_PRICE_FRESHNESS_NANOS: u64 = 60 * 1_000_000_000;

/// Ensures the given ckstable token is not depegged before allowing an operation.
/// On-demand only — fetches from XRC if cached price is stale or missing.
/// Returns Ok(()) if the price is within the safe band [$0.95, $1.05].
pub async fn ensure_stable_not_depegged(
    token_type: &crate::StableTokenType,
) -> Result<(), crate::ProtocolError> {
    let (symbol, needs_refresh) = read_state(|s| {
        let now = ic_cdk::api::time();
        match token_type {
            crate::StableTokenType::CKUSDT => {
                let stale = match s.last_ckusdt_timestamp {
                    None => true,
                    Some(ts) => now.saturating_sub(ts) > STABLE_PRICE_FRESHNESS_NANOS,
                };
                ("USDT".to_string(), stale)
            }
            crate::StableTokenType::CKUSDC => {
                let stale = match s.last_ckusdc_timestamp {
                    None => true,
                    Some(ts) => now.saturating_sub(ts) > STABLE_PRICE_FRESHNESS_NANOS,
                };
                ("USDC".to_string(), stale)
            }
        }
    });

    if needs_refresh {
        log!(
            TRACE_XRC,
            "[ensure_stable_not_depegged] Fetching fresh {} price from XRC",
            symbol
        );

        match crate::management::fetch_stable_price(&symbol).await {
            Ok(call_result) => match call_result {
                GetExchangeRateResult::Ok(exchange_rate_result) => {
                    let rate = Decimal::from_u64(exchange_rate_result.rate).unwrap()
                        / Decimal::from_u64(
                            10_u64.pow(exchange_rate_result.metadata.decimals),
                        )
                        .unwrap();

                    log!(
                        TRACE_XRC,
                        "[ensure_stable_not_depegged] {} rate: {} at timestamp: {}",
                        symbol,
                        rate,
                        exchange_rate_result.timestamp
                    );

                    let ts_nanos = exchange_rate_result.timestamp * 1_000_000_000;
                    mutate_state(|s| match token_type {
                        crate::StableTokenType::CKUSDT => {
                            s.last_ckusdt_rate = Some(rate);
                            s.last_ckusdt_timestamp = Some(ts_nanos);
                        }
                        crate::StableTokenType::CKUSDC => {
                            s.last_ckusdc_rate = Some(rate);
                            s.last_ckusdc_timestamp = Some(ts_nanos);
                        }
                    });
                }
                GetExchangeRateResult::Err(error) => {
                    log!(
                        TRACE_XRC,
                        "[ensure_stable_not_depegged] XRC error for {}: {:?}",
                        symbol,
                        error
                    );
                    return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                        "Cannot verify {} price: XRC returned error {:?}",
                        symbol, error
                    )));
                }
            },
            Err(error) => {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] Failed to call XRC for {}: {}",
                    symbol,
                    error
                );
                return Err(crate::ProtocolError::TemporarilyUnavailable(format!(
                    "Cannot verify {} price: {}",
                    symbol, error
                )));
            }
        }
    }

    // Now check the cached price against depeg thresholds
    let rate = read_state(|s| match token_type {
        crate::StableTokenType::CKUSDT => s.last_ckusdt_rate,
        crate::StableTokenType::CKUSDC => s.last_ckusdc_rate,
    });

    match rate {
        Some(price) => {
            if price < DEPEG_LOWER_BOUND || price > DEPEG_UPPER_BOUND {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] DEPEG DETECTED: {} at ${}, outside [{}, {}]",
                    symbol,
                    price,
                    DEPEG_LOWER_BOUND,
                    DEPEG_UPPER_BOUND
                );
                Err(crate::ProtocolError::GenericError(format!(
                    "{} appears to be depegged (current price: ${:.4}). \
                     Operations with this token are suspended until the price \
                     returns to the ${:.2}–${:.2} range.",
                    symbol, price, DEPEG_LOWER_BOUND, DEPEG_UPPER_BOUND
                )))
            } else {
                log!(
                    TRACE_XRC,
                    "[ensure_stable_not_depegged] {} price ${} is within safe band",
                    symbol,
                    price
                );
                Ok(())
            }
        }
        None => Err(crate::ProtocolError::TemporarilyUnavailable(format!(
            "No {} price available. Cannot verify peg safety.",
            symbol
        ))),
    }
}