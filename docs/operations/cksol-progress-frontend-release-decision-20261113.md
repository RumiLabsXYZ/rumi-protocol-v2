# SEC18 ckSOL progress frontend release decision — HELD

Proposed single approval: Dorian may coordinate one bounded frontend build and
additive production publication of exact source
**ab44413da1a8b394245cd7734da4124a2335621b**, branch
`codex/cksol-mint-status-20261112`. Source report05ec4221 is documentation only.
This is separate from the completed SEC17 release086fbcc0. No candidate production
build, fresh allocation or publication exists yet. No prior grant or approval is
reused. The checked latest CTO/source-owner replies contain no matching SEC18
release approval; the source request authorizes preparation, not this publication.

## Behavior and accepted source

The ckSOL mint action gains persistent progress: spinner, last successful check,
automatic/paused checking, read-only Check again and action-needed guidance.
Allowance, processing funds, dispatch, payment settlement and official mint
confirmation stay distinct. Only official Minted evidence bound to the current
deposit ID shows the exact minted amount, destination and ledger block. Confirmed
success persists until Start another deposit, retaining the previous receipt.
Deposit-ID transitions invalidate stale evidence/reads; active or unknown holds
retain precedence; competing terminal histories use the authoritative current
deposit. An already queued request does not prompt for new SOL.

The delta from released00cf550d is four files: two runtime Svelte files and two
tests. Backend, generated interfaces, dependencies, payment services and repayment
code are unchanged. [Exact source pins](evidence/cksol-progress-frontend-release-20261113/source-pins.json)
and [accepted source receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/05ec4221/docs/operations/cksol-progress-ux-result-20261113.md).

Reuse88focused passing tests/3suites, both bounded independent correction rechecks
and final synthetic actual-page desktop/mobile QA with zero financial calls.
Source-only checks and mocked layouts do not establish production composition,
wallet signing, concurrency acceptance, a real mint or customer completion. No
repeated test/review campaign, broad suite, full typecheck or paid CI is added.

## Explicit two-way messaging and one fresh allocation

Approval explicitly authorizes BOTH directions between these existing chats:
- **Dorian → Cody:** Dorian chat01a1a800-7fde-71b2-a128-2b2325c4b232 may send Cody
  chat01a0eeee-d768-71b1-8090-0ca4e680e5b3 the exact SEC18 source/runner/capacity
  request and this build's start, native-result, manifest/resource/closure receipts.
- **Cody → Dorian:** Cody may reply directly to that original Dorian chat with
  one fresh allocation path/UUID/hash/start window, this lane's slot status and
  independent native/resource/closure verification. No unrelated private usage,
  process payload, paid/API fallback or additional lane authority is included.

Earlier SEC17/240 messages and consumed grants are not this authorization. The
central approval question should name both directions explicitly so a generic
approval is not mistaken for an agent-only relay. Preserve any existing rejected
payloads; no alternate route or automatic retry of them is authorized.

Cody performs fresh capacity/owned-closure checks and issues ONE exclusive slot.
Fresh allocation fields are UUID, absolute JSON path, SHA256 and a start deadline
within10minutes. No build starts before that new grant; approval is not capacity.

## Exact build and mechanical output pins

1. Verify the existing isolated producer
   `.worktrees/xrp-customer-withdrawal-address-20261106` is tracked-clean at00cf550d.
   Verify its current740files against the frozen SEC17 manifest and its retained
   frozen output. Preserve all previous bundles, metadata, grants and dependency
   links; only then detach this producer to exactab44413d. Do not reset the dirty
   primary/source-owner/release worktrees, remove caches or create another producer.
2. Run ONCE `python3 -I build-once.py {freshAllocationPath} {freshAllocationSha256}`.
   [Readable runner](evidence/cksol-progress-frontend-release-20261113/build-once.py)
   SHA256 **9200f67c24b2ec1698596e5813b8887c902a3c83bf54e084b9501ccaa178da7a**.
   Runtime namespace is `/private/tmp/rumi-sec18-frontend-release-20261113`.
   It executes only `npm run build --workspace=src/vault_frontend`, with installed
   offline dependencies and mandatory auth/domain postbuild. No backend build,
   dependency install or other heavy job. Total15minutes includes60seconds owned
   closure; Node heap6GiB, aggregate ownedRSS8GiB, physical growth8GiB and20GiB
   free-space floor. Exclusive fsynced dispatch marker, native exits and owned
   process closure are recorded. Fail/bound/unknown stops; no automatic rebuild.
3. Freeze and push the actual source-bound full manifest, version, full additive
   comparison, publisher/reader/verifier/pins and native/resource/closure receipt.
   Actual file count is verified rather than assuming successful completion.
   Verify every output hash, exact source and unchanged auth/domain bytes.
   Ellis independently verifies frozen files/compression/pins, accepted-source
   inclusion and additive plan, reusing accepted tests and source reviews.
4. The timestamp version, derived hashes/new immutable-key count and returned
   batch/chunk IDs are mechanical outputs of this exact source/environment. This
   single approval covers their verified publication below; producing these pins
   alone does not require another approval. Freeze/push them before production
   mutation. A source change, essential check failure or expanded mutation scope
   stops; do not silently substitute a different candidate or runner.

## Frontend-only production sequence

Target only existing mainnet frontend **tcfua-yaaaa-aaaap-qrd7q-cai**, networkic,
identityrumi_identity. The last independently verified baseline is SEC17
version1794555990015,5124keys/5318encodings, frontend module
04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62 and backend
**6428d1039a9f54e691acfb8cd6c8d5c4cdba8d675b7350f3ad5913815d5575f7** on
**tfesu-vyaaa-aaaap-qrd7a-cai**. These are dated expectations, not a fresh live read
in this preparation; actual current state must be reconciled before writes.

1. Freshly verify exact source/output/publisher pins, identity, both Running
   modules, settings/controllers, unchanged canonical backend API, full stable
   asset inventory and Prepare/Commit permission lists. Preserve deployed
   repayment, Vault51 automatic behavior, OISY/recovery/ckSOL and all exact holds.
2. Reuse the established additive publisher. Stage only newly built
   content-addressed keys under `/_app/immutable/`; use existing unambiguous
   property templates. No deletion, clear, unset, existing-property edit or auth/
   domain change. All prior keys/properties/untouched encodings remain retained.
   Entry-point writes are limited to the existing seven index/version content
   sets and encodings. New counts are measured and frozen, never guessed.
3. Verify the full stable staged inventory and all new served bytes before
   activating the seven entry-content sets. Keep the current entry point until
   that verification passes. Fresh artifact/identity/inventory/module/settings/
   permission guards precede both phases, using actual returned unique IDs.
4. Activate once, then verify full stable final inventory, changed served bytes,
   primary-domain root/version/auth, `/solana`, repayment/recovery/help/OISY/ckSOL
   compiled inclusion and applicable SPA fallbacks. Ellis independently checks
   public inventory/bytes, unchanged modules/controllers and actual desktop/mobile
   production layout. Accepted synthetic cases cover progress/confirmation states
   unavailable without a real mint; do not manufacture a customer outcome.
5. Preserve both module hashes/settings/controllers and permission lists. No
   backend stop/start/upgrade/build, snapshot replacement/restoration, controller/
   settings/mode/permission/reserve mutation or whole-stack deploy. No newcanister.

Every asset write has exclusive durable evidence and known batch/chunk IDs.
Unknown outcomes stop for readback, never write retry or another route. Recover
forward while preserving earlier source/artifacts; do not clear served assets.

## Financial and customer limits

No SOL transfer, payment collection, fee approval, mint submission, signing,
repayment, withdrawal, swap, cancellation, financial retry or customer message is
authorized. No wallet/account switch, stored-customer-state clearing or use of
Start another deposit on a real pending customer flow. Read-only observation must
not invoke a financial continuation. Existing financial controls and request holds
remain unchanged. A real mint's settlement/completion and customer acceptance
remain separate; software delivery alone establishes neither. Any existing
unrelated token-logo console noise is recorded, not called a clean console.

This pushed decision is the sole SEC18 release request for central presentation.
Until Rob approves it, allocation messaging, build and production publication stay
held. Preparation does not add a new optional harness, review or economic decision.
