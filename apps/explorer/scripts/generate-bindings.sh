#!/usr/bin/env bash
# Regenerate TypeScript bindings from explorer_bff.did without running the dev server.
# The Vite plugin (icpBindgen) also runs this automatically during `npm run dev` and
# `npm run build`, so you only need this script if you want to generate bindings in CI
# or outside of a Vite context.
set -euo pipefail

cd "$(dirname "$0")/.."

# CLI flags: --did-file <path> --out-dir <dir>
# Using --declarations-typescript to avoid emitting .js + .d.ts pairs (single .did.ts instead).
# Using --declarations-flat so declaration files land directly in the outDir instead of
# a declarations/ subfolder, keeping imports clean (e.g. ./bindings/explorer_bff/explorer_bff.did).
npx @icp-sdk/bindgen \
  --did-file canisters/explorer_bff/explorer_bff.did \
  --out-dir frontend/src/bindings/explorer_bff \
  --declarations-typescript \
  --declarations-flat \
  --force
