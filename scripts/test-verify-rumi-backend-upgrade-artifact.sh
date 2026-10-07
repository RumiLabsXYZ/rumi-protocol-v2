#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
python3 "$SCRIPT_DIR/verify-rumi-backend-upgrade-artifact.py" --self-test

if ! rg -q -- 'cargo build --locked --release --target wasm32-unknown-unknown --package rumi_protocol_backend' "$SCRIPT_DIR/../icp.yaml"; then
  printf 'FAIL backend ICP build recipe must use cargo --locked\n' >&2
  exit 1
fi
if rg -q -- '14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287|44d13c58f20d53dda91030f2c6c038e9db976b5e83cd2cb019b56219b744654e|b3663122eb3b4d712d1e1da00a2f5ab468b12681aec9fd020e5477657f4e7e25' "$SCRIPT_DIR/verify-rumi-backend-upgrade-artifact.py"; then
  printf 'FAIL verifier must not accept frozen historical hashes\n' >&2
  exit 1
fi
if rg -q -- 'module_hash.{0,100}(gzip bytes|gzip hash is the live)|gzip bytes.{0,100}module_hash' "$SCRIPT_DIR/../icp.yaml"; then
  printf 'FAIL ICP module_hash documentation must identify the installed Wasm bytes\n' >&2
  exit 1
fi
printf 'PASS locked build recipe and no historical hash pins\n'
