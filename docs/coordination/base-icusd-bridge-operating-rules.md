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
root approval needed): after each material change to the canonical board, update the short
summary and any affected glossary entries in the presentation sidecar, re-derive the projection
(which also validates the sidecar), and run the same scoped publisher with `--dry-run`. If it reports a change, publish with
`--publish`, then confirm an immediate `--dry-run` reports the exact source as already published
(no-op). Never republish identical data. Record each published revision and source revision in
the local publisher record outside this public repository, not here. No hourly monitor or
additional timer.

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

## Card ID mapping

Miles proposed seed cards BASE-01..BASE-07 as an early-progress placeholder. Their
definitions were not provided to this session, so they are not mapped yet. The board's
stable IDs are the `BB-*` cards. If the seed definitions arrive, record the mapping here
rather than renaming board cards.

## Live-action boundary

No merge, deploy, live signing or broadcast, funds movement, or controller/role changes.
Public-testnet steps are prepared as a runbook only.
