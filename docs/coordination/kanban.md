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

Last updated: 2026-09-13

Branch: `codex/cycle-sentinel-telemetry`

Spec: `docs/superpowers/specs/2026-09-13-cycle-sentinel-telemetry-design.md`

Plan: `docs/superpowers/plans/2026-09-13-cycle-sentinel-telemetry.md`

| ID | State | Owner | Task | Depends on | Evidence / blocker |
| --- | --- | --- | --- | --- | --- |
| CS-00 | Done | Codex coordinator | Lock executable plan and interfaces | None | Fresh Sonnet review PASS; coordinator corrected proxy, execution, and card-ID issues |
| CS-01 | Done | Sonnet backend mapper | Map canister, state, Candid, and test integration | None | Accepted `/private/tmp/cycle-sentinel-backend-map.md` |
| CS-02 | Done | Sonnet frontend mapper | Map both telemetry surfaces | None | Accepted `/private/tmp/cycle-sentinel-frontend-map.md` |
| CS-03 | Done | Sonnet reference mapper | Audit delegated-vault reuse and failure gaps | None | Accepted `/private/tmp/cycle-sentinel-reference-map.md` |
| CS-10 | In progress | Sonnet Task 1 implementer | Stable domain, registry, and governance | CS-00 | Domain model accepted after 191 tests and fresh Sonnet closeout PASS; stable state remains in progress |
| CS-11 | Blocked | Unassigned | Observation, history, and public API | CS-10 | Dependency not complete |
| CS-12 | Blocked | Unassigned | Both funding rails, recovery, and reconciliation | CS-10, CS-11 | Dependencies not complete |
| CS-13 | Blocked | Unassigned | Timers, Candid, config, declarations, and PocketIC | CS-10 through CS-12 | Dependencies not complete |
| CS-20 | Blocked | Unassigned | Public root-domain telemetry UI | CS-13 | Dependency not complete |
| CS-21 | Blocked | Unassigned | Authenticated operator telemetry UI | CS-13 | Dependency not complete |
| CS-30 | Blocked | Codex plus two fresh Sonnet reviewers | Deterministic and adversarial source gate | CS-10 through CS-21 | Source not complete |
| CS-40 | Blocked | Codex coordinator | Push, PR, terminal exact-head CI, and merge | CS-30 | Source gate not complete |
| CS-50 | Blocked | Codex live operator | Create, deploy, and verify Sentinel | CS-40 | Requires merged exact-head rebuild and live preflight |
| CS-51 | Blocked | Codex live operator | Add verified blackhole as an additional controller | CS-50 | One target at a time with before/after controller proof |
| CS-52 | Blocked | Codex live operator | Fund, shadow, and activate policies incrementally | CS-50, CS-51 | Exact policy manifest must be authoritative |
| CS-53 | Blocked | Codex live operator | Remove CycleOps after replacement proof | CS-52 | Terminal cutover gate |

## CS-00 details

- Scope: approved design, implementation plan, shared interfaces, task ordering, and board transitions only.
- Worktree/branch: `/Users/robertripley/.codex/worktrees/455c/rumi-protocol-v2`, `codex/cycle-sentinel-telemetry`.
- Acceptance: every approved requirement maps to a task, shared files have sequential ownership, and exact source, merge, and live gates are explicit.
- Owner/harness: Codex coordinator using three authenticated Sonnet repository mappers plus a fresh Sonnet plan reviewer.
- Review evidence: Sonnet PASS at `/private/tmp/cycle-sentinel-plan-review.md`; coordinator separately rejected the earlier unnecessary new-proxy design, preserved distinct proposal approval/execution, and corrected board IDs.

## CS-10 details

- Current slice: implementation-plan Task 1 only, canister scaffold plus versioned stable domain model.
- File ownership: `src/rumi_cycle_sentinel/Cargo.toml`, `src/rumi_cycle_sentinel/src/{lib,types,state}.rs`, root `Cargo.toml`, and `Cargo.lock` only when dependency resolution requires it.
- Acceptance: disabled defaults, strict init signer validation, reserved-principal and bound checks, versioned stable round trips, distinct memory IDs, focused tests, host check, and Wasm build.
- Worktree/branch: `/Users/robertripley/.codex/worktrees/455c/rumi-protocol-v2`, `codex/cycle-sentinel-telemetry`.
- Review evidence: Task 1b domain model passed 191 focused tests, host check, Wasm release build, and fresh Sonnet closeout review after closing findings 1-10 and A-H. Task 1c stable-memory wiring and whole-state validation remain pending.

## Cycle Sentinel coordination rules

- Only Ready cards may be dispatched. One Sonnet implementer owns each sequential card and a separate Sonnet reviewer assesses its evidence.
- Sonnet has task-relevant repository read/write access but no secrets, merge authority, wallet authority, or live-action authority.
- Codex owns architecture, accepted findings, shared-file integration, exact-head verification, merge, controller changes, funding, activation, and cutover.
- Source tests, a pushed branch, a PR, a merge, a deployment, funding, controller changes, activation, and CycleOps removal are distinct states.
- The existing `rumi_treasury` source, Candid, controller configuration, and `/treasury` behavior remain unchanged.
