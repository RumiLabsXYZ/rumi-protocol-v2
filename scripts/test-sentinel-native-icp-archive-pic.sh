#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

: "${POCKET_IC_BIN:?Set POCKET_IC_BIN to a local PocketIC server binary}"
: "${RUMI_TEST_NNS_LEDGER_WASM_GZ:?Set this to the pinned 69b755 official NNS ledger gzip}"
[[ -x "$POCKET_IC_BIN" ]] || { echo "POCKET_IC_BIN is not executable: $POCKET_IC_BIN" >&2; exit 2; }
[[ -f "$RUMI_TEST_NNS_LEDGER_WASM_GZ" ]] || { echo "NNS ledger gzip not found: $RUMI_TEST_NNS_LEDGER_WASM_GZ" >&2; exit 2; }

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rumi-security-target}"
canonical_sentinel_wasm="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/rumi_cycle_sentinel.wasm"
export RUMI_TEST_SENTINEL_WASM="$canonical_sentinel_wasm"
export RUMI_TEST_SENTINEL_MOCK_WASM="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/cycle_sentinel_test_mock.wasm"
temp_dir="$(mktemp -d /private/tmp/rumi-sentinel-archive.XXXXXX)"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [[ -f "$temp_dir/rumi_cycle_sentinel.default.wasm" ]]; then
    cp "$temp_dir/rumi_cycle_sentinel.default.wasm" "$canonical_sentinel_wasm" || status=1
  else
    echo "No saved default Sentinel Wasm; cannot restore test-feature target artifact" >&2
    status=1
  fi
  rm -f "$temp_dir/rumi_cycle_sentinel.default.wasm" \
    "$temp_dir/rumi_cycle_sentinel.test_endpoints.wasm"
  rmdir "$temp_dir" 2>/dev/null || status=1
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ensure_disk_floor() {
  local available_kib
  available_kib="$(df -Pk "$CARGO_TARGET_DIR" | awk 'NR == 2 { print $4 }')"
  if [[ "$available_kib" -lt 1048576 ]]; then
    echo "Stopping before Cargo work: less than 1 GiB free on the workspace volume" >&2
    return 1
  fi
}

ensure_disk_floor
cargo build --locked -p rumi_cycle_sentinel \
  --target wasm32-unknown-unknown --release
cp "$canonical_sentinel_wasm" "$temp_dir/rumi_cycle_sentinel.default.wasm"
ensure_disk_floor
cargo build --locked -p rumi_cycle_sentinel --features test_endpoints \
  --target wasm32-unknown-unknown --release
cp "$canonical_sentinel_wasm" "$temp_dir/rumi_cycle_sentinel.test_endpoints.wasm"
export RUMI_TEST_SENTINEL_WASM="$temp_dir/rumi_cycle_sentinel.test_endpoints.wasm"
ensure_disk_floor
cargo build --locked -p cycle_sentinel_test_mock \
  --target wasm32-unknown-unknown --release
ensure_disk_floor
cargo test --locked -p rumi_cycle_sentinel --test native_icp_archive_pic \
  sentinel_reconciles_exact_native_icp_receipt_from_official_archive_callback \
  -- --ignored --exact --nocapture --test-threads=1
