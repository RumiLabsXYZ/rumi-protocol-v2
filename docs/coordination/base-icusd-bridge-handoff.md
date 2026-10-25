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

## Held live actions

Authorized, executed only by Miles from the deployment packet: the isolated Base Sepolia test,
meaning steps A to E (a new synthetic test ledger and a dedicated bridge canister on ICP, their
installation and configuration, deriving the bridge's public Base addresses) and steps F to J on
Base Sepolia (chain 84532) with the test deployer signer created on Rob's 2026-10-25 instruction.

Still held: anything touching the production icUSD ledger or any existing
Rumi canister, frontend or configuration; Base mainnet (8453); production controller, role or
governance changes; real funds; and merging PR #591. (Lookout publication of sanitized board
metadata is approved; see milestone 7.)
