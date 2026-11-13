# SEC19 — restore safe top-ups to existing ICRC vaults

Ellis owns the existing customer case and integration; Rowan owns backend source/accounting and essential tests; Dorian owns any later release. Rob's screenshot shows an existing ckSOL vault's Deposit action paused before approval or transfer. The amount and user account are not the trigger. This is separate from approved SEC18 frontend progress publication.

## Current cause and accepted hold

Released source00cf550d and approved SEC18 sourceab44413d retain the same hold introduced in accepted PR508, commit3b9e37874688ec21f1692e2dbc598a6fbbe0521e (October10):

- `src/vault_frontend/src/lib/components/vault/VaultCard.svelte:33` hardcodes ADD_MARGIN_V2_ENABLED false; handler682-685 and input/button1070-1082 stop before any approval.
- `src/vault_frontend/src/lib/services/protocol/apiClient.ts:1320-1325` independently rejects before sequential operation/wallet work.
- `src/rumi_protocol_backend/src/main.rs:6937-6950` unconditionally rejects both legacy and operation-ID add-margin methods.
- `src/rumi_protocol_backend/src/vault.rs:3420-3428` names the exact unresolved invariant: a successful submitted pull can outlive its callback; liquidation can remove the target before exact-receipt reconciliation, so a liquidation-compatible recovery liability/refund destination must exist before enabling pulls.
- `src/rumi_protocol_backend/src/state.rs:6273-6275` rejects all AddMargin terminal results. Regression786-823 requires that an otherwise confirmed pull cannot be terminalized without recovery credit. This condition is not fulfilled by the released source; saved-request open-vault recovery does not implement it.

Line references above are pinned to00cf550d, not the possibly divergent primary checkout. A fresh certificate-verified backend6428d103 and Candidf0b0fcc2 match the independently verified installed SEC17 artifact/source linkage; frontend04e565b3/controllers unchanged. No add-margin method was called to prove rejection. No funds movement or customer financial outcome is inferred from a screenshot or source.

## Bounded source remedy and acceptance

CTO assigned bounded source preparation to this existing case. Root reused its clean source isolate at00cf on branchcodex/add-margin-recovery-20261113, preserving pushed SEC18ab/report05ec and dependency link. Rowan actually started backend investigation/source preparation; compilation remains held until the already-approved SEC18 build lane closes or capacity is explicitly coordinated.

Use the smallest compatible existing-journal extension. A confirmed exact owner/operation/ledger/amount/memo-bound receipt may credit the still-owned surviving vault atomically once; if the target disappeared, preserve an accounted owner-bound recovery/refund liability instead. Unknown transfers remain held without double-pull; retries cannot double-credit or double-refund; liquidation must remain safe and collateral accounting conserved. Persisted old journals must remain decodable. Finish essential lost-reply, exact replay, changed owner/arguments, liquidation-before-reconciliation and recovery-liability tests plus independent accounting/security review. Only then pair backend and frontend source enablement.

No flag-only fix, legacy/push-route bypass, live payment/cancellation, new reserve action or silent SEC18 repin. Source/test preparation is authorized; any later production mutation needs its own pushed exact release decision. This is the concrete required recovery gap from accepted508, not an optional general framework.
