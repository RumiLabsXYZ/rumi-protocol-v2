#!/usr/bin/env bash
set -euo pipefail

audit_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$audit_dir/../.." && pwd)
server_bin=${POCKET_IC_SERVER_BIN:-${POCKET_IC_BIN:-}}
if [[ -z "$server_bin" ]]; then
  echo "set POCKET_IC_SERVER_BIN (or POCKET_IC_BIN) to a PocketIC 9.0.3 server" >&2
  exit 2
fi
if [[ "$server_bin" != /* ]]; then
  server_bin=$(cd "$repo_root" && realpath "$server_bin")
fi
if [[ ! -x "$server_bin" ]]; then
  echo "PocketIC server is not executable: $server_bin" >&2
  exit 2
fi
server_bin="$(cd "$(dirname "$server_bin")" && pwd -P)/$(basename "$server_bin")"
tmp_parent=${TMPDIR:-/tmp}
tmp_dir=$(mktemp -d "$tmp_parent/borrow-mint-pocketic9.XXXXXX")
cleanup() {
  if [[ -L "$tmp_dir/pocket-ic" ]]; then rm "$tmp_dir/pocket-ic"; fi
  rmdir "$tmp_dir"
}
trap cleanup EXIT
ln -s "$server_bin" "$tmp_dir/pocket-ic"
server_version=$("$tmp_dir/pocket-ic" --version)
if [[ "$server_version" != "pocket-ic-server 9.0.3" ]]; then
  echo "expected PocketIC server 9.0.3, got: $server_version" >&2
  exit 1
fi
echo "PocketIC server: $server_version"

export CARGO_TARGET_DIR="$audit_dir/target"
cd "$repo_root"
cargo build --offline --release --target wasm32-unknown-unknown -p rumi_protocol_backend --bin rumi_protocol_backend --features test_endpoints
cargo build --offline --release --target wasm32-unknown-unknown -p flaky_ledger
backend_wasm="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm"
ledger_wasm="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/flaky_ledger.wasm"
export RUMI_BORROW_RECEIPT_BACKEND_WASM="$backend_wasm"
export RUMI_BORROW_RECEIPT_LEDGER_WASM="$ledger_wasm"
export POCKET_IC_BIN="$tmp_dir/pocket-ic"
cd "$audit_dir"
cargo test --offline --test borrow_mint_recovery -- --nocapture
