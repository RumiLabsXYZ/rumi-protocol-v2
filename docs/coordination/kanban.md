# Conflux Public Launch Kanban

Updated: 2026-09-13

## Active

| Card | State | Owner | Requested / actual model | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- | --- |
| CFX-10 | Done | Terra design reviewer + coordinator | Terra / GPT-5.6 Terra | Independent critique of Conflux branding, Rumi branding, current borrow page, and refined two-column mockup. Read-only product review; no implementation, deployment, or wallet actions. | All four inputs inspected. Coordinator reviewed the report and verified current layout and Conflux lifecycle against source. Visual improvement is not proof that swapped columns improve usability. |
| CFX-05 | Review | Luna component worker + coordinator | Sonnet / GPT-5.6 Luna fallback | Accepted stats-left, form-right Conflux design implemented. Worker owned BorrowWorkspace.svelte; coordinator integrated public data, assets, styles and controller-preserving markup. | 57 tests in each of three modes; typecheck and public artifact policy17files pass. Browser checks cover desktop/tablet/mobile, composition, navigation, wallet-free controls and unavailable states. Local preview running; source not merged/deployed. See frontend design-qa.md. |
| CFX-11 | Done | Two isolated Sonnet reviewers + coordinator | Sonnet / claude-sonnet-5, firstParty | Two-round independent adversarial source review. | Round one font-hosting provenance concern resolved using licensed Inter; homepage untouched. Both fresh round-two reviewers PASS. Stale ignored build regenerated without Circular. See frontend source-review.md. No wallet signatures or live writes. |

### CFX-10 details

- Requested explicitly by Rob; Terra overrides the usual Sonnet-first model preference for this card.
- Worktree: `/Users/robertripley/.codex/worktrees/aa1b/rumi-protocol-v2`, existing branch `codex/conflux-public-ui-launch`; no new worktree.
- Inputs: official Conflux Figma guide, `rumiprotocol.com/branding`, `app.rumiprotocol.com`, and refined mockup `exec-06b265a6-9865-4a03-ba76-6ff4cc3b8b86.png`.
- Output: evidence-backed critique returned to coordinator. Coordinator owns board updates and final recommendations.
- Evidence: `/Users/robertripley/.codex/visualizations/2026/09/13/01a0996f-9772-7160-a8e2-53af92b85dbe/rumi-conflux-directions/terra-review.md`.
- Review disposition: accepted the brand, scale, repeated-summary, visible-protocol-context, and responsive recommendations. Corrected the initial stepper objection using the actual Conflux Open/deposit/observed-mint source, rather than the ICP compound operation. Terra prefers a compact side preview; coordinator favors retaining stats left and the transaction plus risk below on the right for this two-field page. Both are design judgments, not usability-test findings.
- Limits: no wallet connection, transaction, production change, or responsive functional test. Only the coordination record and local review artifact changed.
- Dependencies: none. Blocker: none.

## Blocked

| Card | Owner | Requested / actual model | Scope and file ownership | Blocker | Depends on |
| --- | --- | --- | --- | --- | --- |
| CFX-06 | Sonnet reviewers + coordinator | Sonnet / claude-sonnet-5 | Browser and wallet-path QA, production build, custom-domain code readiness. | Local UI/build/source checks complete; connected-wallet signing and real mint round-trip explicitly skipped at user request. Deploy-origin exact artifact/custom-domain verification remains a separate release step. | CFX-05 |
| CFX-07 | coordinator | GPT-5 / GPT-5 | Merge, exact-head backend deploy, $1,800 config mutation, price/proof refresh, monitor, activation, and post-activation verification. | Source gates are green. Awaiting merge, exact-head rebuild, snapshot, artifact proof, and live readbacks. Frontend redesign is not a backend activation gate. | CFX-01, CFX-03 |
| CFX-08 | coordinator | GPT-5 / GPT-5 | Deploy the exact approved frontend and complete custom-domain registration. | Requires CFX-06; Cloudflare SSL impact must be resolved safely. | CFX-06 |

## Done

| Card | Owner | Requested / actual model | Evidence |
| --- | --- | --- | --- |
| CFX-04 | coordinator | Coordinator | User accepted the revised stats-left/form-right mockup with Collateral composition collapsed by default. Exact image exec-fc519458-1fec-44eb-bc74-09aa33454270.png; design accepted, no further selection required. |
| CFX-00 | coordinator | GPT-5 / GPT-5 | Live chain state, endpoint digest, vault/supply zero-state, pool depth, three-source CFX price, controller/module status, and disabled-readiness blockers captured. Price was refreshed to 4,784,000 e8; Conflux remained disabled. |
| CFX-01 | Sonnet worker + coordinator | Sonnet / Sonnet 5 | The compiled public-readiness ceiling, coupled production fixture, and current launch row now use 1,800e8. Old/new config digests were reproduced; `cargo check -p rumi_protocol_backend --lib` and `git diff --check` passed. Coordinator restored consumed recovery scripts and historical evidence unchanged. |
| CFX-02 | Sonnet worker | Sonnet / Sonnet 5 | Read-only control/brand audit completed with file-line evidence, QA matrix, wallet-owner test boundary, and complete Rumi visual brief. |
| CFX-03 | Luna fallback reviewer + coordinator | Sonnet / Sonnet 5 aborted; Luna fallback | Read-only release-delta audit completed after Sonnet's unused Playwright MCP child hung. Luna correctly required a fresh artifact, snapshot, exact module hashes, stopped upgrade, and post-upgrade readbacks. Coordinator rejected rewriting consumed historical recovery artifacts: they truthfully record the earlier $2,000 operation and are not inputs to this launch. Current live source lineage was matched to `28ecfe8b`, whose backend source/tests/dependency inputs match `b155976c` except the intended ceiling change. |
| CFX-09 | Sonnet reviewer | Sonnet / Sonnet 5 | Independent no-MCP review passed. It confirmed the narrow change and fail-closed transition, and requested the explicit old-$2,000 rejection case now covered by the 11/11 focused readiness suite. |

## Coordination rules

- Sonnet workers run with repository shell, git, search, read, and file-write tools when their card permits writes.
- File ownership is exclusive while a card is active. Read-only reviews may overlap.
- A fallback to Luna must be recorded here with the Sonnet failure reason before work starts.
- Only the coordinator may perform merges, canister upgrades, DNS mutations, config mutations, activation, or fund-moving tests.
- Production readiness is an exact-head and live-state claim, not a build-only claim.

---

# Cycle Sentinel implementation board

Last updated: 2026-09-14

Branch: `codex/cycle-sentinel-telemetry`

Spec: `docs/superpowers/specs/2026-09-13-cycle-sentinel-telemetry-design.md`

Plan: `docs/superpowers/plans/2026-09-13-cycle-sentinel-telemetry.md`

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| CS-00 | Done | Codex coordinator | Lock executable plan and interfaces | None | Fresh Sonnet review PASS; coordinator corrected proxy, execution, and card-ID issues |
| CS-01 | Done | Sonnet backend mapper | Map canister, state, Candid, and test integration | None | Accepted `/private/tmp/cycle-sentinel-backend-map.md` |
| CS-02 | Done | Sonnet frontend mapper | Map both telemetry surfaces | None | Accepted `/private/tmp/cycle-sentinel-frontend-map.md` |
| CS-03 | Done | Sonnet reference mapper | Audit delegated-vault reuse and failure gaps | None | Accepted `/private/tmp/cycle-sentinel-reference-map.md` |
| CS-10 | Done | Sonnet implementer + Sonnet/Luna reviewers + Codex | Stable domain, registry, governance, and alarms | CS-00 | 409 tests, host check, release Wasm, fmt, and diff validation passed. Initial Sonnet reviews found and corrected pre-staged unpause, cap-revalidation, and active-alarm eviction flaws. After Sonnet hit its authenticated session cap, two user-approved Luna fallback closeouts independently returned PASS. |
| CS-11 | Done | Luna fallback implementer + Luna reviewers + Codex | Observation, history, and public API | CS-10 | 433 tests, host check, release Wasm, fmt, and diff validation passed. Independent review found and corrected eviction-unsafe pagination, incomplete burn-credit coverage, public operation-ID leakage, exact-Nat overflow, and a same-second retention edge. Two final closeouts returned PASS. |
| CS-12 | Done | Sonnet implementer + Luna fallback implementer/reviewers + Codex | Both funding rails, recovery, and reconciliation | CS-10, CS-11 | Cycles Ledger withdrawal, protected self-recovery, ICRC-1/CMC fallback, exact retry/quarantine, authoritative proof/refund reconciliation, single-flight guards, and source reservations accepted. Adversarial corrections closed unsafe Cycles `Duplicate` semantics, cache races, actual-cycle cap accounting, refund association, attempt exhaustion, and lifecycle coverage. Final dual Task 5 closeout passed with 573 tests including 14 reconciliation tests, scoped fmt, host check, release Wasm, and diff validation. |
| CS-13 | Done | Codex + Sonnet/Luna reviewers | Timers, Candid, config, declarations, and PocketIC | CS-10 through CS-12 | Independent review passed; exact production Wasm and focused live integration evidence recorded |
| CS-20 | Done | Codex + Sonnet/Luna review fallback | Public root-domain telemetry UI | CS-13 | 9 tests and production build passed; injected canister ID verified; independent review passed after recent-top-up and mobile-navigation corrections |
| CS-21 | Done | Luna fallback implementer + Sonnet reviewer + Codex | Authenticated operator telemetry UI | CS-13 | 607 frontend tests and production build passed; 28-error type-check baseline unchanged; authenticated Sonnet closeout PASS |
| CS-30 | Done | Codex plus Sonnet and Luna fallback reviewers | Deterministic and adversarial source gate | CS-10 through CS-21 | Strict Clippy/fmt, 593 Rust tests, 4 official-ledger tests, 20 PocketIC tests, 616 frontend tests, declaration regeneration, root production build, fresh Wasm hash/endpoint scan, and diff check passed. Sonnet security review PASS; independent Luna release review found no source blocker. |
| CS-40 | Done | Codex coordinator | Push, PR, terminal exact-head CI, and merge | CS-30 | PR #390 and deployed-ID follow-up PR #391 merged. GitHub has no configured PR checks; exact local source gate and independent reviews are recorded in CS-30. |
| CS-50 | Done | Codex live operator | Create, deploy, and verify Sentinel | CS-40 | Sentinel `joh3a-5aaaa-aaaap-quy6a-cai` installed and running with the four approved controllers; anonymous overview reports 16 targets. Both published telemetry routes return HTTP 200. |
| CS-54 | Done | Codex + Luna fallback reviewers | Interoperate with wallet ICRC-21 consent preflight without auto-opening operator auth | CS-50 | PR #393 merged; Sentinel upgraded to `0xf16735b24985af403d54c54ca0c87cdd9e06046bb4afed16c4e7a47d7bad403e`, ICRC-10 advertises ICRC-21, and an anonymous `FieldsDisplay` consent update returns the bounded read-only message. Frontend assets synced and live Brave verification shows `Access not checked` plus the explicit check button, without a raw replica error. No target, policy, signer, controller, or funding state changed. |
| CS-51 | Blocked | Codex live operator | Add verified blackhole as an additional controller | CS-50 | One target at a time with before/after controller proof |
| CS-52 | Blocked | Codex live operator | Fund, shadow, and activate policies incrementally | CS-50, CS-51 | Requires an authoritative policy manifest and a second 2-of-4 signer. All 16 initial targets remain disabled with auto-top-up off. |
| CS-53 | Blocked | Codex live operator | Remove CycleOps after replacement proof | CS-52 | Terminal cutover gate |

## CS-00 details

- Scope: approved design, implementation plan, shared interfaces, task ordering, and board transitions only.
- Worktree/branch: `/Users/robertripley/.codex/worktrees/455c/rumi-protocol-v2`, `codex/cycle-sentinel-telemetry`.
- Acceptance: every approved requirement maps to a task, shared files have sequential ownership, and exact source, merge, and live gates are explicit.
- Owner/harness: Codex coordinator using three authenticated Sonnet repository mappers plus a fresh Sonnet plan reviewer.
- Review evidence: Sonnet PASS at `/private/tmp/cycle-sentinel-plan-review.md`; coordinator separately rejected the earlier unnecessary new-proxy design, preserved distinct proposal approval/execution, and corrected board IDs.

## CS-10 details

- Completed scope: implementation-plan Tasks 1 and 2, including the canister scaffold, versioned stable domain model, dynamic registry, multisig/timelock governance, immediate pause, governed unpause, and bounded alarm lifecycle.
- File ownership: `src/rumi_cycle_sentinel/Cargo.toml`, `src/rumi_cycle_sentinel/src/{lib,types,state,governance}.rs`, root `Cargo.toml`, and `Cargo.lock` only when dependency resolution requires it.
- Acceptance: disabled defaults, strict init signer validation, reserved-principal and bound checks, versioned stable round trips, distinct memory IDs, signer authorization, threshold/timelock execution, current-state revalidation, exact-once proposal execution, safe signer removal, alarm deduplication/acknowledgement/resolution, focused tests, host check, and Wasm build.
- Worktree/branch: `/Users/robertripley/.codex/worktrees/455c/rumi-protocol-v2`, `codex/cycle-sentinel-telemetry`.
- Review evidence: Task 1 accepted after 343 tests and two fresh Sonnet closeout PASS verdicts. Task 2 accepted after 409 tests and independent governance/alarm closeouts; Sonnet found three blockers that were corrected, then its session cap required the recorded Luna fallback for the final two PASS verdicts. Reports are under `.superpowers/sdd/2026-09-13-cycle-sentinel-telemetry/`.

## CS-11 details

- Completed scope: verified self-report and pinned-blackhole observation, six distinct public health states, bounded hourly sample history, top-up-corrected burn accounting, anomaly pause/alarm behavior, and cached anonymous telemetry queries.
- Safety properties: failed reads never become zero or healthy; target advisory policy cannot override the governed threshold; stale registry results are discarded after await; unknown or retention-gapped funding intervals produce indeterminate burn; public rows omit signer, proposal, ledger, reconciliation, and operation-ID internals.
- Acceptance: 433 tests, host check, release Wasm build, scoped formatting, and diff validation passed. The initial observation review passed; the public review rejected four correctness/privacy defects, which were corrected. A fresh full closeout passed, and the specialist closeout passed after tightening the same-second eviction boundary.
- Fallback record: Sonnet's authenticated CLI remained session-capped until 7:10 a.m.; the explicitly approved Luna fallback implemented and reviewed this bounded card. Reports are under `.superpowers/sdd/2026-09-13-cycle-sentinel-telemetry/`.

## Cycle Sentinel coordination rules

- Only Ready cards may be dispatched. One Sonnet implementer owns each sequential card and a separate Sonnet reviewer assesses its evidence.
- Sonnet has task-relevant repository read/write access but no secrets, merge authority, wallet authority, or live-action authority.
- Codex owns architecture, accepted findings, shared-file integration, exact-head verification, merge, controller changes, funding, activation, and cutover.
- Source tests, a pushed branch, a PR, a merge, a deployment, funding, controller changes, activation, and CycleOps removal are distinct states.
- The existing `rumi_treasury` source, Candid, controller configuration, and `/treasury` behavior remain unchanged.


# Cycle Sentinel automatic funding release — 2026-09-28

Worktree: `/Users/robertripley/.codex/worktrees/sentinel-funding-live/rumi-protocol-v2`; branch: `codex/sentinel-auto-funding-live`.
Authorization: Rob requested implementation, subagents, deployment, and activation using existing approved target policies. No external-wallet deposit amount was supplied.

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| CS-60 | Done | backend worker | Durable ICP reserve refill, cycles-first funding, runtime recovery, public funding status | Approved policy at 51f4a20e | Frozen production source1ff42252:608 lib and full36 integration tests pass, including16 new real-timer flows; exact production artifact verified |
| CS-61 | Done | frontend worker | Obvious deposit destinations, balances, funding status, readable target rules | CS-60 interface | Focused tests and full production asset recipe pass; built desktop/mobile proof and both live receiving-address copy buttons verified; served page decodes upgraded backend |
| CS-62 | Done | operations investigator | Verify current live policy, signers, mappings, deploy recipe and activation path | None | /private/tmp/sentinel-live-before.txt; /private/tmp/sentinel-release-prep.txt; official PocketIC server7.0.0 matches pinned crate6.0.0; managed local server used |
| CS-63 | Done | independent reviewers | Security, stable-state, interface and product review | CS-60, CS-61 | Full36 integration suite and608 backend tests passed; two fresh isolated final reviewers returned PASS; readable reports committed with release evidence |
| CS-64 | Done | coordinator | Merge, deploy, sync assets, activate and verify live | CS-63 | PR401 merged4f667585; Sentinel upgrade53647exit0/hashdf085a63 verified;645assets sync8071exit0; exact controllers/governance/rules preserved; live cards/copy buttons verified; wallet empty awaiting user deposit |

Model selection: Sonnet CLI reports loggedIn=false. Native Terra is selected for CS-60 due to durable token conversion, stable-state, async retries and ICP-only runtime recovery complexity. Native Luna is selected for bounded frontend and operations work. Coordinator owns generated interfaces and release.

Acceptance: deposited cycles are used first; ICP supports an empty cycles reserve and runtime recovery; configured target refill amounts stay exact; caps, cooldown, protected reserves and one unresolved operation remain enforced; unknown or duplicate payments never cause a second debit; state survives upgrades; deposit destinations are derived from the configured Sentinel principal and default account; the live page distinguishes runtime fuel from funding-wallet money.

Reserve conversion approval artifact: `docs/coordination/sentinel-auto-funding-reserve-policy-2026-09-28.md`, pushed in commit `51f4a20e`, SHA-256 `a984e3c3f2895cc2f67b84ea18306db31febf86fabff61ba4920801fc4a04d0c`. User explicitly approved this policy in the following message: "approved". Implementation merged and deployed with authoritative and browser proof; no external-wallet deposit performed.

Verified default ICP address correction: SDK and independent SHA-224/CRC-32 derivations agree on `a907060d486046ae0a21bcca2a4a2cbd8c48a9c4a7ab31ffbd06a4b5a20a1e2e`; earlier `1253387a...` draft was incorrect and is not a deposit destination.

Approval accepted 2026-09-28: preserve policy artifact at commit `51f4a20e` unchanged, including its SHA-256. Draft implementation PR: https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/401 at WIP `928833c8`. Current disk-pressure helper run: 13 registered worktrees, 6% free, 0 eligible and 0 removed; protected/dirty/unmerged worktrees preserved.


# Cycle Sentinel four-hour checks — 2026-09-28

Worktree: `/Users/robertripley/.codex/worktrees/sentinel-funding-live/rumi-protocol-v2`; branch: `codex/sentinel-four-hour-checks`.
Authorization: Rob requested checks every four hours with a manual refresh button; prior publication and activation approval persists.

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| CS-65 | Done | sentinel_backend | Signer-only immediate maintenance, shared concurrency guard, actual scheduled deadline and cadence fields | None | Existing Terra worker retained for integration of the previously reviewed asynchronous maintenance path; no stable schema or funding-rail changes |
| CS-66 | Review | cadence_frontend | Obvious active cadence and signer-only Run check now; preserve public read-only refresh | CS-65 interface | Source and assets are live; public page verified. Signer-button browser proof awaits user OISY sign-in |
| CS-67 | Done | sentinel_flow_tests | Real canister auth, manual pass and four-hour timer regression coverage | CS-65 | Reuse exact official PocketIC7 and existing fixture/cache stack |
| CS-68 | Blocked | coordinator | Preserve policy, propose four-hour cadence, integrate declarations, review, publish and verify | CS-65 through CS-67 | Source merge, upgrade, asset sync and immediate mainnet check complete. Four-hour proposal17 awaits a second signer and execution after 2026-09-29T17:55:57Z |

Acceptance: immediate maintenance is authorized, single-flight and follows existing reserves/caps/cooldowns; public reads cannot spend funds; manual checks do not postpone the automatic schedule; active cadence is derived from live policy; four-hour policy changes only sample interval to14,400 seconds and stale window to28,800 seconds, preserving the current two-interval relationship and all other fields. Source publication, schedule proposal, approval and execution are separate proof states.

Sonnet availability remains the previously verified loggedIn=false; native workers continue without external source sharing. Root owns shared bindings, ICRC-21 consent, commits, review and all live actions. Disk-pressure cleanup fetched origin and removed0 worktrees:0 eligible,12 protected/dirty/unmerged; 23GiB free, preserved.

### Four-hour cadence review checkpoint

Final source: `77f8445000342e13fcb0978f78d006464d8cac94`; PR406. CS-65 and CS-66 are in Review with 615 native tests plus five final consent tests, 24 frontend tests, and both production builds passing. CS-67 final full 40-case run uses the frozen fixture `bc15f76e37ce562f7f757fe03539a4f83979f704a058a96e62f0dbb02e2ef374`; four new cases already passed on the unchanged functional implementation. Two fresh native GPT-5.6 Luna reviewers own the second review round; Sonnet authentication was unavailable and that fallback was already surfaced. Reviewers have no write or deployment authority.

CS-68 policy proposal17 has one signer approval and a 24-hour waiting period ending 2026-09-29T17:55:57Z. A second configured signer and eventual execution are required for the four-hour schedule; manual-check publication is independent of that wait. There is no bypass or automatic assumption that pending policy is active.

### Four-hour cadence live checkpoint

PR406 merged as `1b525338cb7b391f406f84d226a13155bc781ab6`. Both fresh final reviewers closed PASS; the final full real-canister suite exited0 with40/40 cases passed. CS-65 and CS-67 are complete. Backend upgrade and frontend assets-only publication succeeded with authoritative hashes/controller inventories preserved. An authenticated mainnet immediate check returned Ok and kept the scheduled deadline unchanged. CS-66 public UI is live and verified; the remaining signer-button browser proof awaits OISY sign-in. CS-68 publication is complete, but four-hour activation is blocked on the existing second-signer/timelock/execution requirements. Receiving balances are0 cycles/0 ICP. [Live evidence](sentinel-cadence-live-proof-2026-09-28.md).

# Bitcoin experience — 2026-09-29

Worktree: `/Users/robertripley/.codex/worktrees/f22c/rumi-protocol-v2`; branch `codex/bitcoin-minter-vaults`.
Authorization: Rob requested DFINITY ckBTC mint/redeem at `/bitcoin`, a dedicated `/bitcoin/borrow` journey matching DOGE mechanics, Bitcoin audience design, and Luna subagents. Coordinate with ckUSDC chat `01a0ebc3-fc5e-7070-874b-279ecec48a67` before PR/deployment. No live financial action or deployment authorized.

| ID | State | Owner | Requested / actual model | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- | --- |
| BTC-01 | Done | bitcoin_minter | Luna / gpt-6-luna | `/bitcoin` minter, ckBTC IDL/actors and minter helpers/tests | Official upstream interface, bounded polling, exact amounts/allowance/fees, frozen withdrawals, owner/session guards, DFINITY ckBTC only |
| BTC-02 | Done | bitcoin_borrow | Luna / gpt-6-luna | `/bitcoin/borrow`, dedicated BTC wizard helpers/tests; scoped Bitcoin-only VaultCard repayment prop | DOGE parity, explicit existing ckBTC opt-in, partial-outcome recovery, strict current terms and refresh action |
| BTC-03 | Done | coordinator | root | Shared navigation/config/wallet permissions, integration, deterministic checks and ckUSDC coordination | Preserves reviewed ckERC20 commit6a7e44ae history; combined build and427/427 focused checks pass;28-error existing typecheck baseline retained |
| BTC-05 | Done | bitcoin_fixtures | Luna / gpt-6-luna | Bitcoin mounted-component regression fixtures | 9 borrow+5 minter mounted cases; exact satoshi/allowance/fee/destination, wallet changes, partial outcomes and persisted uncertain withdrawal recovery |
| BTC-04 | Done | two fresh reviewer pairs | Luna / gpt-6-luna | Two independent wallet/release review rounds | Round1 excess allowance corrected; unused txid helper corrected. Both fresh round2 reviewers PASS on0d0f0cf1. No live wallet/funds/deployment proof |

Workers own only their listed paths. Shared `config.ts`, `pnp.ts`, `ledgerFeeService.ts`, `routes/+layout.svelte` and coordination artifacts belong to root. Existing DOGE sources are read-only for workers. No worker commits, PRs, deployment, wallet signatures or real deposits.


# 3pool deposit concentration cap — 2026-10-06

Branch: `codex/3pool-deposit-cap`; worktree: `/Users/robertripley/.codex/worktrees/3pool-deposit-cap/rumi-protocol-v2`.
Authorization: implement a 66.6% deposit cap with Luna workers and merge when verified. No deployment or funds action. Coordinator owns integration, generated interfaces, board, commits, PR and merge.

| ID | State | Owner | Requested / actual model | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- | --- |
| DEP-01 | Done | deposit_backend | Luna / gpt-5.6-luna | 3pool deposit admission and pure unit tests | Exact 666/1000 cap; before pulls; query/update parity; corrective deposits above cap; integer normalization |
| DEP-02 | Done | deposit_frontend | Luna / gpt-5.6-luna | Deposit form and service helpers/tests | Explain cap, block invalid deposits before wallet approval, offer paired amounts, preserve wallet flows |
| DEP-03 | Done | deposit_integration | Luna / gpt-5.6-luna | New focused PocketIC deposit-policy tests | Rejection before debit; accepted paired/stable deposits; quote/update parity |
| DEP-04 | Merge gate passed | coordinator + two independent Luna reviewers | Luna / gpt-5.6-luna | Interfaces, deterministic checks, independent review, PR and merge | Focused tests/builds pass; both independent reviewers pass; source merged |

## Acceptance contract

- Nominal normalized token amounts (decimal normalization, no price oracle). Scope is icUSD (coin0) deposits only; swaps, donations and withdrawals remain unchanged.
- Limit is exactly 66.6% = 666/1000. A pool at or below the limit cannot accept a deposit whose resulting icUSD share exceeds it.
- Above the limit, the incoming deposit itself must contain no more than 66.6% normalized icUSD, so it reduces the existing concentration. Either or both stablecoins qualify. Stable-only corrective deposits are accepted. This supersedes the initial 1:1 matching draft per Rob's clarification.
- Initial liquidity must obey the cap. No stable-state migration or new admin controls are needed.
- Update admission runs under the existing pool lock before token pulls. Quotes use the same policy. Rejection is explicit and presented before wallet approvals.
- UI offers the minimum required combined stable amount split across both coins, rounded up to their native precision. At exact 66.6%, 200 icUSD requires approximately 100.300301 total stablecoins; 700 icUSD +300 stablecoins is rejected when the pool is 70% icUSD. Proportional deposits at the cap are allowed. Treat corrective deposits as gradual improvement, not a guarantee of restoring the pool to the cap in one deposit.
- Existing unrelated work and historical board entries remain untouched. Models are explicitly user-selected Luna, so no Sonnet check/fallback is required.

Evidence and shared reviewer rubric: `docs/coordination/3pool-deposit-cap-evidence-2026-10-06.md`. DEP-01: 123 native tests passed. DEP-03: 9 new real-ledger tests and 9 existing canister regressions passed. Final frontend: 934 tests and production build passed; type-check baseline unchanged. Both independent reviewer pairs passed across two rounds. The merge gate passed; authoritative merge state is recorded in PR #420. No deployment or live wallet transaction is claimed.

## Fiat stable 4x release — October 7, 2026

User authorized implementation, merge, deployment and historical points top-ups. Coordinator owns merge/deployment. Worktree `/private/tmp/rumi-points-release`, branch `codex/fiat-stable-points-release`. Policy: `docs/coordination/fiat-stable-points-policy.md`. No unrelated backend, 3pool, Stability Pool, governance or funds changes.

| Card | State | Owner | Requested / actual model | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- | --- |
| PS4-BACKEND | Complete | backend worker | Sonnet unavailable / GPT-5.6 Terra | Points-only policy, durable historical adjustment, epoch transition, tests and Rust interface | Preserve historical 5x, per-row floor(3x/3) top-ups exactly once; epoch-consistent future4x; old-state upgrade, auth, boundedness, reconciliation tests |
| PS4-FRONTEND | Complete | frontend worker | Sonnet unavailable / GPT-5.6 Luna | Points rule UI, historical labels, docs and focused tests | Runtime policy truth; historical5x and top-ups identifiable; no new5x composition incentive |
| PS4-PREFLIGHT | Complete | preflight worker | Sonnet unavailable / GPT-5.6 Luna | Read-only mainnet points identity, controllers, module, epoch/ledger state; deploy commands | Exact current target and availability; no mutations |
| PS4-INTEGRATE | In progress | coordinator | coordinator | Generated declarations, reporting, isolated builds, independent review, PR merge and authorized live release | Two independent adversarial reviewers; deterministic gates; exact merged artifact and deployed hash; ledger/top-up reconciliation and served frontend bytes |

Backend uses Terra because a retry-safe persisted migration must coexist with in-flight randomized snapshots and populated legacy-state upgrade; Luna handles narrower frontend and evidence tasks. Every subagent must return exact evidence and blockers. Deployment and admin migration mutations remain coordinator-only.

| Card | State | Owner | Actual model | Scope | Evidence |
| --- | --- | --- | --- | --- | --- |
| PS4-REPORT | Complete | reporting worker | GPT-5.6 Luna | Read-only prefix preview and post-migration reconciliation | 15 tests pass; live original prefix 401 rows; expected uplift 215174119482 e8s |
| PS4-REVIEW-1A | Complete | code_review_a | GPT-5.6 Luna | Independent source and release review | First code review round |
| PS4-REVIEW-1B | Complete | code_review_b | GPT-5.6 Luna | Independent source and release review | First code review round |

| PS4-REVIEW-2A | Complete | final_review_a | GPT-6 Luna | Fresh independent adversarial review | Completed code and upgrade evidence |
| PS4-REVIEW-2B | Complete | final_review_b | GPT-6 Luna | Fresh independent adversarial review | Completed code and upgrade evidence |

| PS4-REVIEW-3A | Complete | round3_a | GPT-6 Luna | Independent source review | Consistent unscheduled frontend metadata gap accepted and fixed |
| PS4-REVIEW-3B | Complete | round3_b | GPT-6 Luna | Independent source review | Same metadata gap independently confirmed |

## ckSOL integration — 2026-10-18

Worktree: `/private/tmp/rumi-cksol-20261018`; branch: `codex/cksol-collateral-minter`.

| Card | State | Owner | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- |
| CKSOL-1 | Done | Ben | Backend registration, finite-budget deposit relay, Candid and backend tests | Copy current ckETH risk terms; caller-bound mint owner; durable budget/attempts; no live mutations. Sonnet provider check rejected by automatic approval review for private-repo export; native fallback. |
| CKSOL-2 | Done | Rob's coordinator | Frontend minter, collateral metadata, upstream bindings, tests | Exact bigint lamports, wallet session binding, durable recovery, status semantics, dynamic collateral. |
| CKSOL-3 | Done | Dana and Eli | Read-only adversarial verification | Deterministic checks first; two isolated reviews; resolve concrete findings. |

### Shared API contract

- Upstream pinned `dfinity/cksol` `2acf44e9e2243f1d10ac0fd75f264f6a0dfb52a2`; deployed Candid matches the sweep API.
- Minter `lh22c-kyaaa-aaaar-qb5nq-cai`; ledger `ls5lp-lqaaa-aaaar-qb5oa-cai`; 9 decimals.
- Backend `register_cksol_collateral() -> Result<(), ProtocolError>`.
- Backend `set_cksol_relay_budget(cycles: u64) -> Result<(), String>` developer only. Finite remaining budget, default zero; cap 100T.
- Backend query `get_cksol_relay_budget() -> u64`.
- Backend `start_cksol_deposit() -> Result<u64, String>` caller is credited owner, default subaccount; retain/reuse accepted deposit id while pending.
- Backend query `get_cksol_deposit(owner: Principal) -> opt CkSolRelayAttempt`; attempt exposes `status: variant { SubmittedOrUnknown; Rejected: text; Queued: nat64 }`, `started_at_ns: nat64`.
- Budget reserves entire 1T attached cycles before dispatch, conservatively never refunds budget on callback. User-visible fees are distinct from sponsorship allocation.
- Unknown outcomes block resubmission; developer recovery method may bind an observed deposit id but cannot spend/mint.
- No deployment, activation, wallet signing, token movements, or governance calls.

### ckSOL source verification

- Backend: 10 focused tests passed; backend binary check passed; source-generated Candid compatibility passed; `didc check` against the prior service passed. Backend declarations regenerated.
- Frontend: 82 focused and existing ckBTC/ckDOGE regression tests passed; production build and authentication-asset verification passed. Full typecheck has exactly the baseline's 30 errors and 66 warnings in 25 files, with identical error paths/messages.
- Final independent reviews: Dana PASS and Eli PASS, scoped to source readiness. Initial reviews led to operation-bound journal/status protections, active signer checks, commit-time registration protection, and bounded reclamation of abandoned definite rejections.
- Browser: signed-out desktop and 390px mobile layout and direction-tab interaction checked in the in-app browser; mainnet ckSOL actions disabled on localhost. Existing global live-data queries fail against the local replica. A preview server required restart after rebuilt asset hashes replaced its original manifest.
- Limits: no PocketIC minter/relay lifecycle, optimized Wasm artifact/install, connected-wallet action, deployment, collateral activation, sponsorship allocation, or SOL settlement proof. These remain distinct from this source acceptance.
- Safe worktree inventory under disk pressure: zero eligible removals; dirty/locked/unmerged/active trees preserved.


---

# SP conversion and liquidation restoration

Root coordinates implementation, integration and all production operations.
Starting source: `b16077a97639a60cf2f3064ba3b504b9a83c1733`.
Working branch: `codex/sp-conversion-liquidation-restoration-20261023`.

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| R-01 | Done | agent:rails_conversion_review | Review durable conversion requirements | — | Caller intent, escrow, capped fees and 3pool durable receipt requirements accepted |
| R-02 | Done | agent:rails_liquidation_review | Review ordinary stable liquidation receipt requirements | — | Payment/result/payout identities, exact rounding, fallback metadata and routing markers accepted |
| R-03 | Review | agent:rails_3usd_review | Review 3USD ingress cutover and deployed protocol coupling | — | 34 reserve operations matched; 82.19310823 3USD recorded, 2.51791010 excess retained separately |
| R-04 | In progress | root | Fresh live/source inventory and scoped implementation | R-01, R-02, R-03 | Current public state captured privately; preserve reopened SP and interest receipts |
| R-04a | Review | agent:rails_3pool_receipts_impl | Durable add-liquidity receipts and fee-capped escrow compensation | R-01 | Source ready for root compile/runtime checks; prior revision compiled |
| R-04b | Review | agent:rails_backend_receipts_impl | Ordinary liquidation payment/debt/payout journal | R-02 | Source checkpoint f853310e; full backend library tests 1296 pass, one prior ignored |
| R-04c | Review | agent:rails_sp_conversion_impl | SP escrow conversion and recovery | R-04a | Explicit budget/sequence, escrow proofs, exact recovery implemented; SP unit suite passes |
| R-04d | Review | agent:rails_sp_ordinary_impl | SP exact ordinary settlement and routing | R-04b | Durable ordinary rail, affordable partials, exact proofs and cancellation implemented |
| R-04e | Review | agent:rails_conversion_ui_impl | Explicit quote, fee authorization and wallet recovery UI | R-04c | Exact NotFound replay and approval Duplicate fixes; 13 focused tests pass; final build pending |
| R-05a | In progress | agent:rails_3usd_review | Exact-artifact local multi-canister runtime harness | R-04 | Candidate01 runtime checks in progress |
| R-05b | In progress | agent:rails_aged_backend_runtime | Actual aged payment, protocol fee and payout proof recovery | R-04b | Separate local fixture; exact candidate02 pending |
| R-05 | In progress | root | Deterministic regression, runtime, migration and interface checks | R-04 | Repaired source: SP267, 3pool180, backend1299 pass; final interface refresh/build/runtime/migration repetition pending |
| R-06 | In progress | two fresh independent reviewers | Adversarial final release review | R-05 | Both candidate01 reviews FAIL; findings assigned and repaired; fresh candidate review pending |
| R-07 | Blocked | root | Deploy and restore reviewed conversion/liquidation paths | R-06 | No task upgrades or activation yet; exact final artifact and adversarial passes required |

## Scope and acceptance

- Restore SP convenience conversion, ordinary stablecoin liquidations, and the
  configured 3USD liquidation route without weakening ambiguity/replay safeguards.
- Preserve all depositor credit, interest receipts, existing unknown operations,
  controller/protocol bindings, and unrelated production features.
- Every financial phase must have a durable identity before dispatch and conserve
  funds under success, rejection, transport ambiguity, retry and upgrade.
- New public interfaces and state fields must have compatibility and migration
  evidence. Runtime proof must cover real ledgers and fault outcomes.
- Root alone executes authorized production upgrades/activation. Unknown outcomes
  require authoritative reconciliation before another mutation.

## Execution notes

Local native reviewers are used for R-01 through R-03 and root owns implementation.


Candidate01 is explicitly rejected for deployment. Its immutable evidence and
both review reports are retained under the private task evidence root. Production
remains on the reopened SP predecessor while source repairs and new checks run.

## 2026-10-23 completion and donation reconciliation

The preceding R-series working rows are superseded by the completed candidate08
release: source8d7c7e75, PR590, deployed/open at19:29UTC, both source and both live
adversarial audits PASS. See docs/operations/sp-rails-restoration-20261023.md.

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| DON-01 | Review | agent:donation_ledger_evidence | Exhaustive icUSD ledger history and pool-directed donations | — | Must prove contiguous ledger coverage, archives and every matching tuple; read-only |
| DON-02 | In progress | agent:donation_pool_accounting | Map pool donation receipts/reserves and legacy accounting | — | Current source plus reviewed snapshots; identify exact read-only extraction; no production mutation |
| DON-03 | In progress | root | Backend mint/donation journals, histories and live baseline | — | Pin live modules/identity; no remint or re-credit without positive missing-credit proof |
| DON-04 | In progress | root | Repair confirmed missed donation credits | DON-01, DON-02, DON-03 | Only proven gaps, exact replay identities, conservation and migration/runtime checks |
| DON-05 | Ready | two independent reviewers | Adversarial reconciliation/repair verification | DON-01, DON-02, DON-03, DON-04 | Exhaustive coverage, exact amounts, no double credit, all residuals classified |

All DON evidence is retained in the operator's private evidence store.
Root is sole production executor. Investigators may make bounded read-only calls;
no stop/start, snapshots, installs, transfers, mints or accounting writes.

DON-04 frozen repair manifest v1 (SHA256 30edc54ad35151a4dba7a7ff37d9d17f0bf2a7a4630a3d4a58162bdf65d011a6): 7 October 14 credits totaling 36063541 raw. Existing deployed V2 controller path only; no new code. Ledger 1999 pool mints found; early 92 before April 10 remain under investigation.

DON-05a claimed by agent:donation_repair_review_a; DON-05b claimed by agent:donation_repair_review_b. Both independently apply repair-review-rubric-v1.md to the frozen seven-credit manifest/executor and write separate reports. They may not inspect each other's reports. PASS admits only the seven-credit repair; early92 audit remains DON-02 in progress. Root alone may execute.

DON-04 seven-credit repair completed 2026-10-24T00:31:42Z: seven exact receive_donation_v2 calls, executed once, all Ok. Reserve +36063541 raw, physical and supply unchanged, receipts 100 to 107. Independent pre- and post-execution audits PASS. Remaining 19616202 raw surplus untouched. DON-02 older-92 audit continues; no whole-audit completion claim.

DON-06c claimed by fresh agent:donation_final_review_c; DON-06d claimed by fresh agent:donation_final_review_d. Both independently apply final-audit-rubric-v1.md to entire1999paymentaudit, early92reconstruction andactualsevenrepair. They maynotread/contacteachother. Onlyassignedreviewwrites; no productioncalls. Rootownscoordination andfinalartifacts. Wholeauditcompletionstillpendingtheirreviews.

## Donation audit final disposition — 2026-10-24

- DON-01 Done: certified contiguous icUSD ledger0..42696, supplementalcertifiedtip42698;1999poolmints145.04987166icUSD, allrowsclassified.
- DON-03 Done:93exactstoredreceipts+journaljoins,7newnormaldonations, latestlive00:55:35UTCwith107receipts,unchangedmodules/backing andemptyjournals.
- DON-04 Done:7positiveOctober14gaps correctedonce,0.36063541icUSD,allOk; no transfer/mint/install/stop/export. Both preflight and postexecution A/B PASS.
- DON-06c/06d Done: both fresh final reviewers PASS exact reportv4/CSVv4, specifically bounded findings and proof limits. They do NOT certify every historical individual credit.
- DON-02 Pending external historical evidence: earliest 92 arrivals (39.87276282 icUSD) verified; observed accounting model closes, but part of the pre-March-13 operation history is unavailable. Need older backup or original operation logs; requested asynchronously. No additional positive credit amount, no speculative correction.
- Separateinitial0.10icUSDpartialdepositclaim and residual0.19616202icUSDremainuntouched; ownership/oldLPallocations notassumed.
- No source files changed for this audit.
# ckSOL user-funded processing

| ID | State | Owner | Scope | Acceptance / evidence |
| --- | --- | --- | --- | --- |
| SOLPAY-1 | Implemented | Root | Backend payment/refund state, Candid, consent | 29 focused tests pass; durable caller-funded effects/refunds; stable global replay fence; legacy sponsorship disabled; rejection pruning and post-funding capacity recheck |
| SOLPAY-2 | Implemented | Nora | Frontend paid deposit service/page/tests | Global quote sequence, exact expiring approval with expected allowance, generated Candid signature parity, durable receipt recovery without local journal, pre-approval legacy relay gate and rejected-attempt acknowledgment, settled next-payment and deposit-status UI, owner/session-safe recovery, Plug/II/OISY and PNP whitelist coverage; old sponsored start removed. Focused frontend suites: 72 tests pass. No live wallet/funds actions. |
| SOLPAY-3 | Passed | Yara and Zane | Final integrated patch | 101 focused tests; production builds; interface/state compatibility; both fresh round-5 reviewers PASS after capacity/recovery fixes. No live funds or deployment proof. |

Worktree: `/private/tmp/rumi-cksol-user-funded-20261020`; branch `codex/cksol-user-funded`, base `ed99a4d7`. Existing dirty primary preserved. Shared generated backend declarations owned by Root only. Backend/source integration owned by Root. Native frontend worker uses available GPT-6 Luna; external provider route previously rejected for private-source envelope and is not retried. No live funds test or deployment is part of this implementation pass.
