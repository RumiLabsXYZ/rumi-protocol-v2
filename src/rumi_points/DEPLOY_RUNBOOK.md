# rumi_points — airdrop launch runbook

Authoritative sequence for the **full public launch** of the airdrop points
engine. Maps to launch items 1-5. Companion to `STATUS.md` (what the canister
does) — this file is *how to ship it*.

> Conventions: `DFX="$HOME/Library/Application Support/org.dfinity.dfx/bin/dfx"`,
> `export DFX_WARNING=-mainnet_plaintext_identity`, and **always**
> `--identity rumi_identity` for mainnet. NEVER reinstall a stateful canister —
> upgrades only. `rumi_points` itself is a *fresh install* (new canister).

---

## 0. Status at time of writing (2026-06-07)

| Item | State |
|------|-------|
| 1. POINTS-001/002 hardening | ✅ committed + verified (146 lib / 7 PocketIC / candid), PR #228 open, **awaiting merge OK** |
| 2. Reserve mainnet canister id | ⛔ not done — Rob's action (irreversible) |
| 3. Deploy trio (points + backend + 3pool) | ⛔ blocked — see the blocker below |
| 4. Operate (commit H0 / poll / start_season) | ⛔ blocked on 3; needs secret seed `S0` from Rob |
| 5. Phase 6 frontend | ⛔ not built; declarations generated (this pass) |

## ⚠️ THE deploy blocker (verified 2026-06-07)

The poller reads two **source endpoints that are NOT live on mainnet**. Verified
by direct query call:

```
$DFX canister --network ic --identity rumi_identity call \
  tfesu-vyaaa-aaaap-qrd7a-cai get_events_forward_filtered '(0:nat64,1:nat64,null)' --query
  -> CanisterError: Canister has no query method 'get_events_forward_filtered'

$DFX canister --network ic --identity rumi_identity call \
  fohh4-yyaaa-aaaap-qtkpa-cai get_liquidity_events_v2_forward '(0:nat64,1:nat64)' --query
  -> CanisterError: Canister has no query method 'get_liquidity_events_v2_forward'
```

Both endpoints are **committed on `main`** but the deployed mainnet wasms predate
them. So the launch is genuinely a **trio deploy**: install `rumi_points` AND
upgrade `rumi_protocol_backend` AND upgrade `rumi_3pool`.

**Consequence — the airdrop deploy rides with / follows the 2026-06-05 audit.**
There are ~37 uncommitted backend + ~8 uncommitted 3pool audit changes in the
working tree (incl. LIVE backend HIGHs per the audit memory). Deploying the
backend/3pool forward endpoints from bare `main` would ship a backend WITHOUT
those audit fixes and burn a backend upgrade. Correct order: **land the
2026-06-05 audit first**, then one combined upgrade carries audit fixes + the
forward endpoints, and `rumi_points` installs alongside.

---

## Gates that are Rob's (cannot proceed without)

1. **Merge PR #228** (item 1).
2. **Reserve the mainnet canister id** for `rumi_points` (item 2) — irreversible,
   spends ICP/cycles on `rumi_identity`. Add to `canister_ids.json` + `mainnet-live`.
3. **Deploy authorization** (item 3) — and the audit landing first (above).
4. **The secret 32-byte season seed `S0`** (item 4) — Rob generates and holds it
   (commit-reveal; the canister must not generate its own secret). `H0 = sha256(S0)`
   is committed at install; `S0` is revealed via `start_season(S0)`.

---

## Pre-flight facts

- **Wasm exceeds the 2 MiB ingress limit.** Release wasm is **2.43 MB**
  (2,429,258 B) > 2,097,152 B. Must `ic-wasm shrink` + `gzip` before install
  (gzips to ~606 KB). Same treatment as `rumi_analytics`.
- **H0 is set at install** via `InitArgs.snapshot_seed_commit` (opt blob). If left
  `None` it defaults to all-zeros → `is_committed()` is false → `start_season`
  rejects with `NotCommitted` (the F3 security fix). **You must pass H0 at install.**
- **`excluded_principals` REPLACES, not merges.** `None` applies the built-in
  9-canister protocol-owned seed; passing a vec overrides it entirely.
- **Source/asset config is mainnet-seeded automatically** at init (no override
  needed on mainnet; `set_source_canister`/`set_asset_ledger` are for local/test).
- **Default season window:** `2026-06-01 00:00 UTC → 2026-08-31 23:59 UTC` (91 days).
  Already started; fine (poll backfills from cursor 0). Confirm or override via
  `season_start_ns`/`season_end_ns`.

```
InitArgs = record {
  admin                : opt principal;   // defaults to deploying identity
  excluded_principals  : opt vec principal; // None = 9-canister seed (REPLACES)
  snapshot_seed_commit : opt blob;        // H0 = sha256(S0) — REQUIRED to start a season
  season_start_ns      : opt nat64;       // default 2026-06-01
  season_end_ns        : opt nat64;       // default 2026-08-31
}
```

---

## Deploy sequence

**A. Land the 2026-06-05 audit** (prerequisite; separate work) so `main` carries
audit fixes + the committed forward endpoints.

**B. Merge PR #228** (item 1) into the deploy `main`.

**C. Reserve the canister id** (Rob):
```
$DFX canister --network ic --identity rumi_identity create rumi_points
# add the returned id to canister_ids.json and the mainnet-live list
```

**D. Generate the seed** (Rob, offline, keep S0 secret):
```
S0 = 32 random bytes            # keep secret, store safely
H0 = sha256(S0)                 # the commitment passed at install
```

**E. Build + shrink + gzip + install `rumi_points`:**
```
cargo build --release --target wasm32-unknown-unknown -p rumi_points
ic-wasm target/wasm32-unknown-unknown/release/rumi_points.wasm -o /tmp/rumi_points.shrunk.wasm shrink
gzip -c /tmp/rumi_points.shrunk.wasm > /tmp/rumi_points.wasm.gz
# install with InitArgs carrying H0 + the chosen season window:
$DFX canister --network ic --identity rumi_identity install rumi_points \
  --wasm /tmp/rumi_points.wasm.gz \
  --argument '(opt record { snapshot_seed_commit = opt blob "<H0>"; season_start_ns = null; season_end_ns = null; admin = null; excluded_principals = null })'
```

**F. Upgrade backend** (forward endpoints) — Upgrade variant + description:
```
$DFX deploy rumi_protocol_backend --network ic --identity rumi_identity \
  --argument '(variant { Upgrade = record { mode = null; description = opt "Add airdrop source-forward read endpoints (+ 2026-06-05 audit)" } })'
```

**G. Upgrade 3pool** (forward endpoint) — dummy ThreePoolInitArgs required on upgrade:
```
$DFX deploy rumi_3pool --network ic --identity rumi_identity --argument '(<dummy ThreePoolInitArgs>)'
```

**H. Verify the trio:** re-run the two query calls from the blocker section — both
must now return data instead of "no query method".

> The pre-deploy hook (`.claude/hooks/pre-deploy-test.sh`) runs the full suite
> (incl. the POCKET_IC_BIN E2E) before any mainnet deploy.

---

## Operate sequence (item 4)

1. Confirm config: `get_source_canisters`, `get_asset_ledgers` (mainnet-seeded);
   `get_pending_commit` should equal H0.
2. `set_poll_enabled(true)` — start ingestion.
3. Let the poll timer run (or `trigger_poll`); confirm registrations via
   `get_ingest_status` and `leaderboard`.
4. `start_season(<S0 as blob>)` — opens epoch 0, enables the epoch driver,
   begins the commit-reveal chain.
5. Monitor: public `get_epoch_status`; admin-only `get_epoch_status_admin` for
   cursor/progress; `get_revealed_seed(i)` for the audit chain. `force_epoch_tick`
   is manual step / ops recovery.

---

## Frontend (item 5)

- Declarations generated at `src/declarations/rumi_points/` (this pass).
- `index.js` resolves the id from `process.env.CANISTER_ID_RUMI_POINTS` — auto-set
  once the id is in `canister_ids.json` (step C).
- Surfaces to build: enrollment (confetti), personal status, leaderboard.

---

## Known non-blockers

- **5x ckUSDC/ckUSDT repayment window** is gated on the upstream backend
  `RepayToVault.repayment_asset` field (confirmed still NOT built 2026-06-07).
  Season 1 launches without it; the window is skipped + logged. Flip
  `source_types::backend::normalize` to read the real field when it ships — no
  other change here.
- **Capture stall counter** (STATUS follow-up): a permanently-unreachable source
  safely stalls the epoch (never closes); visible via `get_epoch_status_admin`,
  recoverable via `add_excluded` / `force_epoch_tick`. Surfacing a stall count is
  a nicety, not a blocker.
