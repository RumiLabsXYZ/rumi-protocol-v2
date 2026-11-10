# Vault 51 raw collateral guard correction — November 10

Owner: Adrian (source); Dorian (independent release/replacement decision).
This is the concrete first-timer defect, not a new audit or financial action.

## Cause and bounded correction

The installed source42478174 requires `vault.collateral_amount == 0`.
`CandidVault::from_state` projects zero for a fenced claim, but the private claim
event deliberately retains the original raw nICP collateral. I incorrectly used
the public projection as the raw-state invariant. The prior replay fixture also
used zero and therefore missed this real state.

Dorian independently verified the original raw amount150000000000 against the
unique vault51 record in stopped snapshot1a of the prior2ca84c20 backend. The
owner/type/ID/debt match and bot_processing is false. Per his release readback,
the first timer holds before settlement reads or any escrow sweep; frontend
activation remains held. This source work performs no live query or mutation.

Require raw collateral to equal the already retained `claim.original_nicp_e8s`
exactly. Preserve the approved claim binding, owner, nICP type, native ICP
custody/config, bot-state and payout-obligation guards. Do not remove the amount
guard or allow arbitrary/zero amounts. No fee, receipt, transfer, journal, API,
state-shape, timer cadence or final custody/index transition changes.

## Regression and evidence

A realistic nonzero-raw fixture must pass the initial guard while its Candid
projection remains zero and state/claim fencing is unchanged. Changed owner,
type, amount (+1/-1/zero) or bot_processing must still be refused. The existing
replay/once-only/debt/index regression now starts from original raw nICP rather
than a synthetic zero, exercising the completion guard too.

The production change is one predicate: `vault.collateral_amount !=
claim.original_nicp_e8s` replaces `!= 0`, with a clarifying comment.

`cargo test --locked -p rumi_protocol_backend --lib nicp -- --test-threads=1`
on corrected source: **25 passed, 0 failed**. No further tests/builds are queued.
The old predicate failed the two added realistic-state tests and the updated
nonzero-raw replay fixture (22 passed, 3 failed), consistent with Dorian's live
timer and frozen-heap reproduction. An initial test compile encountered a stale
shared `rumi_3pool` cache before tests ran; refreshing its unchanged source mtime
resolved it. No dependency/source change was made to that crate. The live/snapshot
failure is sufficient reproduction; do not repeat a red compile for release.

Candid, generated bindings, state schemas, all other production source and all
frontend sources/manifest are unchanged. Reuse those valid proofs. No regeneration
or frontend rebuild is necessary for this predicate/test-only change.
Guarded cleanup found zero eligible worktrees and removed zero.

## Release boundary

No replacement production build, installation, timer activation, custody
transfer, frontend activation, customer action or new review team was run.
The existing artifact remains deployed and fenced per Dorian's readback; its
XRP pre-install reconciliation stays with his release lane. Source/tests/evidence
are supplied for his independent correction review. A replacement artifact and
exact install decision are prepared separately; prior artifact/build receipts
do not prove this changed predicate is deployed.
