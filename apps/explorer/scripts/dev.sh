#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# Start replica if not already running
if ! icp network status > /dev/null 2>&1; then
  echo "Starting local replica..."
  icp network start -d
fi

# Build frontend
echo "Building frontend..."
npm run build

# Deploy both canisters
echo "Deploying canisters..."
icp deploy

# Run Vite dev server
echo "Starting Vite dev server..."
npm run dev
