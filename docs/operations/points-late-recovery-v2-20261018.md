# Revised points recovery after the October 18 resume boundary

## Status

This revises the original recovery source. It is not a live restoration receipt.
The original upgrade request still lacks authoritative disposition; no retry,
alternate executor, indirect execution, replay or resume is authorized until it
is resolved. Changing source or method names does not clear that hold.

No numerical outage allocation is approved or applied. The allocation digest
remains unset. Reconstructed records cannot supply the random snapshots which
were never observed; illustrative references are not payout authorization.

## Explicit V2 transition

The original `admin_resume_held_points` and its strict epoch19 window remain.
The separate `admin_resume_points_recovery_v2` is the reviewed late path for
the verified no-previous-authorization prestate. Recovery mode cannot be switched
after a V1 schedule; an existing V1 record is not V2 coverage metadata:

1. Keep the shared single-use epoch18 recovery record, original A, seed,
   original reward bounds, min(A,B) formula, ordinary ledger and historical
   correction policy. Require held18, untouched B/close, enabled polling,
   exclusive epoch/poll guards, one fresh management draw, drained retained
   source logs and exact post-await state/binding/policy/config equality.
2. Authorize exactly one B15–30minutes later and append separate coverage
   metadata in memory19. No original State/OpenEpoch/PointEntry encoding changes.
3. Run real B18, chunked real credit18, the original unmatched correction,
   summary18 and seed18 reveal. Those real receipts remain distinguishable from
   compensation. Sources failing later retain their capture/close retry cursors.
4. After real close18, choose a current scheduled epoch with remaining duration
   exceeding `(2*ceil(registrants/100)+6)*driver_interval`, or the next epoch.
   Do not open before its scheduled start or extend the season. This is a
   conservative selection rule, not a guarantee of external source uptime.
5. At the actual synchronous opening, freeze the exact gap end, target and
   actual opening bounds, refreshed bounded registration census and seed bridge.
   The bridge digest is SHA256(domain || predecessor18 || real summary hash ||
   predecessor successor commitment || missing-start || target-index || actual
   opening || epoch-end). Integers are little-endian u64. The target seed is
   SHA256(domain || private preloaded successor seed || bridge digest). The
   private successor used the fresh close18 entropy. Reveal that entropy only
   when the target closes. No retry redraws B or a normal target seed.
6. No ordinary summary or seed reveal is fabricated for a skipped epoch.
   Logical seed/summary lookups use binary search by epoch ID. Public history
   offsets and counts still paginate real records, so offsets may differ from
   scheduled IDs after this explicit gap.

The domain is `rumi-points-late-recovery-bridge-v2` followed by a null byte.
The public coverage query reveals commitments, actual bounds and the census,
never private entropy or future capture timestamps. A changed season bound or
an exceeded100-person total registered census fails closed for explicit review
(including registrants added after reopening).

## Disjoint accounting

An immutable reviewed manifest must contain every audited participant/period
row, including zero rows, in canonical epoch/principal order (at most512rows):

- Epoch18: whole scheduled-epoch floor, minus all actual ordinary18credits.
- Completely missed epochs: whole scheduled-epoch floors, using explicit gap
  proof instead of fabricated normal summaries; offset any real ordinary credit.
- A partially reopened epoch: only `[scheduled_start,actual_open)` is eligible
  for the outage target. Its row must specify these exact coverage bounds.
  Later normal `[actual_open,epoch_end)` credits never offset this prefix.

Thus a partial epoch's outage reconciliation need not wait for its normal close.
The same message commits the opening and the cutoff, leaving no timing gap.
The opening census is an observation, not a complete eligibility seal. Staging
holds both writer guards, checks current retained source tails and exact
post-await state/binding/config equality, and rebuilds the eligible census from
all current registrations timestamped before the immutable opening cutoff.
Each continuation rejects any subsequently discovered pre-cutoff registration
missing from its immutable manifest; it cannot claim completeness or silently
omit that participant. Such a discovery requires an explicitly reviewed
replacement/supplemental allocation; this draft does not grant it automatically.
Source retention, append timestamp/provenance and final manifest evidence must
be reviewed; a sequence of non-atomic tail queries is not a certified global tip.
New registrations discovered after reopening are therefore admitted at staging;
ineligible participant/period targets must be zero. Original15registration
records and all417original rows remain bound by the unchanged original-prefix
hash chain. Repayment eligibility, dust, historical source/valuation gaps and
unobserved captures still need explicit allocation treatment.

Progress is durable and bounded: at most100ordinary rows scanned per call or
one participant-period receipt applied. Compensation remains a separate audit
kind/log, and synchronous total+credit+receipt writes are atomic. Repeated calls
do not add credits again. No existing points are subtracted.

## Evidence and completion conditions

Compile and actual native Candid export are checked separately from a release
build, live execution and observed captures. No tests were added or run in this
revision. Before execution, revalidate exact candidate, identity/controller,
sole owner, current state, retained source coverage and a fresh backup after
authoritative original disposition. Preserve the original request receipts.

Completion requires actual processing/captures/normal epoch evidence and a
reviewed applied allocation whose receipts cover every missed interval. A
timer flag, source commit, gap record or the original417rows alone is insufficient.
