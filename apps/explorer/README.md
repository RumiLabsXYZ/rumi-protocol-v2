# Rumi Explorer

Standalone, public explorer for the Rumi Protocol. Read-only, no auth.

## Quick start

```bash
cd apps/explorer
npm install
icp network start -d                  # local replica (project-local)
icp deploy                            # build + deploy both canisters
npm run dev --workspace=frontend      # Vite dev server
```

Then open http://localhost:5173.

## Layout

- `canisters/explorer_bff/` — Motoko BFF canister
- `frontend/` — Vite + React SPA
- `icp.yaml` — icp-cli project config

## Spec

See `docs/specs/explorer-v3-spec.md` in the repo root.
