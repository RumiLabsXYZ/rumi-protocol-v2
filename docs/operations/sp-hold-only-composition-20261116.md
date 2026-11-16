# Hold-only source component for Dorian's XRP retry fix

This branch isolates **only preservation of the existing Stability Pool
new-liquidation hold** from Adrian's complete Vault 208 source. It is source
preparation, not tested or built release approval. No installation, payment,
claim settlement, compensation or resumption is authorized by this component.

## Identity and exact scope

- **Cherry-pick source commit: `68946c3419055960f5a7ae8d6910adad3b3ae2e7`.**
- Base: `9b7669e799d4e6e6cc6d52174c06e87e3569994c`.
- Full Vault 208 candidate remains independently pushed at
  `c2cbffc872b514034035b8c71d54474eca95c142`, branch
  `codex/vault208-xrp-partial-20261115`. **Do not cherry-pick that whole candidate
  into the retry release:** it includes the separate partial-liquidation cap
  and stale-quote admission changes.
- This component changes six source paths: backend `lib.rs`, `main.rs`,
  `storage.rs`; exactly three hold lines in `state.rs`; and the two integration
  fixtures that construct the public `UpgradeArg` directly.
- `vault.rs` is byte-identical to the base. No liquidation formula, dispatch,
  preflight cap, claim settlement, automatic/manual origin, fee, recipient,
  dependency or public Event-variant change is included.

The [source manifest](evidence/sp-hold-only-composition-20261116/source-manifest.json)
pins all six source-file hashes and four focused storage test names. The only
change beyond extraction is typing two test-only reopen defaults as `0u8`,
matching the persisted byte cell instead of Rust's unconstrained `i32` literal
fallback. Runtime hold behavior is identical to the extracted candidate.

## Hold semantics that must survive composition

`UpgradeArg.preserve_sp_writedown_hold: opt bool` is additive. The exact current
installation arguments must carry **`opt true`**, and the private hold overlay
runs immediately after snapshot restore/event replay, before migrations,
state replacement and timers. `Some(false)` is not resumption authority.

The private stable latch uses MemoryId11 (historical IDs9/10 are not reused):
0 unset,1 explicit false,2 true. The current true installation override persists
true; otherwise a previously stored latch overrides historical replay. With an
unset latch, the restored state initializes it. Only the existing authorized
controller setter explicitly changes the latch; it persists before changing
heap state. No new public Event variant is added, preserving the existing
variant API shape. The first migration's explicit true argument is essential
because the currently installed setter did not record its earlier hold in the
public event log.

**Keep the flag true before/after any later approved installation and through
verification. Rob's separate explicit approval is required to resume.** A build,
merge, retry-fix release, restart or successful payout does not authorize clearing
it. This hold controls new SP liquidations; it does not stop existing XRP payout
reconciliation, so it does not itself replace Dorian's cycle-waste correction.

## Composition check and current evidence

Dorian's draft advanced to committed `f0df15285cc4b51e83045ab6b0469a1bab4a44c6`
during preparation. The isolated patch applies cleanly to that exact commit.
`git apply --check` was read-only; its worktree/index/diff remained unchanged.
See [composition receipt](evidence/sp-hold-only-composition-20261116/composition.json)
and [exact six-file patch](evidence/sp-hold-only-composition-20261116/hold-only.patch).
The actual shared source path is `main.rs`, in different hunks: the hold adds
post-upgrade/setter handling while Dorian changes the automatic-settlement call.
This check is not permission to edit Dorian's worktree and must be repeated if
his candidate changes before integration.

Fresh read-only [live baseline](evidence/sp-hold-only-composition-20261116/live-baseline.json)
confirms the installed backend remains Running on
`59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111`, with the SP
write-down admission flag true. Base9b7669e7 has no backend/Cargo differences
from installed-source f72ede81. This source comparison does not prove the final
composed artifact installs or preserves state.

## Validation and handoff

Only source extraction, byte comparisons, `git diff --check`, and read-only
patch composition were run. **No Rust tests, Candid generation, CI, Wasm build,
installation or payout ran for this component.** The full Vault 208 candidate
also has no local runtime-result directories or PR; its earlier test/build
handoff remained blocked by automatic cross-chat review despite verified human
authorization. Do not promote either source-only status to a passing result. The isolated
component corrects two byte-typed test defaults from the full candidate; when
resuming the separate full Vault 208 work, carry those test corrections forward
rather than restoring its older untyped defaults.

Dorian should cherry-pick only this component's source commit onto the frozen
automatic-retry candidate, run the four hold tests together with his focused
origin/tuple-retention regressions under Cody's fresh allocation, regenerate
canonical Candid/declarations from the final Rust and verify compatibility,
then obtain one source-matched release build. Preserve the installed baseline
and existing producer artifacts. Present one exact combined retry+hold packet
for Rob's later deployment approval. No Vault 208 sizing or financial remedy
is silently bundled.
