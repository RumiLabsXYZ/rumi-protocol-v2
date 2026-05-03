# Rumi Explorer

Standalone, public read-only explorer for the Rumi Protocol on the Internet Computer.

## Architecture

- **explorer_bff** (Motoko BFF) — composes data from upstream canisters into shaped DTOs
- **explorer_assets** (asset canister) — serves the SPA + sets `ic_env` cookie with canister IDs
- **mock_analytics** / **mock_backend** — local dev mocks that mirror the real Rumi `rumi_analytics` / `rumi_protocol_backend` Candid surface

## Local development

```bash
cd apps/explorer
npm install
icp network start -d                    # local replica (project-local, port 8000)
icp deploy                              # builds + deploys all 4 canisters

# First-time BFF install requires init args (mock canister IDs):
ANALYTICS_ID=$(icp canister status mock_analytics -e local | grep "Canister Id:" | awk '{print $3}')
BACKEND_ID=$(icp canister status mock_backend -e local | grep "Canister Id:" | awk '{print $3}')
icp canister install explorer_bff --mode reinstall \
  --args "(record { analytics = principal \"$ANALYTICS_ID\"; backend = principal \"$BACKEND_ID\" })" \
  -e local --yes

# Open the explorer in a browser
icp project show -e local | grep explorer_assets
# Visit http://<assets-canister-id>.localhost:8000/
```

## Frontend dev loop

```bash
npm run dev --workspace=frontend         # vite dev server with bindgen + cookie shim
```

## Layout

- `canisters/explorer_bff/` — Motoko BFF
- `canisters/mock_analytics/` — analytics mock (replace with real ID in Plan 6 mainnet deploy)
- `canisters/mock_backend/` — backend mock (replace with real ID in Plan 6 mainnet deploy)
- `frontend/` — Vite + React + TypeScript + Tailwind + recharts
- `scripts/dev.sh` — convenience: start replica + build + deploy + dev server
- `scripts/capture-versions.sh` — captures wasm hashes into `frontend/public/versions.json`

## Spec + plans

- Spec: [`docs/specs/explorer-v3-spec.md`](../../docs/specs/explorer-v3-spec.md) (gitignored — local file)
- Plans 1-6: [`docs/superpowers/plans/`](../../docs/superpowers/plans/) (also gitignored)

## Mainnet deploy

Pending explicit authorization. See Plan 6 Phase 4. The BFF accepts source canister IDs as init args, so:

```bash
icp canister install explorer_bff --mode install -e ic \
  --args "(record { analytics = principal \"<real-rumi-analytics-id>\"; backend = principal \"<real-rumi-backend-id>\" })"
```

Real Rumi mainnet canister IDs (per repo memory):
- `rumi_analytics`: `jagpu-pyaaa-aaaap-qtm6q-cai`
- `rumi_protocol_backend`: `tfesu-vyaaa-aaaap-qrd7a-cai`

## Known limitations (v1)

- Read-only, no auth
- No mobile-first UI (responsive cards above 768px, horizontal scroll on mobile tables)
- English only
- Single-source events (only backend events; multi-source merging is a follow-up)
- Stub data on mainnet swap until Plan 6's deploy lands
