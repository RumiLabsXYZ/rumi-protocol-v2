# Proposed redemption-oracle cadence settings

Status: **evidence and proposal only**. No timer setter has been called. These are proposed production writes and require explicit authorization before execution.

## Finding and narrow proposal

The redemption queue accepts a candidate price only when its timestamp is not in the future and is no more than 600 seconds old (`src/rumi_protocol_backend/src/vault.rs:34-36,148-150`). In an anonymous read of the live backend at 2026-09-28 14:18:07–14:18:08 UTC, `get_collateral_price_fetch_intervals()` returned the raw list below. Separate anonymous `get_collateral_config` queries confirmed the symbols for the three affected principals.

| Collateral | Ledger principal | Current interval | Proposed interval | Scheduled attempts/day, current → proposed |
|---|---|---:|---:|---:|
| ckDOGE | `efmc5-wyaaa-aaaar-qb3wa-cai` | 900 s | 300 s | 96 → 288 |
| ckXAUT | `nza5v-qaaaa-aaaar-qahzq-cai` | 1,800 s | 300 s | 48 → 288 |

The proposed changes are only for ckDOGE and ckXAUT. The raw interval response also reported ICP `ryjl3-tyaaa-aaaaa-aaaba-cai` at 300, EXE `rh2pm-ryaaa-aaaan-qeniq-cai` at 1,200, ckBTC `mxzaz-hqaaa-aaaar-qaada-cai` at 900, ckETH `ss2fx-dyaaa-aaaar-qacoq-cai` at 900, nICP `buwm7-7yaaa-aaaar-qagva-cai` at 900, BOB `7pail-xaaaa-aaaas-aabmq-cai` at 1,200, and native XRP `5zjma-7dsov-wwsll-yojyc-23tbo-ruxmz-i` at 900. These settings are not proposed for change.

The target 300 seconds is the existing default for a non-ICP collateral without an override (`src/rumi_protocol_backend/src/xrc.rs:150-166`). An accepted XRC result is timestamped with a 60-second margin (`src/rumi_protocol_backend/src/management.rs:653-695`). Thus, if a scheduled request succeeds on cadence, its nominal timestamp age just before the next 300-second tick is about 360 seconds. A failed, rejected, skipped, or delayed price fetch can still leave a stale value; this change does not weaken the queue's strict 600-second gate or guarantee freshness.

## nICP and ICP timer boundary

nICP is intentionally left at its observed 900-second backup timer in this proposal. Its price is derived from the cached ICP price and WaterNeuron rate; the backend source documents that it does not make a duplicate XRC request for the ICP/USD pair (`src/rumi_protocol_backend/src/management.rs:400-423,650-653`). The companion backend source fix is planned to republish nICP from newly accepted ICP samples, so the nICP timer remains a backup. That code change and its verification are a separate source task; this settings proposal does not claim that the existing nICP interval alone meets the freshness window.

The interval getter includes an ICP row, but its comment says that row is informational: actual ICP refresh uses Timer A (`src/rumi_protocol_backend/src/main.rs:10590-10607`). Timer A has a separate developer-gated setter, `set_xrc_fetch_interval_secs`, and a source default of 480 seconds (`src/rumi_protocol_backend/src/main.rs:10431-10449`; `src/rumi_protocol_backend/src/state.rs:279-281,1391-1396`). The live value is not exposed by the current public getter, so the 300-second ICP row must not be interpreted as Timer A's current setting. No Timer A write is included here. Its effective value should be read back through the planned corrected getter before making any further freshness claim.

## Authorization, effect, and rollback

`set_collateral_price_fetch_interval_secs(collateral, secs)` accepts a principal and `nat64` seconds. It is restricted to the configured developer principal, rejects intervals below 60 seconds and rejects ICP, writes one global map value for that collateral, and re-registers that collateral's timer in place (`src/rumi_protocol_backend/src/main.rs:10539-10587`; `src/rumi_protocol_backend/src/state.rs:1407-1419`; `src/rumi_protocol_backend/src/xrc.rs:185-200`). The setting is per collateral ledger, not per caller. If the chosen CLI identity does not equal the developer principal, the canister should reject the call; controller status by itself does not grant this setter authority.

If separately authorized, the supported CLI form is an update call (do not pass `--query`):

```sh
icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
  set_collateral_price_fetch_interval_secs \
  '(principal "efmc5-wyaaa-aaaar-qb3wa-cai", 300 : nat64)' \
  --environment mainnet-live --identity rumi_identity

icp canister call tfesu-vyaaa-aaaap-qrd7a-cai \
  set_collateral_price_fetch_interval_secs \
  '(principal "nza5v-qaaaa-aaaar-qahzq-cai", 300 : nat64)' \
  --environment mainnet-live --identity rumi_identity
```

Before either proposed write, re-read the two symbols and intervals and verify that the caller is still the canister's developer principal. After each call, re-read `get_collateral_price_fetch_intervals()` and require exactly 300 seconds for the targeted principal. Then observe subsequent queue reads until both prices have timestamps within 600 seconds and `ranking_fresh=true`; do not call a refresh or redemption method as part of this check. The setter re-registers the timer but does not itself perform a price fetch.

Rollback, if separately authorized, uses the same setter with the observed prior value: restore ckDOGE to 900 seconds and ckXAUT to 1,800 seconds. Re-read and confirm those exact values. Do not restore unrelated entries or modify liquidation, borrowing, fee, freshness, or oracle-source thresholds.

## Bounded cost estimate

For the two direct XRC-fed assets, the new schedule adds 192 ckDOGE ticks and 240 ckXAUT ticks per 24 hours: **432 additional scheduled XRC request attempts/day** if both remain eligible and every timer tick reaches XRC. Source attaches 1,000,000,000 cycles to each XRC request (`src/rumi_protocol_backend/src/management.rs:653-695`), so the conservative attached-payment upper bound is **432 billion cycles/day**. This is not a claim about cycles actually consumed; skipped calls and refunded unused payment can lower actual burn. The nICP backup timer is unchanged and excluded: its LST path uses cached ICP input rather than a second XRC call.

## Read-only checks performed

- `icp canister call --help` confirmed the supported positional syntax `<CANISTER> [METHOD] [ARGS]`, Candid argument format, `--environment`, and `--identity` flags.
- Live interval query used the anonymous identity and `--query`; symbol mapping used anonymous `get_collateral_config` queries for nICP, ckXAUT, ckDOGE, and EXE.
- No setter call, identity change, timer change, source write, or asset/threshold write was made for this proposal.
