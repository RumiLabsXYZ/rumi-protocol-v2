# Rumi security and recovery board

Updated: 2026-10-27. Coordinator: Bram. This board tracks the security review; the separate incident and release records remain evidence, not a second task queue. A merged PR, installed module, active feature, and settled obligation are different outcomes.

| ID | State | Owner | Task | Evidence and next check |
| --- | --- | --- | --- | --- |
| SEC-01 | In progress | Bram | Verify Stability Pool recovery and current rails | PR #590 merged. Check current canister state, interest receipts, remaining holds, and source-to-Wasm binding before declaring live closure. |
| SEC-02 | In progress | Bram | Reconcile all accepted audit findings | Compare the September audit, independent October review, and later fixes against current main and live modules. Record each finding's source, deployment, and liability status separately. |
| SEC-03 | Ready | Bram | Verify Points held-epoch resolution | PR #583 merged. Recheck the installed module, held epoch decision, and reward receipts before claiming recovery complete. |
| SEC-04 | Review | Bram | Check 3pool payout and archive recovery | Review exact ledger/archive proof and pending payout identities. Keep ambiguous outcomes held until positive evidence or a safe recovery route exists. |
| SEC-05 | Blocked | Bram | Refresh release and public security status | Needs current live readback and finding closure evidence. Publication must describe the verified production state without private exploit detail. |

## Coordination

- The first active priority is SEC-01. If it blocks, record the blocker and continue SEC-02 or SEC-03 without duplicating another owner's release work.
- No card authorizes a canister install, activation, signing, funds movement, or release of a hold. Use the separate approved release procedure for those actions.
- Whenever a task's state, owner, blocker, next step, evidence, or priority changes, update the canonical board and its sanitized Lookout projection, technical summary, plain-English summary, and ranked priorities as applicable. Publish promptly with the project-scoped identity, then read back the exact revision. This applies while the coordinator is actively working; it is not an unattended sync.
- Write the plain-English summary for someone unfamiliar with the code: use two or three short sentences, normally 35–60 words, covering the useful result or current state, the main obstacle, what happens next, and whether Rob needs to act. Use ordinary words and avoid unexplained acronyms, hashes, pull request numbers, internal labels, and lists of technical caveats. Keep the precise evidence in the technical summary and cards. Read the summary aloud and rewrite it if it needs an explanation.
- Keep explicit card-to-subproject links in the priority sidecar. If a card has no certain link, leave it unlinked until its owner classifies it. Only sanitized summaries and public links may leave the repository.
- Run the Lookout publisher from the Rumi repository root using this directory's manifest. Dry-run first, publish changed data, then dry-run again. The source path in the manifest is relative to the command's working directory.

## Evidence

- [Stability Pool rails PR #590](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/590) merged 2026-10-24. Merge is not proof of installation or historical settlement.
- [Points recovery PR #583](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/583) merged 2026-10-24. The current live epoch and receipts require a fresh check.
