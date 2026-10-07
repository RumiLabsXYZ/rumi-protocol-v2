#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -z "${POCKET_IC_BIN:-}" ]]; then
  echo "Set POCKET_IC_BIN to the PocketIC server binary before running this suite." >&2
  exit 2
fi

wasm_dir="target/wasm32-unknown-unknown/release"
canonical_wasm="$wasm_dir/rumi_3pool.wasm"
test_endpoints_wasm="$wasm_dir/rumi_3pool_test_endpoints.wasm"

# Keep the test-only ingress endpoints in a separately named fixture. Rebuild
# the canonical path last without test_endpoints so production guards are tested
# against the same artifact shape that release builds use.
cargo build --locked -p rumi_3pool --release \
  --target wasm32-unknown-unknown --features test_endpoints
cp "$canonical_wasm" "$test_endpoints_wasm"
cargo build --locked -p rumi_3pool --release --target wasm32-unknown-unknown

cargo test --locked -p rumi_3pool --test deposit_concentration_cap -- --test-threads=1
