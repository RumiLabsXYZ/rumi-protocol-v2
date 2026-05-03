#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

ENV="${1:-local}"
OUT="frontend/public/versions.json"

mkdir -p frontend/public

# Use icp canister status to get module hashes
BFF_HASH=$(icp canister status explorer_bff -e "$ENV" 2>&1 | grep -i "Module hash:" | awk '{print $3}' || echo "unknown")
ASSETS_HASH=$(icp canister status explorer_assets -e "$ENV" 2>&1 | grep -i "Module hash:" | awk '{print $3}' || echo "unknown")
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
DEPLOYED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

cat > "$OUT" <<EOF
{
  "explorer_bff": "$BFF_HASH",
  "explorer_assets": "$ASSETS_HASH",
  "git_sha": "$GIT_SHA",
  "deployed_at": "$DEPLOYED_AT",
  "environment": "$ENV"
}
EOF

echo "Wrote $OUT"
cat "$OUT"
