# SEC12 customer recovery recheck — November 9, 2026

The reviewed implementation remains deployed. Source is `f1d2e77e2387ab6b9fc9a5b5676047b25d7175ff`; backend is Running on `2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`, frontend version `1794195655486`. The existing [release receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/0f00344208bbf9f46184956fafaceb3fa9961ccb/docs/operations/sec12-release-20261108/RESULT.md) remains the installation and historical preservation evidence.

## Fresh verification

- Existing Vitest suites: journal 38, full action boundary 62, mounted recovery components 8; **108 passed** in four files, exit zero.
- Existing cached native library test binary: all **four cancellation/replay tests passed**, exit zero. No fresh Rust build is claimed. The initially selected bin-target executable contained zero matching tests; the correct library executable supplied the four actual tests.
- Two fresh isolated source and customer-copy reviews: **both PASS**, no confirmed code blockers. Customer instructions were corrected to make controls conditional on authenticated phase, require reviewing vaults/balance after retirement before a new request, and avoid promising a separate Internet Identity authorization popup.
- Release owner independently checked current backend Running module, canonical public API and settings. All **18** fresh HTTPS identity checks across primary and canister hosts matched the frozen build, including entry, recovery UI, API service, declarations, version and authentication files.
- Case owner independently matched **six** primary-domain files to the frozen build. TLS validation remained enabled.
- Public ckSOL configuration query confirms Active status, the actual ckSOL ledger and nine decimals. This establishes registration, not eligibility of every loan amount or the customer's current balance.

No code changed during this recheck. The original global typecheck limitation remains 30 pre-existing errors and 66 warnings; no clean whole-project typecheck or new production build is claimed. Previous desktop/mobile UI evidence is retained, rather than presented as a fresh customer transaction.

## Customer boundary

The actual saved intent and authenticated backend phase remain unknown. Caller-scoped operator queries cannot establish that phase. Restore is read-only; Continue uses the exact saved raw amounts and original ID. Confirmed retirement fences no-effect operations before clearing only matching local browser evidence. Submitted, uncertain and post-pull outcomes remain protected. A retired ID may have resolved elsewhere, so review current vaults and balance before preparing another request.

No customer Continue, Cancel, ACK, allowance approval, loan, storage clear or transfer occurred. Historical SOL mint/owner-credit verification is separate from SEC12; no new mint payment is needed for that already processed deposit. Successful collateral borrowing still requires the owner's authenticated action and receipt.

CTO and release chats received the deployment updates under Rob's November 9 explicit messaging authorization. No customer message was sent. The unified case stays in Review pending owner acceptance; ICP reserve activation remains separate.

The guarded worktree inventory/pressure cleanup found zero eligible removals and removed nothing. Dirty, locked, active and unmerged worktrees were preserved.
