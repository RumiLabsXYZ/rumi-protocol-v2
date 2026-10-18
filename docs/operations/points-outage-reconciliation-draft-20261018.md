# Points outage reconciliation draft

Status: partial local implementation; **not a final allocation or release**.
Points processing remains held. The original request disposition still forbids
another live upgrade, replay, resume or credit. No production call is part of
this draft.

## Purpose and current scope

The requested result includes points for the paused period. The existing
PR583 resume path preserves old rows but cannot by itself cover epoch 19's late
opening. Some random snapshot inputs were never observed. See the separate
accounting evidence and `points-makewhole-policy-proposal-20261018.md`.

This module implements application of a future immutable allocation. It does
not determine its economic policy. Neither the proposed policy nor any
worksheet is approved, sealed or applied. Resolving numerical compensation
must not become an extra prerequisite to restoring the reviewed normal driver
once the original execution hold and its actual release prerequisites clear.

## Application mechanism

A future reviewed allocation binds the full scheduled epochs 18 and 19, exactly
15 unique original participants in each (including zero targets), season bounds,
an evidence digest and the original 417-entry normal-ledger prefix digest.
Rows are canonical by epoch and principal. Domain-separated Candid encoding
identifies the immutable allocation.

Each row is a **total epoch floor**. Every ordinary credit already earned in
that epoch offsets it. The addition is `max(target - already_earned, 0)`; existing
excess is preserved. The ordinary earning sources are never rewritten.

The first draft attributed additions to ordinary PointSource categories. That
representation is superseded: a ceiling allowance for an unobserved snapshot
is compensation, and cannot be presented as an observed debt award or as the
existing `floor(original_unmatched /3)` correction.

The new separate append-only credit log labels each addition `OutageMakeWhole`.
It records allocation/evidence digests, participant, epoch, scheduled coverage,
the frozen normal-ledger cutoff, normal points, target, added points and actual
application time. Original PointEntry/PointSource types and readers retain
their existing contract. Once integrated, the complete aggregate identity is:

`principal total = normal audit-ledger credits + separate outage credits`.

A future public audit projection and frontend breakdown must show this second
ledger; ordinary-ledger rows alone must not be claimed to decompose the new
total. No such ingress or frontend integration exists yet.

Staging requires both epochs to have actually closed, their summaries to match
the scheduled end, and the established legacy-18 / flat-4x-19 cutover and original
corrections to be complete. This mechanism cannot apply credits before epoch 19
closes. Normal resume is a separate step. If delay extends beyond epoch 19,
coverage must be extended explicitly before claiming completion.

Each call scans at most 100 frozen normal-ledger rows **or** applies one
participant/epoch row. The original prefix chain starts at SHA256 of
`rumi-points-october-2026-original-ledger-v1\0`; for offsets 0 through416 it hashes
previous digest || offset as little-endian u64 || `candid::encode_one(PointEntry)`.
Both the digest and 15 original registration principals must match before any
credit. This is a canonical logical-entry binding, not a hash of raw stable pages.

A credit's separate audit append, aggregate principal increment and durable
receipt are synchronous, without await or returned error after the first write.
Retries use the stored application cursor; zero additions still receive a
progress receipt. The separate credit-log count must agree with durable receipts.
A log found without its progress record fails closed instead of starting over.

## Compatibility and disabled state

- Existing State, OpenEpoch, PrincipalState, PointEntry, seed, cursor, policy,
  snapshot and version wrappers are unchanged. Normal ledger stays append-only.
- New MemoryId16 holds progress; new17/18 hold the outage audit log index/data.
  These draft types have never been installed. Earlier draft layouts are not
  claimed to have been deployed or to be frozen release schemas.
- No new PointSource variant or existing IC signature is changed.
- Mutation helpers require the existing nonanonymous admin predicate, both
  writer guards, checked totals and the immutable approved allocation digest.
- The digest pin is **unset**, rejected before lazy progress initialization.
  No main.rs ingress reaches staging, continuation or the new query helpers.
- Existing timers, hold and the original resume timing safeguard are unchanged.
- No tests were added or run. Locked offline native compilation is not
  production artifact, retained-state, interruption or live activation proof.

## Still required

Finish and accept the economic policy; close the numerical/historical exceptions;
seal a complete allocation; configure its digest; integrate bounded ingress,
audit projection and frontend; verify actual retained state and restart/reply-loss
behavior; produce exact compatible artifacts; obtain original-request disposition;
and observe normal resume/epoch advancement and the reconciled ledger.

Do not merge or install this draft as a complete repair. The separately frozen
original release remains distinct from this changing draft source.
