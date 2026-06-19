# CFX price monitor + auto-refresher

Interim mitigation for the 2026-06-18 chains-rail audit **F-01** (manual CFX
oracle with no on-chain staleness check, and no liquidation on the chains rail).
This off-chain daemon keeps the backend's manual CFX price fresh + accurate and
alerts on vault risk and on its own downtime.

> This is the interim control for the **gated, developer-only** eSpace-mainnet
> soft-launch. The eventual proper fix is an automated on-chain oracle
> (XRC/Pyth/Chainlink) — out of scope here.

## What it does, every `POLL_SEC` (default 60s)

1. Fetches CFX/USD from **CoinGecko + Kraken + OKX** (independent; Binance is
   excluded — its API returns "restricted location" from many hosts), takes the
   **median**, and drops any source more than `OUTLIER_PCT` from it. It refuses
   to act on fewer than `MIN_SOURCES` healthy sources.
2. Reads the on-chain price + freshness via `get_manual_collateral_price`.
3. **Refreshes** via `set_manual_collateral_price` when the market has drifted
   more than `DRIFT_BPS` OR the on-chain price is older than `MAX_AGE_SEC`
   (the monitor owns freshness), then **verifies** the write landed.
4. Recomputes **every** chain vault's *true* CR at the live market price and
   **alerts** any vault below `CR_WARN_BAND_E4` (warn), escalating to **critical**
   below `MCR_E4` (under-collateralized — there is no liquidation, so a human
   must add collateral / repay / close).
5. Alerts on its own **downtime** (a dead monitor = a frozen oracle = the F-01 risk).

## Backend dependency

This daemon relies on three additive endpoints shipped alongside it in
`rumi_protocol_backend`:

- `get_manual_collateral_price(chain, symbol) -> opt record { price_e8; set_at_ns }`
- `set_manual_collateral_price` widened to accept a scoped **price-pusher** principal
- `set_price_pusher_principal(opt principal, vec record { nat32; text })` (developer-gated):
  registers the pusher AND the exact `(chain_id, symbol)` pairs it may set — it can
  set nothing else, and cannot be the anonymous principal
- `get_price_pusher_principal()` / `get_price_pusher_allowed()` (read-only)

## Setup

```bash
npm install

# 1. Create a DEDICATED price-pusher identity (NOT the developer key) and export its PEM:
dfx identity new cfx-pusher
dfx identity export cfx-pusher > pusher.pem        # secp256k1 EC private key

# 2. Find its principal and register it on the backend (as the developer),
#    scoped to ONLY the (chain_id, symbol) pairs it may set (CFX on 1030 here):
dfx identity get-principal --identity cfx-pusher
icp canister call <CANISTER> set_price_pusher_principal \
  '(opt principal "<pusher-principal>", vec { record { 1030 : nat32; "CFX" } })' \
  -e <ENV> --identity rumi_identity
# The pusher can set NOTHING outside that allow-list (and cannot be the anonymous
# principal). To revoke: set_price_pusher_principal '(null, vec {})'.

# 3. Configure + run:
cp .env.example .env        # edit CANISTER_ID, IDENTITY_PEM, optional SLACK_WEBHOOK_URL
npm start                   # or: node --env-file=.env --import tsx src/index.ts
```

The price-pusher principal can ONLY call `set_manual_collateral_price`. If the
host is compromised, the blast radius is "set chain prices", not the full
developer key (which can open/close vaults, sweep treasury, freeze, etc.).

## Configuration

All via environment variables — see [.env.example](.env.example). Defaults match
the launch runbook (2% drift, 5-min max age, 1.6x warn band, ≥2 sources).

## Alerts

Every alert is written to stdout as a single structured JSON line (code, level,
message, context, ISO timestamp) for any log pipeline/pager. If
`SLACK_WEBHOOK_URL` is set, criticals/warnings are also POSTed to Slack. A Slack
failure never crashes the monitor — it is logged as a fallback line.

Alert codes: `vault_below_band`, `insufficient_sources`, `refresh_failed`,
`verify_mismatch`, `monitor_downtime`, `identity_not_registered_pusher`,
`tick_exception`, `alert_webhook_failed`.

## Tests

```bash
npm test          # vitest: pure modules (cr, aggregate, policy), client mapping,
                  # source parsers (real fixtures), alert sink, runner, config, identity
npm run typecheck
```

The CR math in `src/cr.ts` is a byte-faithful port of the backend's
`collateral_ratio_e4`, tested against hand-computed vectors so the monitor sees
the same CR the canister would.
