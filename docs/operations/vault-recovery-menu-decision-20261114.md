# Focused vault recovery menu correction — 2026-11-14

## Scope and authority

Rob requested a compact alternative to the large generic recovery card shown on every OISY connection, then directly authorized finishing, verifying and publishing this focused correction. Original approval: Merritt chat human message `01a1db85-3658-7c01-8637-73e787fa6eb4`: “Okay, have Ellis finish that and get it published once it's verified. Let me know when it's live.”

Base: deployed SEC19 source `4d8322a71059f146f6ec44c95add49789be2b6f8`. Frontend-only presentation change; no backend, journal, amount conversion, approval, transfer, reserve or other financial changes.

## Diagnosis and behavior

The global layout mounts VaultPullRecoveryBanner before page content. Every connected OISY session starts `manualCheckPending` because authenticated status reads require explicit user consent. The previous template rendered that unknown state as a prominent card, even without evidence of an unfinished action.

Remove only that generic template branch. Connected users can choose **Wallet → Recover a vault action**, a secondary menu row. Opening the menu makes no recovery call. Selecting the row closes the menu and synchronously dispatches the existing owner-scoped recovery event; the existing handler opens OISY before its first await. Genuine saved requests, submitted transfers, held collateral and errors retain their existing prominent guidance. Explicit empty-result guidance and contextual form navigation remain. Cross-browser discovery requires no local intent. Owner/session guards and restore/cancel confirmations remain unchanged.

## Verification

- Red regression reproduced the original unwanted global card in the mounted OISY component.
- Final 23 tests across four mounted suites pass: banner consent, saved exact requests, borrow navigation and real wallet-menu-to-banner integration. These include no mount/background/menu-open status or signer calls, synchronous explicit-click channel ownership, owner/session transition guards, cross-browser submitted discovery and no financial dispatch from discovery.
- `git diff --check` passes. Two independent reviewers returned PASS, then each rechecked the final neutral color/tooltip delta and returned final PASS.
- Actual source components rendered with mocked connected-wallet/API services at 1280×900 and 390×844. Menu and recovery row fit; no horizontal overflow or generic recovery card. Menu-open observations: reads=0, channels=0, financial=0. Explicit click: reads=1, channels=1, financial=0; menu closed. Submitted cross-browser receipt remains prominent. Screenshots and DOM measurements are in [evidence/vault-recovery-menu-20261114](evidence/vault-recovery-menu-20261114).
- This fixture is connected-state presentation evidence, not a real wallet signature or customer transaction. Existing unrelated component accessibility warnings were not expanded into this scope.

## Publication handoff

Dorian is the sole publisher. Obtain a fresh coordinated frontend build allocation, preserve prior SEC19 outputs, and build the exact pushed source once using existing auth verification. Reuse the asset-only publication procedure: fully paged inventory, preserve all existing keys/properties/untouched encodings/auth domains/permissions, stage immutable keys, verify served staged bytes, then activate reviewed entry/version encodings. No backend lifecycle call, snapshot operation, controller change or financial canary.

Ellis independently verifies exact build/source pins, final served entry and relevant immutable bytes, backend/frontend modules unchanged, current version and desktop/mobile page layout before calling it live. Source/test approval alone is not publication proof.
