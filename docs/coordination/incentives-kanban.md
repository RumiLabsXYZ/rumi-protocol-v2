# Rumi incentives — Orin

Accountable orchestrator: **Orin**, under Merritt, Rumi CTO. Updated November 24, 2026.
Project ID: `rumi-incentives`. This is the scoped source board; Lookout is its sanitized mirror.
The authenticated inventory contained no incentives board at setup. Existing Security card SEC-03 remains Bram's Season 1 closeout evidence; it is not copied as new work.

## Incentives task queue

| ID | State | Accountable owner | Scope | Evidence / next action |
| --- | --- | --- | --- | --- |
| S2-BASELINE-01 | Review | Orin | Reconcile inherited Season 1/2 evidence and ownership | Acceptance: distinguish finalized points, intended start, historical rates and parked claims. Existing finalization receipt says no Season 2 activation or token transfers. Prior chat records intended November 1 UTC start. Main source separates legacy 5x/3x and flat 4x; no current live policy read claimed. Next: Merritt review of this bounded handoff. [Finalization PR](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/608) |
| S2-TERMS-01 | Review | Orin | Reflect Rob clarified continuous Season 2 direction | November1 UTC start confirmed; no voluntary uncredited gap. Token split deferred, end undecided, January31 tentative. Nov30 end and rate/eligibility changes are not approved. Next: inherited-rule accounting preparation, with precise unresolved interpretations only. |
| S2-HISTORY-01 | In progress | Orin | Reconcile November1 history independently of live scorer | Nov24 audit verifies all90 baseline hashes and60 raw position responses;15 vault responses omit hidden accrual clock. File-time anchor confirmed, no failed-call receipt found in bounded original tree. Later SP audit verifies39 inputs and2,548 contiguous IDs through its Nov10 endpoint, no overlap conflicts; owner allocation/snapshot gaps remain. Prior residual result not rerun; some final comparison paths unavailable. Next: provenance-bound clock/operation evidence or original comparison recovery; no fitted clocks, private export or historical credits. [Provenance receipt](season2-preflight-history-20261124.md) |
| S2-DECISIONS-01 | Done | Orin | Record exact prospective-only release approval | Rob directly approved item1 in Merritt on Nov12 22:29:14UTC: bf27c8e3, 900.1B-cycle debit, fresh fixed cutoff, zero historical credits, S1 untouched. Approval expires Nov13 22:29:14UTC. Derived window pushed0289c365; no repeated approval needed. |
| TOKEN-CUSTODY-01 | Parked | Orin | Custody of RUMI token, SNS/DAO and airdrop claims | Rob parked project 9. Acceptance: preserve evidence and separate point totals from token allocation, funding, launch and executable claims. Next: retain custody; resume only on Rob's instruction. Historical launch dates and claim tiers are planning terms, not readiness or authorization. |
| POOL-GROWTH-CUSTODY-01 | Parked | Orin | Custody of 3pool liquidity growth and promotion | Rob parked project 10. Acceptance: preserve promotion ideas without publishing promises. Next: retain custody; resume only on Rob's instruction. Main-branch report corrects the old 10x label; future messaging still needs approved Season 2 rates and eligibility. |
| FEE-NEURON-PROP-01 | Backlog | Orin | Preserve liquidity plus RUMI neuron fee-sharing sketch | Rob requested proposal custody only. Illustrative realized-surplus split50/30/20 and capped1x-2x commitment boost; ordinary yield and earned Season1/2 points preserved. No implementation or rewards approved. Next: Merritt discussion review when allocated. [Proposal](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/season2-terms-proposal-20261104/docs/coordination/lp-neuron-fee-sharing-proposal-20261105.md) |
| S2-DELIVERY-01 | Review | Orin | Restore actual forward S2 scoring and truthful current-season banner | Exact approved release executed Nov24: single1T+100M ledger block17,318,269; verified private backup, exact guarded upgrade, epoch0 archive and26 held passes complete. Epoch1 active at21:02Pacific; normal timer independently advanced583324→583329. Correct S2 banner served/browser verified, S1 history separate. Next: observe scheduled A/B and Nov26 15:45Pacific weekly close; no award claimed yet, missed coverage remains pending. [Execution receipt](../operations/season2-release-execution-20261124.md) |

## Task details and evidence

- **S2-BASELINE-01:** scope is existing evidence reconciliation only. The retained Season 1/2 chat is `01a197f9-96ad-7f22-b54f-52162c64a2c5`. Its November 1 exchange records the intended retroactive start as November 1, 2026, 00:00 UTC, with rules and historical measurability still unresolved. Later closeout reports final Season 1 points and disabled scoring timers. Local receipt `output/season1-reconciliation-20261101/README.md`, section “Completed mainnet finalization”, explicitly records no token claims/transfers and no Season 2 activation. This pass did not query live Points state; treat that as historical verified evidence, not a fresh mainnet read.
- **S2-TERMS-01:** draft owner Orin; no writing implementation, new financial commitment, or reopening parked claims. Candidate baseline for discussion: debt and icUSD 3pool/Stability Pool 1x; fiat-backed 3pool flat 4x; 3USD Stability Pool and AMM LP 2x; qualifying stablecoin repayment 5x with the existing window. These are inherited design/source rates, not approved Season 2 terms. Rob directs continuous November1 accounting under inherited rules. Token allocation and end date are deferred and do not block accounting; surface only demonstrated rate/eligibility ambiguities. Claim economics belong to parked project 9 and may remain unresolved without blocking this proposal.
- **S2-HISTORY-01:** independent of selecting a numerical budget. Reconciliation and implementation are now allocated for completion. Registration policy, event completeness, holding verification and unavailable random snapshots must be explicit. Observed events may support a disclosed model but cannot establish exact historical snapshot awards. No current completeness or claim of guaranteed backdating is made.
- **S2-DECISIONS-01:** dependencies are proposal inputs, not new technical gates. There is no approval request in this setup pass. Before any later formal contract/plan approval ask, push the readable artifact to the working branch and cite path plus sha256 under the repository scope-gate instruction.
- **TOKEN-CUSTODY-01:** separate subproject 9; parked by Rob on November 4. Preserve the historical 5%-of-supply Season 1 design, 40%/70%/100% claim ladder and target dates as design history only. Neither final points nor that specification establishes a funded token pool, deployed claim system, SNS/DAO decision, launch date or permission to execute one.
- **POOL-GROWTH-CUSTODY-01:** separate subproject 10; parked by Rob on November 4. Prior promotion chat `01a19802-999b-76e2-aeb2-0a4cb5dfc5d3` contained ideas, not publication. Source reconciliation uses `origin/main` at `279fdc2c5f0cba08e89d50cfc4e471c7216b399a`: `docs/coordination/fiat-stable-points-policy.md`, `src/rumi_points/src/accrual.rs`, `src/rumi_points/src/state.rs`, and `scripts/airdrop-report.py`. They distinguish historical matched 5x/unmatched 3x, separately recorded uplift, and the scheduled flat-4x policy. The primary checkout's older 10x report label is not current main. Source does not prove current live configuration or future eligibility.

## Allocation and boundaries

November 4 follow-up: one bounded documentation tranche completed; terms, source-based coverage limits and decision package are in Review. Current deliverable: reconciled backfill and tested segregated scoring candidate, then exact release decision and verified accrual. November10 direction supersedes the earlier terms hold: continuous November1 accounting is allocated, and neither token allocation nor an undecided end blocks it. Historical random captures never made require an explicit disclosed accounting method; retained source history is being reconstructed before a concrete decision packet. No live activation is authorized.

Requested coordinator model: `gpt-6.1-sol`, high effort. Actual runtime model is not independently attested here. One bounded extraction/drafting worker requested gpt-6-luna/medium through native collaboration; runtime model not independently attested. Orin reviewed its evidence and wrote the final recommendation. No builds/tests/CI or new paid credits. Future narrow evidence work should use the smallest capable approved worker; record requested/actual model and any fallback on dispatch. Keep the aggregate Rumi allocation near the CTO's approximately 20% guidance; top recoveries retain priority.

Demo-first policy applies. A separate-instance source candidate is implemented and functionally tested; it is not deployed or live-ready. Sustained cohort resource measurement is deferred under DEMO-S2-001. Preserve all existing credits, receipts, signing, governance, deployment, secret and financial action holds. No product membership or product priority writes; Merritt owns those.

Publisher setup and verification receipts stay in ignored `output/orin-incentives/`. Reuse the existing authorized Lookout identity without granting access or creating a competing publisher. Future updates retain this board, stable IDs and source revision binding.

## November pilot review artifact

[Proposed terms and history treatment](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/season2-terms-proposal-20261104/docs/coordination/season2-proposal-20261104.md). One native Luna worker supplied source evidence; Orin checked rate/lifecycle pins and reviewed the recommendation. No live reads or November completeness proof. Recommend the zero-spend pilot package, leaving unobserved awards pending. Await Merritt review; this is not a Rob approval request.

## November history follow-up

Merritt accepted the proposal as reviewable, not approved terms, and asked Rob asynchronously; answer pending. Option A, the pilot package and pending-history default are recommended. The uncalibrated 0.5% reserve is deferred, not a required decision. One Luna worker inspected existing local exports and singleton storage; Orin reviewed the result. Private coverage summary stays in ignored output. No public participant export, live query, award or activation. Next engineering candidate: isolated new-instance initialization contract covering prospective seed timing, registration/initial positions and separate pending backfill; no implementation allocation assumed.

## November 5 clarification — controlling status

Rob directs continuous November1 UTC Season2 accounting with no voluntary uncredited gap. Token allocation/split is deferred, not a preparation blocker. End undecided; January31 tentative only. Nov30, changed rates/eligibility and blanket uncredited default are not approved. This supersedes conflicting earlier recommendations/status notes above. Actual anonymous analytics/source reads confirm substantial history and completed existing Season1 compensation. Detailed rows stay private; no source loss was established from failed queries or incomplete exports. [Clarification and sanitized evidence](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/season2-terms-proposal-20261104/docs/coordination/season2-clarification-20261105.md). No awards, activation, new spending or parked work resumed.

## Retention proposal custody

FEE-NEURON-PROP-01 records Rob requested proposal-only LP plus neuron fee-sharing. Orin retains the readable sketch and questions; no new team, automation, emissions, implementation or rewards. This is future incentive strategy, not reactivation of parked token/claims or promotion, nor a gate on continuous Season2 accounting.

## November 10 delivery resumption

Rob requests Season2 running and eligible points counted from November1 under inherited rules. Orin owns follow-through; fee-sharing stays backlog and claims/promotion parked. Active locked implementation branch codex/season2-accounting-20261110. Current Points reads confirm S1 epoch21, no open epoch and polling/scoring off. Next: private per-category replay, segregated candidate and focused tests; then concrete release preparation with Dorian. Provisional estimate2–4hours tested candidate, up toone workingday full reconciliation if inputs align; no deployment promise. No missing token-budget/end-date decision is an accounting blocker.

### Current worker allocation

S2-DELIVERY-01: requested gpt-6.1-terra/high for bounded cross-store retry-safe bootstrap, justified by auth, stable-stage/digest/cursor/receipt integration and S1 preservation. Sonnet login check returned loggedIn:false; alert surfaced. Existing gpt-6-luna/medium worker owns private source replay. Runtime model identity independently unattested. No new team; Orin integrates. Dorian direct coordination permission received and message sent. No live scoring mutation yet.

## November 10 correction — controlling release status

The earlier fixed-cutoff approval request expired without approval. Its estimated credit decision and 20-point allowance are withdrawn: current records show positive balances for the old vaults, and the public liquidation-history query supplies the per-ledger payment omitted from the earlier analysis. Replay disagreement is unfinished reconciliation, not proof that source history is missing. Retained history is being reconstructed before a corrected packet; no action is requested from Rob now. Funding is received, while creation, installation, credit import and activation remain unexecuted. Approval-relative timing preparation does not authorize credits or execution.

## November 10 prospective-first direction — controlling delivery status

Rob directs prospective scoring to run without waiting for historical reconciliation. The tested candidate now permits a zero-credit all-eligible bootstrap at fixed T0, real ongoing epochs and separate once-only later credits for [November1,T0). The exact prospective-only packet is pushed and approval is pending; Dorian coordinates after the approved Vault51 release. Historical replay found a concrete stale-price cause and improved matching debt balances from10/49 to41/49, with eight global residuals including two cohort debts. No estimate or20-point allowance is revived, and history does not gate the prospective slot. No creation/install/start or historical award has occurred.

## Included Claude queue — November 10 Pacific

S2-R8-01 is prepared locally but not dispatched. Scope: one source-only synthetic operation-accrual timing reproducer and test-only correction or precise unavailable-input finding; Codex then independently inspects the patch/focused tests and runs the private eight-residual comparison if supported. Current configured launcher reports loggedIn=false, authMethod=none; included usage is not verified. No API/paid fallback, raw participant export, new team, duplicate review or live action. Ansel coordinates shared login; dispatch requires actual authenticated subscription, included allowance and enforced source-only data boundary. Existing prospective packet/module stays held pending direct exact approval and is unchanged by this queue.

## Claude launch attempt correction — November 11

Host subscription authentication is verified (Claude.ai/firstParty/max); the earlier sandbox logged-out status was a visibility issue. The shared included-allowance receipt was read. Both actual launch requests were rejected before process creation by automatic approval review, including after the original Rob messages were independently read. The review does not accept cross-chat authorization as trusted approval for this exact source export and test-edit action. No launch UTC, PID, session, Claude diff or worker test result exists; no source was sent. Direct approval of the six-file source-only scope at210bee66 was requested. No alternative launcher/provider or indirect retry will bypass the rejection. Prospective packetbf27c8e3/module2ffde290 remains unchanged and separately held.

## S2-R8-01 actual launch — November 11

Direct human approval in this chat now authorizes the exact six ordinary source files and synthetic-test edits. The native included-subscription worker launched at16:11:28UTC/08:11:28Pacific, PID72409, sessionf64f5147-7467-4eed-92a0-3dbe6c726b3c. Initialization reports claude-sonnet-5-5; scoped Read operations are observed. This supersedes the earlier zero-launch blocker for this exact task only. No participant data, API/paid fallback, shell/network tool, production change, new review campaign or live release is authorized. Codex owns diff acceptance and focused tests.

## S2-R8-01 accepted worker result

Claude completed at16:16:54UTC with successful result and zero permission denials. Codex independently inspected the267-line addition inside the existing cfg(test) module, ran all six named tests successfully and preserved local commitbc6510a8. No production source, Candid, dependency or deployed artifact changed. Four actual operation memo clocks have now been privately validated against indexed ledger blocks with actor/amount/default-account bindings; Codex is running one frozen endpoint-only comparison. This is not a historical award transcript or a completed8→N claim. The next useful task depends on that returned residual cause.

## S2-R8 completed comparison — controlling history status

Both Claude source-only tasks completed and were independently accepted; six plus two focused tests pass. Corrected private replay finished at276.24seconds: global debts42/49 match, residuals8→7; validated-cohort residuals2→1. Four actor/amount-bound memo checkpoints were processed, including a verified zero-debt opening no-op. The first adapter attempt produced no count and is not reported as a balance failure. Next queued work is Codex-owned retained redemption-claim timeline/valuation reconstruction; raw records remain excluded from Claude. Earlier first16h51m and unanchored global cases remain unresolved. Prospective packet/module and exact live-release hold are unchanged.

## November12 approved release execution — controlling status

Rob directly approved the prospective-only packet at22:29:14UTC in Merritt. Dorian cleared the backend slot. Fresh subnet revision620b81c4 has byte-identical pricing to the prepared revision; ledger fee and13-node eligibility match. New canister2cvsh-3aaaa-aaaaj-qspga-cai is installed, with controller/admin, fixed23:45UTC cutoff, H0 and inherited bindings read back. Both timers are off; begin_season2_import(0) succeeded beforeT0. No historical points imported. Next is cutoff bootstrap and real scoring start; installed is not yet scoring.

## November12 live prospective scoring — controlling status

Season2 is RUNNING on2cvsh-3aaaa-aaaaj-qspga-cai. The15-row all-eligible zero-credit bootstrap finished with exactT0 cursors562874/110/2596/5; manifest3ea7394dcc25c73edb17876b2c0b5fcc1fafbe0e19b2e921ece3edd3d2309e41. Post-cutoff poll completed; epoch0 coversNov12 23:45UTC→Nov19 23:45UTC, polling and driver both enabled. Independent anonymous readback confirms state; uploaded gzip hashd301b54 matches approved bytes, decompressed module2ffde290 verified locally. Source debit900.1B, remaining deploy account3.40055T; scorer388.66B and128.1MB at post-start readback. No historical credits/backfill or ordinary awards have occurred. FinalizedS1 remains epoch21 with both timers off. First actual capture and weekly close are subsequent proof stages; future capture times and private source rows remain local. Sanitized receipt pushedf0b8d320.

First automatic poll independently observed Nov12 23:52:41UTC: backend cursor advanced562874→562885 without manual trigger after start, matching the fresh global source tail. Epoch0 and both timers remain enabled. Public receipt updated2f34a7b5; first balance capture and weekly awards remain subsequent stages.

## November12 retained claim journal recovery

S2-CLAIM-03 recovered8 original Prepared→ClaimConfirmed transitions from the existing local snapshot private append-only journal. Independent CBOR versus raw-Candid binding matched all19 final claim fields; the Prepared quote also matches the independent global-protocol receipt interval calculation. This resolves the prior unavailable-original-quote and unavailable-initial-save inputs without a snapshot inside the short Prepared interval. One offline endpoint-only replay with actual journal entries is underway. Raw snapshot bytes, journal records, participant data and future capture schedule remain local; no new Claude inference or historical award.

S2-CLAIM-03 completed: original claim journal recovery resolved the Prepared quote/save-time inputs. The single native replay passed with the same frozen endpoint/late anchor; seven global/one tracked debt mismatch remains, with the tracked discrepancy reduced>99.99%. No fitted balance, historical award or production change. Sanitized evidence90db3f61. Existing query-only first-capture observer remains active; the future schedule stays private.

## November13 first capture and bounded residual inspection

Fresh live readback proves epoch0 A capture complete, B incomplete, both timers enabled and source cursors progressing. The old observer process was not available and had saved no completion receipt, so prior pending/observer-active wording is stale and superseded. Receipt4117ba1b records the new evidence; ordinary ledger and historical credits remain zero.

The frozen debt residual remains nonzero. The common historical anchor uses local file-write times, not canister-returned clock state; CandidVault omits last_accrual_time. Successful repayment/withdrawal memo checkpoints and all8 claim stages are already applied, and the accepted dust case does not describe the recorded non-dust operation. Next source-supported owner work is local original-call/anchor provenance auditing, including rejected validation paths that accrue before failing without a success event. No such historical failed call is proved yet. No new Claude inference: another synthetic timing/dust test would repeat accepted coverage, while the current localization needs private evidence. No stale allowance or expanded export assumed. SEC19 untouched.

## November 21 stalled scorer repair — controlling current status

The earlier expectation was B capture and weekly close after the completed A. Actual readback is an overdue open epoch with B absent, zero ordinary ledger and stationary source cursors. Running/enabled flags did not prove successful tracking. Current usable cycle headroom is insufficient; exact installed-module local reproduction confirms the sender-admission mechanism, while the original missed-B instant is not reconstructed.

Orin recommends narrowing the immediate work to the prepared late-capture guard and truthful S2 current-status display. Those source outcomes are built/tested and pushed; customer impact is still unverified because neither is deployed. The next decision is bounded UI publication and safe stop/fund/guard-upgrade approval, with missed-week recovery policy separate. No production funding, deployment, activation, historical award or SP/financial-hold change occurred. Packet: https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/4dea81f3/docs/operations/season2-repair-20261121.md

## Orin handoff entry point

Before each material closeout or handoff, reconcile this queue, priorities and plain summary with canonical evidence. Publish material changes through the existing authorized incentives publisher and verify its dry-run readback; record old/new revision, classification, source and readback evidence. Leave unchanged paused/held work unchanged. This does not grant release, financial, point-award or access authority.

## November 21 forward-scoring scope correction

The guard-only packet is containment, not delivery of Rob’s requested working Season 2. The current open-epoch and single-buffer model lacks an S2 archive/advance method; the existing held-resume methods are S1 epoch18-only. Orin is preparing the additive archive-and-resume path, with epoch0 and all unscored coverage retained pending recovery and only a genuinely new prospective window eligible for normal scoring. Source implementation and its essential verification are in progress; no forward activation, funding, deployment or earned-point decision has occurred.

## November 21 full forward release preparation — controlling status

Source2d0be167 adds the bounded private epoch0 archive and actual forward activation, not merely a held guard. UI9a846fa0 keeps earlier unresolved S2 coverage visible alongside future awards and retains S1 history separately. The exact canonical candidate passed the ordinary-timer acceptance: A/B, prorated close and next epoch2, with no epoch0 summary or activation credit. Timer-aligned polling had starved close; the concrete priority fix passed the same failed acceptance and focused review. Both reviewers PASS with existing repayment-attribution and fail-closed capture-tail limits explicit.

One exact production packet is ready; no source build, new team or broad audit is queued as an extra gate. Live scorer and public banner remain unchanged until approval/readback. The existing deploy ledger already holds the proposed funding; no new wallet transfer is needed. Original unscored coverage remains pending, not zero or forfeited. Earlier guard-only packet4dea81f3 is superseded. Packet: https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/32fdd7ac/docs/operations/season2-repair-20261121.md

Board reconciliation: revision24 →25 corrected the accepted scope to actual forward scoring, and its unchanged dry-run verified board, presentation, plain summary and priorities. The completed-source/production-approval state is a further material update; publish/readback its revision before handoff. Parked rows and original completion dates remain unchanged.

Proposed durable owner/catalog note, verified by this incident: a manual forced-tick close does not establish periodic-driver liveness. Use one ordinary-timer acceptance for a changed poll/close lifecycle; source evidence is the reproduced guard starvation and its timer-driven regression. This is a proposed catalog lesson, not an additional review campaign or memory edit.

Final material publication/readback: revision25 →26, source revision `b3afc67998b156acb7efaa265dfa8fa7a42a81b9d6f80ee0b6e4f2f901a4016f`; dry-run confirmed board, presentation, plain summary, priorities and repository association unchanged. Classification: stale-corrected accepted scope (24→25), then current source readiness held for exact production approval (25→26). Existing parked projects remain unchanged-paused. Source entry point: this file's Incentives task queue and Orin handoff entry point. No completion dates or action holds were reset.

## November24 approved release preflight and history audit — controlling status

Rob approved packet32fdd7ac in Ansel; approval is approved, execution blocked by automatic review of the initial stop. Read-only preflight and all761 UI asset hashes passed. No stop/funding/publication/upgrade/activation occurred. Direct authorization in this execution chat is required by review, with no change to the approved scope. The pinned activation deadline remains Nov26 15:05Pacific; missed points remain pending. History audit verified90 baseline inputs and the original file-time anchor, but recovered no hidden clock or failed-call receipt. See [the bounded result](season2-preflight-history-20261124.md). No tests were repeated and no new team or private export was used.

## November24 release executed — controlling production status

Direct approval here resolved the prior execution-review boundary. Exact packet32fdd7ac was executed: one1T+100M funding, private verified backup, exact guarded upgrade, epoch0 archive,26 held catch-up passes and real epoch1 activation at21:02Pacific. The banner is published with served-byte and browser proof on both points and global Borrow header; original keys/properties/controllers/environment preserved. First ordinary timer advanced583324→583329 without a manual trigger. No activation or historical point award; captures and first weekly award remain subsequent proof. Original bootstrap/backfill/archive preserved; earlier intervals remain pending, no forfeiture/compensation. Continue existing owner follow-through; no repeated source tests, new team or added release gate. See [the exact execution receipt](../operations/season2-release-execution-20261124.md).
