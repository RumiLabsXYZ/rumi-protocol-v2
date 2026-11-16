# Recovery-menu restoration after canonical frontend regression

Owner: Ellis; one coordinated frontend-only publisher. November15 Pacific. This restores the already approved SEC20 UX on current canonical main, preserving PR666 wallet-session/security protections.

## Confirmed served regression

Fresh primary-domain responses at `2026-11-16T02:57:13.665Z` (November15 Pacific) identify version `1794779370215`. `/vaults` SHA256 is `cef95b895b9563d223e82f215b79a9849b6f769613c99aa1497876809718d584`; the entry references root `/_app/immutable/nodes/0.CKx57ZA5.js`, SHA256 `99bf0829eb9d0fe980ec3c3cb6ed518058ab8390f4cfe9989e13c523fa1c23de`. That served root contains the old generic prompt and button and does not contain “Recover a vault action”. This is served release regression evidence; a stale-tab/cache explanation is unnecessary and unproved.

Prior SEC20 code `8a64f818` was published and retained in SEC21 `e7b736c9`, but was not an ancestor of canonical source. PR666 canonical source `f72ede81` reintroduced the unknown-state template branch and lacked the wallet-menu row. Refreshed canonical main `9b7669e7` still has those old behaviors. It also contains newer session-bound recovery code that must remain.

## Minimal correction and durable integration

- Remove only the five-line generic `manualCheckPending` template branch. Unknown/unchecked OISY state does not imply a pending financial action.
- Restore the secondary **Wallet → Recover a vault action** row using the existing synchronous owner-scoped review event. Opening the wallet menu performs no status/signing/financial call; selecting the row explicitly opens the OISY channel before awaiting status.
- Keep genuine saved/submitted/held/error recovery visible and all newer PR666 protections intact. The **entire Banner script block is byte-identical to canonical base**; only the template branch changes. The restored WalletConnector is byte-identical to the previously verified SEC20 component.
- Retain current canonical regression tests and adapt only their discovery action. Add the actual wallet-menu→banner mounted suite. The existing frontend CI step now runs both recovery suites alongside XRP deposit guidance, with unchanged jobs, triggers, permissions, dependencies and build steps.
- Merge this patch into canonical main after exact-head CI before publication. Do not publish another isolated historical source branch that a later main-based release can omit.

Separate SEC21 manual-queue differences were reported to Merritt and original release owners; they are not silently bundled into this focused restoration or used as a gate. No backend, canister API, dependency, financial, unrelated UI redesign or old-source rollback is included.

## Verification completed

- On canonical base, the restored expectations reproduced three failures across nine mounted tests: the unwanted global card and missing menu control.
- Corrected source:28 tests/five suites PASS, including Banner consent/replacement session and captured refresh contexts, saved exact requests, borrow recovery navigation, actual wallet menu, and postActionRefresh.
- The exact modified CI test command passes11 tests/three suites, retaining XRP coverage.
- Two independent reviewers returned PASS, then both returned final PASS for the narrow CI-test-step addition. `git diff --check` passes.
- Actual current source components rendered with mocked connected wallet/API at1280×900 and390×844: secondary menu row fits, no global card or horizontal overflow, no read/channel/financial call on mount or menu opening. Explicit row click records one read/one channel/zero financial calls and closes the menu. A submitted cross-browser request retains exact-block recovery guidance. This is mocked connected-state proof, not real wallet signing.
- Current PR666 warm dependencies were reused; no package install or production build ran during source work. Guarded worktree cleanup found zero eligible removals and removed none.

[Evidence](evidence/recovery-menu-regression-20261115) retains served identity, source preservation, red/green/CI-command logs and connected screenshots/DOM measurements.

## Publication boundary

Rob's explicit publish-after-verification instruction for this exact recovery UX is recorded in Merritt human message `01a1db85-3658-7c01-8637-73e787fa6eb4`. The current recurrence report is human message `01a1e247-080b-7240-8a3f-3e71b41f46e7`. Existing scope authorizes the verified restoration; no duplicate unchanged-scope approval is requested.

Before publication: exact-head canonical CI/merge, current certified module/API/controller and fully paged asset baseline, fresh coordinated production frontend/auth build with frozen hashes, bounded additive asset staging and served checks, then reviewed entry/version activation. Preserve the current PR666 backend/security release, asset-canister module/settings/permissions, auth/domain files, old properties and untouched encodings. One publisher, no source campaign repetition or financial canary.

After publication: independently read back final inventory and critical primary bytes; verify old generic prompt absent and menu label present in the active root; inspect actual live route layouts and retain connected-state mock/test limits. No customer wallet action, deposit, allowance, recovery mutation or funds movement is included. Source/tests/CI/merge and served publication remain separate proof grades until each completes.
