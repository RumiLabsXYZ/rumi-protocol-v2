# CFX price monitor + auto-refresher — design

Date: 2026-06-18
Branch: `feat/cfx-price-monitor` (worktree, off `main`)
Status: approved (Rob: "just do it")

## Problem (audit F-01)

The chains rail lets users lock native CFX (Conflux eSpace, chain 1030) / MON as
collateral and mint icUSD. Each chain vault's collateral ratio is computed against
a **manual** price set by `set_manual_collateral_price(chain, symbol, price_e8)`.
There is **no on-chain staleness check, no freshness timestamp, and no liquidation
mechanism** on the chains rail. A stale or wrong manual price is therefore the
single highest risk for the gated eSpace-mainnet soft-launch: a vault that opened
over-MCR can drift under-collateralized at the true market price with no recovery
path. Price discipline + monitoring is the PRIMARY risk control.

This is the **interim mitigation**. The eventual proper fix (an automated on-chain
oracle: XRC/Pyth/Chainlink) is explicitly OUT OF SCOPE.

## Confirmed repo facts (read at d1181f3)

- `set_manual_collateral_price(chain: nat32, symbol: text, price_e8: nat64) -> Result`
  at `main.rs:1398`. Guard: inline `read_state(|s| s.developer_principal != caller)`.
  Rejects `price_e8 == 0`. Writes `multi_chain.manual_prices`.
- Storage: `manual_prices: BTreeMap<(ChainId, String), u64>` (e8 scale) in
  `MultiChainStateV4` (`chains/multi_chain_state.rs:256`), ciborium-encoded stable state.
- **No getter** for the manual price exists. **No timestamp** is stored.
- CR math: `chains/vault.rs:146` `collateral_ratio_e4(collateral_native, native_decimals,
  price_e8, debt_e8s) = (collateral_native * price_e8 / 10^native_decimals) / debt_e8s * 10_000`.
  CFX = 18 decimals, debt = e8s, price = e8. **MCR = 13000 (130%)** for EVM chains
  (`chains/monad/chain_vault.rs:38`).
- `list_chain_vaults(chain) -> vec ChainVaultV1` query returns per-vault
  `collateral_amount_e18` + `debt_e8s` — enough to recompute true CR off-chain.
- Auth: single `developer_principal` (`state.rs:903`); no scoped role exists.
- `State` is ciborium/serde-encoded; adding a `#[serde(default)]` field is the
  established non-breaking pattern (e.g. `custody_kind` at `state.rs:557`).
- `MultiChainState` versioning is documented in `multi_chain_state.rs:1-30`: bump
  `V4 -> V5`, keep V4 verbatim, add a `#[serde(default)]` field, rebind the alias.
  No `post_upgrade` migration call needed for a non-breaking reshape.

## Architecture (decided)

Two independently-deployable parts, one branch, two commits.

### Part A — minimal additive backend changes (`rumi_protocol_backend`)

All three are non-breaking ciborium reshapes (no state-wipe risk).

1. **On-chain set-timestamp** (parallel map, leaves CR path untouched):
   - Bump `MultiChainStateV4 -> MultiChainStateV5`. Keep V4 verbatim. Add:
     `#[serde(default)] pub manual_price_set_at_ns: BTreeMap<(ChainId, String), u64>`.
   - Rebind `pub type MultiChainState = MultiChainStateV5;`.
   - The existing `manual_prices` map and every CR read of it are unchanged.
2. **Getter query**:
   - `get_manual_collateral_price(chain: nat32, symbol: text) -> opt record { price_e8: nat64; set_at_ns: nat64 }`.
   - Returns `None` if no price set. `set_at_ns == 0` for a price set before this
     upgrade (the parallel map is empty until the next write); self-heals on first refresh.
3. **Scoped price-pusher principal** (least privilege):
   - `State` gains `#[serde(default)] pub price_pusher_principal: Option<Principal>`
     (near `developer_principal`, `state.rs:903`).
   - `set_price_pusher_principal(principal)` / `get_price_pusher_principal()` —
     **developer-gated** (only the dev can grant/rotate the pusher).
   - `set_manual_collateral_price` guard widens to:
     `caller == developer_principal || Some(caller) == price_pusher_principal`.
     It ALSO stamps `manual_price_set_at_ns.insert((chain, symbol), now_ns)` on write.
   - No price clamp (Rob chose the plain scoped principal). The `price_e8 == 0`
     reject stays.

`.did` + generated declarations updated to match.

### Part B — off-chain daemon (`monitoring/cfx-price-monitor/`, Node/TS, Vitest)

Small pure modules + a thin runner. Config-driven over a list of
`{chainId, symbol, coinGeckoId, network, canisterId}` so CFX/1030 is the only
seeded entry today but MON/others are config later.

| Module | Responsibility | Tested |
|---|---|---|
| `src/sources/*.ts` | One adapter per source: `fetchCfxUsd() -> {source, priceUsd, ts}`. CoinGecko + >=1 CEX (exact CFX pairs verified during build). Pluggable. | mocked fetch |
| `src/aggregate.ts` | Median of healthy quotes; drop any quote >`outlierPct` from median; require >=`minSources`. Pure. | unit, all branches |
| `src/cr.ts` | Exact port of Rust `collateral_ratio_e4`. Pure. | unit vs Rust vectors |
| `src/policy.ts` | The brain: `(marketE8, onChain{priceE8,setAtNs}, nowNs, vaults+CRs, cfg) -> {shouldRefresh, reason, alerts[]}`. Pure. | unit, all branches |
| `src/canister.ts` | `@dfinity/agent` actor: `getOnChainPrice`, `setPrice`, `listChainVaults`. Identity = price-pusher PEM from env. | mock/local replica |
| `src/alerts.ts` | Structured JSON alert to stdout always; POST Slack webhook iff `SLACK_WEBHOOK_URL` set. | unit |
| `src/index.ts` | 60s loop; heartbeat + downtime watchdog; per-cycle errors alert, never crash. | integration |
| `src/config.ts` | env + `monitors.json`; thresholds. | unit |

## Data flow (per `pollSec` tick)

1. Fetch all sources concurrently -> quotes.
2. `aggregate` -> median market price (USD) -> convert to e8.
3. Read on-chain `{price_e8, set_at_ns}` via the new getter.
4. `policy`: refresh iff `|market - onchain| > driftBps` OR `(now - set_at_ns) > maxAgeSec`.
5. If refresh: `setPrice(chain, "CFX", marketE8)`, then **verify** via the getter.
6. `listChainVaults(chain)` -> recompute each vault's true CR at the live market price.
7. Alert any vault with `CR < crWarnBandE4`.
8. Record heartbeat (last successful cycle ts).
9. Downtime watchdog: alert if no successful cycle within `downtimeIntervals * pollSec`.

## Error handling (safety-first — no liquidation exists)

- `< minSources` healthy -> **do NOT write**; alert "insufficient sources"; keep
  the last on-chain price. Never push a stale/zero price (backend rejects 0 anyway).
- Canister-call failure -> alert, retry next tick, never crash the loop.
- The daemon can ONLY set prices (scoped key). It never opens/closes vaults or
  touches any other endpoint.
- Write-verify mismatch (getter != what we wrote) -> alert; retry next tick.

## Thresholds (runbook defaults, all config-overridable)

`driftBps=200` (2%), `maxAgeSec=300` (5 min), `crWarnBandE4=16000` (1.6x),
`outlierPct=5`, `minSources=2`, `pollSec=60`, `downtimeIntervals=3`.

## Testing

- **Rust**: unit tests for the getter (returns set value + timestamp; `None` when
  unset), the widened guard (price-pusher CAN set, developer CAN set, anyone else
  rejected, zero rejected), and a `serde(default)` missing-field decode test for
  both `price_pusher_principal` and `manual_price_set_at_ns` (mirror
  `test_serde_default_handles_missing_fields`, `state.rs:6645`). A PocketIC test
  exercising the price-pusher auth path end to end.
- **TS** (Vitest): exhaustive unit tests for `aggregate`, `cr` (vs Rust vectors),
  `policy` (every branch: drift trigger, age trigger, no-trigger, insufficient
  sources, vault-below-band, downtime), `alerts`. Canister client against a mock
  actor; an integration test of one full loop tick with mocked sources + actor.

## Out of scope

On-chain automated oracle (XRC/Pyth/Chainlink); liquidation; debt-ceiling
enforcement; on-chain price clamp; MON/Monad wiring (config-ready, CFX/1030 only).
**No merge, no deploy** — build + test only; Rob authorizes anything beyond.

## Deploy/run notes (for later, not this task)

Backend changes deploy via the normal upgrade path. Daemon runs wherever Rob
hosts it: `network=ic`, `canisterId` = kvg63 staging (or the launch canister),
identity PEM = the price-pusher key granted via `set_price_pusher_principal`.
