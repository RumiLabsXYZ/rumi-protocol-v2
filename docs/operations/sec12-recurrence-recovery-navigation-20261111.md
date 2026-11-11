# Saved-request recurrence: recovery navigation — November 11, 2026

Accepted frontend code: **ef5b6506c8010c71fdb6c5772c186f5534abfee6** on
`codex/vault-pull-recovery-navigation-20261111`, based on producer801d60aa and
accepted OISY codecb1701ee. Four production frontend files and four test/fixture
files change. Backend, API/journal production logic, Candid, dependencies,
OISY connection repair, ckSOL processing and reserve state are unchanged.

## Diagnosis and limits

A browser intent is saved before approval. If approval is interrupted, the exact
request remains saved even though no backend operation has been dispatched.
Changing collateral or amounts then reaches the exact-intent guard before a new
approval. An earlier completed loan does not retire a later interrupted request.
A regression reproduces acknowledged sequence2, interrupted new request3,
changed-term refusal with no second approval/dispatch, then backend-fenced
cancellation3 and admission of a new exact request4. These are synthetic fixtures;
they do not identify the customer's actual interruption cause, phase or transfer
outcome. The earlier customer-reported success remains valid historical evidence.

The form's held-request error had no direct recovery action. The global recovery
panel was rendered outside main content, before the fixed-header spacing. A
browser fixture using the actual layout/home/recovery components reproduced saved
amount text at y29 under fixed header/portfolio strip ending at y120. The strip
and decorative widgets were mocked; the actual customer viewport was not measured.

## Correction

- At a saved-operation form error, the owning connected wallet sees **Review
  saved request**. The healthy form and unrelated validation errors have no new
  control. The action requests only the existing guarded status read.
- Recovery moves inside the main content's existing fixed-header spacing. The
  explicit review action scrolls and focuses it, fenced to owner, wallet type,
  session and the active review. OISY's existing channel opens directly from the
  click, never from a new mount/background request.
- The panel shows the original exact collateral and borrow amounts. Restore is
  review only; Continue is separate and uses the saved terms rather than edits
  to the form. Cancellation still requires a separate confirmation and the
  existing authoritative backend fence. No automatic cancellation, clearing,
  approval, transaction, account adoption or retry is added.
- Submitted/unknown transfers retain the exact-receipt recovery path. A missing
  browser intent with an active operation is shown as held/support-needed;
  a failed status read is not treated as empty. Empty status is described without
  automatically starting a replacement.
- Both reviewers identified a minor duplicate-click empty-notice edge. A focused
  red-to-green test and one-line epoch correction preserve the initial read's
  completion notice without a second read or financial action.

## Verification

Initial focused run: **56 tests /4 suites passed**. This covers the durable
journal, actual home-page error→recovery wiring, saved-request controls and OISY
status consent. After the small epoch correction: **18 targeted tests /3 suites
passed**, including the new repeated-click regression. These runs overlap; they
are not74 distinct tests. Production journal logic stayed unchanged.

Two independent adversarial source reviews passed, followed by both focused
confirmations of the one-line epoch change. No blocking findings remain in this
scope. Typecheck remains at **30 existing errors /66 warnings**, including known
warnings on unchanged home-page markup; no new diagnostics on changed lines.
`git diff --check` passed. No production build, CI or publication was performed
by the source owner.

Synthetic rendered checks at1280×720 and390×844 used actual home/layout/recovery
components and installed Tailwind preflight with decorative widgets mocked.
The corrected settled recovery region was y140 and saved text y159, below the
fixed strip ending at y120. Mobile scrollWidth equaled viewport390. Review and
Restore left continuation/cancellation counters at zero; cancellation showed a
separate confirmation. Submitted state exposed receipt recovery without cancel,
and an injected unavailable read stayed explicit. No relevant console error or
warning occurred before that deliberate synthetic read failure. Browser viewport
was reset, temporary tab closed, and the positively identified localhost fixture
process stopped. Screenshots were emitted through the browser tool, not committed.
No real wallet or canister transport was used.

Retained local evidence SHA256:

| Evidence | SHA256 |
| --- | --- |
| Initial56-test log | 0ca4a1573a537a738a2761670ce2151fe33b2fae72767ee6065ce1356c8379ab |
| Final18-test log | 741ea2be403369993839dfa66b572f98d2ccb5ddc9e74bd5dad0d76d76682782 |
| Typecheck log | 23112cdd110d433026b215f8869e00f2df7e1f00c0651caf6a6ef5a55dfc97a1 |

## Release and customer acceptance

Dorian owns the single coordinated frontend build and additive publication lane.
Use the current paired backend and preserve its module/state, all old frontend
keys/properties/untouched encodings and auth/domain files. Independently verify
frozen artifacts and served version/routes/recovery code after publication.
No backend installation, reserve action or financial canary is part of this fix.

Keep SEC12 open until the software is deployed/read back and the customer can
recover their actual saved request. Source/tests/mock UI do not establish that
customer outcome. The owner's safe choices remain review→explicit continuation,
or confirmed cancellation only when the authoritative phase permits it. Unknown
transfers remain protected. No agent should act in the customer's wallet or clear
browser data. The separately accepted OISY swap repair remains Done.
