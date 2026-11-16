# Recovery-menu restoration after canonical frontend regression

Owner: Ellis; one coordinated frontend-only publisher. November15 Pacific. This restores the already approved SEC20 UX on current canonical main, preserving PR666 wallet-session/security protections.

## Confirmed served regression

Fresh primary-domain responses at `2026-11-16T02:57:13.665Z` (November15 Pacific) identify version `1794779370215`. `/vaults` SHA256 is `cef95b895b9563d223e82f215b79a9849b6f769613c99aa1497876809718d584`; the entry references root `/_app/immutable/nodes/0.CKx57ZA5.js`, SHA256 `99bf0829eb9d0fe980ec3c3cb6ed518058ab8390f4cfe9989e13c523fa1c23de`. That served root contains the old generic prompt and button and does not contain “Recover a vault action”. This is served release regression evidence; a stale-tab/cache explanation is unnecessary and unproved.

Prior SEC20 code `8a64f8188422d1e7692fca67531782fb2f04d241` was published and retained in SEC21 `e7b736c9`, but was not an ancestor of canonical source. PR666 canonical source `f72ede81` reintroduced the unknown-state template branch and lacked the wallet-menu row. Refreshed canonical main `9b7669e7` still has those old behaviors. It also contains newer session-bound recovery code that must remain.

## Minimal correction and durable integration

- Remove only the five-line generic `manualCheckPending` template branch. Unknown/unchecked OISY state does not imply a pending financial action.
- Restore the secondary **Wallet → Recover a vault action** row using the existing synchronous owner-scoped review event. Opening the wallet menu performs no status/signing/financial call; selecting the row explicitly opens the OISY channel before awaiting status.
- Keep genuine saved/submitted/held/error recovery visible and all newer PR666 protections intact. The **entire Banner script block is byte-identical to canonical base**; only the template branch changes. The restored WalletConnector is byte-identical to the previously verified SEC20 component.
- Retain current canonical regression tests and adapt only their discovery action. Add the actual wallet-menu→banner mounted suite. The existing frontend CI step now runs both recovery suites alongside XRP deposit guidance, with unchanged jobs, triggers, permissions, dependencies and build steps.
- Merge this patch into canonical main after exact-head CI before publication. Do not publish another isolated historical source branch that a later main-based release can omit.

Bram and Merritt confirmed that the previously approved SEC21 manual-queue availability correction must also be carried into canonical source. ManualLiquidations.svelte and its five mounted regressions are restored byte-for-byte from deployed e7b736c9: existing public bot-claim IDs, reserved/loading/unavailable execution guards, rejection-versus-stale-read fencing, independent XRP settlement and accurate status wording. The canonical component had no newer PR666 logic in those differences. CI includes those suites too. No backend, canister API, dependency, financial, unrelated UI redesign or old-source rollback is included.

## Discovery publication preservation

Dorian is separately publishing only the two approved discovery assets. To preserve them in the next full frontend release, source commit `1b42e9048077dbf0b34505cf4a12e0f97a2311e1` carries exactly three runtime paths from his pushed `7ca180793fa58beecba22ea98f4bc46507754854`: `scripts/generate-frontend-discovery.mjs`, the frontend package postbuild chain and `.ic-assets.json` content-type rules. Both generated mainnet files match his frozen assets byte-for-byte; the local-network run removes them, and the unchanged auth verifier remains first in postbuild. No dependency change or production build was required for these checks. His publication receipt and a fresh live asset inventory must precede our later asset publication; preserve both keys and their properties. This does not change the three UI components or their completed test/review evidence.

## Discovery live-result preservation

Dorian's [immutable publication receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/8a5886b59bac5e5a40b04a2939a4de4bd485c149/docs/operations/icp-discovery-live-result-20261115.md), SHA256 `6de58d96eb82747f0769d6f9c324adaf7cf6aa67c5f25fd781020ee1e7492683`, records batch 704 and the inventory increase from 759 to 761 assets with all prior assets unchanged. The next publication must preserve `/.well-known/ic-architecture` identity bytes (1048 bytes, SHA256 `22bcdf6a5f1ad8e4c1b241a46e3a7f1ae5bec0ab16a79e62ce215288e862f702`) and `/.well-known/ii-derivation-origin` identity bytes (44 bytes, SHA256 `16cf6b5a8eb787b3222ba58308cd6234755b20be869337e1e11e42b51eee53a0`), with their recorded properties. This receipt was read and its document hash independently matched; its live inventory claims remain Dorian's publication evidence until our fresh pre-publication baseline is captured. It does not establish UI deployment, authenticated identity equivalence or financial execution.

## Verification completed

- On canonical base, the restored expectations reproduced three failures across nine mounted tests: the unwanted global card and missing menu control.
- Corrected source:28 tests/five suites PASS, including Banner consent/replacement session and captured refresh contexts, saved exact requests, borrow recovery navigation, actual wallet menu, and postActionRefresh.
- The initial menu-only CI command passed11 tests/three suites, retaining XRP coverage. Final combined local verification passes39 tests/seven suites after restoring the queue guard. The existing CI step now covers both recovery and manual-queue regressions alongside XRP guidance.
- Two independent reviewers returned PASS, then both returned final combined PASS for the queue restoration and narrow CI-test-step additions. `git diff --check` passes.
- Actual current source components rendered with mocked connected wallet/API at1280×900 and390×844: secondary menu row fits, no global card or horizontal overflow, no read/channel/financial call on mount or menu opening. Explicit row click records one read/one channel/zero financial calls and closes the menu. A submitted cross-browser request retains exact-block recovery guidance. This is mocked connected-state proof, not real wallet signing.
- Current PR666 warm dependencies were reused; no package install or production build ran during source work. Guarded worktree cleanup found zero eligible removals and removed none.

[Evidence](evidence/recovery-menu-regression-20261115) retains served identity, source preservation, red/green/CI-command logs and connected screenshots/DOM measurements.

## Canonical CI and merge completed

[PR #671](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/671) merged on November 16 at `0edaf018bbc2b9c5ab648c9244001437e3af5ea4`. Its tree `c6e49ea9ef799757ec7d519edf21a5b494c8c781` exactly equals the tested head `f12337079e7b0ee420920e9210fa97599a65cff8`. The single [Frontend validation run](https://github.com/RumiLabsXYZ/rumi-protocol-v2/actions/runs/43982949272), attempt 1, and its only job `133703764015` completed successfully, including scoped regressions, production build and auth verification. The complete owned-run census was terminal; Cody released allocation `73acd474-7ef9-4e5a-8bf9-793333a3241b`. [Terminal CI receipt](evidence/recovery-menu-regression-20261115/hosted-run-terminal.json) records actual steps and timestamps. No rerun, manual dispatch, extra workflow or local production build occurred.

A fresh independent production baseline on November 16 confirms 761 stable assets (761 identity and 207 gzip encodings), with both discovery identity hashes matching Dorian's pinned publication. Certified frontend module `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`, backend module `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111`, controllers and backend Candid `74cd6c8f7a8b0667bf10f2cd50b1a3c81e55ff863b0cb9520fb8eca1795e03fb` are unchanged. Primary-domain served version/root remain the old regressed bytes. Source merge and hosted build do not establish production publication. Dorian's separately admitted fresh production artifact and the served/layout readbacks remain outstanding.

## Publication boundary

Rob's explicit publish-after-verification instruction for this exact recovery UX is recorded in Merritt human message `01a1db85-3658-7c01-8637-73e787fa6eb4`. The current recurrence report is human message `01a1e247-080b-7240-8a3f-3e71b41f46e7`. Existing scope authorizes the verified recovery restoration and Rob's prior explicit SEC21 queue-publication instruction (Merritt human message01a1dd32-aece-7ee0-b545-2ddbc605e891) covers that already-deployed guard. Bram's current publication coordination confirms carrying both corrections forward. No duplicate unchanged-scope approval is requested.

Before publication: exact-head canonical CI/merge, current certified module/API/controller and fully paged asset baseline, fresh coordinated production frontend/auth build with frozen hashes, bounded additive asset staging and served checks, then reviewed entry/version activation. Preserve the current PR666 backend/security release, asset-canister module/settings/permissions, auth/domain files, old properties and untouched encodings. One publisher, no source campaign repetition or financial canary.

After publication: independently read back final inventory and critical primary bytes; verify old generic prompt absent and menu label present in the active root; inspect actual live route layouts and retain connected-state mock/test limits. No customer wallet action, deposit, allowance, recovery mutation or funds movement is included. Source/tests/CI/merge and served publication remain separate proof grades until each completes.
