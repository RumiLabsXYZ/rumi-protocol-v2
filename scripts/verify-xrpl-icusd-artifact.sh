#!/usr/bin/env bash
# Inspect the FINAL backend Wasm (raw or gzip) for its XRPL environment marker.
#
#   --require-mainnet PATH         pass only for a normal build: marker `production`,
#                                  no fixture marker, no fixture query export.
#   --require-testnet-fixture PATH pass only for the xrpl-testnet-fixture artifact:
#                                  marker `testnet-fixture` AND the marker query export.
#   --self-test                    exercise both modes on tiny crafted modules.
#
# Mirrors chains::xrp::network::{verify_release_artifact, verify_fixture_artifact}
# (pure Rust, unit-tested as `release_rejects_fixture_artifact`). The marker is the
# public custom section `rumi:xrpl-environment` baked in by main.rs; the fixture also
# exports `canister_query xrpl_fixture_environment_marker`.
set -euo pipefail

METADATA_NAME="rumi:xrpl-environment"
FIXTURE_QUERY="canister_query xrpl_fixture_environment_marker"

usage() {
  cat <<'USAGE'
Usage:
  scripts/verify-xrpl-icusd-artifact.sh --require-mainnet PATH
  scripts/verify-xrpl-icusd-artifact.sh --require-testnet-fixture PATH
  scripts/verify-xrpl-icusd-artifact.sh --self-test
PATH may be a raw .wasm or a gzip-compressed Wasm (the icp build output).
USAGE
}

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

command -v ic-wasm >/dev/null 2>&1 || fail "ic-wasm is required (cargo install ic-wasm)"

# Decompress (if gzip) into a temp file so ic-wasm sees a raw module.
TMP_DIR=""
cleanup() { [[ -n "$TMP_DIR" ]] && rm -rf "$TMP_DIR"; return 0; }
trap cleanup EXIT

raw_wasm() {
  local path="$1"
  [[ -f "$path" ]] || fail "artifact not found: $path"
  TMP_DIR="${TMP_DIR:-$(mktemp -d)}"
  local out="$TMP_DIR/artifact.wasm"
  if [[ "$(head -c 2 "$path" | od -An -tx1 | tr -d ' \n')" == "1f8b" ]]; then
    gzip -dc -- "$path" > "$out"
  else
    cp -- "$path" "$out"
  fi
  printf '%s' "$out"
}

# Prints marker values, one per line (ic-wasm exposes the public section by name).
markers_of() {
  ic-wasm "$1" metadata "$METADATA_NAME" 2>/dev/null || true
}

has_fixture_query() {
  # Capture first: `grep -q` closing the pipe early would SIGPIPE ic-wasm and, under
  # pipefail, turn a match into a false negative.
  local info
  info="$(ic-wasm "$1" info 2>/dev/null)" || return 1
  grep -Fq "$FIXTURE_QUERY" <<<"$info"
}

check() {
  local mode="$1" path="$2" wasm markers
  wasm="$(raw_wasm "$path")"
  markers="$(markers_of "$wasm" | sed -e 's/[[:space:]]*$//' -e '/^$/d')"
  case "$mode" in
    mainnet)
      if has_fixture_query "$wasm"; then fail "$path exports the fixture marker query (xrpl_fixture_environment_marker)"; fi
      if printf '%s\n' "$markers" | grep -Fxq "testnet-fixture"; then fail "$path carries the testnet-fixture environment marker"; fi
      [[ "$markers" == "production" ]] || fail "$path: expected a single '$METADATA_NAME' = production marker, found: '${markers:-<none>}'"
      printf 'OK release artifact: %s (environment=production)\n' "$path"
      ;;
    fixture)
      has_fixture_query "$wasm" || fail "$path does not export the fixture marker query"
      [[ "$markers" == "testnet-fixture" ]] || fail "$path: expected a single '$METADATA_NAME' = testnet-fixture marker, found: '${markers:-<none>}'"
      printf 'OK fixture artifact: %s (environment=testnet-fixture)\n' "$path"
      ;;
  esac
}

self_test() {
  # Minimal valid modules: () -> () func exported as `$2`, plus an optional marker.
  local dir; dir="$(mktemp -d)"; TMP_DIR="$dir"
  local base="$dir/base.wasm"
  printf '\x00asm\x01\x00\x00\x00\x01\x04\x01\x60\x00\x00\x03\x02\x01\x00\x0a\x04\x01\x02\x00\x0b' > "$base"
  mk() { # out export value
    local out="$1" export_name="$2" value="$3" ex="$dir/ex.wasm"
    local len=${#export_name}
    # type, func, export(len = 1 + 1 + len + 2), code
    { printf '\x00asm\x01\x00\x00\x00\x01\x04\x01\x60\x00\x00\x03\x02\x01\x00'
      printf "\\x07\\x$(printf '%02x' $((len + 4)))\\x01\\x$(printf '%02x' "$len")%s\\x00\\x00" "$export_name"
      printf '\x0a\x04\x01\x02\x00\x0b'; } > "$ex"
    if [[ -n "$value" ]]; then
      ic-wasm "$ex" -o "$out" metadata "$METADATA_NAME" -d "$value" -v public
    else
      cp "$ex" "$out"
    fi
  }
  mk "$dir/prod.wasm" "canister_query q" "production"
  mk "$dir/fix.wasm" "canister_query xrpl_fixture_environment_marker" "testnet-fixture"
  mk "$dir/unmarked.wasm" "canister_query q" ""
  mk "$dir/strip.wasm" "canister_query xrpl_fixture_environment_marker" ""
  gzip -9 -n -c "$dir/fix.wasm" > "$dir/fix.wasm.gz"
  ( check mainnet "$dir/prod.wasm" ) >/dev/null || fail "self-test: production artifact rejected"
  ( check mainnet "$dir/fix.wasm" ) >/dev/null 2>&1 && fail "self-test: fixture accepted as release"
  ( check mainnet "$dir/fix.wasm.gz" ) >/dev/null 2>&1 && fail "self-test: gzip fixture accepted as release"
  ( check mainnet "$dir/unmarked.wasm" ) >/dev/null 2>&1 && fail "self-test: unmarked accepted as release"
  ( check mainnet "$dir/strip.wasm" ) >/dev/null 2>&1 && fail "self-test: stripped-marker fixture accepted as release"
  ( check fixture "$dir/fix.wasm" ) >/dev/null || fail "self-test: fixture artifact rejected"
  ( check fixture "$dir/fix.wasm.gz" ) >/dev/null || fail "self-test: gzip fixture artifact rejected"
  ( check fixture "$dir/prod.wasm" ) >/dev/null 2>&1 && fail "self-test: production accepted as fixture"
  printf 'OK self-test (7 cases)\n'
}

case "${1:-}" in
  --require-mainnet) [[ $# -eq 2 ]] || { usage >&2; exit 2; }; check mainnet "$2" ;;
  --require-testnet-fixture) [[ $# -eq 2 ]] || { usage >&2; exit 2; }; check fixture "$2" ;;
  --self-test) self_test ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
