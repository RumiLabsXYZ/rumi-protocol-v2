#!/bin/sh
# Run the CFX price monitor under a process supervisor (launchd, systemd, pm2,
# Docker, ...). Supervisors run with a minimal environment, so this wrapper sets
# PATH + the daemon's config, then execs the daemon. Override any value via the
# supervisor's own env block instead of editing this file.
#
# See SERVICE.md for the full setup (pusher identity, PEM, install).
set -eu

# --- node on PATH ---------------------------------------------------------
# NODE_BIN_DIR must contain `node` and `npm`. Find it with:
#   dirname "$(command -v node)"
# nvm users: point this at the ACTIVE version's bin (nvm is not on a fixed path).
NODE_BIN_DIR="${NODE_BIN_DIR:-/usr/local/bin}"
export PATH="$NODE_BIN_DIR:$PATH"

# --- where the daemon lives -----------------------------------------------
# A checkout of this repo's monitoring/cfx-price-monitor with `npm ci` already run.
APP_DIR="${APP_DIR:-$HOME/cfx-price-monitor/monitoring/cfx-price-monitor}"
cd "$APP_DIR"

# --- required config ------------------------------------------------------
# CANISTER_ID : the backend canister. Staging kvg63-wiaaa-aaaao-bbabq-cai (chain 71),
#               or the eventual mainnet-1030 launch canister.
# IDENTITY_PEM: path to the SCOPED price-pusher key (chmod 600). NOT the developer key.
: "${CANISTER_ID:?set CANISTER_ID}"
: "${IDENTITY_PEM:?set IDENTITY_PEM (path to the scoped price-pusher PEM)}"
export CANISTER_ID IDENTITY_PEM

# --- target (defaults to Conflux eSpace mainnet) --------------------------
export CHAIN_ID="${CHAIN_ID:-1030}"        # 1030 = eSpace mainnet, 71 = eSpace testnet (staging)
export SYMBOL="${SYMBOL:-CFX}"
export COINGECKO_ID="${COINGECKO_ID:-conflux-token}"

# --- optional: alerting + threshold overrides (see .env.example) ----------
# export SLACK_WEBHOOK_URL="https://hooks.slack.com/services/..."
# export POLL_SEC=60 DRIFT_BPS=200 MAX_AGE_SEC=300 CALL_TIMEOUT_SEC=15

exec npm start
