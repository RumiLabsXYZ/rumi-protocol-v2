#!/usr/bin/env bash
#
# Bootstrap icp-cli's canister-ID mapping for the mainnet-live environment
# from the project's canister_ids.json. Run once per workstation.
#
# Why this exists: icp-cli does not auto-import canister IDs from dfx-style
# canister_ids.json. Without the mapping file at .icp/data/mappings/<env>.ids.json,
# `icp canister install <NAME>` cannot resolve a canister name to its mainnet
# principal, and `icp canister list --environment <env>` returns nothing.
#
# Note: `icp canister install <PRINCIPAL>` (target by principal directly) does
# NOT need this file. But `icp canister install <NAME>` does (it reads this
# file to resolve the name to a principal).
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

count="$(python3 -c "
import json
src = json.load(open('canister_ids.json'))
# Flatten dfx's {name: {network: principal}} into icp-cli's flat {name: principal},
# ic network only.
out = {name: nets['ic'] for name, nets in src.items() if 'ic' in nets}
json.dump(out, open('$target', 'w'), indent=2)
print(len(out))
")"

echo "wrote $target ($count entries)"
