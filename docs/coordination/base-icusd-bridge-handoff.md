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

## Held live actions

Authorized, executed only by Miles from the deployment packet: the isolated Base Sepolia test,
meaning steps A to E (a new synthetic test ledger and a dedicated bridge canister on ICP, their
installation and configuration, deriving the bridge's public Base addresses) and steps F to J on
Base Sepolia (chain 84532) with the test deployer signer created on Rob's 2026-10-25 instruction.

Still held: anything touching the production icUSD ledger or any existing
Rumi canister, frontend or configuration; Base mainnet (8453); production controller, role or
governance changes; real funds; and merging PR #591. (Lookout publication of sanitized board
metadata is approved; see milestone 7.)
