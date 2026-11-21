# Season 2 isolated release preparation — November 10, 2026

**Accounting and execution remain paused pending corrected analytics
reconstruction. No new approval request is being presented. The window mechanism
is timing-only and authorizes no historical credit or participant query.**

Dorian owns configuration and this release-preparation record. Orin owns Points
Rust, calculator, private participant reconciliation and the actual running-scoring
outcome. This source-only change is on `codex/season2-accounting-20261110`, atop
Orin’s committed `87f78737` implementation. This release-preparation lane has executed no new build, worker, review campaign,
canister creation, funding, installation, imported award or activation. Rob’s
incoming funding has now been independently observed as recorded below.

## Exact configuration changes

- Add selector **`rumi_points_season2`**, using the same `rumi_points` Rust package
  and Candid, pinned `@dfinity/rust@v3.2.0` recipe with locked dependencies and
  shrink enabled. Existing candidate artifacts are reused; this change did not
  execute that recipe or rebuild the binary.
- Add **`mainnet-season2`**, network `ic`, containing only that selector. Do not
  use implicit `ic`, `mainnet-live` or a whole-stack deployment for this release.
- Add `.icp/data/mappings/mainnet-season2.ids.json` as **`{}`**. It is a separate
  tracked ID namespace, not a fabricated principal. No ID is allocated; a later
  specifically authorized creation records its returned ID there. Never copy the
  S1 ID into it or repoint the S1 selector.
- Proposed settings are explicitly `rumi_identity` as the sole controller, zero
  reserved compute, zero fixed memory allocation, 30-day freezing threshold and
  controller-only logs. They have not been applied to any canister.
- File-backed, explicit initialization is in
  `evidence/season2-release-preparation-20261110/season2-init.did`: fixed admin
  `rumi_identity`, default protocol exclusions (`null` preserves the built-in
  seed), prospective cutoff T0, `u64::MAX` as the technical open end, and public
  H0 commitment. **H0 is fixed; T0 is now the non-executable token
  `DERIVED_T0_NS`. The old 19:00 UTC T0 and 18:45 UTC approval deadline are
  EXPIRED/SUPERSEDED.** The replacement is a new, unapproved proposal: direct
  approval valid for 24 hours; select the first UTC 15-minute boundary at or
  after fresh preflight completion + 60 minutes, no later than approval + 24
  hours. Root's local renderer must freeze, encode and push the exact init
  before install. No silent retiming after install/begin; missed T0, snapshot A
  margin or window expiry means HOLD. Never put S0 in public source.

S1 selector/definition, every existing environment and the complete
`mainnet-live.ids.json` are unchanged. S1 remains
`bfnu3-6aaaa-aaaab-qhanq-cai`. No Points Rust, generated bindings, calculator,
backend `event.rs`, source endpoints or frontend route was edited by this change.

## Initialization boundary

The candidate’s `begin_season2_import` requires an empty stopped instance,
committed seed hash, a future T0 later than November 1 and no decided end
(`u64::MAX`). T0 is both exclusive historical replay end and prospective engine
start. The logical backfill start remains November 1, 2026 00:00 UTC. Those two
starts are deliberately distinct. The approval-relative window proposal leaves T0 unrendered and execution held.
Orin owns the final cutoff source manifest and participant reconciliation. The public H0 commitment is fixed as
`d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8`
(32 bytes). Orin reports preparing the prospective seed once using OS randomness
at `2026-11-10T17:21:39.845469Z` and retaining S0 only in a private ignored
mode-0600 file. Dorian pinned the supplied public commitment without accessing
or transmitting S0. See [commitment evidence](evidence/season2-release-preparation-20261110/prospective-seed-commitment.json).
Token allocation and a final end-date decision are not added as accounting blockers.

[CLI 1.3 file-backed init syntax](https://cli.internetcomputer.org/1.3/reference/configuration/#init-args)
was checked against the installed `icp 1.3.0`. The previous fixed-time init
encoded to 139 bytes, but that artifact is EXPIRED/SUPERSEDED and is not usable.
The current held template deliberately fails encoding until root's renderer
`scripts/prepare_season2_release_window.py` supplies the approved derived T0.
Re-encode and record exact text/binary hashes of the rendered file, then push it
before installation. The tracked hex file is also a non-executable held marker.
No sync hook automatically imports credits, starts the season or enables timers.
See the [held exact command plan](season2-creation-install-plan-20261110.md).
The [final decision packet](../coordination/season2-release-decision-20261110.md)
owns any future accounting-method proposal, fresh cohort-query scope and
activation authority. Estimated historical-credit methods, the earlier sample
and the 20-point legacy compensation proposal are withdrawn pending corrected
history. This preparation accepts no credit model and grants no permissions.

## One exact bounded funding proposal

Creation source account: cycles ledger **`um5iw-rqaaa-aaaaq-qaaba-cai`**,
owner **`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`**
(`rumi_identity`), **default/null subaccount**. Independently re-read balance:
**4,300,650,044,313 cycles**, exactly **4,000,000,000,000** above the previous
300,650,044,313 observation. Orin relays Rob as the payer; the balance read
verifies the credit amount but does not independently attribute the sender.
The previous +650B funding request is superseded by funding received. No
additional funding is requested here.

The separate earlier **1T** top-up of the ledger canister itself is **not** a
credit to this deploy account and is excluded from the 4T delta. See
[funding received](evidence/season2-release-preparation-20261110/funding-received.json).
These additional funds do not expand the proposed creation allocation or
authorize spending the account balance.

| Item | Exact cycles |
| --- | ---: |
| Previous deploy-account balance | 300,650,044,313 |
| Independently observed account credit | 4,000,000,000,000 |
| Current deploy-account balance | 4,300,650,044,313 |
| Amount forwarded for creation, unchanged | 900,000,000,000 |
| Subnet creation fee deducted from amount | 500,000,000,000 |
| Initial new canister balance before install | 400,000,000,000 |
| Additional ledger creation fee | 100,000,000 |
| Total proposed deploy-account creation debit | 900,100,000,000 |
| Deploy account remaining after that proposal | 3,400,550,044,313 |

The target proposal pins public CMC-eligible **13-node** application subnet
`jtdsg-3h6gi-hs7o5-z2soi-43w3z-soyl3-ajnp3-ekni5-sw553-5kw67-nqe`.
Deployed replica revision
`83f29e0803496ba6aaea3d6b43f6c6bebea12dd9` sets creation to 500B,
storage to 317,500 cycles/GiB/second and base idle cost to 10,000/second.
[Deployed pricing source](https://github.com/dfinity/ic/blob/83f29e0803496ba6aaea3d6b43f6c6bebea12dd9/rs/config/src/subnet_config.rs#L112),
[ledger debit semantics](https://github.com/dfinity/cycles-ledger/blob/main/INTERFACE_SPECIFICATION.md#create_canister).
Node count, eligibility, pricing revision and ledger fee are execution-time
preconditions, not permission to silently substitute a different subnet.

The measured first-epoch footprint is 151,493,408 bytes in both the original
one-owner fixture and the bounded 15-owner fixture. Local PocketIC
pricing is not mainnet pricing. At a **256 MiB planning envelope** (77.2% memory
headroom), 30-day idle reserve is **231.66B**. Partition the proposed 400B into
that reserve, **50B** install/reconciliation, **100B** initial polling/driver and
**18.34B** margin. The 15-owner fixture passed with the same footprint, but it
is not a sustained real-cohort workload, enforceable spending subaccounts, or
guaranteed runway. Read actual post-install/import memory,
idle burn and cycles before authorized activation; retain the normal freezing
threshold. The generic 1T health watermark will still show `healthy=false` at a
400B balance; that indicator is distinct from replica freezing, not a claimed
blocker or permission to change the watermark.

These exact figures are in [funding-proposal.json](evidence/season2-release-preparation-20261110/funding-proposal.json).
This packet proposes no funding from S1 or another canister. Incoming funding
does not authorize creation, installation, historical award import, timer
activation, ICP conversion or spending beyond any separately approved scope.

## Existing candidate and narrow validation

Orin’s already-built candidate is recorded in
[candidate-artifact.json](evidence/season2-release-preparation-20261110/candidate-artifact.json):
optimized Wasm `dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17`,
3,018,333 bytes; executable code 2,457,901 bytes; gzip
`967e22d162579f5626283b094bd0a1457f09c009f34ed455b676f252975def8c`,
747,354 bytes. These are not deployed hashes. This configuration-only addition
does not approve installation, historical credit or activation.

- `icp project show` passed, expanding the recipe and explicit init path without
  building, installing or invoking an update.
- Parsed comparison proves every pre-existing canister definition/environment
  preserved and S2 environment contains exactly one new selector.
- S1 mapping bytes match the previous commit; S2 mapping is empty.
- The prior fixed-time encoding evidence is retained only as expired history.
  Current template is intentionally unencodable; exact rendered Candid must be
  validated and pushed before install. No private seed was read here.
- Scoped `git diff --check` passed; only the configuration, new mapping and
  Dorian-owned release-preparation paths are staged for this commit.

See [configuration-validation.json](evidence/season2-release-preparation-20261110/configuration-validation.json).
The source board and numerical reconciliation remain Orin-owned. His next
release packet can retain Rob’s reported funding and independently observed
balance, complete historical result, frozen
T0 and private seed custody (public H0 already fixed), target/controller/artifact,
exact import transcript and separate activation authority. Rob’s actual-running/all-November-1-points outcome is still
open; this preparation is not a source-only substitute for it.

Running configuration and awarded points are separate proof: a successful poll
and an open committed epoch can show that configuration is running; ordinary
prospective snapshot awards are observed only when an epoch closes, normally
after a week. Running timers do not establish an ordinary ledger accrual or
complete historical November 1 reconciliation. No earlier close or live award
is inferred here.

## Bounded 15-owner budget cross-check

The existing lifecycle fixture passed with 15 synthetic owners on the same
candidate. All traverse import, snapshots A/B and epoch close; one has mock
debt and fourteen have zero positions. After-epoch managed memory remains
151,493,408 bytes and stable memory 142,671,872 bytes. Local balance decreases
were 45,078,705 cycles install-to-import, 16,640,757,193 import-to-upgrade,
30,645,558 upgrade-to-partial-poll, and 11,468,040,159 partial-poll-to-close.
These include intervening calls/time and are not isolated transaction costs.

The mock poll succeeds only for its backend endpoint; 3pool/SP/AMM event-poll
endpoints are absent. This is not a complete initial-poll cost, full real
15-owner positions or deployed-price measurement. It supports the unchanged
100B operating allowance as plausible for this minimal case, not guaranteed
runway or permission to activate. Use deployed pricing and actual post-install
readback; no additional fixture campaign is requested. See
[fifteen-owner evidence](evidence/season2-release-preparation-20261110/fifteen-owner-lifecycle.json).

Historical reconciliation remains unresolved. Orin reports that all 14 old
loan IDs remain positive in the raw snapshot; the earlier zero-current/writeoff
interpretation was wrong. A replay mismatch is not proof of missing history.
The estimated model/sample and 20-point compensation proposal are withdrawn;
no historical credit method is accepted by this timing/configuration update.

## Source PR status snapshot

Orin reports draft [PR #630](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/630)
at source head `12eae76c`, merge state CLEAN and no checks matching the changed
paths. This is a reported source/PR snapshot, **not CI validation or merge
acceptance**. This later funding-document commit does not alter the scorer.
Historical credit methods remain withdrawn pending corrected reconstruction;
this record does not authorize historical awards. No complete all-category November 1 import, ordinary
live accrual or activation is claimed.
