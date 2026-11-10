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

## Source-owner handoff boundary

At this source-owner checkpoint, no replacement production build, installation,
timer activation, custody transfer, frontend activation, customer action or new
review team was run. Dorian’s subsequent backend-only build and exact replacement
decision are recorded below.
The existing artifact remains deployed and fenced per Dorian's readback; its
XRP pre-install reconciliation stays with his release lane. Source/tests/evidence
are supplied for his independent correction review. A replacement artifact and
exact install decision are prepared separately; prior artifact/build receipts
do not prove this changed predicate is deployed.

## Exact replacement decision — HELD

Dorian completed the backend-only production build from corrected source
`312ebe036a7651365c293294bedc8cb3a6585179`. The independent release check confirms
the production diff changes only the original-amount predicate and comment;
other edits are realistic test fixtures. Twenty-five focused tests pass,
including exact-original admission, owner/type/bot and +/-1/zero amount refusals,
and the once-only completion/replay test starting with actual retained raw nICP.
The prior custody/receipt/journal review is reused; no general review campaign
or frontend rebuild is required. No live corrected timer/custody outcome is claimed.

| Frozen input | Bytes | SHA256 |
| --- | ---: | --- |
| Optimized backend | 9,813,788 | `2286e0873086279ce632995330ccbab59116fd8f01f0eb4dc5e5edfeea6d3b03` |
| Gzip upload | 2,378,535 | `730f54713dca1938c40b661ef6bd6ddc23555d92ca4c8e1feca282e3a33f8cf9` |
| Canonical/embedded Candid | unchanged | `43da24f0341ca8fe52837071020e910f9ff3ecdef83a04f0dbc8f6e3033334db` |
| Exact upgrade args | file-backed | `f0e7ff2303b1e0225be7cce48783e99b39d9e18c9c588ec8fe002118aeff0bd6` |
| Reused 740-file frontend manifest | unchanged | `79e75bd3d0f6016b4d4ae968a83d79b6b558dcf35828c595aad0b6e96afac6a3` |

Gzip decompresses exactly to the optimized module. Executable code is 8,880,664
bytes, within the unchanged 10,223,616-byte project guard. Embedded Candid and all
397 public methods match; only the already existing internal timer executor is
additional. Exact args encode against canonical `ProtocolArg`, retain
`mode = null` and name the corrected source/automatic timer. See
[release pins](evidence/vault51-correction-release-20261110/release-pins.json),
[artifact report](evidence/vault51-correction-release-20261110/artifact-report.json),
[actual artifact verification](evidence/vault51-correction-release-20261110/artifact-verification.json)
and [upgrade args](evidence/vault51-correction-release-20261110/upgrade-args.did).

**Decision requested:** approve one corrective upgrade of existing mainnet
backend `tfesu-vyaaa-aaaap-qrd7a-cai` from currently Running upload
`43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09`
to the exact corrected gzip above, using `rumi_identity`, upgrade mode and the
pinned args. This replacement re-registers the hourly/+10-second checks and
permits the already described eventual verified settlement sweep and same-vault
ICP adoption. It adds no financial scope, new withdrawal, owner payout, repayment,
controller/setting change or manual claim/flag clearing. The artifact differs
from the specifically approved `9d02bea8` packet, so this corrective installation
remains held for an exact decision rather than silently substituting new bytes.

Reuse the [approved preservation and financial procedure](vault51-release-review-20261110.md#exact-release-decision-for-rob--held)
and the [observed live receipt](vault51-live-release-20261110.md):

1. Freshly recheck module/settings/controllers, snapshot inventory, private claim,
   raw vault/owner/debt/interest/custody and SEC11/SEC12/158 retained state. Do not
   infer raw state from the projected zero. Drift requires reconciliation.
2. Stop once and verify controller Stopped. The next oldest slot currently is
   `00000000000000110000000001f088fe0101`; its complete read-only local archive
   already verifies. After fresh inventory confirmation, replace only that archived
   slot with a new stopped-state backup, download/verify it against the current
   module and preserve the other nine, including prior release backup 1a.
3. Install only the pinned corrected gzip/args in upgrade mode once. Independently
   verify the uploaded hash/API/settings while stopped, then start once. No
   reinstall, restore, old-writer rollback or uncertain write retry is included.
4. Verify the first automatic check passes the original-vault guard. Separately
   report pending WaterNeuron payout, verified settlement, verified sweep and
   completed adoption. A pending-claim timestamp may advance from the intended
   reconciliation; actual financial changes require exact immutable receipt
   proof. Preserve all liabilities, other claims and completed vault158 work.
5. Complete the already approved frontend activation on existing
   `tcfua-yaaaa-aaaap-qrd7q-cai`: stage686 is already independently verified, so do
   not stage it again. Freshly compare its full inventory against that stage;
   activate only the same seven index/version content sets after backend
   readback passes. Reuse the exact original bundle/manifest and additive
   publisher, updating its paired-backend pin to the corrected upload. Verify
   full inventory, all preserved assets/properties/permissions, served changed
   bytes and primary-domain auth assets.

Unknown outcomes stop for readback before any further write. No manual
settlement/reconciliation transfer is authorized by this replacement decision.
Escrow was zero with no settlement/adoption journal at the last release readback;
it is not assumed frozen until a future approval. If an actual payout arrives,
the exact verified automatic sweep/adoption scope remains the one Rob selected.
