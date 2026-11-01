# Oisy vault approval UX

Subproject: OVUX-01. Owner: Oisy vault approval UX agent. Source branch: `codex/oisy-vault-approval-ux`. Status: source prepared and independently reviewed; not deployed. No wallet transaction was executed.

## Reported problem and cause

A user reported about eight Oisy confirmations when opening a new ICP vault and borrowing icUSD. The ordinary journal flow sends six authenticated signer calls: initial status, collateral approval, vault dispatch, terminal status, ACK, and ACK status. The recovery banner can also query on connection or background notifications.

Both the pinned PNP signer-agent 3.20.0 and the installed direct signer integration convert authenticated query calls into signed canister calls. `node_modules/@windoge98/plug-n-play/node_modules/@slide-computer/signer-agent/lib/esm/agent.js` and `node_modules/@icp-sdk/signer/lib/esm/agent/agent.js` implement this in `query()`. The exact user's eight screens were not recorded, so source calls explain the mechanism rather than reconstructing every screen.

The active direct integration uses `@icp-sdk/signer` v5, whose API does not expose batching. The existing `oisySigner.ts` records that ICRC-112 was not adopted by wallets; adding old `batch()/execute()` calls would not establish a single confirmation flow.

## Bounded change

- A definitive successful deposit-only `open_vault_v2` reply already proves creation completed. Use it directly instead of another signed journal read.
- A definitive successful `ack_vault_operation` reply returns the acknowledged sequence. Use it instead of a redundant ACK status read. An invalid or lost reply keeps the exact saved intent.
- Preserve the post-dispatch journal read for open-and-borrow: its ordinary success reply does not distinguish a created vault whose borrow failed. Preserve all lost-reply and submitted-transfer recovery paths.
- For Oisy, do not run private status checks automatically on connection or background notifications. Render an explicit check control. Its click opens the signer channel and pins the wallet owner, provider and session generation before querying.

Expected normal signing requests: deposit-only 6 -> 4; open-and-borrow 6 -> 5, plus removal of unsolicited banner queries. These are request counts, not a demonstrated number of Oisy screens. A wallet may show additional permission or consent screens.

Existing ownership checks, amount/operation-ID binding, durable browser intent, cross-tab lock, ambiguous-outcome recovery and the add-collateral pause remain intact. No backend code, Candid, dependency versions, balances or live settings change.

## Verification

- 81 focused tests pass across `vaultOperationJournal.spec.ts`, `actionBoundary.spec.ts` and the actual mounted `VaultPullRecoveryBanner.spec.ts` component.
- Fixtures verify fewer normal reads, preservation of an unknown transfer, invalid ACK retaining intent, no Oisy read on mount/background signals, explicit click discovery, owner replacement and same-owner session transition.
- Production frontend build and `verify-frontend-auth-assets.mjs` pass.
- Two independent bounded source reviewers pass after fixing the click/session race found during review.
- `git diff --check` passes.

Browser plugin was not available. Rendered component behavior was tested under the repository's Svelte/jsdom fixture, not a real connected Oisy wallet. Actual wallet screen counts and production publication remain unverified. No live transfer was needed for source verification.

## Deferred check OVUX-D1

Owner: subproject/release coordinator. Live connected-wallet confirmation count and reload recovery must be demonstrated with an explicitly authorized small transaction before claiming real Oisy UX completion. The current delivery is a reviewed frontend source change with focused tests; this note does not authorize a wallet transaction or publication. Further reductions that need a richer compound-operation reply or another backend change are optional future work, not gates on this bounded source fix.

## Proposed durable lesson

For the persona/catalog owner to review: authenticated queries through the actual Oisy signer can be signed calls and cause user prompts. Audit signer request counts, including background status reads, before adding recovery polling. Trust a certified positive update reply only for semantics it actually proves; compound vault creation can require its journal to distinguish borrow failure. Do not assume ICRC-112 support from older skill examples.
