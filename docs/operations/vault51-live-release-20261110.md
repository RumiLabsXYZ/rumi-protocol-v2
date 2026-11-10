# Vault 51 live release — November 10, 2026

Rob directly approved exact packet `9d02bea8`, including installation, automatic
checks and the eventual verified escrow sweep and ICP collateral adoption.
Dorian executed the bounded backend release. **The backend is deployed and
Running, but automatic adoption is held by a confirmed source defect. Frontend
new assets are staged; entry-point activation is paused. No ICP sweep or adoption
has occurred.** The corrected source/artifact will need an exact replacement
release decision; the approved artifact is not silently substituted.

## Installed bytes and preservation

Source `42478174029e6edb54e26578bebfe79a25f576bc` was installed on existing mainnet
backend `tfesu-vyaaa-aaaap-qrd7a-cai` using `rumi_identity`, upgrade mode and the
pinned `mode = null` args. Running module hash (the uploaded gzip bytes) is
`43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09`.
It decompresses exactly to optimized module
`941224b5ae5878e889e8fc864887f2b22e18949d08f6a9ec700204519d07d5d8`.
Actual installed Candid equals the accepted canonical API. Existing settings,
controllers and mode are preserved. No rebuild or repeat source review preceded
this exact approved deployment.

| Step | UTC on November 10 | Result |
| --- | --- | --- |
| Stop and drain | 22:37:59–22:38:33 | Controller Stopped confirmed |
| Replace archived oldest slot | 22:38:36–22:38:38 | Fresh snapshot `000000000000001a0000000001f088fe0101` |
| Exact upgrade | 22:39:04–22:39:24 | Exact upload/API/settings verified while stopped |
| Start | 22:39:27–22:39:28 | Running exact module confirmed |

The exact oldest slot `00000000000000100000000001f088fe0101` was completely
archived and verified before replacement. Fresh stopped-state backup 1a was
fully downloaded and verified against prior deployed module
`2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`.
All other nine managed snapshots remain unchanged. Every management write was
dispatched once with durable local markers; no unknown write, retry, restore,
reinstall or downgrade occurred. Source and snapshot details remain private.

Retained-state readbacks: ten of eleven original scoped projections match;
one XRP-claim projection changed before installation and was independently
reconciled to the old-module stopped heap. Its claim IDs and immutable anchors
match; the two changed financial fields already have their post-readback values
in that snapshot. This is not an ordering normalization or omitted-field pass,
and does not establish external XRP payment finality. Three additional
comparisons (vault51/158 state, completed vault158 recovery and bot recovery
configuration) match. Normalized fixed configuration is equal; only the existing
price/timestamp/derived-rate exclusions are used. SEC11 and SEC12 source/state
preservation requirements remain in force; recover forward, never old-writer
rollback.

## Frontend staging

Existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai` retained its module/settings and
existing Prepare/Commit permission lists. Candidate's 740 files match the frozen
manifest. Batch **686** added **300** new keys only. Independent double-read
inventory and **302 served-byte checks** passed: **3,987 keys / 4,181 encodings**,
all 3,687 original keys/properties/encodings preserved. Index/version activation
has not been dispatched. The seven prepared mutable content sets remain held;
the previous primary-domain entry points remain active.

## Confirmed automatic-check defect

The +10-second check ran and logged an original-vault validation hold before any
settlement read or spend. The stopped heap independently confirms the same vault,
owner/type/debt, **150,000,000,000 raw nICP units** and `bot_processing = false`.
The public `CandidVault::from_state` projection deliberately reports zero while
the redemption claim fences the vault. Claim-event application retains the raw
original collateral amount. `adoption::validate_original_vault` incorrectly
requires that raw amount to be zero. The review/fixture used the projected shape
instead of the retained raw state; that omission is now recorded rather than
calling installed code functional.

The exact claim remains `ClaimConfirmed`, with no settlement block, received ICP,
payout or adoption journal; the dedicated escrow balance is **zero**. The fixed
target/request/burn/owner anchors match, and no backend bot claim is present.
No flag was cleared, no manual claim advance/retry/repayment/payout was issued,
and no financial movement is inferred from Running status. The hourly check
remains fenced by this guard. Adrian owns the minimal source correction: validate
the exact retained original amount while preserving owner/type/bot-state and
changed-amount refusal, with a realistic raw-state regression. No new team or
general review campaign is required. Any changed artifact is separately pinned
and reviewed before seeking its exact replacement-install approval.
