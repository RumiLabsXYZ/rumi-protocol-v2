# Running the CFX price monitor as a service

The monitor is the interim mitigation for audit **F-01**: the chains-rail CFX
oracle is a manual price with no on-chain staleness check, and there is no
liquidation on the rail, so a stale price is the highest risk. **A dead monitor =
a frozen oracle.** Host it accordingly.

> **Pick an always-on host.** A laptop is the wrong home — sleep, lid-close, and
> Wi-Fi drops are exactly the failure the monitor exists to prevent. Use a small
> always-on Linux VM/server (systemd, below) or a Mac that never sleeps (launchd).

This dir ships three things:
- `run.sh` — supervisor-agnostic wrapper (sets PATH + config, execs the daemon)
- `cfx-monitor.service` — **systemd** unit (recommended host)
- `com.rumi.cfx-monitor.plist` — macOS **launchd** LaunchAgent

---

## 1. Prerequisites
- Node.js 20+ on the host.
- This repo checked out; in `monitoring/cfx-price-monitor/` run `npm ci`.
- The backend deployed with the F-01 endpoints (`get_manual_collateral_price`,
  `set_price_pusher_principal`, `get_price_pusher_allowed`).

## 2. Create + register the scoped price-pusher (one time)
The daemon authenticates as a **narrowly-scoped** principal that can ONLY set the
manual price, and ONLY for its allow-listed `(chain, symbol)` pairs. Never give it
the developer key.

```sh
# a) generate a dedicated key (plaintext so a headless service can read the PEM)
dfx identity new cfx-pusher --storage-mode plaintext
PUSHER=$(dfx identity get-principal --identity cfx-pusher)

# b) register it (as the DEVELOPER), scoped to the chain+symbol it may set.
#    staging = chain 71 ("CFX"); eSpace mainnet = chain 1030.
icp canister call rumi_protocol_backend set_price_pusher_principal \
  "(opt principal \"$PUSHER\", vec { record { 71 : nat32; \"CFX\" } })" \
  --environment mainnet-staging --identity rumi_identity

# c) export the PEM to a secure, persistent path (chmod 600), NOT /tmp
#    (dfx blocks plaintext identities on mainnet without this warning suppressed)
DFX_WARNING=-mainnet_plaintext_identity dfx identity export cfx-pusher > cfx-pusher.pem
chmod 600 cfx-pusher.pem
```

Verify: `icp canister call rumi_protocol_backend get_price_pusher_allowed '()' -e mainnet-staging`
should show your `(chain, symbol)`. The daemon's startup also warns if its identity
isn't the registered pusher.

## 3. Configure
All config is via env vars (see [`../.env.example`](../.env.example) for the full
list + defaults). The minimum: `CANISTER_ID`, `IDENTITY_PEM`, `CHAIN_ID`, `SYMBOL`,
`COINGECKO_ID`. Set `SLACK_WEBHOOK_URL` to get alerts in Slack (otherwise they go to
the log as structured JSON).

## 4a. Install on Linux (systemd — recommended)
Edit the `__PLACEHOLDER__`s in `cfx-monitor.service`, then follow the comment block
at its top. Watch it: `journalctl -u cfx-monitor -f`. Stop: `systemctl disable --now cfx-monitor`.

## 4b. Install on macOS (launchd)
Edit the `__PLACEHOLDER__`s in `com.rumi.cfx-monitor.plist` (absolute paths only),
then follow the comment block at its top. `KeepAlive` restarts it if it ever exits;
`RunAtLoad` starts it at login. Stop: `launchctl unload ~/Library/LaunchAgents/com.rumi.cfx-monitor.plist`.

## 5. What "healthy" looks like
Each cycle logs one `"message":"tick"` line:
```json
{"message":"tick","ok":true,"refreshed":false,"marketE8":"4874000","usedSources":["coingecko","kraken","okx"],"alerts":0,"sourceFailures":0}
```
- `ok:true` every cycle (a run of `ok:false` trips the downtime watchdog → a `monitor_downtime` alert).
- `refreshed:true` when it pushed a new price (drift > `DRIFT_BPS` or age > `MAX_AGE_SEC`).
- `alerts:0` — non-zero means a vault is under the CR warn band, sources disagreed, a write failed, etc. (search the log for `"level":"critical"`).

## 6. Staging vs eSpace-mainnet
| | staging | eSpace mainnet |
|---|---|---|
| `CANISTER_ID` | `kvg63-wiaaa-aaaao-bbabq-cai` | the mainnet-1030 launch canister |
| `CHAIN_ID` | `71` | `1030` |
| pusher scope | `record {71;"CFX"}` | `record {1030;"CFX"}` |

## 7. Security notes
- The pusher key can ONLY set prices for its allow-listed `(chain, symbol)` — not
  open/close vaults, not any other endpoint. Blast radius if the host is compromised
  is "set the CFX price", which the backend also rejects at `0`.
- Rotate or revoke instantly from the developer key:
  `set_price_pusher_principal(opt principal "<new>", vec {…})` or `(null, vec {})`.
- Keep the PEM `chmod 600`, owned by the service user, off shared/tmp paths.
