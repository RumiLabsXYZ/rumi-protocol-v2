# Sentinel cadence/manual-check A2 review

## Verdict

**PASS for source, interface, frontend, and production-artifact review; no confirmed blocking issue.** The final 40-test source integration run was still non-terminal when this review was written, so its completion remains a separate release condition. No live publication or live-data claim is made here.

## Scope and subject

- Worktree: `/Users/robertripley/.codex/worktrees/sentinel-funding-live/rumi-protocol-v2`
- Branch/source under review: `codex/sentinel-four-hour-checks` at `77f8445000342e13fcb0978f78d006464d8cac94`
- Actual baseline: `2b11bbfc39f68e42700fcef9141e74f2ef47c193`
- Review was read-only. The worktree's dirty coordination files and `target` entry were excluded; no source or live call was made.

## Rubric results

### Authorization and consent: PASS

`src/rumi_cycle_sentinel/src/lib.rs:197-223` rejects anonymous callers before signer lookup and before entering `sampler::run_maintenance_now`; non-signers receive `NotSigner`. The PocketIC focused cases cover anonymous and non-signer calls with zero observation/funding/source calls. `src/rumi_cycle_sentinel/src/icrc21.rs:147-160` adds an explicit `run_maintenance_now` consent message that names ICP conversion, Sentinel self-recovery refueling, target top-ups, and the fact that automatic policy/schedule are unchanged. The consent unit test checks all of those disclosures.

### Shared single-flight and timer behavior: PASS

`src/rumi_cycle_sentinel/src/sampler.rs:125-158,223-314` uses one shared `TickGuard` for automatic and signer-triggered maintenance, holds it across all awaits, releases it in `Drop`, re-arms the one-shot timer before work, and records the actual callback-clock deadline. `src/rumi_cycle_sentinel/src/governance.rs:520-529` re-arms after a durable global-policy write, clearing the old timer. The four focused PocketIC cases cover overlap/busy behavior, old-timer cancellation, manual checks preserving the deadline, and the next automatic pass. Manual checks do not change the deadline.

### Funding and governance invariants: PASS

`sampler.rs:316-461` retains the existing maintenance order: self-recovery, pending-operation resume, target sampling, source-cache refresh, then ordinary funding. Ordinary funding enters `funding::cycles::run_ordinary_with_outcome`; `funding.rs:65-128,309-414` continues to enforce target enabled/observed/low state, auto-top-up for the automatic lane, stale sample bounds, one-operation exclusion, target/global caps, protected reserve, and durable reservations. `sampler.rs:417-483` admits ICP conversion only after the existing proven no-spend/insufficient-reserve gates. The focused cap/reserve/refill case shows the signer-triggered pass cannot exceed the target cap, and the consent/UI wording does not promise unconditional delivery.

### State, Candid, and generated declarations: PASS

The only public projection additions are optional `sample_interval_secs` and `stale_after_secs` fields in `types.rs:5341-5381`; runtime timer deadline remains transient. No stable state shape was changed. The new update method is additive (`run_maintenance_now`) and uses `MaintenanceResult` (`Ok`/text `Err`). The source `.did`, checked-in declarations `.did`, `.did.js`, and `.did.d.ts` all contain the same additions. The recorded production artifact checks report strict duplex Candid compatibility in both directions and `ic-wasm check-endpoints` exit 0.

### Frontend truth and wallet/session safety: PASS

`src/vault_frontend/src/routes/explorer/telemetry/+page.svelte:363-439` keeps public refresh anonymous/query-only, shows the backend-published cadence/deadline, requires the confirmed signer actor for the manual action, and rechecks epoch, wallet session, actor identity, and signer state before the update, before refresh, and before claiming completion. Busy and other backend errors are preserved by `cycleSentinelService.ts:197-200`; the contract tests assert no false completion. The built preview (`/private/tmp/sentinel-cadence-built-preview.png`) shows the unavailable/failed transport state honestly and does not claim a four-hour schedule without live telemetry. The frontend build and 24 contract tests passed.

### Production artifact and scope: PASS

`/private/tmp/sentinel-cadence-production-artifact.json` identifies source `77f84450`, production raw artifact SHA-256 `20241bf7f2144226f5601301dfc7c837f6c9ca89da058f6e0571437953507102`, build exit 0, frontend build exit 0, and test-fixture artifact separation. Production endpoint checking passed with the intended runtime export list, so no test controls were included in the production Candid surface. The commit diff is limited to the cadence/manual-check docs, Sentinel implementation/tests/Candid, and the telemetry frontend/services/tests.

## Deterministic evidence observed

- Native/unit suite: **615 passed, 0 failed** (`/private/tmp/sentinel-cadence-native-tests.log`).
- Consent suite: **5 passed, 0 failed** (`/private/tmp/sentinel-cadence-consent-final-tests.log`).
- Frontend contract/service suite: **24 passed, 0 failed** (`/private/tmp/sentinel-cadence-final-frontend-tests.log`).
- Focused source integration suite: **4 passed, 0 failed** (`/private/tmp/sentinel-cadence-focused.log`).
- Production and frontend builds: exit 0 (`/private/tmp/sentinel-cadence-production-artifact.json`).
- Strict duplex Candid and production endpoint checks: exit 0 (`/private/tmp/sentinel-cadence-production-artifact.json`).

## Blocking issues

None found in this independent A2 source/artifact review.

## Non-blocking notes and missing evidence

- The final 40-test source integration suite was initially pending during the first review pass; its terminal passing result is recorded in the closure below.
- The built preview intentionally has no local transport and displays `Failed to fetch`; it is UI/layout evidence only, not live-data proof.
- Fresh live baseline and proposal state were supplied separately: 16 enabled/auto-top-up targets, 17 executed proposals, proposal 17 open, no unresolved operations, and zero receiving ledger balances. No publication, execution of proposal 17, or live-data verification was performed by this review.
- The repo-wide frontend typecheck retains pre-existing errors in 23 files; the provided changed-surface tests/build passed and no changed telemetry/service/helper error was identified.

## Final full-suite closure

The previously pending condition is now closed: `/private/tmp/sentinel-cadence-full-final.log` terminates with **40 passed, 0 failed, 0 ignored, 0 measured, finished in 1288.85s**. The source worktree remains at `77f8445000342e13fcb0978f78d006464d8cac94`, and the test fixture remains SHA-256 `bc15f76e37ce562f7f757fe03539a4f83979f704a058a96e62f0dbb02e2ef374`. Only root-owned coordination files are dirty in the worktree; no source edit occurred after the reviewed commit.

## Release boundary

The reviewed source/artifact path is clear of confirmed A2 blockers, and all listed deterministic suites are now terminal and passing. Do not represent proposal 17 as executed or the four-hour cadence as live until the authoritative second approval/timelock execution and fresh live readback occur.
