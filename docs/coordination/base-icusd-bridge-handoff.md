# Base icUSD bridge: milestone handoff

Compact status for supervisors. The canonical board is `base-icusd-bridge-kanban.md`; this file
is chronological milestone history. Older statements describe their dated checkpoint, not current
authority. Use the current operating rules and the latest milestone for ongoing decisions.

Current retained evidence: the isolated synthetic command-line test moved test tokens to Base and
back. The real browser trip still awaits Rob's sign-in and Base-wallet unlock/connection; no new
browser settlement is claimed. Routine sanitized Lookout updates are authorized through the
existing scoped publisher without per-checkpoint questions. The live mirror was verified current
on October 30 without uploading unchanged data. Earlier missing-visible-authorization holds are
historical, resolved by the October 29 renewal; genuinely new rejections and unrelated holds stand.

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

## Milestone 5 (2026-10-24, about 07:00 PT): frozen round-3 candidate

| Commit | Content |
| --- | --- |
| `e2a1cd7d` | Round-2 review record and milestone 4 checkpoint (docs) |
| `eab65670` | Round-2 follow-ups (BB-16): the frozen candidate under round-3 review |

Release Wasm built from `eab65670`: sha256
`de4ad399550504d7ab06b5d7299cce4f20d9ea81e34ea75a69d743a3fb2eacf6`, 3,849,016 B raw,
3,210,909 B shrunk, exports `canister_inspect_message`. A forced recompile of the bridge crate from
the clean tree at `eab65670` reproduced the same hash, so the suites below ran this candidate.

| Check (on `eab65670`) | Result | Wasm |
| --- | --- | --- |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 428 passed | n/a |
| `cargo clippy --offline -p rumi_icusd_bridge --lib -- -D warnings` | clean | n/a |
| `bridge_round2_pic` (new) | 11 passed (7 new, plus 4 shared support tests) | `de4ad399` |
| `bridge_e2e_pic` | 24 passed | `de4ad399` |
| `bridge_faults_pic` | 16 passed | `de4ad399` |
| `harness_smoke_pic` | 12 passed | `de4ad399` |
| Negative control: the 7 new tests on the pre-round-2 Wasm | all 7 fail | pre-round-2 build |
| vitest (vault_frontend) | 1506 passed, 1 skipped | n/a |
| `npm run check` | 30 errors / 66 warnings, identical to baseline, none in bridge files | n/a |
| `npm run build` | passes | n/a |
| `forge test --offline` | not rerun: contracts and tests unchanged since `e4213be1` (111 passed at `779bd37f`, reviewer A) | n/a |
| Backend suites | not rerun: backend and other canisters unchanged (`git diff` empty) | n/a |

Measured on local anvil through the canister: the worst-case first mint used 152,342 gas of the
300,000 limit, with every preflight `eth_call` carrying that limit.

`cargo clippy --tests` still reports style lints in pre-existing integration-test harness files;
not a gate and not new in this round.

## Milestone 6 (2026-10-24, about 08:00 PT): round 3 resolved, re-check PASS

| Commit | Content |
| --- | --- |
| `2fb60b03` | Frozen round-3 candidate evidence (docs) |
| `8f2abbfc` | Round-3 fixes: runbook `--query`, ingress size caps, timer re-arm order, constant-time admission count, checksummed consent text, gas notice tab |
| `fb005f1b` | Re-check follow-ups, tests and docs only (release Wasm byte-identical to `8f2abbfc`) |

Round 3 on `eab65670`: reviewer A PASS (one minor regression), reviewer B FAIL (2 blockers); all
three accepted and fixed in `8f2abbfc`. A fresh re-check of `8f2abbfc`: PASS, no blockers. Details:
`base-icusd-bridge-review-record.md`.

Release Wasm built from `8f2abbfc` (and unchanged at `fb005f1b`): sha256
`c03ce4f11205438769493ccea1322b0ba21b442963f703acc6b5a2c60ee4d6ff`, 3,852,177 B, reproduced by a
forced recompile from the clean tree; every PocketIC result below used exactly that file.

| Check | Result | Commit |
| --- | --- | --- |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 432 passed | `8f2abbfc` and `fb005f1b` |
| `cargo clippy --offline -p rumi_icusd_bridge --lib -- -D warnings` | clean | both |
| `bridge_round2_pic` | 12 passed | `8f2abbfc` |
| `bridge_e2e_pic` | 24 passed: 20 in the concurrent run, the other 4 on a serial rerun | `8f2abbfc` |
| `bridge_faults_pic` | 16 passed: 11 concurrent, 5 on the serial rerun | `8f2abbfc` |
| `harness_smoke_pic` | 12 passed: 11 concurrent, 1 on the serial rerun | `8f2abbfc` |
| `bridge_e2e_pic` tests using the stricter refusal helper | 2 passed | `fb005f1b` |
| `forge test --offline` | 111 passed | `8f2abbfc` |
| vitest (vault_frontend) | 1507 passed, 1 skipped | `8f2abbfc` |
| `npm run check` | 30 errors / 66 warnings, identical to baseline, none in bridge files | `8f2abbfc` |
| `npm run build` | passes | `8f2abbfc` |
| Backend suites | not rerun: backend and other canisters unchanged | n/a |

Serial rerun: with all four PocketIC suites running at once, 10 tests failed inside pocket-ic
before any bridge call (the PocketIC server's HTTP adapter could not load macOS root certificates,
"I/O error -36", and `POST /instances` failed). The same 10 tests passed when rerun one suite at a
time on the same Wasm from a clean tree.

## Next action

1. Rob or Miles: review draft PR #591. Merge, deploy and every live action stay held.
2. Before any public-testnet run: the open follow-ups in the review record, a fee and minimum that
   cover per-deposit cost, and the held runbook steps in `foundry/DEPLOY-BASE.md`.

## Unresolved risks (as of this milestone)

- Local evidence only: canister-signed transactions executed on a local anvil; nothing on Base Sepolia or Base.
- pocket-ic 6.0.0 `await_call` panics after 100 rounds; the harness works around it.
- Lane throughput is one unincluded tx at a time per lane (acceptable for v1, documented).
- `rumi_analytics` still reports ICP ledger supply as circulating; subtracting escrow is a follow-up.

## Milestone 7 (2026-10-24): local delivery accepted; Lookout approval

- Miles (supervisor) verified a clean worktree, PR #591 draft and unmerged at `5fdf2145`, the
  bounded `eab65670` to `8f2abbfc` closure diff and the test and docs-only follow-ups, an unchanged
  backend and `IcUSD.sol`, a clean `git diff --check`, and independently hashed the release Wasm
  (`c03ce4f1...4d6ff`, matching this handoff). He accepted the local implementation and reviewable
  draft PR milestone, relying on the recorded local evidence without rerunning suites. This is
  not production safety, live readiness, or merge, deploy, signing, funding or role approval.
- Rob approved uploading this board's sanitized snapshot and future sanitized updates to his
  private Lookout backend (verified by Miles). The first-upload hold is resolved. After the setup
  owner's cutover the snapshot was published and its readback verified as a no-op; this
  worktree's coordinator owns syncing each changed sanitized checkpoint from here on. Rules and
  limits: `base-icusd-bridge-operating-rules.md`.

## Milestone 8 (2026-10-24, about 11:30 PT): follow-ups closed, Base Sepolia packet ready

Rob asked (relayed by Miles) to fix the three PR follow-ups and get ready for Base Sepolia
testing; Miles is the one public executor.

| Commit | Content |
| --- | --- |
| `4169feb8` | The three follow-ups: clear-halt success coverage, refund and sweep re-arm, at most 5 RPC providers |
| `2f0e25fa` and later | Deployment packet `base-icusd-bridge-sepolia-packet.md`, runbook pointer, board, review record |

| Check | Result | File |
| --- | --- | --- |
| `cargo test --offline -p rumi_icusd_bridge --lib` | 445 passed | n/a |
| clippy `--lib -D warnings`, `git diff --check` | clean | n/a |
| Five PocketIC suites, one after another | follow-ups 7, round 2 12, smoke 12, faults 16, e2e 24 (71 passed, no infrastructure failure) | raw `a0049b9a` |
| Three e2e round-trip and upgrade tests via `BRIDGE_WASM` | 3 passed | install artifact `0b324387` |
| Focused review A (source, invariants) and B (tests, evidence, fit) | PASS, PASS | `4169feb8` |
| Read-only preflight | CLI, subnet, test key, EVM RPC costs, providers, deploy dry run checked | public read-only |

Budget: 600B ledger plus 1.9T bridge plus fees, 2,500,200,000,000 of the 3,050,950,044,313 available,
leaving about 0.55T. HELD: the test-only Base Sepolia deployer signer (approval-review refusal,
Rob's answer pending), which steps F and I.2 need. Pre-merge follow-ups: a PocketIC test of the
`post_upgrade` refusal, and a zero-provider-call test of the provider cap on replacement.

## Milestone 9 (2026-10-24): partial public setup, installation held

Executed by Miles (the one public executor), packet steps A and B only:

- Proven (`public-created`): two new test canisters exist on the packet's test subnet, both running,
  with the single intended controller verified, funded as budgeted with the reserve kept.
- Not proven: no code is installed (both module hashes read back none), so no synthetic test
  tokens were minted, no bridge configuration exists and no Base addresses were derived.
- Ledger install (step C): the executor's install request never returned a session or a result.
  Miles ended only his local wait, which is not a network cancellation. Installation is HELD:
  reconcile the request against the canister's authoritative status before any fresh install, and
  never retry it through another executor.
- Bridge install (step D) was never attempted. The Base side (steps F to J) stays held on the
  test deployer signer, pending Rob's explicit answer.
- Exact canister ids, cycles figures and the reconciliation rule are in the executor's private run
  record, not in this repository.

## Milestone 10 (2026-10-25): signer resolved, budget amended

- Rob's direct instruction (relayed and verified by Miles): "Yes, create the encrypted Base Sepolia
  test key and protected local password file. Reconcile the stalled install, then finish
  deploying and testing." This resolves the 2026-10-24 signer refusal. Miles created the encrypted
  test-only keystore with a protected password file and is its only user; nobody else reads,
  prints, copies or exports either. Its public address is in the executor's private run record.
- Reconciled: fresh reads after the stalled request showed both canisters running, the same
  single controller, and no module installed.
- Retried (same command line): a definite no-effect rejection, `IC0207`, the ledger needs
  229,352,977,276 more cycles to execute the install. Nothing installed.
- Packet Amendment 1: top up the test ledger by 250B from the run's reserve (30-day freezing
  threshold kept), then retry after a fresh no-module read. Run total 2,750,300,000,000 (was
  2,500,200,000,000); reserve about 300.65B (was about 550.75B). The 0.5T reserve was a planning
  choice; Rob's instruction authorizes completing the test.
- Miles owns funding, installs and testing. Source stays frozen at `4169feb8`.

## Milestone 11 (2026-10-25): ICP side installed, Base deposit receipt

Reported by Miles (the one public executor) as sanitized phase and proof facts; runtime frozen at
`4169feb8`; packet Amendment 2 at `dc79764a`.

| Proof | Status |
| --- | --- |
| `public-installed` (BB-24) | Both isolated test modules installed; module hashes match the packet; controller checks passed; test RPC chain-id quorum verified |
| Contract deployment (BB-25) | The reviewed `BridgedIcUSD` deployed on Base Sepolia; runtime and configuration verified; deployment finality verified |
| Bind (BB-25) | The original bind call was reconciled as successful; it was not repeated |
| `public-receipt` (BB-25, deposit) | The first deposit's mint transaction has a successful receipt |
| Not yet proven | Mint finality and deposit settlement (`Delivered`); the return burn has not started, so no release exists |

Not claimed: settlement of any operation, a completed round trip, or production readiness.
Operational balances, reserves, addresses and logs stay in the executor's private record.

## Milestone 12 (2026-10-25): Base Sepolia CLI round trip settled

Reported by Miles (the one public executor) as sanitized phase and proof facts. Runtime frozen at
`4169feb8`. Execution evidence: `docs/coordination/base-icusd-bridge-sepolia-execution.md` and the
corrected `docs/coordination/base-icusd-bridge-sepolia-packet.md` (both by Miles, commit
`88872433`).

| Proof | Status |
| --- | --- |
| `public-settled`, ICP to Base | The original deposit reached `Delivered` |
| `public-settled`, Base to ICP | Its return burn finalized and the release reached `Released` |
| Independent readbacks | Ledger and Base readbacks match the packet's predefined vectors |
| Fresh audit | Coverage, supply bound and exact match all true |
| Open operations | Zero pending or held operations |
| Remaining liability | The Base tokens still outstanding remain fully backed by matched escrow; this is a live, backed obligation, not a cleared one |
| Window closed | All four pauses verified, then the test bridge stopped; reviewed module and sole controller unchanged |

What this proves: the bridge works end to end on Base Sepolia through the command line with
synthetic test assets (CLI and public-testnet proof).

What it does not prove: browser-wallet acceptance (Internet Identity, Plug, Oisy, Base wallets),
production activation, Base mainnet, or anything about the production icUSD ledger. PR #591 stays
draft and unmerged.

## Milestone 13 (2026-10-25): isolated test UI recipe (BB-26)

Rob authorized testing the Bridge page through an isolated test UI (relayed by Miles). Recipe:
`base-icusd-bridge-staging-recipe.md`, written from read-only source inspection (W8 Sonnet, W9
Haiku, checked by the coordinator). Nothing was compiled, run or deployed.

- Verdict: editing only ids in a copy is not enough, because the root layout and the wallet store
  query production canisters on every route and connect, the wallet panel can send real ledger
  transfers, and Internet Identity would use the production derivation origin. A small isolation
  patch confined to a throwaway copy is enough: bridge-only shell, test-only ids, no balance
  polling, no production II origin, and a request guard. No tracked file changes; the production
  bridge id stays empty.
- Hazard to avoid: never serve the copy on port 4943 (the production alternative-origins file
  lists it, which would give the production II principal).
- Open unknowns: Oisy's handling of the bridge's production-only trusted origins on a localhost
  origin, Plug's per-call prompts, II session length.
- Next: Miles reviews the recipe, then creates the copy, starts the single dev server and runs
  the section 5 acceptance checks before any wallet action (BB-27).

## Milestone 14 (2026-10-25): staging UI rendered; fixture wallet checks (BB-27 in progress)

Reported by Miles (sanitized). He reviewed the recipe and ran the single isolated copy, outside
the worktree, reusing installed dependencies, with explicit dependency and declaration paths, no
environment plugin, loopback-only serving and the two-target request guard. The bridge route,
components and services are byte-identical to the working branch; production source is
unchanged; runtime candidate unchanged at `4169feb8`.

| Check | Result | Proof level |
| --- | --- | --- |
| Rendering at desktop and mobile sizes | correct test network and pause state, both direction tabs, keyboard navigation, no horizontal overflow, no framework overlay, no runtime errors | `staging-fixture` |
| Request guard negative control | a non-test request was blocked before it left the browser | `staging-fixture` |
| Wallet adapter (read-only EIP-6963 fixture) | connect rejection, switch rejection, successful test-network switch, account-change rendering | `staging-fixture` |
| Real-wallet approvals, transaction rejection, browser transfer, reload recovery | not done | none |

The short paused rendering window is closed and the test bridge is verified stopped again.

Next: Rob performs the real browser-wallet gestures; then the synthetic browser round trip and
the recovery checks (recipe section 7), with Miles as the executor.

## Milestone 15 (2026-10-25): real Internet Identity sign-in on the staging page (BB-27)

Reported by Miles (sanitized). A failed dynamic wallet-module download showed the earlier server
had stopped; he restored the single isolated loopback server and verified the wallet module now
serves. Current official Internet Identity docs support local development.

| Check | Result | Proof level |
| --- | --- | --- |
| Internet Identity sign-in (real Brave browser) | the official II popup opened and returned to the staging page, which shows a non-anonymous ICP principal and the disconnect control; the request guard stayed at zero blocked requests | `staging-auth` |
| Scope of that sign-in | an existing II browser session was reused; no credential was entered and no new passkey ceremony was exercised | n/a |
| Earlier rendering and fixture wallet checks (milestone 14) | unchanged | `staging-fixture` |
| Real Base-wallet connection, transaction consent and rejection, browser round trip, reload recovery | not done | none |

No test transaction, canister restart, module, controller or role change, or funding happened in
this pass. Runtime and source unchanged (`4169feb8`).

Rule recorded (recipe section 5): verify the local server is reachable at every handoff and
before every wallet step. A previously rendered tab is not proof it is still running.

## Milestone 16 (2026-10-26): priority list and selection rule (BB-28)

Rob asked for ordered sub-project priorities and supervisor operating rules (relayed and verified
by Miles). Miles decides ranking and execution; the coordinator maintains the list.

- `base-icusd-bridge-priorities.json` (array order is the rank), mirrored in the board's
  "Priorities" section; the selection rule is in the operating rules ("Priority selection").
- Order: 1 browser-round-trip (blocked: Rabby is locked and its unlock timed out; clears when Rob
  unlocks it and approves the staging connection), 2 priorities-and-handoff (this checkpoint),
  3 acceptance-handoff (blocked until BB-27 settles and fresh reconciliation and review evidence
  exists; no merge authority), 4 production-launch (paused on a separate Rob authorization and a
  reviewed launch scope), 5 implementation-local-verification (done), 6 cli-public-test (done).
- After item 2 no item is ready; the actual blockers are the locked Base wallet and a production
  authorization that is deliberately out of scope.
- BB-27 status (Miles, sanitized): no Base account connected, no browser transfer, synthetic seed
  or test-ETH funding sent; the test bridge is stopped again; two read-only providers agree on the
  reviewed test contract, its runtime and its pre-test counters. The real Internet Identity
  sign-in (milestone 15) stands.
- Not published: Alden owns the initial priority cutover and publication and has not confirmed
  readiness, so this checkpoint (including its card changes) stays local until then.
- Done: committed and pushed as `ae72444b`, validated offline with the publisher's own
  `validatePriorities` through a temporary validator outside the repository (accepted unchanged; a
  negative control with an unknown card was rejected); item 2 and BB-28 are now done, so no item is
  ready. The derive script was left byte-identical to its pre-task version.

## Milestone 17 (2026-10-26): priority cutover complete

- Alden made the Lookout priority feature live and completed the initial priority cutover and
  publication. Miles independently checked that the scoped manifest's priority path reads this
  worktree's unchanged priority JSON and that a `--dry-run` reports board, presentation and
  priorities all unchanged. Guide: `outputs/PRIORITIES.md` in the Lookout repository.
- From now on the priority list publishes with the cards, summary and glossary through the same
  manifest and scoped identity: dry-run, publish only changed content, then a no-op readback
  (operating rules, event rule and "Priority selection" clause 6).
- Unchanged: the six-item ranking and statuses (no item is ready; rank 1 still waits on Rob
  unlocking Rabby and approving the staging connection), and every bridge, wallet, production,
  signing and merge boundary. Only the initial priority-publishing hold was released.

## Milestone 18 (2026-10-27): priority card membership for Lookout badges

- Request: Rob asked for priority badges on Lookout cards (relayed by Alden; Miles read Rob's
  message himself). A badge comes only from an item's explicit `cardIds` and its array rank, with
  the item title on hover, never from keyword guessing.
- Review: each card's scope, owned files, dependencies and evidence on the canonical board were
  checked against the six items, together with Miles' proposed links. Only `cardIds` changed in
  the priority JSON; every item's id, title, status, blocked reason, next action and rank is
  unchanged, and every earlier link is kept.
- Links: `browser-round-trip` BB-26 (the staging recipe browser acceptance runs from) and BB-27;
  `priorities-and-handoff` BB-28; `acceptance-handoff` BB-14, BB-22, BB-27; `production-launch`
  none; `implementation-local-verification` 25 cards (recon BB-R1 to BB-R4, design BB-01, code
  BB-02 to BB-08, harness and local scenarios BB-09a and BB-09b, frontend BB-10, reviews BB-12 and
  BB-13, fixes BB-05b, BB-15, BB-16, BB-17, follow-ups BB-18 to BB-20 and their focused reviews
  BB-22); `cli-public-test` BB-11 (the Base Sepolia test runbook), BB-21 (read-only preflight),
  BB-23 (deployment packet), BB-24 and BB-25 (execution).
- Coverage: 11 of 35 distinct cards linked before, 34 of 35 after. Unlinked: BB-00 (baseline and
  inventory), which owns the coordination docs and serves every item. Two cards are linked to two
  items: BB-22 (it finishes the implementation reviews and is review evidence for the PR handoff)
  and BB-27 (the browser test, which item 3 also waits on). BB-14 stays only under the PR handoff
  because it is the PR card itself.
- Rule: "Priority selection" clause 7 in the operating rules covers membership upkeep; clause 5's
  offline check now also compares the board table's Cards column with the JSON.
- Not published: Alden has not yet confirmed the badge feature is live and asked that this update
  wait, so there was no publisher run or Lookout API call. Validation was entirely offline (the
  publisher's exported `validatePriorities` against the re-derived projection, card references,
  and table alignment through a temporary validator outside the repository). After Alden confirms,
  normal same-manifest changed-event publication applies.
- Unchanged: board cards, states, owners and proof levels (the derived projection is byte-identical);
  no item is ready; BB-27 stays blocked until Rob unlocks Rabby and approves the staging connection;
  every production, merge, signing and funds boundary.

## Milestone 19 (2026-10-27): two maintained project statuses

- Rob's direct dual-status standard was independently verified by Miles in Alden's chat, turn
  `01a18127-91be-7022-80a5-4088d969b9d7`. The maintained presentation now keeps Technical in
  `summary` and a separate 2–3 sentence `plainSummary`, each at most 600 Unicode codepoints.
  The glossary is unchanged. Both summaries follow the same source revision and proof limits.
- Current BB-27: prior real staging identity sign-in remains verified, but the renewed browser
  session awaits Rob's Touch ID and Rabby unlock/connection. Read-only test-network checks pass;
  no browser transfer, seed or funding occurred. The test bridge is stopped with all pauses.
- The previous badge-readiness hold is satisfied: Miles independently read Alden's final
  deployed-badge confirmation in turn `01a17f5e-a18f-7982-919b-caba43c022f5`. Prepared memberships
  are unchanged. Every real denial and live-action boundary remains; this is no production grant.
- Nora's ongoing coordinator duty is the operating event rule: during active work, promptly
  update canonical cards, both summaries, affected glossary and explicit memberships after task
  movement; same scoped dry-run, changed-only publish, immediate no-op readback. No new timer and
  no promise of automatic updates while inactive. Keep private inputs out of Lookout's public repo.
- Publication evidence stays in the private local publisher record; this is a metadata checkpoint,
  not browser settlement or production readiness.

### Milestone 19 publication disposition (historical; resolved at milestone 21)

- Local checks passed: 35-card projection, both summary limits, unchanged 113-entry glossary,
  unchanged explicit card memberships, and only BB-27 evidence changed. The local projection
  validator now accepts optional `plainSummary`; valid legacy/dual summaries pass, while
  overlong, private-path and non-text plain summaries are rejected. The real publisher's dry-run
  accepted the checkpoint and reported pending board/presentation/plain-summary/priority changes.
- The `--publish` command was then rejected before execution by automatic approval review because
  exact payload/private-destination authorization could not be verified. No upload occurred and
  no post-publication no-op is claimed. Exact input fingerprints are saved privately.
- At that checkpoint, the rejection created an active publication hold and no alternate executor
  or wrapper was used. This receipt is historical: Rob's October 29 renewed standing metadata
  authorization resolved its missing-visible-approval condition, as recorded at milestone 21.
  It is not an ongoing ban on routine sanitized publishing.

## Milestone 20 (2026-10-28): plain-English writing correction

- Miles independently read Rob's direct correction in Alden's chat, turn
  `01a1848c-5a49-7f70-be08-4b66d0d8878c`. The plain status now uses three short sentences for a
  reader outside the project: what works, what still blocks the useful result, and what happens
  next including Rob's action. The stronger 35–60 word standard is in the operating event rule.
- Plain status: "The bridge has moved test tokens to Base and back, but that was done outside
  the app. We still need to prove the same trip works through the browser. Rob needs to finish
  signing in and unlock his wallet; then we can test sending tokens both ways and reopening the
  page."
- The technical status, glossary, cards, priorities and proof limits are unchanged. This is a
  writing correction, not new browser evidence. No runtime or wallet action was attempted.
- At this October 28 checkpoint, the milestone 19 publication rejection was still active and
  no publish retry or alternative executor was used. This historical state was superseded by
  Rob's October 29 renewed standing authorization and successful publication at milestone 21;
  it does not require per-checkpoint approval today.

## Milestone 21 (2026-10-29): renewed private board publishing scope

- Miles independently verified Rob's direct zero-friction stale-page instruction in Alden's
  turn `01a189b2-fe21-7b53-8e88-c18d11712115`, and read the private publishing guide's October 29
  standing scope. Routine sanitized updates to the same private backend via the same canonical
  bridge manifest and scoped identity no longer wait on the historical missing-visible-approval
  note. The operating instructions now preserve that rejection as history, not an active gate.
- The prepared 51-word plain status, Technical status, glossary, cards and priorities retain their
  actual evidence ceiling; no fresh browser or chain observation is claimed. No completion times
  are invented. No product runtime, wallet, governance or financial action is part of this pass.
- Publication uses the original dry-run, changed-only publish and no-op readback. A genuinely new
  review rejection still stops the action; no rerouting, duplicate publisher or additional timer.
  Exact publication results and fingerprints stay in the private local publisher record.
- Completed: original scoped publisher accepted the changed-only upload. Immediate authenticated
  dry-run reports board, Technical/glossary, Plain English, priorities and repository association
  all unchanged. The live mirror contains the prepared 35 cards and 51-word plain status, bound
  to the same source revision. The historical authorization hold is resolved; no fresh rejection.
- Routine metadata upkeep now follows the existing event rule without per-checkpoint permission.
  No additional timer or publisher was created, and no bridge runtime, wallet or financial action
  was attempted. This handoff-only receipt does not trigger another publication.

## Milestone 22 (2026-10-30): bounded stale-hold audit

- Miles independently verified Rob's all-supervisor stale-hold audit request in Alden's turn
  `01a18eb6-9686-7091-9b4e-0bb8d1f20b97`. The operating rules already carry the renewed October 29
  scope and the same scoped manifest/identity. Two historical handoff statements using present
  tense were corrected to avoid reviving a resolved ban; original rejection evidence is preserved.
- No matching bridge publishing/supervision automation prompt was found in the saved automation
  configuration. No timer, service, alternate writer or team was created.
- The authenticated existing publisher dry-run reports the canonical 35-card board, Technical and
  Plain English statuses, glossary, priorities and repository association all unchanged. No new
  unpublished task movement or sync blocker was found; no upload or artificial freshness update
  occurred. Exact revision/source match and input fingerprints are recorded privately.
- The next product step remains the authorized browser demo after Rob's sign-in and wallet
  access. No feature/release work, new checks, wallet action or financial operation was undertaken.

## Milestone 23 (2026-10-30): nine-chain groundwork started

- Rob authorized groundwork for icUSD on Ethereum, Base, Arbitrum, Robinhood Chain, HyperEVM, Monad,
  Solana, Sui and XRPL; Miles relayed the mission. Same coordinator, board, worktree and Lookout
  project; Claude Sonnet and Haiku subagents only.
- Baseline: branch head `c82f2880`; `origin/main` at `39da7d1f`, 36 commits past the branch base,
  none touching chain, bridge, Foundry or Base-bridge frontend code. Disk about 47 GiB free (95%
  used); no worktree cleanup attempted, since the earlier removal pass was denied.
- Worker models, read from each run's own record (the model field on its replies), not from task
  names: a Sonnet probe ran as `claude-sonnet-5-5` and a Haiku probe as `claude-haiku-5-5`; BB-30 runs
  on Haiku 5.5, BB-31 and BB-32 on Sonnet 5.5. This Desktop session's `claude auth status` reports
  signed in (claude.ai, first party), while Miles' standalone CLI reported signed out; the working
  path is this session's own subagents. No Codex agent was used.
- Board: Lookout display title "Rumi · Multichain icUSD" through the same manifest (project id
  unchanged). Queued cards map to a real `Queue` state; the manifest's column order is Queue, Ready,
  In progress, Review, blocked, done. Offline check with the publisher's snapshot code and Lookout's
  frontend column helper: six displayed columns, every card in exactly one, and a control without
  Queue is rejected. Without a configured order the publisher would put Queue after Done.
- Priorities: 15 ranked items. The Base browser test stays first (next demo step, waiting on Rob);
  nine-chain research is the highest unblocked item; the shared escrow contract, EVM reuse, Solana,
  XRPL, Sui, profiles, frontend wallets and the integrated plan are queued in that order. The
  publisher's own validator passes; 46 of 47 cards linked (BB-00 unlinked as the shared baseline).
- Publication: same manifest and scoped identity; the dry run reported board, both summaries and
  priorities changed; the publish succeeded; the immediate dry run reported everything unchanged.
  Exact revisions are in the private publisher record.
- Next: review BB-30 to BB-32 when they return, then one Sonnet owner for the shared contract
  (BB-33), then two Sonnet lanes at a time over BB-34 to BB-39 by rank, then integration (BB-40).
  No actual omission has been recorded in the deferral ledger yet.

## Milestone 24 (2026-10-30): board accepted; worker record

- Miles independently verified the live board (title, Queue with all eight queued cards, the three
  research cards in progress, the Base blocker and history kept) and accepted BB-29 for board and
  interface behavior. BB-29 is done, so the `priorities-and-handoff` item's Done status is consistent.
  The board's membership note now names item 11 (the PR handoff), not item 3, as also waiting on BB-27.
- Worker record. Handles are the coordinator's worker labels; each label maps to one subagent run in
  this Desktop session, which the coordinator keeps for resuming that same run (the session's internal
  run identifiers are not written to this public repository). Actual model is the model field on the
  run's own replies.

  | Handle | Card | Requested | Actual (run record) | Owned path | State |
  | --- | --- | --- | --- | --- | --- |
  | P1 | availability probe | Sonnet | `claude-sonnet-5-5` | none | finished |
  | P2 | availability probe | Haiku | `claude-haiku-5-5` | none | finished |
  | W10 | BB-30 | Haiku | `claude-haiku-5-5` | `docs/coordination/multichain/reuse-inventory.md` | running |
  | W11 | BB-31 | Sonnet | `claude-sonnet-5-5` | `docs/coordination/multichain/sources-evm.md` | running |
  | W12 | BB-32 | Sonnet | `claude-sonnet-5-5` | `docs/coordination/multichain/sources-native.md` | running |

- Concurrency now: two Sonnet lanes (W11, W12) and one Haiku task (W10), the stated maximum. The next
  lane starts only when one of these finishes; no lane is duplicated.

## Milestone 25 (2026-10-30): first research accepted

- BB-30 accepted. The coordinator spot-checked four of its citations against source. Miles found one
  stale row: it described the debt invariant from the old header of `chains/supply.rs`; the function
  bodies (lines 96-178) compare `sum(chain_supplies)` with debt plus reserve backing plus pending chain
  burn. The coordinator corrected the inventory from the bodies (no backend or runtime change) and
  added a note: one shared canister would collide on the release indexes keyed by bare sequence, the
  per-lane nonce, the single chain id, contract binding and supply snapshot; separate per-destination
  instances isolate all of these. The live Base signing domains stay unchanged. The Haiku slot was
  busy with BB-41, so W10 was not reopened.
- BB-31 accepted. The coordinator rechecked Robinhood Chain's chain ids on the official page (4663
  mainnet, 46630 testnet). Two focused constraints from Miles are recorded in the EVM facts file as
  disabled activation blockers: HyperEVM's default RPC serves only `latest` calls, while the observer
  needs numbered finalized historical state (archive-capable independent providers or an observer
  redesign); Monad's `finalized` tag fixes ordering only, so receipts, logs and the supply audit must
  come from a Verified block (finalized minus the execution delay, from a primary source).
- BB-41 dispatched: a read-only inventory of open PR #586 (XRPL icUSD borrow and repay, disabled by
  default), so XRPL groundwork neither duplicates nor collides with it. The PR is not modified.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W10 | BB-30 | Haiku | `claude-haiku-5-5` | finished, accepted |
  | W11 | BB-31 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W12 | BB-32 | Sonnet | `claude-sonnet-5-5` | running |
  | W13 | BB-41 | Haiku | `claude-haiku-5-5` | running |

- Concurrency: one Sonnet lane (W12) and one Haiku task (W13). BB-33 waits for BB-32, so the second
  Sonnet slot is idle rather than starting work ahead of its dependency.

## Milestone 26 (2026-10-30): research done; shared contract started

- BB-32 accepted. The coordinator rechecked Alpenglow being live on Solana mainnet-beta (Anza's page:
  "confirmed = finalized") and that its Sui RPC claim cites Sui's own docs (JSON-RPC off on Foundation
  mainnet nodes; gRPC and GraphQL instead). Its "live query" lines were public read-only reads, no
  transaction. Correction from Miles, recorded in the file: the "Ed25519 only on mainnet" quote is about
  the dfx local replica; PocketIC can provision threshold Schnorr Ed25519 keys and this worktree has
  existing seams (`solana_m2_pic.rs`, `xrp_native_e2e_pic.rs`, best-effort `solana_m2_sign_pic.rs`).
  They are capability evidence, not proof of a passing run; none was rerun.
- BB-41 accepted. The coordinator read PR #586's `xrpl_icusd/issuance.rs` on its head commit: imports
  pull the user's canonical icUSD into escrow with no debt; the borrow path sits under ordinary vault
  debt; neither touches the debt-backed chain supply. The PR was not modified.
- BB-33 (W14) and BB-39 (W15) dispatched. Miles' constraints were forwarded to W14 mid-run: keep the
  local replica, PocketIC capability and execution proof distinct; native token programs alone lack
  consumed-mint and bridge-out semantics, so returns must bind to an ICP account and a durable
  operation id and retried mints must be idempotent (name the smallest guard or reconciliation seam);
  PR #586's import and return path is the default XRPL seam, with its debt distinction, exact decimal
  parser and caps kept, and the Multi-Purpose Token choice recorded as an open decision.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W12 | BB-32 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W13 | BB-41 | Haiku | `claude-haiku-5-5` | finished, accepted |
  | W14 | BB-33 | Sonnet | `claude-sonnet-5-5` | running |
  | W15 | BB-39 | Sonnet | `claude-sonnet-5-5` | running |

- Concurrency: two Sonnet lanes (W14, W15); the Haiku slot is idle because no narrow task is ready.

## Milestone 27 (2026-10-30): frontend wallet requirements accepted

- BB-39 accepted. The coordinator checked two findings in source: `IcpToBaseIntent` has no chain field
  (`src/vault_frontend/src/lib/services/baseBridge/intents.ts` lines 28-42), and the pause or halt gate
  `bridgeBlocked` exists only in `IcpToBaseForm.svelte` (line 67). So saved intents need a destination
  key and a version bump, and a disabled destination must block both directions before any wallet or
  signer call. EVM chains reuse the existing EIP-1193 wallet code with new chain rows; Solana and Sui
  need Wallet Standard libraries (named, not installed); XRPL wallet support comes from PR #586, which
  allows only Payment and TrustSet, so Multi-Purpose Token opt-in would be a gap.
- W15 (BB-39, `claude-sonnet-5-5`) finished. Running: W14 (BB-33, `claude-sonnet-5-5`). Every other
  queued card depends on BB-33, so no lane starts until it returns.

## Milestone 28 (2026-10-30): shared design accepted; profiles and EVM lanes started

- BB-33 accepted (`docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md`). The coordinator read it in full.
  It recommends one bridge instance per destination network with the frontend choosing the actor, so the reviewed Base
  APIs, Candid, stable layout and live signing domains stay unchanged; PR #586's XRPL lane is the XRPL seam (no second
  token); escrow buckets stay apart from the debt invariant (debt plus reserve backing plus pending chain burn); amounts
  are exact on eight chains and bounded at 15 significant digits for XRPL trust-line tokens, never rounded; unknown
  outcomes are reconciled as the same attempt (unsigned bytes plus a single-use sequencing token); Solana and Sui need an
  explicit guard for return binding and idempotent mints (implementation missing); the dfx local replica, PocketIC
  capability and execution proof are kept apart. It defines the profile schema and validator rules V1 to V11 and lists nine
  open decisions (D1 to D9) for Rob and Miles.
- Coordinator edits: the per-family negative decoder test is labeled future implementation; decisions D1, D3, D4 and D5
  gate implementation, not groundwork, so BB-34 to BB-36 proceed on the stated recommendations; BB-39 is marked done.
- Dispatched: BB-38 (W16), the profile crate, validator and nine disabled profiles, the mission's only build lane; BB-34
  (W17), the read-only EVM-family steps.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W14 | BB-33 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W16 | BB-38 | Sonnet | `claude-sonnet-5-5` | running (build lane) |
  | W17 | BB-34 | Sonnet | `claude-sonnet-5-5` | running |

- Concurrency: two Sonnet lanes; the Haiku slot is idle because no narrow task is ready.

## Milestone 29 (2026-10-30): accounting correction to the shared design

- Miles accepted BB-33's direction (per-instance actors, unchanged Base domains, PR #586 as the XRPL seam, a
  default-disabled validator) and found one accounting error: the Solana return account was added to C2 without removing
  the same units from `PR_d`, so a valid return awaiting its burn was counted twice.
- The coordinator corrected the design (sections 2.2 to 2.4 and the Solana row of 6.3) instead of reopening W14, because
  both Sonnet slots were busy and nothing downstream had used the equations yet: new disjoint buckets `RC_d`
  (ReturnCustodied) and `BU_d` (BurnUnknown); `PR_d` is payable only after the tokens are destroyed; no ICP payout while a
  unit is custodied or its burn is unknown; C2 and the derived bound count custodied units once; aggregate reporting uses
  explicit index sets (bridge destinations `D`, the XRPL lane `X` outside `D`) and subtracts the measured custody balance
  once; `E_d + U_d` is described as coverage adjusted for unknown outflows, not proof of a physical lock; the Solana
  observer must check the exact allowed instruction, mint, owner or authority and memo, never a net balance delta alone.
- BB-35's acceptance now names those checks. W16 (BB-38) was asked to add a `solana-return-custody` blocker to the Solana
  profile. No runtime code changed and no test was added or run for this correction.

## Milestone 30 (2026-10-30): EVM steps accepted; Solana started

- BB-34 accepted (`docs/coordination/multichain/chain-evm.md`). The coordinator spot-checked `validate_init` (state.rs:240),
  `validate_rpc_services` (285), `to_rpc_service` (rpc.rs:217) and `finalized_head` (742). Two conflicts it raised were checked:
  the contract hard-codes `ERC20("Rumi icUSD", "icUSD")` (`BridgedIcUSD.sol:157`), so identical bytecode (D5) cannot also give
  Monad's escrow token a distinct name (D8): the decisions are coupled. The facts file both gave and denied a Monad delay;
  Monad's developer summary states Verified is three blocks after Finalized, now recorded as sourced (the observer change and
  its blocker remain). New open decisions D1a (method names under one Candid) and D1c (keyed providers versus no secrets in
  state) are in the design's section 12.
- BB-35 (W18) dispatched read-only. BB-38 (W16) still running; it was told the Monad delay value and the Solana custody blocker.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W17 | BB-34 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W18 | BB-35 | Sonnet | `claude-sonnet-5-5` | running |
  | W16 | BB-38 | Sonnet | `claude-sonnet-5-5` | running (build lane) |

## Milestone 31 (2026-10-30): planning decisions applied

- Miles settled the routine design decisions for planning under Rob's implementation-decision authority; they authorize no
  deployment, live upgrade, signing, funding, controller change or merge. D1 in-place generalization on a working branch;
  D1a legacy Base-named methods on new EVM instances; D1b additive native RPC variants where useful; D2 and D7 PR #586's
  issued currency with 15-digit per-operation and total caps, MPT deferred; D3 Solana memo plus durable nonce with
  mandatory same-attempt reconciliation and archival history, C4 limit explicit; D4 Sui Move gate with a separate
  UpgradeCap authority; D5 identical bytecode; D8 Monad escrow-backed labels and exact binding; D9 synthetic test ledgers.
  D6 (controllers, custody, authority activation) stays open and does not block disabled groundwork.
- Applied: design section 12 (settled block), 1.5 (D1a naming), 9.1 (`planning_decision` status); the EVM note's
  "proves Base is unchanged" heading now reads as a domain and id regression check only, and owners are assigned by BB-40;
  BB-36 and BB-37 acceptance and their priority items follow D4, D2 and D7. W18 (BB-35) and W16 (BB-38) were told the
  decisions that affect them.

## Milestone 32 (2026-10-30): destination profiles and validator accepted

- BB-38 accepted: new library crate `src/rumi_bridge_destinations` (types, validator rules V1 to V11, nine JSON profiles
  embedded with `include_str!`, focused tests, `examples/dry_run.rs`). No existing canister changes or depends on it; the root
  `Cargo.toml` gains one `members` line and `Cargo.lock` one package entry (serde and serde_json only).
- Miles' scoped preview (three notes) went to the same worker, W16, which applied them: V5 now checks finality VALUES (EVM and
  Solana `finalized`, Sui checkpoint fields, XRPL `validated` plus `tesSUCCESS`, Monad a sourced positive delay of 3 or the
  unresolved blocker), with negative tests for `latest`, `processed`, a zero or missing Monad delay; `solana-return-custody` is a
  required Solana blocker with a missing-blocker test; amount bounds are canonical decimal strings parsed to checked u64, with an
  exact u64::MAX round trip and rejection of non-canonical strings and bare numbers.
- Evidence (W16's run, the mission's one build lane): `cargo fmt -p rumi_bridge_destinations` clean;
  `cargo test --offline -p rumi_bridge_destinations` 51 passed, 0 failed; `cargo run --offline -p rumi_bridge_destinations
  --example dry_run -- 2026-10-30` exit 0 with 20 network lines, all `status=disabled`. The coordinator read the code paths for
  the three notes, the workspace diff and a hygiene scan of the profiles (no addresses, hashes, local paths or keyed URLs) and
  did not rerun the tests.
- Recorded gaps: no Sui devnet record (no stable identifier published), Ethereum endpoint list empty (none on ethereum.org; the
  EVM RPC canister has built-in providers), Solana named wallets empty (none sourced).
- BB-37 (W19) dispatched read-only on PR #586's seam.
- Miles accepted BB-38 at source and local-CLI level on `570b2fae`: he independently ran the built dry-run example for
  2026-10-30 (exit 0, 9 profiles, 20 disabled networks, 0 violations), inspected the three fixes, and confirmed by `git diff`
  that no bridge, backend, Foundry or frontend runtime file changed since `c82f2880`. No tests were duplicated.
- BB-40 direction (Miles): turn the concrete implementation next steps into scoped QUEUED cards and ranked subprojects so
  the Queue stays useful after groundwork closes. Queued means not started: no rollout, runtime implementation, new broad
  test or review gate, or live action begins from this.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W16 | BB-38 | Sonnet | `claude-sonnet-5-5` | finished, accepted (build lane) |
  | W18 | BB-35 | Sonnet | `claude-sonnet-5-5` | running |
  | W19 | BB-37 | Sonnet | `claude-sonnet-5-5` | running |

## Milestone 33 (2026-10-30): Solana note accepted; Sui started

- BB-35 accepted (`docs/coordination/multichain/chain-solana.md`). It plans on D3 (memo plus durable nonce) with exact shapes:
  mint `[AdvanceNonce, CreateIdempotent ATA, MintToChecked(8), Memo]`; user return exactly one top-level `TransferChecked(8)` into
  the canonical return account authorized by the owner plus exactly one return memo (ComputeBudget the only other allowed
  instruction); burn `[AdvanceNonce, BurnChecked(8) by the return owner, Memo]`. Anything else is a stray: never credited, refunded
  to the verified sender. History availability is a mandatory mitigation with blocker `solana-history-availability-unmeasured`; C4
  is stated as unavailable. The SOL RPC canister id is resolved on paper to DFINITY's published production id (README, the NNS
  proposal thread and the IC dashboard agree; the other id fails its checksum); the profile keeps the blocker as an activation
  re-verification. Only published Solana program ids appear in the note.
- The coordinator checked the stray accounting: strays stay in `O_d`, the audit is `custody_measured == RC_d + BU_d + stray_d`,
  and they are excluded from circulation while held. One line added to the design's section 2.4.
- Profile refresh deferred: the chain notes propose profile changes (Solana `burn_id` domain and `mint-account` role, the resolved
  SOL RPC wording, others to come from BB-36 and BB-37). They will be applied in one pass by the same builder (W16) during BB-40,
  not as a second build now.
- BB-36 (W20) dispatched read-only.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W18 | BB-35 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W19 | BB-37 | Sonnet | `claude-sonnet-5-5` | running |
  | W20 | BB-36 | Sonnet | `claude-sonnet-5-5` | running |

## Milestone 34 (2026-10-30): two Solana corrections

- Miles' review of BB-35 (D3 kept). Applied by the coordinator as a paper correction, no program rewrite and no tests:
  1. Section 4 adopted an unrecorded signature on finding our memo. A memo is discovery only: adoption now needs finalized
     successful execution AND message bytes equal to the stored unsigned message (nonce account and value, signer roles, mint,
     recipient, amount), else hold and halt. An advance-only transaction carrying our memo must not make a missing mint look
     delivered.
  2. Classic SPL and Token-2022 let any owner or delegate burn, lowering supply without a return. Our burn is proven by its
     exact `BurnChecked` bytes and the return account's balance effect, never the global supply change. C3 becomes
     `supply_d + UB_d == O_d` (`UB_d` = unattributed holder burns; zero on EVM, the Sui gate and XRPL); until a burn observer
     exists only `supply_d <= O_d` is checked. Policy blocker `solana-unattributed-burn-policy`: no ICP credit without a
     recipient-bound claim, no automatic escrow sweep or refund, no global halt from a legitimate supply decrease; the
     accounting seam is implementation missing.
- Updated: `chain-solana.md` (step 3 wording, C3 line, new section 12 with two future adversarial tests) and the design (burn
  lifecycle, C3, the Solana guard row, V7's Solana blocker list).
- Queued for the single profile refresh pass by W16 during BB-40: add `solana-unattributed-burn-policy` to the Solana profile
  and to the validator's required Solana blockers, with a missing-blocker test, alongside the earlier Solana items.

## Milestone 35 (2026-10-30): final-integration notes received; BB-40 checklist

- Solana main-text contradiction fixed: section 3 of `chain-solana.md` no longer requires global supply to fall by exactly our
  burn. `BU_d -> PR_d` now needs finalized success, message bytes equal to the stored `BurnChecked` message (nonce, signer
  roles, mint, return account, amount, memo) and the return account down by that amount in the same transaction; section 12
  stays as history and the plan is internally consistent.
- BB-40 checklist from Miles' final notes (to run once BB-36 and BB-37 return; no new review round, builder, deployment or
  live action):
  1. Rewrite PR #591's title and body for the final Base implementation plus nine-chain groundwork: keep the exact historical
     Base code and test pins and limits; separate prior Base public-test proof (isolated Sepolia CLI round trip, staging
     sign-in) from current host-only groundwork; keep browser-wallet acceptance and production activation unproven; link the
     design, plan and host verification; no private records, addresses, balances or transactions; no merge.
  2. One profile refresh by the same builder (W16): Solana `solana-unattributed-burn-policy` (profile, required list, missing
     test), `burn_id` domain and `mint-account` role, resolved SOL RPC wording; any XRPL and Sui profile mismatches; drop the
     style-only `sources_contain_no_em_dashes` unit test; keep the meaningful mutations, exact amounts and finality checks;
     report final test counts honestly.
  3. Concrete future queued cards for all nine destinations with owners, paths, dependencies and focused commands, ranked
     subprojects, implementation left queued (at most two Sonnet lanes plus one Haiku task if started later); Base browser
     consent stays separate; no deployment, signing or funding step executable by default.
  4. A small proposed persona or catalog lesson with code pointers (finality-rule enums need value checks; shared JSON amount
     bounds use canonical decimal strings). Proposal only: no memory or catalog write.

## Milestone 36 (2026-10-30): XRPL note accepted

- BB-37 accepted (`docs/coordination/multichain/chain-xrpl.md`; PR #586 untouched). The no-vault return is specified as a
  follow-up to #586: an ICP-side intent binds the destination account, payer, amount and a never-reused tag; the observer accepts
  only a quorum-read, validated, `tesSUCCESS` Payment to the issuer with that tag from that payer, credited on `delivered_amount`;
  payout is an escrow-to-owner ledger job; it never touches a vault or debt.
- The coordinator checked two of its corrections on `b02584e4`: `parse_normal_request` (the 15-digit check) has no non-test
  caller, and `begin_receive_canonical` returns escrow for undelivered claims, not delivered tokens. Other findings recorded as
  implementation missing in the design: caps accept any u64, pending imports can race past the total cap, `O` has no origin
  split. Implementation constraint: the backend Wasm is 9,648,104 bytes, against PocketIC 7's 10 MiB cap and the 12 MiB
  production limit.
- Profile refresh items added for W16 (BB-40): the XRPL devnet record is unsupported by #586 (remove or mark unsupported), the
  derivation template must use #586's network names rather than a numeric id, and the testnet RPC blocker should reflect two
  hosts at quorum two with one on a non-443 port. The design's 9.2 now lists `max_outstanding_e8s`.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W19 | BB-37 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W20 | BB-36 | Sonnet | `claude-sonnet-5-5` | running |

## Milestone 37 (2026-10-30): Sui note accepted; BB-40 started

- BB-36 accepted (`docs/coordination/multichain/chain-sui.md`). It specifies the D4 Move gate `rumi_icusd_gate`: `init` (wraps
  `TreasuryCap` in a shared gate, supply cap 0 so it is disabled on chain), `mint` (aborts on a consumed mint id, updates the minted
  total), `bridge_out` (burns, computes `transfer_id`, increments the counter, emits `BridgeOut` with the ICP account), `set_limits`
  and a version guard `migrate` (old package versions stay callable after an upgrade). Design corrections applied: the signing
  digest and the lookup transaction digest are different hashes; the equivocation lock is sourced, so same-bytes retries are
  mandatory; `TreasuryCap` lives inside the gate, so `UpgradeCap` custody is a mint-path authority.
- All nine chains now have accepted groundwork notes. BB-40 started: the coordinator writes the plan and queued cards; W16 was
  resumed for the single final profile refresh (Solana, XRPL and Sui items from the chain notes, plus dropping the style-only
  em-dash test).
- Sui correction (Miles, after acceptance; applied by the coordinator): section 3 rule 4 of `chain-sui.md` no longer lets
  epoch advancement plus a missing digest authorize a replacement or refund. Attempts stay held until authoritative non-execution
  proof; delivery is reconciled from the gate's consumed mint-id record; no second payable claim. Unresolved activation seam
  `sui-epoch-expiry-nonexecution-proof`, also in the design's 6.2 rule 4 and added to W16's refresh batch. No new research or
  build gate.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W20 | BB-36 | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W16 | BB-40 profile refresh | Sonnet | `claude-sonnet-5-5` | resumed (build lane) |
  | W21 | BB-40 step extraction | Haiku | `claude-haiku-5-5` | running (scratch output only) |

## Milestone 38 (2026-10-30): groundwork close-out, BB-40 in Review

- Final profile refresh by the same single builder (W16), no Cargo changes: Solana `solana-unattributed-burn-policy` (now
  required), `solana-history-availability-unmeasured`, `burn_id` domain and roles, SOL RPC re-verification wording; XRPL devnet
  record removed, `{network_name}` derivation template, per-network port blocker, the PR #586 wiring gaps as blockers; Sui
  prerequisites, roles and five blockers including `sui-epoch-expiry-nonexecution-proof`; the style-only em-dash test dropped.
  Focused run: `cargo test --offline -p rumi_bridge_destinations` 63 passed, 0 failed; dry run exit 0, 9 profiles, 19 networks,
  0 violations. The coordinator scanned the profiles statically (no private data, all disabled) and did not rerun the tests.
  Not applied, recorded in the deferral lane: Solana refund-id domain and extra Solana fields.
- Plan: `docs/superpowers/plans/2026-10-30-multichain-icusd-groundwork.md`. Queued future cards BB-42 to BB-61 in six paused
  subprojects ranked after the Base browser test: EVM family, shared reporting and frontend, Solana, XRPL follow-ups to PR #586,
  Sui, and Rob's D6 decision. Miles' scheduling notes applied: BB-47 and BB-51 do not wait on BB-58 (preference only), BB-53
  waits on D6 only for live authority steps, BB-55 may use a stacked or rebased #586 branch, the external Move review is deferred
  pre-real-value work. Nothing starts, deploys, signs or funds by default.
- PR #591: title and description rewritten for the Base bridge plus nine-chain groundwork; historical Base pins kept (`4169feb8`,
  raw Wasm `a0049b9a`, install artifact `0b324387`, the check table); prior Base public-test proof kept apart from host-only
  groundwork; browser-wallet acceptance and production activation stated as not established. Still a draft, open, unmerged,
  head `9806e539` at the edit. Verified before writing: no bridge, backend, Foundry, frontend or declaration file changed after
  `4169feb8`; since `c82f2880` the only non-docs change is the new library crate and its workspace line.
- Deferral lane: `docs/coordination/demo-deferred/multichain-icusd.md` (MC-DEF-1 to MC-DEF-6, actual shortcuts only). The central
  index `docs/coordination/DEMO_DEFERRED_WORK.md` lives in the primary checkout, which holds protected unrelated changes, so its
  link line is left for Miles to add rather than written from here.
- Proposed durable lessons (proposal only, no persona or catalog write): plan section 6, with code pointers (finality values,
  not just the rule enum; canonical decimal strings for shared JSON amount bounds).

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W16 | BB-38 and BB-40 profile refresh | Sonnet | `claude-sonnet-5-5` | finished |
  | W21 | BB-40 step extraction (scratch output) | Haiku | `claude-haiku-5-5` | finished |

  All workers in this mission: W10, W13, W21 Haiku; W11, W12, W14 to W20 Sonnet; no Codex agent; no lane was duplicated.

- Accepted (Miles): `9806e539` at groundwork source and local-CLI level. He reviewed the final type, validator and test diff
  and the plan and priority corrections, and independently ran the rebuilt dry run (9 profiles, 19 networks, all disabled,
  0 violations); the runtime, backend, Foundry and frontend diff since `c82f2880` is empty. W16's 63 passing tests were not
  rerun. BB-40 is done; BB-42 to BB-61 stay queued and no implementation has started.
- Final pins: branch `codex/base-icusd-bridge`; groundwork acceptance commit `9806e539`; Base code candidate `4169feb8`, raw
  Wasm sha256 `a0049b9a...e157`, install artifact sha256 `0b324387...929a` (full values in PR #591 and the review record);
  baseline `main` `32358d23`, multichain freshness check `origin/main` `39da7d1f`. Commands: `cargo test --offline -p
  rumi_bridge_destinations` (63 passed), `cargo run --offline -p rumi_bridge_destinations --example dry_run -- 2026-10-30`
  (exit 0), `python3 docs/coordination/base-icusd-bridge-lookout-derive.py`, the offline priority validator, then the scoped
  Lookout publisher's dry run, changed-only publish and no-op readback.
- Final Lookout state: published changed-only through the same scoped manifest and identity; the immediate dry run reported
  the board (68 cards: 20 Queue, 1 Blocked, 47 Done), both summaries, glossary, priorities and repository unchanged. Exact
  revisions are in the private publisher record.
- Final readbacks (Miles, 2026-10-30): Lookout revision 37 shows "Rumi · Multichain icUSD" with 68 cards, Queue 20,
  In progress 0, the ranked future subprojects paused and BB-27 blocked. GitHub showed PR #591 open, draft and unmerged with the
  rewritten description at `9806e539`; the branch then gained only the docs close-out commit `9eb95661` and this note. Miles
  added the branch-URL link to this lane file in the primary checkout's `DEMO_DEFERRED_WORK.md` index, preserving its contents.
  Groundwork is closed; BB-42 to BB-61 stay queued and no implementation has started.

## Milestone 39 (2026-10-30): implementation phase opened; BB-42 started

- Rob's goal (relayed by Miles): "Work through your prioritized queue of tasks with Sonnet and Haiku. Keep the work focused
  on a couple of tasks at a time. Keep the lookout board updated." This opens local source delivery of BB-42 to BB-60 in rank
  order. Same coordinator, board, Lookout project, branch and worktree. Starting head `1701ebf7`; the groundwork evidence on
  `9806e539` (63 host tests, dry run 9 profiles and 19 networks) stays valid until a card changes that crate.
- Historical claims versus new work: every earlier statement that no bridge runtime, backend, Foundry, frontend or
  declaration file changed after `4169feb8` covers commits up to `1701ebf7` only. BB-42 onward change bridge runtime source by
  design, keeping the Base Candid interface, stable-state decoding, signing bytes and durable ids unless a card says otherwise.
- Still held: merge, deployment, public signing, broadcast or funding, governance, controller or role changes, production or
  new-network activation, any wallet use (BB-27), BB-46 live probes and BB-61 custody decisions.
- Priorities: `evm-implementation` and `shared-reporting-frontend` in progress; Solana, XRPL and Sui source-authorized but held
  behind them by rank; D6 and production launch unchanged.
- Staffing at dispatch (actual models read from each run's record right after start):

  | Handle | Card | Scope | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- | --- |
  | W22 | BB-42 | `ids.rs`, new `chains.rs`, `lib.rs`, one `state.rs` call site and test call sites; sole build executor | Sonnet | `claude-sonnet-5-5` | running |
  | W23 | BB-58 inventory | read-only: analytics supply formula, bridge audit fields, focused test commands; no build | Haiku | `claude-haiku-5-5` | finished (coordinator spot-checked) |

## Milestone 40 (2026-10-30): BB-42 accepted; BB-43 and BB-58 started

- BB-42 `c57c6009` (W22): `chains.rs` (Base rows 8453, 84532, 31337, slug `base`), slug templates for the mint and re-mint
  domains, escrow label, key root and salt label in `ids.rs`; the removed constants survive only as test pins;
  `ids::chain_slug` panics on an unknown id instead of falling back to Base; `post_upgrade` refuses a stored chain id with no
  profile. Worker run: `cargo test --offline -p rumi_icusd_bridge --lib` 453 passed (baseline 445); clippy clean; release Wasm
  3,850,049 bytes; no `.did`, declaration or Cargo change. Coordinator check: reran the unit tests (453 passed) and recomputed
  all six golden mint and re-mint ids with Foundry `cast` from the pre-change domain strings (all equal).
- Accepted (Miles): `c57c6009` at source and local-test level after reviewing all six files (template strings, hash input
  order, key bytes, init salt and escrow preserved; unknown chains fail closed; no Candid or stable shape change).
- Dispatched with exclusive files (both read from their run records as `claude-sonnet-5-5`):

  | Handle | Card | Scope | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- | --- |
  | W22 | BB-42 | as above | Sonnet | `claude-sonnet-5-5` | finished, accepted |
  | W24 | BB-43 | `chains.rs`, `state.rs`, `evm/rpc.rs`, `observer.rs`, `icrc21.rs`, `views.rs`, `ids.rs` tests; only release builder | Sonnet | `claude-sonnet-5-5` | running |
  | W25 | BB-58 | `src/rumi_analytics/**`, its declarations by the canonical script, `rumi_bridge_destinations` aggregate module; unit tests only | Sonnet | `claude-sonnet-5-5` | running |

  One builder: W24 alone runs release Wasm builds; W25 runs unit tests only, and both share this worktree's `target/`, where
  cargo's lock serializes the runs.

## Milestone 41 (2026-10-30): BB-58 in Review

- `15bb73dc` (W25, `claude-sonnet-5-5`), on top of `628adba2`. Analytics: new optional `SlimState.bridge_escrow_accounts`
  (Candid `opt`, so a pre-change snapshot decodes it as null; tests `decodes_legacy_slim_state_without_new_schedule_fields`
  extended and `decodes_pre_bridge_escrow_slim_state_with_none_escrow_list` added); admin update
  `admin_set_bridge_escrow_accounts` (at most 32, no duplicate including the zero-subaccount alias, no anonymous owner, empty
  restores today's behavior); `refresh_supply_cache` stores `total - sum(escrow balances)`; a failed read or a sum above the
  total keeps the previous value and bumps the error counter; a changed list clears the cached figure and a result computed
  under a stale list is discarded, so the raw total is never published once accounts are configured. With no accounts the
  refresh is unchanged. `ProtocolSummary`, HTTP shapes and metric names unchanged. Additive Candid (`didc check` new against
  old exit 0); bindings regenerated only by `scripts/regenerate-declarations.sh rumi_analytics` (its `.did` copy also picked
  up drift that predates this card).
- Host-only aggregate `rumi_bridge_destinations::aggregate` (design 2.4): only `in_d` destinations count, the XRPL lane is
  separate, custody is excluded once, `custody != rc + bu + stray` is a failure flag, checked u128 arithmetic, duplicate ids
  refused; ten golden tests.
- Worker checks: `cargo test --offline -p rumi_analytics --lib` 146 passed (baseline 129); `--test audit_pocs_dos_010_stagger`
  21 passed; `cargo test --offline -p rumi_bridge_destinations` 73 passed (63 plus 10); dry run exit 0, 9 profiles, 19
  networks, 0 violations. Clippy with `-D warnings` is not clean at baseline in either crate (pre-existing, outside these files);
  the new files are clean. No Cargo change. No Wasm build in this lane (one release builder, W24).
- Coordinator review: read the state field, setter, refresh path, stale-result guard and aggregate formulas against design
  2.4; no defect found. Omission logged: MC-DEF-7 (no PocketIC test of the on-chain read path).

## Milestone 42 (2026-10-30): BB-43 and BB-58 follow-ups

- BB-43 `fecaf9d5` (W24, `claude-sonnet-5-5`): `chains.rs` rows gain display name, allowed keys, native provider family,
  head rule, install gate and consent wording; 13 rows (Base three enabled; Ethereum, Arbitrum, Robinhood, HyperEVM, Monad
  mainnet and testnet described but install-disabled as an interim state). `validate_init` and both RPC validators follow the
  row; every Base error string unchanged (new tests pin them). `rpc::finalized_head` is now the dispatcher:
  `tag_finalized_quorum` is the old body unchanged; `MonadVerified { delay_blocks: 3 }` takes the quorum finalized height
  minus 3 and the quorum hash there; `Blocked` errors before any provider call; no rule sends `latest`. The idle probe keeps
  Base's finalized-tag call and pins Monad to the numbered head. ICRC-21 and the finality text come from the row with Base
  byte-identical. Worker run: 483 unit tests passed (was 453), clippy clean, release Wasm 3,865,565 bytes; no `.did`,
  declaration, stable-state or Cargo change. Coordinator review: read the validation, dispatcher and Monad head code; the
  test helper now sets a Base Sepolia chain id because a head rule needs a profiled id.
- BB-43 not yet accepted (Miles, `review-bb43.md`): three settlement proofs still read the raw `finalized` tag, the sites W24
  had flagged: `request_refund`'s mint-consumed check (`flows/deposit.rs:546`), `fetch_nonces` (`evm/lane.rs:975-982`) and
  `clear_lane_halt_with` (`evm/lane.rs:1559`). Monad needs its Verified head there; Base stays unchanged. Same W24 resumed with
  `flows/deposit.rs` and `evm/lane.rs` added to its files, and it is again the only release builder.
- BB-58 review (Miles): the analytics subtraction, stable field, legacy decode, stale-result discard and declarations are sound.
  Not accepted as complete reporting: `aggregate` is a pure function with no consumer that reads balances and supply itself
  (design 2.4), and D/X disjointness was only documented (a D entry named for XRPL plus the XRPL lane counted one escrow twice).
  Same owner W25 resumed for one bounded follow-up in `rumi_bridge_destinations`: a typed input boundary (catalog-bound
  network ids, XRPL family only as X, unique escrow bindings across D and X) and a `Readings`-driven collector with a fixture
  transport and a runnable example; Solana, Sui and XRPL adapters report unavailable with their source card. Miles'
  clarification: the real transport source (IC query and EVM JSON-RPC client, with a scoped dependency if needed) is already
  authorized source work, queued next on BB-58 for the same owner and verified with a mocked transport; only executing public
  queries is held. BB-58 is not Done on fixture-only collection.
- Staffing (two Sonnet lanes, no Haiku task; W26 briefly made a third and was stopped):

  | Handle | Card | Scope | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- | --- |
  | W24 | BB-43 | as above | Sonnet | `claude-sonnet-5-5` | first pass finished; reopened below |
  | W25 | BB-58 follow-up | `src/rumi_bridge_destinations` only; unit tests only | Sonnet | `claude-sonnet-5-5` (same run, resumed) | running |
  | W24 | BB-43 correction | adds `flows/deposit.rs`, `evm/lane.rs`; only release builder (also builds the analytics Wasm once) | Sonnet | `claude-sonnet-5-5` (same run, resumed) | running |
  | W26 | BB-44 | `types.rs`, `evm/rpc.rs`, `chains.rs`, `state.rs`, the bridge `.did` and bindings | Sonnet | `claude-sonnet-5-5` | stopped by the coordinator minutes after start to keep two Sonnet lanes when BB-43 reopened; partial edits saved privately, files restored to `fecaf9d5`; BB-44 back to Queue |

## Milestone 43 (2026-10-30): both follow-ups delivered

- BB-43 `685984b9` (W24, same run): `request_refund` reads `MintConsumed` at `rpc::settlement_block_tag()`; new required
  `LaneIo::settlement_tag()` used by `fetch_nonces` and `clear_lane_halt_with` (the head is resolved first, so a blocked
  chain sends nothing; Base request order and error strings unchanged; latest and pending reads for allocation and
  rebroadcast untouched). Seven new tests drive the production `ChainIo` and refund path against the mock transport (Monad
  reads at `0x3e5` when finalized is 1000). Worker run: 490 unit tests passed (was 483), clippy clean, bridge release Wasm
  3,867,458 bytes, analytics release Wasm builds (3,671,657 bytes). Coordinator check: the raw `BlockTag::Finalized` now
  appears in production code only at `evm/rpc.rs:635` (tag-to-string mapping) and `:776` (the dispatcher's tag-rule arm);
  every other hit is inside test modules.
- BB-58 `78e8ae3d` (W25, same run): `escrow.rs` (`EscrowBinding`: canonical principal text decoded and compared as bytes,
  missing and all-zero subaccount equal, anonymous refused); `aggregate.rs` `check_boundary` (catalog-bound network ids,
  unique across D, excluded entries and X; an XRPL-family network never in D; the lane must be XRPL; every counted D entry
  needs a binding; no escrow repeated across D and X); `collect.rs` (`Readings` trait with no self-report method; EVM reads
  supply and escrow at the profile's head rule; Solana, Sui and the XRPL lane report unavailable with BB-49, BB-53 and
  BB-56/57 before any read; `FixtureReadings` prints `source: fixture`); example `collect_fixture` and two synthetic
  fixtures. Worker run: 97 tests passed (was 73); dry run unchanged. Coordinator check: reran the crate tests (97 passed), ran
  both fixtures (base-only exit 0 with `source: fixture` and `circulating_total` 1000; overlap exit 1, "destination
  xrpl-mainnet is an XRPL network: XRPL is the lane X and never a member of D"), and re-encoded the synthetic fixture
  principal independently (matches).
- Still open on BB-58 (not Done): the real client transport (IC query and EVM JSON-RPC), queued next for the same owner with
  mocked-transport tests; running it against public networks stays held.

## Milestone 44 (2026-10-30): BB-43 accepted; BB-58 components accepted; BB-44 resumed

- Accepted (Miles) at source and local-test level: BB-43 through `685984b9` (dispatcher, table and the three corrected proof
  paths; request-body regressions keep Base and number Monad's Verified head). BB-58 components `15bb73dc` and `78e8ae3d`
  (D/X boundary, normalized escrow identity, collector; Miles independently ran `collect_fixture`: Base exit 0, overlap exit 1,
  both `source: fixture`, no calls). BB-58 stays open for the real IC query and EVM transport source and later native
  adapters.
- Dispatch (two Sonnet lanes, one release builder): W25 resumed (same run) on the BB-58 transport, feature-gated in
  `rumi_bridge_destinations`, mocked transports only, never run live; W26 resumed (same run) on BB-44 from its preserved
  patch, which applied cleanly to `324e6d13`; W26 is the only release builder.

  | Handle | Card | Requested | Actual (run record) | State |
  | --- | --- | --- | --- | --- |
  | W25 | BB-58 transport | Sonnet | `claude-sonnet-5-5` | running |
  | W26 | BB-44 | Sonnet | `claude-sonnet-5-5` | running |

- PR #591 description rewritten in place (draft, open, unmerged, head `324e6d13`): "Three parts"; the Base checks are stated
  for `4169feb8` only, with runtime unchanged only through `1701ebf7`; new Part 3 lists the cards, commits, acceptance,
  checks and the MC-DEF-7 omission; holds now name public queries and analytics escrow configuration.

## Milestone 45 (2026-10-30): BB-44 in Review; BB-45 started

- BB-44 `0bf16f9f` (W26, resumed run, `claude-sonnet-5-5`): `RpcServiceConfig` gains `EthMainnet`, `EthSepolia` and
  `ArbitrumOne` (appended; `BaseMainnet` and `Custom` unchanged), mapped in `to_rpc_service`; no Arbitrum Sepolia native
  service exists in the EVM RPC mirror, so none was added; the Ethereum, Ethereum Sepolia and Arbitrum One rows list their
  family, and each family is valid only on its own row. The crate `.did` was regenerated by its Candid test, bindings only by
  `scripts/regenerate-declarations.sh rumi_icusd_bridge`; `didc check` new against old exit 0 (input type widened). Legacy
  stored configs (Base `BaseMainnet` and `Custom` sets) decode unchanged, with golden bytes pinned; a config holding a new
  variant cannot be read by an older module (only non-Base rows can hold one). Worker run: 503 unit tests (was 490), clippy
  clean on `--lib`, release Wasm 3,887,014 bytes (+19,556). Coordinator review: commit touches only its eight files; Base rows,
  gates and head rules unchanged.
- BB-45 dispatched to the same W26 (only release builder): source-level installs for Ethereum, Arbitrum and Monad with
  per-chain defaults and local tests, Robinhood and HyperEVM still blocked, a sibling Foundry deploy script; W25 continues the
  BB-58 transport.

## Milestone 46 (2026-10-30): BB-44 accepted; proposed lesson

- Accepted (Miles): BB-44 `0bf16f9f` at source and local-test level (provider mappings, per-chain family validation, the
  five-provider cap, legacy CBOR pins and decoding, additive input-only Candid and generated diff); no repeat run.
- Proposed durable lesson (proposal only; no memory, persona or catalog write): when introducing a head-rule dispatcher, audit
  every raw finality-tag state proof, including refund and nonce recovery. Recorded as plan section 6 item 3 with pointers.
- BB-58 transport `8b88a96f` (W25, same run): feature `live-transport` (off by default; the default build still depends only
  on `serde` and `serde_json`) adds `JsonRpcTransport` with a `reqwest` client (https only, no redirects, timeout and body
  cap) and `IcQuery` with an `ic-agent` 0.37.1 client (anonymous, query calls only, response verification at its default,
  mainnet root key only). EVM supply is read at the profile's settlement head with quorum agreement on height, hash and
  `totalSupply` (Monad finalized minus 3; Robinhood and HyperEVM blocked with zero requests; only `finalized` is ever sent
  as a tag). IC reads: ledger `icrc1_total_supply` and `icrc1_balance_of`, and the backend's per-chain debt-backed supply
  through `get_chain_public_launch_status` (an unregistered chain is an error, not zero). `LiveReadings` labels
  `source: live` with a read time; example `collect_live` needs an explicit endpoint file and `--live`, and was not run.
  Worker run: default tests 97 passed, `--features live-transport` 127 passed, dry run and both fixtures unchanged, canister
  crates still check. Coordinator check: Cargo.lock adds 16 packages and removes or changes none (versions and checksums
  compared). Not exercised: `ic-agent` query-signature verification against a replica. crates.io was reached once to fetch
  the new crates.
- Proposal for Miles: close BB-58 on this transport and track the Solana, Sui and XRPL readers on BB-49, BB-53 and BB-57.
- Miles' review of `8b88a96f` (source and mock evidence only; IC method, signature and record slice match the backend
  Candid; anonymous query-only with default verification kept): not closed. (1) `live/evm.rs` never asks `eth_chainId`, so a
  manifest labelled for one network with another network's providers could vote; providers must be bound to the profile's
  chain identity before head and supply votes, blocked profiles still sending nothing. (2) `live/ic.rs` sums a raw chain-id
  list, so `[1030, 1030]` doubles one supply; duplicates must be rejected before any query. (3) The accepted per-destination
  audit query collection (plan line 56, design 2.4) is still missing from `Readings`. The native readers move to BB-49,
  BB-53 and BB-57, whose cards and plan rows now name them (bookkeeping, not completion evidence). BB-58 back to Queue; the
  bounded correction goes to the same W25 at the next free lane, with W26 (BB-45) and W27 (BB-59) left running and no third
  lane.
- Running (two Sonnet lanes): BB-45 (W26, only Rust and Wasm release builder) and BB-59 (W27, new run, `claude-sonnet-5-5`,
  frontend only: `baseBridge` services, bridge components and route; focused vitest, `npm run check` against its baseline,
  one `npm run build`).

## Milestone 47 (2026-10-30): BB-45 in Review; BB-58 corrections started

- BB-45 `3c5029cb` (W26, same run): install gate `Enabled` in source for Ethereum 1 and 11155111, Arbitrum 42161 and 421614,
  Monad 143 and 10143; Robinhood (4663, 46630) and HyperEVM (999, 998) stay disabled with reasons that lead with their blocker
  ids (`robinhood-finalized-tag-probe`, `hyperevm-historical-state`). Each row carries `ChainDefaults` applied by
  `validate_init` (Base byte-identical to the pre-table config, pinned field by field and as CBOR bytes). Sourced: Monad base
  fee minimum 100 gwei (default cap 500 gwei), 10 MON reserve (default 15 MON), 100-block log range; HyperEVM 50-block range.
  Unsourced starting points for `admin_set_gas`: Ethereum caps 50 and 2 gwei and 0.001 ETH reserve; Arbitrum and Robinhood
  gas limits 600,000 and 200,000; Monad gas limits 500,000 and 150,000; all 90 s replace waits and non-Base 2,000-block
  ranges. New `foundry/script/DeployBridgedIcUSDEvm.s.sol` (allow-list 11155111, 421614, 10143, 31337, plus 1, 42161, 143
  each behind its own `ALLOW_<CHAIN>_MAINNET`; identical `BridgedIcUSD` bytecode with a runtime code-hash check; Base ids
  rejected) and `foundry/test/BridgedIcUSDEvmDeploy.t.sol`. Worker run: 513 unit tests (was 503), clippy clean, release Wasm
  3,888,045 bytes, `forge test --offline --match-contract BridgedIcUSDEvmDeploy` 17 passed and `--match-contract
  DeployBridgedIcUSD` 17 passed (the Base script and its tests unchanged). Coordinator check: the Base script and test diff is
  empty; `admin.rs` unchanged (defaults fit the global bounds). Still unmeasured: mint gas on Arbitrum, Monad and Ethereum
  Sepolia, and Ethereum fee caps; Monad fast-tag consensus and D8 labels stay activation items. Downgrade caveat now live in
  source: an Ethereum or Arbitrum instance installed with native providers must never be rolled back below BB-44.
- PocketIC test files were not compiled in this lane; they do not import the crate, and a search found no assertion that
  install refuses chain ids 1, 42161 or 143.
- W26 left a filesystem-wide search for `forge` running after it finished; the coordinator stopped it (read-only, no effect).
- BB-58 corrections dispatched to the same W25 (resumed, `claude-sonnet-5-5`): provider chain identity, count-once chain ids,
  per-instance audit collection with independent C1 and supply checks; mocks only. Lanes: W25 and W27.

## Milestone 48 (2026-10-30): BB-45 accepted

- Accepted (Miles): BB-45 `3c5029cb` at source and local-test level (install and default application, Base whole-config
  CBOR pin, own-chain, key and provider gates, unchanged admin bounds and Base script, sibling chain flags, role separation
  and runtime-code check, 513 unit tests and forge 17 plus 17). No public deployment and no measured new-chain gas is claimed.
- Wording note applied by the coordinator (comment only, `chains.rs` Arbitrum defaults): simulation at the configured limit
  lowers the risk of a paid revert but cannot rule one out after state or fee changes. No behavior change; not rebuilt.
- EVM family source work through BB-45 is accepted. Miles' priority decision: BB-46 needs public activation evidence and
  stays held (board `blocked`); optional probe tooling and runbooks are deferred preparation, not a lane, and BB-46 is never
  Done from tooling alone. Order: BB-59 review, then BB-60 (next bounded source card), then BB-47 Solana. Lanes stay W25
  (BB-58 corrections) and W27 (BB-59).

## Milestone 49 (2026-10-30): BB-58 corrections in Review; BB-47 started

- BB-58 `10c18502` (W25, same run; source and mocks only, nothing run against a public network): every EVM provider is
  asked `eth_chainId` after the blocked check and before any head or supply vote, and only providers matching the profile
  network's chain id vote (quorum from matching providers alone; blocked profiles still send nothing); repeated debt-backed
  chain ids are refused before any query; each counted EVM destination binds its bridge instance, whose `get_supply_audit`
  (record pinned to the bridge `.did` text) is reported beside the independent reads, with C1 and the supply bound
  recomputed from independent reads (`CoverageBroken`, `SupplyBoundBroken` and `AuditEscrowAccountMismatch` are failures;
  self-report differences are info). Worker run: 107 default and 147 feature tests passed; no Cargo change. Coordinator
  check: reran the default tests (107 passed) and the three fixtures (base exit 0; C1 break exit 3 with `CoverageBroken`;
  XRPL-in-D overlap exit 1); liabilities match the bridge's own definition (`accounting.rs` liabilities: outstanding,
  pending mint, pending release, pending refund, held, fees). For Miles: `AuditEscrowAccountMismatch` as a failure goes
  beyond the brief (an audit naming another escrow account would make C1 compare the wrong account).
- BB-47 dispatched on the freed lane to a new Sonnet run (W28, `claude-sonnet-5-5`, the only Rust builder): new crate
  `rumi_icusd_sol_bridge` (name proposed), ids and roles, memo grammar, accounting with `RC`, `BU` and `stray`, mint, burn and
  refund builders plus the return shape check with golden vectors, minimal canister skeleton. BB-60 waits for BB-59's review
  and then takes W27's lane, keeping Miles' order.
- Push pause (Miles, resource decision): each push starts the heavy Stability Pool PR workflow, so no git push until
  2026-10-31 03:45 UTC, then one batched push of the reviewed source and docs. Local source, tests, commits and sanitized
  Lookout updates continue; pins after `10c18502` are local, not yet pushed. Last push confirmed at 03:06 UTC: remote head
  equals local `10c18502`. No workflow edit, cancellation, new CI or timer.

## Milestone 50 (2026-10-31): BB-58 accepted for EVM reporting

- Accepted (Miles): BB-58 at `10c18502` together with `15bb73dc`, `78e8ae3d` and `8b88a96f`, at source, fixture and mock-test
  level (chain identity filtering before votes with unchanged quorum and zero-call blocked paths; duplicate backend chain ids
  refused before queries; the actual `SupplyAudit` Candid mapping; instance uniqueness and account binding; independent
  escrow and supply comparisons; 107 default and 147 feature tests and the fixtures). `AuditEscrowAccountMismatch` as a
  failure confirmed: C1 must compare the configured escrow, so it implements the accepted binding requirement.
- Not claimed: any live query, replica-verified query signature or deployed analytics configuration. Native reporting stays a
  deliverable of BB-49 (Solana), BB-53 (Sui) and BB-57 (XRPL), not complete.
- Lanes: W28 (BB-47, sole Rust builder) and W27 (BB-59, then BB-60). Push pause in force until 03:45 UTC; evidence after
  `10c18502` is local and not yet pushed.

## Milestone 51 (2026-10-31): BB-59 delivered; one correction; BB-60 queued for the same owner

- BB-59 `2191d2b6` (W27, `claude-sonnet-5-5`, local, not pushed): `destinations.ts` (Base synthesized from `get_bridge_info`
  through the unchanged `resolveBridgeConfig`; five static disabled EVM entries with string ids), `destinationLoader.ts`,
  synchronous `signingGuard.ts` `assertCanSign` (disabled, missing or wrong-network refusal; no pause input, so DISABLED and
  PAUSED stay apart), guards in both flows before the first wallet or signer call, intents version 2 with a destination key
  on new intents (legacy intents keep `v: 1`, no key, read as Base; records above version 2 stay read-only), EVM chain rows,
  `DestinationSelector.svelte` (disabled entries not selectable or focusable) and page wiring. Worker run: bridge vitest 17
  files, 476 passed, 1 skipped; `npm run check` 30 errors and 66 warnings before and after with identical problem lists (none
  in bridge files); `npm run build` passes. Coordinator check: reran the bridge vitest (476 passed, 1 skipped) and read the
  intent version logic (the record version moves to 2 while each legacy intent keeps `v: 1`, so a legacy recovery row is
  never dropped).
- Miles' review (`review-bb59.md`): `icpToBase.ts` `submitToBridge`, shared by second click and uncertain retry, checks that
  the deps' destination is enabled but not that the intent's destination matches it, so an intent saved for another
  destination could be sent through Base. Same W27 resumed: reproduce, refuse synchronously before any signer call without
  rewriting the durable intent, focused tests for both paths and legacy Base success.
- BB-60 then goes to the same W27, with Miles' interface rule: profile network ids are per network, so a mainnet and a testnet
  instance never share one new intent destination key, and a keyless legacy intent binds to the page's Base network. The
  coordinator runs the Rust validator and dry run at integration (W28 stays the sole Rust builder).

## Milestone 52 (2026-10-31): BB-59 fix and BB-60 delivered

- BB-59 fix `103ad82c` (W27, same run; local, not pushed): `submitToBridge` refuses, synchronously and before any signer
  call, an intent whose destination differs from the deps' destination (`intentBindingMessage`); the intent and request id
  are left unchanged. Already-run results from W27's handback: `signingGuard.spec.ts` 6 failed and 25 passed before the fix
  (reproduction), 31 passed after; all bridge specs 17 files, 485 passed, 1 skipped; `npm run check` 30 errors and 66
  warnings, problem list identical to baseline. Miles found the source sound; acceptance pending this receipt.
- BB-60 `28ba5867` (W27, same run): pure `profiles.ts` reader (amounts validated as canonical decimal u64 strings, a JSON
  number there is an error; identities stay strings; duplicates refused) with `profiles.spec.ts` over the real nine files;
  not wired into the page (the files carry about 165 KB of internal prose); destination ids are now profile network ids
  (`base-mainnet`, `base-sepolia`, `ethereum-mainnet`, ...), the Base key comes from the configured actor's chain id (8453,
  84532, 31337; an unknown chain is a disabled `base-unknown` that never gets a writable key); keyless legacy intents belong
  only to the page's Base destination; a version 2 intent keyed with a bare family label is unreadable and goes to the
  existing recovered path. EVM profile upkeep: `evm-wasm-generalization` retired on the five EVM profiles and replaced by
  `evm-instance-not-activated` (external); Monad and Arbitrum code blockers reworded to the remaining measurements or the
  fast-tag probe; Robinhood and HyperEVM blockers say the chain table keeps those rows closed; status stays `disabled`,
  `instance_ref` null; Base, Solana, Sui and XRPL profiles untouched. Worker run: bridge vitest 18 files, 521 passed, 1
  skipped; check at baseline; build passes. Coordinator check: `cargo test --offline -p rumi_bridge_destinations` 107 passed
  and the dry run `OK: 9 profiles, 19 networks, 0 violations (date 2026-10-31)` on the edited profiles; reran the bridge
  vitest at `28ba5867` (521 passed, 1 skipped).
- Naming note for BB-46: the chain table's Robinhood blocker is `robinhood-finalized-tag-probe` while the profile id the
  validator requires is `robinhood-finalized-tag-unverified`; align them when BB-46 flips the row.
- Lanes: W28 (BB-47, sole Rust builder) running; W27 free.

## Milestone 53 (2026-10-31): BB-59 and BB-60 accepted; BB-55 started

- Accepted (Miles): BB-59 (`2191d2b6` plus `103ad82c`) and BB-60 (`28ba5867`) at source and local-test level. The reader is
  pure and not wired into the page; no dynamic activation and no browser-wallet proof is claimed. The shared reporting and
  frontend subproject is done for its current cards; the Solana and Sui wallet connectors stay on BB-50 and BB-54.
- BB-55 (XRPL, rank ahead of Sui): Miles authorized an isolated stacked source checkout. The coordinator created
  `.worktrees/xrpl-icusd-bb55-20261031` on new branch `codex/xrpl-icusd-followups` at PR #586's exact head
  `b02584e4413fbffe0d65d2c6ff44d16bf82c19ed` (PR #586 open and unmerged; the protected checkout
  `.worktrees/xrpl-icusd-20261018` and branch `codex/xrpl-icusd` untouched). Builds share this bridge worktree's `target/`
  through `CARGO_TARGET_DIR`, serialized by cargo's lock; focused backend unit tests only, no Wasm build. Frontend parity
  tests may reuse this worktree's installed node modules by symlink, without installing anything (PR #586 also lists
  `xumm`). Hazard noted: an XRPL-branch backend Wasm built into the shared target would shadow this branch's; none is built.
- Dispatched: W29 (new run, `claude-sonnet-5-5`) on BB-55. Lanes: W28 (BB-47, sole Rust build owner) and W29.
- Sui BB-51 needs the Sui CLI, which is not installed. Miles' scope correction: local implementation authorization already
  covers necessary development dependencies and CLI tooling, so when BB-51 reaches a free lane its worker installs a pinned
  official Sui CLI with verification, local to the workspace or a temporary folder; no separate human approval and no
  speculative install now. Likewise canonical local npm setup for BB-55 parity is in scope if the reused packages are not
  sufficient. An actual automatic tool-review denial is still reported with its exact action and reason, never retried
  another way. No wallet, public-probe, deploy or live authority follows from this.

## Milestone 54 (2026-10-31): BB-47 in Review; BB-48 started

- BB-47 `df53ea32` (W28, `claude-sonnet-5-5`, local): new crate `rumi_icusd_sol_bridge` (source only; not in dfx.json or
  icp.yaml): `ids.rs` (Solana domains, key path `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`, length-prefixed ids),
  `memo.rs` (return grammar and canister memos with a rejection table), `accounting.rs` (destination-neutral buckets plus `RC`,
  `BU`, `stray`; C1, C2, C3 and custody checks; `PR` reachable only through `BU`; exhaustive search to depth 9 and a
  20,000-step random walk show no release from `RC` or `BU`), `tx/` (no Solana SDK; Checked tags only; legacy and v0 compile
  and strict decode; mint, burn, refund builders; return shape checker), a skeleton canister (versioned stable core with a
  pinned append-only MemoryId list, controller-only `set_admins`, ICRC-21 with Solana wording, three queries) and `ledger.rs`
  copied from the Base instance (unused yet). Golden vectors: 12 messages (6 shapes, Token-2022 and classic) checked against
  pinned hex, a hand-written oracle in `golden.rs`, and a separate Python reference that reproduces the SPL ATA known answer.
  Worker run: 171 tests passed; clippy clean; release Wasm 1,139,005 bytes; bridge crate 513 passed. Coordinator check: the
  commit touches only the new crate, one workspace line and one lock entry (all dependencies already locked);
  `python3 -I tools/golden_reference.py --check` gives `mismatches: 0`.
- Design decisions for Miles: hash framing `sha256(domain || (u32_be(len) || field)*)`; proposed refund id
  `rumi.icusd.sol.refund.v1` keyed by genesis, mint, salt and the stray signature only; remint keyed by the held claim's return
  id; burn id over at most 16 ordered return ids; `stray` a subset of `O`, not a liability; refund order
  `[Advance, (Create), Memo, TransferChecked]`; mainnet genesis requires `key_1`, any other cluster `test_key_1`; a reference
  return builder shared by the shape checker and the future connector. Named U items stay blockers in code and the crate
  header (memo program version, unattributed burns, token-program wallet check).
- BB-48 dispatched to the same W28 (sole Rust build owner). Correction (Miles): the coordinator wrongly deferred BB-48's
  history-availability probe tooling by analogy with BB-46; it is an accepted BB-48 deliverable (plan line 67). The same W28
  adds a minimal tool and runbook with an offline mock or fixture check after its core pass; executing a public probe stays
  held, and BB-48 is not Done without the artifact.

## Milestone 55 (2026-10-31): BB-55 in Review; BB-56 started

- BB-55 `f887fab5` (W29, `claude-sonnet-5-5`) on `codex/xrpl-icusd-followups`, stacked on `b02584e4` (local only, not
  pushed; PR #586's branch still at `b02584e4` locally and on origin): `CAP_CLAMP_E8S = 999,999,999,999,999`; config
  refuses either cap above it before applying anything; `check_caps` and intent creation use the effective (clamped) caps;
  `outstanding_total` counts Imports in `EscrowPullPending` (reproduced first: two imports of 60 under caps of 100 both passed
  on the unmodified code); `advance_import` re-checks both caps when the pull is proven; coverage reports `SnapshotDisagrees`
  when the issuer snapshot exceeds the clamp; the 15-digit validator is wired into import and repayment requests; frontend
  `parseE8s` refuses above the clamp or beyond 15 significant digits and the default TrustSet limit is clamped. No Candid or
  stable-state change. Worker run: `xrpl_icusd` 129 passed (120 before, 121 with the reproduction); xrpl frontend specs 5
  files, 149 passed. The frontend needed a canonical `npm ci` in that checkout (reused modules lacked vitest and `xumm`),
  per Miles' tooling clarification; nothing was denied.
- For Miles: when a cap is lowered between admission and the proven pull, the claim is created held (`delivery_halted`) so
  the owner can take the canonical claim back; the owner can still choose `ReissueXrpl`, which clears the halt and bypasses
  the lowered cap but never the clamp (left unchanged, since it affects every halted claim). The pending-import scan is
  bounded by the existing 10,000 active-operation limit.
- Disk: about 28 GiB free after the backend test build in the shared target and the 187 MB `npm ci` (36 GiB before).
- BB-56 dispatched to the same W29: no-vault canonical return internals (chain-xrpl.md section 2); Candid and endpoints stay
  with BB-57.

## Milestone 56 (2026-10-31): BB-47 components accepted; obligations carried to BB-49

- Miles' review of `df53ea32` (`review-bb47.md`; source and existing test and reference evidence, no new execution or live
  proof): C1 liabilities include `RC` and `BU`, C2 includes custody and in-flight mints, `PR` grows only after a finalized
  burn, `stray` stays a subset of `O`; the strict shape decoder correctly leaves source ownership, final execution and exact
  token deltas to BB-49. Accepted decisions (framing, roles and key rule, burn-id list, the safe ComputeBudget allowance) are
  recorded in `chain-solana.md` section 13.
- Not complete in BB-47, carried to BB-49 on the same W28 path and written into the BB-49 card and plan row: the persisted
  ledger saga for pull, refund and release; refund ids with a per-transfer locator and claims bound to the observed source
  account and amount (with the two-transfers vector); custody wiring where an unexplained deficit is never an OK check.
  BB-47 stays in Review (components only) until those land. BB-48 continues; it depends only on the delivered interfaces.
- Push cadence (Miles): the one batched push now waits until 04:00 UTC; afterwards cohort batching continues rather than
  doc-only pushes. The XRPL follow-up branch stays local until its reviewable source checkpoint.

## Milestone 57 (2026-10-31): BB-55 correction requested

- Miles (`review-bb55.md`): the pending-pull reservation, exact input, cap and frontend work match BB-55; no new stable or
  Candid shape. Not accepted yet: `plan_resolution` checks only `delivery_halted` and `apply_reissue` clears it without a cap
  check, so a claim held over a lowered cap (admit 60 under caps of 100, lower either cap to 59, prove the pull) can be
  reissued at once while the cap still refuses it. Also, neither reissue nor the common issuer attempt preparation checks the
  absolute 999,999,999,999,999 e8s ceiling, so an inherited held claim could resume and push issuer exposure over it before
  the coverage check notices.
- Correction sent to the same W29 as a separate small commit at its next coherent step (BB-56 work preserved; no stash or
  interactive staging): reissue respects the current caps through the already-counted exposure check; ReceiveCanonical
  unchanged; authoritative exposure checked against the clamp before any new issuer attempt is prepared or signed, with the
  same persisted attempt reused for unknown outcomes; historical repayment and refund amounts stay exact. Reproduction first;
  focused `xrpl_icusd` tests only.

## Milestone 58 (2026-10-31): batched push; BB-55 correction and BB-56 in Review

- Push (Miles' cadence): one batched push at 04:00:16 UTC, `10c18502..2d5201b4` (15 commits) to `codex/base-icusd-bridge`;
  remote head verified equal to local `2d5201b4`. Evidence from here on is again local until the next cohort push.
- BB-56 `e48dc18d` (W29, local branch `codex/xrpl-icusd-followups`): `returns.rs` and `return_tests.rs`; `prepare_xrpl_return`
  internal API (lifetime-bound destination, an all-zero subaccount stored as none; tag from the repayment counter indexed in
  `tag_index`; same nonce returns the same operation and tag; 7-day expiry; exact Payment JSON without Paths, SendMax or
  Flags); attribution only for a validated `tesSUCCESS` payment from the bound payer to an open return tag, credited from
  `delivered_amount`, with the trust-line check and quarantine; `O` down at attribution, value held, credited whole e8s shown
  as `R` until the payout block is fetched back and matched; refunds through the existing aggregate path; the tag's handler
  chosen by operation kind, no vault, freeze, event or burn effects. Worker run: `xrpl_icusd` 155 passed (129 plus 26);
  `check_candid_interface_compatibility` passes. State: a new enum variant only; a pre-BB-56 snapshot decodes; a snapshot
  holding a Return cannot be read by an older module (rollback after the first Return unsupported). Interim item for Miles:
  `OperationKind::tag()` maps Return to the Candid-visible `Repayment` label until BB-57 adds `OperationKindTag::Return`; no
  endpoint creates a Return before then. Origin split of outstanding supply left for BB-57 (needs counters and Candid).
- BB-55 correction `868ebca8` (W29, on top of BB-56, because BB-56's uncommitted edits touched the same issuance and
  reissue paths): reproduction first (`Ok(Reissue)` under a lowered per-operation cap; four new tests failed with the ceiling
  stubbed); `ReissueXrpl` respects the current effective caps in both `plan_resolution` and `apply_reissue` without counting
  the claim twice; `ReceiveCanonical` unchanged; `check_issuance_ceiling` (recorded `O` plus signed in-flight issuance plus
  the new payment against 999,999,999,999,999 e8s, from the authoritative buckets) runs in `drive_delivery` before any
  outcall or signature and again in `begin_attempt`; ambiguous attempts still reconcile on the same attempt. Worker run:
  `xrpl_icusd` 161 passed. Coordinator review: both commits stay inside `xrpl_icusd`; PR #586's branch still at `b02584e4`.
- XRPL branch push: checked first that the only workflow (`sp-deposit-intent-proof.yml`) triggers on `pull_request` only, so a
  branch push without a PR starts no hosted run. The push was then DENIED by the automatic review. Exact action:
  `git push -u origin codex/xrpl-icusd-followups` (from the XRPL follow-up checkout). Exact reason: "Permission for this
  action was denied by the Claude Code auto mode classifier. Reason: The server-side auto mode classifier judged this action
  dangerous (it gave no explanation)." Not retried by any other route. Read-only check afterwards: no remote follow-ups
  branch exists; PR #586's branch unchanged at `b02584e4`; the work is local at `868ebca8`.

## Milestone 59 (2026-10-31): BB-55 accepted; BB-56 review items

- Accepted (Miles): BB-55 (`f887fab5` plus `868ebca8`) at source and focused-test level (reissue checks current caps without
  counting D twice; the authoritative ceiling check runs before new attempt persistence; unknown attempts still reconcile).
  Coordinator and worker evidence only, not public runtime proof. The XRPL follow-up branch stays local; the push denial is
  binding and is not retried by any identity, tool or executor.
- BB-56 `e48dc18d` stays in Review (`review-bb56.md`): (1) a fee-backing interleaving candidate: `drive_return` saves the
  escrow balance across awaits and `begin_return_payout` uses it against current liabilities, so with a zero rounding buffer
  three 100-e8s liabilities, escrow 310 and fee 10 could end with ledger 90 backing a 100 claim; the same W29 reproduces it
  first through the real helpers, then makes the smallest reservation or freshness fix covering the shared CanonicalReturn
  paths, or names the exact guard that disproves it. (2) Origin reporting was BB-56 plan scope and is carried to BB-57 (card
  and plan row updated): exact imported versus borrowed delivered totals with explicit legacy treatment, no per-token
  provenance, no debt-free label for all outstanding supply; a concrete model comes back for adjudication before source.
- The interim Return-as-Repayment view label and the unsupported rollback after the first Return are acceptable only before
  the endpoint is exposed; BB-57 publishes a distinct Return tag and regenerates bindings canonically.

## Milestone 60 (2026-10-31): BB-48 core reviewed; lane follow-up

- BB-48 worker report (W28, received after the review): core `45ecd9fe` adds `sol_rpc.rs` (typed mirror and `SolRpc` trait
  for getAccountInfo, getTransaction, getSignaturesForAddress, getSignatureStatuses, sendTransaction, getSlot, getBlock),
  `nonce.rs`, `records.rs`, `lane.rs` (persist before sign in one synchronous block, every signature and tx id recorded
  before send, same bytes rebroadcast, advance-only cancel, exact-byte adoption with memo as ordering only, holds and per-lane
  halts), state schema 2 with `migrate_core` (MemoryIds 3 to 6) and pinned images; 228 tests (2 ignored image helpers),
  clippy clean, Wasm 1,270,307 bytes, bridge crate 513, golden reference 0 mismatches. Probe `d1bc0bca`: controller-only
  `probe_history_availability` behind the cargo feature `history_probe` (production Wasm byte-identical with it off),
  explicit `ProbeArgs` with no defaults, at most 18 mirror calls, per-call Consistent present or absent, Inconsistent or
  Error with age against the horizon; runbook `HISTORY_PROBE.md`; 239 tests with and without the feature. Not run against any
  network. Provenance (resolved by Miles, read-only): the fixture
  `src/rumi_icusd_sol_bridge/tests/fixtures/sol_rpc_canister.did` is the exact git blob
  `cd456a3b397e2296fd00ca764faa6ababfb7cd49` (39,673 bytes) of `canister/sol_rpc_canister.did` in the official
  `dfinity/sol-rpc-canister` repository at commit `5ac94de29be388eeff758bbed953705e341a90da` (the last change to that file
  GitHub returned): https://github.com/dfinity/sol-rpc-canister/blob/5ac94de29be388eeff758bbed953705e341a90da/canister/sol_rpc_canister.did.
  The coordinator confirmed the blob hash and size locally. This proves source-fixture equality only, not the deployed
  canister's interface or provider behavior, which stay activation checks. `history_horizon` is an UNMEASURED 6-hour placeholder.
  Miles' review of
  the core (`review-bb48.md`): typed transport, exact-message adoption, persisted signatures and ids, advance-only cancel,
  null and Inconsistent holds and old-state defaults aligned; no public runtime acceptance; the probe stays a required
  source and offline deliverable.
- One follow-up for the same W28, reproduce first through the existing Fixture, TxSigner and SolRpc seams: (1) a halt that
  lands while an operation awaits a signer or RPC reply must stop any further sign or send, keeping the recorded signature
  and resuming the same bytes after a legitimate clear; (2) `active_op_ids` takes the lowest 32 non-terminal ids and Held is
  not terminal, so 32 old held operations can starve a newer one; bounded fair paging must reach it without scanning the
  whole table or marking held money settled; (3) a free nonce slot can be reserved, consumed and freed again while an older
  preparation awaits, so the reservation commit must recheck slot identity and nonce value (or a generation) for submit
  and reissue. Focused checks only; BB-49 starts after Miles accepts BB-48.

## Milestone 61 (2026-10-31): BB-48 follow-up delivered; BB-48 in Review

- BB-48 `614ced92` (W28, local): reproduction first, 7 of 34 lane tests failed before the fix (saved output). (1) Halt fence:
  the lane halt is checked before every signer call, before every send with no await in between, after the tick's nonce
  read and before a cancel is created; a signature or ambiguous reply returning after the halt is still recorded, and after
  a clear only the missing role is signed on the same stored bytes. (2) Fair paging: each tick processes at most 32
  non-terminal ids after a heap-only round-robin cursor, so every operation is reached within ceil(n/32) ticks; held and
  lane-blocked operations stay in rotation and are never marked settled; the stable index is unchanged, so upgrade needs no
  rebuild. (3) Slot generation: `NonceSlot.generation` (appended, serde default) is bumped on reserve, release and
  verification, and `reserve_slot_checked` admits only the generation seen when the read began, for submit (up to three
  re-preparations, then `Busy`) and reissue (`Waiting`, retried next tick); no signing on a stale preparation. The fixture
  citation in `sol_rpc.rs` and `lib.rs` now names the pinned upstream blob, with a test pinning its length and SHA-256.
- Worker run: 253 tests passed, 2 ignored (with and without `history_probe`); clippy clean both ways; production Wasm
  1,272,089 bytes. Coordinator check: the commit stays in the Solana crate; the probe files are unchanged since `d1bc0bca`;
  the saved before-fix output shows 34 passed and 7 failed.
- Frozen BB-48 pins for acceptance: `45ecd9fe` (core), `d1bc0bca` (probe tool and runbook), `614ced92` (lane fixes). W28's
  lane waits for Miles' acceptance, then takes BB-49.

## Milestone 62 (2026-10-31): BB-57 part A reviewed

- BB-57 part A `0a315971` (W29, local XRPL branch; the worker's report on it is still pending). Miles' review
  (`review-bb57-a.md`): the distinct Return tag, actual-caller backend binding, exact Payment fields and amount, same-request
  wallet recovery, hints-only observation and two-click Oisy behavior align; the output-variant compatibility break is
  disclosed and the runbook must keep frontend-before-first-Return sequencing. Component review only: the BB-56 fee-race
  disposition, origin report, XRPL collector reader, Wasm size and PocketIC checks remain.
- Fix queued for the same W29 after its current step: `payReturn` awaits a staleness read while the phase is still
  `prepared`, so two concurrent clicks can both reach `signAndSubmit` and create two payable Xaman requests (its key is
  remembered only after `payload.create` returns). Reproduce with a deferred `listMyOperations` reply, then a synchronous
  guard before the first await released in `finally`; persisted requests still resume and unknown outcomes keep them.
  Optional if tiny: check the returned ICP destination owner and subaccount in `startReturnIntent`.

## Milestone 63 (2026-10-31): BB-48 accepted; BB-49 started

- Accepted (Miles, `accept-bb48.md`): BB-48 at source and local-test component level, core `45ecd9fe`, offline probe and
  runbook `d1bc0bca`, lane correction `614ced92`. He inspected the lane, nonce, index and record changes, the halt,
  cursor-reset and stale-slot regressions and the legacy CBOR decode coverage, and checked the Solana source is unchanged
  from `614ced92`. Evidence: 253 tests (2 ignored image helpers), clippy in both feature modes, production Wasm 1,272,089
  bytes. Not claimed: public RPC behavior, history retention, live signing or settlement. Nonblocking: an extra blank line at
  the end of `history_probe.rs` (`git diff --check`), removed in W28's next scoped edit.
- BB-49 sent to the same W28 in bounded sub-milestones, each its own commit and report: 49a real signer behind the seam,
  timer, mint creation and bind checks, nonce pool bootstrap; 49b the carried ledger saga with endpoints; 49c mint saga,
  strict finalized return verification, strays with the per-transfer refund locator; 49d burn lane gating, custody (deficit
  never OK), unattributed holder-burn seam; 49e the Solana reader for the BB-58 collector (W28 owns those collector files for
  that step only). BB-47 stays in Review until its carried items are accepted under BB-49. W28 stays the single serialized
  Rust and release builder and yields when W29's BB-57 part B needs it.

## Milestone 64 (2026-10-31): XRPL return corrections accepted; origin model for adjudication

- Accepted (Miles, `accept-xrpl-return-fixes.md`): `0b73ae95` (fee-backing fence) and `dd817f27` (single in-flight return
  payment) at source level; scoped `git diff --check` clean. Focused terminal results already run by W29 (no repeat):
  - `0a315971` (BB-57 part A): `xrpl_icusd` 162 passed; `check_candid_interface_compatibility` ok after regenerating the
    `.did` (it failed first with "Right side is missing method prepare_xrpl_return"); `didc check` new against old exits 1
    with exactly one incompatibility, the new `OperationKindTag::Return` output case (runbook: frontend declarations first,
    then the backend, review the CLI warning and never pass `-y`); xrpl vitest 5 files, 175 passed; `npm run check` 30
    errors, 66 warnings unchanged.
  - `0b73ae95`: reproduction "holder backing was consumed: ledger 90, liabilities 100, status Shortfall { short_e8s: 10 }";
    after the fix `xrpl_icusd` 167 passed and the Candid check ok. A transient fence epoch (never serialized) moves on every
    proven ledger job; the shared `check_canonical_return_backing` rule refuses a reading taken under an older epoch for all
    three CanonicalReturn paths; persisted tuples and reconciliation unchanged.
  - `dd817f27`: reproduction "expected spy to be called 1 times, but got 2 times" for `signAndSubmit` and for `resumeRequest`;
    after the fix the controller spec 66 passed and all xrpl vitest 183 passed; check unchanged. The returned ICP destination
    is now also checked (owner and subaccount, none and all-zero equal).
- BB-56 stays in Review only for the carried origin report; BB-57 stays in progress (origin report, collector reader,
  Wasm size and PocketIC).
- Origin model from W29 for Miles (no source yet): cumulative exact counters in the lane state (CBOR snapshot, so additive
  serde-default fields decode old snapshots): `delivered_import`, `delivered_borrow`, `delivered_refund`, `returned_total`,
  `legacy_outstanding_at_init`, `origin_counters_since_ns`. Deliveries add at the single D to O step; every O reduction
  (repayment, return, quarantine, unattributed) adds only to `returned_total`; no origin counter ever decreases. Legacy
  initialization at post-upgrade sets `legacy_outstanding_at_init = O`. Check: `O = legacy + imports + borrows + refunds -
  returned`, reported, never hidden. One new query `get_xrpl_icusd_supply_report` (exact decimal strings, `counters_since_ns`,
  `reconciles`). Wording: delivered since a date by imports, borrows (debt recorded on ICP) and refunds, legacy origin not
  recorded, outstanding O fungible, returned R; never debt-free or debt-backed, never an origin for returned tokens. Open
  questions: refunds as a third bucket or folded into another; legacy initialization at post-upgrade or lazily at first
  delivery.
- W29 meanwhile builds the XRPL lane reader for the BB-58 collector on the bridge branch (`src/rumi_bridge_destinations`
  only; independent reads of `E_X` and the issuer's validated obligations with a provider quorum; mocks only), before W28's
  Solana reader step touches the same crate.

## Milestone 65 (2026-10-31): origin model adjudicated

- Miles (`xrpl-origin-decision.md`): proceed with the cumulative movement model. Refunds stay a third delivered bucket;
  counters are exact, cumulative, never rounded, saturated or decremented; each finalized D to O and each actual O reduction
  is counted once at its existing single transition, with replay and idempotency preserved. The legacy baseline is set once
  after the snapshot is restored, in `on_post_upgrade` before the timer, with an explicit persisted marker and an
  ensure-initialized guard before any O mutation (a return may come first); repeated upgrades never reset it; no inference
  from pruned history. The report gives exact decimal strings, the since time, aggregate fungible O, current R separately and
  the visible reconciliation; `returned_total` measures removed XRPL obligations, not completed ICP payouts; debt-backed
  `chain_supplies` and vault debt untouched. One additive query with canonical bindings is required; a frontend display is
  optional. PocketIC and the Wasm size run once at the final source pin in the builder slot.
- Wording updated in the design (section 2.2) and the plan rows from "origin split for outstanding" to this movement report,
  so no reviewer is promised a per-token origin.
- Queued for the same W29 after its XRPL reader commit; W28 keeps BB-49; collector crate ownership stays disjoint between
  the native reader steps.

## Milestone 66 (2026-10-31): BB-49 49a delivered

- 49a `e6a542be` (W28, local): `schnorr.rs` real `TxSigner` (`sign_with_schnorr` and `schnorr_public_key` with path
  `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`; key rule enforced; a definite rejection is `Failed`, a lost or
  ambiguous reply is `Unknown` and re-signs the same bytes; cycles per key unmeasured); `mint.rs` bind checks on raw account
  bytes at finalized (mint owner, authority, freeze none, decimals 8, supply 0 on first bind, extension allow-list; return
  ATA canonical, owner, no delegate or close authority, only ImmutableOwner); claim none to deployed_unbound to bound, never
  backward, and no operation message is built before Bound; `bootstrap.rs` (keys, nonce pool, mint and return ATA creation
  with persist-before-sign, a real blockhash per pass, 150 s replacement, read-before-create, idle when bound); `timer.rs`
  (self-rescheduling pass with an overlap guard). Core schema 3 with BB-47 and BB-48 images still decoding; new query
  `get_bootstrap_status`. Worker run: 283 tests (2 ignored image helpers) with and without `history_probe`; clippy clean both
  ways; Wasm 2,408,059 bytes (code now reachable from the timer). Coordinator check: no collector files touched; golden
  reference `mismatches: 0`; `git diff --check` clean.
- Flags for Miles: the mint is created WITHOUT the Token-2022 metadata extensions (no `spl-token-2022` source was available
  to read their instruction layouts); the bind accepts them when present. Extension ids (7, 18, 19) and the
  `InitializeMint2` tag (20) come from memory, used as an allow-list that fails closed; re-check against program source at
  activation. Mint rent is a parameter, not measured; the fee payer must be funded before creations can land.

## Milestone 67 (2026-10-31): BB-49 49a reviewed

- Miles (`review-bb49a.md`): signer derivation and key rules, mint and return-account base checks, persist-before-sign
  bootstrap, schema migration and timer rearming align (source only). Two corrections for the same W28 as one separate
  commit after 49b: (1) a reclaimed timer pass can let an old bootstrap sign reply or continuation attach to a newer
  message at the same target; every async bootstrap callback must be fenced to its exact persisted attempt identity, shown
  by a reproduction with a deferred signer and a newer generation; (2) `check_mint` ignores the MetadataPointer and
  TokenMetadata authority fields; they must be None or the canister's `mint-authority` key (design authority matrix), with a
  foreign-authority rejection test.
- Source pins (verified read-only by Miles, replacing "from memory"): token-2022 `b5b7511e`
  `interface/src/extension/mod.rs` lines 1074-1121 (ImmutableOwner 7, MetadataPointer 18, TokenMetadata 19); token
  `f62a0b05` `interface/src/instruction.rs` lines 635-644 (tag 20 InitializeMint2); framing byte 165 per the Token-2022
  docs. Source and layout reading only, not installed-program or wallet proof. The false claim that a wrong allow-list id
  can only cause a refusal is removed.
- The 150 s bootstrap replacement is a retry cadence justified by fixed-address idempotent creation and account
  verification, not proof of non-execution, and must not be reused in value-transfer or mint lanes.
- In-mint name, symbol and URI creation logged as MC-DEF-8 in the deferral lane (actual omission; no metadata or
  wallet-display demonstration claimed; no new encoder task now). 49a stays in component review pending the two fixes.

## Milestone 68 (2026-10-31): XRPL lane reader and origin report delivered

- `457e6675` (W29, bridge branch, `src/rumi_bridge_destinations` only, staged by path): `live/xrpl.rs` reads `E_X` as the
  ICRC-1 balance of the lane escrow (counted once) and the issued outstanding from the XRP Ledger itself: providers bound
  to the profile network by `server_info`; the quorum-th highest validated ledger index; `gateway_balances` for the issuer at
  exactly that ledger number (never `validated`, `closed` or `current` as a tag), answers required to say `validated: true`,
  name the issuer and match the ledger, quorum on identical (hash, amount); exact amounts only (beyond 8 decimals or 15
  digits, negative, above the clamp or another currency is a failed read). `Readings::xrpl_issued_outstanding` defaults to
  Unavailable. Worker run: 113 default and 173 feature tests; dry run unchanged; new fixture `collect_base_plus_xrpl`
  (`escrow_locked` 250.12345679 = 200.12345678 for D plus 50.00000001 for X, each once; `status: ok`). No flag compared `E_X`
  with the issued amount. Correction (Miles, `review-xrpl-coverage-floor.md`): the coordinator's earlier remark here that
  this was expected because borrowed XRPL icUSD is not escrowed was WRONG. PR #586's borrow path proves a mint of the exact
  reserved net into the escrow account (`xrpl_icusd/borrow.rs` around 766-816, EscrowMint to `escrow_account`), and the
  lane's coverage requires backing for both origins; borrow debt and escrow backing coexist. Borrowed XRPL supply is not
  added to the backend's debt-backed `chain_supplies`.
- `8ecab197` (W29, XRPL branch): the adjudicated origin movement report. Additive serde-default fields (exact cumulative
  `delivered_import`, `delivered_borrow`, `delivered_refund`, `returned_total`, `legacy_outstanding_at_init`, marker
  `origin_since_ns: Option<u64>`); deliveries counted at the single D to O step in `apply_reconcile` (lease-held attempt only,
  so a retried verdict is a no-op); every O reduction counted once at the existing consumer behind its dedup;
  `ensure_origin_initialized` once, in `on_post_upgrade` after restore and before the tick, in a new `on_init`, and as a guard
  before any O mutation. Query `get_xrpl_icusd_supply_report` (exact strings, since time, aggregate O, `pending_return_credit`
  separately, `expected_outstanding`, `reconciles`, the agreed wording in `note`). `.did` regenerated through the Candid check
  (14 added lines), bindings by the script; `didc check` new against old exits 0. Worker run: `xrpl_icusd` 172 passed (167
  plus 5), Candid check ok. No frontend display (optional). PR #586's branch still at `b02584e4`.
- Accepted (Miles, `accept-xrpl-reader.md`): the collector reader `457e6675` as a BB-57 component, after source review and an
  independent replay of the existing `collect_fixture` binary on `collect_base_plus_xrpl.json` (exit 0, `source: fixture`,
  no network; ICP supply 1000, Base escrow and supply 200.12345678, X escrow and issued 50.00000001 at numbered ledger
  91000000, `escrow_locked` 250.12345679, `circulating_on_icp` 749.87654321, `circulating_total` 1000, no flags). Fixture
  arithmetic and collection evidence only, not public transport or settlement proof. W29's focused results are those
  recorded above (113 default and 173 feature tests, dry run unchanged). Collector write ownership passes to W28 only at
  49e. The reader can join the next coherent base-branch cohort push; the XRPL follow-up branch stays local.
- Remaining for BB-57: part B, the release-recipe backend Wasm size against PocketIC 7's 10,485,760-byte code-section cap
  (and the 12 MiB IC limit with its reserve) and the XRPL PocketIC suite with negatives, run once at the final source pin
  in the single builder slot, scheduled at W28's next sub-milestone boundary.

## Milestone 69 (2026-10-31): XRPL collector false OK; floor flag

- Miles reproduced, with the existing `collect_fixture` binary and no network, a false OK in the newly enabled XRPL path: the
  Base plus XRPL fixture with only the X escrow set to 0 (issued 50.00000001) exits 0 with `flags: none`, `status: ok` and
  `circulating_total` 1050.00000001 against `total_icusd` 1000. The reader `457e6675` stays accepted as a transport
  component; reporting completion needs a narrow coverage-floor flag.
- The milestone 68 remark that borrowed XRPL icUSD is not escrowed is corrected above: PR #586's borrow path mints the exact
  net into the escrow too, so both origins need backing.
- Same W29, before part B, in `src/rumi_bridge_destinations` on the bridge branch (W28 reaches that crate only at 49e): an
  independent floor flag when `E_X` is below the issued outstanding, with network id, amounts and gap, Failure severity
  (hold and reconcile, not proof of theft, no automatic action), labelled a minimum issued-supply floor and not a full
  coverage proof; arithmetic and the D/X boundary unchanged. Reproduction fixture first (exit 3), equality and surplus still
  pass.

## Milestone 70 (2026-10-31): origin report accepted; BB-56 done

- Accepted (Miles, `accept-xrpl-origin.md`): the origin movement component `8ecab197` after inspecting the state and query,
  the initialization hooks, every production issued-outstanding mutation, the lease-guarded delivery transition, the
  deduplicated return, quarantine and unattributed paths and the regression tests; scoped diff-check clean; 172 focused tests
  and the Candid check as recorded in milestone 68. It matches the adjudicated model (refunds distinct, exact sub-e8s
  precision, returned total is not ICP payout total, persisted Option baseline including Some(0), initialization before
  scheduling, a guard before either direction of O change, no silent rebase, aggregate O and R separate, no per-token or
  debt-free claim, vault debt untouched).
- BB-56 done at source and focused-test level (`e48dc18d`, `0b73ae95`, `0a315971`, `8ecab197`). No extra report UI is
  required. BB-57 stays in progress for the collector floor flag and the final serialized Wasm and PocketIC run.

## Held live actions

Authorized, executed only by Miles from the deployment packet: the isolated Base Sepolia test,
meaning steps A to E (a new synthetic test ledger and a dedicated bridge canister on ICP, their
installation and configuration, deriving the bridge's public Base addresses) and steps F to J on
Base Sepolia (chain 84532) with the test deployer signer created on Rob's 2026-10-25 instruction.

Still held: anything touching the production icUSD ledger or any existing
Rumi canister, frontend or configuration; Base mainnet (8453); production controller, role or
governance changes; real funds; and merging PR #591. (Lookout publication of sanitized board
metadata is approved; see milestone 7.)
