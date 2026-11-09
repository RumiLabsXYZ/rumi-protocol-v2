# SEC12: saved vault-pull recovery

Source base: deployed SEC10/SEC11 composition `b78e8db916835ec78a7c812ef3f09e173b2c5155`
(production build source `0eff84c9`, module `b81583ae`). Miles owns this source
lane, Ellis the unified case/Lookout, and Dorian the sole release lane. This
patch does not invoke a customer's wallet, loan, cancellation, approval or storage.

## Behavior and exact boundary

The browser already saved an owner, operation ID and exact arguments before
approval. Editing the borrow form could conflict with that retained request,
without a visible way to restore it or safely retire it. The customer's original
saved arguments are unknown; the screenshot is not substituted for them.

The existing recovery banner now shows the saved token amount and borrow amount,
with raw values, ledger and ID under advanced details. Restore only reveals the
reviewed request. A separate Continue action uses the same raw bigint arguments
and original ID through the existing V2 approval/dispatch/ACK adapter. It cannot
silently allocate a fresh request if the reviewed one was acknowledged elsewhere.
Completed and post-pull phases retain their existing recovery semantics.

Cancellation requires a separate confirmation. The new synchronous owner-only
`cancel_vault_operation({operation_id, request})` update atomically fences the
next ID with the existing acknowledged watermark. It accepts only no active row,
or the exact matching Prepared row with zero dispatch attempts, or the exact
matching proven SafeNoEffect row with one attempt; a wide positive receipt is
always held. Submitted, PullConfirmed, VaultCredited, Completed, mismatches, gaps
and uncertain outcomes cannot be cancelled. Existing terminal ACK rules remain
unchanged. A retired ID may have completed elsewhere: the UI says retired and
asks for refreshed balances/vaults, never claiming that watermark alone proves
no transfer. Cancellation does not revoke an existing allowance or refund funds.

The update has no external calls or awaits and deliberately does not acquire
GuardPrincipal: an original request may hold it while awaiting preflight. The
durable watermark is rechecked by reservation/dispatch, so a cancellation that
wins first prevents a delayed original request from pulling collateral. A
dispatch that wins first makes cancellation fail closed. Empty query status
alone is insufficient for this fence.

`get_vault_operation_status()` has **zero arguments and is caller-scoped**.
An operator's empty result is not evidence of the customer's phase. The browser
uses its actual authenticated owner/session actor, with checks around awaits.
No customer status or intent is inferred from an operator call.

## State and browser compatibility

- Reuses existing state fields. One ordered private cancellation event preserves
  the trusted retirement watermark on replay. Replay may advance to a cancelled
  ID despite earlier ordinary ACKs being snapshot-only; it does not reconstruct
  every ordinary ACK. Existing snapshots retain their existing ACK map semantics.
- Existing SEC11 origin/receipt maps and journal are unchanged and must survive
  all releases. No old writer, stale snapshot restore or dedup reset is authorized.
- Retains valid v1 browser intents. Typed canonical request equality ignores
  object field order; this is a robustness fix, not the proven screenshot cause.
  Malformed records stay held. A reviewed intent is bound to its exact stored
  snapshot, owner, ID and request; a changed record cannot be cleared or resumed.
- Uses the same Web Lock across actions. Lost cancellation replies reconcile
  once without a second send; no browser deletion occurs without authoritative
  retirement. Save/clear signals are delivered after releasing the lock.
- Oisy status/signing requests remain explicit gestures. Wallet/session changes
  invalidate pending UI actions. The new request clears the prior success notice;
  small screens use wrapped text and single-column saved-request actions.

## Focused evidence

- Journal regression suite: **38 passed**, native exit 0. Includes v1 parsing,
  field order, failed approval discovery, exact raw resume, cross-tab/storage and
  session changes, missing/failed status, unknown reply, positive phases and ACK.
- Rust `vault_pull_cancel` filter: **4 passed**, exit 0. Tests late preflight,
  cancellation/dispatch interleavings, exact binding/owner namespaces, safe phases,
  wide receipts, next ckSOL eligibility, snapshot and ordered replay.
- Canonical bin Candid check with `self_check` and regeneration: **1 passed**,
  exit 0; canonical declarations script and DID-copy comparison exit 0. Only the
  cancellation argument record and method are added to the public contract.
- Real ApiClient adapter plus existing standard ICRC-2 path filter: **18 passed**.
  Includes zero-argument status, caller-switch/unavailable-status holds, metadata
  retirement without approval/dispatch, and exact raw ckSOL/borrow continuation.
  Round-one independent reviews found one queue-session race: Continue captured
  its context only after waiting for the sequential-operation queue. Two real
  queue regressions first reproduced a successful stale mocked loan result
  after same-owner reconnect/provider round-trip. The fix captures the context
  synchronously before enqueueing and checks it before queued actor access.
  Both now prove zero actor/approval/dispatch/ACK calls and unchanged saved bytes.
  Backend, interface and UI evidence above is reused because those files did not
  change in this focused correction. Fresh independent review remains required
  for the corrected candidate before release.
- Mounted recovery components: **8 passed** (four existing Oisy consent/session,
  four saved-request/precision/confirmation/next-notice cases). No test moves funds.
- Svelte parsing succeeds. Project typecheck remains exit 1 with **30 errors,
  66 warnings**; none in changed source. The same 30 unchanged-file diagnostics
  match path/location/text using exact prepatch generated declarations and the
  same dependencies. Two existing readonly-wallet mock typing errors were fixed
  in the touched test. Canonical declarations were restored byte-for-byte after
  that diagnostic comparison. No clean project-wide typecheck is claimed.
- Real Chrome QA by Ellis on a temporary localhost fixture: desktop and 390x844
  passed. Restore leaves resume/cancel counters zero; explicit Continue increments
  resume once; cancellation needs confirmation; next ID clears stale notices;
  full-precision `0.03057162 ckSOL` fits with page width equal to viewport width.
  No framework overlay or application console error; one Phantom extension
  injection error is external to the app. Fixtures use the actual ckSOL ledger,
  9 decimals and synthetic raw values, never a claim about the customer's intent.

Browser fixture, screenshots and raw logs remain outside the repository. No new
dependency, team, worktree, broad suite, production build, deployment or customer
financial invocation was performed by this source lane. Dorian must build and
review exact new artifacts, preserve the live composition/state/assets, deploy
the additive backend before activating the new UI, and record served evidence.
The actual owner retry and case communication remain with Ellis.
