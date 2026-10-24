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
| BB-03 Core types, state, accounting, flows, timer | done | coordinator (Opus 5.5 / Opus 5.5) | `src/rumi_icusd_bridge/src/{types,state,accounting,ids,env,admin,views,icrc21,timer,lib}.rs`, `flows/*` | BB-01 | Committed `a02b4292`; unit tests for every bucket transition, coverage invariant, admission, pull taxonomy, mint hooks, releases, holds, re-mints, hint bounds, minting-account hold | local-test |
| BB-04 EVM primitives | done | worker (Sonnet / Sonnet) | `evm/abi.rs`, `evm/tx.rs`, `evm/signer.rs` | BB-02, BB-03 | 56 tests; pins recomputed from signatures; golden EIP-1559 vectors decoded by cast and accepted by local anvil with matching hashes; low-s normalization; chain-id guard in signing | local-test |
| BB-05 RPC transport and observer | done | worker (Sonnet / Sonnet) | `evm/rpc.rs`, `observer.rs` | BB-03, BB-04 | 67 tests; quorum on parsed values (two values both reaching quorum decide nothing); finalized head = quorum-th highest height plus hash quorum; count-proven chunked log scan; bind refusals; coordinator added the cheap idle probe | local-test |
| BB-06 Transaction lanes | done | worker (Sonnet / Sonnet) | `evm/lane.rs` | BB-03, BB-04, BB-05 | 83 tests; pure decisions; attempts persisted before broadcast; receipts checked for every attempt; replacement only at an unconsumed nonce after the wall-clock delay; reorg back to pending; atomic nonce assignment; lane halt on unexplained nonce use | local-test |
| BB-07 Ledger client | done | worker (Sonnet / Sonnet) | `ledger.rs` | BB-03 | 67 tests; classification table; exact tuple matching; archive callbacks only to listed archives with id-exact replies; absence proven only at tip after the dedup window | local-test |
| BB-08 Endpoints, Candid, declarations | done | coordinator (Opus 5.5 / Opus 5.5) | `lib.rs`, `rumi_icusd_bridge.did`, `scripts/regenerate-declarations.sh`, `src/declarations/rumi_icusd_bridge/**` | BB-03..BB-07 | Committed `a02b4292`; Candid equality test; bindings regenerated by the canonical script; release Wasm 3.70 MB raw, 3.08 MB shrunk | local-test |
| BB-09a Integration harness | done | worker (Sonnet / Sonnet) | `src/rumi_icusd_bridge/tests/{evm_rpc_relay/**,support/**,harness_smoke_pic.rs}` | BB-02 | 12/12 smoke tests: relay passthrough to anvil, fault injection (mocked), tECDSA keys test_key_1, key_1 and dfx_test_key, real ledger archives, anvil reorg via snapshot | local-integration |
| BB-09b Bridge scenarios on PocketIC + anvil | done | worker (Sonnet / Sonnet) x2 | `tests/support/bridge.rs`, `tests/bridge_e2e_pic.rs`, `tests/bridge_faults_pic.rs` | BB-04..BB-08, BB-09a | On `e4213be1`: e2e 24 passed, faults 16 passed (real ledger, real tECDSA, real anvil execution; mocked/simulated parts labeled per test); C3 and mint-then-burn regressions un-ignored and passing | local-integration |
| BB-10 Frontend Bridge page | done | worker (Sonnet / Sonnet) | `src/vault_frontend/src/routes/bridge/**`, `src/vault_frontend/src/lib/services/baseBridge/**`, `src/vault_frontend/src/lib/components/bridge/**`, small edits to `config.ts`, `pnp.ts`, `+layout.svelte` | BB-02, BB-08 | Committed `ac7a517a`; 314 bridge specs and 1428 total vitest pass; svelte-check at baseline; build passes; no new deps or CSP change | local-test |
| BB-11 Docs and testnet runbook | done | worker (Haiku / Haiku) + coordinator | `foundry/DEPLOY-BASE.md` | BB-02, BB-08 | 250-line Base Sepolia runbook; every create/install/fund/broadcast/verify step labeled HELD; dry run shown before broadcast; coordinator reviewed | source |
| BB-12 Adversarial review A (accounting, authority, replay) | in progress (round 3 pending) | worker (Sonnet / Sonnet) | read-only | all | R1 FAIL at 2fee9970 plus follow-ups, 4 blockers fixed in 6208c117; R2 FAIL at e4213be1 and re-run on clean 779bd37f, 1 blocker (mint gas limit) accepted, fix in BB-16; see review record | local-integration |
| BB-13 Adversarial review B (IC semantics, cycles, upgrade, wallet flow) | in progress (round 3 pending) | worker (Sonnet / Sonnet) | read-only | all | R1 FAIL, 4 blockers fixed in 6208c117; R2 FAIL at e4213be1, 2 blockers (double release, lost timer) accepted, fixed in 779bd37f and re-checked by reviewer A; see review record | local-test |
| BB-05b Observer/RPC review fixes | done | worker (Sonnet / Sonnet) | `observer.rs`, `evm/rpc.rs` | BB-12, BB-13 | Committed in `6208c117`; adaptive log span (halving) and cap escalation, contiguity completeness with rewind, backoff, chain-id quorum, count-based C3, receipt cap; 98 observer/rpc tests | local-test |
| BB-15 Cumulative mint guard | done | worker (Sonnet / Sonnet) | contract `totalMinted`, `evm/abi.rs`, `observer.rs`, fault test | BB-05b | Committed `e4213be1`; forge 111; lib 390; mint-then-burn regression passing (attacker burn not paid, bridge halts) | local-integration |
| BB-14 Branch and draft PR | in progress | coordinator (Opus 5.5 / Opus 5.5) | none | all | Branch pushed; draft PR RumiLabsXYZ/rumi-protocol-v2#591 open (not merged); description to be updated on the frozen round-3 candidate | source |
| BB-16 Round-2 follow-up fixes | in progress | coordinator (Opus 5.5 / Opus 5.5) + workers W2, W3, W4 (Sonnet / Sonnet) | gas, lane, rpc, deposit, admin (coordinator); ledger_ops, observer, timer, state, views, types, lib, .did, declarations (W2); PocketIC tests and harness (W3); ids.rs and bridge page (W4); keccak (coordinator) | BB-12, BB-13 | Uncommitted. Interim on the dirty tree: lib 428 passed; vitest 1506 passed, 1 skipped; npm run check at baseline; build ok; PocketIC runs pending on a frozen candidate | local-test |

## Regression evidence (backend untouched)

- `git diff origin/main -- src/rumi_protocol_backend foundry/src/IcUSD.sol`: empty.
- `cargo test --offline -p rumi_protocol_backend --lib chains::`: 659 passed (private scratch target).
- `cargo test --offline -p rumi_protocol_backend --test multi_chain_supply_invariant`: 2 passed.
- `forge test --offline`: the 14 existing IcUSD tests pass unchanged.

## Held (needs separate authorization; not started)

- Public-testnet (Base Sepolia) contract deployment and canister install.
- Any mainnet deploy, controller change, contract role assignment, live signing/broadcast, or funding.
- Lookout first operational upload (held by the setup owner's approval review).
- Worktree removal pass (`scripts/cleanup_worktrees.sh --remove-eligible`): denied by the permission classifier on 2026-10-24; dry run showed 0 eligible anyway.

## Change log

- 2026-10-24: board created; recon cards dispatched.
- 2026-10-24: recon done; design and plan committed (`7ceb605e`); contract committed (`225fece7`); harness done; BB-04/05/06/07/10 dispatched.
- 2026-10-24: canister committed (`a02b4292`, 339 unit tests); BB-09b part 1 dispatched.
- 2026-10-24: public history queries (`19fbb0b5`), frontend (`ac7a517a`); first PocketIC + anvil round trip passed; BB-09b part 2 and reviews A/B (round 1) running.
- 2026-10-24: round 1 fixed (`6208c117`), C4 (`e4213be1`), draft PR #591; round 2 A and B both FAIL, reviewer B's blockers fixed (`779bd37f`); reviewer A's gas blocker and notes in BB-16 (uncommitted); review record added.
