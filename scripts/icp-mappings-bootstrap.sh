#!/usr/bin/env bash
#
# Bootstrap icp-cli's canister-ID mapping for the mainnet-live environment
# from the project's canister_ids.json. Run once per workstation.
#
# Why this exists: icp-cli does not auto-import canister IDs from dfx-style
# canister_ids.json. Without the mapping file, `icp canister list
# --environment mainnet-live` returns nothing and other commands that resolve
# canister names will fall back to "create" semantics.
#
# Note: the actual deploy command (`icp canister install <PRINCIPAL>
# --wasm <PATH> ...`) does NOT read this file; it targets canisters by
# principal directly. The mapping file is for human-facing read commands
# (`icp canister list`, `icp project show`, etc.).
#
# See docs/superpowers/notes/2026-05-27-icp-cli-deploy-pattern.md.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -f canister_ids.json ]; then
  echo "error: canister_ids.json not found at $repo_root" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 not found on PATH" >&2
  exit 1
fi

target=".icp/data/mappings/mainnet-live.ids.json"
# Canonical mapping path per cli.internetcomputer.org/0.2/migration/from-dfx/.
# .icp/data/ is committed to source control; .icp/cache/ is gitignored.
mkdir -p "$(dirname "$target")"

python3 -c "
import json
src = json.load(open('canister_ids.json'))
# Flatten dfx's {name: {network: principal}} into icp-cli's flat {name: principal},
# ic network only.
out = {name: nets['ic'] for name, nets in src.items() if 'ic' in nets}
json.dump(out, open('$target', 'w'), indent=2)
"

echo "wrote $target ($(grep -c '"' "$target" | awk '{print $1/2}') entries)"
