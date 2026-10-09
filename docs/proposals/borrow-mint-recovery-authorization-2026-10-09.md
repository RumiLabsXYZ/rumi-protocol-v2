# Borrow mint recovery authorization proposal (2026-10-09)

Status: **proposal only**. This document authorizes no production action and
does not change the current owner-only recovery endpoints. The source changes
in the security worktree have not been merged or installed.

## Problem and existing proof boundary

A borrow mint with an uncertain ledger outcome must block every liquidation
route until exact receipt evidence commits its debt, or complete bounded
history evidence establishes that it never minted. Committing speculative debt
or liquidating against the old recorded debt can misaccount the vault. The
current recovery endpoint is owner-only (`vault.rs`,
`advance_pending_borrow_mint_recovery` and
`reconcile_pending_borrow_mint_from_block`). Thus a non-cooperating owner can
prevent recovery even when evidence can be collected independently. The
new-row history floor and fixed-tip scanner remain fail-closed; legacy rows
without that floor remain positive-receipt-only.

## Requested authority change

Permit **the configured developer principal** as well as the journal owner
to invoke the two existing borrow-mint recovery methods for one vault ID.
No other caller gains that authority. The developer supplies neither a mint
tuple nor a debt amount: both come from the immutable pending journal. The
ordinary backend authentication and frozen-mode checks remain in force.

On an exact positive ICRC-3 mint receipt, the recovery code may apply the
journal's borrower debt and event on behalf of the recorded owner, once,
after the existing journal/claim compare-and-set checks. On complete covered
absence through an immutable post-expiry ledger tip, it may remove only that
unminted pending journal and its recovery sidecar. An incomplete, malformed,
oversized, unavailable, or contradictory history read must leave both held.
No path may release collateral while mint outcome remains uncertain.

For a future journal whose owner does not replay, a separate zero-value,
domain-separated ledger probe with the original timestamp is proposed as an
expiry fence. Only a decoded typed `TooOld` response can arm the existing
history scan. A successful zero-value probe, `Duplicate`, another ledger
error, or an outer call rejection cannot clear or advance the journal. The
probe is not a substitute for the complete scan. Its behavior must be tied to
the exact deployed icUSD ledger Wasm before production use; the current
repository does not record source-to-Wasm provenance for that assumption.

## Scope and verification before merge

- Add developer-or-owner authorization only to these proof-based methods;
  leave borrow, liquidation, transfer, and vault ownership rules unchanged.
- Ensure a developer-triggered positive receipt commits for the journal's
  recorded owner, never the developer as borrower, and cannot double-commit.
- Test wrong caller, wrong receipt, stale journal, overlapping bot claim,
  concurrent owner recovery, incomplete archive page, and full absence.
- Exercise the actual ledger's expiry and zero-value behavior in PocketIC;
  establish deployed icUSD Wasm provenance separately before installation.
- Keep a bounded per-vault operation guard and an operator-visible audit trail.
  A timer or permissionless keeper is a separate choice; it is not required
  for this narrow authorization proposal.

The automatic approval review rejected an attempted source patch broadening
these methods because the financial authorization and owner-scoped debt
mutation need explicit approval. No equivalent patch was applied through
another route. This document makes that specific requested authority and its
limits reviewable.
