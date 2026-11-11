# S2-R8-01: bounded Claude source implementation scope

Purpose: diagnose the remaining offline debt-replay operation-accrual differences using synthetic fixtures only. Codex remains owner and independently inspects the diff and runs the named focused tests. The prospective-only release packet bf27c8e3 and module2ffde290 remain held and unchanged.

## Exact proposed external source boundary

Send only these ordinary task-relevant repository source files to the installed native Claude Code/Anthropic service through the existing Claude.ai included subscription:

- src/rumi_protocol_backend/src/event.rs
- src/rumi_protocol_backend/src/state.rs
- src/rumi_protocol_backend/src/vault.rs
- src/rumi_protocol_backend/src/numeric.rs
- src/rumi_protocol_backend/src/lib.rs
- src/rumi_protocol_backend/Cargo.toml

Base: 5ecd5196. Assigned branch: codex/season2-operation-accrual-20261111, isolated source-only checkout. The six files were locally scanned against retained participant identifiers with zero matches. The dirty private replay checkout is excluded. No retained participant identifiers/amounts, raw logs/events, ledger records, source selectors, private seed, credentials, account details or unrelated chats are provided. No previously denied query or external-source packet is retried through this task.

Claude may read those files and edit only test code within event.rs. Available tools are scoped Read/Edit, with restricted file confinement; no shell, network, MCP, browser, git/push or subagents. Normal customizations/hooks are disabled without using bare/API mode. No production code, stable/Candid shape, dependency or release artifact edits. Codex alone may run the focused tests after reviewing their scope. No creation, install, start, award or financial action is included.

## Work and stopping condition

Use synthetic IDs/principals/amounts/timestamps to reproduce live single-vault interest accrual before borrow/repay/partial withdrawal versus canonical event replay. Include a timer harvest between operation admission and commit. Trace whether an existing serialized field preserves the pre-operation clock; do not equate a post-await event timestamp to that clock. Borrowed debt includes accrued interest already.

Return one concrete reproducer with named baseline/corrected expected invariants and a justified test-only adapter correction, or source-line proof that the needed clock cannot be selected from existing metadata plus the exact next retained input. Stop after one attempt/20minutes. No full replay, ending-balance forcing, credit model or extra review campaign.

If a correction is supported, Codex privately runs one frozen endpoint comparison and reports eight-to-N global and two-to-M cohort residual counts. That retained evidence is never sent to Claude. Repayment-token attribution is a later separate task after this result is accepted.

## Billing/authentication and current action status

The host native CLI is authenticated with Claude.ai, firstParty, max; the shared read-only receipt records included allowance available, zero usage credits and auto-reload off. Use only this included subscription route through the five-day instruction expiry 2026-11-16T07:49:45Z. No bare/API/key-helper/alternate provider, paid context or fallback, credits purchase or settings change. Stop on auth, limit, billing or unknown-route prompts.

Two actual launch requests were rejected before process creation by automatic approval review. Its latest reason: authorization read from another chat/tool result cannot establish the trusted approval it requires for sending private source to Claude and allowing isolated edits. No PID/session exists, no source was sent and no code change was made by Claude. This source-scope approval request is separate from the already pending live prospective release approval.
