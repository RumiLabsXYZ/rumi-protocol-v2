# Base icUSD bridge: milestone handoff

Compact status for supervisors. The canonical board is `base-icusd-bridge-kanban.md`; this file
summarizes it at milestones. Evidence here is local only (source, unit, PocketIC, anvil). Nothing
has been deployed, signed live, broadcast to a public network, or funded.

## Where

- Worktree: `.worktrees/base-icusd-bridge-20261024` in the rumi-protocol-v2 checkout.
- Branch: `codex/base-icusd-bridge`, pushed; draft PR RumiLabsXYZ/rumi-protocol-v2#591 (not merged).
- Review findings and resolutions: `base-icusd-bridge-review-record.md`.
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

## Milestone 2 (2026-10-24): canister committed

| Commit | Content |
| --- | --- |
| `88a403ed` | Milestone 1 board, projection, handoff |
| `a02b4292` | `rumi_icusd_bridge` canister, PocketIC + anvil harness, generated declarations, runbook |

| Check | Result |
| --- | --- |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 339 passed |
| `cargo clippy --offline -p rumi_icusd_bridge --lib` | no warnings in bridge sources |
| `cargo build --offline --release --target wasm32-unknown-unknown -p rumi_icusd_bridge` | ok; 3,699,998 B raw, 3,078,884 B after `ic-wasm shrink` |
| `cargo test --offline -p rumi_protocol_backend --lib chains::` (private target) | 659 passed; backend source unchanged |
| `cargo test --offline -p rumi_protocol_backend --test multi_chain_supply_invariant` | 2 passed |

Worker models so far (requested / actual): BB-04, BB-05, BB-06, BB-07, BB-09a Sonnet / Sonnet; BB-11 Haiku / Haiku.

Decisions since milestone 1: hints never select the fast timer interval and are capped per caller;
releases to the ledger minting account are held; idle observer probes `bridgeOutCount()` at the
finalized tag between hourly full passes; supply reconciliation every 30 minutes; no `icp.yaml`
entry; lane halts on unexplained nonce use stay halted until a controller decision.

## Milestone 3 (2026-10-24): review round 1 fixed, cumulative mint guard, all local suites green

| Commit | Content |
| --- | --- |
| `19fbb0b5` | Public history queries, Base-sender index, contract binding refresh |
| `ac7a517a` | Frontend Bridge page |
| `6208c117` | Review round 1 fixes, Oisy two-click, PocketIC + anvil e2e and fault suites |
| `19d589c3` | Frontend types against generated bindings |
| `e4213be1` | Cumulative mint guard (invariant C4) |

| Check (on `e4213be1`) | Result |
| --- | --- |
| `forge test --offline` | 111 passed |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 390 passed |
| `cargo test --offline -p rumi_icusd_bridge --test bridge_e2e_pic` (PocketIC + anvil) | 24 passed |
| `cargo test --offline -p rumi_icusd_bridge --test bridge_faults_pic` | 16 passed |
| `cargo test --offline -p rumi_icusd_bridge --test harness_smoke_pic` | 12 passed |
| vitest (vault_frontend) | 1455 passed (341 bridge) |
| `npm run check` | 30 errors / 66 warnings, identical to baseline, none in bridge files |
| `npm run build` | passes |
| Wasm | 3,822,456 B raw, 3,185,887 B shrunk |

Review round 1: both reviewers FAIL with 8 distinct blockers (2 shared); all accepted and fixed (see the design's
"Review round 1 changes"). The scenario suites also found the C3 reporting bug and the unauthorized mint-then-burn
payout gap; both fixed with regression tests. Round 2 (fresh reviewers) running.

## Milestone 4 checkpoint (2026-10-24, about 06:00 PT): round 2 adjudicated, follow-ups uncommitted

| Commit | Content |
| --- | --- |
| `92a6941d`, `b50297a1` | Milestone 3 handoff and board, draft PR recorded |
| `779bd37f` | Round-2 fixes for reviewer B's blockers (double release, lost timer) |

Round 2: reviewer A FAIL (1 blocker, mint gas limit) at `e4213be1` and re-run at `779bd37f`;
reviewer B FAIL (2 blockers) at `e4213be1`. All three accepted. Details, original-failure evidence
and dispositions: `base-icusd-bridge-review-record.md`.

Evidence for `779bd37f`: reviewer A's clean-tree runs (lib 394, faults 16, e2e 24, forge 111,
vitest 1455). The e4213be1 results (24 / 16) do not validate `779bd37f` or later writes.

Owned, uncommitted round-2 follow-ups (BB-16): mint gas sizing and gas-exact preflight
simulation, ingress filter, re-mint ordering, EIP-55 (canister and page), gas-wait notice, owner
token on lane guards, ICRC-3 absence-proof freshness, deferral visibility, timer cadence,
controller capture in `admin_set_rpc_services`, docs and runbook checks. Interim local results
on the dirty tree (not a frozen candidate): lib 428 passed; vitest 1506 passed (1 skipped);
`npm run check` at baseline; build ok. PocketIC test work for these changes is in progress.

## Next action

1. Finish the BB-16 writes, freeze one candidate commit, rebuild the Wasm and record its hash.
2. Run lib, forge, the four PocketIC suites and the frontend checks on that candidate; record which
   suites used which Wasm.
3. Round 3: fresh independent re-check of the material fixes on the frozen candidate.
4. Update PR #591 and this handoff. Merge, deploy and live actions stay held.

## Unresolved risks (as of this milestone)

- Local evidence only: canister-signed transactions executed on a local anvil; nothing on Base Sepolia or Base.
- pocket-ic 6.0.0 `await_call` panics after 100 rounds; the harness works around it.
- Lane throughput is one unincluded tx at a time per lane (acceptable for v1, documented).
- `rumi_analytics` still reports ICP ledger supply as circulating; subtracting escrow is a follow-up.

## Held live actions

Base Sepolia or Base deployment, canister creation or install on any network, contract role
assignment, gas funding, enabling deposits, any mainnet change, and the first Lookout upload.
