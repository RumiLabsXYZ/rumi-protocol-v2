use crate::numeric::{ICUSD, ICP};
use crate::state::read_state;
use crate::StableTokenType;
use candid::{Nat, Principal};
use ic_xrc_types::{Asset, AssetClass, GetExchangeRateRequest, GetExchangeRateResult};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_client_cdk::{CdkRuntime, ICRC1Client};
use num_traits::ToPrimitive;
use sha2::{Sha256, Digest};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use crate::log;
use crate::DEBUG;

// ─── Wave-3 ICRC transfer hygiene helpers ───
//
// Audit-driven (`audit-reports/2026-04-22-28e9896` ICRC-001..005). Two pieces:
//
//   1. `transfer_idempotent` / `transfer_from_idempotent` set a deterministic
//      `created_at_time` (derived from `op_nonce`) so the ledger can
//      deduplicate retries, treat `Duplicate { duplicate_of }` as success
//      (the previous attempt landed at that block index — not a failure),
//      and refresh the fee cache on `BadFee`.
//
//   2. `cached_fee_for` / `refresh_fee_cache` give callers a fast read of
//      the most recent ledger fee with a 10-minute TTL. The cache updates
//      automatically when an idempotent transfer comes back BadFee.

const FEE_CACHE_TTL_NS: u64 = 600_000_000_000; // 10 minutes

thread_local! {
    /// `ledger -> (fee, last_refresh_ns)`. Populated by `refresh_fee_cache`,
    /// invalidated on `BadFee`, and queried by callers that need to size a
    /// transfer (e.g., subtract the fee from the gross amount before sending).
    static LEDGER_FEE_CACHE: RefCell<BTreeMap<Principal, (u64, u64)>> =
        RefCell::new(BTreeMap::new());
}

/// Extract the `created_at_time` (nanoseconds since UNIX epoch) embedded in a
/// nonce produced by `crate::state::next_op_nonce`. The upper 64 bits hold
/// the timestamp captured at first issuance; the lower 64 bits hold a
/// monotonic counter for collision resistance.
pub fn nonce_to_created_at_time(op_nonce: u128) -> u64 {
    (op_nonce >> 64) as u64
}

/// Encode an `op_nonce` as a 16-byte big-endian memo. Useful for explorer
/// correlation and as a tie-breaker in the dedup tuple.
pub fn nonce_to_memo(op_nonce: u128) -> Memo {
    Memo::from(op_nonce.to_be_bytes().to_vec())
}

/// Read the cached fee for a ledger if it is fresh; otherwise return None.
pub fn cached_fee_for(ledger: Principal) -> Option<u64> {
    let now = ic_cdk::api::time();
    LEDGER_FEE_CACHE.with(|c| {
        let cache = c.borrow();
        cache.get(&ledger).and_then(|(fee, ts)| {
            if now.saturating_sub(*ts) < FEE_CACHE_TTL_NS {
                Some(*fee)
            } else {
                None
            }
        })
    })
}

/// Force-set the cache for a ledger (used internally on BadFee and by tests).
pub fn set_cached_fee(ledger: Principal, fee: u64) {
    LEDGER_FEE_CACHE.with(|c| {
        c.borrow_mut().insert(ledger, (fee, ic_cdk::api::time()));
    });
}

/// Query `icrc1_fee()` and update the cache. Returns the freshly-fetched fee.
pub async fn refresh_fee_cache(ledger: Principal) -> Result<u64, String> {
    let fee = get_ledger_fee(ledger).await?;
    set_cached_fee(ledger, fee);
    Ok(fee)
}

/// Convenience: return the cached fee if fresh, else fetch and cache it.
pub async fn get_or_refresh_fee(ledger: Principal) -> Result<u64, String> {
    if let Some(fee) = cached_fee_for(ledger) {
        return Ok(fee);
    }
    refresh_fee_cache(ledger).await
}

/// Idempotent ICRC-1 transfer.
///
/// `op_nonce` MUST be stable across retries of the same logical operation
/// (mint via `crate::state::next_op_nonce` once, persist alongside the
/// pending record, reuse on every retry). The `created_at_time` is derived
/// from `op_nonce` so the ledger's dedup tuple matches across retries.
///
/// Behaviour:
///   * `Ok(block)` — transfer landed at `block`.
///   * `Err(TransferError::Duplicate { duplicate_of })` is converted to
///     `Ok(duplicate_of)` — the previous attempt already landed at that
///     block, the operation succeeded (audit ICRC-003).
///   * `Err(TransferError::BadFee { expected_fee })` updates the fee cache
///     for `ledger` and propagates the error so the caller can retry with
///     the fresh fee (audit ICRC-005).
pub async fn transfer_idempotent(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    op_nonce: u128,
    memo: Option<Memo>,
) -> Result<u64, TransferError> {
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = memo.unwrap_or_else(|| nonce_to_memo(op_nonce));

    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let outer = client
        .transfer(TransferArg {
            from_subaccount,
            to,
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
            amount: Nat::from(amount),
        })
        .await;

    handle_transfer_outcome(ledger, outer)
}

/// Idempotent ICRC-2 transfer_from. Same semantics as `transfer_idempotent`
/// but for pull-based transfers (pre-approved spend).
pub async fn transfer_from_idempotent(
    ledger: Principal,
    from: Account,
    to: Account,
    amount: u128,
    op_nonce: u128,
    memo: Option<Memo>,
) -> Result<u64, TransferFromError> {
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = memo.unwrap_or_else(|| nonce_to_memo(op_nonce));

    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let outer = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: None,
            from,
            to,
            amount: Nat::from(amount),
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
        })
        .await;

    handle_transfer_from_outcome(ledger, outer)
}

fn handle_transfer_outcome(
    ledger: Principal,
    outer: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<u64, TransferError> {
    match outer {
        Ok(Ok(block)) => Ok(block.0.to_u64().unwrap_or(0)),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => {
            let block = duplicate_of.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_idempotent] ledger {} reported Duplicate; treating as success (block {})",
                ledger, block
            );
            Ok(block)
        }
        Ok(Err(TransferError::BadFee { expected_fee })) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_idempotent] ledger {} returned BadFee (expected {}), refreshing cache",
                ledger, fee
            );
            set_cached_fee(ledger, fee);
            Err(TransferError::BadFee { expected_fee })
        }
        Ok(Err(other)) => Err(other),
        Err((code, msg)) => Err(TransferError::GenericError {
            error_code: Nat::from(code.max(0) as u64),
            message: msg,
        }),
    }
}

fn handle_transfer_from_outcome(
    ledger: Principal,
    outer: Result<Result<Nat, TransferFromError>, (i32, String)>,
) -> Result<u64, TransferFromError> {
    match outer {
        Ok(Ok(block)) => Ok(block.0.to_u64().unwrap_or(0)),
        Ok(Err(TransferFromError::Duplicate { duplicate_of })) => {
            let block = duplicate_of.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_from_idempotent] ledger {} reported Duplicate; treating as success (block {})",
                ledger, block
            );
            Ok(block)
        }
        Ok(Err(TransferFromError::BadFee { expected_fee })) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_from_idempotent] ledger {} returned BadFee (expected {}), refreshing cache",
                ledger, fee
            );
            set_cached_fee(ledger, fee);
            Err(TransferFromError::BadFee { expected_fee })
        }
        Ok(Err(other)) => Err(other),
        Err((code, msg)) => Err(TransferFromError::GenericError {
            error_code: Nat::from(code.max(0) as u64),
            message: msg,
        }),
    }
}

/// Represents an error from a management canister call
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    method: String,
    reason: Reason,
}

impl CallError {
    /// Returns the name of the method that resulted in this error.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the failure reason.
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "management call '{}' failed: {}",
            self.method, self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The reason for the management call failure.
pub enum Reason {
    /// Failed to send a signature request because the local output queue is
    /// full.
    QueueIsFull,
    /// The canister does not have enough cycles to submit the request.
    OutOfCycles,
    /// The call failed with an error.
    CanisterError(String),
    /// The management canister rejected the signature request (not enough
    /// cycles, the ECDSA subnet is overloaded, etc.).
    Rejected(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueIsFull => write!(fmt, "the canister queue is full"),
            Self::OutOfCycles => write!(fmt, "the canister is out of cycles"),
            Self::CanisterError(msg) => write!(fmt, "canister error: {}", msg),
            Self::Rejected(msg) => {
                write!(fmt, "the management canister rejected the call: {}", msg)
            }
        }
    }
}

/// Query the XRC canister to retrieve the last BTC/USD price.
/// https://github.com/dfinity/exchange-rate-canister
pub async fn fetch_icp_price() -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let icp = Asset {
        symbol: "ICP".to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(), 
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: icp,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),  // Clone args for logging
        XRC_CALL_COST_CYCLES,
    )
    .await;

    // Add detailed logging
    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC error code: {:?}, message: {}", code, msg);  // Changed to {:?}
            Err(format!(
                "Error while calling XRC canister ({:?}): {:?}",  // Changed to {:?}
                code, msg
            ))
        }
    }
}

/// Fetch USDT/USD or USDC/USD price from the XRC canister.
/// Used on-demand for depeg protection on ckstable operations.
pub async fn fetch_stable_price(symbol: &str) -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let stable = Asset {
        symbol: symbol.to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(),
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: stable,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),
        XRC_CALL_COST_CYCLES,
    )
    .await;

    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_stable_price] XRC request for {}: {:?}", symbol, args);
            log!(DEBUG, "[fetch_stable_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_stable_price] XRC error for {}: {:?}, message: {}", symbol, code, msg);
            Err(format!(
                "Error fetching {} price from XRC ({:?}): {:?}",
                symbol, code, msg
            ))
        }
    }
}

/// Minimal subset of WaterNeuron's CanisterInfo response.
/// Candid deserialization ignores unknown fields, so we only define what we need.
#[derive(candid::CandidType, serde::Deserialize)]
struct LstCanisterInfo {
    exchange_rate: u64,
}

/// Generic price fetch for any collateral type using its PriceSource config.
/// Routes to XRC, CoinGecko HTTPS outcall, or LstWrapped depending on config.
pub async fn fetch_collateral_price(collateral_type: Principal) {
    use crate::state::{mutate_state, PriceSource, XrcAssetClass};
    use ic_canister_log::log;
    use crate::logs::TRACE_XRC;
    use rust_decimal::prelude::FromPrimitive;

    let price_source = read_state(|s| {
        s.get_collateral_config(&collateral_type).map(|c| c.price_source.clone())
    });

    let price_source = match price_source {
        Some(ps) => ps,
        None => {
            log!(TRACE_XRC, "[fetch_collateral_price] No config for {}", collateral_type);
            return;
        }
    };

    // CoinGecko variant uses HTTPS outcalls — completely separate path from XRC
    if let PriceSource::CoinGecko { ref coin_id, ref vs_currency } = price_source {
        let result = fetch_coingecko_price(coin_id, vs_currency).await;
        match result {
            Some(price) => {
                let ts_nanos = ic_cdk::api::time();
                log!(
                    TRACE_XRC,
                    "[fetch_collateral_price] CoinGecko {} price: {} at {}",
                    coin_id, price, ts_nanos
                );
                let should_update = read_state(|s| {
                    s.get_collateral_config(&collateral_type)
                        .map(|c| match c.last_price_timestamp {
                            Some(last_ts) => last_ts < ts_nanos,
                            None => true,
                        })
                        .unwrap_or(false)
                });
                if !should_update {
                    return;
                }
                // Wave-5 LIQ-007: gate every accepted price through the sanity band
                // (rejects single outliers, accepts after N consecutive confirmations).
                let accepted = mutate_state(|s| s.check_price_sanity_band(&collateral_type, price));
                if !accepted {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] rejecting outlier CoinGecko price {} for {}; awaiting confirmation",
                        price, coin_id
                    );
                    return;
                }
                mutate_state(|s| {
                    if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
                        config.last_price = Some(price);
                        config.last_price_timestamp = Some(ts_nanos);
                        if let Some(price_dec) = rust_decimal::Decimal::from_f64(price) {
                            crate::event::record_price_update(collateral_type, price_dec, ts_nanos);
                        }
                    }
                });
            }
            None => {
                log!(TRACE_XRC, "[fetch_collateral_price] CoinGecko failed for {}", coin_id);
            }
        }
        return;
    }

    // XRC-based path (Xrc and LstWrapped variants)
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let (base_asset, base_asset_class, quote_asset, quote_asset_class) = match &price_source {
        PriceSource::Xrc { base_asset, base_asset_class, quote_asset, quote_asset_class }
        | PriceSource::LstWrapped { base_asset, base_asset_class, quote_asset, quote_asset_class, .. } => {
            (base_asset.clone(), base_asset_class.clone(), quote_asset.clone(), quote_asset_class.clone())
        }
        PriceSource::CoinGecko { .. } => unreachable!(), // handled above
    };

    let base = Asset {
        symbol: base_asset.clone(),
        class: match base_asset_class {
            XrcAssetClass::Cryptocurrency => AssetClass::Cryptocurrency,
            XrcAssetClass::FiatCurrency => AssetClass::FiatCurrency,
        },
    };
    let quote = Asset {
        symbol: quote_asset.clone(),
        class: match quote_asset_class {
            XrcAssetClass::Cryptocurrency => AssetClass::Cryptocurrency,
            XrcAssetClass::FiatCurrency => AssetClass::FiatCurrency,
        },
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: base,
        quote_asset: quote,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),
        XRC_CALL_COST_CYCLES,
    )
    .await;

    let underlying_rate = match res_xrc {
        Ok((GetExchangeRateResult::Ok(exchange_rate_result),)) => {
            let rate = rust_decimal::Decimal::from_u64(exchange_rate_result.rate).unwrap()
                / rust_decimal::Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals)).unwrap();

            log!(
                TRACE_XRC,
                "[fetch_collateral_price] {} rate: {} at timestamp: {}",
                base_asset, rate, exchange_rate_result.timestamp
            );

            Some((rate, exchange_rate_result.timestamp * 1_000_000_000))
        }
        Ok((GetExchangeRateResult::Err(error),)) => {
            log!(TRACE_XRC, "[fetch_collateral_price] XRC error for {}: {:?}", base_asset, error);
            None
        }
        Err((code, msg)) => {
            log!(TRACE_XRC, "[fetch_collateral_price] Call error for {}: {:?} {}", base_asset, code, msg);
            None
        }
    };

    let Some((rate, ts_nanos)) = underlying_rate else { return };

    // For LstWrapped, multiply by the redemption rate and apply haircut
    let final_rate = match &price_source {
        PriceSource::Xrc { .. } => rate,
        PriceSource::LstWrapped { rate_canister_id, rate_method, haircut, .. } => {
            let rate_result: Result<(LstCanisterInfo,), _> =
                ic_cdk::call(*rate_canister_id, rate_method.as_str(), ()).await;

            match rate_result {
                Ok((info,)) => {
                    if info.exchange_rate == 0 {
                        log!(TRACE_XRC, "[fetch_collateral_price] LstWrapped exchange_rate is 0, skipping");
                        return;
                    }
                    let multiplier = rust_decimal::Decimal::from(crate::E8S)
                        / rust_decimal::Decimal::from(info.exchange_rate);
                    let haircut_dec = rust_decimal::Decimal::from_f64(*haircut)
                        .unwrap_or(rust_decimal::Decimal::ZERO);
                    let adjusted = rate * multiplier * (rust_decimal::Decimal::ONE - haircut_dec);

                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] LstWrapped final: {} (underlying={}, multiplier={}, haircut={})",
                        adjusted, rate, multiplier, haircut
                    );
                    adjusted
                }
                Err((code, msg)) => {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] LstWrapped rate canister error: {:?} {}",
                        code, msg
                    );
                    return;
                }
            }
        }
        PriceSource::CoinGecko { .. } => unreachable!(),
    };

    let should_update = read_state(|s| {
        s.get_collateral_config(&collateral_type)
            .map(|c| match c.last_price_timestamp {
                Some(last_ts) => last_ts < ts_nanos,
                None => true,
            })
            .unwrap_or(false)
    });
    if !should_update {
        return;
    }

    // Wave-5 LIQ-007: gate every accepted price through the sanity band.
    let final_rate_f64 = match final_rate.to_f64() {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => {
            log!(
                TRACE_XRC,
                "[fetch_collateral_price] {}: dropping non-positive/non-finite final rate {}",
                base_asset, final_rate
            );
            return;
        }
    };
    let accepted = mutate_state(|s| s.check_price_sanity_band(&collateral_type, final_rate_f64));
    if !accepted {
        log!(
            TRACE_XRC,
            "[fetch_collateral_price] rejecting outlier {} rate {} for {}; awaiting confirmation",
            base_asset, final_rate_f64, collateral_type
        );
        return;
    }

    mutate_state(|s| {
        if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
            config.last_price = Some(final_rate_f64);
            config.last_price_timestamp = Some(ts_nanos);
            crate::event::record_price_update(collateral_type, final_rate, ts_nanos);
        }
    });
}

/// Fetch a token price from the CoinGecko simple/price API via HTTPS outcall.
/// Returns the price as f64, or None on failure.
async fn fetch_coingecko_price(coin_id: &str, vs_currency: &str) -> Option<f64> {
    use ic_cdk::api::management_canister::http_request::{
        http_request, CanisterHttpRequestArgument, HttpHeader, HttpMethod,
        TransformContext,
    };
    use ic_canister_log::log;
    use crate::logs::TRACE_XRC;

    // Response body is small (~50 bytes) but headers can be large (~2-3 KB).
    // IC counts headers + body against this limit before transform strips headers.
    const MAX_RESPONSE_BYTES: u64 = 4096;
    // HTTPS outcall cost: base 49_140_000 + 5_200 per request byte + 10_400 per response byte
    // Plus per-node scaling. 100M gives comfortable headroom for 13-node subnets.
    const OUTCALL_CYCLES: u128 = 100_000_000;

    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies={}",
        coin_id, vs_currency
    );

    let request = CanisterHttpRequestArgument {
        url,
        max_response_bytes: Some(MAX_RESPONSE_BYTES),
        method: HttpMethod::GET,
        headers: vec![
            HttpHeader {
                name: "Accept".to_string(),
                value: "application/json".to_string(),
            },
        ],
        body: None,
        transform: Some(TransformContext::from_name(
            "coingecko_transform".to_string(),
            vec![],
        )),
    };

    let result = http_request(request, OUTCALL_CYCLES).await;

    match result {
        Ok((response,)) => {
            let status = response.status.0.to_u64().unwrap_or(0);
            if status != 200 {
                log!(TRACE_XRC, "[coingecko] HTTP {} for {}", status, coin_id);
                return None;
            }

            let body = String::from_utf8(response.body).ok()?;
            // Response format: {"bob-3":{"usd":0.0957}}
            let json: serde_json::Value = serde_json::from_str(&body).ok()?;
            let price = json.get(coin_id)?.get(vs_currency)?.as_f64()?;

            if price <= 0.0 {
                log!(TRACE_XRC, "[coingecko] Non-positive price {} for {}", price, coin_id);
                return None;
            }

            Some(price)
        }
        Err((code, msg)) => {
            log!(TRACE_XRC, "[coingecko] Outcall error for {}: {:?} {}", coin_id, code, msg);
            None
        }
    }
}

pub async fn mint_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

pub async fn transfer_icusd_from(amount: ICUSD, caller: Principal) -> Result<u64, TransferFromError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: caller, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}


/// Thin wrapper around generic transfer_collateral_from for ICP. One-shot
/// callers; retry-loop callers must use `transfer_collateral_from_with_nonce`.
pub async fn transfer_icp_from(amount: ICP, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral_from(amount.to_u64(), caller, ledger).await
}

/// Thin wrapper around generic transfer_collateral for ICP. One-shot callers;
/// retry-loop callers must use `transfer_collateral_with_nonce`.
pub async fn transfer_icp(amount: ICP, to: Principal) -> Result<u64, TransferError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral(amount.to_u64(), to, ledger).await
}

pub async fn transfer_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

/// Idempotent icUSD transfer with a caller-supplied op_nonce. Wave-4 ICC-007:
/// used by the durable refund queue so retries reuse the same dedup tuple at
/// the icUSD ledger across canister upgrades.
pub async fn transfer_icusd_with_nonce(amount: ICUSD, to: Principal, op_nonce: u128) -> Result<u64, TransferError> {
    let ledger = crate::state::read_state(|s| s.icusd_ledger_principal);
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

/// Query the ICRC-1 transfer fee for a given ledger canister.
pub async fn get_ledger_fee(ledger: Principal) -> Result<u64, String> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let fee = client.fee().await.map_err(|e| format!("icrc1_fee call failed: {:?}", e))?;
    Ok(fee.0.to_u64().unwrap_or(0))
}

/// Generic collateral transfer: move tokens from the protocol canister to a recipient.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
///
/// One-shot variant: mints a fresh `op_nonce` per call. Use this for
/// caller-initiated transfers that don't have a persistent retry record.
/// For pending-transfer retry loops, use `transfer_collateral_with_nonce` and
/// pass the nonce stored on the pending entry.
pub async fn transfer_collateral(amount: u64, to: Principal, ledger: Principal) -> Result<u64, TransferError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    transfer_collateral_with_nonce(amount, to, ledger, op_nonce).await
}

/// Idempotent collateral transfer with a caller-supplied nonce. Retry-loop
/// callers (process_pending_transfer, try_process_pending_transfers_immediate,
/// schedule_transfer_retry) must persist the nonce alongside the pending entry
/// and pass the same value on every retry so the ledger deduplicates.
pub async fn transfer_collateral_with_nonce(
    amount: u64,
    to: Principal,
    ledger: Principal,
    op_nonce: u128,
) -> Result<u64, TransferError> {
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

/// Generic collateral transfer_from: pull tokens from a user into the protocol canister.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
pub async fn transfer_collateral_from(amount: u64, from: Principal, ledger: Principal) -> Result<u64, TransferFromError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: from, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

/// Transfer ckUSDT or ckUSDC from a user to the protocol (for vault repayment/liquidation)
/// Amount is in e6s (6-decimal stable token units)
pub async fn transfer_stable_from(token_type: StableTokenType, amount_e6s: u64, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger_principal = match token_type {
        StableTokenType::CKUSDT => read_state(|s| s.ckusdt_ledger_principal),
        StableTokenType::CKUSDC => read_state(|s| s.ckusdc_ledger_principal),
    }.ok_or_else(|| TransferFromError::GenericError {
        error_code: Nat::from(0u64),
        message: format!("{:?} ledger not configured", token_type),
    })?;

    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger_principal,
        Account { owner: caller, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
        amount_e6s as u128,
        op_nonce,
        None,
    )
    .await
}

/// Query the ICRC-1 balance of the protocol canister on any token ledger.
pub async fn get_token_balance(ledger: Principal) -> Result<u64, String> {
    let protocol_id = ic_cdk::id();
    let result: Result<(Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (Account {
            owner: protocol_id,
            subaccount: None,
        },),
    )
    .await;
    match result {
        Ok((balance,)) => Ok(balance.0.to_u64().unwrap_or(0)),
        Err((code, msg)) => Err(format!("icrc1_balance_of failed: {:?} {}", code, msg)),
    }
}

// ─── Protocol 3USD reserves ───

/// Deterministic subaccount for protocol-held 3USD reserves from SP liquidations.
pub fn protocol_3usd_reserves_subaccount() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"protocol_3usd_reserves");
    hasher.finalize().into()
}

/// Pull 3USD from the stability pool into the protocol's reserves subaccount via ICRC-2.
/// The SP must have approved this canister to spend `amount` on `ledger` beforehand.
pub async fn transfer_3usd_to_reserves(
    ledger: Principal,
    from: Principal,
    amount: u64,
) -> Result<u64, TransferFromError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: from, subaccount: None },
        Account {
            owner: protocol_id,
            subaccount: Some(protocol_3usd_reserves_subaccount()),
        },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

// ─── Push-deposit helpers (Oisy wallet integration) ───

/// Compute a deterministic deposit subaccount for a given caller.
/// Subaccount = SHA-256(b"rumi-deposit" || caller.as_slice())
pub fn compute_deposit_subaccount(caller: &Principal) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rumi-deposit");
    hasher.update(caller.as_slice());
    hasher.finalize().into()
}

/// Return the deposit Account for a caller. The account is owned by the
/// backend canister with a caller-specific subaccount.
pub fn get_deposit_account_for(caller: &Principal) -> Account {
    Account {
        owner: ic_cdk::id(),
        subaccount: Some(compute_deposit_subaccount(caller)),
    }
}

/// Query the ICRC-1 balance of a specific account on a ledger.
pub async fn get_balance_of(account: Account, ledger: Principal) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (account,),
    )
    .await;
    match result {
        Ok((balance,)) => Ok(balance.0.to_u64().unwrap_or(0)),
        Err((code, msg)) => Err(format!("icrc1_balance_of failed: {:?} {}", code, msg)),
    }
}

/// Sweep funds from a deposit subaccount into the protocol's main account.
/// Returns (amount_received, sweep_block_index) where amount is balance minus ledger fee.
pub async fn sweep_deposit(
    caller: &Principal,
    ledger: Principal,
    ledger_fee: u64,
) -> Result<(u64, u64), String> {
    let subaccount = compute_deposit_subaccount(caller);
    let deposit_account = Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    };

    // Read how much is sitting in the deposit subaccount
    let balance = get_balance_of(deposit_account, ledger).await?;

    if balance == 0 {
        return Err("No deposit found in subaccount".to_string());
    }

    if balance <= ledger_fee {
        return Err(format!(
            "Deposit balance ({}) is not enough to cover the ledger fee ({})",
            balance, ledger_fee
        ));
    }

    let transfer_amount = balance - ledger_fee;
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());

    let block_index_u64 = transfer_idempotent(
        ledger,
        Some(subaccount),
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        transfer_amount as u128,
        op_nonce,
        None,
    )
    .await
    .map_err(|e| format!("sweep transfer error: {:?}", e))?;

    log!(DEBUG,
        "[sweep_deposit] Swept {} from subaccount for {} on ledger {} (block {})",
        transfer_amount, caller, ledger, block_index_u64
    );

    Ok((transfer_amount, block_index_u64))
}

/// Approve a spender to transfer icUSD from the protocol canister.
/// Used by interest distribution to approve the 3pool for `donate`.
///
/// Sets `created_at_time` from a fresh nonce so the ledger can dedup, and
/// treats `ApproveError::Duplicate { duplicate_of }` as success (the approve
/// already landed at that block — same effective allowance).
pub async fn approve_icusd(spender: Principal, amount: u64) -> Result<u64, ApproveError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = nonce_to_memo(op_nonce);

    let result: Result<(Result<Nat, ApproveError>,), _> = ic_cdk::call(
        ledger,
        "icrc2_approve",
        (ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: spender, subaccount: None },
            amount: Nat::from(amount),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
        },),
    ).await;
    match result {
        Ok((Ok(block_index),)) => Ok(block_index.0.to_u64().unwrap_or(0)),
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => {
            log!(DEBUG,
                "[approve_icusd] ledger {} reported Duplicate; treating as success (block {})",
                ledger, duplicate_of
            );
            Ok(duplicate_of.0.to_u64().unwrap_or(0))
        }
        Ok((Err(ApproveError::BadFee { expected_fee }),)) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            set_cached_fee(ledger, fee);
            Err(ApproveError::BadFee { expected_fee })
        }
        Ok((Err(e),)) => Err(e),
        Err((code, msg)) => Err(ApproveError::GenericError {
            error_code: Nat::from(code as u64),
            message: msg,
        }),
    }
}
