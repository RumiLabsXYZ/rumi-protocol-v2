# Cycle Sentinel and Telemetry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to execute this plan one task at a time. Each implementation task requires a fresh independent review before the next task begins.

**Goal:** Replace CycleOps with a Rumi-owned canister that observes every governed target, maintains cycle balances through either the Cycles Ledger or ICP/CMC, and serves public and authenticated telemetry at the two approved `/telemetry` routes.

**Architecture:** A new Rust canister, `rumi_cycle_sentinel`, owns a versioned stable registry, multisig and timelock governance, cached observations, bounded history, alarms, and durable exact-argument funding operations. It observes Rumi-aware targets through `cycles_status` and generic targets through the existing immutable `ic-blackhole` status relay. Anonymous frontends read cached queries only. Every mutation is authorized in the canister. The existing `rumi_treasury` implementation and route are excluded from edits.

**Tech stack:** Rust 2021, Candid, `ic-cdk` 0.12.x, `ic-cdk-timers` 0.10.x, `ic-stable-structures` 0.6.x, PocketIC 6.x, SvelteKit, TypeScript, Vitest, `dfx.json`, and `icp.yaml`.

**Approved design:** `docs/superpowers/specs/2026-09-13-cycle-sentinel-telemetry-design.md`

## Locked decisions

- The project and visible product name is Cycle Sentinel or Cycle Telemetry.
- Do not modify `src/rumi_treasury/**`, `src/vault_frontend/src/routes/treasury/**`, its Candid, or its controller configuration.
- The public route is exactly `rumiprotocol.com/telemetry`; the authenticated operator route is exactly `app.rumiprotocol.com/telemetry`.
- Sentinel is never a target controller.
- Generic target observation uses the existing `ic-blackhole` canister `e3mmv-5qaaa-aaaah-aadma-cai`, method `canister_status`, and pinned Wasm SHA-256 `210cf941e5ca77daac314a91517483ac171264527e3d0d713b92bb95239d7de0`. Source and current ICP documentation identify that canister as self-controlled and expose the reproducible hash. Reverify its live module hash and sole self-controller immediately before onboarding any target.
- `approve_proposal` records a signer approval only. `execute_proposal` separately checks approval threshold and timelock, then applies the proposal once. Approval never auto-executes.
- The stable registry is the only funding authority. Discovery and target reports cannot add a recipient or change an amount.
- Every target starts with `enabled = false` and `auto_topup = false`. The Conflux frontend starts `Unobserved`.
- Cycles Ledger withdrawal is the primary rail. ICP transfer plus CMC notification is fallback only after cycles funding is proven unavailable, never while its result is unknown.
- Every external funding call is preceded by a stable reservation and exact immutable operation snapshot. Retries reuse identical arguments and `created_at_time`.
- One unresolved operation is allowed per target. Manual and timer paths use the same reservation table.
- Sentinel self-recovery is a separate hard-coded lane whose destination is always `ic_cdk::id()` and whose protected reserve cannot fund ordinary targets.
- Production signer principals, governance threshold, timelocks, per-target policy values, and funding amounts are deployment-manifest inputs. They must be read from authoritative identities and the CycleOps export or UI, never inferred from compact badges.

## Stable state map

Use one `MemoryManager<DefaultMemoryImpl>` and never reuse a memory ID. Stable values use explicit versioned envelopes and bounded `Storable` implementations.

| Memory ID | Contents |
| --- | --- |
| 0 | Versioned global configuration cell |
| 1 | Target registry keyed by principal |
| 2 | Governance proposals keyed by proposal ID |
| 3 | Governance counters cell |
| 4 | Hourly sample ring keyed by target and slot |
| 5 | Per-target sample metadata |
| 6 | Alarms keyed by alarm ID |
| 7 | Alarm counter cell |
| 8 | Funding operations keyed by operation ID |
| 9 | Funding counters and global monotonic timestamp cell |
| 10 | Per-target reservation and rolling-spend state |
| 11 | Global rolling-spend state |
| 12 | Sentinel self-recovery state |
| 13 | Bounded terminal funding summaries |

Bounds are constants with rejection or pruning tests: 128 targets, 2,160 samples per target, 1,024 alarms, 256 retained proposals, 512 terminal summaries, and 100 public records per page. Nonterminal operations are never pruned.

## Task 1: Scaffold the canister and versioned stable domain model

**Kanban:** CS-10

**Files:**

- Create `src/rumi_cycle_sentinel/Cargo.toml`.
- Create `src/rumi_cycle_sentinel/src/lib.rs`.
- Create `src/rumi_cycle_sentinel/src/types.rs`.
- Create `src/rumi_cycle_sentinel/src/state.rs`.
- Add `src/rumi_cycle_sentinel` to root `Cargo.toml`.

**Implementation:**

- Match repository dependency pins and add only `candid`, `serde`, `serde_bytes`, `sha2`, `ic-cdk`, `ic-cdk-timers`, `ic-stable-structures`, and the path dependency on `rumi_cycle_manager` unless a test proves another dependency necessary.
- Define bounded Candid/stable types for init arguments, global policy, signer permissions, target records, observation modes, public states, samples, alarms, proposals, immutable policy snapshots, reservations, both rail state machines, and self-recovery.
- Use checked `Nat` to `u128` conversion for policy and funding inputs. Oversized values are rejected, never saturated. Target-reported advisory balances may saturate to `u128::MAX` for display while remaining ineligible for a low-balance decision.
- Validate non-anonymous unique signers and `1 <= threshold <= signers.len()` at init. There is no zero-signer production initialization.
- Reject anonymous, Sentinel-self, management, ICP Ledger, CMC, Cycles Ledger, and duplicate target principals, plus oversized fields and policies.
- Implement stable accessors and versioned decode tests. Do not seed production policies in code.
- Expose only `cycles_status` in this task so Sentinel can later observe itself through the existing Rumi interface.

**Tests and gate:**

```bash
cargo test -p rumi_cycle_sentinel state::
cargo test -p rumi_cycle_sentinel types::
cargo check -p rumi_cycle_sentinel
cargo build -p rumi_cycle_sentinel --target wasm32-unknown-unknown --release
```

Tests must cover stable round trips, legacy envelope decoding, every reserved principal, bounds, duplicate signers, invalid thresholds, disabled defaults, and distinct memory IDs.

**Commit:** `feat(cycles): scaffold Cycle Sentinel stable domain`

## Task 2: Implement multisig, timelock, registry mutations, and alarms

**Kanban:** CS-10

**Files:**

- Create `src/rumi_cycle_sentinel/src/governance.rs`.
- Extend `src/rumi_cycle_sentinel/src/state.rs` and `src/rumi_cycle_sentinel/src/lib.rs`.

**Implementation:**

- Implement proposals for register, update, remove, set global policy, add signer, remove signer, set signer threshold, and unpause.
- Snapshot the full bounded payload in stable storage at proposal creation.
- Require a signer for propose, approve, execute, cancel, immediate pause, and manual top-up entry points.
- `approve_proposal` is idempotent for the same signer and never applies a payload.
- `execute_proposal` requires threshold approvals and elapsed per-kind timelock, then applies exactly once.
- Removal and edits stop new work but leave immutable in-flight snapshots untouched.
- Safe immediate pause is single-signer. Unpause and spend-widening changes are governed.
- Bound and compact proposals without deleting open proposals.
- Implement deduplicated bounded alarms with acknowledge and auto-resolve behavior.

**Tests and gate:**

```bash
cargo test -p rumi_cycle_sentinel governance::
cargo test -p rumi_cycle_sentinel state::alarms
```

Tests must include anonymous and nonsigner negatives for every update, threshold edges, early execution, replayed execution, signer removal invariants, revision increments, edit/remove during an in-flight snapshot, immediate pause, and governed unpause.

**Commit:** `feat(cycles): add governed target registry`

## Task 3: Implement observation, blackhole verification, history, and public reads

**Kanban:** CS-11

**Files:**

- Create `src/rumi_cycle_sentinel/src/observation.rs`.
- Create `src/rumi_cycle_sentinel/src/history.rs`.
- Create `src/rumi_cycle_sentinel/src/public_api.rs`.
- Extend `src/rumi_cycle_sentinel/src/lib.rs` and state types.

**Implementation:**

- Decode `SelfReport` replies using `rumi_cycle_manager::CycleManagerCyclesStatus`.
- Implement the exact `ic-blackhole` Candid subset locally. Verify the proxy by asking it for its own status and require the pinned hash plus controllers exactly equal to the proxy principal. Then read the target status and require proxy membership, `Running`, and a nonempty target module hash.
- Distinguish Healthy, Low, Stopped, Uninstalled, Unreachable, and Unobserved. A failure never becomes zero or a top-up trigger.
- Store hourly samples in a per-target ring. Calculate burn as `max(0, start + confirmed topups - end)`. Any interval containing an unknown funding operation is indeterminate.
- Raise an alarm and pause new target funding on configured burn anomaly.
- Implement cached anonymous overview, target, sample, top-up, and alarm queries with opaque cursor pagination capped at 100.
- Public data excludes signer principals, proposal payloads, operation internals, ledger subaccounts, and pending reconciliation evidence.

**Tests and gate:**

```bash
cargo test -p rumi_cycle_sentinel observation::
cargo test -p rumi_cycle_sentinel history::
cargo test -p rumi_cycle_sentinel public_api::
```

Tests must cover all proxy invariants independently, exact threshold equality, all six states, stale data, target-report deception bounded by registry policy, burn correction, unknown intervals, ring pruning, public pagination, and public-field privacy.

**Commit:** `feat(cycles): add verified observation and bounded telemetry`

## Task 4: Implement the Cycles Ledger outbox and self-recovery lane

**Kanban:** CS-12

**Files:**

- Create `src/rumi_cycle_sentinel/src/cycles_ledger.rs`.
- Create `src/rumi_cycle_sentinel/src/funding.rs`.
- Create `src/rumi_cycle_sentinel/src/self_recovery.rs`.
- Extend `src/rumi_cycle_sentinel/src/lib.rs`, types, and state.

**Implementation:**

- Hand-type the current Cycles Ledger `Account`, `WithdrawArgs`, result, and all known error variants from the official Candid.
- Reserve balance, per-target cap, global cap, cooldown, and the one-operation slot in stable state before the first await.
- Persist one globally monotonic `created_at_time = max(now, last + 1)` and the exact call arguments before submission.
- Model `PlannedReserved`, `Submitted`, `Confirmed`, `Unknown`, `Complete`, `Terminal`, and `Quarantined` as explicit persisted states.
- Retry Unknown using the byte-equivalent original arguments. Treat Duplicate as proof of original success. Keep TooOld without proof quarantined and reserved.
- Never fall through to ICP while the cycles result is unknown.
- Implement self-recovery through the same idempotent withdrawal contract but separate stable state, protected reserve, cap, and hard-coded destination `ic_cdk::id()`.
- An unresolved self-recovery operation suppresses all ordinary distribution.

**Tests and gate:**

```bash
cargo test -p rumi_cycle_sentinel cycles_ledger::
cargo test -p rumi_cycle_sentinel funding::cycles
cargo test -p rumi_cycle_sentinel self_recovery::
```

Tests must prove the reservation exists before the adapter is invoked, exact retry, Duplicate, TooOld, unknown quarantine, cap and cooldown behavior, one in-flight operation, immutable revision snapshot, protected reserve, immutable self-destination, low-fund alarm, and global suppression on unknown self-recovery.

**Commit:** `feat(cycles): add durable cycles funding and self-recovery`

## Task 5: Implement the ICP and CMC fallback plus reconciliation

**Kanban:** CS-12

**Files:**

- Create `src/rumi_cycle_sentinel/src/icp_cmc.rs`.
- Extend `src/rumi_cycle_sentinel/src/funding.rs` and `src/rumi_cycle_sentinel/src/lib.rs`.

**Implementation:**

- Hand-type ICP transfer, block lookup, account identifier, exchange rate, and CMC notification contracts compatible with repository pins.
- Persist the exact ICP transfer, fee, memo, rate snapshot, and `created_at_time` before calling the ledger.
- Require a fresh nonzero rate and enforce per-attempt, target, and global caps before reserving.
- Implement `PlannedReserved`, `LedgerSubmitted`, `TransferUnknown`, `TransferConfirmed`, `NotifyPending`, `Complete`, `Refunded`, `Terminal`, and `Quarantined`.
- Treat a duplicate transfer as the original block. Every CMC retry reuses that block. Never submit a second ICP transfer while the first is unknown.
- Implement `resolve_unknown_as_spent` conservatively and `attach_block_proof` only after querying and matching the authoritative ledger block to the immutable operation.
- Manual top-up enters the same state machine as the timer and cannot bypass policy.

**Tests and gate:**

```bash
cargo test -p rumi_cycle_sentinel icp_cmc::
cargo test -p rumi_cycle_sentinel funding::icp
cargo test -p rumi_cycle_sentinel funding::reconciliation
```

Tests must cover published account and memo vectors, stale and zero rates, proven-unavailable versus unknown cycles outcomes, lost replies, exact retry, Duplicate, TooOld, notify pending, refund, quarantine, mismatched proof, matching proof, and the timer/manual race.

**Commit:** `feat(cycles): add durable ICP fallback and reconciliation`

## Task 6: Wire timers, Candid, configuration, declarations, and integration tests

**Kanban:** CS-13

**Files:**

- Create `src/rumi_cycle_sentinel/src/sampler.rs`.
- Create `src/rumi_cycle_sentinel/rumi_cycle_sentinel.did`.
- Create `src/rumi_cycle_sentinel/tests/` fixtures and PocketIC suites.
- Vendor the reviewed official Cycles Ledger Candid fixture under `src/rumi_cycle_sentinel/tests/vendor/` with source commit and SHA-256 recorded.
- Extend root `Cargo.toml`, `dfx.json`, `icp.yaml`, and `scripts/regenerate-declarations.sh`.
- Generate `src/declarations/rumi_cycle_sentinel/**`.

**Implementation:**

- Timer order is self-recovery, resume pending operations, sample, then evaluate new target funding.
- Re-arm timers after init and upgrade. Public queries remain cached and make no inter-canister calls.
- Add test-only mock canisters for SelfReport, blackhole status, Cycles Ledger, ICP Ledger, and CMC. Mock ledgers enforce dedup by exact arguments and timestamp and can model committed-with-lost-reply, Duplicate, TooOld, and refund.
- Add test-only state injection behind a nonproduction Cargo feature solely where needed to place an operation at every persisted state before an upgrade. Production Candid and Wasm must exclude those endpoints.
- Verify handwritten Candid against exported Candid and regenerate JS/TS declarations deterministically.
- Add `rumi_cycle_sentinel` to local and `mainnet-live` project configuration, but perform no deployment in this task.

**Tests and gate:**

```bash
bash scripts/regenerate-declarations.sh rumi_cycle_sentinel
cargo test -p rumi_cycle_sentinel
cargo build -p rumi_cycle_sentinel --target wasm32-unknown-unknown --release
```

PocketIC coverage must include every persisted outbox state across upgrade/restart, live timer/manual interleaving, target edit/removal during an operation, all 16 inventory entries disabled by default, anonymous public reads, anonymous mutation rejection, full observation-state rendering, stable bounds, Candid conformance, and a current official Cycles Ledger fee/withdraw contract fixture.

**Commit:** `feat(cycles): integrate Cycle Sentinel canister and tests`

## Task 7: Build the public telemetry page

**Kanban:** CS-20

**Files:**

- Create `src/rumi_homepage/src/lib/cycleSentinel.ts` and its unit test.
- Create `src/rumi_homepage/src/routes/telemetry/+page.svelte` and its unit test.
- Extend `src/rumi_homepage/src/routes/+layout.svelte`, `vite.config.ts`, and `tsconfig.json` only as needed for the declaration import.

**Implementation:**

- Use an anonymous actor and the generated Sentinel declaration.
- Resolve the canister ID from the normal canister build environment. If absent, render a clear not-configured state and expose no mutation surface.
- Render overview counts, reserve availability, alarms, last/next sample, and the complete public table.
- Preserve `bigint` through formatting. Never convert cycle amounts to JavaScript `number`.
- Add an operator link to `https://app.rumiprotocol.com/telemetry`.
- Add Telemetry to root navigation and footer without changing existing routes.

**Tests and gate:**

```bash
npm test --workspace rumi_homepage
npm run check --workspace rumi_homepage
npm run build --workspace rumi_homepage
```

Tests cover all six target states, precision above `Number.MAX_SAFE_INTEGER`, stale/unreachable states, pagination, empty/not-configured behavior, and absence of mutation controls.

**Commit:** `feat(telemetry): add public cycle health page`

## Task 8: Build authenticated management telemetry

**Kanban:** CS-21

**Files:**

- Create `src/vault_frontend/src/lib/services/cycleSentinelService.ts` and tests.
- Create `src/vault_frontend/src/routes/telemetry/+page.svelte` and tests.
- Extend `src/vault_frontend/src/lib/config.ts` and `src/vault_frontend/src/routes/+layout.svelte`.

**Implementation:**

- Render public data before login.
- Use the existing wallet identity/agent path for signer queries and updates.
- Show proposal, registry, pause, manual top-up, and reconciliation controls only after `get_my_permissions` confirms signer status. Canister authorization remains the security boundary.
- Surface exact canister rejections and immutable target revision/operation identifiers.
- Add an always-visible Telemetry nav entry on desktop and mobile. Do not alter `/treasury`.

**Tests and gate:**

```bash
npm run test:unit --workspace vault_frontend -- --run
npm run check --workspace vault_frontend
npm run build --workspace vault_frontend
```

Tests cover anonymous viewing, signer and nonsigner UI states, authenticated actor construction, proposal approve/execute separation, canister rejection propagation, bigint precision, and every management action.

**Commit:** `feat(telemetry): add Cycle Sentinel operator console`

## Task 9: Deterministic and adversarial source gate

**Kanban:** CS-30 and CS-40

- Run focused checks after every final edit, then the full relevant Rust, Candid, frontend, and build suite.
- Compare exported and committed Candid plus regenerated declarations with a clean diff.
- Run `cargo clippy -p rumi_cycle_sentinel --all-targets -- -D warnings` and repository formatting checks.
- Run two fresh independent Sonnet reviews: one IC security/state-machine review and one operations/frontend/release review. Each receives the approved spec, implementation plan, exact diff, and deterministic evidence.
- Resolve all accepted blockers, rerun exact-head checks after the final commit, push, open a PR, wait for terminal CI on the exact head, and merge only when green.
- Do not call a build, test, pushed branch, PR, or merge proof of deployment or activation.

## Task 10: Deploy, fund, shadow, and activate incrementally

**Kanban:** CS-50 through CS-53

- From merged `main`, rebuild Sentinel and both frontends. Require clean exact-head evidence.
- Verify `rumi_identity`, create the Sentinel canister if absent, and record the resulting principal, full controller set, subnet, module hash, and starting cycles.
- Deploy Sentinel with authoritative initial signer principals, valid threshold, timelocks, and conservative global/self-recovery policy. Verify its live Candid and `cycles_status` before funding accounts.
- Fund runtime cycles, Cycles Ledger reserve, and ICP fallback with separately recorded amounts. Verify all three balances after finality.
- Import all 16 reviewed registry entries disabled. Compare principals to live project mappings and adjacent repositories.
- Observe the eight SelfReport targets in shadow mode first.
- Reverify the pinned blackhole principal, live module hash, and sole self-controller. For each generic target, read the full current controller list, append the blackhole without replacing any controller, write the exact expanded set, and verify it before enabling observation. Stop on any mismatch or capacity issue.
- Keep the Conflux frontend Unobserved until it is installed and its authoritative production mapping is confirmed.
- Obtain unambiguous threshold, refill, cap, cooldown, and anomaly values for every target from CycleOps. Apply them through reviewed proposals, one target at a time.
- Exercise one deliberately bounded top-up on each rail and prove the exact ledger/CMC result before enabling broader automation.
- Run a shadow observation window, then enable automatic maintenance incrementally. Preserve CycleOps until Sentinel has proven sampling, alerting, top-up, retry, recovery, and self-liveness behavior.
- Remove CycleOps only after an explicit controller diff proves Sentinel coverage for every target and the replacement remains healthy. Removal never changes existing non-CycleOps controllers.

## Handoff rules

- Each implementation task has one Sonnet implementer, one separate Sonnet reviewer, and Codex integration review. Tasks run sequentially because they share the new canister and generated interfaces.
- The kanban card moves to Done only after its diff and evidence are accepted.
- Any proposed change to locked architecture returns to Codex before editing.
- Do not expose secrets, local credentials, unrelated source, or live wallet material to workers.
- A production identifier or policy value that cannot be verified is a live activation blocker, not a reason to guess and not a source implementation blocker.
