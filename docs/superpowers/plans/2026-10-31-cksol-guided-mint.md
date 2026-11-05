# ckSOL guided mint implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace manual completion acknowledgement and refresh navigation with a recoverable Send SOL → Mint ckSOL → progress flow.

**Architecture:** Preserve the existing paid journal, owner locks, exact approval cap and backend receipts. Reconcile terminal relay records inside the next authenticated paid request, before funding. Keep one scoped OISY channel across approval and dispatch. Anonymous reads restore and poll progress and address balance.

**Tech stack:** Existing Rust canister, Svelte page, Vitest; no new dependencies.

**Spec:** Rob approved the three-step flow in this chat and explicitly requested implementation and deployment on 2026-10-31.

## Global constraints

- No financial actions by the agent. All paid calls require the user's wallet consent.
- No stable-state or Candid interface changes; unknown outcomes remain held.
- Only the official minter can establish terminal status. RPC balance is advisory.
- Preserve unrelated working-tree changes and existing frontend assets.

## Review focus

- Same owner returning after a completed mint, including a settled cycles receipt before mint finality.
- Wallet owner/session changes and responses arriving after teardown.
- Lost approval/start/refund replies and concurrent tabs.
- RPC unavailable, balance below minimum and accumulated rent balance.
- Old completed messages must never override current minter or payment state.

## Task 1: Reconcile before funding

- [x] Add failing Rust tests for terminal, pending, unknown and failed minter status; unsettled receipts, stale sequences and same-sequence replay.
- [x] Add a transport status read; under the owner guard verify terminal status and unchanged retained attempt before clearing. Revalidate sequence and payment in prepare before any funding.
- [x] Run focused backend tests and compare generated Candid with source and live interface.

## Task 2: Guided frontend

- [x] Add failing service/page tests for sequential OISY consent, terminal legacy records, automatic restoration/polling, message precedence and owner changes.
- [x] Retain one signer scope across approval and processing; keep explicit resume for interrupted confirmed approvals.
- [x] Show address automatically, advisory finalized SOL balance, one state-dependent primary action, progress and collapsed technical receipt. Refresh reads in background and on focus; stop on teardown.
- [x] Run focused tests, production build and desktop/narrow interaction fixtures.

## Task 3: Review and publication

- [x] Run independent adversarial reviews and fix confirmed issues.
- [x] Push source and reviewable release evidence; merge after checks pass.
- [x] Verify backend source/artifact compatibility and publish the authorized upgrade; publish frontend assets with paginated inventory and atomic entry update.
- [x] Read back deployed hashes/interface/assets and inspect the user's page without initiating payment. Record any remaining limitation accurately.
