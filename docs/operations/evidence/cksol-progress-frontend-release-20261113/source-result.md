# ckSOL progress and completion UX — source candidate

Candidate source: `ab44413da1a8b394245cd7734da4124a2335621b` on `codex/cksol-mint-status-20261112`, based on approved repayment source `00cf550d3fcb97870cd4643883d5abd8900fa1a2`. This is separate from the installed SEC17 repayment release. No backend, generated-interface, dependency or payment-service changes; no production build, publication, reserve change, wallet action or real mint.

The mint action now carries a persistent progress panel with a spinner, last successful check, automatic/paused checking, a read-only Check again control and explicit action-needed guidance. An allowance, collected processing funds, dispatch underway and payment settlement remain distinct from official mint confirmation. Only official Minted evidence bound to the current deposit ID displays the exact minted amount, destination and ledger block. Confirmed success stays visible until Start another deposit, with the previous receipt retained separately.

Review found and closed two blockers: legacy deposit-ID changes could retain old Minted evidence until a delayed/failed read; and competing terminal payment histories could hide the current mode's confirmed mint. Every ID transition now invalidates prior evidence and reads. Active/unknown holds retain precedence; terminal histories are selected by the authoritative relay deposit. A final mobile check also removed a contradictory new-SOL prompt during an already queued request. No financial retry or recovery action is triggered by checking progress.

## Verification

- Native focused run: **88/88 PASS**, three suites: page67, ICP panel17, polling4. Single worker, 1536MB Node limit, 3.56seconds. Final log SHA256 `0399a7cd603ca6083a81c43895448cc751365c2d26e1e56e6f5646f9aed0b243`.
- Covers delayed mint after settled payment, exact Minted confirmation, historical receipt versus new request, Approved/Funded continuation, owner/session stale results, unknown/network recovery, changed legacy ID delayed/failed reads, both terminal payment modes and response orders, follow-up refresh, and no duplicate financial submission.
- Rowan independently passed the evidence-reset correction at4005612e; independent frontend reviewer passed cross-mode correction at4005612e and final display-only correction atab44413d. Reviews are bounded source checks, not independent reruns of the supplied tests.
- Actual page compiled locally with existing Vite dependencies and synthetic service/WebLocks mocks; existing Playwright runtime used because the browser plugin was unavailable. Desktop1280×900 confirmation and mobile390×844 queued/error/resume/completion render without overflow or error overlay. Receipt opens, Start another deposit retains history, failed status retains the successful-check timestamp, final queued view has no new-SOL prompt. Console zero errors/warnings in local fixture; recorded financial calls **zero**.
- Screenshots remain outside Git. The fixture omits the production shell and real wallet. This verifies the actual component's mocked behavior and layout, not production composition, wallet signing, concurrency acceptance or a real customer outcome. No broad campaign or redundant production build was performed.

Ready for a separately authorized frontend release review. This document does not authorize deployment or represent customer financial acceptance.
