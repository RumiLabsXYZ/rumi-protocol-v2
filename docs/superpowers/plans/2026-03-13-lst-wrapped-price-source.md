# LstWrapped PriceSource Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `PriceSource::LstWrapped` variant that prices LSTs by composing XRC price × redemption rate × (1 - haircut), then use it to add nICP as a collateral type.

**Architecture:** Extend the `PriceSource` enum with a new `LstWrapped` variant. Modify `fetch_collateral_price()` to handle it by fetching ICP/USD from XRC and the exchange rate from WaterNeuron via inter-canister call. Store haircut as `f64` in candid, use `to_bits()` for `Eq`.

**Tech Stack:** Rust, Candid, IC CDK, XRC

---

## Chunk 1: Implementation

### Task 1: Add `LstWrapped` variant to `PriceSource` in `state.rs`

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs:192-204`

- [ ] **Step 1: Add the new variant and fix Eq**

Replace the current `PriceSource` enum at line 192-204 with:

```rust
/// Price source configuration for a collateral type.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, Serialize)]
pub enum PriceSource {
    /// Use the ICP Exchange Rate Canister (XRC) with specified asset pair
    Xrc {
        base_asset: String,
        #[serde(default)]
        base_asset_class: XrcAssetClass,
        quote_asset: String,
        #[serde(default = "default_fiat")]
        quote_asset_class: XrcAssetClass,
    },
    /// Liquid staking token: price = underlying_xrc_price × redemption_rate × (1 - haircut)
    LstWrapped {
        /// Underlying asset for XRC lookup (e.g., "ICP")
        base_asset: String,
        #[serde(default)]
        base_asset_class: XrcAssetClass,
        /// Quote asset (e.g., "USD")
        quote_asset: String,
        #[serde(default = "default_fiat")]
        quote_asset_class: XrcAssetClass,
        /// Canister to query for the LST→underlying exchange rate
        rate_canister_id: candid::Principal,
        /// Method name to call on rate_canister_id (e.g., "get_info")
        rate_method: String,
        /// Conservative discount applied to redemption value (e.g., 0.15 = 15%)
        haircut: f64,
    },
}

impl PartialEq for PriceSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                PriceSource::Xrc { base_asset: ba1, base_asset_class: bac1, quote_asset: qa1, quote_asset_class: qac1 },
                PriceSource::Xrc { base_asset: ba2, base_asset_class: bac2, quote_asset: qa2, quote_asset_class: qac2 },
            ) => ba1 == ba2 && bac1 == bac2 && qa1 == qa2 && qac1 == qac2,
            (
                PriceSource::LstWrapped { base_asset: ba1, base_asset_class: bac1, quote_asset: qa1, quote_asset_class: qac1, rate_canister_id: rc1, rate_method: rm1, haircut: h1 },
                PriceSource::LstWrapped { base_asset: ba2, base_asset_class: bac2, quote_asset: qa2, quote_asset_class: qac2, rate_canister_id: rc2, rate_method: rm2, haircut: h2 },
            ) => ba1 == ba2 && bac1 == bac2 && qa1 == qa2 && qac1 == qac2 && rc1 == rc2 && rm1 == rm2 && h1.to_bits() == h2.to_bits(),
            _ => false,
        }
    }
}

impl Eq for PriceSource {}
```

Note: Remove `PartialEq, Eq` from the derive macro since we're implementing them manually.

- [ ] **Step 2: Verify it compiles**

Run: `cd ~/coding/rumi-protocol-v2 && cargo check -p rumi_protocol_backend 2>&1 | tail -20`
Expected: Successful compilation (or only unrelated warnings)

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs
git commit -m "feat: add LstWrapped variant to PriceSource enum"
```

---

### Task 2: Update `fetch_collateral_price()` in `management.rs`

**Files:**
- Modify: `src/rumi_protocol_backend/src/management.rs:177-265`

- [ ] **Step 1: Replace the destructuring and add LstWrapped handler**

The current code at line 198 does:
```rust
let PriceSource::Xrc { base_asset, base_asset_class, quote_asset, quote_asset_class } = price_source;
```

Replace the entire match/destructure and XRC call logic (lines 198-264) to handle both variants:

```rust
    match price_source {
        PriceSource::Xrc { base_asset, base_asset_class, quote_asset, quote_asset_class } => {
            // --- existing XRC logic (unchanged) ---
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

            match res_xrc {
                Ok((GetExchangeRateResult::Ok(exchange_rate_result),)) => {
                    let rate = rust_decimal::Decimal::from_u64(exchange_rate_result.rate).unwrap()
                        / rust_decimal::Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals)).unwrap();

                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] {} rate: {} at timestamp: {}",
                        base_asset, rate, exchange_rate_result.timestamp
                    );

                    let ts_nanos = exchange_rate_result.timestamp * 1_000_000_000;
                    mutate_state(|s| {
                        if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
                            let should_update = match config.last_price_timestamp {
                                Some(last_ts) => last_ts < ts_nanos,
                                None => true,
                            };
                            if should_update {
                                config.last_price = rate.to_f64();
                                config.last_price_timestamp = Some(ts_nanos);
                            }
                        }
                    });
                }
                Ok((GetExchangeRateResult::Err(error),)) => {
                    log!(TRACE_XRC, "[fetch_collateral_price] XRC error for {}: {:?}", base_asset, error);
                }
                Err((code, msg)) => {
                    log!(TRACE_XRC, "[fetch_collateral_price] Call error for {}: {:?} {}", base_asset, code, msg);
                }
            }
        }

        PriceSource::LstWrapped {
            base_asset, base_asset_class, quote_asset, quote_asset_class,
            rate_canister_id, rate_method, haircut,
        } => {
            // Step 1: Fetch the underlying asset price from XRC (e.g., ICP/USD)
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
                (args,),
                XRC_CALL_COST_CYCLES,
            )
            .await;

            let underlying_rate = match res_xrc {
                Ok((GetExchangeRateResult::Ok(exchange_rate_result),)) => {
                    let rate = rust_decimal::Decimal::from_u64(exchange_rate_result.rate).unwrap()
                        / rust_decimal::Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals)).unwrap();
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] LstWrapped underlying {} rate: {}",
                        base_asset, rate
                    );
                    rate
                }
                Ok((GetExchangeRateResult::Err(error),)) => {
                    log!(TRACE_XRC, "[fetch_collateral_price] LstWrapped XRC error for {}: {:?}", base_asset, error);
                    return;
                }
                Err((code, msg)) => {
                    log!(TRACE_XRC, "[fetch_collateral_price] LstWrapped call error for {}: {:?} {}", base_asset, code, msg);
                    return;
                }
            };

            // Step 2: Fetch the LST exchange rate from the rate canister
            // WaterNeuron's get_info() returns a record with exchange_rate : nat64 (e8s).
            // exchange_rate represents how much nICP you get per 1 ICP (in e8s).
            // So 1 nICP = 1e8 / exchange_rate ICP.
            let rate_result: Result<(WaterNeuronCanisterInfo,), _> =
                ic_cdk::call(rate_canister_id, rate_method.as_str(), ()).await;

            let lst_multiplier = match rate_result {
                Ok((info,)) => {
                    if info.exchange_rate == 0 {
                        log!(TRACE_XRC, "[fetch_collateral_price] LstWrapped exchange_rate is 0, skipping");
                        return;
                    }
                    let multiplier = rust_decimal::Decimal::from(crate::E8S)
                        / rust_decimal::Decimal::from(info.exchange_rate);
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] LstWrapped LST multiplier: {} (exchange_rate={})",
                        multiplier, info.exchange_rate
                    );
                    multiplier
                }
                Err((code, msg)) => {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] LstWrapped rate canister call error: {:?} {}",
                        code, msg
                    );
                    return;
                }
            };

            // Step 3: Combine: underlying_price × lst_multiplier × (1 - haircut)
            let haircut_decimal = rust_decimal::Decimal::from_f64(haircut)
                .unwrap_or(rust_decimal::Decimal::ZERO);
            let final_rate = underlying_rate * lst_multiplier * (rust_decimal::Decimal::ONE - haircut_decimal);

            log!(
                TRACE_XRC,
                "[fetch_collateral_price] LstWrapped final price: {} (underlying={}, multiplier={}, haircut={})",
                final_rate, underlying_rate, lst_multiplier, haircut
            );

            let ts_nanos = ic_cdk::api::time();
            mutate_state(|s| {
                if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
                    let should_update = match config.last_price_timestamp {
                        Some(last_ts) => last_ts < ts_nanos,
                        None => true,
                    };
                    if should_update {
                        config.last_price = final_rate.to_f64();
                        config.last_price_timestamp = Some(ts_nanos);
                    }
                }
            });
        }
    }
```

- [ ] **Step 2: Add the WaterNeuronCanisterInfo struct**

Add this at the top of `management.rs` (after the existing imports/types), or just before `fetch_collateral_price`:

```rust
/// Minimal subset of WaterNeuron's CanisterInfo response.
/// We only need the exchange_rate field for LST pricing.
#[derive(candid::CandidType, serde::Deserialize)]
struct WaterNeuronCanisterInfo {
    exchange_rate: u64,
}
```

This works because Candid deserialization ignores unknown fields — we don't need to define every field in WaterNeuron's response.

- [ ] **Step 3: Verify it compiles**

Run: `cd ~/coding/rumi-protocol-v2 && cargo check -p rumi_protocol_backend 2>&1 | tail -20`
Expected: Successful compilation

- [ ] **Step 4: Commit**

```bash
git add src/rumi_protocol_backend/src/management.rs
git commit -m "feat: handle LstWrapped price source in fetch_collateral_price"
```

---

### Task 3: Update Candid interface

**Files:**
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did:316-323`

- [ ] **Step 1: Update PriceSource type in .did file**

Replace the existing `PriceSource` type (lines 316-323) with:

```candid
type PriceSource = variant {
  Xrc : record {
    quote_asset_class : XrcAssetClass;
    quote_asset : text;
    base_asset_class : XrcAssetClass;
    base_asset : text;
  };
  LstWrapped : record {
    quote_asset_class : XrcAssetClass;
    quote_asset : text;
    base_asset_class : XrcAssetClass;
    base_asset : text;
    rate_canister_id : principal;
    rate_method : text;
    haircut : float64;
  };
};
```

- [ ] **Step 2: Verify candid check passes**

Run: `cd ~/coding/rumi-protocol-v2 && cargo build -p rumi_protocol_backend 2>&1 | tail -20`
Expected: Successful build

- [ ] **Step 3: Commit**

```bash
git add src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat: add LstWrapped to PriceSource candid type"
```

---

### Task 4: Add nICP as collateral type via dfx

- [ ] **Step 1: Run add_collateral_token**

```bash
dfx canister call rumi_protocol_backend add_collateral_token '(record {
  ledger_canister_id = principal "buwm7-7yaaa-aaaar-qagva-cai";
  price_source = variant { LstWrapped = record {
    base_asset = "ICP";
    base_asset_class = variant { Cryptocurrency };
    quote_asset = "USD";
    quote_asset_class = variant { FiatCurrency };
    rate_canister_id = principal "tsbvt-pyaaa-aaaar-qafva-cai";
    rate_method = "get_info";
    haircut = 0.15 : float64;
  }};
  liquidation_ratio = 1.38 : float64;
  borrow_threshold_ratio = 1.55 : float64;
  liquidation_bonus = 1.15 : float64;
  borrowing_fee = 0.005 : float64;
  interest_rate_apr = 0.04 : float64;
  debt_ceiling = 10000 : nat64;
  min_vault_debt = 10_000_000 : nat64;
  min_collateral_deposit = 10_000_000 : nat64;
  display_color = null;
  redemption_fee_floor = null;
  redemption_fee_ceiling = null;
})' --network ic
```

Expected: `(variant { Ok })`

- [ ] **Step 2: Verify the collateral was registered**

```bash
dfx canister call rumi_protocol_backend get_collateral_config '(principal "buwm7-7yaaa-aaaar-qagva-cai")' --network ic
```

Expected: Returns the nICP config with `LstWrapped` price source and correct parameters.

- [ ] **Step 3: Verify pricing is working**

Wait ~5 minutes for the first price timer tick, then:
```bash
dfx canister call rumi_protocol_backend get_collateral_config '(principal "buwm7-7yaaa-aaaar-qagva-cai")' --network ic
```

Expected: `last_price` should be populated (non-null) with a USD value.
