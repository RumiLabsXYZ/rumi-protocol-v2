#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$repo_root"

base_ref="${1:-origin/main}"
base_commit="$(git merge-base HEAD "$base_ref")"
old_did="$(mktemp)"
trap 'rm -f "$old_did"' EXIT

git show "$base_commit:src/stability_pool/stability_pool.did" > "$old_did"
didc check src/stability_pool/stability_pool.did "$old_did"
