#!/usr/bin/env bash
set -euo pipefail

audit_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$audit_dir/../.." && pwd)
wasm_input=${1:-${RUMI_BACKEND_TEST_WASM:-}}
if [[ -z "$wasm_input" ]]; then
  echo "usage: $0 /path/to/exact-production-backend.wasm" >&2
  exit 2
fi
if [[ "$wasm_input" != /* ]]; then
  wasm_input="$PWD/$wasm_input"
fi
if [[ ! -f "$wasm_input" ]]; then
  echo "Wasm file not found: $wasm_input" >&2
  exit 2
fi

expected_sha256=571d45032008755e36d590f29ab8fe67b6c077ff0888bc132b2ff18c3b578a99
actual_sha256=$(shasum -a 256 "$wasm_input" | awk '{ print $1 }')
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Wasm SHA-256 mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi
echo "Verified raw backend Wasm SHA-256: $actual_sha256"

server_bin=${POCKET_IC_SERVER_BIN:-${POCKET_IC_BIN:-}}
if [[ -z "$server_bin" ]]; then
  echo "set POCKET_IC_SERVER_BIN (or POCKET_IC_BIN) to an executable PocketIC 9.0.3 server" >&2
  exit 2
fi
if [[ "$server_bin" != /* ]]; then
  if [[ -e "$server_bin" || -L "$server_bin" || "$server_bin" == */* ]]; then
    server_bin="$PWD/$server_bin"
  else
    server_bin=$(command -v "$server_bin" || true)
  fi
fi
if [[ -z "$server_bin" || ! -x "$server_bin" ]]; then
  echo "PocketIC server executable not found: ${POCKET_IC_SERVER_BIN:-${POCKET_IC_BIN:-}}" >&2
  exit 2
fi
server_bin="$(cd "$(dirname "$server_bin")" && pwd -P)/$(basename "$server_bin")"

tmp_parent=${TMPDIR:-/tmp}
tmp_dir=$(mktemp -d "$tmp_parent/bot001-pocketic9.XXXXXX")
cleanup() {
  if [[ -L "$tmp_dir/pocket-ic" ]]; then
    rm "$tmp_dir/pocket-ic"
  fi
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
export POCKET_IC_BIN="$tmp_dir/pocket-ic"
export RUMI_BACKEND_TEST_WASM="$wasm_input"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$repo_root/target"}

# The existing source test keeps a compile-time fallback fixture even though
# this runner always supplies the verified exact Wasm at runtime. Create only
# a temporary link when that ignored build fixture is absent; preserve existing
# build outputs and refuse to overwrite a different artifact.
fixture="$repo_root/target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm"
fixture_dir=$(dirname "$fixture")
created_fixture=0
if [[ -e "$fixture" || -L "$fixture" ]]; then
  if [[ ! -e "$fixture" ]]; then
    echo "compile-time Wasm fixture is a dangling symlink: $fixture" >&2
    exit 1
  fi
  fixture_sha256=$(shasum -a 256 "$fixture" | awk '{ print $1 }')
  if [[ "$fixture_sha256" != "$expected_sha256" ]]; then
    echo "compile-time Wasm fixture exists with a different SHA-256: $fixture_sha256" >&2
    exit 1
  fi
else
  mkdir -p "$fixture_dir"
  ln -s "$wasm_input" "$fixture"
  created_fixture=1
fi
cleanup_fixture() {
  if [[ "$created_fixture" == 1 && -L "$fixture" ]]; then
    rm "$fixture"
  fi
}
trap 'cleanup_fixture; cleanup' EXIT

cd "$audit_dir"
cargo test --offline --features pocketic9_wrapper --test bot001 -- --nocapture
