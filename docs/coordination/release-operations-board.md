# Rumi release closeout and operations

Accountable orchestrator: **Dorian**, reporting to Merritt. Established 2026-11-04.
Lookout project ID: `rumi-release-operations`. This scoped lane owns portfolio items #13 and #15 and custodianship of parked #11. Existing release implementers, Cycle Sentinel cards CS-00 through CS-68, source branches and protected worktrees retain their ownership and history. No duplicate implementation team is created.

## Frontend reservation — November 11, 2026

The next focused frontend production-build/preparation slot is reserved for
Ellis's active OISY `/swap` signer-owner fix, composed with accepted ckSOL source
`94ffd5d5` and Vault51 lineage `097bf775`. Ellis owns diagnosis/source/regressions;
Dorian owns serial build, artifact/inventory preservation and the exact release
packet. No wallet retry is required to finish source/mock checks tonight.

Frozen Vault51 artifacts, stage686 and its unexecuted entry-point activation
remain preserved. The corrected backend packet097bf775 retains its separate
approval hold. Do not make that backend correction an automatic prerequisite
for an API-compatible frontend-only swap fix: verify current served backend
compatibility and prepare the distinct exact frontend decision. A new composed
bundle is not silently substituted under the old publication approval. No new
deployment, signing, allowance, swap, customer retry or financial authority is
inferred. Usable-flow incident work takes precedence over optional polish;
Season2 remains the independent product priority and its Points-only release
uses no frontend publisher slot.


The reserved composed frontend build at d54ac7b7 / accepted code6e6f32fa
closed PASS at2026-11-11 08:48:50 UTC, with mandatory auth/domain checks. All740
files and411-new-key/seven-mutable-set plan passed independent artifact and
served-baseline checks. The exact frontend-only publication remains held in
[the concrete decision](../operations/oisy-cksol-frontend-publication-20261111.md);
it does not install the separately held Vault51 backend correction.

Mara's owner-inspected local-only f26c10d6 remains next eligible for focused test
Wasm → xlm_confirm_pic → production compile guard check, but **no XLM admission
was written or delivered**. A competing existing-host LOCAL reservation appeared
during admission preflight; it was subsequently released. Cross-project scheduler
coordination delivery was then rejected by automatic approval review for missing
destination/payload authorization. Do not retry/relay that message or treat free
capacity as an accepted grant. The exact scoped communication request is
[prepared here](../operations/build-slot-coordination-permission-20261111.md).
This is a tool communication hold, not a new implementation/test approval gate.
Production Frozen/release/financial and paid-CI/concurrency boundaries remain.

## Release and operations queue

| ID | State | Accountable owner | Scope | Evidence / next action |
| --- | --- | --- | --- | --- |
| ROPS-01 | Done | Dorian | Close superseded PR bookkeeping | Rob explicitly approved both closures. [PR605 evidence](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/605#issuecomment-6652564836) and [PR606 evidence](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/606#issuecomment-6652568933) posted. Both verified closed/unmerged; original remote branches/heads preserved. No build/CI, publication, deployment or financial action. Source and bookkeeping closeout complete. |
| ROPS-02 | Done | Dorian | Source coverage and PR disposition for #605/#606 | Final comparison at0dea74a1: PR605 files exact; all PR606 accepted behavior retained, including pool delta via PR623. Remaining differences are subsequent backend/nICP work and formatting, not missing Sunset scope. PR605/606 are source-superseded and now closed under explicit approval; historical deployment receipts remain distinct. No new source task. |
| ROPS-05 | Done | Existing frontend release owner; Dorian reconciliation | Preserve #605 in paired frontend composition | [Planba1ef31b](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/ba1ef31b4d2bf3faeca96213a00232aa7b526708/docs/operations/cksol-frontend-publication-plan-20261104.md) records all2667livekeys/properties and auth preservation. Both balance component/spec byte-match45af74a4 in pairedsource6847982b and currentmain0dea74a1. Source/artifact preparation complete; no rebuild or cherry-pick needed. Publication remains a separate decision; no new live-wallet proof claimed. |
| ROPS-06 | Done | Dorian; original sunset/pool implementer | Integrate missing #606 pool admission checks | [PR623](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/623) merged as34cd6769048f88a224dd78fa8fa59b3230768cee at2026-11-05T04:19:09Z. Approvedheadf660f751 unchanged; [requiredCI41965537346](https://github.com/RumiLabsXYZ/rumi-protocol-v2/actions/runs/41965537346) passed. All3 current-source focused tests passed; ownedgroups closed. Source integration complete only; no deployment or liquidation. ROPS05 remains separate. |
| ROPS-07 | Done | Dorian integration; Ellis case; Miles source; Bram canonical publisher | SEC10 XRP customer source integration | [PR628](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/628) merged as5edd1ff1 at2026-11-06T09:31:28Z, head1d019794. [RequiredCI42193041195](https://github.com/RumiLabsXYZ/rumi-protocol-v2/actions/runs/42193041195) passed20m6s; merged3Rustblobs match acceptedc02c4bff,6tests/bincheck reused. Interest/rounding clarified. CanonicalSEC10 publisher remains Bram; handoff retained, this card is integration-only. Source integration complete. Deployment and now-active SEC11 work are tracked separately in ROPS08; financial/customer actions remain held. |
| ROPS-08 | Done | Dorian release; Miles source; Ellis case | Deploy XRP withdrawal and repeat-deposit repair | Both fixes deployed: backend controllerRunning exactuploadb81583ae, source0eff84c9; frontendversion1794099947058 served. All14focused+1canonical checks and production/backend/frontendauth builds passed; 7frontend source tests reused. Stopped drain/freshbackup18/9other snapshots preserved; all11 scopedreads+fixedconfig match. Full3384keys/3578encodings verified; all3078oldkeys/properties retained,306newkeys+7approvedmutablecontents;308stage+9activation+5primary servedchecks pass. Frontmodule/settings/auth unchanged. [Terminal evidence](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db9/docs/operations/sec11-live-release-20261107.md). Release acceptance complete; actualcustomerretry/message remains Ellis-owned and held. FuturebackendmustretainSEC11receiptmaps/privatejournal; olderwriterrollback prohibited. No financial/customer action. |
| ROPS-09 | Review | Dorian release; Adrian source | Vault 51 deployed check held; exact correction decision | Rob approved9d02bea8; exact backend43bbbca9 is Running with Stopped/backup1a/other9/API/settings and retained state verified, including one old-module XRP projection reconciliation. Stage686 added300 assets; entry points remain unchanged. First10-second adoption check exposed raw-versus-public collateral guard defect and held before spend; escrow0, no adoption. Adrian correction312ebe03 has25 focused tests passing; corrected backend730f5471 build and397-method/Candid/gzip checks pass, reusing frontend740-file manifest. Next: Rob approves exact corrective backend bytes and same verified automatic financial scope in the [replacement decision](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/vault51-auto-icp-adoption/docs/operations/vault51-raw-collateral-fix-20261110.md#exact-replacement-decision--held); then finish the already approved frontend activation. [Live receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/vault51-auto-icp-adoption/docs/operations/vault51-live-release-20261110.md). No new team or broad audit; Season2 accounting higher priority. |
| ROPS-03 | Ready | Dorian | Reconcile stale Cycle Sentinel operation cards | Existing CS-66/CS-68 records leave signer-screen proof and proposal 17 activation unresolved; [#416](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/416) is open/unmerged and source-only. Acceptance: timestamped read-only current policy/proposal/status evidence, with stale versus current facts separated and old card owners preserved. Next: one bounded read-only inspection at the next operations allocation. No maintenance-now, proposal execution, activation, funding or controller changes. |
| ROPS-04 | Ready | Dorian | Produce a small operational-cost baseline | Acceptance: observation date, available cycle-burn/runway coverage, gaps and concrete cost drivers; no fabricated zeroes or savings claims. Next: reuse current Sentinel telemetry and available existing billing evidence, label unavailable inputs, and recommend only evidence-supported upkeep. Lower priority than release closeout and the portfolio's recovery work. |
| COMMS-01 | Paused | Dorian | Preserve communications, X workflow and introductory video | Rob parked portfolio item #11. Acceptance: retain existing drafts and workflow history without drafting, publishing, scheduling or restarting workers. Next: resume only if Rob changes the parking decision. Blocker owner: Rob; this is deliberate parking, not a technical failure. |

## Evidence and limits

- GitHub metadata read on 2026-11-04: #605 head `45af74a433c169c08e52bc1ad2bda23e1df215a9`, #606 head `0c520df74ef4d0609a0be93bea4bff0bd2974cbb`, both open/unmerged. A non-null GitHub test merge SHA does not establish a merge. #606's description records backend/pool/frontend deployment, focused verification and no funded liquidation; its additional deposit-intent CI proof was described as in progress. This pass has not inferred a current CI conclusion from that older description.
- Retained sunset frontend receipt: `served=true`, 1,437 old keys preserved, 280 encodings checked, timestamp 2026-11-01T08:52:48.436Z. Private receipt: `/private/tmp/rumi-sunset-release-20261101-final-verification.json`. This is historical release evidence, not a new live readback.
- #611 merged as `a71156d644dfa151a40b9008e971054075ae484f`. [Retained release note](../../output/oisy-vault-release-20261102/README.md) records combined build `5aca4853fd66cd40a615ed4eeb369ba540150e38`, preserving the already-served Vault 51 interface from open #613. Its retained frontend receipt reports `served=true`, 2,412 old keys preserved, 262 encodings checked at 2026-11-02T21:58:01.825Z. It supersedes the earlier frontend entry point as historical publication evidence, not the PRs' source disposition. Actual Oisy signer screens were not observed.
- Local `git merge-base --is-ancestor` returned 1 for each #605/#606 head against combined source `5aca4853...`; simple ancestry does not establish their inclusion. ROPS-02 owns the narrow source comparison; no missing fix is asserted from ancestry alone.
- Existing Sentinel source board remains [kanban.md](kanban.md), with [cadence live proof](sentinel-cadence-live-proof-2026-09-28.md). Those September facts are historical. Do not assume proposal 17 remains pending or is now active without readback. #416 head `497346640125bd655ec4897e5ca92fb62cb8d5c0` is currently open/unmerged and reported source-only; it does not prove installation of the dirty main-checkout Sentinel edits.
- [SUNSET-DEF-1](demo-deferred/sunset-liquidation.md) stays open: no funded manual/pool liquidation proof. A financial demonstration is outside this assignment and is not a release-housekeeping gate. No new demo implementation shortcut was taken in this documentation-only pass, so no invented deferred-work entry is added.

## Allocation, acceptance and publishing

- Dorian requested model/effort: `gpt-6.1-sol/high`; actual runtime model is not independently attested. This pass used no workers, builds, test suites or extra credits. A future bounded source inventory can use native `gpt-6-luna/medium`; substantive code work is not dispatched by this assignment. No external-provider source transfer is authorized here.
- Release closeout precedes lower-priority operations upkeep. Rumi's approximate 20% portfolio allocation is aggregate; vault 158/ckSOL remain first and core security steady. Ready cards awaiting allocation are not technically blocked.
- Acceptance for this setup: named ownership, usable queue, honest evidence separation, parked communications, sanitized Lookout snapshot and exact readback. Merritt is the independent acceptance owner; ROPS-01 remains Review until accepted.
- No release changes, deployment, signing, funds, extra credits, new governance actions, access grants or product membership/priority changes. Unknown outcomes and existing exact action holds remain intact. No implementation worktree was created or altered.
- Publisher: reuse the existing authenticated Lookout app identity through the ICP connector. Session readback confirmed admin access and `canManageProduct("rumi-protocol")`/`canManageProject("rumi-release-operations")` true; no access grant was made. Initial board inventory contained no release/operations board, so this one scoped board is created. Merritt owns product membership and project ranking.
- Publication inputs and readback receipts are retained under `output/release-operations-20261104/`. Source-bound mirror and metadata updates use this lane's sanitized table; no private receipt contents or paths are published.
- Worktree hygiene: 75 registered, 9% free; required eligible cleanup removed 0, with 74 non-primary entries protected/dirty/unmerged/missing. Existing release worktrees were preserved. Disk pressure limits future heavy work; it does not block this small queue setup.

## Initial publication readback

Lookout accepted board revision **1**, source revision `4a2b52396010eece7f8f5adff0e1f1fae8bae2770ba2eb1e1a7f4c63e255fcc9`. Exact decoded snapshot readback matched every field, including all five card IDs, owners and states. Technical and Plain English summaries, ordered three-item subpriorities with card associations, and repository metadata matched the expected values on readback. Evidence: `output/release-operations-20261104/verification.json` and scoped readbacks. No completion dates were fabricated. No product membership or product priority was written; return this project ID to Merritt for that step. No publisher-access blocker remains for this existing app identity.

Next deliverable: ROPS-02's source-coverage/PR-disposition recommendation, then the lower-priority ROPS-03/04 upkeep pass when allocated. No additional work was dispatched. No new durable persona lesson was established in this bounded inventory; the existing distinction between merge, publication and execution remains applicable.

## ROPS06 accepted scope basis

Rob’s original sunset request in chat `01a19323-6af7-7313-bf78-2154169de5e3` explicitly asks to bypass the Stability Pool for sunsetting collateral and route it to manual liquidations only. The64-line source delta supports a narrow source-reconciliation task, not current live-failure proof or a release gate for unrelated delivery. Existing-intent recovery remains preserved. Focused verification addresses this accepted behavior only; any new broader hardening is optional and deferred. No additional audit/review layer is assigned.

ROPS06 allocation checkpoint: Bram confirms no earlier Rumi/ckSOL local build reservation. Cody admitted a25min,8GiB aggregate RSS,8GiB physical-growth window with20GiB free floor, offline/locked/one Cargo job, isolated target, first-failure stop, exactly three selected tests. Its five-minute start deadline expired before Cargo invocation; no process started and no grant was replayed. A fresh ordinary allocation was requested, with dispatch lead time; this is resource scheduling, not a new Rob approval or unrelated release gate.

Ready checkpoint: original owner prepared `python3 /private/tmp/rumi-sunset-sp-admission-test-window.py` (SHA256 aca01a33021677784e3c033615c1f070f177a2a102a1198a2486ef138503df16), syntax/exact-test-output guards passed; no runner/Cargo invocation. No further preflight preparation is needed. Cody readiness delivery failed twice: `Cannot steer conversation ... without an active turn id`. Existing reservation remains expired/unrun; actual XLM occupancy is preserved.

## Fresh-grant preflight result

Merritt delivered grant ROPS06-20261105T023402Z directly to the original owner, but the prepared runner’s allocation file still contained the old5d8217d1 packet without `executionAllowed:true`. The single invocation stopped at that guard before Cargo; no test/PGID/target was created, and no retry occurred. This is an allocation-file synchronization defect, not a code/test failure or current live-failure proof. Correct fresh packet delivery to the exact runner allocation path is the next concrete prerequisite; do not replay either grant. Source head remains cleanf660f751.

## ROPS06 current-source terminal checkpoint

Replacement packet was synchronized explicitly under Merritt temporary authority. Original owner started Cargo02:37:25UTC, ownedPG89486. One serialized offline/locked build and exactly3 named tests passed on cleanf660f751; Cargo16m08s, fullwindow16m11s, peakRSS1.62GiB, physicalgrowth2.26GiB, completionfree71.6GiB. All ownedgroups closed; no next local run/retry. Historical binaries were not used for this result. Coordinator reviewed the one-file diff and diffcheck. PRtitle/body/request metadata are in `output/release-operations-20261104/pr-content/`; no PR or new hostedCI was created.

Automatic approval review rejected a broader outgoing handoff because it combined slot release with a request for a separate delegated-vault allocation and future hosted allowance; the stated reason was lack of specific authorization for those additional allocations. A narrowed terminal-status-only message to Thayer succeeded. The rejected broader request is not rerouted. Later task-specific coordination sent before the latest hold did not launch a run; latest Merritt instruction explicitly holds this checkpoint to unaffected source handoff/localPRcontent/board/readback. This is distinct from a rejected PR creation: PR creation was not attempted.

## Canonical approval pending

Merritt queued one canonical async approval in his chat for opening the pushedf660f751 pool-fix PR, only required CI within existing allowance/no extra credits, and merge when green. Actual answer is pending; no action is inferred from a preselected option or elapsed time. Code-file SHA256 supplied/verified by Merritt: f7472577cec2a63ffeb0162d1778e940942ec6475d28d35e19f3343659394190. Thayer confirms DVhosted1036/1037 terminal and no hostedcollision. This clears capacity only, not the approval hold. Preserve prepared PRcontent and testreceipts; no more local work is needed while waiting.

## ROPS06 merged source checkpoint

Rob directly answered “Approve PR, required CI and merge” in Merritt. Exact codeSHA256 f7472577cec2a63ffeb0162d1778e940942ec6475d28d35e19f3343659394190 matched before PR creation. PR623 opened at approvedf660f751, onefile79insertions; required archived-receiptCI41965537346 passed in12m58s, including exact testWasms, deposit recovery across upgrade, and pool state transitions. Normal merge used explicit expected head, with no admin/bypass or branch deletion. GitHubreadback confirms merged34cd6769048f88a224dd78fa8fa59b3230768cee at2026-11-05T04:19:09Z. No conflicts/source changes/local suite or extra credits were needed. No deployment, liquidation or financial action occurred. OriginalPR605/606 remain outside this merge disposition; ROPS05 frontend retention stays separate. Historical approval holds above are superseded only for this exact approved integration.

## Final accepted-work reconciliation

Read-only comparison after successful main refresh pinned main0dea74a1d33456ee28a9837b99a3be9dae7f674f. Pushed planba1ef31b4d2bf3faeca96213a00232aa7b526708 at `docs/operations/cksol-frontend-publication-plan-20261104.md` is a completed preparation artifact, not publication. Its committed auth/balance checks and independent Git-blob comparison establish both PR605 files exact in pairedsource6847982b44f247e0c99f0a453b583d3fab8107a2 and currentmain. Thus ROPS05 is complete at its accepted source/artifact-preservation scope; no additional build, cherry-pick, verification campaign or source change is queued.

All13 PR606 changed files were compared:8 byte-match its head; the remaining5 reflect unrelated subsequent backend/nICP changes, dashboard claim accounting, or pool formatting while retaining the accepted Sunset guards/presentation. Pool port623 remains merged. PR605/606 are therefore stale source-duplicate bookkeeping candidates, still open and not closed here. Existing release history and live deployment proof are not erased or inferred from this classification. The concrete remaining release action is a separate publication decision/authorization for the prepared paired artifact and its existing conditional ordering, plus owner disposition of duplicate PRs. No current source gap was found in the accepted scope. No builds, CI, live calls, new review, deployment or PR closure occurred in this reconciliation.

## Closure bookkeeping approval block

Merritt accepted the final classification and requested closing PR605/606 without merge/branch deletion. Automatic approval review rejected the first attempted `gh pr close605` command before execution: it stated PR623 integration approval did not explicitly approve closing separate historical PRs. No alternate tool/route or PR606 close was attempted. Fresh GitHub readback still shows both open/unmerged. Source work/ROPS05 remain complete; only closure authority is blocked. Direct human approval is needed for those two closures; this does not reopen builds, CI, publication, liquidation or financial actions.

## Explicitly approved closure complete

Rob directly answered “Approve closing both superseded PRs,” resolving the earlier authority rejection for these two closures only. PR605 closed2026-11-05T07:42:20Z with evidencecomment6652564836; PR606 closed2026-11-05T07:42:34Z with evidencecomment6652568933. GitHub readback verifies both closed and merged=false. Remote refs verified intact: codex/3usd-wallet-balances-20261031 at45af74a433c169c08e52bc1ad2bda23e1df215a9; codex/sunset-manual-release-20261031 at0c520df74ef4d0609a0be93bea4bff0bd2974cbb. No branch-deletion or merge flags were used. The interrupted CLI outcome was reconciled by readback without retry. Historical release evidence, publication/installation holds and all exact financial boundaries remain unchanged; no builds/CI/deployment/funds action occurred in this bookkeeping tranche. Earlier open-state/blocker notes above are historical and superseded by this explicit approved closure.

## SEC10 source integration — November6

Merritt admitted candidatec02c4bffffd10521362d3cae3047efdb4626cf42 on separate branchcodex/xrp-customer-withdrawal-address-20261106. Exactbase/currentmain0dea74a1, no composition conflict. Existing candidate worktree reused; untrackednode_modules preserved. Dorian soleintegrationowner; Ellis supplied no-overlap handoff and Miles existing readback confirms ownership. Automatic review rejected the two outgoing coordination messages as lacking specific destination authorization; neither was retried/rerouted. Independent incoming handoffs/current readback support unaffected source work.

Only documentation clarification was added as1d0197946aa48fdc82f8b42e7e456025fa3dd18e; threeRustblobs unchanged, so prior6focusedtests and offlinebincheck reused. Debt already includes accrued interest, upward-rounded during accrual before partial admission; inherited Token*Ratio/native conversion truncation and max1residual for positiveXRPdebt are documented, not silently described as ceiling-rounded fractionalCR. No fresh arithmetic-policy scope/audit was introduced. DraftPR628 open, one existing requiredCI42193041195 running; no extra paidcredits/localtests/builds/deployment/livefinancialactions. SEC11repeatdeposit/address remains deferred. CanonicalSEC10 securityboard ownership/publisher stays Bram; releaseboard card is integration support only. Required cleanup helper ran under6percentdiskpressure but remote refresh was sandbox-blocked, so0removed and protectedworktrees preserved; mainrefresh separately succeeded.

## SEC10 source integration terminal

DraftPR628 opened at1d019794, attached to this chat, marked ready after requiredCI42193041195 passed20m6s; normalmerge5edd1ff1e49d68dc77f50a109017ab6d834bc218 at2026-11-06T09:31:28Z used explicit expected head and noadminoverride. APImergedreadback and3remoteRustblobchecks match acceptedcandidatec02c4bff exactly. Only documentation clarification followed candidate tests. No compositionconflict, redundantlocalbuild/test, paidcredits, deployment or livefinancial/customeraction. Exact receipts and canonicalSEC10 handoff retained under output/xrp-source-integration-20261106/. Bram still owns canonicalsecurity publication; releaseview is integrationstatus only. Source merge does not resolve a live customer attempt or finish SEC11.

## Vault 51 review assignment — November 10, 2026

Merritt assigned Dorian accountable release-review ownership and Adrian source reconciliation. The old d41a4c5c packet/build is based on SEC11 0eff84c9 and is superseded for release preparation by deployed SEC12 f1d2e77e (receipt 0f003442). It is retained as historical source/test evidence, not an installable candidate. Builds stay serialized; none is dispatched by this review. Review/preparation does not authorize deployment, timer activation, custody transfers, customer actions or communications. Next deliverable: concrete source findings and an exact reconciled-artifact release recommendation.
