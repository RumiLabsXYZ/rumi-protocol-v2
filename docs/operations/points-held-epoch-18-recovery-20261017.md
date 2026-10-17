# Held epoch 18 points recovery

## Repair

The production points canister retained snapshot A for epoch 18 and held its
legacy public-seed transition. Its driver was disabled. The earlier prepared
resume methods were never deployed, and the original epoch end has now passed.

This repair adds an admin-only retained-event replay and one delayed snapshot B
15–30 minutes after secure management-canister randomness and successful source
tail validation. Epoch 18 keeps its original reward bounds, snapshot A, original
seed provenance, existing A/B minimum rule and fiat-stable uplift policy. It does
not award extra days for the delay. Epoch 19 uses the existing secure seed path
with its opening rebased to the actual opening time.

A single versioned record in new stable memory 15 records the legacy exception.
Existing memories 0–14 and frozen State/OpenEpoch layouts are unchanged. The
replacement schedule is hidden publicly until B completes or the epoch closes;
the audit record survives close. Repeating resume cannot resample B.

Replay propagates source errors, verifies retained rotating-log coverage and
requires all source cursors at their observed tails before resume. A final
post-await comparison rejects concurrent configuration or state changes.

Capture rejects zero, future or older-than-600-second cached ICP timestamps,
including after source awaits and before each principal's snapshot write. AMM
transport/decode errors retry capture instead of being treated as zero balances.
The existing scoring formulas and audit rows are unchanged.

## Release boundary

- Target: points canister `bfnu3-6aaaa-aaaab-qhanq-cai`, upgrade mode only.
- Base source: `4465e618c30578bb402151bf2dce7b350952b0b1`.
- Raw Wasm SHA-256: `45bd7eb5a4cb4b551311703ea4550f6d30acab8e6e38b92ecffb3ebc68508ab1`.
- Compressed upload SHA-256: `fbf182b8960512be1e22684aea9bc611098e62440ef7634fa204f861c19b7b1a`.
- Raw module 2,570,051 bytes; executable code 2,085,140 bytes; gzip 643,701 bytes.
- Locked library/binary compilation and two canonical points builds succeeded.
- Generated Candid is embedded exactly, backward compatible, with exact modes
  for all 40 methods. Existing generated JS/TS declarations were regenerated.
- No tests were added or run during this repair.

## Execution and proof

Before upgrade: verify identity/controller, original module and held epoch;
record all 417 existing audit rows and current totals; stop points and take a
fresh snapshot. Preserve all older snapshots. Upgrade with the exact compressed
artifact and read back its hash and preserved state before replay.

Replay bounded retained-event batches, verify cursor tails, then call the
one-time resume. Verify driver enabled/hold cleared and public B redacted.
Observe B completion, epoch 18 close, new audit rows and secure epoch 19 opening
before claiming points processing is restored. Preserve the original 417 rows.

Stale source prices can delay capture. They must not be overridden or replaced
with invented values. Any backend oracle source repair is a separate scope;
this release does not mutate its timer, mode, cap or financial state.
