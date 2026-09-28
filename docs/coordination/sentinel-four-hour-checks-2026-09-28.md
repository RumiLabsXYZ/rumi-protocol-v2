# Cycle Sentinel four-hour schedule and manual checks

Rob requested automatic checks every four hours and a manual refresh button, following approval of publication and activation of the funding feature.

## Policy change

The exact governed argument is `sentinel-four-hour-policy-2026-09-28.did`. Fresh signer-authenticated mainnet `list_governance_proposals` returned17 executed proposals; proposal0 is the sole executed global policy. The change preserves all its fields except sample_interval_secs3,600→14,400 and stale_after_secs7,200→28,800. The stale window must be at least the interval (`types.rs`, GlobalPolicy::validate); preserving the existing two-interval relationship avoids marking normal between-check readings stale.

Global cap40T, protected reserve10T, ICP reserve0, runtime threshold/refill1T and cap10T, timelocks86,400/86,400/86,400/172,800 seconds, four signers and threshold2 stay unchanged. Target thresholds/refills/caps/cooldowns are untouched.

Creating and approving the proposal records one signer approval and cannot activate the change. It needs a second distinct signer and the existing86,400-second waiting period before execution. Only robvector is currently available locally. No upgrade, test endpoint or policy default may bypass these requirements.

## Manual check contract

`run_maintenance_now` is an authenticated update for configured signers. It shares the existing timer single-flight guard and maintenance path, may convert ICP or deliver cycles under the stored policy, and rejects overlapping checks. It does not reset or postpone the repeating timer. Public Refresh telemetry reloads cached readings without initiating maintenance. Wallet consent must accurately describe these possible effects.

The page shows the actual active interval and actual next scheduled deadline. The four-hour policy is pending until authoritative execution; the UI must not claim it is active earlier.

Verification and publication evidence will be appended after implementation.

## Live cadence proposal

Exact policy was pushed at4f43a0ec; argument SHA-256 is b2ebe76efacabaa8b2cf3ab9a7580e3e8fab96bd31a32d03eafad29044661441. A bounded independent review compared all fields against the fresh executed-policy snapshot and returned PASS.

Mainnet `propose_set_global_policy` returned Ok17; explicit robvector `approve_proposal(17)` returned Oktrue. Fresh authenticated readback shows proposal17 Open with exactly one distinct approval, created1790618157. The earliest existing24-hour timelock expiry is2026-09-29T17:55:57Z (10:55:57 Pacific). A second configured signer must approve before execution. Cadence remains hourly until execution. Raw local receipts are /private/tmp/sentinel-cadence-propose.json, -approve.json and -after-proposals.json. Creating/approving this policy caused no cycle delivery or ICP conversion.
