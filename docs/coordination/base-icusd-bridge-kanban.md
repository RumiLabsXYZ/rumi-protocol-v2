# Base icUSD bridge: canonical Kanban board

This is the single canonical board for the ICP icUSD <-> Base bridge work.
`base-icusd-bridge-lookout.json` is derived from this file and must stay aligned with it.
`base-icusd-bridge-presentation.json` holds the board's short plain-English status and term
definitions; update it with every material board change.

- Worktree branch: `codex/base-icusd-bridge`
- Baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90` (fetched 2026-10-24)
- Coordinator: Claude (Opus 5.5, main thread). Supervisor: Miles (Codex), for Rob.
- Boundaries: no merge, no deploy, no live signing or broadcast, no funds movement, no controller/role changes.
- Delivery: local implementation complete; draft PR RumiLabsXYZ/rumi-protocol-v2#591 open, not merged, not deployed. Miles (supervisor) accepted the local implementation and reviewable draft PR milestone at `5fdf2145`, relying on the recorded local test and review evidence. That acceptance is not production safety, live readiness, or approval to merge, deploy, sign, fund or change roles.
- Lookout: Rob approved uploading this board's sanitized metadata to his private Lookout backend, and future sanitized updates (2026-10-24, verified by Miles; see `base-icusd-bridge-operating-rules.md`). The first snapshot was published after the setup owner's cutover and verified as a no-op readback; this worktree's coordinator now publishes each changed sanitized checkpoint (see the event rule there).

## Proof levels

| Level | Meaning |
| --- | --- |
| `none` | Not started or no evidence yet. |
| `source` | Source read or written; no test executed. |
| `local-test` | Deterministic local tests (cargo unit, forge, vitest) passed. |
| `local-integration` | Local PocketIC and/or local anvil execution passed. |
| `public-created` | Authorized Base Sepolia run only: new test canisters created and funded, controllers verified; no code installed. |
| `public-installed` | Authorized run: code installed and its module hash read back equal to the packet's install artifact. |
| `public-signed` | Authorized run: a transaction signed by the bridge's threshold key. |
| `public-broadcast` | Authorized run: a transaction accepted by Base Sepolia providers (hash known). |
| `public-receipt` | Authorized run: a receipt with status 1 on two providers. |
| `public-finalized` | Authorized run: the receipt block is at or behind the finalized head with a matching canonical hash. |
| `public-settled` | Authorized run: the bridge records the operation complete and ledger, Base and supply-audit readbacks agree. |

The `public-*` levels apply only to the isolated Base Sepolia run in the deployment packet (synthetic
test assets, chain 84532), each at the exact step it names. No card may claim a production or
mainnet-funds proof level. Exact targets, transactions and funding stay in the executor's private
run record, never on this board.

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
| BB-06 Transaction lanes | done | worker (Sonnet / Sonnet) | `evm/lane.rs` | BB-03, BB-04, BB-05 | 83 tests; attempts persisted before broadcast; receipts checked for every attempt; replacement only at an unconsumed nonce after the wall-clock delay; reorg back to pending; atomic nonce assignment; lane halt on unexplained nonce use | local-test |
| BB-07 Ledger client | done | worker (Sonnet / Sonnet) | `ledger.rs` | BB-03 | 67 tests; classification table; exact tuple matching; archive callbacks only to listed archives with id-exact replies; absence proven only at tip after the dedup window | local-test |
| BB-08 Endpoints, Candid, declarations | done | coordinator (Opus 5.5 / Opus 5.5) | `lib.rs`, `rumi_icusd_bridge.did`, `scripts/regenerate-declarations.sh`, `src/declarations/rumi_icusd_bridge/**` | BB-03..BB-07 | Committed `a02b4292`; Candid equality test; bindings regenerated by the canonical script; release Wasm 3.70 MB raw, 3.08 MB shrunk | local-test |
| BB-09a Integration harness | done | worker (Sonnet / Sonnet) | `src/rumi_icusd_bridge/tests/{evm_rpc_relay/**,support/**,harness_smoke_pic.rs}` | BB-02 | 12/12 smoke tests: relay passthrough to anvil, fault injection (mocked), tECDSA keys test_key_1, key_1 and dfx_test_key, real ledger archives, anvil reorg via snapshot | local-integration |
| BB-09b Bridge scenarios on PocketIC + anvil | done | worker (Sonnet / Sonnet) x2 | `tests/support/bridge.rs`, `tests/bridge_e2e_pic.rs`, `tests/bridge_faults_pic.rs` | BB-04..BB-08, BB-09a | On `e4213be1`: e2e 24 passed, faults 16 passed (real ledger, real tECDSA, real anvil execution; mocked/simulated parts labeled per test); C3 and mint-then-burn regressions un-ignored and passing | local-integration |
| BB-10 Frontend Bridge page | done | worker (Sonnet / Sonnet) | `src/vault_frontend/src/routes/bridge/**`, `src/vault_frontend/src/lib/services/baseBridge/**`, `src/vault_frontend/src/lib/components/bridge/**`, small edits to `config.ts`, `pnp.ts`, `+layout.svelte` | BB-02, BB-08 | Committed `ac7a517a`; 314 bridge specs and 1428 total vitest pass; svelte-check at baseline; build passes; no new deps or CSP change | local-test |
| BB-11 Docs and testnet runbook | done | worker (Haiku / Haiku) + coordinator | `foundry/DEPLOY-BASE.md` | BB-02, BB-08 | 250-line Base Sepolia runbook; every create/install/fund/broadcast/verify step labeled HELD; dry run shown before broadcast; coordinator reviewed | source |
| BB-12 Adversarial review A (accounting, authority, replay) | done | worker (Sonnet / Sonnet) | read-only | all | R1 FAIL at 2fee9970, fixed in 6208c117; R2 FAIL at e4213be1, gas blocker fixed in eab65670; R3 PASS at eab65670 (one minor regression, fixed in 8f2abbfc); see review record | local-integration |
| BB-13 Adversarial review B (IC semantics, cycles, upgrade, wallet flow) | done | worker (Sonnet / Sonnet) | read-only | all | R1 FAIL, fixed in 6208c117; R2 FAIL at e4213be1, fixed in 779bd37f; R3 FAIL at eab65670 (runbook reads refused, no ingress size bound), fixed in 8f2abbfc; fresh re-check of 8f2abbfc PASS | local-integration |
| BB-05b Observer/RPC review fixes | done | worker (Sonnet / Sonnet) | `observer.rs`, `evm/rpc.rs` | BB-12, BB-13 | Committed in `6208c117`; adaptive log span (halving) and cap escalation, contiguity completeness with rewind, backoff, chain-id quorum, count-based C3, receipt cap; 98 observer/rpc tests | local-test |
| BB-15 Cumulative mint guard | done | worker (Sonnet / Sonnet) | contract `totalMinted`, `evm/abi.rs`, `observer.rs`, fault test | BB-05b | Committed `e4213be1`; forge 111; lib 390; mint-then-burn regression passing (attacker burn not paid, bridge halts) | local-integration |
| BB-14 Branch and draft PR | done | coordinator (Opus 5.5 / Opus 5.5) | none | all | Branch pushed; code reviewed at 8f2abbfc, docs-only commits after it; draft PR RumiLabsXYZ/rumi-protocol-v2#591 open, not merged, not deployed; supervisor accepted the local delivery milestone at 5fdf2145 | source |
| BB-16 Round-2 follow-up fixes | done | coordinator (Opus 5.5 / Opus 5.5) + workers W2, W3, W4 (Sonnet / Sonnet) | gas, lane, rpc, deposit, admin, keccak (coordinator); ledger_ops, observer, timer, state, views, types, lib, .did, declarations (W2); PocketIC tests and harness (W3); ids.rs and bridge page (W4) | BB-12, BB-13 | Frozen candidate eab65670, Wasm de4ad399 (sha256 prefix; clean rebuild matches); on it: round2 11 (7 new, all failing on the old Wasm), e2e 24, faults 16, smoke 12; lib 428; clippy clean; vitest 1506 (1 skipped); check at baseline; build ok | local-integration |
| BB-17 Round-3 fixes | done | coordinator (Opus 5.5 / Opus 5.5) | lib.rs, state.rs, timer.rs, lane.rs, deposit.rs, icrc21.rs, runbook, design, bridge page, PocketIC tests | BB-12, BB-13 | 8f2abbfc, Wasm c03ce4f1 (clean rebuild matches); fb005f1b tests and docs, Wasm byte-identical: lib 432; round2 12, e2e 24, faults 16, smoke 12 (10 on a serial rerun); forge 111; vitest 1507; check at baseline; build ok; re-check PASS | local-integration |
| BB-18 Follow-up: clear-halt success coverage | done | worker W6 (Sonnet / Sonnet) | new PocketIC test file, additive test helpers | BB-17 | 4169feb8: a real halt cleared on fresh checks while the canister's own timer fires mid-check; the escrowed mint goes out within seconds; a mutant without the post-clear re-arm fails | local-integration |
| BB-19 Follow-up: refund and sweep re-arm | done | worker W5 (Sonnet / Sonnet) + W6 tests | deposit flows | BB-17 | 4169feb8: an unresolved refund or sweep re-arms the timer, same tuple, no new transfer; 9 unit tests plus flaky-ledger PocketIC tests counting exactly one transfer; mutants fail | local-integration |
| BB-20 Follow-up: RPC provider-count limit | done | worker W5 (Sonnet / Sonnet) | state validation, init and upgrade | BB-17 | 4169feb8: at most 5 providers, checked at install, on replacement before any provider call, and at upgrade; exact-boundary unit tests; upgrade wiring covered by existing upgrade tests only (pre-merge follow-up) | local-integration |
| BB-21 Base Sepolia read-only preflight | done | worker W7 (Sonnet / Sonnet) + coordinator | read-only | none | Read-only: CLI ready via a separate test project; cycles fit a reduced budget; test key and providers checked; EVM RPC request costs measured; deploy script simulated; deployer signer held | source |
| BB-22 Focused reviews of the follow-up candidate | done | worker (Sonnet / Sonnet) x2 | read-only | BB-18, BB-19, BB-20 | Review A PASS and review B PASS on 4169feb8, no blockers; raw Wasm a0049b9a passes all five PocketIC suites run one after another (71 tests) | local-integration |
| BB-23 Base Sepolia deployment packet | done (ready for executor validation) | coordinator (Opus 5.5 / Opus 5.5) | deployment packet and runbook | BB-21, BB-22 | Packet with raw and install-artifact hashes kept apart; install artifact 0b324387 passes three PocketIC round-trip and upgrade tests; budget 2.5T cycles with 0.55T reserve; deployer steps held | local-integration |
| BB-24 Base Sepolia phase 1: ICP side (packet steps A to E) | held (installation held after a stalled executor request; reconcile before retry) | supervisor (Miles) | none (public executor) | BB-23 | Created: two test canisters running, single controller verified. Ledger install request stalled at the executor with no result; both module hashes read back none, so no code. Held: reconcile before retry; bridge install not attempted | public-created |
| BB-25 Base Sepolia phase 2: Base side (packet steps F to J) | blocked (deployer signer held) | supervisor (Miles) | none (public executor) | BB-24 | Contract deploy, funding, bind, round trip and close. HELD: creating a test-only Base Sepolia deployer signer was refused by an approval review; waiting on Rob's explicit test-key answer | none |

## Regression evidence (backend untouched)

- `git diff origin/main -- src/rumi_protocol_backend foundry/src/IcUSD.sol`: empty.
- `cargo test --offline -p rumi_protocol_backend --lib chains::`: 659 passed (private scratch target).
- `cargo test --offline -p rumi_protocol_backend --test multi_chain_supply_invariant`: 2 passed.
- `forge test --offline`: the 14 existing IcUSD tests pass unchanged.

## Held (needs separate authorization; not started)

- Base Sepolia test: now authorized in scope (synthetic test ledger, dedicated bridge canister, chain 84532 only), executed only by Miles after he validates the deployment packet (BB-23, BB-24 ICP side, BB-25 Base side). The production icUSD ledger, existing Rumi canisters, Base mainnet and merge stay held.
- Base Sepolia deployer signer provisioning: HELD. An approval review refused creating a test-only deployer key (2026-10-24); no key exists. Do not create, import or relay one by any route until Rob's explicit answer arrives.
- Anything outside that test: production deploys (existing Rumi canisters, the production icUSD ledger, Base mainnet), production controller, role or governance changes, live signing or broadcast beyond the packet's Base Sepolia steps, real funds, and merging PR #591.
- Worktree removal pass (`scripts/cleanup_worktrees.sh --remove-eligible`): denied by the permission classifier on 2026-10-24; dry run showed 0 eligible anyway.

Resolved: the "first operational Lookout upload" hold (2026-10-24 approval-review rejection) was cleared by Rob's approval of sanitized board metadata uploads, and the first snapshot has been published and verified.

## Change log

- 2026-10-24: board created; recon cards dispatched.
- 2026-10-24: recon done; design and plan committed (`7ceb605e`); contract committed (`225fece7`); harness done; BB-04/05/06/07/10 dispatched.
- 2026-10-24: canister committed (`a02b4292`, 339 unit tests); BB-09b part 1 dispatched.
- 2026-10-24: public history queries (`19fbb0b5`), frontend (`ac7a517a`); first PocketIC + anvil round trip passed; BB-09b part 2 and reviews A/B (round 1) running.
- 2026-10-24: round 1 fixed (`6208c117`), C4 (`e4213be1`), draft PR #591; round 2 A and B both FAIL, reviewer B's blockers fixed (`779bd37f`); reviewer A's gas blocker and notes in BB-16 (uncommitted); review record added.
- 2026-10-24: round-2 follow-ups committed as frozen candidate `eab65670` (Wasm sha256 `de4ad399550504d7ab06b5d7299cce4f20d9ea81e34ea75a69d743a3fb2eacf6`); all four PocketIC suites ran on that exact Wasm; round 3 (fresh A and B) started on it.
- 2026-10-24: round 3 on `eab65670` (A PASS, B FAIL); fixes in `8f2abbfc`; fresh re-check PASS; test and doc follow-ups in `fb005f1b` (Wasm unchanged); PR description updated.
- 2026-10-24: supervisor accepted the local implementation and draft PR milestone at `5fdf2145`; Rob approved sanitized Lookout uploads, so the first-upload hold is resolved; event publishing waits for the setup owner's cutover confirmation.
- 2026-10-24: Lookout cutover confirmed by the setup owner; first snapshot published and verified as a no-op readback; this worktree's `--dry-run` confirms no change; routine sync of changed checkpoints is owned here.
- 2026-10-24: presentation sidecar added (short status plus 89 definitions, Rob's request); projection evidence now ends at a clause or word boundary (BB-06, BB-16, BB-17 shortened to complete text); publishing held for the root owner's presentation cutover.
- 2026-10-24: Rob asked to fix the three follow-ups and prepare Base Sepolia testing; BB-18..BB-24 claimed; Miles is the one public executor.
- 2026-10-24: deployer signer provisioning held after an approval-review refusal; BB-24 blocked on it; fixes, reviews and the packet continue.
- 2026-10-24: preflight done (read-only); reduced cycles budget fits; W5 fixes implemented; W7 recovered from a download stuck at approval.
- 2026-10-24: follow-ups closed in 4169feb8; focused reviews A and B PASS; raw Wasm passes all five suites; install artifact passes three round-trip and upgrade tests; packet ready for Miles; deployer signer still held.
- 2026-10-24: Miles validated the packet; BB-24 split into phase 1 (ICP side, steps A to E, ready) and BB-25 phase 2 (Base side, held on the deployer signer).
- 2026-10-24: BB-24 step B done by Miles (two test canisters created, controller verified); step C ledger install pending at the executor boundary; Base side still held.
- 2026-10-24: BB-24 created only; the ledger install request stalled at the executor without a result (only the local wait was ended), both module hashes read back none; installation held until reconciled; bridge install never attempted; no test tokens minted, no bridge addresses derived.
