# Base icUSD bridge: adversarial review findings and resolutions

Accepted-finding and resolution record for the canonical board (`base-icusd-bridge-kanban.md`).
Every review is local and read-only (source, unit tests, PocketIC, local anvil). Reviewers were
fresh Sonnet agents per round and did not see each other's findings. "Pin" is the source a
reviewer actually read and tested.

## Round 1

| Reviewer | Pin | Verdict |
| --- | --- | --- |
| A (accounting, authority, replay) | `2fee9970` plus the uncommitted follow-ups later committed as `19fbb0b5` | FAIL, 4 blockers |
| B (IC semantics, cycles, upgrade, wallet flow) | same | FAIL, 4 blockers |

Eight distinct blockers (two shared), all accepted and fixed in `6208c117`; listed in the design,
section 11a, "Review round 1 changes". The scenario suites then found two more defects (C3
reporting, unauthorized mint-then-burn payout), fixed in `6208c117` and `e4213be1` with
regression tests.

## Round 2

| Reviewer | Pin | Verdict |
| --- | --- | --- |
| A | `e4213be1` end to end, then the `779bd37f` diff, re-run on a clean tree at `779bd37f` | FAIL, 1 blocker (A-B1) |
| B | `e4213be1` | FAIL, 2 blockers (B-B1, B-B2) |

Reviewer A's own runs at `779bd37f` (clean tree, authoritative for that commit): lib 394,
`bridge_faults_pic` 16, `bridge_e2e_pic` 24, forge 111, vitest 1455, `npm run check` at baseline,
build ok, regenerated bindings byte-identical. Reviewer B ran lib 390 and vitest 341 at
`e4213be1` and did not run PocketIC, anvil or forge.

The coordinator's own PocketIC rerun (e2e 24, faults 16) built its Wasm about one minute after the
`779bd37f` commit; tree cleanliness at that moment was not recorded, so it is supporting evidence
only. The e4213be1 results (24 / 16) predate the round-2 fixes and do not validate `779bd37f` or
anything later.

### B-B1: one finalized burn could be released twice (accepted, fixed in `779bd37f`)

- Original failure: `process_releases` decided from a stale snapshot and `prepare_release_op`
  replaced an existing ledger tuple with a new one (new `created_at_time`, attempts reset), so two
  overlapping ticks could send two distinct tuples for one burn. Reviewer B's scratch test
  `double_prepare_overwrites_an_op_that_was_already_sent` failed with "a second distinct ledger
  tuple replaced the one already sent". Reviewer A independently confirmed the defect at `e4213be1`.
- Fix: the release is re-read per iteration; `prepare_release_op` acts only on `Pending` and never
  replaces an existing tuple; the tick guard carries an owner token. Unit test
  `preparing_twice_never_replaces_a_tuple_that_may_have_been_sent`.
- Independent re-check: reviewer A verified the fix at `779bd37f` by reading and unit test. No
  PocketIC test overlaps ticks.

### B-B2: admin endpoints could not revive a lost timer (accepted, fixed in `779bd37f`)

- Original failure: a due time left in the past by a lost timer task made every later
  `schedule_in` return early. Reviewer B's scratch test
  `admin_rearm_cannot_revive_a_lost_timer_whose_due_time_is_in_the_past` failed
  (armed equal to after, both in the past).
- Fix: a pending due more than 60 s in the past counts as lost and is re-armed.
- Independent re-check: reviewer A confirmed the 779bd37f change by reading; a trap after an await
  cannot be unit tested, so the evidence is source reading plus the `should_reschedule` tests.

### A-B1: the first mint after the contract mint window could run out of gas (accepted, fix uncommitted)

- Original failure: default `mint_gas_limit` 150,000. Reviewer A, on anvil with the repo's
  artifact: the first mint after the initial mint window, to a recipient with 20 non-zero bytes,
  estimates 152,342; sent with a 150,000 limit it mined with status 0 and gasUsed 150,000. The
  coordinator reproduced 152,390 (first mint ever, new window, maximal amount) and observed the
  anvil out-of-gas reply `-32603 "EVM error OutOfGas"`, which the old code treated as a failed
  provider. The preflight `eth_call` sent no gas field, so it could not catch this.
- Fix (uncommitted, in the round-2 follow-ups): default 300,000, floor 250,000, admin floor
  80,000, cap 1,000,000; the preflight simulation carries the exact gas limit; an out-of-gas answer
  is a preflight hold (no nonce, nothing signed, retried automatically); the transaction is created
  only if the configured limit still equals the simulated one.
- Regression proof: unit tests in `rpc.rs`, `lane.rs`, `admin.rs`; a PocketIC test of the
  worst-case first mint with a gas-margin assertion is being written (BB-16).

### Reviewer notes taken in the same round (bounded to the stated failure cases)

| Note | Disposition |
| --- | --- |
| A: no `inspect_message` (mainnet gate) | Fixed (uncommitted): ingress filter by method class; in-method checks unchanged. |
| A: re-mints could delay user deposits on the shared mint lane (mainnet gate) | Fixed (uncommitted): user deposits first, at most 2 re-mints in flight. |
| A: gas starvation invisible in the UI (mainnet gate) | Fixed (uncommitted): page notice and "waiting for bridge gas" status from `lanes[].awaiting_gas`. |
| A: no EIP-55 recipient check | Fixed (uncommitted) in the canister (`sha3` crate) and the page (local Keccak-256 pinned to `cast keccak` vectors). |
| A: lane run guard drop not owner-checked | Fixed (uncommitted): owner token, unit test. |
| A: ICRC-3 absence proof applied to an op that changed during the search | Fixed (uncommitted): applied only if the op is unchanged and still searchable. |
| A: a long burn deferral is invisible | Fixed (uncommitted): `deferred_since_ns` / `deferred_reason` in `get_bridge_info`, audited at start and at most every 30 min. |
| A: `admin_set_rpc_services` reads the caller after an await | Fixed (uncommitted): controller captured before the await. |
| A: e2e no-approval test accepts both outcomes; harness tick expects Ok | Being fixed (BB-16 test work). |
| A: Escrowed with mints paused keeps the fast timer | Fixed (uncommitted): counts only when a mint can be signed. |
| A: admission count decodes all active deposits | Fixed (uncommitted): owner index walk. |
| A: design drift (7.8 lane halt vs global halt, 7.10 method name) | Fixed (uncommitted). |
| A: windows consumable by one actor; parked-lane recovery needs an upgrade; finalized-head block cap not adaptive; throughput under load; ledger metadata not verified at init | Documented as risks or runbook checks; no code change. |
| B: held states keep the fast timer; `admin_run_tick` silently no-ops; doc drift (5.1 RPC authority, 7.9 audit log, hold copy) | Fixed in `779bd37f` and its docs commits. |
| B: spam economics, admission shared with re-mints, release backoff, head-of-line, soft release cap, ECDSA fee, supply reporting | Production configuration or documented risks; the backoff case was addressed by the typed deferral in `779bd37f`. |

A-B1 resolution evidence (frozen candidate `eab65670`, Wasm sha256 prefix `de4ad399`): the
PocketIC test `first_mint_after_the_contract_window_elapsed_to_a_dense_recipient_succeeds_within_gas_margin`
mints the first token ever after the contract window to a 20-non-zero-byte recipient: status 1,
gasUsed 152,342 of 300,000, every preflight `eth_call` carries the limit. The out-of-gas hold path
is covered with a mocked provider reply (`out_of_gas_mint_simulation_holds_the_deposit_without_a_transaction_then_retries`).
All 7 new round-2 PocketIC tests fail on the pre-round-2 Wasm.

## Round 3

Running on the frozen candidate `eab65670` (Wasm `de4ad399`) with two fresh reviewers. Scope is
bounded: does each round-2 fix close its original failure case, and did the round-2 changes
introduce regressions. Reviewer A: canister-side fixes. Reviewer B: caller paths, interfaces,
wallet flow, artifacts and test honesty.
