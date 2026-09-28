# Independent adversarial review B2: Sentinel four-hour checks

## Verdict

**PASS for source, Candid, frontend, and production-artifact review.** No confirmed blocking issue was found in two independent passes. This review does not claim the full 40-test terminal result or any live publication/deployment.

## Scope and baseline

- Reviewed frozen source commit `77f8445000342e13fcb0978f78d006464d8cac94` against actual baseline `2b11bbfc39f68e42700fcef9141e74f2ef47c193`.
- Reviewed only the named Sentinel source, generated Candid, frontend, tests, production artifact metadata, and built preview. No source edits, live calls, deployment, or publication were performed.
- The worktree had unrelated dirty coordination docs and an untracked `target` directory; these were excluded from the source review.

## Deterministic evidence inspected

- `/private/tmp/sentinel-cadence-native-tests.log`: 615 passed, 0 failed.
- `/private/tmp/sentinel-cadence-consent-final-tests.log`: 5 passed, 0 failed.
- `/private/tmp/sentinel-cadence-focused.log`: 4 focused PocketIC regressions passed.
- `/private/tmp/sentinel-cadence-final-frontend-tests.log`: 24 frontend tests passed.
- Backend production build session `68306` and frontend build session `57034` both exited 0.
- `/private/tmp/sentinel-cadence-production-artifact.json`: source pin matches `77f84450`; strict duplex Candid, generated production Candid, and production endpoint checks all exited 0. The production export list contains no `test_*` controls.
- `/private/tmp/sentinel-cadence-built-preview.png`: public refresh failure is shown as an error, cadence is `Unavailable`, deposit balances remain `Unavailable`, and signer controls are absent. This is build-preview evidence only; local transport is intentionally unavailable.

## Backend and timer findings

- **Authentication precedes work:** `src/rumi_cycle_sentinel/src/lib.rs:197-223` rejects anonymous callers before signer lookup/work-guard acquisition, then invokes the existing sampler path. The focused integration test also confirms rejected callers cause zero observations, ledger calls, or funding operations.
- **Single-flight spans awaits and drop:** `src/rumi_cycle_sentinel/src/sampler.rs:271-324` holds `TickGuard` across the full async tick, returns `Busy` for overlap, and releases through `Drop`, including aborted futures. The timer and manual entrypoint share this guard.
- **No policy bypass:** `src/rumi_cycle_sentinel/src/sampler.rs:329-461` preserves self-recovery, pending-operation resume, sampling, source-cache refresh, reserve gating, target policy, cooldown, and fallback order. `maybe_fallback_after_no_spend` remains restricted to a proven no-spend outcome (`:501-532`).
- **Governed timer reload:** `src/rumi_cycle_sentinel/src/governance.rs:515-527` writes the validated policy before re-arming. `src/rumi_cycle_sentinel/src/sampler.rs:223-269` clears the previous timer, records the actual one-shot deadline, and re-arms from the live policy. The focused timer regression passed for old-timer cancellation, manual non-postponement, and callback overlap.
- **Manual check does not postpone:** `src/rumi_cycle_sentinel/src/public_api.rs:232-239` and `:321-405` expose the transient armed deadline rather than deriving it from observations. `run_maintenance_now` does not arm a timer, and the focused regression confirms the deadline is unchanged after manual checks.

## Candid, stable state, and consent findings

- The change adds no stable-state fields; runtime timer state is thread-local and rebuilt on init/post-upgrade. Public telemetry additions are optional record fields in `src/rumi_cycle_sentinel/src/types.rs:5369-5381`.
- Generated `.did`, `.did.d.ts`, and `.did.js` files match the source declaration. The supplied strict duplex checks and endpoint check passed, including old/new compatibility checks.
- Production exports include `run_maintenance_now` and exclude test endpoints. The production artifact is distinct from the test fixture in the supplied artifact record.
- `src/rumi_cycle_sentinel/src/icrc21.rs:153-160` accurately describes immediate maintenance as potentially converting ICP, refueling Sentinel under self-recovery policy, and topping up registered targets under their target policies, while stating reserves/caps/cooldowns and no schedule/policy change. Tests at `:238-254` cover all four material phrases, including Sentinel self-recovery.

## Frontend findings

- Public `refresh()` remains anonymous/read-only (`src/vault_frontend/src/routes/explorer/telemetry/+page.svelte:363-371`); it does not call `run_maintenance_now` or create an authenticated actor.
- The manual control is rendered only for a confirmed signer/actor and its handler authenticates again before the update (`:409-439`, `:479-489`). Session generation, wallet type, principal, actor identity, and epoch are rechecked after the update and after telemetry refresh, so a changed wallet cannot display a false completion.
- The UI distinguishes scheduled checks from manual refresh, displays the live published cadence and next scheduled deadline, and labels the action as potentially refueling Sentinel or topping up targets under current limits. The built preview is honest when transport/state is unavailable.
- Funding UI explicitly separates Sentinel runtime fuel, cycles-ledger reserve, ICP reserve, ICP-to-cycles conversion, and the Sentinel owner/account destinations (`:498-527`).

## Nonblocking notes

1. The operator global-policy form still initializes `sampleInterval`/`staleAfter` to `300`/`900` at `src/vault_frontend/src/routes/explorer/telemetry/+page.svelte:116-118` and does not hydrate those fields from the live published policy. The public cadence display is live and truthful, and proposing a policy remains signer/governance/timelock controlled, so this is a stale-form UX hazard rather than a release blocker. Hydrating all policy fields from the live overview/config would reduce accidental cadence reversion.
2. The repository-wide frontend typecheck still has its pre-existing 28 errors across 23 files; the supplied scoped Sentinel tests and production frontend build pass with zero changed telemetry/service/helper errors.

## Missing or separate release evidence

- No live publication, deployment, or live canister proof was claimed. The supplied live baseline is informational and remains separate from source/build/artifact evidence.

## Final evidence closure

The previously pending full-suite condition is now closed: `/private/tmp/sentinel-cadence-full-final.log` ends with `40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1288.85s` (session `42930`, exit 0). The worktree HEAD remains `77f8445000342e13fcb0978f78d006464d8cac94`; only the already noted root-owned coordination-doc edits and untracked `target` are present. The test fixture hash independently matches the recorded `bc15f76e37ce562f7f757fe03539a4f83979f704a058a96e62f0dbb02e2ef374`, and the production artifact record still pins the same source and successful strict-Candid/export checks. Source/artifact readiness remains separate from publication and pending Proposal 17 execution.
