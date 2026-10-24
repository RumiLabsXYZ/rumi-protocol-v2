# Base icUSD bridge: milestone handoff

Compact status for supervisors. The canonical board is `base-icusd-bridge-kanban.md`; this file
summarizes it at milestones. Evidence here is local only (source, unit, PocketIC, anvil). Nothing
has been deployed, signed live, broadcast to a public network, or funded.

## Where

- Worktree: `.worktrees/base-icusd-bridge-20261024` in the rumi-protocol-v2 checkout.
- Branch: `codex/base-icusd-bridge` (local; not pushed yet).
- Baseline: `origin/main` `32358d23d98b6430cab0236a3245519c9322fe90`.

## Milestone 1 (2026-10-24, about 02:00 PT): interfaces fixed, workers running

Commits on the branch:

| Commit | Content |
| --- | --- |
| `7ceb605e` | Design spec, implementation plan, board, operating rules (Lookout upload held) |
| `225fece7` | `BridgedIcUSD` Base contract, tests, deploy script |

Uncommitted (in progress, compiles): crate `src/rumi_icusd_bridge` with core types, stable state,
accounting, flows, timer, endpoints, generated `.did`; harness (relay canister, anvil and PocketIC
support); generated declarations; workspace and generator registration.

Decisions taken (coordinator, under the user's authorization):

- Separate canister `rumi_icusd_bridge`, not a backend module: backend code section is about 66 KB
  past its reserve, the bridge must not run inside the icUSD minting account, and the existing EVM
  mint pipeline's reconciliation semantics do not fit. The backend is untouched.
- New versioned contract `BridgedIcUSD` (no repayment burn; `bridgeOut` binds amount, ICP account,
  transfer id and sequence). `IcUSD.sol` and its backend consumers are untouched.
- Settlement finality: Base `finalized` tag (L1 batch finality), quorum-agreed number and hash.
- Escrow-backed buckets are kept out of the backend's debt-backed `chain_supplies` invariant.

Checks run (exact):

| Check | Result |
| --- | --- |
| `forge test --offline` (foundry/) | 105 passed, 0 failed (14 existing IcUSD tests unchanged) |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 105 passed at last coordinator run (worker modules partially landed) |
| `cargo test --offline -p rumi_icusd_bridge --test harness_smoke_pic` with `POCKET_IC_BIN` | 12 passed (worker-reported) |
| Candid equality test (`candid_interface_matches_checked_in_did`) | passes |
| `bash scripts/regenerate-declarations.sh rumi_icusd_bridge` | ok; only the new canister's bindings created |

Workers (requested / actual model): BB-04, BB-05, BB-06, BB-07, BB-10 Sonnet / Sonnet, running.
All recon (BB-R1..R4), BB-02 and BB-09a were Sonnet / Sonnet. No Haiku task has run yet; BB-11
(runbook mechanics) is queued for Haiku.

## Next action

1. Integrate BB-04/05/06/07 deliveries, run the full crate suite and the wasm release build, commit.
2. Dispatch BB-09b (bridge scenarios on PocketIC + anvil with real ledger and tECDSA).
3. Integrate BB-10 frontend, then two adversarial reviews (BB-12, BB-13), fixes, push, draft PR.

## Unresolved risks (as of this milestone)

- No end-to-end evidence yet: the bridge has not executed a canister-signed transaction on anvil.
- pocket-ic 6.0.0 `await_call` panics after 100 rounds; the harness works around it.
- Lane throughput is one unincluded tx at a time per lane (acceptable for v1, documented).
- `rumi_analytics` still reports ICP ledger supply as circulating; subtracting escrow is a follow-up.

## Held live actions

Base Sepolia or Base deployment, canister creation or install on any network, contract role
assignment, gas funding, enabling deposits, any mainnet change, and the first Lookout upload.
