# Rumi · Multichain icUSD: canonical Kanban board

This is the single canonical board for Rumi's escrow-backed icUSD work. Cards BB-00 to BB-28 are the
ICP icUSD <-> Base bridge (kept as history and proof). From 2026-10-30, cards BB-29 onward lay
groundwork for nine destinations: Ethereum, Base, Arbitrum, Robinhood Chain, HyperEVM, Monad,
Solana, Sui and XRPL. The file name and Lookout project id stay the same; the Lookout display title
is "Rumi · Multichain icUSD".
`base-icusd-bridge-lookout.json` is derived from this file and must stay aligned with it.
`base-icusd-bridge-presentation.json` holds the board's short plain-English status and term
definitions; update it with every material board change.

- Worktree branch: `codex/base-icusd-bridge`
- Baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90` (fetched 2026-10-24)
- Multichain baseline (fetched 2026-10-30): branch head `c82f2880`; `origin/main` at `39da7d1f`,
  36 commits past the branch base, none touching chain, bridge, Foundry or Base-bridge frontend code.
- Multichain workers: Claude Sonnet and Haiku subagents only (no Codex agents), at most two Sonnet
  lanes plus one Haiku task at once. Actual models are read from each run's own record, not assumed.
- Coordinator: Claude (Opus 5.5, main thread). Supervisor: Miles (Codex), for Rob.
- Boundaries: no merge, no deploy, no live signing or broadcast, no funds movement, no controller/role changes.
- Delivery: local implementation complete; draft PR RumiLabsXYZ/rumi-protocol-v2#591 open, not merged, not deployed. Miles (supervisor) accepted the local implementation and reviewable draft PR milestone at `5fdf2145`, relying on the recorded local test and review evidence. That acceptance is not production safety, live readiness, or approval to merge, deploy, sign, fund or change roles.
- Lookout: Rob approved uploading this board's sanitized metadata to his private Lookout backend, and future sanitized updates (2026-10-24, verified by Miles; see `base-icusd-bridge-operating-rules.md`). The first snapshot was published after the setup owner's cutover and verified as a no-op readback; this worktree's coordinator now publishes each changed sanitized checkpoint (see the event rule there).

## Priorities (authoritative order)

Ranked sub-projects for this board. `base-icusd-bridge-priorities.json` holds the same list (array
order is the rank) and is the machine-readable source; this table must match it exactly (checked
offline at each checkpoint, see the operating rules). Miles (supervisor) decides ranking and execution; the coordinator maintains
this list and the JSON. Selection rule: `base-icusd-bridge-operating-rules.md`, "Priority selection".

| Rank | Item | Status | Cards | Blocker and what clears it | Next action |
| --- | --- | --- | --- | --- | --- |
| 1 | `browser-round-trip`: Finish real-wallet browser acceptance | blocked | BB-26, BB-27 | The renewed staging identity sign-in awaits Rob's Touch ID, and Rabby unlock and connection are still pending. Cleared when Rob completes sign-in, unlocks Rabby and approves the staging connection. | After sign-in and wallet approval: connect the Base wallet, then test transaction consent and rejection, the synthetic browser round trip and reload recovery. Earlier real Internet Identity sign-in remains scoped proof. |
| 2 | `multichain-recon`: Research what each of the nine chains needs | in_progress | BB-30, BB-31, BB-32, BB-41 | - | BB-30 (repository reuse) and BB-31 (six EVM chains) are done. Still running: BB-32 (Solana, Sui and XRPL facts, Sonnet) and BB-41 (what the open XRPL icUSD PR defines, Haiku). Next: start the shared escrow contract when BB-32 returns. |
| 3 | `shared-escrow-contract`: Shared escrow accounting and destination-profile contract | blocked | BB-33 | Queued: starts when the three research cards (BB-30, BB-31, BB-32) return and are reviewed. | Write the canonical escrow design for all nine chains: locked ICP icUSD backs every representation with no new debt, kept apart from debt-backed chain supply, eight-decimal value, chain and network separated ids, nonces and escrows, unknown outcomes held until reconciled. |
| 4 | `evm-expansion`: Reuse the Base design for Ethereum, Arbitrum, Robinhood Chain, HyperEVM and Monad | blocked | BB-34 | Queued: needs the shared escrow contract (BB-33) and the EVM facts (BB-31). | Name the config-only and code-change seams for each EVM chain and the smallest generalization of the Base bridge, without changing Base behavior. Keep HyperEVM (historical state) and Monad (Verified execution state) as disabled activation blockers. |
| 5 | `solana-groundwork`: Solana groundwork | blocked | BB-35 | Queued: needs the shared escrow contract (BB-33) and the native-chain facts (BB-32). | Choose the token standard and authority model, the ICP signing and RPC path and the finality wait, then write implementation-ready Solana next steps. |
| 6 | `xrpl-groundwork`: XRPL groundwork | blocked | BB-37 | Queued: needs the shared escrow contract (BB-33), the native-chain facts (BB-32) and the open XRPL icUSD PR inventory (BB-41). | Choose between issued currency and Multi-Purpose Token, the issuer and holder opt-in model, the ICP signing and RPC path and the finality wait, then write implementation-ready XRPL next steps. |
| 7 | `sui-groundwork`: Sui groundwork | blocked | BB-36 | Queued: needs the shared escrow contract (BB-33) and the native-chain facts (BB-32). | Choose the coin standard and treasury and upgrade custody, the ICP signing and RPC path and the finality wait, then write implementation-ready Sui next steps. |
| 8 | `destination-profiles`: Disabled destination profiles for all nine chains and a validator | blocked | BB-38 | Queued: needs the profile schema from BB-33 and the facts from BB-31 and BB-32. | Check in nine disabled-by-default profiles with a source for each fact, plus a focused validator and dry run that reject activation, missing sources and decimal mismatches. |
| 9 | `frontend-chain-wallets`: Frontend chain selection and wallet requirements | blocked | BB-39 | Queued: needs the wallet facts from BB-31 and BB-32. | List each chain's wallet connection standard and the page changes needed to choose a destination. Read-only; no interface build yet. |
| 10 | `integrated-plan`: Integrated implementation plan and local smoke | blocked | BB-40 | Queued: needs the EVM, native-chain, profile and frontend cards (BB-34 to BB-39). | Write the sequential implementation plan from the actual missing seams with owners and focused commands, run the validator and existing focused checks once, and update the draft PR without merging. |
| 11 | `acceptance-handoff`: Close browser evidence and PR handoff | blocked | BB-14, BB-22, BB-27 | Waits until the browser acceptance (BB-27) settles and fresh reconciliation and review evidence is recorded. There is no merge authority in this work. | Once BB-27 settles: record fresh reconciliation and review evidence, then update the PR description and the handoff. Do not merge. |
| 12 | `priorities-and-handoff`: Keep priorities and test handoff current | done | BB-28, BB-29 | - | None for this checkpoint. Ongoing upkeep follows the priority selection rule at each later checkpoint. |
| 13 | `production-launch`: Production launch | paused | none | Paused until Rob separately authorizes a production launch and a reviewed launch scope exists. This is held future scope and never a gate on the authorized test. | - |
| 14 | `implementation-local-verification`: Implementation and local verification | done | BB-R1, BB-R2, BB-R3, BB-R4, BB-01, BB-02, BB-03, BB-04, BB-05, BB-06, BB-07, BB-08, BB-09a, BB-09b, BB-10, BB-12, BB-13, BB-05b, BB-15, BB-16, BB-17, BB-18, BB-19, BB-20, BB-22 | - | - |
| 15 | `cli-public-test`: Public synthetic CLI round trip | done | BB-11, BB-21, BB-23, BB-24, BB-25 | - | - |

Item 1 (the Base browser test) stays first because it is the next demo step, but it waits on Rob's
sign-in and wallet. Work proceeds on the highest unblocked item, the nine-chain research (item 2).
Items 3 to 10 are queued behind their dependencies, item 11 waits on item 1, and item 13 on a
separate production authorization.

Card membership: the Cards column (each item's `cardIds`) decides which cards Lookout badges with
that item's rank. Links are set from each card's scope, files, dependencies and evidence, never its
title alone (operating rules, "Priority selection", clause 7). 47 of the 48 cards are linked.
BB-00 (baseline and inventory) stays unlinked because it serves every item. BB-22 and BB-27 each
belong to two items: BB-22 finishes the implementation reviews and is review evidence for the PR
handoff, and BB-27 is the browser test that item 11 also waits on.

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
| `staging-fixture` | Isolated staging UI only: the page rendered against the test canisters and wallet adapters were exercised with a read-only fixture. Not a real-wallet approval, transaction rejection, browser transfer or reload-recovery proof. |
| `staging-auth` | Isolated staging UI: a real wallet sign-in completed and the page shows the signed-in principal. Sign-in only: no transaction consent or rejection, browser transfer or reload-recovery proof. |

The `public-*` levels apply only to the isolated Base Sepolia run in the deployment packet (synthetic
test assets, chain 84532), each at the exact step it names. No card may claim a production or
mainnet-funds proof level. Exact targets, transactions and funding stay in the executor's private
run record, never on this board.

## Card states

`queued` (Lookout's Queue column): scoped and authorized, waiting on the cards it depends on; not
running and not done. `ready`: dependencies met, can be dispatched. `in progress`: an owner is
working. `review`: result delivered, awaiting the supervisor. `blocked` and `done` as before. The
derive script maps these to Lookout columns Queue, Ready, In progress, Review, Blocked and Done.

## Shared build coordination

- One cargo target dir per worktree (this worktree's `target/`). Do not share `CARGO_TARGET_DIR` across sessions.
- Workers edit only their owned files and run focused `cargo test --offline -p rumi_icusd_bridge --lib <module>`; a compile failure in a file someone else owns means wait and retry, never edit it.
- The coordinator owns shared interfaces: `types.rs`, `state.rs`, `accounting.rs`, `ids.rs`, `flows/*`, `timer.rs`, `lib.rs`, `admin.rs`, `views.rs`, `icrc21.rs`, the `.did`.
- Disk is tight (about 47 GiB free, 95% used, on 2026-10-30). Workers must not create extra worktrees or target dirs.
- Multichain lanes write only their own new files under `docs/coordination/multichain/` (or their
  card's named path); the coordinator owns shared interfaces, integration and the one build.

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
| BB-24 Base Sepolia phase 1: ICP side (packet steps A to E) | done | supervisor (Miles) | none (public executor) | BB-23 | Installed: both isolated test modules match the packet hashes; controller checks passed; test RPC chain-id quorum verified. Runtime frozen at 4169feb8 | public-installed |
| BB-25 Base Sepolia phase 2: Base side (packet steps F to J) | done | supervisor (Miles) | none (public executor) | BB-24 | CLI synthetic test: deposit Delivered; return burn finalized; release Released; readbacks match packet vectors; fresh audit coverage, supply bound, exact match true; zero pending; remaining Base tokens fully backed; bridge stopped | public-settled |
| BB-26 Isolated test UI: staging recon and recipe | done | coordinator (Opus 5.5 / Opus 5.5) + worker W8 (Sonnet / Sonnet) + worker W9 (Haiku / Haiku) | read-only source; tracked recipe doc | BB-25 | Recipe written from read-only source checks: ids alone do not isolate the app, so a throwaway copy gets a bridge-only shell, test-only ids, no polling, no production II origin and a request guard. Not compiled or run | source |
| BB-27 Isolated test UI: browser acceptance | blocked (Touch ID and Rabby unlock/approval) | supervisor (Miles), with Rob for real-wallet gestures | none (live and browser execution) | BB-26 | Earlier real staging sign-in proven. Renewed sign-in awaits Touch ID; Rabby unlock/connection pending. Test-network read-only checks pass; no browser transfer, seed or funding; bridge stopped. Rob completes sign-in and wallet approval next | staging-auth |
| BB-28 Priorities and supervisor operating rules | done | coordinator (Opus 5.5 / Opus 5.5) | docs/coordination priorities, board, rules, handoff | BB-27 | Priority list, board priority section and selection rule pushed (ae72444b); validated offline with the publisher's own validator run from outside the repo; published at Alden's priority cutover, readback unchanged | source |
| BB-29 Multichain board: rename, Queue column and ranked priorities | done | coordinator (Opus 5.5 / Opus 5.5) | docs/coordination board, priorities, presentation, rules, handoff; derive script state map | BB-28 | Board widened to nine chains; queued cards map to a real Lookout Queue state, checked offline with the publisher and the frontend column helper; priorities re-ranked with explicit cards; Miles verified the live board and accepted it | source |
| BB-30 Cross-chain reuse inventory for all nine chains | done | worker W10 (Haiku / Haiku 5.5) | `docs/coordination/multichain/reuse-inventory.md` only | BB-00 | Source inventory with file references (reuse, change, missing per module and chain); coordinator spot-checked four claims; supply row corrected from the function bodies (Miles); shared-canister collision note added | source |
| BB-31 EVM official-source facts: Ethereum, Base, Arbitrum, Robinhood Chain, HyperEVM, Monad | done | worker W11 (Sonnet / Sonnet 5.5) | `docs/coordination/multichain/sources-evm.md` only | BB-00 | Primary-source facts for all six with links; each needs a canister code change; HyperEVM needs archive providers or an observer redesign, Monad must read Verified state; disabled blockers recorded; Robinhood ids rechecked | source |
| BB-32 Native official-source facts: Solana, Sui, XRPL | in progress | worker W12 (Sonnet / Sonnet 5.5) | `docs/coordination/multichain/sources-native.md` only | BB-00 | Token standard, eight-decimal fit, ICP signature scheme, finality, RPC path, wallets and prerequisites from primary sources with links and the date checked | none |
| BB-33 Shared escrow accounting and destination-profile contract | queued | worker (Sonnet / not started) | multichain design spec (shared sections); profile schema | BB-30, BB-31, BB-32 | Escrow for all nine: backed by locked ICP icUSD, no new debt, apart from debt-backed chain supply; eight-decimal value; chain and network separated ids, nonces and escrows; unknown outcomes held until reconciled | none |
| BB-34 Six-EVM readiness and Base-bridge reuse | queued | worker (Sonnet / not started) | `docs/coordination/multichain/chain-evm.md` | BB-31, BB-33 | For each EVM chain: config only or code change against the Base design, the smallest generalization seam and next steps, with the HyperEVM and Monad read-evidence blockers kept; Base behavior unchanged | none |
| BB-35 Solana groundwork | queued | worker (Sonnet / not started) | `docs/coordination/multichain/chain-solana.md` | BB-32, BB-33 | Token standard and authority model, ICP signing and RPC path, finality wait, prerequisites, trust limits stated plainly and implementation-ready next steps | none |
| BB-36 Sui groundwork | queued | worker (Sonnet / not started) | `docs/coordination/multichain/chain-sui.md` | BB-32, BB-33 | Coin standard, treasury and upgrade custody, ICP signing and RPC path, finality wait, prerequisites, trust limits and implementation-ready next steps | none |
| BB-37 XRPL groundwork | queued | worker (Sonnet / not started) | `docs/coordination/multichain/chain-xrpl.md` | BB-32, BB-33, BB-41 | Issued currency or Multi-Purpose Token, issuer and holder opt-in, ICP signing and RPC path, finality wait, reserves, trust limits and implementation-ready next steps | none |
| BB-38 Disabled destination profiles and validator | queued | worker (Sonnet / not started) | profile files for the nine chains; focused validator | BB-31, BB-32, BB-33 | Nine checked-in profiles, disabled by default, each fact with its source and status; a focused validator and dry run that fail on activation, missing sources or decimal mismatch | none |
| BB-39 Frontend chain selection and wallet requirements | queued | worker (Sonnet / not started) | `docs/coordination/multichain/frontend-wallets.md` | BB-31, BB-32 | Each chain's wallet connection standard and the page changes needed to choose a destination; read-only, no interface build | none |
| BB-40 Integrated implementation plan, local smoke and PR update | queued | coordinator (Opus 5.5 / not started) | implementation plan; handoff; draft PR description | BB-34 to BB-39 | Sequential plan from the actual missing seams with owners and focused commands; validator and existing focused checks run once; draft PR updated, no merge | none |
| BB-41 Existing XRPL icUSD PR #586: read-only inventory | in progress | worker W13 (Haiku / Haiku 5.5) | `docs/coordination/multichain/xrpl-pr586-inventory.md` only | BB-30 | What token the open PR defines on XRPL, whether debt- or escrow-backed, its issuer and signing model, and any collision with an escrow-backed bridge token; PR left untouched | none |

## Regression evidence (backend untouched)

- `git diff origin/main -- src/rumi_protocol_backend foundry/src/IcUSD.sol`: empty.
- `cargo test --offline -p rumi_protocol_backend --lib chains::`: 659 passed (private scratch target).
- `cargo test --offline -p rumi_protocol_backend --test multi_chain_supply_invariant`: 2 passed.
- `forge test --offline`: the 14 existing IcUSD tests pass unchanged.

## Held (needs separate authorization; not started)

- Base Sepolia test: now authorized in scope (synthetic test ledger, dedicated bridge canister, chain 84532 only), executed only by Miles after he validates the deployment packet (BB-23, BB-24 ICP side, BB-25 Base side). The production icUSD ledger, existing Rumi canisters, Base mainnet and merge stay held.
- Base Sepolia deployer signer: resolved 2026-10-25. After an approval review refused it on 2026-10-24, Rob directly instructed creating the encrypted test key and protected password file; Miles created it and is its only user. Nobody else reads, copies or exports it.
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
- 2026-10-25: Rob's direct instruction resolves the deployer signer hold (Miles created the encrypted test key); stalled ledger install reconciled (no module), the retry got a definite no-effect rejection for too few cycles; packet Amendment 1 tops up the ledger from the run's reserve before the retry.
- 2026-10-25: BB-24 done (both test modules installed and match the packet hashes); BB-25 in progress: contract deployed and verified, bind reconciled, deposit mint receipt successful, mint finality pending, return burn not started. Packet Amendment 2 (`dc79764a`) by Miles.
- 2026-10-25: BB-25 done at public-settled: the isolated CLI round trip on Base Sepolia with synthetic tokens passed and the test bridge was stopped (Miles; evidence `docs/coordination/base-icusd-bridge-sepolia-execution.md`, commit `88872433`). Browser-wallet acceptance and production activation are not established; PR #591 stays draft and unmerged.
- 2026-10-25: Rob authorized testing the Bridge page through an isolated test UI; BB-26 (staging recon and recipe) and BB-27 (browser acceptance, Miles) claimed.
- 2026-10-25: BB-26 done: staging recipe `docs/coordination/base-icusd-bridge-staging-recipe.md` (bridge-only throwaway copy; no tracked file changes); BB-27 waits for Miles' review before the single dev server.
- 2026-10-25: BB-27 in progress at staging-fixture: Miles rendered the isolated copy and ran fixture wallet checks; the paused rendering window is closed and the test bridge stopped again; real-wallet gestures (Rob) come next, then the browser round trip and recovery checks.
- 2026-10-25: BB-27 at staging-auth: a real Internet Identity sign-in (existing browser session reused, no new credential or passkey ceremony) returned a non-anonymous principal to the staging page with the guard at zero blocks; no transaction, restart, module, role or funding change.
- 2026-10-26: priority list and selection rule added (BB-28; Rob's request relayed by Miles); BB-27 blocked on the locked Base wallet; this checkpoint is intentionally not published (Alden owns the priority cutover).
- 2026-10-26: BB-28 done (ae72444b pushed and validated offline); no item is ready; still unpublished pending Alden's cutover.
- 2026-10-26: Alden's priority cutover is complete (priority feature live, initial priority publication done; Miles' independent dry-run: board, presentation and priorities unchanged); priorities now follow the normal changed-event publication.
- 2026-10-27: priority card membership reviewed for Lookout's card badges (Rob's request): only `cardIds` changed, linking 34 of 35 cards (11 before; BB-00 unlinked); ranking, statuses and board cards unchanged; not published until Alden confirms the badge feature is live.

- 2026-10-27: Miles verified the renewed browser checkpoint: identity sign-in waits for Touch ID and Rabby connection remains pending; no browser transfer. Both technical and plain-English project statuses now describe that same checkpoint. Alden's deployed-badge confirmation satisfies the old readiness hold.
- 2026-10-30: Rob authorized nine-chain icUSD groundwork (relayed by Miles). Board renamed for Lookout to "Rumi · Multichain icUSD" (same file and project id); Queue column added as a real card state; priorities re-ranked with the Base browser test still first; BB-29 to BB-40 added; research cards BB-30 (Haiku) and BB-31, BB-32 (Sonnet) dispatched read-only. No deploy, signing, funding or merge.
- 2026-10-30: Miles verified the live board (title, Queue with the eight queued cards, three research cards in progress, Base blocker and history kept) and accepted BB-29; BB-29 done; membership note now says item 11 (not item 3) also waits on BB-27; worker record added to the handoff.
- 2026-10-30: BB-30 accepted after source spot-checks; its supply row corrected from the actual function bodies (debt + reserve backing + pending chain burn) and a shared-canister collision note added (Miles). BB-31 accepted; Robinhood chain ids rechecked; HyperEVM historical-state and Monad Verified-state reads recorded as disabled activation blockers (Miles). BB-41 (Haiku) dispatched read-only on the open XRPL icUSD PR.
