# LstWrapped PriceSource Design

## Goal

Add a new `PriceSource::LstWrapped` variant that prices liquid staking tokens (LSTs) by composing the underlying asset's XRC price with a redemption rate fetched from the LST canister, minus a configurable haircut.

## Architecture

The new variant extends the existing `PriceSource` enum. It reuses the XRC infrastructure for the underlying asset price and adds an inter-canister call to the LST protocol for the exchange rate. The existing `fetch_collateral_price()` function is extended with a match arm for the new variant. No changes to vault logic, liquidation, CR calculations, or timer infrastructure.

## Formula

```
lst_price_usd = underlying_usd_price × (1e8 / lst_exchange_rate_e8s) × (1 - haircut)
```

For nICP with WaterNeuron `exchange_rate = 81_671_955` and 15% haircut:
- ICP = $8.00 → nICP = $8.00 × 1.2244 × 0.85 = **$8.33**

## Data Source

- **Canister:** `tsbvt-pyaaa-aaaar-qafva-cai` (WaterNeuron)
- **Method:** `get_info() -> CanisterInfo` (query)
- **Field:** `exchange_rate : nat64` (e8s, represents ICP-per-nICP inverse: 1e8 / exchange_rate = nICP→ICP multiplier)

## Files Changed

1. `src/rumi_protocol_backend/src/state.rs` — Add `LstWrapped` variant to `PriceSource`, manual `PartialEq`/`Eq` impl
2. `src/rumi_protocol_backend/src/management.rs` — Handle `LstWrapped` in `fetch_collateral_price()`
3. `src/rumi_protocol_backend/rumi_protocol_backend.did` — Add `LstWrapped` to candid `PriceSource` type
4. No changes to `lib.rs` (`AddCollateralArg` already accepts `PriceSource` generically)
5. No changes to timers, vault logic, or CR calculations
