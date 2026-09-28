# Cycle Sentinel four-hour schedule and manual checks

Rob requested automatic checks every four hours and a manual refresh button, following approval of publication and activation of the funding feature.

## Policy change

The exact governed argument is `sentinel-four-hour-policy-2026-09-28.did`. Fresh signer-authenticated mainnet `list_governance_proposals` returned 17 executed proposals; proposal 0 is the sole executed global policy. The change preserves all its fields except sample_interval_secs 3,600→14,400 and stale_after_secs 7,200→28,800. The stale window must be at least the interval (`types.rs`, GlobalPolicy::validate); preserving the existing two-interval relationship avoids marking normal between-check readings stale.

Global cap 40T, protected reserve 10T, ICP reserve 0, runtime threshold/refill 1T and cap 10T, timelocks 86,400/86,400/86,400/172,800 seconds, four signers and threshold2 stay unchanged. Target thresholds/refills/caps/cooldowns are untouched.

Creating and approving the proposal records one signer approval and cannot activate the change. It needs a second distinct signer and the existing 86,400-second waiting period before execution. Only robvector is currently available locally. No upgrade, test endpoint or policy default may bypass these requirements.

## Manual check contract

`run_maintenance_now` is an authenticated update for configured signers. It shares the existing timer single-flight guard and maintenance path, may convert ICP or deliver cycles under the stored policy, and rejects overlapping checks. It does not reset or postpone the repeating timer. Public Refresh telemetry reloads cached readings without initiating maintenance. Wallet consent must accurately describe these possible effects.

The page shows the actual active interval and actual next scheduled deadline. The four-hour policy is pending until authoritative execution; the UI must not claim it is active earlier.

Verification and publication evidence will be appended after implementation.

## Live cadence proposal

Exact policy was pushed at 4f43a0ec; argument SHA-256 is b2ebe76efacabaa8b2cf3ab9a7580e3e8fab96bd31a32d03eafad29044661441. A bounded independent review compared all fields against the fresh executed-policy snapshot and returned PASS.

Mainnet `propose_set_global_policy` returned Ok 17; explicit robvector `approve_proposal(17)` returned Ok true. Fresh authenticated readback shows proposal 17 Open with exactly one distinct approval, created 1790618157. The earliest existing 24-hour timelock expiry is 2026-09-29T17:55:57Z (10:55:57 Pacific). A second configured signer must approve before execution. Cadence remains hourly until execution. Raw local receipts are /private/tmp/sentinel-cadence-propose.json, -approve.json and -after-proposals.json. Creating/approving this policy caused no cycle delivery or ICP conversion.

## Verification checkpoint

Implementation is frozen at `77f8445000342e13fcb0978f78d006464d8cac94`, based on current main `2b11bbfc39f68e42700fcef9141e74f2ef47c193`. Backend and frontend ownership handoffs are complete. Native tests passed 615/615; after the consent wording correction, all five ICRC-21 tests passed again. The final focused frontend run passed 24/24. Both final production builds exited zero. The full real-canister integration suite is running with the final fixture; four new maintenance regressions already passed against the preceding functional source.

Production Candid extracted from the fresh Wasm matches the canonical interface in both directions, preserves the previous service, and its endpoints match the production interface with only the known CDK lifecycle exports exempted. No test controls are exported. Final raw artifact SHA-256 is `20241bf7f2144226f5601301dfc7c837f6c9ca89da058f6e0571437953507102`; deterministic gzip SHA-256 is `6f94d733548f65e1ec5e46309b2aa52ffd8de35ad68c544d625984927f57143a`.

The first source review round returned one PASS and one confirmed consent omission: immediate checks can refuel Sentinel under its separate self-recovery policy. The UI and wallet message now explicitly describe that effect along with ICP conversion and target top-ups. The fresh final review round remains pending. Source verification does not claim publication or live availability.

## Second review round

Two fresh independent reviewers returned PASS for frozen source, interface, frontend and production artifacts, with no confirmed blockers. Their readable reports are `sentinel-cadence-review-a2-2026-09-28.md` and `sentinel-cadence-review-b2-2026-09-28.md`. They explicitly retained the terminal full-suite result and live publication as separate evidence.

The existing advanced global-policy form starts from default values rather than hydrating every field from current configuration. That was classified as a nonblocking UX hazard; improving it is deferred groundwork and does not bypass or replace the existing proposal, two-signer and timelock requirements. The current public schedule always comes from authoritative telemetry.

Proposed durable persona lesson: publish the actual armed timer deadline as transient runtime state. Deriving it from the last observed target balance makes manual checks appear to postpone scheduled work. This source records the deadline when arming, and the real-canister regression proves manual maintenance does not alter it.

## Final deterministic gate

The final full real-canister suite terminated with exit 0: **40 passed, 0 failed**, 1,288.85 seconds, session42930. It used frozen source `77f8445000342e13fcb0978f78d006464d8cac94`, Sentinel test fixture `bc15f76e37ce562f7f757fe03539a4f83979f704a058a96e62f0dbb02e2ef374`, and the unchanged mock `77fed5805cdaabc2f29dcd6825181a341a787e8c73721713aaf0b21860565a36`. All four new maintenance regressions and all existing funding/recovery regressions passed. Production raw/gzip hashes were rechecked before publication and match the recorded build.

The PR has no configured GitHub status checks; the exact local terminal checks and independent reviews form this release evidence. Mainnet publication remains limited to upgrading Sentinel and syncing frontend assets; no other canister deployment, controller change or target-policy mutation is included.
