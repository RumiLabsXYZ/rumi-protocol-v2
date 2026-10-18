# Points outage reconciliation draft

Status: partial local implementation; **not a final allocation or release**.
Points processing remains held. The original request disposition still forbids
another live upgrade, replay, resume or credit. No production call is part of
this draft.

## Why this code is needed

The requested result includes credits for the paused period. The existing
PR583 resume path preserves old rows but cannot by itself cover the late opening
of epoch 19. Its A/B captures also cannot reconstruct inputs that were never
observed. The accounting evidence at `points-no-gap-accounting-evidence-20261018.md`
therefore requires a separate immutable make-whole allocation.

This draft implements the **application mechanism**, leaving the economic
allocation explicitly unfinished. It does not turn the illustrative references
into credits. In particular, current debt plus repayments is still not a strict
historical bound because of unlogged dust forgiveness.

## Implemented mechanism

`src/rumi_points/src/outage_reconciliation.rs` accepts a future reviewed allocation
for the full scheduled epochs 18 and 19, with 15 unique participants in each,
including zero-target rows. The allocation is ordered by epoch and principal and
binds the evidence digest, season and original 417-row ledger prefix digest.
Candid encoding plus a fixed domain defines its immutable SHA256 digest.

Each row specifies a total epoch floor through an explicit source breakdown.
Existing credits **in every source** offset that total floor. The addition is
`max(sum(target) - sum(already_earned), 0)`. It is attributed deterministically
only to positive source deficits, never to registration. If another source
already earned more, that excess is preserved and reduces the addition; the
code does not promise independent extra floors in every source.

Staging requires actual closed summaries for both epochs and the established
legacy-18 / flat-4x-19 cutover and completed original corrections. Epoch 19 may
have a later actual start, but its full scheduled interval is what the final
allocation must cover. This prevents overlap with normal rewards: original
credits are subtracted after close, rather than adding a second full award.
It also means final reconciliation cannot be applied before epoch 19 closes.
A future integration must separately resume processing and cover any later
missed interval if delay extends beyond these two epochs.

The mechanism scans at most 100 frozen ledger rows per call. It verifies every
original prefix entry using the following hash chain:

1. Start with SHA256 of `rumi-points-october-2026-original-ledger-v1\0`.
2. For each original offset 0 through 416, replace the digest with
   SHA256(previous digest || offset as little-endian u64 ||
   `candid::encode_one(PointEntry)`).
3. Require the result and the 15 original registration principals to match the
   reviewed allocation before writing any credit.

Then each call applies at most one participant/epoch row. Appends, aggregate
principal total and receipt commit in one synchronous IC message, without an
await. A retry uses the stored cursor/receipt. Applied receipts retain added
source amounts, original ledger offsets and actual application time. They
identify adjustments separately from original snapshot observations.

## Compatibility and current safety boundary

- Existing State, OpenEpoch, PrincipalState, PointEntry and version wrappers
  are unchanged. The original ledger remains append-only.
- Only new MemoryId 16 is reserved for versioned reconciliation progress.
- Deposits, repayment windows, last processed epoch, source cursors, A/B
  snapshots, seed chain, existing timing safeguard and timers are unchanged.
- Existing PointSource variants and all IC method signatures remain unchanged.
- Both mutation functions require the existing nonanonymous admin predicate,
  epoch/poll writer guards and checked arithmetic.
- The allocation digest pin is **unset**. Staging and continuation therefore
  fail closed before accessing lazy progress storage. There is no main.rs ingress for these functions. The draft
  cannot currently be invoked through the IC interface.
- No tests were added or run. A locked offline native compilation covers the
  library and binary; it is not production artifact or retained-state proof.

## Remaining work before use

1. Finish the numerical allocation from retained evidence, including dust,
   missing historical observations, time bounds, registration and eligibility.
   The original economic policy remains unchanged unless an explicit make-whole
   exception is reviewed and accepted.
2. Freeze readable input/allocation manifests and independently verify amounts.
   Only then configure the immutable digest and add reviewed bounded ingress
   plus an audit projection. Include generated Candid compatibility proof.
3. Verify actual snapshot preservation and reconciliation accounting, restart,
   reply-loss and replay behavior. Native compilation alone does not prove them.
4. Integrate the normal resume sequence, the mandatory timing safeguard and
   coverage after epoch 19 if needed. This draft does not resolve the execution
   request hold and is not a substitute for observed normal epoch advancement.

Do not merge or install this draft as a complete recovery, claim no-gap coverage,
or describe any allocation as applied. Existing reviewed release binaries remain
frozen separately from this source draft.
