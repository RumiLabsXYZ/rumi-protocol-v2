# Base icUSD bridge: canonical Kanban board

This is the single canonical board for the ICP icUSD <-> Base bridge work.
`base-icusd-bridge-lookout.json` is derived from this file and must stay aligned with it.

- Worktree branch: `codex/base-icusd-bridge`
- Baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90` (fetched 2026-10-24)
- Coordinator: Claude (Opus 5.5, main thread). Supervisor: Miles (Codex), for Rob.
- Boundaries: no merge, no deploy, no live signing or broadcast, no funds movement, no controller/role changes.
- Lookout: **FIRST OPERATIONAL UPLOAD HELD** (see `base-icusd-bridge-operating-rules.md`). The projection is maintained locally only; nothing is published.

## Proof levels

| Level | Meaning |
| --- | --- |
| `none` | Not started or no evidence yet. |
| `source` | Source read or written; no test executed. |
| `local-test` | Deterministic local tests (cargo unit, forge, vitest) passed. |
| `local-integration` | Local PocketIC and/or local anvil execution passed. |

No card may claim a live, testnet, or mainnet proof level. Live work is listed under Held.

## Shared build coordination

- One cargo target dir per worktree (this worktree's `target/`). Do not share `CARGO_TARGET_DIR` across sessions.
- Workers edit only their owned files and run focused `cargo test --offline -p rumi_icusd_bridge --lib <module>`; a compile failure in a file someone else owns means wait and retry, never edit it.
- The coordinator owns shared interfaces: `types.rs`, `state.rs`, `accounting.rs`, `ids.rs`, `flows/*`, `timer.rs`, `lib.rs`, `admin.rs`, `views.rs`, `icrc21.rs`, the `.did`.
- Disk is tight (about 11 GiB free). Workers must not create extra worktrees or target dirs.

## Cards

| Card | State | Owner (requested / actual model) | Files owned | Depends on | Acceptance evidence | Proof |
| --- | --- | --- | --- | --- | --- | --- |
| BB-00 Baseline and inventory | done | coordinator (Opus 5.5 / Opus 5.5) | `docs/coordination/*` | none | Worktree inventory run (0 eligible, 65 blocked); removal pass denied by the permission classifier and not retried; baseline recorded | source |
| BB-R1 Recon: Foundry contracts | done | worker (Sonnet / Sonnet) | read-only | BB-00 | IcUSD has no pause/cap, `burn(amount,vault_id)` is repayment, Base absent from backend `evm_chain_config`, no Foundry CI; baseline `forge test` 14/14 | source |
| BB-R2 Recon: backend EVM rails | done | worker (Sonnet / Sonnet) | read-only | BB-00 | Mint pipeline gaps (no pre-broadcast claim, RBF overwrites hash, tick-count stuck detection), global debt-backed invariant, raw EVM RPC `request` transport | source |
| BB-R3 Recon: backend core, build, Candid, CI | done | worker (Sonnet / Sonnet) | read-only | BB-00 | Backend code section about 66 KB past reserve; outcome taxonomy; ICRC-3 rules; sentinel stable layout; one CI workflow | source |
| BB-R4 Recon: vault frontend | done | worker (Sonnet / Sonnet) | read-only | BB-00 | EIP-1193 precedent, CSP, Oisy rule, delegation targets, declarations script | source |
| BB-01 Design spec and plan | done | coordinator (Opus 5.5 / Opus 5.5) | `docs/superpowers/specs/2026-10-24-base-icusd-bridge-design.md`, `docs/superpowers/plans/2026-10-24-base-icusd-bridge.md` | BB-R1..R4 | Committed `7ceb605e` (force-added past `/docs/` ignore) | source |
| BB-02 Base contract | done | worker (Sonnet / Sonnet) | `foundry/src/BridgedIcUSD.sol`, `foundry/test/BridgedIcUSD.t.sol`, `foundry/script/DeployBridgedIcUSD.s.sol`, `foundry/test/DeployBridgedIcUSD.t.sol` | BB-R1 | Committed `225fece7`; `forge test --offline` 105 passed (14 existing unchanged), coordinator rerun confirmed; runtime 8,205 B; 12 mutation checks caught | local-test |
| BB-03 Core types, state, accounting, flows, timer | in progress | coordinator (Opus 5.5 / Opus 5.5) | `src/rumi_icusd_bridge/src/{types,state,accounting,ids,env,admin,views,icrc21,timer,lib}.rs`, `flows/*` | BB-01 | Unit tests for every bucket transition, coverage invariant, admission, pull taxonomy, mint hooks, releases, holds, re-mints | local-test |
| BB-04 EVM primitives | in progress | worker (Sonnet / Sonnet) | `evm/abi.rs`, `evm/tx.rs`, `evm/signer.rs` | BB-02, BB-03 | Pins vs Foundry, golden EIP-1559 vector via `cast decode-transaction`, strict decoders | none |
| BB-05 RPC transport and observer | in progress | worker (Sonnet / Sonnet) | `evm/rpc.rs`, `observer.rs` | BB-03, BB-04 | Quorum on parsed values, finalized head + hash quorum, count-proven log scan, bind checks | none |
| BB-06 Transaction lanes | in progress | worker (Sonnet / Sonnet) | `evm/lane.rs` | BB-03, BB-04, BB-05 | Pure decision tests for every lost-reply, replacement, reorg, cancel and halt case | none |
| BB-07 Ledger client | in progress | worker (Sonnet / Sonnet) | `ledger.rs` | BB-03 | Classification table, exact ICRC-3 matching, archive callbacks limited to listed archives, proven absence | none |
| BB-08 Endpoints, Candid, declarations | in progress | coordinator (Opus 5.5 / Opus 5.5) | `lib.rs`, `rumi_icusd_bridge.did`, `scripts/regenerate-declarations.sh`, `src/declarations/rumi_icusd_bridge/**` | BB-03..BB-07 | Candid equality test passes; bindings generated by the canonical script (not hand-edited) | local-test |
| BB-09a Integration harness | done | worker (Sonnet / Sonnet) | `src/rumi_icusd_bridge/tests/{evm_rpc_relay/**,support/**,harness_smoke_pic.rs}` | BB-02 | 12/12 smoke tests: relay passthrough to anvil, fault injection (mocked), tECDSA keys test_key_1, key_1 and dfx_test_key, real ledger archives, anvil reorg via snapshot | local-integration |
| BB-09b Bridge scenarios on PocketIC + anvil | todo | worker (Sonnet) | `src/rumi_icusd_bridge/tests/bridge_e2e_pic.rs` | BB-04..BB-08, BB-09a | Round trip, lost replies, replacement, revert+refund, reorg, inconsistent provider, held re-mint, caps/pauses, upgrade mid-flight, invariants | none |
| BB-10 Frontend Bridge page | in progress | worker (Sonnet / Sonnet) | `src/vault_frontend/src/routes/bridge/**`, `src/vault_frontend/src/lib/services/baseBridge/**`, `src/vault_frontend/src/lib/components/bridge/**`, small edits to `config.ts`, `pnp.ts`, `+layout.svelte` | BB-02, BB-08 | vitest specs for both flows, wallet/network changes, recovery; `npm run check` adds no errors; build passes | none |
| BB-11 Docs and testnet runbook | todo | worker (Haiku) + coordinator | `foundry/DEPLOY-BASE.md`, handoff runbook | BB-02, BB-08 | Runbook with every live step marked held | none |
| BB-12 Adversarial review A (accounting, authority, replay) | todo | worker (Sonnet) | read-only | BB-03..BB-09b | Findings with dispositions | none |
| BB-13 Adversarial review B (state, Candid, upgrade, wallet flow) | todo | worker (Sonnet) | read-only | BB-03..BB-10 | Findings with dispositions | none |
| BB-14 Branch and draft PR | todo | coordinator (Opus 5.5) | none | all | Branch pushed, draft PR open, handoff updated | none |

## Held (needs separate authorization; not started)

- Public-testnet (Base Sepolia) contract deployment and canister install.
- Any mainnet deploy, controller change, contract role assignment, live signing/broadcast, or funding.
- Lookout first operational upload (held by the setup owner's approval review).
- Worktree removal pass (`scripts/cleanup_worktrees.sh --remove-eligible`): denied by the permission classifier on 2026-10-24; dry run showed 0 eligible anyway.

## Change log

- 2026-10-24: board created; recon cards dispatched.
- 2026-10-24: recon done; design and plan committed (`7ceb605e`); contract committed (`225fece7`); harness done; BB-04/05/06/07/10 dispatched.
