# Earn navigation implementation brief

Status: Product direction and first-party Anthropic source sharing approved.
Implementation and verification are tracked on `codex/earn-navigation` and its PR.

## Accepted behavior

The primary navigation becomes Borrow, Earn, Swap, Vaults, Explorer, with the
existing Points treatment preserved on desktop and mobile. Earn links to a new
`/earn` overview. Remove the standalone 3USD navigation item and the primary
navigation's APY pill.

Show both opportunities immediately on the overview:

| Opportunity | Explanation | Action |
| --- | --- | --- |
| Stablecoin Liquidity · 3USD | Deposit stablecoins, receive 3USD, and earn from swap fees and a share of borrowing interest. | Provide liquidity |
| Stability Pool | Supply liquidation funds and receive collateral gains. icUSD deposits also earn borrowing interest. | Deposit |

Each opportunity has its own existing live rate calculation, loading state, and
unavailable state. Explain that stability-pool deposits can be converted to
liquidated collateral, while 3USD represents a share of pool liquidity. Identify
the stability-pool interest rate as applying only to icUSD, excluding liquidation
gains. Preserve the existing eligibility exclusions and financial calculations.
Do not invent rates, imply guarantees, or add automatic polling.

Keep `/3usd` and `/stability-pool` working. Shared secondary navigation links
between the overview and the two opportunities; Earn remains the active parent.
Use accessible links and retain responsive behavior.

Swap's Provide liquidity entry and its 3pool selection lead to the canonical
`/3usd` experience. Preserve AMM liquidity management, including its paused
deposit state and existing withdrawal path.

Use Deposit and Withdraw as the primary 3USD form labels. Explain that a deposit
receives 3USD representing the user's share of the pool. Retain technical
mint/redeem terminology where needed in transaction details and identifiers;
leave wallet and transaction behavior unchanged.

Keep manual Liquidate in the footer and provide a contextual link from the
stability pool. Preserve the current dark, teal, and purple visual system.

## Sonnet provider and source envelope

Destination: first-party Anthropic through authenticated local Claude Code,
using Sonnet. Repository content read by the worker is sent to Anthropic for
model processing. This includes scoped unpublished changes and local test or
build output, not only source already published on the default branch.

Assigned worktree:
`/Users/robertripley/.codex/worktrees/a239/rumi-protocol-v2`

Assigned branch: `codex/earn-navigation`.

Permitted reads:

- This brief, repository `AGENTS.md`, frontend package manifests and test/build configuration.
- `src/vault_frontend/src/routes/+layout.svelte`.
- Frontend routes `earn`, `3usd`, `stability-pool`, `swap`, and directly relevant pool documentation.
- Frontend components under `swap`, `stability-pool`, `layout`, and `common` needed for these flows.
- Existing frontend rate helpers, related services/stores/utilities, and focused tests needed to understand and verify this change.
- Diffs and focused test/build/browser evidence for the same frontend scope.

Permitted writes:

- The main layout and the `earn`, `3usd`, `stability-pool`, and `swap` page components.
- Shared Earn navigation components and 3USD liquidity form copy.
- Additive completeness metadata in the existing frontend rate helper, preserving its financial calculations and existing numeric fallbacks.
- Focused tests and directly related navigation documentation if needed.
- Local worker handoff files containing scoped implementation and review evidence.

Excluded: secrets, environment files, credentials, private keys, unrelated
repositories or source, backend and canister changes, Candid or generated
declaration changes, financial calculations, transaction services, dependencies,
deployment configuration, wallet connections, signing, and live transactions.

## Worker ownership and verification

- EARN-01: Sonnet implementation worker, one writer for the above files.
- EARN-02 and EARN-03: independent Sonnet reviews of the final diff, with no edits.
- Coordinator: dispatch, diff adjudication, local checks, browser verification,
  commit/push/PR/merge, and final evidence.

Verify shell, repository read, and edit capability before accepting implementation
work. Do not substitute another model when Sonnet cannot run. Workers must return
actual model evidence where available, changed files, checks, and unresolved issues.

Validation covers frontend build, relevant typecheck results with baseline errors
distinguished, focused tests, and desktop/mobile navigation and deposit entry
behavior in the local browser. Run deterministic checks before the independent
reviews; resolve confirmed findings before source delivery.

Source delivery includes commit, push, PR, and merge after applicable checks.
Deployment and all live mutations remain outside this task.
