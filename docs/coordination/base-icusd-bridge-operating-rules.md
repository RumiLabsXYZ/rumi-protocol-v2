# Base icUSD bridge: worktree operating rules

These rules apply to every agent working in this worktree on the Base bridge.

## Coordination files

- Canonical board: `docs/coordination/base-icusd-bridge-kanban.md` (the only board).
- Derived projection: `docs/coordination/base-icusd-bridge-lookout.json`, shape
  `{cards:[{id,state,owner,scope,evidenceText,evidenceLinks:[{title,url}]}]}`.
  It holds board metadata only. Never include credentials, keys, wallet addresses,
  balances, transaction or provider payloads, private logs, or local filesystem paths.
  Evidence text uses the proof levels `none`, `source`, `local-test`, `local-integration`.
- Milestone handoff: `docs/coordination/base-icusd-bridge-handoff.md`.

## Lookout publication rule

**FIRST OPERATIONAL UPLOAD HELD.** On 2026-10-24 the Lookout setup owner's automatic approval
review rejected the first Rumi operational snapshot upload, because approval of the exact
payload and private destination was not explicit enough. The command did not run and no
cards were uploaded.

Until trusted evidence resolves that boundary with appropriate human approval:

1. Do not publish, relay, or upload any board snapshot to Lookout, whether through this
   session, another identity, another tool or executor, or an altered wrapper.
2. A later confirmed manifest or an empty project registration does NOT clear this hold.
3. Keep the board and projection current locally at each material milestone. That is the
   only Lookout-related action allowed.
4. This hold is a visibility hold only. It is not an implementation or test gate.

Once the hold is cleared by appropriate human approval, the event rule becomes: after each
material board checkpoint, publish the changed sanitized projection with the confirmed
scoped publisher, then verify readback and a no-op republish. No additional timer.

## Card ID mapping

Miles proposed seed cards BASE-01..BASE-07 as an early-progress placeholder. Their
definitions were not provided to this session, so they are not mapped yet. The board's
stable IDs are the `BB-*` cards. If the seed definitions arrive, record the mapping here
rather than renaming board cards.

## Live-action boundary

No merge, deploy, live signing or broadcast, funds movement, or controller/role changes.
Public-testnet steps are prepared as a runbook only.
