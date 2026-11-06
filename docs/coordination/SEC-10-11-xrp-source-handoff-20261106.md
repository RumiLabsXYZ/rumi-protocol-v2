# SEC-10 / SEC-11 source handoff — November 6, 2026

Ellis owns the unified customer case; Miles owns the XRP lane; Bram owns the
existing Security & Recovery board/publisher. This is a source handoff, not a
second board or a release authorization. Canonical ownership/acceptance records
are `rumi-security-board.md` and SEC-10/11 in `rumi-security-lookout.json` on
`codex/cksol-release-decision-20261104` (ownership checkpoint `5cc6ee90`).
No customer balances, identities or transaction payloads belong in this file.

Source baseline: `0dea74a1d33456ee28a9837b99a3be9dae7f674f`.
Branch: `codex/xrp-customer-withdrawal-address-20261106`.
Primary checkout, other teams and the denied XRPL follow-up branch are protected.
No merge, deployment, live refresh, signing, funds movement or customer-message
send is authorized by this source task.

## SEC-10 acceptance

- Exact-zero-debt native-XRP withdrawals need no oracle. Revalidate current
  ownership, debt, collateral/status and processing/claim constraints at mutation;
  preserve anonymous/frozen and existing per-vault exclusion checks. Positive
  debt, including dust, retains freshness and collateral-ratio protections.
- Do not apply the native-XRP activation/deposit minimum as a withdrawal minimum.
  Preserve positive amount, available collateral, settlement fees and claim safety;
  leave unrelated collateral behavior intact.
- Ellis accepted a native-XRP-only effective background interval cap of 300s,
  preserving configured shorter intervals. The scheduler and reported effective
  interval must agree. Preserve existing single-flight/bar admission, the strict
  600s maximum sample age, source-count/sanity checks and current-state validation.
  Do not add immediate paid retries or relabel stale prices as fresh.
- At configured 900s, scheduled upper rate changes from 4 to 12 calls/hour per
  eligible feed, before admission/provider effects. Verify the existing 1B-cycle
  call cost in source. This is bounded proposed source cost, not measured spend.
- Provider failures remain explicitly retryable/unavailable; the cadence change
  cannot guarantee provider availability. Current debt/health may independently
  constrain withdrawal. A historical zero-debt screenshot is not current proof.

## SEC-11 acceptance and accounting boundary

The current confirm removes the pending address entry and frontend cache;
`XrpVaultPanel` lists pending deposits only. Generic native-XRP top-ups are
rejected. The developer-only address-derivation endpoint is not a user API.

An existing-owner address view must use the original custody derivation, bind the
current wallet/vault and survive reload. A repeat-credit action additionally
needs proven new net balance excluding reserves, collateral already recorded,
and all unresolved or outgoing claims. Repeated/stale observations and callbacks
must never recredit a paid claim; concurrent settlement/liquidation must serialize
or invalidate the observation. Stop before advertising a reusable receive address
if those invariants require an unbounded accounting change. Report the missing
primitive/design explicitly rather than inventing a stable-state migration.

## Validation / execution

One serial local-test lane only; offline Cargo jobs=1 using existing cache,
targeted frontend tests, no installation, broad build, Wasm/PocketIC or paid CI.
Use canonical Candid/declaration generation if a public interface actually changes.
Focused review must assess custody/auth/await semantics before source acceptance.

Attempt 1: connector returned Sonnet 5.5, plan mode/file reader only; no source
inspection, edits or tests. Local Claude Code CLI reports signed out. Desktop
navigation did not reach Nora; automatic review rejected raising an unrelated
private window. No retry through another UI route. Executor substitution awaits
Ellis's decision. No code/tests complete at this checkpoint.

Ellis subsequently approved reuse of existing Codex worker
`/root/xrpl_final_source_0b91` as the explicit execution fallback, without model
escalation or a new team. Worker dispatched with this same scope and limits;
coordinator review remains separate. The rejected unrelated-window route remains
stopped. Source work is in progress; no passing checks are claimed yet.

## Codex fallback attempt — 2026-11-06

Ellis authorized reuse of the existing Codex implementation worker after the
Sonnet connector/CLI limitation above. No Sonnet API call or model override was
made; the exact served backend model identifier is not exposed to the worker.
Attempt began 08:47:00 UTC, with hard stop 09:02:00 UTC. Only the assigned source
worktree was edited; existing untracked node_modules symlinks were preserved.

SEC-10 local source changes:

- Three withdrawal wrappers skip oracle calls only for the current owner's
  exactly-zero-debt native-XRP vault. Anonymous/frozen checks remain. Positive
  debt (including dust) retains oracle gates; full native withdrawal cannot use
  dust forgiveness to clear debt. Non-native collateral keeps its existing path.
- Native partial withdrawal admits against current state in the claim-creation
  block: owner, freeze, status, available amount and existing processing/claim
  exclusions are rechecked. Exact zero debt does not read a price. Positive debt
  requires a valid sample no older than 600s and CR headroom, including Recovery
  mode. The deposit activation minimum does not apply; zero remains rejected.
- Native-XRP background cadence alone is capped at an effective 300s, preserving
  shorter intervals and the existing 60s floor. Actual scheduler registration and
  operator reporting share the same resolver. Existing lease/bar admission,
  120s soft reuse, source-count/sanity checks and strict sample age are untouched.
- Configured 900s becomes an upper scheduled 12 calls/hour instead of 4 per
  eligible feed, before admission/provider effects. Existing XRC payment remains
  1,000,000,000 cycles per call (`management.rs` XRC-based branch). This is source
  arithmetic, not measured spend or guaranteed provider availability.

Claim fees are unchanged: `xrp_claim_send_amount` requires `drops > fee` at
settlement and sends `drops - fee`. No arbitrary new withdrawal minimum was
invented. A tiny positive claim may therefore remain unpayable at the current
fee; the 0.01 XRP regression verifies removal of the old 1 XRP activation floor,
not a guarantee of future fees or live settlement.

Validation (one serial executor, existing shared target, no install/network/CI):

```sh
CARGO_TARGET_DIR=/Users/robertripley/coding/rumi-protocol-v2/target cargo test --offline --locked -p rumi_protocol_backend --lib sec10 --jobs 1 -- --test-threads=1
CARGO_TARGET_DIR=/Users/robertripley/coding/rumi-protocol-v2/target cargo check --offline --locked -p rumi_protocol_backend --bin rumi_protocol_backend --jobs 1
```

- Initial focused library tests: native exit 0, 5 passed, 0 failed, 86.31s elapsed.
- Coordinator-requested fixed-vector rerun: native exit 0, 6 passed, 0 failed,
  20.08s elapsed. It pins 2 XRP at $1 / 1 icUSD debt / 150% CR:
  500,000 drops allowed, 500,001 rejected; future timestamp rejected. Recovery
  at 180% allows 200,000 drops and rejects 200,001. Production code unchanged.
- Changed binary entrypoint check: native exit 0, 22.07s elapsed.
- Cumulative observed free-space drop: 1,087,959,040 bytes, below 2 GiB. Both
  resource watchers completed normally. No test process remains.
- Only new Rust blocks were formatted between the semantic test run and binary
  check. No production semantic change followed the successful first test run; the
  later change added only the coordinator-requested fixed-vector regression.
- No public interface/state schema change, so Candid/declarations are unchanged.
  No Wasm, PocketIC, broad suite, frontend tests, live provider/customer validation,
  release or financial action was performed. Independent Miles review is separate.

SEC-11 is **deferred, not complete**, per Ellis's accepted disposition. No reusable
address UI/API or repeat-credit action was added. The precise missing durable
outflow/frontier primitive and historical reconciliation boundary are recorded in
[the SEC-11 deferred design](SEC-11-xrp-repeat-deposit-design-20261106.md).
It is not a SEC-10 shipping gate. The coordinator retains existing board/ledger
ownership and may force-add these normally ignored local handoff documents.

## Miles coordinator source review

### Integration clarification: interest and native-unit rounding

Dorian's normal integration review confirmed that partial withdrawal calls
`accrue_single_vault` before the new native-XRP admission/claim block.
`State::accrue_single_vault` includes the newly accrued interest in
`borrowed_icusd_amount` and tracks that increment in `accrued_interest`; the debt
accrual uses `ceil` at icUSD-e8 precision. The separate accrued-interest field is
therefore not an additional debt amount to add again. Exactly zero recorded debt
does not accrue interest. Any positive debt, including one e8, keeps the oracle
and residual-collateral checks and cannot use native-XRP dust forgiveness.

The residual-collateral calculation deliberately retains the existing numeric
semantics: `Token * Ratio` narrows to integer e8 units, and
`icusd_to_collateral_amount` narrows to native collateral units by truncation.
The native-XRP check then retains at least one raw unit for positive debt via
`max(1)`. This is not a new conservative ceiling-rounding policy: fractional
thresholds can truncate below the mathematical ratio by native-unit precision.
The focused vectors pin exactly representable normal/Recovery boundaries, not
every fractional rounding case. Any broader change to the inherited rounding
policy is separate from this source fix, not a new integration gate.

Current main was refreshed and remains the candidate base
`0dea74a1d33456ee28a9837b99a3be9dae7f674f`; no source conflict or production-code
change was needed for integration. The six focused tests and offline binary
check remain evidence for the unchanged three Rust blobs. SEC-11 stays deferred.

Reviewed the actual frozen three-file diff independently of its implementer and
read native-exit JSON/log receipts. Accepted this bounded source/test outcome:
only owned exact-zero NativeXrp skips oracle work, with no suspension before
current-state native claim admission; positive debt retains strict freshness
and normal/Recovery CR checks. Existing claim fees and custody accounting remain
unchanged. Scheduler registration and operator getter both use the changed
cadence resolver. Non-native routing and oracle admission/publication code are
unchanged. Requested and verified the additional normal/Recovery fixed vectors
and future-time rejection before acceptance. Source design links checked against
actual function locations. No whole-system, Wasm, deployment, customer withdrawal
or provider availability acceptance is implied. No additional broad test/review
round was required for this demo-first bounded source handoff.

Final three-file source diff SHA256:
`0f9a1d8bc99faac7f9b8fcbdb3b9ca696c8348332e410e01dcadab1034f6cb15`.
SEC-11 stays deferred/not complete and must be revisited before receiving further
customer deposits through a reusable address flow. This does not block SEC-10
source integration. Tiny claims still need to cover the actual settlement fee.
