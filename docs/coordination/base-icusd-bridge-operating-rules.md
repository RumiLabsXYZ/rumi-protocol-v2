# Base icUSD bridge: worktree operating rules

These rules apply to every agent working in this worktree on the Base bridge.

## Coordination files

- Canonical board: `docs/coordination/base-icusd-bridge-kanban.md` (the only board).
- Derived projection: `docs/coordination/base-icusd-bridge-lookout.json`, shape
  `{cards:[{id,state,owner,scope,evidenceText,evidenceLinks:[{title,url}]}]}`.
  It holds board metadata only. Never include credentials, keys, wallet addresses,
  balances, transaction or provider payloads, private logs, or local filesystem paths.
  Evidence text uses the proof levels `none`, `source`, `local-test`, `local-integration`.
- Presentation sidecar: `docs/coordination/base-icusd-bridge-presentation.json`, shape
  `{summary, glossary:[{term, definition, cardId?}]}` and nothing else: a short plain-English
  status (at most 600 characters) and up to 200 term definitions (`cardId` scopes a term to one
  card, for example R1 on a review card). It is board visibility only, held to the same
  sanitization limits, and never adds fields to the cards. The derive script validates it.
- Priority list: `docs/coordination/base-icusd-bridge-priorities.json`, shape
  `{items:[{id, title, status, blockedReason, nextAction, cardIds}]}` and nothing else; array order
  is the rank; status is one of `ready`, `in_progress`, `blocked`, `paused`, `done`; at most 30
  items; every field present (empty strings allowed); `blocked` and `paused` need a reason that
  says what clears it, `ready` and `in_progress` need a next action; card ids must be current board
  cards. Same sanitization limits as the projection. Mirrored in the board's "Priorities" section.
- Milestone handoff: `docs/coordination/base-icusd-bridge-handoff.md`.

## Lookout publication rule

**Approved for sanitized board metadata (2026-10-24).** Rob approved uploading this bridge
board's sanitized snapshot to his existing private Lookout backend, and future sanitized
board updates, without asking each time. His words, relayed and verified by Miles (Codex
supervisor), who checked Rob's direct message in the Lookout setup chat: "I approve uploading
the Rumi bridge snapshot and the 17-card Musical Chairs update to my private Lookout backend,
plus future sanitized board updates. Keep all connected boards synced without asking me each
time." The chat and turn identifiers and the backend identifier are held by the supervisor and
in the publisher manifest outside this public repository; they are deliberately not recorded
here.

Scope and limits (unchanged by the approval):

1. The approval covers sanitized board metadata only: the derived projection's card id, state,
   owner role, scope, evidence text with its proof level, and public evidence links. Never
   upload credentials, keys, wallet addresses, balances, transaction or provider payloads,
   private logs, or local filesystem paths.
2. Proof levels stay `none`, `source`, `local-test`, `local-integration`. No card may claim a
   live, testnet or mainnet proof level.
3. Publish only through the existing scoped publisher manifest and its scoped identity, to the
   approved private backend. Honor any genuinely new tool or approval rejection: stop and
   report it rather than routing around it.
4. The approval does not touch any other hold: merge, deploy, governance, controller or role
   changes, live signing or broadcast, and funding all stay held.

Event rule (owned by this worktree's coordinator under Rob's standing approval; no setup-owner or
root approval needed): while actively working, promptly record every material task movement,
progress/evidence, owner, blocker, next action or priority change on the canonical board. Update
BOTH statuses in the presentation sidecar: `summary` is Technical and `plainSummary` is a separate
2–3 short sentences, normally 35–60 words, for someone who does not work on the code. Say what
someone can actually do today or how far along the useful result is; name the main thing blocking
the next useful result; then say what happens next and whether Rob needs to do anything. Use
ordinary words, not technical phrases with their abbreviations expanded. No hashes, review
numbers, test counts, acronyms, task labels, release codes, engineering chronology or lists of
technical caveats. Keep the detailed evidence in Technical and the task cards. Read it aloud:
if a friend outside the project would need an explanation, rewrite it. Each summary is at most
600 Unicode codepoints. Keep their facts and proof limits aligned; code, tests, deployment,
execution and settlement are distinct claims, expressed plainly without a disclaimer list. Update affected glossary entries and the
priority list (including card membership, "Priority selection", clause 7) and the board's priority
section; re-derive the projection (which also validates
the sidecar); run the offline priority validation ("Priority selection", clause 5); then run the
same scoped publisher with `--dry-run`. If it reports a change to the board, presentation or
priorities, publish with `--publish`, then confirm an immediate `--dry-run` reports the exact source as already published
(no-op). Never republish identical data. Record each published revision and source revision in
the local publisher record outside this public repository, not here. No hourly monitor or
additional timer. This duty applies while actively working; it does not promise automatic updates
while this chat is inactive. Both summaries bind to the exact board source revision; verify both
in the changed-only publication and immediate no-op readback. Keep real project inputs and
production publisher configuration private; the Lookout code repository is public. A real hold
or denial must be recorded as a visibility blocker and reported to Miles and Rob, never bypassed.
Schema: the Lookout repository's `docs/PUBLISHING.md`.

Presentation cutover (2026-10-24, complete): the root Lookout owner built the board's status and
definitions feature and made the first presentation upload; the publisher manifest now reads
cards and presentation from this worktree's projection and sidecar. A read-only `--dry-run`
afterwards reported both unchanged. Routine ownership is active: each material checkpoint
publishes cards, summary and glossary together with the same scoped publisher under Rob's
standing approval (no setup or root approval), then verifies the no-op readback. Identical data
is never republished. No new wrapper, timer or monitor.

First publication (2026-10-24): the Lookout setup owner completed the initial cutover and
published this projection; the immediate readback was an exact no-op, and this worktree's own
`--dry-run` afterwards confirmed no change. The publisher's manifest reads the projection file
directly, so the board and its single derived projection remain the only source.

History (resolved): on 2026-10-24 the setup owner's automatic approval review rejected the first
Rumi operational snapshot upload, because approval of the exact payload and private destination
was not explicit enough. That command did not run and no cards were uploaded; a "FIRST
OPERATIONAL UPLOAD HELD" rule applied here until Rob's approval above resolved the boundary. The
hold never gated implementation or tests.

### Current standing publication authorization (renewed 2026-10-29)

Miles independently read Rob's direct request in Alden's Lookout owner chat, turn
`01a189b2-fe21-7b53-8e88-c18d11712115`: Rob raised the stale-page automatic-review hold and said
he wants "zero friction for them keeping the live page updated". The maintained private Lookout
publishing guide's October 29 scope renews his earlier approval of future sanitized updates for
all connected projects, including this bridge project.

Routine sanitized cards, Technical and Plain English statuses, glossary, priorities, genuine
completion times and repository metadata may publish to the SAME private Lookout backend using
this project's SAME canonical manifest, publisher and scoped identity. Validate inputs, publish
changed content, verify exact source-revision binding and immediate no-op readback. No new
per-checkpoint permission request, timer, watcher or alternative publisher. Keep the canonical
board authoritative, preserve genuine observation times, and keep the plain status understandable
to someone outside the project. No new completion metadata is inferred from an upload date.

The October 27 rejection remains historical evidence: its command did not run, and no upload
occurred then. Its missing-visible-authorization condition is resolved by this renewed direct
human request; do not perpetuate that obsolete blanket hold. The exact input review document is
retained for provenance, not as a pending approval gate.

Excluded: credentials, private keys, addresses, balances, transaction payloads, raw provider or
support records and unrelated private content. Publishing does not authorize product release,
private-source transfer, governance, signing, broadcast, settlement or funds movement. Separate
source/provider/release and financial holds remain unchanged. A genuinely NEW tool-review
rejection stops that action and must be reported with its exact reason; never reroute or blindly
retry it. Schema/length validation failures are data errors to fix. Current private guide:
the Lookout repository's `outputs/PUBLISHING.md`, October 29 standing authorization.

Audit maintenance: historical milestone receipts do not override this current standing scope.
On October 30 Miles checked the maintained rules, handoff and saved automation configuration;
obsolete present-tense handoff bans were marked historical and no bridge recurring prompt remained.
With no task movement, verify the same source readback instead of republishing for freshness.

## Priority selection

Applies to this board's ranked list (`base-icusd-bridge-priorities.json`, mirrored in the board's
"Priorities" section). Miles decides ranking and execution; the coordinator maintains the list.

1. Work on the highest-ranked item that is `ready` or `in_progress` and that you are authorized
   and assigned to do.
2. When an item blocks, record why and what clears it in `blockedReason`, then immediately select
   the next ready item. Never invent work or take unrelated protected Rumi work to fill a gap; if
   nothing is ready, say so and name the blockers.
3. Re-evaluate at every checkpoint, worker completion, unblock event and existing wake-up. Return
   to higher-ranked work at the next safe checkpoint.
4. Never interrupt an irreversible action in progress, never assign a second owner to the same
   work, never override `paused` items or any hold, and never create a timer or monitor for this.
5. Keep the priority JSON, the board's priority section and cards, the summary and the glossary
   aligned in the same checkpoint. Offline validation at each checkpoint: re-derive the card
   projection, then run a temporary validator kept outside the repository that calls the Lookout
   publisher's exported `validatePriorities` on the JSON against that projection, and confirm the
   board's priority table lists the same items, order, status and cards. No publisher run and no API call
   is part of this check; no repository helper code is added for it.
6. Publication (priority cutover complete, 2026-10-26): Alden (the Lookout owner) made the
   priority feature live and completed the initial priority cutover and publication; the scoped
   manifest's priority path reads this worktree's priority JSON, and Miles' independent
   `--dry-run` reported board, presentation and priorities all unchanged. From now on the priority
   list publishes with the cards, summary and glossary through the same manifest and scoped
   identity under the event rule above: dry-run, publish only changed content, then an immediate
   no-op readback. Exact backend identifiers, revisions and source hashes stay in the private
   publication record, never in this repository. Only the initial priority-publishing hold was
   released; every other hold is unchanged.
7. Card membership (Lookout card badges, 2026-10-27): Lookout badges a card only from an item's
   explicit `cardIds` and the item's array rank (the item title shows on hover), never by guessing
   from titles or keywords. When a card is added or its scope changes, choose its item from the
   card's actual scope, files, dependencies and evidence, never its title alone, and update
   `cardIds` and the board table's Cards column in the same checkpoint. Link completed cards when
   their item is clear; link a card to more than one item only when it belongs to each; leave
   shared or ambiguous cards unlinked and record why in the handoff. Coverage is not a quota. A
   membership change touches only `cardIds`: never alter an item's id, title, status, reason, next
   action or rank to fit a link. Badge readiness hold resolved (2026-10-27): Miles independently
   read Alden's final response in turn `01a17f5e-a18f-7982-919b-caba43c022f5`, confirming the card
   badges are live and deployed frontend assets were verified. This satisfies the temporary
   sequencing condition for the prepared membership checkpoint. Membership changes now publish
   under the same event rule. Only this readiness condition is cleared; real denials and all
   bridge, production, merge, signing, funding and credential boundaries stay unchanged.

## Card ID mapping

Miles proposed seed cards BASE-01..BASE-07 as an early-progress placeholder. Their
definitions were not provided to this session, so they are not mapped yet. The board's
stable IDs are the `BB-*` cards. If the seed definitions arrive, record the mapping here
rather than renaming board cards.

## Live-action boundary

No merge, deploy, live signing or broadcast, funds movement, or controller/role changes.
Public-testnet steps are prepared as a runbook only.
