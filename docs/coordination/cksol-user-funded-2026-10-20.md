# Caller-funded ckSOL processing

Request: implement cycles-funded SOL deposit notifications for Plug, Internet Identity, and OISY, keep public Rumi sponsorship disabled, and return unused attached cycles to the payer.

## Payment boundary

The connected principal approves a capped allowance on the official Cycles Ledger (`um5iw-rqaaa-aaaaq-qaaba-cai`). The backend uses `withdraw_from` to collect 1,001,000,000,000 cycles into itself before contacting the fixed official ckSOL minter (`lh22c-kyaaa-aaaar-qb5nq-cai`). Payer, mint owner, and refund recipient are the same authenticated principal, default subaccount. No arbitrary target, method, payer, or refund account is accepted.

The exact allowance is 1,001,100,000,000 cycles: funding plus the 100,000,000 withdrawal fee. Approval has a separate 100,000,000 ledger fee, so the displayed maximum balance requirement is 1,001,200,000,000 cycles. Approval expires in ten minutes and uses the current expected allowance. The payment includes a 1T minter attachment and a 1B processing fee. The latter includes a 100M refund-fee reserve; when the minter returns attached cycles, the backend returns those cycles plus that reserve through Cycles Ledger `deposit` to the payer. Any actual deposit fee is covered by the reserve. A zero minter refund produces no refund call.

Public sponsorship remains off: the old start method rejects, budget reads return zero, and nonzero budget writes reject. User payments do not replenish or enable the public allowance. Ordinary canister execution still consumes operating cycles; this change funds the minter attachment and charges a processing fee, rather than promising zero operating cost to Rumi.

## Durable effects and recovery

Each funding, minter dispatch, and refund effect is recorded before its await. Stable snapshots retain pending and unknown outcomes. A lost response never authorizes a second charge or mint. Withdraw-from `Duplicate` alone is not delivery proof. Refund deposit has no deduplication; it is retried only after the callback proves the entire attachment was returned. Unknown outcomes remain retained for reconciliation, without an automatic retry or an operator proof bypass.

New requests reserve a global monotonic sequence. Same-owner same-sequence requests return the retained receipt. The bounded journal may evict only settled zero-credit rows, retaining active, uncertain, and refundable entries. The stable global high-watermark rejects evicted sequence replay before funding. A stale quote returns exactly `Refresh the ckSOL payment sequence`; only this definitive pre-funding denial permits a fresh client sequence. Existing minter attempt records independently retain queued or unknown deposits until upstream reconciliation.

Processing requires the 10T backend operating reserve plus all user credits. Refunds require backing for user credits but do not hold those credits to preserve Rumi's reserve. A funded user can reclaim credit instead of waiting for dispatch.

Frontend intents are owner-bound and saved before approval/start. Cross-tab writes use Web Locks. Signer principal, wallet session, exact fees, ledger principal, and payment sequence are checked before signing. OISY uses the existing v5 sequential signer integration and backend consent messages. Internet Identity and Plug permissions include the Cycles Ledger. Users must already have cycles in the connected principal's Cycles Ledger account; this change does not purchase cycles or convert ICP.

## Verification

- `cargo test -p rumi_protocol_backend --lib cksol --locked --offline`: 29 passed, including funding/refund uncertainty, replay, reserve isolation, bounded settled eviction, and real upgrade-codec round trips.
- Rust binary `check_candid_interface_compatibility`: strict source equality passed; `didc check` against the base service passed. Backend declaration copies and JS/TS bindings regenerated using the repository script.
- Focused frontend Vitest suites: 72 tests passed after the final recovery fixes, across payment service, existing minter/flow regressions, mounted page fixtures, and wallet permissions.
- Production frontend build and authentication/custom-domain asset verification passed.
- Full Svelte check: 30 errors and 66 warnings in 25 files. A separate archived base checkout with the same installed dependencies has identical error files/messages; the added config entry shifts one existing diagnostic by one line. No new diagnostics.
- Production backend release build passed. The configured metadata + shrink + deterministic gzip recipe produced executable code of 12,434,995 bytes, below the current 12,582,912-byte network limit by 147,917 bytes. Compressed artifact: 3,116,200 bytes, SHA-256 `e0eab8ec13ec3f0a730a6c8705fbcd7ab17f2e4f1ba0b7b1292c6d51dfc523d2`. Decompressed SHA-256 `6e90a765698dc4b7cac2ad03d89ed75c5214f35667c3c65d8e0430c1fa983950`. Gzip round trip verified. This is build evidence, not an install receipt.
- Artifact endpoint check passed against generated Candid with only the seven explicit SDK/lifecycle exports hidden (`timer_executor`, global timer, init, inspect-message, pre/post-upgrade, and `main`). Extracted public Candid passed both directions against the source service.
- Built page visually inspected on localhost, signed out: layout and disabled mainnet-only deposit controls verified. Connected wallet states are covered by component fixtures, not live signatures.

Independent review round 1 identified permanent capacity exhaustion by unfunded owners; fixed with global sequencing and settled receipt eviction, with regression tests. Round 2 identified recovery without browser storage and acknowledgment after a rejected minter call; both were fixed and covered by regression tests before round 3. The page recovers the durable backend receipt without local storage, preserves uncertain local intents, and acknowledges rejected attempts through the existing cooldown-protected clear method. A read-only relay check blocks approval while any prior attempt remains retained. Round 3 found that the paid path bypassed established expired-rejection pruning at the legacy attempt cap. The paid path now reuses that exact cooldown-based policy, preserving all queued/unknown attempts and every paid credit; the regression covers a full map, cooldown expiry, newcomer funding/dispatch, and retained uncertain/refundable rows. Round 4 found capacity could fill while funding awaited; dispatch now rechecks capacity atomically before reserving its attempt. A full-capacity callback retains Funded credit for refund/resume, with an interleaving regression proving no dispatch, duplicate charge, or lost credit. Round 5 fresh independent reviewers Yara and Zane both returned PASS with no blocking findings on the final source. Their reviews explicitly exclude live financial completion.

No production install, activation, token transfer, approval, or paid mint has been performed for this implementation. Live signing, real Cycles Ledger funding/refunds, and actual minter settlement remain unverified.

Worktree inventory: zero eligible removals; zero worktrees removed. Dirty, locked, active, and unmerged worktrees were preserved.
