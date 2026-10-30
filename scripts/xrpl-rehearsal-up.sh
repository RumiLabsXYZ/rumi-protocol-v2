#!/usr/bin/env bash
# Bring up the LOCAL XRPL-testnet rehearsal of the XRPL icUSD lane.
#
# Starts a long-lived PocketIC server (gateway http://localhost:4950, never 4943),
# installs the xrpl-testnet-fixture backend, icUSD ledger, XRC mock and a dummy-auth
# Internet Identity, and configures the lane against the PUBLIC XRPL TESTNET.
# Nothing here touches IC mainnet or XRPL mainnet.
#
#   scripts/xrpl-rehearsal-up.sh            # build artifacts if missing, start, wait for READY
#   scripts/xrpl-rehearsal-up.sh --rebuild  # force a fresh fixture backend build
#
# Work files live in target/xrpl-rehearsal (survives reboots, unlike /private/tmp).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="${REHEARSAL_DIR:-$ROOT/target/xrpl-rehearsal}"
POCKET_IC="${POCKET_IC_BIN:-$(cd "$ROOT" && git rev-parse --path-format=absolute --git-common-dir | xargs dirname)/pocket-ic}"
SERVER_PORT=4951
II_RELEASE="https://github.com/dfinity/internet-identity/releases/download/release-2026-05-26-be"
II_BACKEND_SHA=80946901c67705df8c56236d0cb4732d21f4988a655e6662abe7c5d396e3f903
REBUILD=0
[[ "${1:-}" == "--rebuild" ]] && REBUILD=1

mkdir -p "$DIR"
[[ -x "$POCKET_IC" ]] || { echo "pocket-ic server not found at $POCKET_IC (set POCKET_IC_BIN)"; exit 1; }

fetch_checked() { # url dest sha256
  if [[ ! -f "$2" ]]; then curl -fsSL "$1" -o "$2.part" && mv "$2.part" "$2"; fi
  echo "$3  $2" | shasum -a 256 -c - >/dev/null || { echo "sha256 mismatch for $2"; rm -f "$2"; exit 1; }
}
fetch_checked "$II_RELEASE/internet_identity_dev.wasm.gz" "$DIR/ii.wasm.gz" "$II_BACKEND_SHA"
FRONT_SHA="$(curl -fsSL "$II_RELEASE/internet_identity_frontend.wasm.gz.sha256" | awk '{print $1}')"
fetch_checked "$II_RELEASE/internet_identity_frontend.wasm.gz" "$DIR/ii_frontend.wasm.gz" "$FRONT_SHA"

if [[ $REBUILD == 1 || ! -f "$DIR/backend_fixture.wasm.gz" ]]; then
  # PocketIC 7 caps the code section at 10 MiB, so the rehearsal uses a size-optimised fixture build.
  (cd "$ROOT" && cargo build --release --target wasm32-unknown-unknown --package rumi_protocol_backend \
    --features xrpl-testnet-fixture \
    --config 'profile.release.package.rumi_protocol_backend.opt-level="z"' \
    --config 'profile.release.lto="thin"' --config 'profile.release.codegen-units=4')
  ic-wasm "$ROOT/target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm" -o "$DIR/backend_fixture.wasm" shrink --keep-name-section
  gzip -9 -n -f "$DIR/backend_fixture.wasm"
  "$ROOT/scripts/verify-xrpl-icusd-artifact.sh" --require-testnet-fixture "$DIR/backend_fixture.wasm.gz"
fi

if ! curl -fs -o /dev/null "http://127.0.0.1:$SERVER_PORT/instances"; then
  nohup "$POCKET_IC" --port "$SERVER_PORT" --ttl 86400 > "$DIR/server.log" 2>&1 &
  echo $! > "$DIR/server.pid"
  for _ in $(seq 1 30); do curl -fs -o /dev/null "http://127.0.0.1:$SERVER_PORT/instances" && break; sleep 1; done
fi

(cd "$ROOT" && cargo test --release -p rumi_protocol_backend --test xrpl_icusd_testnet_rehearsal --no-run 2>&1 | tail -1)
BIN="$(cd "$ROOT" && cargo test --release -p rumi_protocol_backend --test xrpl_icusd_testnet_rehearsal --no-run --message-format=json 2>/dev/null \
  | python3 -I -c 'import json,sys
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("target",{}).get("name")=="xrpl_icusd_testnet_rehearsal" and m.get("executable"): print(m["executable"])' | tail -1)"
cp "$BIN" "$DIR/harness-bin"

REHEARSAL_DIR="$DIR" REHEARSAL_SERVER_URL="http://127.0.0.1:$SERVER_PORT" POCKET_IC_BIN="$POCKET_IC" \
  nohup "$DIR/harness-bin" run_rehearsal_harness --ignored --nocapture --test-threads=1 > "$DIR/harness.log" 2>&1 &
echo $! > "$DIR/harness.pid"
echo "harness pid $(cat "$DIR/harness.pid"); waiting for READY (log: $DIR/harness.log)"
for _ in $(seq 1 300); do grep -q "READY" "$DIR/harness.log" && break; sleep 2; done
grep -q "READY" "$DIR/harness.log" && echo "READY: gateway http://localhost:4950, state $DIR/state.json" || { echo "not ready; see $DIR/harness.log"; exit 1; }
